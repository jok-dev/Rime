//! End-to-end over stdio: initialize -> didOpen a broken file -> expect a
//! publishDiagnostics with R0900 -> shutdown/exit; and the quick-fix path
//! for a file the client opens unmodified after the startup publish.
//!
//! The child's stdout is read on a dedicated thread and forwarded through a
//! channel with a bounded `recv_timeout`, so a server that never publishes
//! fails this test within ~30s instead of hanging CI forever.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn send(stdin: &mut impl Write, body: &str) {
    write!(stdin, "Content-Length: {}\r\n\r\n{}", body.len(), body).unwrap();
    stdin.flush().unwrap();
}

/// One framed message, or `None` on a clean EOF (the child closed stdout).
/// Distinguishing EOF from "still waiting" keeps the forwarding thread below
/// from busy-spinning once the child exits: a plain `read_line` returns
/// `Ok(0)` at EOF without ever producing the blank `"\r\n"` line the header
/// loop is waiting for, so it would otherwise loop forever doing zero-byte
/// reads.
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

/// `child.wait()`, bounded. A server that never exits after `exit` would
/// otherwise hang the test run forever; killing it at the deadline turns
/// that into a plain assertion failure (a killed child never reports
/// success).
fn wait_bounded(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            return child.wait().unwrap();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A running `rime lsp` child, with its stdout forwarded through a channel.
struct Server {
    child: Child,
    stdin: ChildStdin,
    rx: mpsc::Receiver<String>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn spawn() -> Server {
        Server::spawn_with_fault(None)
    }

    /// `fault` is a `RIME_INTERNAL_FAULT` spec (`rime_check::guard::fault`).
    fn spawn_with_fault(fault: Option<&str>) -> Server {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rime"));
        cmd.env_remove("RIME_INTERNAL_FAULT");
        if let Some(fault) = fault {
            cmd.env("RIME_INTERNAL_FAULT", fault);
        }
        let mut child = cmd
            .arg("lsp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut out = BufReader::new(child.stdout.take().unwrap());
        // Forward every framed message from the child's stdout through a
        // channel on its own thread, so `recv_until` below can bound its
        // wait with `recv_timeout` instead of blocking forever on a server
        // that never publishes.
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
        send(&mut self.stdin, body);
    }

    /// The next message matching `pred`, answering the server's
    /// `client/registerCapability` request along the way — `initialized`
    /// awaits that reply before it posts the startup recheck.
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
        assert!(wait_bounded(&mut self.child).success());
        // The child has exited, so its stdout pipe is closed: `recv_one`
        // now sees a clean EOF and the forwarding thread returns on its own.
        self.reader.take().unwrap().join().unwrap();
    }
}

/// `file:///C:/x/y` for the canonical `root` — the spelling this test's
/// "client" uses for `rootUri`.
fn root_uri(root: &std::path::Path) -> String {
    let s = root.to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/");
    format!("file:///{}", s.trim_start_matches('/'))
}

#[test]
fn publishes_parse_error_for_opened_document() {
    let dir = std::env::temp_dir().join(format!("rime-lsp-stdio-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("broken.luau");
    std::fs::write(&file, "local = \n").unwrap();
    let root = dir.canonicalize().unwrap();
    let root_uri = root_uri(&root);
    let file_uri = format!("{root_uri}/broken.luau");

    let mut s = Server::spawn();
    s.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root_uri}","capabilities":{{}},"initializationOptions":{{"debounceMs":10}}}}}}"#
    ));
    let reply = s.recv_until(|m| m.contains("\"capabilities\""));
    assert!(reply.contains("\"capabilities\""), "{reply}");
    s.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    s.send(&format!(
        r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{file_uri}","languageId":"luau","version":1,"text":"local = \n"}}}}}}"#
    ));

    let published =
        s.recv_until(|m| m.contains("textDocument/publishDiagnostics") && m.contains("broken.luau"));
    assert!(published.contains("R0900"), "{published}");

    s.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Ctrl+click inside a `require(...)` jumps to the required file, and a
/// member of the required module jumps to that member's declaration in it.
/// End-to-end over stdio: the worker answers `textDocument/definition` off
/// the debounce path, against the warm session.
#[test]
fn goto_definition_across_a_require() {
    let dir = std::env::temp_dir().join(format!("rime-lsp-gotodef-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("default.project.json"),
        "{\n  \"name\": \"probe\",\n  \"tree\": {\n    \"$className\": \"DataModel\",\n    \
         \"ReplicatedStorage\": {\n      \"$className\": \"ReplicatedStorage\",\n      \
         \"$path\": \"src\"\n    }\n  }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src").join("dep.luau"),
        "local Dep = {}\nfunction Dep.foo(a: number): number\n\treturn a\nend\nreturn Dep\n",
    )
    .unwrap();
    // `require(script.Parent.dep)` on line 0; `Dep.foo` on line 1.
    let main_text = "local Dep = require(script.Parent.dep)\nreturn Dep.foo\n";
    std::fs::write(dir.join("src").join("main.luau"), main_text).unwrap();

    let root = dir.canonicalize().unwrap();
    let root_uri = root_uri(&root);
    let main_uri = format!("{root_uri}/src/main.luau");
    let dep_uri_frag = "/src/dep.luau";

    let mut s = Server::spawn();
    s.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root_uri}","capabilities":{{}},"initializationOptions":{{"debounceMs":10}}}}}}"#
    ));
    let caps = s.recv_until(|m| m.contains("\"capabilities\""));
    assert!(caps.contains("\"definitionProvider\""), "server must advertise definition: {caps}");
    s.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    // Both files are clean, so nothing publishes: wait for the startup log
    // line, which the worker sends only after the session is built.
    s.recv_until(|m| m.contains("rime: checked"));

    let open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": { "textDocument": {
            "uri": main_uri, "languageId": "luau", "version": 1, "text": main_text } },
    });
    s.send(&open.to_string());

    // Inside `require(script.Parent.dep)` — line 0, column 30 (the `dep`).
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 10, "method": "textDocument/definition",
        "params": {
            "textDocument": { "uri": main_uri },
            "position": { "line": 0, "character": 30 },
        },
    });
    s.send(&req.to_string());
    let resp = s.recv_until(|m| m.contains("\"id\":10"));
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    let loc = &v["result"];
    assert!(
        loc["uri"].as_str().unwrap().ends_with(dep_uri_frag),
        "require jump should land in dep.luau: {resp}"
    );

    // `Dep.foo` — line 1, column 11 (the `foo`). Jumps to dep.luau's `foo`
    // declaration on its line 1.
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 11, "method": "textDocument/definition",
        "params": {
            "textDocument": { "uri": main_uri },
            "position": { "line": 1, "character": 11 },
        },
    });
    s.send(&req.to_string());
    let resp = s.recv_until(|m| m.contains("\"id\":11"));
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    let loc = &v["result"];
    assert!(loc["uri"].as_str().unwrap().ends_with(dep_uri_frag), "member jump target file: {resp}");
    assert_eq!(loc["range"]["start"]["line"], 1, "foo is declared on dep.luau line 1: {resp}");

    s.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Hovering a required module's function shows its signature and doc
