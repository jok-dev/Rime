//! The one thread that owns the Session. Handlers push
//! `Event`s; this loop debounces them into a `Batch`, applies it, and hands
//! `Outbound` messages (each file's `Publish`, then one `Log` line per
//! batch) to the async side. Never touches the LSP client directly.

use super::convert::to_lsp;
use super::text::LineIndex;
use crate::config::Config;
use crate::workspace::Workspace;
use rime_check::session::Session;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tower_lsp_server::ls_types::{
    FileChangeType, Hover, HoverContents, Location, MarkupContent, MarkupKind, Position, Uri,
};

pub enum Event {
    Open { path: PathBuf, uri: Uri, text: String },
    Change { path: PathBuf, text: String },
    Close { path: PathBuf },
    Watched { path: PathBuf, kind: FileChangeType },
    Recheck,
    /// Go-to-definition: answered synchronously against current
    /// state and NOT batched, so a Ctrl+click never waits out the debounce.
    /// The reply is `None` when nothing at that position resolves.
    Definition { path: PathBuf, pos: Position, reply: tokio::sync::oneshot::Sender<Option<Location>> },
    /// Hover documentation: answered the same way as
    /// `Definition` — inline, off the debounce path.
    Hover { path: PathBuf, pos: Position, reply: tokio::sync::oneshot::Sender<Option<Hover>> },
    Shutdown,
}

pub struct Publish {
    /// Canonical path — how the publisher keys its state. The `uri` is the
    /// client's own spelling and is for the wire only.
    pub path: PathBuf,
    pub uri: Uri,
    pub index: LineIndex,
    pub raw: Vec<rime_diag::Diagnostic>,
    pub lsp: Vec<tower_lsp_server::ls_types::Diagnostic>,
}

/// What the worker hands the async publisher: one `Publish` per changed
/// file, then exactly one `Log` per batch — never attached to
/// a `Publish` itself, so it can never be silently dropped by a `Publish`
/// that `publish_for` decides not to send.
pub enum Outbound {
    Publish(Publish),
    Log(String),
}

/// One path's queued state for the batch being built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    /// The client's own buffer content (`Open`/`Change`). Always wins over
    /// whatever was queued before, including an earlier `Reread`.
    Text(String),
    /// Re-read from disk at apply time: `Close` (always forces this,
    /// overriding an in-batch `Text`), or the first `Watched{CHANGED}` for
    /// a path with nothing queued yet in this batch.
    Reread,
}

#[derive(Default)]
pub struct Batch {
    pub full: bool,
    /// Per path, the latest queued state — see `Pending`.
    pub texts: Vec<(PathBuf, Pending)>,
    pub opened: Vec<(PathBuf, Uri)>,
    pub closed: Vec<PathBuf>,
}

/// A file whose change means the workspace has to be loaded again:
/// rime.toml, any Rojo project file, and `.luaurc` (string-require
/// aliases, which the resolver caches).
fn is_config(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n == "rime.toml" || n == ".luaurc" || rime_roblox::is_project_file_name(n))
}

impl Batch {
    fn set(&mut self, path: PathBuf, pending: Pending) {
        match self.texts.iter_mut().find(|(p, _)| *p == path) {
            Some(slot) => slot.1 = pending,
            None => self.texts.push((path, pending)),
        }
    }

    /// Returns false when the event is `Shutdown`.
    pub fn push(&mut self, ev: Event) -> bool {
        match ev {
            Event::Open { path, uri, text } => {
                self.opened.push((path.clone(), uri));
                // Close-then-reopen inside one batch: the reopened buffer is
                // the truth again, so the close must not survive to strip it
                // from `st.open` (and force a disk re-read) at apply time.
                self.closed.retain(|p| *p != path);
                self.set(path, Pending::Text(text));
            }
            Event::Change { path, text } => self.set(path, Pending::Text(text)),
            Event::Close { path } => {
                self.closed.push(path.clone());
                self.set(path, Pending::Reread);
            }
            Event::Watched { path, kind } => {
                if is_config(&path) || kind != FileChangeType::CHANGED {
                    self.full = true;
                } else if !self.texts.iter().any(|(p, _)| *p == path) {
                    // Nothing queued yet for this path: a bare disk-change
                    // marker. If a `Text` (or another `Reread`) is already
                    // queued, this adds nothing — a `Change` in the same
                    // batch always wins, and a redundant `Watched` echo of
                    // the client's own save is a no-op either way.
                    self.texts.push((path, Pending::Reread));
                }
            }
            Event::Recheck => self.full = true,
            // Queries are answered inline by `run` and never reach a batch;
            // these arms keep `push` total.
            Event::Definition { .. } | Event::Hover { .. } => {}
            Event::Shutdown => return false,
        }
        true
    }
}

