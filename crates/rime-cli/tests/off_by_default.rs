//! End-to-end: a rule on `rime_diag::OFF_BY_DEFAULT` reports nothing until
//! rime.toml gives it a level, or `--only` names it. Each such rule checks a
//! convention a project has to choose, so a project that never asked for it
//! hears nothing about it, and the summary does not count it as ignored.

use std::path::PathBuf;
use std::process::Command;

/// (rule, code, severity once on, a file with exactly one finding of it).
/// The file is `src/case.luau` of a small Rojo project.
const CASES: [(&str, &str, &str, &str); 5] = [
    (
        "mixed-table",
        "R0703",
        "warning",
        "local t = { 1, 2, kind = \"list\" }\nreturn t\n",
    ),
    (
        "pcall-discarded",
        "R0110",
        "error",
        "local function save(f: () -> ())\n\tpcall(f)\nend\nreturn save\n",
    ),
    (
        "exported-function-reassign",
        "R0305",
        "error",
        "local case = {}\nfunction case.run() end\ncase.run = function() end\nreturn case\n",
    ),
    (
        "module-name-mismatch",
        "R0310",
        "warning",
        "local Cart = {}\nreturn Cart\n",
    ),
    (
        "enumeration-require",
        "R0508",
        "error",
        "local list = {}\nfor _, child in script:GetChildren() do\n\ttable.insert(list, require(child))\nend\nreturn #list\n",
    ),
];

/// A project file that mounts `src`. Rules that read the project graph
/// (requires, module names) run only inside a Rojo project.
const PROJECT: &str = r#"{ "name": "case", "tree": { "$className": "DataModel", "ReplicatedStorage": { "$className": "ReplicatedStorage", "Shared": { "$path": "src" } } } }"#;

fn fixture(name: &str, source: &str, toml: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rime-off-by-default-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("default.project.json"), PROJECT).unwrap();
    std::fs::write(dir.join("src").join("case.luau"), source).unwrap();
    if let Some(toml) = toml {
        std::fs::write(dir.join("rime.toml"), toml).unwrap();
    }
    dir
}

/// Exit code, each diagnostic as (code, severity), and the ignored count.
fn run(dir: &PathBuf, extra: &[&str]) -> (Option<i32>, Vec<(String, String)>, u64) {
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json"])
        .args(extra)
        .arg("src")
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
    (out.status.code(), diags, v["summary"]["ignored"].as_u64().unwrap())
}

#[test]
fn silent_with_no_config_and_not_counted_as_ignored() {
    for (name, _, _, source) in CASES {
        let dir = fixture(&format!("none-{name}"), source, None);
        let (exit, diags, ignored) = run(&dir, &[]);
        assert_eq!(diags, Vec::<(String, String)>::new(), "{name}");
        assert_eq!(exit, Some(0), "{name}");
        assert_eq!(ignored, 0, "{name}: nobody turned it off, so nothing was ignored");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn a_level_in_rime_toml_turns_it_on() {
    for (name, code, _, source) in CASES {
        for (level, severity, exit_code) in [("warn", "warning", 0), ("error", "error", 1)] {
            let toml = format!("[rules]\n{name} = \"{level}\"\n");
            let dir = fixture(&format!("{level}-{name}"), source, Some(&toml));
            let (exit, diags, _) = run(&dir, &[]);
            assert_eq!(diags, vec![(code.to_string(), severity.to_string())], "{name} = {level}");
            assert_eq!(exit, Some(exit_code), "{name} = {level}");
            std::fs::remove_dir_all(&dir).ok();
        }
    }
}

#[test]
fn only_names_it_and_it_reports_at_its_own_severity() {
    for (name, code, severity, source) in CASES {
        let dir = fixture(&format!("only-{name}"), source, None);
        let (_, diags, _) = run(&dir, &["--only", name]);
        assert_eq!(diags, vec![(code.to_string(), severity.to_string())], "{name}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn an_explicit_off_is_counted_like_any_other_off() {
    for (name, _, _, source) in CASES {
        let toml = format!("[rules]\n{name} = \"off\"\n");
        let dir = fixture(&format!("off-{name}"), source, Some(&toml));
        let (exit, diags, ignored) = run(&dir, &[]);
        assert_eq!(diags, Vec::<(String, String)>::new(), "{name}");
        assert_eq!(exit, Some(0), "{name}");
        assert_eq!(ignored, 1, "{name}: the project turned it off, and the summary says so");
        std::fs::remove_dir_all(&dir).ok();
    }
}
