//! End-to-end, for the last two Selene lints built (`must_use`,
//! `mismatched_arg_count`): a rule that is
//! a warning by default fails `rime check` once rime.toml sets it to
//! `"error"`, a rule that is an error fails it as it is, and `"off"`
//! silences either. Same shape as `raise_level.rs`.

use std::path::PathBuf;
use std::process::Command;

/// (rule, code, default severity, a file with exactly one finding of it).
const CASES: [(&str, &str, &str, &str); 2] = [
    ("must-use", "R0708", "warning", "local a, b = 1, 2\nmath.max(a, b)\nreturn a + b\n"),
    // A function declared inside another: out of R0304's reach before it
    // resolved plain names through scope.
    (
        "call-extra-args",
        "R0304",
        "error",
        "local function outer()\n\tlocal function inner(a)\n\t\treturn a\n\tend\n\treturn inner(1, 2)\nend\nreturn outer\n",
    ),
];

fn fixture(name: &str, source: &str, toml: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rime-raise-last-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("case.luau"), source).unwrap();
    if let Some(toml) = toml {
        std::fs::write(dir.join("rime.toml"), toml).unwrap();
    }
    dir
}

/// Exit code, and each diagnostic as (code, severity).
fn run(dir: &PathBuf) -> (Option<i32>, Vec<(String, String)>) {
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "case.luau"])
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
fn each_rule_reports_at_its_default_level() {
    for (name, code, severity, source) in CASES {
        let dir = fixture(&format!("default-{name}"), source, None);
        let (exit, diags) = run(&dir);
        assert_eq!(diags, vec![(code.to_string(), severity.to_string())], "{name}");
        assert_eq!(exit, Some(if severity == "error" { 1 } else { 0 }), "{name}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn each_rule_blocks_once_set_to_error() {
    for (name, code, _, source) in CASES {
        let dir = fixture(&format!("error-{name}"), source, Some(&format!("[rules]\n{name} = \"error\"\n")));
        let (exit, diags) = run(&dir);
        assert_eq!(diags, vec![(code.to_string(), "error".to_string())], "{name}");
        assert_eq!(exit, Some(1), "{name} = \"error\" must fail the run");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn each_rule_can_be_switched_off() {
    for (name, _, _, source) in CASES {
        let dir = fixture(&format!("off-{name}"), source, Some(&format!("[rules]\n{name} = \"off\"\n")));
        let (exit, diags) = run(&dir);
        assert_eq!(diags, Vec::<(String, String)>::new(), "{name}");
        assert_eq!(exit, Some(0), "{name}");
        std::fs::remove_dir_all(&dir).ok();
    }
}