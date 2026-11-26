//! End-to-end: `rime-rules/*.luau` beside rime.toml run during `rime check`
//! and report through the normal JSON output. Projects are generated per
//! test in a temp dir (same convention as json_format.rs).

use std::path::{Path, PathBuf};
use std::process::Command;

// The example rules shipped in `examples/rime-rules/` are the fixtures here,
// so the examples stay verified by the same tests that pin the engine.
const WRAPPER_BAN: &str = include_str!("../../../examples/rime-rules/no-direct-destroy.luau");
const MODULE_TAG: &str = include_str!("../../../examples/rime-rules/module-tag.luau");
const CMDR_COMMAND_NAME: &str = include_str!("../../../examples/rime-rules/cmdr-command-name.luau");

const THROWS: &str = r#"
return { files = "src/**/*.lua", check = function(file, ctx) error("boom") end }
"#;

struct Project {
    dir: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rime-custom-e2e-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("rime-rules")).unwrap();
        std::fs::create_dir_all(dir.join("src").join("Tagged")).unwrap();
        std::fs::write(dir.join("rime.toml"), "").unwrap();
        Project { dir }
    }
    fn rule(&self, name: &str, src: &str) -> &Self {
        std::fs::write(self.dir.join("rime-rules").join(name), src).unwrap();
        self
    }
    fn file(&self, rel: &str, src: &str) -> &Self {
        let p = self.dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
        self
    }
    fn toml(&self, src: &str) -> &Self {
        std::fs::write(self.dir.join("rime.toml"), src).unwrap();
        self
    }
    fn check(&self, extra: &[&str]) -> (i32, serde_json::Value, String) {
        let mut args = vec!["check", "--format", "json", "src"];
        args.extend_from_slice(extra);
        let out = Command::new(env!("CARGO_BIN_EXE_rime"))
            .args(&args)
            .current_dir(&self.dir)
            .output()
            .expect("spawn rime");
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("stdout must be JSON: {e}\nstderr: {stderr}"));
        (out.status.code().unwrap_or(-1), v, stderr)
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn diags_with_rule<'a>(v: &'a serde_json::Value, rule: &str) -> Vec<&'a serde_json::Value> {
    v["diagnostics"].as_array().unwrap().iter().filter(|d| d["rule"] == rule).collect()
}

#[test]
fn wrapper_ban_fires_by_name_and_honours_inline_allow() {
    let p = Project::new("wrapper");
    p.rule("no-direct-destroy.luau", WRAPPER_BAN)
        .file("src/A.lua", "local pooledPart = workspace.Part\npooledPart:Destroy()\n")
        .file("src/B.lua", "local pooledPart = workspace.Part\npooledPart:Destroy() -- rime: allow(no-direct-destroy, \"cleanup path\")\n")
        .file("src/Pools/Pool.lua", "local pooledPart = workspace.Part\npooledPart:Destroy()\n");
    let (code, v, stderr) = p.check(&[]);
    assert!(code == 0 || code == 1, "exit {code}: {stderr}");
    let hits = diags_with_rule(&v, "no-direct-destroy");
    assert_eq!(hits.len(), 1, "{v}");
    let d = hits[0];
    assert_eq!(d["severity"], "warning");
    assert!(d["path"].as_str().unwrap().ends_with("A.lua"));
    assert_eq!(d["labels"][0]["start"]["line"], 2);
    assert!(d["code"].as_str().unwrap().starts_with("R9"), "{}", d["code"]);
    assert!(v["summary"]["ignored"].as_u64().unwrap() >= 1, "the allow counts as ignored: {v}");
}

#[test]
fn module_tag_absence_check_flags_only_the_untagged_module() {
    let p = Project::new("tag");
    p.rule("module-tag.luau", MODULE_TAG)
        .file("src/Tagged/Good.lua", "local Good = {}\nGood.Tag = \"good\"\nreturn Good\n")
        .file("src/Tagged/Bad.lua", "local Bad = {}\nreturn Bad\n")
        .file("src/Elsewhere.lua", "local E = {}\nreturn E\n");
    let (_, v, _) = p.check(&[]);
    let hits = diags_with_rule(&v, "module-tag");
    assert_eq!(hits.len(), 1, "{v}");
    assert!(hits[0]["path"].as_str().unwrap().ends_with("Bad.lua"));
    assert_eq!(hits[0]["severity"], "error");
}