/// comment. End-to-end over stdio, answered off the debounce path.
#[test]
fn hover_shows_function_signature() {
    let dir = std::env::temp_dir().join(format!("rime-lsp-hover-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("default.project.json"),
        "{\n  \"name\": \"probe\",\n  \"tree\": {\n    \"$className\": \"DataModel\",\n    \
         \"ReplicatedStorage\": {\n      \"$className\": \"ReplicatedStorage\",\n      \
         \"$path\": \"src\"\n    }\n  }\n}\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src").join("dep.luau"),
        "local Dep = {}\n--- Doubles the input.\nfunction Dep.twice(n: number): number\n\treturn n * 2\nend\nreturn Dep\n",
    )
    .unwrap();
    let main_text = "local Dep = require(script.Parent.dep)\nreturn Dep.twice(3)\n";
    std::fs::write(dir.join("src").join("main.luau"), main_text).unwrap();

    let root = dir.canonicalize().unwrap();
    let root_uri = root_uri(&root);
    let main_uri = format!("{root_uri}/src/main.luau");

    let mut s = Server::spawn();
    s.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root_uri}","capabilities":{{}},"initializationOptions":{{"debounceMs":10}}}}}}"#
    ));
    let caps = s.recv_until(|m| m.contains("\"capabilities\""));
    assert!(caps.contains("\"hoverProvider\""), "server must advertise hover: {caps}");
    s.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    s.recv_until(|m| m.contains("rime: checked"));

    let open = serde_json::json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": { "textDocument": {
            "uri": main_uri, "languageId": "luau", "version": 1, "text": main_text } },
    });
    s.send(&open.to_string());

    // `Dep.twice(3)` on line 1 — hover the `twice` (column 11).
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 20, "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": main_uri },
            "position": { "line": 1, "character": 11 },
        },
    });
    s.send(&req.to_string());
    let resp = s.recv_until(|m| m.contains("\"id\":20"));
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    let value = v["result"]["contents"]["value"].as_str().unwrap_or("");
    assert!(value.contains("function Dep.twice(n: number): number"), "signature in hover: {resp}");
    assert!(value.contains("Doubles the input."), "doc in hover: {resp}");

    s.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A file the startup recheck already published, then opened unmodified by
