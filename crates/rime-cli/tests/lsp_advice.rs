//! End-to-end over stdio: advice is not a quick fix.
//!
//! Most rules tell the reader what to do in words ("guard the use: `if n ~=
//! nil then ... end`") and have no text to put in the file. A quick fix is
//! a text edit, and an editor or an agent applies it without reading it, so
//! advice offered as one deletes the code it points at. This test asks the
//! real server for the code actions on such a diagnostic.
//!
//! The harness is a trimmed copy of `lsp_stdio.rs`'s (kept separate so the
//! two files do not share edits).

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn recv_one(reader: &mut BufReader<impl Read>) -> Option<String> {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap() == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length: ") {
            len = v.trim().parse().unwrap();
        }
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).unwrap();
    Some(String::from_utf8(buf).unwrap())
}

struct Server {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<String>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn spawn() -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_rime"))
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let (tx, rx) = mpsc::channel::<String>();
        let reader = std::thread::spawn(move || {
            while let Some(msg) = recv_one(&mut out) {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        Server { child, stdin, rx, reader: Some(reader) }
    }

    fn send(&mut self, body: &str) {
        write!(self.stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
        self.stdin.flush().unwrap();
    }

    /// The next message matching `pred`, answering the server's
    /// `client/registerCapability` request along the way. Bounded, so a
    /// server that never answers fails the test instead of hanging it.
    fn recv_until(&mut self, pred: impl Fn(&str) -> bool) -> String {
        loop {
            let msg = self.rx.recv_timeout(Duration::from_secs(30)).expect("a message within 30s");
            if msg.contains("client/registerCapability") {
                let id = serde_json::from_str::<serde_json::Value>(&msg).unwrap()["id"].clone();
                self.send(&format!(r#"{{"jsonrpc":"2.0","id":{id},"result":null}}"#));
                continue;
            }
            if pred(&msg) {
                return msg;
            }
        }
    }

    fn shutdown(mut self) {
        self.send(r#"{"jsonrpc":"2.0","id":99,"method":"shutdown","params":null}"#);
        let _ = self.recv_until(|m| m.contains("\"id\":99"));
        self.send(r#"{"jsonrpc":"2.0","method":"exit","params":null}"#);
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                break self.child.wait().unwrap();
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(status.success());
        self.reader.take().unwrap().join().unwrap();
    }
}

#[test]
fn advice_is_shown_in_the_message_and_never_offered_as_an_edit() {
    let dir = std::env::temp_dir().join(format!("rime-lsp-advice-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // R0201 on the `n` of `n + 1` (line 1, columns 8..9). Its advice is
    // "guard the use: `if n ~= nil then ... end`" — words, no replacement.
    let text = "local function f(n: number?)\n\treturn n + 1\nend\nreturn f\n";
    std::fs::write(dir.join("advice.luau"), text).unwrap();

    let root = dir.canonicalize().unwrap();
    let root_uri = {
        let s = root.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/");
        format!("file:///{}", s.trim_start_matches('/'))
    };
    let file_uri = format!("{root_uri}/advice.luau");

    let mut s = Server::spawn();
    s.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root_uri}","capabilities":{{}},"initializationOptions":{{"debounceMs":10}}}}}}"#
    ));
    s.recv_until(|m| m.contains("\"capabilities\""));
    s.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    let open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": { "textDocument": {
            "uri": file_uri, "languageId": "luau", "version": 1, "text": text } },
    });
    s.send(&open.to_string());

    let published = s
        .recv_until(|m| m.contains("textDocument/publishDiagnostics") && m.contains("advice.luau"));
    assert!(published.contains("R0201"), "{published}");

    let action = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "textDocument/codeAction",
        "params": {
            "textDocument": { "uri": file_uri },
            "range": {
                "start": { "line": 1, "character": 8 },
                "end": { "line": 1, "character": 9 },
            },
            "context": { "diagnostics": [] },
        },
    });
    s.send(&action.to_string());
    let resp = s.recv_until(|m| m.contains("\"id\":3"));
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    // No action at all: this diagnostic has nothing to put in the file.
    // In particular no edit that replaces `n` with the empty string.
    let actions = v["result"].as_array().cloned().unwrap_or_default();
    assert!(actions.is_empty(), "advice was offered as a code action: {resp}");
    // The advice still reaches the reader, as part of the diagnostic's
    // message.
    assert!(published.contains("help: guard the use"), "{published}");

    s.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
