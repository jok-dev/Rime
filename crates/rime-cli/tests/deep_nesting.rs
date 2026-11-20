//! Deeply nested source must be checked, not kill the process.
//!
//! The parser and the passes over its tree recurse once per nesting level,
//! and a stack overflow is an abort: no diagnostic, no summary, and on
//! Windows an exit status (0xC00000FD) that says nothing about which file.
//! On the default 2 MiB thread stack this debug binary died at about six
//! nested `if`s, so `rime check` runs its work on large stacks
//! (`WORK_STACK_BYTES` in main.rs). These depths are the floor under that:
//! far beyond anything hand-written, and well inside what the stack size
//! is measured to hold in a debug build (1,700 nested closures, the most
//! expensive shape, with full_moon 3.0 — it was about 590 with 2.2; 8,000
//! in a release build).

use std::path::PathBuf;
use std::process::Command;

const BLOCK_DEPTH: usize = 300;
const EXPR_DEPTH: usize = 1000;

fn shapes() -> Vec<(&'static str, String)> {
    let (b, e) = (BLOCK_DEPTH, EXPR_DEPTH);
    vec![
        (
            "closures",
            format!(
                "local function f(a) return a end\n{}{}",
                "f(function()\n".repeat(b),
                "end)\n".repeat(b)
            ),
        ),
        ("ifs", format!("local c = true\n{}{}", "if c then\n".repeat(b), "end\n".repeat(b))),
        ("dos", format!("{}{}", "do\n".repeat(b), "end\n".repeat(b))),
        ("parens", format!("local x = {}1{}\nreturn x\n", "(".repeat(e), ")".repeat(e))),
        ("tables", format!("local x = {}{}\nreturn x\n", "{".repeat(e), "}".repeat(e))),
        (
            "calls",
            format!(
                "local function f(a) return a end\nlocal x = {}1{}\nreturn x\n",
                "f(".repeat(e),
                ")".repeat(e)
            ),
        ),
        ("unary", format!("local x = {}true\nreturn x\n", "not ".repeat(e))),
        ("concat", format!("local a = ''\nlocal x = a{}\nreturn x\n", " .. a".repeat(e))),
    ]
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rime-deep-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn check(dir: &PathBuf) -> (Option<i32>, serde_json::Value, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "."])
        .current_dir(dir)
        // The floor is for the built-in stack size, not a borrowed one.
        .env_remove("RUST_MIN_STACK")
        .output()
        .expect("spawn rime");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let v = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    (out.status.code(), v, stderr)
}

fn assert_checked(what: &str, files: u64, (code, v, stderr): (Option<i32>, serde_json::Value, String)) {
    assert!(
        matches!(code, Some(0) | Some(1)),
        "{what}: rime did not finish normally (status {code:?}): {stderr}"
    );
    assert!(!stderr.contains("overflowed its stack"), "{what}: {stderr}");
    assert_eq!(v["summary"]["checked"], files, "{what}: {v}");
    assert_eq!(v["summary"]["internalErrors"], 0, "{what}: {v}");
    let codes: Vec<&str> =
        v["diagnostics"].as_array().unwrap().iter().map(|d| d["code"].as_str().unwrap()).collect();
    assert!(!codes.contains(&"R0900"), "{what}: the file must parse, got {codes:?}");
}

#[test]
fn each_deeply_nested_shape_is_checked() {
    for (name, src) in shapes() {
        let dir = temp_dir(name);
        std::fs::write(dir.join("deep.luau"), src).unwrap();
        assert_checked(name, 1, check(&dir));
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// The same files together, in a Rojo project: the parallel passes (rayon
/// workers) and the project-mode paths, not just the single-file one.
#[test]
fn deeply_nested_files_are_checked_in_a_project() {
    let dir = temp_dir("project");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("default.project.json"),
        r#"{ "name": "t", "tree": { "$className": "DataModel", "ReplicatedStorage": { "$path": "src" } } }"#,
    )
    .unwrap();
    let shapes = shapes();
    for (name, src) in &shapes {
        std::fs::write(dir.join("src").join(format!("{name}.luau")), src).unwrap();
    }
    assert_checked("project", shapes.len() as u64, check(&dir));
    std::fs::remove_dir_all(&dir).ok();
}
