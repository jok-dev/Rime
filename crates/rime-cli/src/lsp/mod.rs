//! `rime lsp`: Language Server over stdio. Handlers are thin —
//! they turn LSP notifications into `worker::Event`s; the worker thread
//! owns every piece of state and answers through `Publish`es that a
//! publisher task forwards to the client.

pub mod convert;
pub mod text;
pub mod worker;

use convert::quick_fixes;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, RwLock};
use std::time::Duration;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer, LspService, Server};
use worker::{Event, Outbound};

/// What code actions read: the last publish per canonical PATH. Never keyed
/// by `Uri` — `Uri` equality is `as_str()` equality, and the same file has
/// two spellings in play: VS Code sends `file:///c%3A/…` while the server's
/// own `Uri::from_file_path` capitalizes and percent-encodes the drive
/// (`file:///C%3A/…`). Keyed by URI, a startup publish (server spelling)
/// followed by a `did_open` (client spelling) leaves `code_action` looking
/// at a key that is not there — every quick fix for a pre-existing
/// diagnostic silently dead, plus a second entry per file forever.
type Published =
    Arc<RwLock<HashMap<PathBuf, (text::LineIndex, Vec<(rime_diag::Diagnostic, Diagnostic)>)>>>;

struct Backend {
    client: Client,
    events: Mutex<Option<mpsc::Sender<Event>>>,
    /// Set by `initialize`; the worker starts in `initialized`.
    init: Mutex<Option<(Option<PathBuf>, Duration)>>,
    published: Published,
    /// Set once the event channel is closed (the worker thread is gone), so
    /// the "worker has died" error is logged exactly once.
    worker_dead: AtomicBool,
}

fn path_of(uri: &Uri) -> Option<PathBuf> {
    let p = uri.to_file_path()?;
    p.canonicalize().ok()
}

impl Backend {
    /// Push an event to the worker. A closed channel means the worker
    /// thread is gone (panicked, or returned): say so once, loudly —
    /// silently dropping events leaves a server that answers every request
    /// with stale state and never publishes again.
    async fn send(&self, ev: Event) {
        let dead = match self.events.lock().unwrap().as_ref() {
            Some(tx) => tx.send(ev).is_err(),
            // Before `initialized` there is no worker yet — not a death.
            None => false,
        };
        if dead && !self.worker_dead.swap(true, Ordering::SeqCst) {
            self.client
                .log_message(
                    MessageType::ERROR,
                    "rime: worker thread has died — restart the server \
                     (Rime: Restart Language Server)",
                )
                .await;
        }
    }

