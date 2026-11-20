//! End-to-end: `rime check --format json` emits schema v1 on stdout.
//! Deliberately loose about WHICH rules fire (rules drift; the format must
//! not), strict about the shape.

use std::process::Command;

#[test]
fn json_output_is_schema_v1() {
    let dir = std::env::temp_dir().join(format!("rime-json-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Undefined global read — reliably produces at least one diagnostic.
    std::fs::write(dir.join("case.luau"), "print(definitely_undefined_name)\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "case.luau"])
        .current_dir(&dir)
        .output()
        .expect("spawn rime");

    // 0 = clean, 1 = diagnostics found. Anything else is a real failure.
    let code = out.status.code();
    assert!(
        code == Some(0) || code == Some(1),
        "unexpected exit {code:?}, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON");
    assert_eq!(v["version"], 1);
    assert_eq!(v["summary"]["checked"], 1);

    let diags = v["diagnostics"].as_array().expect("diagnostics is an array");
    assert!(!diags.is_empty(), "expected >=1 diagnostic for an undefined global");
    for d in diags {
        let code = d["code"].as_str().expect("code is a string");
        assert!(code.starts_with('R') && code.len() == 5, "code format: {code}");
        let sev = d["severity"].as_str().unwrap();
        assert!(sev == "error" || sev == "warning");
        assert!(d["conservative"].is_boolean());
        let label = &d["labels"][0];
        assert!(label["primary"].is_boolean());
        assert!(label["start"]["line"].as_u64().unwrap() >= 1);
        assert!(label["start"]["column"].as_u64().unwrap() >= 1);
        assert!(label["span"]["start"].is_u64());
        assert!(d["path"].as_str().unwrap().contains("case.luau"));
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// An unused export's `extent` is its whole definition: the bytes a tool
/// deletes, and the lines it counts. The label still marks only the name.
#[test]
fn unused_export_carries_the_extent_of_its_definition() {
    let dir = std::env::temp_dir().join(format!("rime-json-extent-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("default.project.json"),
        r#"{ "name": "t", "tree": { "$className": "DataModel", "ReplicatedStorage": { "$path": "src" } } }"#,
    )
    .unwrap();
    let lib = "local Lib = {\n\tused = 1,\n\tspare = 2,\n}\n\nfunction Lib.dead(n: number): number\n\tlocal m = n\n\treturn m\nend\n\nreturn Lib\n";
    std::fs::write(dir.join("src/Lib.luau"), lib).unwrap();
    std::fs::write(
        dir.join("src/Main.luau"),
        "local Lib = require(script.Parent.Lib)\nprint(Lib.used)\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "--only", "unused-export", "src"])
        .current_dir(&dir)
        .output()
        .expect("spawn rime");
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON");
    let diags = v["diagnostics"].as_array().expect("diagnostics is an array");
    let extent_of = |member: &str| {
        let d = diags
            .iter()
            .find(|d| d["message"].as_str().unwrap().contains(&format!("`{member}`")))
            .unwrap_or_else(|| panic!("no report for `{member}`: {v}"));
        assert_eq!(d["code"], "R0309");
        d["extent"].clone()
    };
    let text = |e: &serde_json::Value| {
        let start = e["span"]["start"].as_u64().expect("extent.span.start") as usize;
        let end = e["span"]["end"].as_u64().expect("extent.span.end") as usize;
        lib[start..end].to_string()
    };

    let dead = extent_of("dead");
    assert_eq!(text(&dead), "function Lib.dead(n: number): number\n\tlocal m = n\n\treturn m\nend");
    assert_eq!(dead["start"], serde_json::json!({ "line": 6, "column": 1 }));
    assert_eq!(dead["end"], serde_json::json!({ "line": 9, "column": 4 }));

    let spare = extent_of("spare");
    assert_eq!(text(&spare), "spare = 2,");
    assert_eq!(spare["start"]["line"], 3);
    assert_eq!(spare["end"]["line"], 3);

    // Deleting both extents leaves a file with no unused export.
    let (d, s) = (&dead["span"], &spare["span"]);
    let mut rest = lib.to_string();
    rest.replace_range(d["start"].as_u64().unwrap() as usize..d["end"].as_u64().unwrap() as usize, "");
    rest.replace_range(s["start"].as_u64().unwrap() as usize..s["end"].as_u64().unwrap() as usize, "");
    std::fs::write(dir.join("src/Lib.luau"), &rest).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "src"])
        .current_dir(&dir)
        .output()
        .expect("spawn rime");
    let after: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON");
    assert_eq!(after["diagnostics"].as_array().unwrap().len(), 0, "after deletion: {after}");

    std::fs::remove_dir_all(&dir).ok();
}

/// In a project with a require Rime cannot follow, an unused export whose
/// name is also written as a string is still reported, and `stringMatches`
/// has every such line: the path written as a diagnostic's `path` is, the
/// line 1-based. A tool skips the report by looking at that key alone.
#[test]
fn unused_export_lists_the_strings_that_equal_its_name() {
    let dir = std::env::temp_dir().join(format!("rime-json-strings-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("default.project.json"),
        r#"{ "name": "t", "tree": { "$className": "DataModel", "ReplicatedStorage": { "$path": "src" } } }"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("src/Lib.luau"),
        "local Lib = {}\n\nLib.used = 1\n\nfunction Lib.refresh() end\n\nfunction Lib.plain() end\n\nreturn Lib\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/Main.luau"),
        "local Lib = require(script.Parent.Lib)\nprint(Lib.used)\nlocal hooks = { \"refresh\" }\nprint(hooks)\n",
    )
    .unwrap();
    // The require Rime cannot follow.
    std::fs::write(
        dir.join("src/Dynamic.luau"),
        "local target = nil\nlocal function load()\n\treturn require(target)\nend\nprint(load())\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "--only", "unused-export", "src"])
        .current_dir(&dir)
        .output()
        .expect("spawn rime");
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON");
    let diags = v["diagnostics"].as_array().expect("diagnostics is an array");
    let report = |member: &str| {
        diags
            .iter()
            .find(|d| d["message"].as_str().unwrap().contains(&format!("`{member}`")))
            .unwrap_or_else(|| panic!("no report for `{member}`: {v}"))
    };

    let refresh = report("refresh");
    assert_eq!(refresh["code"], "R0309");
    let lib_path = refresh["path"].as_str().unwrap();
    let main_path = lib_path.replace("Lib.luau", "Main.luau");
    assert_eq!(refresh["stringMatches"], serde_json::json!([{ "path": main_path, "line": 3 }]));
    let notes = refresh["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n.as_str().unwrap().contains("also appears as a string, at Main.luau:3")),
        "{notes:?}"
    );

    let plain = report("plain");
    assert_eq!(plain["stringMatches"], serde_json::json!([]));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn no_project_summary_counts_inline_allows() {
    // No-project mode (`Workspace`/`Session` route it the same as
    // project mode) must count an inline `-- rime: allow(...)` suppression
    // toward `summary.ignored`, same as project mode always did — the
    // earlier CLI's no-project path silently dropped this count
    // (`check_source_with` discarded `allow::apply`'s `suppressed`), fixed
    // as a side effect of routing every mode through `Session::reports()`.
    let dir = std::env::temp_dir().join(format!("rime-allow-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Unused local (R0013) — one diagnostic, suppressed by the trailing
    // allow on its own line.
    std::fs::write(
        dir.join("case.luau"),
        "local scratch = 1 -- rime: allow(unused-local, \"test\")\n",
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "case.luau"])
        .current_dir(&dir)
        .output()
        .expect("spawn rime");

    let code = out.status.code();
    assert!(
        code == Some(0) || code == Some(1),
        "unexpected exit {code:?}, stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout must be pure JSON");
    assert_eq!(v["summary"]["checked"], 1);
    assert_eq!(v["summary"]["ignored"], 1);
    let diags = v["diagnostics"].as_array().expect("diagnostics is an array");
    assert!(diags.is_empty(), "suppressed diagnostic must not appear: {diags:?}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn text_format_unchanged_by_default() {
    let dir = std::env::temp_dir().join(format!("rime-text-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("clean.luau"), "return {}\n").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "clean.luau"])
        .current_dir(&dir)
        .output()
        .expect("spawn rime");

    // No --format: stdout must NOT be JSON.
    assert!(
        serde_json::from_slice::<serde_json::Value>(&out.stdout).is_err()
            || out.stdout.is_empty()
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A conservative report is marked in every form of the output, and a
/// firm one in none: `(conservative)` in the default and the `--plain`
/// text, `"conservative": true` in JSON.
#[test]
fn every_format_marks_a_conservative_report() {
    let dir = std::env::temp_dir().join(format!("rime-conservative-mark-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Line 1 is R0023, conservative: a global write may be meant. Line 3
    // is R0041, firm: the write throws.
    std::fs::write(
        dir.join("case.luau"),
        "count = 1\nlocal t = table.freeze({ n = 1 })\nt.n = 2\nreturn t\n",
    )
    .unwrap();
    let run = |args: &[&str]| -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_rime"))
            .arg("check")
            .args(args)
            .arg("case.luau")
            .current_dir(&dir)
            .output()
            .expect("spawn rime");
        assert_eq!(out.status.code(), Some(1), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("utf8")
    };

    let default = strip_colour(&run(&[]));
    let heads: Vec<&str> = default.lines().filter(|l| l.starts_with("[R")).collect();
    assert_eq!(
        heads,
        [
            "[R0023 unscoped-variable] Error (conservative): global write: `count` is not declared `local`",
            "[R0041 freeze-write] Error: cannot write to a frozen table",
        ],
        "{default}"
    );

    let plain = run(&["--plain"]);
    let heads: Vec<&str> = plain.lines().filter(|l| l.starts_with("case.luau: ")).collect();
    assert_eq!(
        heads,
        [
            "case.luau: error[R0023] (conservative): global write: `count` is not declared `local`",
            "case.luau: error[R0041]: cannot write to a frozen table",
        ],
        "{plain}"
    );

    let json: serde_json::Value = serde_json::from_str(&run(&["--format", "json"])).expect("stdout must be pure JSON");
    let marks: Vec<(&str, bool)> = json["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| (d["code"].as_str().unwrap(), d["conservative"].as_bool().unwrap()))
        .collect();
    assert_eq!(marks, [("R0023", true), ("R0041", false)]);

    std::fs::remove_dir_all(&dir).ok();
}

/// `s` without its terminal colour codes.
fn strip_colour(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}