/// the client: no republish happens, so the code action can only work if the
/// published state is keyed by canonical PATH. VS Code spells the URI
/// `file:///c%3A/…` while the server's own `Uri::from_file_path` produces
/// `file:///C%3A/…`, and `Uri` equality is string equality — keying by URI
/// silently loses every quick fix for a pre-existing diagnostic.
#[test]
fn quick_fix_on_unmodified_open_file() {
    // R0707 (a needless `\'` in a double-quoted string) carries a fix:
    // remove the backslash. The file sits in a one-mount Rojo project, the
    // smallest thing that makes the startup recheck find and publish it.
    let dir = std::env::temp_dir().join(format!("rime-lsp-quickfix-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("default.project.json"),
        "{\n  \"name\": \"probe\",\n  \"tree\": {\n    \"$className\": \"DataModel\",\n    \
         \"ReplicatedStorage\": {\n      \"$className\": \"ReplicatedStorage\",\n      \
         \"$path\": \"src\"\n    }\n  }\n}\n",
    )
    .unwrap();
    let text = "return \"it\\'s\"\n";
    std::fs::write(dir.join("src").join("stub.luau"), text).unwrap();

    let root = dir.canonicalize().unwrap();
    let root_uri = root_uri(&root);
    // What VS Code actually sends for the same file: lowercase drive letter,
    // `:` percent-encoded.
    let client_uri = {
        let s = root_uri.trim_start_matches("file:///");
        let s = match s.split_once(':') {
            Some((drive, rest)) => format!("{}%3A{rest}", drive.to_lowercase()),
            None => s.to_string(),
        };
        format!("file:///{s}/src/stub.luau")
    };

    let mut s = Server::spawn();
    s.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root_uri}","capabilities":{{}},"initializationOptions":{{"debounceMs":10}}}}}}"#
    ));
    s.recv_until(|m| m.contains("\"capabilities\""));
    s.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);

    // The startup publish must land BEFORE the didOpen: it is what seeds the
    // published state the code action reads, in the server's own URI
    // spelling.
    let published =
        s.recv_until(|m| m.contains("textDocument/publishDiagnostics") && m.contains("stub.luau"));
    assert!(published.contains("R0707"), "{published}");

    // Same text, so nothing republishes — only the URI spelling differs.
    let open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": { "textDocument": {
            "uri": client_uri, "languageId": "luau", "version": 1, "text": text } },
    });
    s.send(&open.to_string());

    // The `\'`: line 0, UTF-16 columns 10..12.
    let action = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "textDocument/codeAction",
        "params": {
            "textDocument": { "uri": client_uri },
            "range": {
                "start": { "line": 0, "character": 10 },
                "end": { "line": 0, "character": 12 },
            },
            "context": { "diagnostics": [] },
        },
    });
    s.send(&action.to_string());

    let resp = s.recv_until(|m| m.contains("\"id\":3"));
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    let actions = v["result"].as_array().unwrap_or_else(|| panic!("no code actions: {resp}"));
    assert_eq!(actions.len(), 1, "{resp}");
    assert_eq!(actions[0]["kind"], "quickfix", "{resp}");
    let edits = actions[0]["edit"]["changes"][&client_uri]
        .as_array()
        .unwrap_or_else(|| panic!("no edit for the client's own URI: {resp}"));
    assert_eq!(edits.len(), 1, "{resp}");
    assert_eq!(edits[0]["newText"], "", "{resp}");

    s.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Deeply nested text typed into the editor is parsed on the server's own