    /// `path_of`, but on failure logs a warning to the client instead of
    /// silently dropping the event (on failure the event is
    /// dropped and logged).
    async fn resolve(&self, uri: &Uri) -> Option<PathBuf> {
        let path = path_of(uri);
        if path.is_none() {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("rime: ignoring {} — not a canonicalizable file path", uri.as_str()),
                )
                .await;
        }
        path
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // `root_uri` is deprecated in favour of `workspace_folders`, but it's
        // still the only field some clients (including our test harness)
        // populate — keep it as the fallback.
        #[allow(deprecated)]
        let root_uri = params.root_uri;
        let root = params
            .workspace_folders
            .as_ref()
            .and_then(|f| f.first())
            .map(|f| f.uri.clone())
            .or(root_uri)
            .and_then(|u| path_of(&u));
        let debounce = params
            .initialization_options
            .as_ref()
            .and_then(|v| v.get("debounceMs"))
            .and_then(|v| v.as_u64())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(100));
        *self.init.lock().unwrap() = Some((root, debounce));
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
                definition_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
                    code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                    ..Default::default()
                })),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: vec!["rime.recheck".to_string()],
                    ..Default::default()
                }),
                ..Default::default()
            },
            server_info: Some(ServerInfo { name: "rime".into(), version: Some(env!("CARGO_PKG_VERSION").into()) }),
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let (root, debounce) = self.init.lock().unwrap().take().unwrap_or((None, Duration::from_millis(100)));
        let (ev_tx, ev_rx) = mpsc::channel::<Event>();
        let (out_tx, out_rx) = mpsc::channel::<Outbound>();
        *self.events.lock().unwrap() = Some(ev_tx.clone());
        // A panicking worker would otherwise die in silence: the thread is
        // detached, the default hook prints to a stderr nobody reads, and
        // the client sees a server that simply stopped answering. Catch it
        // and say so through the publisher, whose channel is kept alive by
        // this clone for exactly that message.
        let panic_tx = out_tx.clone();
        std::thread::Builder::new()
            .name("rime-worker".into())
            // The worker parses and checks on its own stack as well as on
            // the rayon pool's; see `WORK_STACK_BYTES`.
            .stack_size(crate::work_stack_bytes())
            .spawn(move || {
                let run = std::panic::AssertUnwindSafe(|| worker::run(root, debounce, ev_rx, out_tx));
                if let Err(e) = std::panic::catch_unwind(run) {
                    let msg = e
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_string())
                        .or_else(|| e.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".to_string());
                    let _ = panic_tx.send(Outbound::Log(format!("rime: worker panicked: {msg}")));
                }
            })
            .expect("spawn worker");
        // Publisher: blocking receive on a dedicated thread, async send.
        let client = self.client.clone();
        let published = self.published.clone();
        tokio::task::spawn_blocking(move || {
            let rt = tokio::runtime::Handle::current();
            while let Ok(msg) = out_rx.recv() {
                match msg {
                    Outbound::Publish(p) => {
                        // An empty publish is a clear: drop the entry rather
                        // than parking a `LineIndex` + empty vec per file
                        // for the life of the process.
                        if p.lsp.is_empty() {
                            published.write().unwrap().remove(&p.path);
                        } else {
                            let pairs = p.raw.iter().cloned().zip(p.lsp.iter().cloned()).collect();
                            published.write().unwrap().insert(p.path, (p.index, pairs));
                        }
                        rt.block_on(client.publish_diagnostics(p.uri, p.lsp, None));
                    }
                    Outbound::Log(line) => {
                        rt.block_on(client.log_message(MessageType::INFO, line));
                    }
                }
            }
        });
        let watchers = vec!["**/*.{lua,luau}", "**/rime.toml", "**/*.project.{json,jsonc}", "**/.luaurc"]
            .into_iter()
            .map(|g| FileSystemWatcher { glob_pattern: GlobPattern::String(g.to_string()), kind: None })
            .collect();
        let opts = DidChangeWatchedFilesRegistrationOptions { watchers };
        if let Err(e) = self
            .client
            .register_capability(vec![Registration {
                id: "rime-watched-files".into(),
                method: "workspace/didChangeWatchedFiles".into(),
                register_options: Some(serde_json::to_value(opts).unwrap()),
            }])
            .await
        {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("rime: could not register file watchers: {e} — external changes will not be picked up"),
                )
                .await;
        }
        self.send(Event::Recheck).await;
    }

    async fn shutdown(&self) -> Result<()> {
        self.send(Event::Shutdown).await;
        Ok(())
    }

    async fn did_open(&self, p: DidOpenTextDocumentParams) {
        if let Some(path) = self.resolve(&p.text_document.uri).await {
            self.send(Event::Open { path, uri: p.text_document.uri, text: p.text_document.text }).await;
        }
    }

    async fn did_change(&self, p: DidChangeTextDocumentParams) {
        // Full sync: the last content change carries the whole text.
        let Some(change) = p.content_changes.into_iter().last() else { return };
        if let Some(path) = self.resolve(&p.text_document.uri).await {
            self.send(Event::Change { path, text: change.text }).await;
        }
    }

    async fn did_save(&self, p: DidSaveTextDocumentParams) {
        // Content no-op: the buffer already matched, so there is
        // no `Event` for this — but the save itself is still logged.
        self.client
            .log_message(MessageType::INFO, format!("rime: saved {}", p.text_document.uri.as_str()))
            .await;
    }

    async fn did_close(&self, p: DidCloseTextDocumentParams) {
        if let Some(path) = self.resolve(&p.text_document.uri).await {
            self.send(Event::Close { path }).await;
        }
    }

    async fn did_change_watched_files(&self, p: DidChangeWatchedFilesParams) {
        for c in p.changes {
            // Deleted files cannot canonicalize: use the plain path, but
            // still log (like `resolve`) when even that fails.
            let path = if c.typ == FileChangeType::DELETED {
                match c.uri.to_file_path() {
                    Some(p) => Some(p.into_owned()),
                    None => {
                        self.client
                            .log_message(
                                MessageType::WARNING,
                                format!(
                                    "rime: ignoring {} — not a canonicalizable file path",
                                    c.uri.as_str()
                                ),
                            )
                            .await;
                        None
                    }
                }
            } else {
                self.resolve(&c.uri).await
            };
            if let Some(path) = path {
                self.send(Event::Watched { path, kind: c.typ }).await;
            }
        }
    }

    async fn execute_command(&self, p: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        if p.command == "rime.recheck" {
            self.send(Event::Recheck).await;
        }
        Ok(None)
    }

    async fn goto_definition(
        &self,
        p: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let tdp = p.text_document_position_params;
        let Some(path) = self.resolve(&tdp.text_document.uri).await else { return Ok(None) };
        // Ask the worker; it owns the session and answers off the debounce
        // path so the click is not delayed. A dropped reply (worker gone)
        // reads as "no definition" rather than an error.
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.send(Event::Definition { path, pos: tdp.position, reply }).await;
        match rx.await {
            Ok(Some(loc)) => Ok(Some(GotoDefinitionResponse::Scalar(loc))),
            _ => Ok(None),
        }
    }

    async fn hover(&self, p: HoverParams) -> Result<Option<Hover>> {
        let tdp = p.text_document_position_params;
        let Some(path) = self.resolve(&tdp.text_document.uri).await else { return Ok(None) };
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.send(Event::Hover { path, pos: tdp.position, reply }).await;
        Ok(rx.await.ok().flatten())
    }

    async fn code_action(&self, p: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        // Resolve to the canonical path before the lookup — the inbound URI
        // is the client's spelling and never matches the published state by
        // string. The client's own `Uri` still goes back out in the edit, so
        // the `WorkspaceEdit` names the document the client knows.
        let Some(path) = self.resolve(&p.text_document.uri).await else { return Ok(None) };
        let guard = self.published.read().unwrap();
        let Some((index, pairs)) = guard.get(&path) else { return Ok(None) };
        let actions = quick_fixes(&p.text_document.uri, index, pairs, p.range);
        Ok((!actions.is_empty()).then_some(actions))
    }
}

pub fn run() -> std::process::ExitCode {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio");
    rt.block_on(async {
        let (service, socket) = LspService::new(|client| Backend {
            client,
            events: Mutex::new(None),
            init: Mutex::new(None),
            published: Arc::new(RwLock::new(HashMap::new())),
            worker_dead: AtomicBool::new(false),
        });
        Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service).await;
    });
    std::process::ExitCode::SUCCESS
}
