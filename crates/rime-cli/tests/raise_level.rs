//! End-to-end: a rule Rime reports as a warning fails `rime check` once
//! rime.toml sets it to `"error"`. Selene fails a run on its warning lints
//! (`unused_variable`, `global_usage`, `empty_loop`, `manual_table_clone`,
//! `invalid_lint_filter`); this is how a project asks Rime for the same.

use std::path::PathBuf;
use std::process::Command;

/// One finding from each of the five warning rules that stand in for a
/// blocking Selene lint, and nothing at error level.
const CASE: &str = "local unused_thing = 1\n\
                    local source = { a = 1, b = 2 }\n\
                    local out = {}\n\
                    for k, v in pairs(source) do\n\
                    \tout[k] = v\n\
                    end\n\
                    _G.flag = next(out) ~= nil\n\
                    for _ = 1, 3 do\n\
                    end\n\
                    -- rime: allow(unused-local)\n\
                    return { source = source, copy = out }\n";

const WARNING_RULES: [(&str, &str); 5] = [
    ("unused-local", "R0013"),
    ("global-usage", "R0024"),
    ("empty-loop", "R0702"),
    ("manual-table-clone", "R0704"),
    ("malformed-allow", "R0021"),
];

fn fixture(name: &str, toml: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rime-raise-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("case.luau"), CASE).unwrap();
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

fn severity_of(diags: &[(String, String)], code: &str) -> Vec<String> {
    diags.iter().filter(|(c, _)| c == code).map(|(_, s)| s.clone()).collect()
}

#[test]
fn the_five_rules_are_warnings_that_do_not_fail_the_run_by_default() {
    let dir = fixture("default", None);
    let (exit, diags) = run(&dir);
    for (name, code) in WARNING_RULES {
        assert_eq!(severity_of(&diags, code), vec!["warning".to_string()], "{name}: {diags:?}");
    }
    assert_eq!(diags.len(), WARNING_RULES.len(), "fixture reports nothing else: {diags:?}");
    assert_eq!(exit, Some(0));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn each_warning_rule_blocks_once_set_to_error() {
    for (name, code) in WARNING_RULES {
        let dir = fixture(name, Some(&format!("[rules]\n{name} = \"error\"\n")));
        let (exit, diags) = run(&dir);
        assert_eq!(severity_of(&diags, code), vec!["error".to_string()], "{name}: {diags:?}");
        let others: Vec<&(String, String)> = diags.iter().filter(|(c, _)| c != code).collect();
        assert!(others.iter().all(|(_, s)| s == "warning"), "{name} raised only itself: {diags:?}");
        assert_eq!(exit, Some(1), "{name} = \"error\" must fail the run");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn an_internal_error_cannot_be_configured() {
    // R0901 is always an error and always reported; a level for it parses
    // and changes nothing, in either direction.
    for level in ["off", "warn", "error"] {
        let dir = fixture(&format!("ice-{level}"), Some(&format!("[rules]\ninternal-error = \"{level}\"\n")));
        let (exit, diags) = run(&dir);
        assert_eq!(diags.len(), WARNING_RULES.len(), "{level}: {diags:?}");
        assert_eq!(exit, Some(0), "{level}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
