//! End-to-end: the rules that report code which runs without raising
//! (a clamped `Color3.new`, a `UDim2.new` with defaulted arguments, a
//! division by a literal zero, a write into a required module) warn by
//! default, so they do not fail `rime check` until rime.toml raises them
//! to `"error"`.

use std::path::PathBuf;
use std::process::Command;

/// (rule, code, a file with exactly one finding of it). The file is
/// `src/case.luau` of a small Rojo project, beside `src/dep.luau`.
const CASES: [(&str, &str, &str); 4] = [
    ("color3-bounds", "R0801", "local c = Color3.new(255, 0, 0)\nreturn c\n"),
    ("suspicious-udim2-new", "R0804", "local u = UDim2.new(0.5, 0.5)\nreturn u\n"),
    ("divide-by-zero", "R0602", "local function f(x: number): number\n\treturn x / 0\nend\nreturn f\n"),
    ("module-write", "R0308", "local dep = require(script.Parent.dep)\ndep.limit = 2\nreturn dep.limit\n"),
];

/// A project file that mounts `src`. Rules that read the project graph
/// (requires, module names) run only inside a Rojo project.
const PROJECT: &str = r#"{ "name": "case", "tree": { "$className": "DataModel", "ReplicatedStorage": { "$className": "ReplicatedStorage", "Shared": { "$path": "src" } } } }"#;

/// The module the `module-write` case requires and writes into.
const DEP: &str = "local dep = {}\ndep.limit = 1\nreturn dep\n";

fn fixture(name: &str, source: &str, toml: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rime-warn-by-default-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("default.project.json"), PROJECT).unwrap();
    std::fs::write(dir.join("src").join("dep.luau"), DEP).unwrap();
    std::fs::write(dir.join("src").join("case.luau"), source).unwrap();
    if let Some(toml) = toml {
        std::fs::write(dir.join("rime.toml"), toml).unwrap();
    }
    dir
}

/// Exit code, and each diagnostic as (code, severity).
fn run(dir: &PathBuf) -> (Option<i32>, Vec<(String, String)>) {
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "src"])
        .current_dir(dir)
        .output()
        .expect("spawn rime");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON");
    let diags = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["code"].as_str().unwrap().to_string(), d["severity"].as_str().unwrap().to_string()))
        .collect();
    (out.status.code(), diags)
}

#[test]
fn each_rule_warns_and_does_not_fail_the_run() {
    for (name, code, source) in CASES {
        let dir = fixture(&format!("default-{name}"), source, None);
        let (exit, diags) = run(&dir);
        assert_eq!(diags, vec![(code.to_string(), "warning".to_string())], "{name}");
        assert_eq!(exit, Some(0), "{name}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn each_rule_blocks_once_set_to_error() {
    for (name, code, source) in CASES {
        let dir = fixture(&format!("error-{name}"), source, Some(&format!("[rules]\n{name} = \"error\"\n")));
        let (exit, diags) = run(&dir);
        assert_eq!(diags, vec![(code.to_string(), "error".to_string())], "{name}");
        assert_eq!(exit, Some(1), "{name} = \"error\" must fail the run");
        std::fs::remove_dir_all(&dir).ok();
    }
}