#[test]
fn cmdr_command_name_example_flags_only_definitions_without_name() {
    let p = Project::new("cmdr");
    p.rule("cmdr-command-name.luau", CMDR_COMMAND_NAME)
        .file(
            "src/Server/Admin/Commands/inline.lua",
            "return {\n\tName = \"inline\",\n\tArgs = {},\n}\n",
        )
        .file(
            "src/Server/Admin/Commands/viaLocal.lua",
            "local command = {\n\tName = \"viaLocal\",\n}\nreturn command\n",
        )
        .file(
            "src/Server/Admin/Commands/nameless.lua",
            "return {\n\tDescription = \"no name\",\n}\n",
        )
        .file(
            "src/Server/Admin/Commands/namelessServer.lua",
            "return function(context)\n\treturn context\nend\n",
        );
    let (_, v, _) = p.check(&[]);
    let hits = diags_with_rule(&v, "cmdr-command-name");
    assert_eq!(hits.len(), 1, "{v}");
    assert!(hits[0]["path"].as_str().unwrap().ends_with("nameless.lua"), "{v}");
    assert!(diags_with_rule(&v, "custom-rule-error").is_empty(), "{v}");
}

#[test]
fn broken_and_throwing_rules_report_r0025_and_do_not_stop_others() {
    let p = Project::new("broken");
    p.rule("a-broken.luau", "return { files = \"src/**\" }")
        .rule("b-throws.luau", THROWS)
        .rule("c-good.luau", WRAPPER_BAN)
        .file("src/A.lua", "local pooledPart = workspace.Part\npooledPart:Destroy()\n");
    let (code, v, _) = p.check(&[]);
    assert_eq!(code, 1, "R0025 is an error");
    let errors = diags_with_rule(&v, "custom-rule-error");
    assert_eq!(errors.len(), 2, "{v}");
    assert!(errors.iter().any(|d| d["path"].as_str().unwrap().ends_with("a-broken.luau")));
    assert!(errors.iter().any(|d| d["path"].as_str().unwrap().ends_with("b-throws.luau") && d["message"].as_str().unwrap().contains("src/A.lua")));
    assert_eq!(diags_with_rule(&v, "c-good").len(), 1, "the good rule still ran: {v}");
}

#[test]
fn throwing_rule_over_two_files_reports_once() {
    let p = Project::new("throw-collapse");
    p.rule("b-throws.luau", THROWS)
        .file("src/A.lua", "print(1)\n")
        .file("src/B.lua", "print(2)\n");
    let (code, v, _) = p.check(&[]);
    assert_eq!(code, 1, "R0025 is an error");
    let errors: Vec<_> = diags_with_rule(&v, "custom-rule-error")
        .into_iter()
        .filter(|d| d["message"].as_str().unwrap().contains("b-throws"))
        .collect();
    assert_eq!(errors.len(), 1, "one bug, one report across both files: {v}");
    let notes = errors[0]["notes"].as_array().expect("notes");
    assert!(
        notes.iter().any(|n| n.as_str().unwrap().contains("2 files")),
        "the note counts the files: {notes:?}"
    );
}

#[test]
fn no_custom_rules_flag_skips_everything() {
    let p = Project::new("flag");
    p.rule("a-broken.luau", "return 1")
        .rule("no-direct-destroy.luau", WRAPPER_BAN)
        .toml("[rules]\nno-direct-destroy = \"off\"\n")
        .file("src/A.lua", "local pooledPart = workspace.Part\npooledPart:Destroy()\n");
    let (_, v, stderr) = p.check(&["--no-custom-rules"]);
    assert!(!stderr.contains("unknown rule"), "names still register even when execution is skipped: {stderr}");
    assert!(diags_with_rule(&v, "custom-rule-error").is_empty());
    assert!(diags_with_rule(&v, "no-direct-destroy").is_empty());
}

#[test]
fn rime_toml_can_turn_a_custom_rule_off() {
    let p = Project::new("toml");
    p.rule("no-direct-destroy.luau", WRAPPER_BAN)
        .toml("[rules]\nno-direct-destroy = \"off\"\n")
        .file("src/A.lua", "local pooledPart = workspace.Part\npooledPart:Destroy()\n");
    let (code, v, stderr) = p.check(&[]);
    assert!(!stderr.contains("unknown rule"), "config must accept the custom name: {stderr}");
    assert!(diags_with_rule(&v, "no-direct-destroy").is_empty(), "{v}");
    assert_eq!(code, 0);
}