struct State {
    root: Option<PathBuf>,
    session: Option<Session<'static>>,
    config: Config,
    /// The workspace's own std name set (rime.toml's `std`), refreshed on
    /// every reload. Only a server with no workspace at all keeps the
    /// bundled default — `extra_check` must not silently check a file
    /// against a different std than the session does.
    std: rime_std::StdNames,
    /// The last `Workspace::load` failed (bad rime.toml, unreadable root).
    /// Retried only on an explicit recheck or a config event — never on
    /// every keystroke.
    load_failed: bool,
    /// Which Rojo project file the last load read, or that cross-file
    /// checking is off and why (`ProjectChoice::line`). Part of the log
    /// line a reload sends: the editor's output channel is the one place
    /// a daemon can say it.
    project_line: String,
    requested: HashSet<PathBuf>,
    /// Open documents: buffer text is the truth until Close.
    open: HashMap<PathBuf, String>,
    uris: HashMap<PathBuf, Uri>,
    /// Paths whose report is currently non-empty on the client, so a
    /// vanished report gets an explicit clear. Updated for BOTH
    /// session-backed and `extra` (outside-root) publishes — see `finish`.
    published: HashSet<PathBuf>,
    /// Publishes for files outside the project root (or, in no-project
    /// mode, every file), checked alone (`State::extra_check`) since they
    /// never enter the session.
    extra: HashMap<PathBuf, Publish>,
}

impl State {
    fn new(root: Option<PathBuf>) -> Self {
        State {
            root,
            session: None,
            config: Config::default(),
            std: rime_std::StdNames::bundled().clone(),
            load_failed: false,
            project_line: String::new(),
            requested: HashSet::new(),
            open: HashMap::new(),
            uris: HashMap::new(),
            published: HashSet::new(),
            extra: HashMap::new(),
        }
    }
}

