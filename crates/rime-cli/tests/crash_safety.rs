//! End-to-end: a crash inside Rime while it works on one file (or one
//! whole-project rule) is reported as R0901 against what it was doing, the
//! rest of the run still completes, and the exit status is 101 — never 0,
//! and distinct from the 1 that means "findings".
//!
//! The crashes are injected with `RIME_INTERNAL_FAULT` (see
//! `rime_check::guard::fault`), one site per stage of the pipeline.

use std::path::PathBuf;
use std::process::Command;

struct Project {
    dir: PathBuf,
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|e| panic!("stdout must be JSON: {e}\nstdout: {}\nstderr: {}", self.stdout, self.stderr))
    }
}

/// `(path, code)` of every diagnostic, in output order.
fn found(v: &serde_json::Value) -> Vec<(String, String)> {
    v["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let path = d["path"].as_str().unwrap().replace('\\', "/");
            (path, d["code"].as_str().unwrap().to_string())
        })
        .collect()
}

fn has(v: &serde_json::Value, path: &str, code: &str) -> bool {
    found(v).iter().any(|(p, c)| p == path && c == code)
}

impl Project {
    /// Two files: `boom.luau` is clean (so anything reported on it is the
    /// injected crash), `other.luau` has one R0022 that must survive.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rime-crash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = Project { dir };
        p.file("boom.luau", "local M = {}\nfunction M.go() return 1 end\nreturn M\n");
        p.file("other.luau", "print(definitely_undefined_name)\n");
        p
    }

    fn file(&self, rel: &str, src: &str) -> &Self {
        let path = self.dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, src).unwrap();
        self
    }

    fn run(&self, fault: Option<&str>, args: &[&str]) -> Run {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rime"));
        cmd.arg("check").args(args).current_dir(&self.dir);
        // The test process's own environment must not leak a fault in.
        cmd.env_remove("RIME_INTERNAL_FAULT").env_remove("RUST_BACKTRACE");
        if let Some(fault) = fault {
            cmd.env("RIME_INTERNAL_FAULT", fault);
        }
        let out = cmd.output().expect("spawn rime");
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }

    fn json(&self, fault: Option<&str>) -> Run {
        self.run(fault, &["--format", "json", "."])
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

#[test]
fn without_a_crash_the_exit_status_is_the_findings_one() {
    let p = Project::new("baseline");
    let run = p.json(None);
    assert_eq!(run.code, 1, "one R0022 is an error-severity finding: {}", run.stderr);
    let v = run.json();
    assert_eq!(found(&v), vec![("other.luau".to_string(), "R0022".to_string())]);
    assert_eq!(v["summary"]["internalErrors"], 0);
    assert!(!run.stderr.contains("internal error"), "{}", run.stderr);

    std::fs::remove_file(p.dir.join("other.luau")).unwrap();
    let clean = p.json(None);
    assert_eq!(clean.code, 0, "{}", clean.stderr);
}

/// Every per-file stage: the crash lands on `boom.luau` as R0901, the
/// finding in `other.luau` is still reported, and the run exits 101.
#[test]
fn a_crash_in_any_per_file_stage_is_reported_against_that_file() {
    // Only `parse` faults inside its phase; the others fault at the guard
    // itself, before the work starts (a rule's phase is covered below).
    for (site, doing, phase) in [
        ("parse", "parsing", Some("parse")),
        ("interface", "extracting the interface of", None),
        ("check", "checking", None),
        ("exports", "checking the exports of", None),
        ("finalize", "applying the allow directives of", None),
    ] {
        let p = Project::new(&format!("stage-{site}"));
        let run = p.json(Some(&format!("{site}:boom")));
        let v = run.json();
        assert_eq!(run.code, 101, "{site}: exit status, stderr: {}", run.stderr);
        assert!(has(&v, "boom.luau", "R0901"), "{site}: no R0901 on boom.luau: {v}");
        assert!(has(&v, "other.luau", "R0022"), "{site}: the other file's finding was lost: {v}");
        assert!(!has(&v, "other.luau", "R0901"), "{site}: the crash leaked onto another file: {v}");

        let d = v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["code"] == "R0901")
            .unwrap();
        assert_eq!(d["rule"], "internal-error", "{site}");
        assert_eq!(d["severity"], "error", "{site}");
        assert_eq!(
            d["message"],
            format!("internal error: Rime crashed while {doing} this file"),
            "{site}"
        );
        let notes: Vec<&str> = d["notes"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
        assert!(
            notes[0].starts_with("panicked at ") && notes[0].ends_with(&format!("injected fault at `{site}` (RIME_INTERNAL_FAULT)")),
            "{site}: first note names where and what: {notes:?}"
        );
        assert!(notes[0].contains("guard.rs:"), "{site}: the panic location is carried: {notes:?}");
        if let Some(phase) = phase {
            assert!(notes.contains(&format!("while running `{phase}`").as_str()), "{site}: {notes:?}");
        }
        assert!(notes.iter().any(|n| n.contains("bug in Rime")), "{site}: {notes:?}");

        assert_eq!(v["summary"]["internalErrors"], 1, "{site}");
        assert_eq!(v["summary"]["checked"], 2, "{site}: both files are accounted for");
        assert!(
            run.stderr.contains("1 internal error (R0901)") && run.stderr.contains("status 101"),
            "{site}: stderr says why the status is 101: {}",
            run.stderr
        );
        assert!(
            !run.stderr.contains("thread '") && !run.stderr.contains("RUST_BACKTRACE=1 environment"),
            "{site}: a contained crash prints the diagnostic, not the raw panic: {}",
            run.stderr
        );
    }
}

#[test]
fn a_crash_inside_a_rule_names_the_rule() {
    // `check` faults before any rule runs; a real crash is inside one.
    // `phase:<name>` faults inside the named rule, on every file.
    let p = Project::new("phase");
    let run = p.json(Some("phase:rule: narrowing"));
    let v = run.json();
    assert_eq!(run.code, 101, "{}", run.stderr);
    let crashes: Vec<&serde_json::Value> =
        v["diagnostics"].as_array().unwrap().iter().filter(|d| d["code"] == "R0901").collect();
    assert_eq!(crashes.len(), 2, "one per file the rule crashed on: {v}");
    for d in crashes {
        assert_eq!(d["message"], "internal error: Rime crashed while checking this file");
        assert!(d["notes"].as_array().unwrap().iter().any(|n| n == "while running `rule: narrowing`"), "{d}");
    }
    // The findings of a file whose check crashed part-way are not known,
    // so none are shown for it: half a result would read as a whole one.
    assert!(!has(&v, "other.luau", "R0022"), "{v}");
}

#[test]
fn a_crash_in_a_whole_project_rule_fails_the_run_without_a_file() {
    let p = Project::new("cycles");
    let run = p.json(Some("cycles"));
    let v = run.json();
    assert_eq!(run.code, 101, "{}", run.stderr);
    // Per-file results are intact; the crash belongs to no file.
    assert_eq!(found(&v), vec![("other.luau".to_string(), "R0022".to_string())]);
    assert_eq!(v["summary"]["internalErrors"], 1);
    assert!(
        run.stderr.contains("rime: error[R0901]: internal error: Rime crashed while searching the project for require cycles"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("circular-require (R0509) reported nothing in this run"), "{}", run.stderr);
    assert!(!run.stderr.contains("with this file"), "no file to send: {}", run.stderr);
}

#[test]
fn every_crashing_file_gets_its_own_report() {
    // One diagnostic per occurrence (CLAUDE.md): two files, two R0901s.
    let p = Project::new("each");
    p.file("boom2.luau", "return 2\n");
    let run = p.json(Some("check:boom"));
    let v = run.json();
    assert_eq!(run.code, 101);
    assert!(has(&v, "boom.luau", "R0901") && has(&v, "boom2.luau", "R0901"), "{v}");
    assert!(has(&v, "other.luau", "R0022"), "{v}");
    assert_eq!(v["summary"]["internalErrors"], 2);
    assert!(run.stderr.contains("2 internal errors (R0901)"), "{}", run.stderr);
}

#[test]
fn a_crash_while_rendering_keeps_the_finding() {
    let p = Project::new("render");
    for args in [&["--format", "json", "."][..], &["--plain", "."][..], &["."][..]] {
        let run = p.run(Some("render:other"), args);
        assert_eq!(run.code, 101, "{args:?}: {}", run.stderr);
        assert!(run.stdout.contains("R0022"), "{args:?}: the finding itself is still shown: {}", run.stdout);
        assert!(run.stdout.contains("R0901"), "{args:?}: {}", run.stdout);
        assert!(
            run.stdout.contains("Rime crashed while rendering a diagnostic for this file"),
            "{args:?}: {}",
            run.stdout
        );
    }
    let v = p.json(Some("render:other")).json();
    assert!(has(&v, "other.luau", "R0022") && has(&v, "other.luau", "R0901"), "{v}");
    let finding = v["diagnostics"].as_array().unwrap().iter().find(|d| d["code"] == "R0022").unwrap();
    assert!(finding["labels"].as_array().unwrap().is_empty(), "shown without the spans that broke: {finding}");
}

#[test]
fn a_crash_outside_any_file_boundary_still_exits_101() {
    let p = Project::new("fatal");
    let run = p.json(Some("run"));
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert!(run.stdout.is_empty(), "no partial JSON document: {}", run.stdout);
    assert!(
        run.stderr.contains("rime: error[R0901]: internal error: Rime crashed while running the check"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("injected fault at `run`"), "{}", run.stderr);
    assert!(run.stderr.contains("the run did not finish"), "{}", run.stderr);
}

#[test]
fn plain_text_output_carries_the_report_too() {
    let p = Project::new("plain");
    let run = p.run(Some("check:boom"), &["--plain", "."]);
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert!(
        run.stdout.contains("boom.luau: error[R0901]: internal error: Rime crashed while checking this file"),
        "{}",
        run.stdout
    );
    assert!(run.stdout.contains("other.luau: error[R0022]"), "{}", run.stdout);
    // The default (ariadne) renderer copes with a diagnostic that has no span.
    let fancy = p.run(Some("check:boom"), &["."]);
    assert_eq!(fancy.code, 101, "{}", fancy.stderr);
    // The default (ariadne) renderer drops the path and notes of a
    // diagnostic with no span, so the crash report must not go through it.
    assert!(
        fancy.stdout.contains("boom.luau: error[R0901]: internal error: Rime crashed while checking this file"),
        "{}",
        fancy.stdout
    );
    assert!(fancy.stdout.contains("injected fault at `check`"), "{}", fancy.stdout);
}

/// Nothing that silences findings silences a crash: not a rule level, not
/// `--only`, not an inline allow.
#[test]
fn config_only_and_inline_allows_cannot_hide_a_crash() {
    let p = Project::new("nohide");
    p.file(
        "rime.toml",
        "[rules]\ninternal-error = \"off\"\nundefined-variable = \"off\"\n[conservative]\ndefault = \"off\"\n",
    );
    p.file(
        "boom.luau",
        "-- rime: allow-file(internal-error, \"trying to hide it\")\nlocal M = {}\nreturn M\n",
    );
    for args in [&["--format", "json", "."][..], &["--format", "json", "--only", "unused-local", "."][..]] {
        let run = p.run(Some("check:boom"), args);
        let v = run.json();
        assert_eq!(run.code, 101, "{args:?}: {}", run.stderr);
        assert_eq!(found(&v), vec![("boom.luau".to_string(), "R0901".to_string())], "{args:?}");
    }
    // The allow directive is applied by a later stage than `check`; make
    // sure the stages that run before it are covered as well.
    for site in ["parse", "interface", "exports"] {
        let run = p.json(Some(&format!("{site}:boom")));
        assert_eq!(run.code, 101, "{site}: {}", run.stderr);
        assert!(has(&run.json(), "boom.luau", "R0901"), "{site}");
    }
}

/// Project mode checks every mounted file, then filters reports down to
/// what was asked for. A crash on a filtered-out file still changes what
/// the reported files were checked against, so it is reported regardless.
#[test]
fn a_crash_in_an_ignored_or_unrequested_file_is_still_reported() {
    let p = Project::new("filtered");
    p.file(
        "default.project.json",
        r#"{ "name": "t", "tree": { "$className": "DataModel", "ReplicatedStorage": { "$path": "src" } } }"#,
    );
    p.file("rime.toml", "ignore = [\"src/vendor\"]\n");
    p.file("src/vendor/boom.luau", "local unused_in_vendor = 1\nreturn {}\n");
    p.file("src/app/main.luau", "print(definitely_undefined_name)\nreturn {}\n");
    p.file("src/lib/quiet.luau", "return {}\n");

    // Sanity: without a fault, the ignored file's own findings stay out.
    let v = p.run(None, &["--format", "json", "src"]).json();
    assert!(found(&v).iter().all(|(path, _)| !path.contains("vendor")), "{v}");

    // Ignored by rime.toml.
    let run = p.run(Some("check:vendor"), &["--format", "json", "src"]);
    let v = run.json();
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert!(has(&v, "src/vendor/boom.luau", "R0901"), "{v}");
    assert!(!has(&v, "src/vendor/boom.luau", "R0013"), "its findings are still ignored: {v}");
    assert!(has(&v, "src/app/main.luau", "R0022"), "{v}");

    // Outside the requested path (only `src/lib` was asked for).
    let run = p.run(Some("check:main.luau"), &["--format", "json", "src/lib"]);
    let v = run.json();
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert!(has(&v, "src/app/main.luau", "R0901"), "{v}");
    assert!(!has(&v, "src/app/main.luau", "R0022"), "its findings were not asked for: {v}");
}

/// A crashed file contributed no usage, so every export it uses would look
/// unused. Like a parse failure, that turns R0309 off rather than lying.
#[test]
fn a_crashed_consumer_does_not_turn_into_unused_export_reports() {
    let p = Project::new("usage");
    p.file(
        "default.project.json",
        r#"{ "name": "t", "tree": { "$className": "DataModel", "ReplicatedStorage": { "$path": "src" } } }"#,
    );
    // R0309 only speaks about a module some checked file requires, so a
    // second, healthy consumer (`Main`) keeps `Lib` in its view while the
    // only reader of `Lib.used` (`User`) is the file that crashes.
    p.file(
        "src/Lib.luau",
        "local Lib = {}\nfunction Lib.used() return 1 end\nfunction Lib.also() return 2 end\nreturn Lib\n",
    );
    p.file(
        "src/User.luau",
        "local Lib = require(script.Parent.Lib)\nlocal User = {}\nfunction User.run() return Lib.used() end\nreturn User\n",
    );
    p.file(
        "src/Main.luau",
        "local Lib = require(script.Parent.Lib)\nlocal User = require(script.Parent.User)\nUser.run()\nLib.also()\n",
    );

    let healthy = p.run(None, &["--format", "json", "src"]).json();
    assert!(!has(&healthy, "src/Lib.luau", "R0309"), "fixture: `used` is used: {healthy}");

    let run = p.run(Some("check:User.luau"), &["--format", "json", "src"]);
    let v = run.json();
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert!(has(&v, "src/User.luau", "R0901"), "{v}");
    assert!(!has(&v, "src/Lib.luau", "R0309"), "a crash must not invent an unused export: {v}");
}

#[test]
fn a_crash_in_the_custom_rule_pass_is_contained() {
    let p = Project::new("custom");
    p.file("rime.toml", "");
    p.file(
        "rime-rules/no-foo.luau",
        "return { files = \"**/*.luau\", check = function(file, ctx) end }\n",
    );
    let run = p.json(Some("custom:boom"));
    let v = run.json();
    assert_eq!(run.code, 101, "{}", run.stderr);
    assert!(has(&v, "boom.luau", "R0901"), "{v}");
    assert!(has(&v, "other.luau", "R0022"), "core findings are unaffected: {v}");
    let d = v["diagnostics"].as_array().unwrap().iter().find(|d| d["code"] == "R0901").unwrap();
    assert_eq!(d["message"], "internal error: Rime crashed while running the project's custom rules on this file");
}