#[test]
fn broken_rule_named_in_rime_toml_does_not_abort() {
    let p = Project::new("broken-named");
    p.rule("a-broken.luau", "return 1")
        .toml("[rules]\na-broken = \"error\"\n")
        .file("src/A.lua", "print(1)\n");
    let (code, v, stderr) = p.check(&[]);
    assert!(!stderr.contains("unknown rule"), "a broken rule still registers its stem: {stderr}");
    assert_eq!(diags_with_rule(&v, "custom-rule-error").len(), 1, "{v}");
    assert_eq!(code, 1);
}

#[test]
fn config_flag_with_bare_filename_still_runs_rules() {
    let p = Project::new("bare-config");
    p.rule("no-direct-destroy.luau", WRAPPER_BAN)
        .file("src/A.lua", "local pooledPart = workspace.Part\npooledPart:Destroy()\n");
    let (_, v, stderr) = p.check(&["--config", "rime.toml"]);
    let hits = diags_with_rule(&v, "no-direct-destroy");
    assert_eq!(hits.len(), 1, "a bare --config filename must not blank out the rules root: {v}\nstderr: {stderr}");
}

#[test]
fn timings_list_each_rule() {
    let p = Project::new("timings");
    p.rule("no-direct-destroy.luau", WRAPPER_BAN)
        .file("src/A.lua", "print(1)\n");
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--timings", "src"])
        .current_dir(&p.dir)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("custom: no-direct-destroy"), "{stderr}");
    assert!(stderr.contains("load custom rules"), "{stderr}");
}

