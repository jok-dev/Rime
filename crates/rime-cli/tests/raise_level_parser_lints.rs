//! End-to-end, for the rules that need the parser's help (Selene's
//! `parenthese_conditions` and `bad_string_escape`, `unknown_function_attribute`
//! and `...` outside a variadic function): the two that are warnings fail
//! `rime check` once rime.toml sets them to `"error"`, and the ones that
//! are errors fail it as they are. Same shape as `raise_level.rs`.

use std::path::PathBuf;
use std::process::Command;

const WARNINGS: &str = "local ready = true\n\
                        if (ready) then\n\
                        \tprint(\"it\\'s ready\")\n\
                        end\n";

const WARNING_RULES: [(&str, &str); 2] = [
    ("parenthesized-condition", "R0706"),
    ("needless-quote-escape", "R0707"),
];

/// One file per rule, each with one finding that is an error by default.
const ERRORS: [(&str, &str, &str); 4] = [
    ("undefined-variable", "R0022", "local function f()\n\treturn ...\nend\nreturn f\n"),
    ("unknown-function-attribute", "R0044", "@inline\nlocal function f() end\nreturn f\n"),
    ("bad-string-escape", "R0611", "return \"\\300\"\n"),
    ("bad-string-escape", "R0611", "return \"C:\\Users\"\n"),
];

fn fixture(name: &str, source: &str, toml: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rime-raise-parser-{name}-{}", std::process::id()));
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
fn the_two_warning_rules_do_not_fail_the_run_by_default() {
    let dir = fixture("default", WARNINGS, None);
    let (exit, diags) = run(&dir);
    let expected: Vec<(String, String)> =
        WARNING_RULES.iter().map(|(_, code)| (code.to_string(), "warning".to_string())).collect();
    assert_eq!(diags, expected);
    assert_eq!(exit, Some(0));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn each_warning_rule_blocks_once_set_to_error() {
    for (name, code) in WARNING_RULES {
        let dir = fixture(name, WARNINGS, Some(&format!("[rules]\n{name} = \"error\"\n")));
        let (exit, diags) = run(&dir);
        for (c, severity) in &diags {
            assert_eq!(severity, if c == code { "error" } else { "warning" }, "{name}: {diags:?}");
        }
        assert_eq!(diags.len(), WARNING_RULES.len(), "{name}: {diags:?}");
        assert_eq!(exit, Some(1), "{name} = \"error\" must fail the run");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn each_error_rule_blocks_by_default_and_can_be_turned_off() {
    for (i, (name, code, source)) in ERRORS.into_iter().enumerate() {
        let dir = fixture(&format!("error-{i}"), source, None);
        let (exit, diags) = run(&dir);
        assert_eq!(diags, vec![(code.to_string(), "error".to_string())], "{name}");
        assert_eq!(exit, Some(1), "{name}");
        std::fs::remove_dir_all(&dir).ok();

        let dir = fixture(&format!("off-{i}"), source, Some(&format!("[rules]\n{name} = \"off\"\n")));
        let (exit, diags) = run(&dir);
        assert_eq!(diags, vec![], "{name} = \"off\"");
        assert_eq!(exit, Some(0), "{name} = \"off\"");
        std::fs::remove_dir_all(&dir).ok();
    }
}