/// worker thread, not the rayon pool. On a default-size stack that thread
/// overflowed (an abort: the server just vanished) at a handful of nested
/// blocks in this debug build; see tests/deep_nesting.rs for the depths.
#[test]
fn deeply_nested_text_does_not_kill_the_server() {
    let dir = std::env::temp_dir().join(format!("rime-lsp-deep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("deep.luau"), "return 1\n").unwrap();
    let root = dir.canonicalize().unwrap();
    let root_uri = root_uri(&root);
    let uri = format!("{root_uri}/deep.luau");

    let mut s = Server::spawn();
    s.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root_uri}","capabilities":{{}},"initializationOptions":{{"debounceMs":10}}}}}}"#
    ));
    s.recv_until(|m| m.contains("\"capabilities\""));
    s.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);
    s.recv_until(|m| m.contains("rime: checked"));

    let depth = 300;
    let text = format!(
        "local c = true\n{}print(definitely_undefined_name)\n{}",
        "if c then\n".repeat(depth),
        "end\n".repeat(depth)
    );
    let open = serde_json::json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": { "textDocument": { "uri": uri, "languageId": "luau", "version": 1, "text": text } },
    });
    s.send(&open.to_string());
    let published = s.recv_until(|m| m.contains("textDocument/publishDiagnostics") && m.contains("deep.luau"));
    assert!(published.contains("R0022"), "the innermost statement was checked: {published}");

    s.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A crash inside Rime on one file must not end the language server: the
/// crash is published on that file as R0901, the other files keep their
/// diagnostics, and the server goes on answering — through further edits
/// (one of which crashes again) and to a clean shutdown.
#[test]
fn a_crash_on_one_file_is_published_and_the_server_lives() {
    let dir = std::env::temp_dir().join(format!("rime-lsp-crash-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("boom.luau"), "return 1\n").unwrap();
    std::fs::write(dir.join("other.luau"), "print(definitely_undefined_name)\n").unwrap();
    let root = dir.canonicalize().unwrap();
    let root_uri = root_uri(&root);

    let mut s = Server::spawn_with_fault(Some("check:boom"));
    s.send(&format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"processId":null,"rootUri":"{root_uri}","capabilities":{{}},"initializationOptions":{{"debounceMs":10}}}}}}"#
    ));
    s.recv_until(|m| m.contains("\"capabilities\""));
    s.send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#);

    // The startup publish: one message per file with diagnostics, in
    // either order.
    let (mut boom, mut other) = (None, None);
    while boom.is_none() || other.is_none() {
        let m = s.recv_until(|m| m.contains("textDocument/publishDiagnostics"));
        if m.contains("boom.luau") {
            boom = Some(m);
        } else if m.contains("other.luau") {
            other = Some(m);
        }
    }
    let boom = boom.unwrap();
    assert!(boom.contains("R0901"), "{boom}");
    assert!(boom.contains("Rime crashed while checking this file"), "{boom}");
    assert!(other.unwrap().contains("R0022"), "the healthy file keeps its findings");

    // Edit the healthy file: the incremental recheck still runs and publishes.
    let open = serde_json::json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": { "textDocument": {
            "uri": format!("{root_uri}/other.luau"), "languageId": "luau", "version": 1,
            "text": "print(another_undefined_name)\n" } },
    });
    s.send(&open.to_string());
    let republished =
        s.recv_until(|m| m.contains("textDocument/publishDiagnostics") && m.contains("other.luau"));
    assert!(republished.contains("another_undefined_name"), "{republished}");

    // Edit the crashing file itself: its recheck crashes again, inside
    // the incremental path this time. Its diagnostics are unchanged, so
    // nothing need republish; a request proves the worker still answers.
    let open = serde_json::json!({
        "jsonrpc": "2.0", "method": "textDocument/didOpen",
        "params": { "textDocument": {
            "uri": format!("{root_uri}/boom.luau"), "languageId": "luau", "version": 1,
            "text": "print(yet_another_undefined_name)\nreturn 2\n" } },
    });
    s.send(&open.to_string());
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 30, "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": format!("{root_uri}/other.luau") },
            "position": { "line": 0, "character": 1 },
        },
    });
    s.send(&req.to_string());
    let resp = s.recv_until(|m| m.contains("\"id\":30"));
    assert!(!resp.contains("\"error\""), "{resp}");

    s.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