// A project with no rime.toml or rime-rules/ at its root: the config and the
// rules live under tools/, as in a repo that keeps lint tooling in a subfolder.
fn subfolder_project(name: &str, toml: &str) -> Project {
    let p = Project::new(name);
    std::fs::remove_file(p.dir.join("rime.toml")).unwrap();
    std::fs::remove_dir_all(p.dir.join("rime-rules")).unwrap();
    p.file("tools/rime.toml", toml)
        .file("tools/rime-rules/no-direct-destroy.luau", WRAPPER_BAN)
        .file("src/A.lua", "local pooledPart = workspace.Part
pooledPart:Destroy()
");
    p
}

fn run(p: &Project, args: &[&str]) -> (i32, serde_json::Value, String) {
    run_in(p, "", args)
}

/// `run`, started in `sub` under the project instead of its top.
fn run_in(p: &Project, sub: &str, args: &[&str]) -> (i32, serde_json::Value, String) {
    let mut all = vec!["check", "--format", "json"];
    all.extend_from_slice(args);
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(&all)
        .current_dir(p.dir.join(sub))
        .output()
        .expect("spawn rime");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let v = serde_json::from_slice(&out.stdout).unwrap_or(serde_json::Value::Null);
    (out.status.code().unwrap_or(-1), v, stderr)
}

#[test]
fn custom_rules_root_lets_a_subfolder_config_check_project_files() {
    let p = subfolder_project("root-set", "[custom-rules]
root = \"..\"
");
    let (_, v, stderr) = run(&p, &["src", "--config", "tools/rime.toml"]);
    assert_eq!(diags_with_rule(&v, "no-direct-destroy").len(), 1, "{v}
stderr: {stderr}");
    assert!(!stderr.contains("did not run on"), "every file is under the root: {stderr}");
}

#[test]
fn without_a_root_a_subfolder_config_says_which_files_rules_skipped() {
    let p = subfolder_project("root-unset", "");
    let (_, v, stderr) = run(&p, &["src", "--config", "tools/rime.toml"]);
    assert!(diags_with_rule(&v, "no-direct-destroy").is_empty(), "{v}");
    assert!(stderr.contains("custom rules did not run on 1 file outside"), "{stderr}");
    assert!(stderr.contains("[custom-rules] root"), "the note says how to fix it: {stderr}");
}

#[test]
fn custom_rules_root_gives_each_file_one_path() {
    // game/src/A.lua is `game/src/A.lua` under the root, never `src/A.lua`,
    // so a rule for src/ does not also fire on game/src/.
    let p = subfolder_project("root-unique", "[custom-rules]
root = \"..\"
");
    p.file("game/src/A.lua", "local pooledPart = workspace.Part
pooledPart:Destroy()
");
    let (_, v, stderr) = run(&p, &["src", "game", "--config", "tools/rime.toml"]);
    let hits = diags_with_rule(&v, "no-direct-destroy");
    assert_eq!(hits.len(), 1, "{v}
stderr: {stderr}");
    let file = hits[0]["path"].as_str().expect("a diagnostic has a path");
    assert!(!file.replace('\\', "/").contains("game/"), "the hit is root src/A.lua: {v}");
}

#[test]
fn custom_rules_root_that_is_not_a_directory_is_an_error() {
    let p = subfolder_project("root-missing", "[custom-rules]
root = \"nope\"
");
    let (code, _, stderr) = run(&p, &["src", "--config", "tools/rime.toml"]);
    assert_ne!(code, 0);
    assert!(stderr.contains("[custom-rules] root `nope` is not a directory"), "{stderr}");
}

const DESTROY: &str = "local pooledPart = workspace.Part
pooledPart:Destroy()
";

#[test]
fn custom_rules_root_applies_to_a_config_found_by_walking_up() {
    // Started in tools/, rime.toml is found there, and `..` still reaches src/.
    let p = subfolder_project("root-walk-up", "[custom-rules]
root = \"..\"
");
    let (_, v, stderr) = run_in(&p, "tools", &["../src"]);
    assert_eq!(diags_with_rule(&v, "no-direct-destroy").len(), 1, "{v}
stderr: {stderr}");
    assert!(!stderr.contains("did not run on"), "{stderr}");
}

#[test]
fn custom_rules_root_applies_to_a_bare_config_flag() {
    let p = subfolder_project("root-bare-config", "[custom-rules]
root = \"..\"
");
    let (_, v, stderr) = run_in(&p, "tools", &["../src", "--config", "rime.toml"]);
    assert_eq!(diags_with_rule(&v, "no-direct-destroy").len(), 1, "{v}
stderr: {stderr}");
}

#[test]
fn the_note_names_the_starting_directory_as_dot() {
    // The base is tools/, which is where the run started: shown as `.`, not
    // as an empty name.
    let p = subfolder_project("root-note-cwd", "");
    let (_, _, stderr) = run_in(&p, "tools", &["../src"]);
    assert!(stderr.contains("1 file outside `.`,"), "{stderr}");
}

#[test]
fn the_note_names_a_root_narrower_than_the_config_directory() {
    let p = subfolder_project("root-narrow", "[custom-rules]
root = \"lib\"
");
    p.file("tools/lib/Keep.lua", "return {}
");
    let (_, v, stderr) = run(&p, &["src", "--config", "tools/rime.toml"]);
    assert!(diags_with_rule(&v, "no-direct-destroy").is_empty(), "{v}");
    let want = format!("1 file outside `{}`,", Path::new("tools").join("lib").display());
    assert!(stderr.contains(&want), "want {want:?}: {stderr}");
}

#[test]
fn the_note_counts_every_file_left_out() {
    let p = subfolder_project("root-note-plural", "");
    p.file("src/B.lua", DESTROY);
    let (_, _, stderr) = run(&p, &["src", "--config", "tools/rime.toml"]);
    assert!(stderr.contains("did not run on 2 files outside"), "{stderr}");
}

#[test]
fn no_note_when_no_custom_rule_could_report() {
    let p = subfolder_project("root-note-off", "");
    for args in [
        &["src", "--config", "tools/rime.toml", "--no-custom-rules"][..],
        &["src", "--config", "tools/rime.toml", "--only", "R0041"][..],
    ] {
        let (_, _, stderr) = run(&p, args);
        assert!(!stderr.contains("did not run on"), "{args:?}: {stderr}");
    }
    let p = subfolder_project("root-note-rule-off", "[rules]
no-direct-destroy = \"off\"
");
    let (_, _, stderr) = run(&p, &["src", "--config", "tools/rime.toml"]);
    assert!(!stderr.contains("did not run on"), "the only rule is off: {stderr}");
}

#[test]
fn the_note_counts_only_the_files_asked_for() {
    // In project mode src/B.lua is read for cross-file checking, but only
    // src/A.lua was asked for, so only it counts as left out.
    let p = subfolder_project("root-note-project", "");
    p.file(
        "default.project.json",
        r#"{ "name": "t", "tree": { "$className": "DataModel", "ReplicatedStorage": {
    "$className": "ReplicatedStorage", "Source": { "$path": "src" } } } }"#,
    )
    .file("src/B.lua", DESTROY);
    let (_, _, stderr) = run(&p, &["src/A.lua", "--config", "tools/rime.toml"]);
    assert!(stderr.contains("did not run on 1 file outside"), "{stderr}");
}