pub fn run(root: Option<PathBuf>, debounce: Duration, rx: mpsc::Receiver<Event>, tx: mpsc::Sender<Outbound>) {
    let mut st = State::new(root);
    loop {
        // Block for the first batchable event, answering any definition
        // queries that arrive while idle.
        let first = loop {
            match rx.recv() {
                Ok(Event::Definition { path, pos, reply }) => {
                    let _ = reply.send(st.answer_definition(&path, pos));
                }
                Ok(Event::Hover { path, pos, reply }) => {
                    let _ = reply.send(st.answer_hover(&path, pos));
                }
                Ok(ev) => break ev,
                Err(_) => return,
            }
        };
        let mut batch = Batch::default();
        if !batch.push(first) {
            return;
        }
        loop {
            match rx.recv_timeout(debounce) {
                // A query mid-batch is answered against current (pre-batch)
                // state — at most `debounce` stale, and a settled Ctrl+click
                // is not racing its own keystrokes.
                Ok(Event::Definition { path, pos, reply }) => {
                    let _ = reply.send(st.answer_definition(&path, pos));
                }
                Ok(Event::Hover { path, pos, reply }) => {
                    let _ = reply.send(st.answer_hover(&path, pos));
                }
                Ok(ev) => {
                    if !batch.push(ev) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        apply(&mut st, batch, &tx);
    }
}

fn apply(st: &mut State, batch: Batch, tx: &mpsc::Sender<Outbound>) {
    // Unpack up front: `apply_incremental` needs `texts` and `closed` by
    // themselves (not the whole `Batch`), and moving `opened` out below
    // would otherwise leave `batch` only partially movable.
    let Batch { full, texts, opened, closed } = batch;
    for (path, uri) in opened {
        st.uris.insert(path, uri);
    }
    for path in &closed {
        st.open.remove(path);
    }
    for (path, pending) in &texts {
        if let Pending::Text(t) = pending {
            if !closed.contains(path) {
                st.open.insert(path.clone(), t.clone());
            }
        }
    }
    let started = Instant::now();
    // No workspace folder at all: stay idle on the project side and check
    // only open documents, one file at a time — never take the
    // `full_reload` branch, which has nothing to reload. A previous load
    // failure is retried only for `full` batches (a recheck or a config
    // event), never for the keystroke that follows it.
    let want_reload = st.root.is_some() && (full || (st.session.is_none() && !st.load_failed));
    let (changed, reloaded): (Vec<PathBuf>, bool) = if want_reload {
        (full_reload(st, tx), true)
    } else {
        apply_incremental(st, texts, &closed, tx)
    };
    let mut changed: Vec<PathBuf> = changed.into_iter().collect::<HashSet<_>>().into_iter().collect();
    changed.sort();
    let mut n = 0usize;
    for path in changed {
        if let Some(p) = st.publish_for(&path) {
            n += 1;
            let _ = tx.send(Outbound::Publish(p));
        }
    }
    // One log line per batch, counted after dedup and after `publish_for`
    // filtering — sent unconditionally so it can never be silently dropped
    // by a publish that `publish_for` decided to skip. A reload reports the
    // whole picture instead (the startup line); a failed load has
    // already said so through its own error line.
    let line = if reloaded && !st.load_failed {
        let (files, with_diags) = st.totals();
        format!(
            "rime: checked {files} files, {with_diags} with diagnostics, in {}ms; {}",
            started.elapsed().as_millis(),
            st.project_line
        )
    } else {
        format!("rime: {n} file(s) republished in {:.0?}", started.elapsed())
    };
    let _ = tx.send(Outbound::Log(line));
}

/// Per-path processing for a batch that doesn't need a full reload: either
/// the root has a warm session already, or there is no project root at all
/// (single-file/no-project mode), in which case every path
/// falls through to `extra_check`. Never holds `st.session` borrowed
/// across a `full_reload`/`extra_check`/`forget` call (all `&mut State`):
/// classification and application are two separate passes.
/// Returns the changed paths and whether it ended up doing a full reload.
fn apply_incremental(
    st: &mut State,
    texts: Vec<(PathBuf, Pending)>,
    closed: &[PathBuf],
    tx: &mpsc::Sender<Outbound>,
) -> (Vec<PathBuf>, bool) {
    let mut resolved: Vec<(PathBuf, String)> = Vec::new();
    let mut changed: Vec<PathBuf> = Vec::new();
    let mut need_full_reload = false;
    for (path, pending) in texts {
        // A `Reread` for a path the client still has open, and isn't
        // closing this batch, needs no work: the open buffer is already
        // the truth, and nothing (no `Change`) told us otherwise — most
        // likely a `Watched{CHANGED}` echo of the client's own save.
        // Skipping it here avoids a redundant re-check.
        let still_open = st.open.contains_key(&path) && !closed.contains(&path);
        if pending == Pending::Reread && still_open {
            continue;
        }
        let text = match pending {
            Pending::Text(t) => t,
            Pending::Reread => match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => {
                    // Gone: any read failure counts (not just a bare
                    // ENOENT — a vanished file is a vanished file either
                    // way). Drop it from wherever it lived and, if the
                    // client currently shows anything for it, queue an
                    // explicit empty publish to clear it. A
                    // session member also forces the reload below.
                    let (cleared, forces_reload) = st.forget(&path);
                    changed.extend(cleared);
                    need_full_reload |= forces_reload;
                    continue;
                }
            },
        };
        let known = st.session.as_ref().is_some_and(|s| s.contains(&path));
        if known {
            resolved.push((path, text));
        } else if st.root.as_deref().is_some_and(|r| path.starts_with(r)) {
            // A file the walk did not see (new, untracked): full reload.
            need_full_reload = true;
        } else {
            // Outside the project, or no project at all: single-file
            // check, published alone.
            changed.push(path.clone());
            st.extra_check(path, text);
        }
    }
    // A failed load is not retried per keystroke: `apply` retries on the
    // next `Recheck`/config event instead.
    let reloaded = need_full_reload && !st.load_failed;
    if reloaded {
        changed.extend(full_reload(st, tx));
    } else if let Some(session) = st.session.as_mut() {
        for (path, text) in resolved {
            changed.extend(session.update(&path, text));
        }
    }
    (changed, reloaded)
}

/// Reload the workspace from disk (open buffers override), reset the session.
fn full_reload(st: &mut State, tx: &mpsc::Sender<Outbound>) -> Vec<PathBuf> {
    let Some(root) = st.root.clone() else { return Vec::new() };
    // Not git-ignore-aware yet (`false`): the server still lists every
    // file under the workspace folder, as it always has.
    let ws = match Workspace::load(std::slice::from_ref(&root), None, None, &root, false, None) {
        Ok(ws) => ws,
        Err(e) => {
            // Loud, and only once per triggering event: a server that
            // silently checks nothing looks identical to a clean project.
            eprintln!("rime lsp: {e}");
            let _ = tx.send(Outbound::Log(format!("rime: cannot load workspace: {e}")));
            st.load_failed = true;
            return Vec::new();
        }
    };
    st.load_failed = false;
    st.project_line = ws.project.line(|p| crate::workspace::show_path(p, &root));
    let mut files = ws.files;
    for (path, text) in &st.open {
        match files.iter_mut().find(|(p, _)| p == path) {
            Some(slot) => slot.1 = text.clone(),
            None if path.starts_with(&root) => files.push((path.clone(), text.clone())),
            None => {}
        }
    }
    let resolver: Option<Box<dyn rime_check::project::Resolver>> =
        ws.resolver.map(|r| Box::new(r) as Box<dyn rime_check::project::Resolver>);
    // What the client sees is the raw report AFTER `publish_for`'s config
    // post-processing (rule severities, `ignore`, the requested set), none
    // of which the session knows about. `Session::reset` diffs raw reports
    // only, so a rime.toml that turns a rule off — every raw report intact,
    // every visible one gone — yields an empty changed-set and the client
    // keeps showing what the config just suppressed. When that layer
    // changed, every session path is a candidate: `publish_for` still sends
    // nothing for a path that is clean both before and after.
    let view_changed = st.config != ws.config || st.requested != ws.requested;
    st.config = ws.config;
    st.requested = ws.requested;
    st.std = ws.std.clone();
    // The rules with settings of their own (rime.toml may have just
    // changed them); `reset` rechecks every file under them.
    let options = st.config.rule_options();
    match st.session.as_mut() {
        Some(session) => {
            session.set_options(options);
            let mut changed = session.reset(files, resolver, ws.std);
            if view_changed {
                changed.extend(session.paths().map(Path::to_path_buf));
            }
            changed
        }
        None => {
            let session = Session::with_options(files, resolver, ws.std, options, None);
            let all: Vec<PathBuf> = session.paths().map(Path::to_path_buf).collect();
            st.session = Some(session);
            all
        }
    }
}

impl State {
    fn uri_for(&mut self, path: &Path) -> Option<Uri> {
        if let Some(u) = self.uris.get(path) {
            return Some(u.clone());
        }
        let s = path.to_string_lossy();
        let plain = s.strip_prefix(r"\\?\").unwrap_or(&s);
        let uri = Uri::from_file_path(plain)?;
        self.uris.insert(path.to_path_buf(), uri.clone());
        Some(uri)
    }

    /// `LineIndex` + LSP conversion + `Publish`, shared by `publish_for`
    /// and `extra_check`. The one place `published` gets updated, so a
    /// later vanish (`State::forget`) can tell whether the client is
    /// currently showing anything for `path`.
    fn finish(&mut self, path: &Path, uri: Uri, src: &str, kept: Vec<rime_diag::Diagnostic>) -> Publish {
        if kept.is_empty() {
            self.published.remove(path);
        } else {
            self.published.insert(path.to_path_buf());
        }
        let index = LineIndex::new(src);
        let lsp = kept.iter().map(|d| to_lsp(d, &index, &uri)).collect();
        Publish { path: path.to_path_buf(), uri, index, raw: kept, lsp }
    }

    /// `(files, with diagnostics)` for the startup/reload log line: session
    /// size, and how many of its reports survive config post-processing and
    /// the ignore/requested filter — i.e. how many the client is shown.
    fn totals(&self) -> (usize, usize) {
        let Some(session) = self.session.as_ref() else { return (0, 0) };
        let mut files = 0usize;
        let mut with_diags = 0usize;
        for path in session.paths() {
            files += 1;
            let Some((raw, _)) = session.report(path) else { continue };
            if raw.is_empty() || self.config.ignored(path) || !self.requested.contains(path) {
                continue;
            }
            let (kept, _) = self.config.apply(raw.to_vec());
            if !kept.is_empty() {
                with_diags += 1;
            }
        }
        (files, with_diags)
    }

    /// The publish for one path after config post-processing. `None` when
    /// nothing was ever published for it and nothing needs to be; an
    /// explicit empty `Publish` clears a client that was previously told
    /// about this path — including a path `State::forget` just removed
    /// from the session (or that a `Reread`'s disk read failed for).
    fn publish_for(&mut self, path: &Path) -> Option<Publish> {
        // An outside-root (or no-root) file lives only in `extra` — the
        // session never sees it, so it must be checked before the report
        // lookup below.
        if let Some(p) = self.extra.remove(path) {
            return Some(p);
        }
        let raw = match self.session.as_ref().and_then(|s| s.report(path)) {
            Some((raw, _)) => raw.to_vec(),
            None => Vec::new(),
        };
        let (kept, _) = self.config.apply(raw);
        let ignored = self.config.ignored(path) || !self.requested.contains(path);
        // A crash inside Rime (R0901) is shown even where findings are not,
        // same as `rime check`.
        let kept = if ignored { kept.into_iter().filter(|d| d.is_internal_error()).collect() } else { kept };
        if kept.is_empty() && !self.published.remove(path) {
            return None;
        }
        let uri = self.uri_for(path)?;
        let src = self.session.as_ref().and_then(|s| s.source(path)).unwrap_or("").to_string();
        Some(self.finish(path, uri, &src, kept))
    }

    /// A `Reread` path whose disk read failed: treat it as gone, wherever
    /// it was tracked (a session member, an outside-root `extra` check, or
    /// a previously-published path in neither — e.g. its `extra` entry was
    /// already consumed by `publish_for` in an earlier batch). Returns the
    /// paths whose publish needs recomputing — usually just `path` itself,
    /// or nothing when the client was never told about it — and whether the
    /// batch must end in a full reload.
    ///
    /// A session member is NEVER dropped with `Session::remove` here: the
    /// live resolver's filesystem memo still answers `Found` for the
    /// vanished file, so every consumer would keep resolving through it and
    /// lose its R0501 (`remove` is for `reset`'s internals and
    /// the differential test). The batch takes a `full_reload` with a fresh
    /// resolver instead; this only clears what the client is showing, since
    /// the reload's changed-set cannot mention a path the walk no longer
    /// sees.
    fn forget(&mut self, path: &Path) -> (Vec<PathBuf>, bool) {
        if self.session.as_ref().is_some_and(|s| s.contains(path)) {
            let clear =
                if self.published.contains(path) { vec![path.to_path_buf()] } else { Vec::new() };
            return (clear, true);
        }
        let had_extra = self.extra.remove(path).is_some();
        if had_extra || self.published.contains(path) {
            return (vec![path.to_path_buf()], false);
        }
        (Vec::new(), false)
    }

    /// The current source of a file the worker can see: the open buffer
    /// wins (it is the truth the client's positions are against), else the
    /// session's resident copy. Outside-root `extra` files keep no source,
    /// so go-to-definition does not serve them.
    fn source_of(&self, path: &Path) -> Option<&str> {
        self.open
            .get(path)
            .map(String::as_str)
            .or_else(|| self.session.as_ref().and_then(|s| s.source(path)))
    }

    /// Resolve the token at an LSP position to a `Location`. Only
    /// session-held files (project mode) resolve; a no-project single file
    /// has no cross-file graph and returns `None`.
    fn answer_definition(&mut self, path: &Path, pos: Position) -> Option<Location> {
        let offset = LineIndex::new(self.source_of(path)?).offset(pos);
        let def = self.session.as_ref()?.definition(path, offset)?;
        // Build the target range before taking `&mut self` for `uri_for`.
        let range = LineIndex::new(self.source_of(&def.path)?).range(&def.span);
        let uri = self.uri_for(&def.path)?;
        Some(Location { uri, range })
    }

    /// Hover documentation for the token at an LSP position — the same
    /// resolution as `answer_definition`, rendered as Markdown.
    fn answer_hover(&mut self, path: &Path, pos: Position) -> Option<Hover> {
        let index = LineIndex::new(self.source_of(path)?);
        let offset = index.offset(pos);
        let h = self.session.as_ref()?.hover(path, offset)?;
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: h.contents,
            }),
            range: Some(index.range(&h.range)),
        })
    }

    /// An open file outside the project root (or with no project at all):
    /// checked alone, like `rime check <file>` outside a project. Stored
    /// keyed by path in `extra` so `publish_for` finds it. Uses the
    /// workspace's std, not the bundled one — rime.toml's `std` applies to
    /// every file this server checks.
    fn extra_check(&mut self, path: PathBuf, text: String) {
        let options = self.config.rule_options();
        let diags = rime_check::check_source_configured(&text, Some(&path), &self.std, &options);
        let Some(uri) = self.uri_for(&path) else { return };
        let (kept, _) = self.config.apply(diags);
        let publish = self.finish(&path, uri, &text, kept);
        self.extra.insert(path, publish);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf { PathBuf::from(s) }

    #[test]
    fn last_text_wins_per_path() {
        let mut b = Batch::default();
        b.push(Event::Change { path: p("a"), text: "1".into() });
        b.push(Event::Change { path: p("b"), text: "x".into() });
        b.push(Event::Change { path: p("a"), text: "2".into() });
        assert_eq!(
            b.texts,
            vec![(p("a"), Pending::Text("2".into())), (p("b"), Pending::Text("x".into()))]
        );
        assert!(!b.full);
    }

    #[test]
    fn config_or_tree_event_upgrades_to_full() {
        let mut b = Batch::default();
        b.push(Event::Change { path: p("a"), text: "1".into() });
        b.push(Event::Watched { path: p("rime.toml"), kind: FileChangeType::CHANGED });
        assert!(b.full);
        let mut c = Batch::default();
        c.push(Event::Watched { path: p("new.luau"), kind: FileChangeType::CREATED });
        assert!(c.full);
        let mut d = Batch::default();
        d.push(Event::Watched { path: p("old.luau"), kind: FileChangeType::CHANGED });
        assert!(!d.full);
        assert_eq!(d.texts, vec![(p("old.luau"), Pending::Reread)]);
    }

    #[test]
    fn shutdown_stops_the_batch() {
        let mut b = Batch::default();
        assert!(b.push(Event::Recheck));
        assert!(!b.push(Event::Shutdown));
        assert!(b.full);
    }

    #[test]
    fn change_then_watched_keeps_buffer_text() {
        let mut b = Batch::default();
        b.push(Event::Change { path: p("a"), text: "new".into() });
        b.push(Event::Watched { path: p("a"), kind: FileChangeType::CHANGED });
        assert_eq!(b.texts, vec![(p("a"), Pending::Text("new".into()))]);
    }

    #[test]
    fn change_then_close_rereads_disk() {
        let mut b = Batch::default();
        b.push(Event::Change { path: p("a"), text: "new".into() });
        b.push(Event::Close { path: p("a") });
        assert_eq!(b.texts, vec![(p("a"), Pending::Reread)]);
        assert_eq!(b.closed, vec![p("a")]);
    }

    /// Close then reopen inside one batch: the reopened buffer wins. The
    /// close must not survive to strip the path from `st.open` at apply
    /// time, which would send the reopened file back to disk for its text.
    #[test]
    fn open_after_close_keeps_buffer() {
        // Parsed, not built from a path: `Uri::from_file_path` takes an
        // absolute path, and `C:/x/a.luau` is one on Windows only.
        let uri: Uri = "file:///c%3A/x/a.luau".parse().expect("file uri");
        let mut b = Batch::default();
        b.push(Event::Close { path: p("a") });
        b.push(Event::Open { path: p("a"), uri, text: "buffer".into() });
        assert_eq!(b.texts, vec![(p("a"), Pending::Text("buffer".into()))]);
        assert!(b.closed.is_empty(), "the reopen cancels the close");
    }

    #[test]
    fn watched_then_change_takes_text() {
        let mut b = Batch::default();
        b.push(Event::Watched { path: p("a"), kind: FileChangeType::CHANGED });
        b.push(Event::Change { path: p("a"), text: "new".into() });
        assert_eq!(b.texts, vec![(p("a"), Pending::Text("new".into()))]);
    }

    /// No workspace folder -> the server stays idle project-wise
    /// and checks only open documents, in no-project mode.
    #[test]
    fn rootless_open_publishes_and_logs() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("rime-worker-test-{}.luau", std::process::id()));
        std::fs::write(&path, "local = \n").unwrap();
        let uri = Uri::from_file_path(path.to_str().expect("utf8 temp path")).expect("file uri");

        let mut st = State::new(None);
        let mut batch = Batch::default();
        batch.push(Event::Open { path: path.clone(), uri: uri.clone(), text: "local = \n".into() });

        let (tx, rx) = mpsc::channel();
        apply(&mut st, batch, &tx);

        let first = rx.recv().expect("a publish");
        let Outbound::Publish(publish) = first else { panic!("expected a Publish first") };
        assert_eq!(publish.uri, uri);
        assert!(publish.raw.iter().any(|d| d.code.0 == 900), "expected an R0900 parse-failure diagnostic");

        let second = rx.recv().expect("a log line");
        assert!(matches!(second, Outbound::Log(_)), "expected the batch's Log line last");

        let _ = std::fs::remove_file(&path);
    }

    /// Drain one batch's outbound: every `Publish` up to the `Log` line.
    fn publishes(rx: &mpsc::Receiver<Outbound>) -> Vec<Publish> {
        let mut out = Vec::new();
        loop {
            match rx.recv().expect("outbound") {
                Outbound::Publish(p) => out.push(p),
                Outbound::Log(_) => return out,
            }
        }
    }

    /// A rime.toml that turns a rule off changes what the client should see
    /// without changing any raw report. The reload's changed-set is a diff of
    /// raw reports, so it must be widened to every session path — otherwise
    /// the client keeps showing diagnostics the config just suppressed, until
    /// each file happens to be edited in a way that moves its spans.
    #[test]
    fn config_change_republishes_suppressed_reports() {
        let root = std::env::temp_dir().join(format!("rime-worker-cfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("a.luau");
        std::fs::write(&file, "local = \n").unwrap();
        let root = root.canonicalize().unwrap();

        let mut st = State::new(Some(root.clone()));
        let (tx, rx) = mpsc::channel();
        let mut batch = Batch::default();
        batch.push(Event::Recheck);
        apply(&mut st, batch, &tx);
        let first = publishes(&rx);
        assert_eq!(first.len(), 1, "startup publishes the one broken file");
        assert!(first[0].raw.iter().any(|d| d.code.0 == 900), "expected R0900 before the config");

        std::fs::write(root.join("rime.toml"), "[rules]\nR0900 = \"off\"\n").unwrap();
        let mut batch = Batch::default();
        batch.push(Event::Watched { path: root.join("rime.toml"), kind: FileChangeType::CREATED });
        apply(&mut st, batch, &tx);
        let second = publishes(&rx);
        assert_eq!(second.len(), 1, "the config event must republish the now-clean file");
        assert_eq!(second[0].path, first[0].path);
        assert!(second[0].lsp.is_empty(), "the republish is an explicit clear");

        // And back: removing the config restores the report.
        std::fs::remove_file(root.join("rime.toml")).unwrap();
        let mut batch = Batch::default();
        batch.push(Event::Watched { path: root.join("rime.toml"), kind: FileChangeType::DELETED });
        apply(&mut st, batch, &tx);
        let third = publishes(&rx);
        assert_eq!(third.len(), 1, "removing the config republishes the report");
        assert!(third[0].raw.iter().any(|d| d.code.0 == 900), "expected R0900 back");

        let _ = std::fs::remove_dir_all(&root);
    }
}
