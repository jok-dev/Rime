//! End-to-end: `rime check --only <rules>` reports exactly the listed
//! rules, at their default severity, even when rime.toml turns them off.

use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rime-only-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // R0022 undefined-variable (error) + R0013 unused-local (warning).
    std::fs::write(
        dir.join("case.luau"),
        "local unused_thing = 1\nprint(definitely_undefined_name)\n",
    )
    .unwrap();
    dir
}

fn run(dir: &PathBuf, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rime"))
        .arg("check")
        .args(args)
        .arg("case.luau")
        .current_dir(dir)
        .output()
        .expect("spawn rime")
}

fn codes(out: &std::process::Output) -> (Vec<String>, u64) {
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON");
    let codes = v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["code"].as_str().unwrap().to_string())
        .collect();
    (codes, v["summary"]["ignored"].as_u64().unwrap())
}

#[test]
fn only_keeps_the_listed_rule_and_counts_the_rest_as_ignored() {
    let dir = fixture("keep");
    let baseline = run(&dir, &["--format", "json"]);
    let (all, _) = codes(&baseline);
    assert!(all.contains(&"R0022".to_string()) && all.contains(&"R0013".to_string()), "fixture: {all:?}");

    let out = run(&dir, &["--format", "json", "--only", "undefined-variable"]);
    let (only, ignored) = codes(&out);
    assert_eq!(only, vec!["R0022".to_string()]);
    assert_eq!(ignored as usize, all.len() - 1);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn only_accepts_raw_codes_and_repeats() {
    let dir = fixture("raw");
    let out = run(&dir, &["--format", "json", "--only", "R0022", "--only", "unused-local"]);
    let (mut got, _) = codes(&out);
    got.sort();
    assert_eq!(got, vec!["R0013".to_string(), "R0022".to_string()]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn only_overrides_an_off_rule_in_rime_toml() {
    let dir = fixture("off");
    std::fs::write(dir.join("rime.toml"), "[rules]\nundefined-variable = \"off\"\n").unwrap();
    let silent = run(&dir, &["--format", "json"]);
    let (all, _) = codes(&silent);
    assert!(!all.contains(&"R0022".to_string()), "toml off must silence it: {all:?}");

    let out = run(&dir, &["--format", "json", "--only", "undefined-variable"]);
    let (only, _) = codes(&out);
    assert_eq!(only, vec!["R0022".to_string()]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn only_still_reports_a_file_that_does_not_parse() {
    // A file that fails to parse contributes nothing to ANY rule (no
    // require edges, no globals, ...), so hiding R0900 under `--only`
    // would turn "not checked" into "clean". Found the hard way: an
    // in-progress edit on a large codebase silently dissolved a require
    // ring under `--only circular-require`.
    let dir = fixture("parse");
    std::fs::write(dir.join("case.luau"), "local function f()\nprint(definitely_undefined_name)\n").unwrap();
    let out = run(&dir, &["--format", "json", "--only", "circular-require"]);
    let (got, _) = codes(&out);
    assert_eq!(got, vec!["R0900".to_string()]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn only_rejects_an_unknown_rule_before_checking() {
    let dir = fixture("unknown");
    let out = run(&dir, &["--only", "not-a-rule"]);
    assert!(out.stdout.is_empty(), "nothing checked: {}", String::from_utf8_lossy(&out.stdout));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not-a-rule") && err.contains("unknown rule"), "stderr: {err}");
    std::fs::remove_dir_all(&dir).ok();
}
