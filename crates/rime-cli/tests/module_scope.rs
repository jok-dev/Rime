//! End-to-end for R0312 module-scope-computation, the one rule that is off
//! until rime.toml turns it on: nothing by default, reports at the level
//! the project sets, and the project's own `allow`, `allow-builtin`,
//! `deny`, `allow-loops`, `scripts` and `exclude` under
//! `[rules.module-scope-computation]`.

use std::path::PathBuf;
use std::process::Command;

/// A module that calls three things and loops once as it loads.
const MODULE: &str = "local Shapes = require(script.Parent.Shapes)\n\
                      local Board = {}\n\
                      Board.Origin = Shapes.point(0, 0)\n\
                      Board.Stamp = os.time()\n\
                      for index = 1, 3 do\n\
                      \tBoard[index] = index\n\
                      end\n\
                      print(\"board loaded\")\n\
                      return Board\n";

const ON: &str = "[rules]\nmodule-scope-computation = \"warn\"\n";

fn fixture(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rime-module-scope-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, text) in files {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

struct Run {
    exit: Option<i32>,
    /// (file name, message, severity) of every R0312, in output order.
    reports: Vec<(String, String, String)>,
    stderr: String,
}

impl Run {
    fn messages(&self) -> Vec<&str> {
        self.reports.iter().map(|(_, message, _)| message.as_str()).collect()
    }
}

fn run_with(dir: &PathBuf, extra: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_rime"))
        .args(["check", "--format", "json", "."])
        .args(extra)
        .current_dir(dir)
        .output()
        .expect("spawn rime");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let reports = match serde_json::from_slice::<serde_json::Value>(&out.stdout) {
        Ok(v) => v["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["code"] == "R0312")
            .map(|d| {
                let path = d["path"].as_str().unwrap().replace('\\', "/");
                (
                    path.rsplit('/').next().unwrap().to_string(),
                    d["message"].as_str().unwrap().to_string(),
                    d["severity"].as_str().unwrap().to_string(),
                )
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    Run { exit: out.status.code(), reports, stderr }
}

fn run(dir: &PathBuf) -> Run {
    run_with(dir, &[])
}

const ALL: [&str; 4] = [
    "`Shapes.point` is called at module scope",
    "`os.time` is called at module scope",
    "`for` loop at module scope",
    "`print` is called at module scope",
];

#[test]
fn off_until_a_project_turns_it_on() {
    let dir = fixture("default", &[("Board.luau", MODULE)]);
    let got = run(&dir);
    assert_eq!(got.messages(), Vec::<&str>::new());
    assert_eq!(got.exit, Some(0));
    std::fs::remove_dir_all(&dir).ok();

    // A rime.toml that does not name the rule leaves it off too.
    let dir = fixture("unnamed", &[("Board.luau", MODULE), ("rime.toml", "[rules]\nunused-local = \"error\"\n")]);
    assert_eq!(run(&dir).messages(), Vec::<&str>::new());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_level_turns_it_on_with_the_built_in_lists() {
    let dir = fixture("warn", &[("Board.luau", MODULE), ("rime.toml", ON)]);
    let got = run(&dir);
    assert_eq!(got.messages(), ALL);
    assert!(got.reports.iter().all(|(_, _, severity)| severity == "warning"), "{:?}", got.reports);
    assert_eq!(got.exit, Some(0), "warnings do not fail the run");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn set_to_error_it_fails_the_run() {
    for toml in [
        "[rules]\nmodule-scope-computation = \"error\"\n",
        "[rules]\nR0312 = \"error\"\n",
        "[rules.module-scope-computation]\nlevel = \"error\"\n",
    ] {
        let dir = fixture("error", &[("Board.luau", MODULE), ("rime.toml", toml)]);
        let got = run(&dir);
        assert_eq!(got.messages(), ALL, "{toml}");
        assert!(got.reports.iter().all(|(_, _, severity)| severity == "error"), "{toml}: {:?}", got.reports);
        assert_eq!(got.exit, Some(1), "{toml}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn the_project_says_what_a_module_may_call() {
    let toml = "[rules.module-scope-computation]\n\
                level = \"error\"\n\
                allow = [\"Shapes.*\", \"print\"]\n";
    let dir = fixture("allow", &[("Board.luau", MODULE), ("rime.toml", toml)]);
    let got = run(&dir);
    assert_eq!(got.messages(), vec![ALL[1], ALL[2]]);
    std::fs::remove_dir_all(&dir).ok();
}

/// A module that builds constants with the Luau library and Roblox value
/// constructors, and makes one call that is on no list.
const CONSTANTS: &str = "local Shapes = require(script.Parent.Shapes)\n\
                         local Style = {}\n\
                         Style.Turn = math.rad(90)\n\
                         Style.Title = string.rep(\"-\", 8)\n\
                         Style.Offset = Vector3.new(0, 4, 0)\n\
                         Style.Accent = Color3.fromRGB(255, 170, 0)\n\
                         Style.Source = Random.new(7)\n\
                         Style.Shape = Shapes.point(0, 0)\n\
                         return table.freeze(Style)\n";

#[test]
fn the_built_in_lists_are_in_use_by_default_and_the_project_list_adds_to_them() {
    let dir = fixture("builtin", &[("Style.luau", CONSTANTS), ("rime.toml", ON)]);
    assert_eq!(run(&dir).messages(), vec!["`Shapes.point` is called at module scope"]);
    std::fs::remove_dir_all(&dir).ok();

    // `allow` does not have to repeat `require` or anything built in.
    let toml = "[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\"Shapes.*\"]\n";
    let dir = fixture("builtin-add", &[("Style.luau", CONSTANTS), ("rime.toml", toml)]);
    assert_eq!(run(&dir).messages(), Vec::<&str>::new());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_project_says_which_built_in_lists_are_in_use() {
    let with = |name: &str, lists: &str| {
        let toml = format!(
            "[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\"Shapes.*\"]\nallow-builtin = {lists}\n"
        );
        let dir = fixture(name, &[("Style.luau", CONSTANTS), ("rime.toml", &toml)]);
        let got = run(&dir).messages().iter().map(|m| m.to_string()).collect::<Vec<_>>();
        std::fs::remove_dir_all(&dir).ok();
        got
    };
    assert_eq!(with("builtin-both", "[\"luau\", \"roblox-datatypes\"]"), Vec::<String>::new());
    assert_eq!(
        with("builtin-luau", "[\"luau\"]"),
        vec![
            "`Vector3.new` is called at module scope",
            "`Color3.fromRGB` is called at module scope",
            "`Random.new` is called at module scope",
        ]
    );
    assert_eq!(
        with("builtin-datatypes", "[\"roblox-datatypes\"]"),
        vec![
            "`math.rad` is called at module scope",
            "`string.rep` is called at module scope",
            "`table.freeze` is called at module scope",
        ]
    );
    // Neither: `require` is still not a report.
    assert_eq!(with("builtin-none", "[]").len(), 6);
}

#[test]
fn deny_takes_single_callees_back_out() {
    let toml = "[rules.module-scope-computation]\n\
                level = \"warn\"\n\
                allow = [\"Shapes.*\"]\n\
                deny = [\"Random.new\", \"Shapes.point\", \"require\"]\n";
    let dir = fixture("deny", &[("Style.luau", CONSTANTS), ("rime.toml", toml)]);
    assert_eq!(
        run(&dir).messages(),
        vec![
            "`require` is called at module scope",
            "`Random.new` is called at module scope",
            "`Shapes.point` is called at module scope",
        ]
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn exclude_leaves_files_and_folders_alone() {
    let module = "local started = os.time()\nreturn started\n";
    let files = [
        ("src/Clock.luau", module),
        ("src/Clock.spec.luau", module),
        ("src/tools/Make.luau", module),
        ("src/tools/deep/Gen.luau", module),
        ("vendor/Fork.luau", module),
    ];
    let files_of = |name: &str, toml: &str| {
        let mut all = files.to_vec();
        all.push(("rime.toml", toml));
        let dir = fixture(name, &all);
        let got = run(&dir);
        let mut names: Vec<String> = got.reports.iter().map(|(file, _, _)| file.clone()).collect();
        names.sort();
        std::fs::remove_dir_all(&dir).ok();
        (names, got.stderr)
    };
    let (all, _) = files_of("exclude-none", ON);
    assert_eq!(all, ["Clock.luau", "Clock.spec.luau", "Fork.luau", "Gen.luau", "Make.luau"]);

    // A folder (with or without `/**`), and a file-name pattern.
    let toml = "[rules.module-scope-computation]\nlevel = \"warn\"\n\
                exclude = [\"src/tools/**\", \"vendor\", \"*.spec.luau\"]\n";
    let (left, stderr) = files_of("exclude-set", toml);
    assert_eq!(left, ["Clock.luau"], "stderr: {stderr}");

    // `exclude` does not replace `scripts`: both are read.
    let mut with_script = files.to_vec();
    with_script.push(("src/start.server.luau", "print(os.time())\nreturn nil\n"));
    with_script.push(("rime.toml", toml));
    let dir = fixture("exclude-and-scripts", &with_script);
    let names: Vec<String> = run(&dir).reports.iter().map(|(file, _, _)| file.clone()).collect();
    assert_eq!(names, ["Clock.luau"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_project_says_whether_a_module_may_loop() {
    let toml = "[rules.module-scope-computation]\nlevel = \"warn\"\nallow-loops = true\n";
    let dir = fixture("loops", &[("Board.luau", MODULE), ("rime.toml", toml)]);
    assert_eq!(run(&dir).messages(), vec![ALL[0], ALL[1], ALL[3]]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn entry_scripts_are_left_alone() {
    // By the default file-name patterns, and because a file that returns
    // nothing is not a module.
    let script = "local started = os.time()\nprint(started)\n";
    let dir = fixture(
        "scripts",
        &[
            ("Board.luau", MODULE),
            ("start.server.luau", script),
            ("start.client.lua", script),
            ("returns.server.luau", "print(os.time())\nreturn nil\n"),
            // Every Rojo script suffix, not only the common two.
            ("tool.plugin.luau", "print(os.time())\nreturn nil\n"),
            ("init.plugin.lua", "print(os.time())\nreturn nil\n"),
            ("plain.luau", script),
            ("rime.toml", ON),
        ],
    );
    let got = run(&dir);
    assert!(got.reports.iter().all(|(file, _, _)| file == "Board.luau"), "{:?}", got.reports);
    assert_eq!(got.messages(), ALL);
    std::fs::remove_dir_all(&dir).ok();

    // Rojo makes a ModuleScript of any other dotted name, `.legacy` and
    // `.local` included: those are modules.
    let module = "local started = os.time()\nreturn started\n";
    let dir = fixture(
        "scripts-not",
        &[("old.legacy.lua", module), ("hud.local.luau", module), ("x.spec.luau", module), ("rime.toml", ON)],
    );
    let mut files: Vec<String> = run(&dir).reports.iter().map(|(file, _, _)| file.clone()).collect();
    files.sort();
    assert_eq!(files, ["hud.local.luau", "old.legacy.lua", "x.spec.luau"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_path_entry_works_however_its_slashes_and_its_start_are_written() {
    let module = "local started = os.time()\nreturn started\n";
    let files = [("src/Clock.luau", module), ("src/tools/Make.luau", module), ("vendor/Fork.luau", module)];
    for (name, exclude) in [
        ("plain", "[\"src/tools\", \"vendor/**\"]"),
        ("backslash", "[\"src\\\\tools\", \"vendor\\\\**\"]"),
        ("dot", "[\"./src/tools\", \"./vendor/**\"]"),
        ("dot-backslash", "['.\\src\\tools', '.\\vendor\\**']"),
        ("rooted", "[\"/src/tools\", \"/vendor/**\"]"),
    ] {
        let toml = format!("[rules.module-scope-computation]\nlevel = \"warn\"\nexclude = {exclude}\n");
        let mut all = files.to_vec();
        all.push(("rime.toml", &toml));
        let dir = fixture(&format!("path-form-{name}"), &all);
        let got = run(&dir);
        let names: Vec<&str> = got.reports.iter().map(|(file, _, _)| file.as_str()).collect();
        assert_eq!(names, ["Clock.luau"], "{exclude}\nstderr: {}", got.stderr);
        assert!(!got.stderr.contains("matches nothing"), "{exclude}\nstderr: {}", got.stderr);
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn a_path_entry_that_names_nothing_is_said_out_loud() {
    let module = "local started = os.time()\nreturn started\n";
    let toml = "[rules.module-scope-computation]\nlevel = \"warn\"\n\
                exclude = [\"src/tool/**\", \"src/tools/**\", \"*.spec.luau\"]\n\
                scripts = [\"boot/**\"]\n";
    let dir = fixture("path-missing", &[("src/tools/Make.luau", module), ("src/Clock.luau", module), ("rime.toml", toml)]);
    let got = run(&dir);
    assert_eq!(got.reports.len(), 1, "the entry that is there still works");
    assert!(
        got.stderr.contains("warning: [rules.module-scope-computation] `exclude` entry `src/tool/**` matches nothing: there is no `src/tool` below rime.toml"),
        "stderr: {}",
        got.stderr
    );
    assert!(got.stderr.contains("`scripts` entry `boot/**` matches nothing"), "stderr: {}", got.stderr);
    assert_eq!(got.stderr.matches("matches nothing").count(), 2, "stderr: {}", got.stderr);
    assert_eq!(got.exit, Some(0), "a warning about the config, not a failure");

    // With the rule off the settings are not looked at.
    let off = toml.replace("level = \"warn\"", "level = \"off\"");
    std::fs::write(dir.join("rime.toml"), off).unwrap();
    assert!(!run(&dir).stderr.contains("matches nothing"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_project_says_which_files_are_entry_scripts() {
    let boot = "local started = os.time()\nreturn started\n";
    let files = [
        ("boot/Main.luau", boot),
        ("boot/deep/Late.luau", boot),
        ("lib/Clock.luau", boot),
        ("lib/Clock.spec.luau", boot),
    ];
    let files_of = |r: &Run| r.reports.iter().map(|(file, _, _)| file.clone()).collect::<Vec<_>>();

    let mut all = files.to_vec();
    all.push(("rime.toml", ON));
    let dir = fixture("scripts-default", &all);
    // Files come in byte order of each name: `Main.luau` before the folder
    // `deep`, a capital before a small letter.
    assert_eq!(files_of(&run(&dir)), vec!["Main.luau", "Late.luau", "Clock.luau", "Clock.spec.luau"]);
    std::fs::remove_dir_all(&dir).ok();

    // A pattern with `/` is a path below rime.toml; one without is a file
    // name. `*` stays inside one folder, `**` does not.
    let toml = "[rules.module-scope-computation]\nlevel = \"warn\"\nscripts = [\"boot/**\", \"*.spec.luau\"]\n";
    let mut all = files.to_vec();
    all.push(("rime.toml", toml));
    let dir = fixture("scripts-set", &all);
    assert_eq!(files_of(&run(&dir)), vec!["Clock.luau"]);
    std::fs::remove_dir_all(&dir).ok();

    let toml = "[rules.module-scope-computation]\nlevel = \"warn\"\nscripts = [\"boot/*\"]\n";
    let mut all = files.to_vec();
    all.push(("rime.toml", toml));
    let dir = fixture("scripts-one-level", &all);
    assert_eq!(files_of(&run(&dir)), vec!["Late.luau", "Clock.luau", "Clock.spec.luau"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn script_paths_are_below_the_config_file_however_it_was_named() {
    // `--config` with a bare file name has no directory part to anchor to.
    let boot = "local started = os.time()\nreturn started\n";
    let toml = "[rules.module-scope-computation]\nlevel = \"warn\"\nscripts = [\"boot/**\"]\n";
    let dir = fixture(
        "scripts-config-flag",
        &[("boot/Main.luau", boot), ("lib/Clock.luau", boot), ("named.toml", toml)],
    );
    let got = run_with(&dir, &["--config", "named.toml"]);
    let files: Vec<&str> = got.reports.iter().map(|(file, _, _)| file.as_str()).collect();
    assert_eq!(files, vec!["Clock.luau"], "stderr: {}", got.stderr);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn one_site_is_let_through_with_an_allow_comment() {
    let source = "local Board = {}\n\
                  Board.Stamp = os.time() -- rime: allow(module-scope-computation, \"read once\")\n\
                  -- rime: allow(module-scope-computation, \"three entries\")\n\
                  for index = 1, 3 do\n\
                  \tBoard[index] = tostring(index)\n\
                  end\n\
                  print(\"board loaded\")\n\
                  return Board\n";
    let dir = fixture("inline", &[("Board.luau", source), ("rime.toml", ON)]);
    assert_eq!(run(&dir).messages(), vec![ALL[3]]);
    std::fs::remove_dir_all(&dir).ok();

    // Above a statement written over several lines, it covers every call
    // in that statement and nothing after it.
    let chain = "local Storage = game:GetService(\"ReplicatedStorage\")\n\
                 -- rime: allow(module-scope-computation, \"boot\")\n\
                 local remote = Storage\n\
                 \t:WaitForChild(\"Remotes\")\n\
                 \t:WaitForChild(\"Fire\")\n\
                 local other = Storage\n\
                 \t:WaitForChild(\"Remotes\")\n\
                 return { remote, other }\n";
    let dir = fixture("inline-chain", &[("Board.luau", chain), ("rime.toml", ON)]);
    assert_eq!(run(&dir).messages(), vec!["`Storage:WaitForChild` is called at module scope"]);
    std::fs::remove_dir_all(&dir).ok();

    // And a whole file, from its top.
    let whole = format!("-- rime: allow-file(module-scope-computation, \"a data table built as it loads\")\n{MODULE}");
    let dir = fixture("inline-file", &[("Board.luau", &whole), ("Other.luau", MODULE), ("rime.toml", ON)]);
    let got = run(&dir);
    assert!(got.reports.iter().all(|(file, _, _)| file == "Other.luau"), "{:?}", got.reports);
    assert_eq!(got.messages(), ALL);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn it_runs_in_a_rojo_project_too() {
    let project = r#"{ "name": "fixture", "tree": { "$className": "DataModel", "ReplicatedStorage": { "Shared": { "$path": "src" } } } }"#;
    let dir = fixture(
        "project",
        &[
            ("default.project.json", project),
            ("src/Shapes.luau", "local Shapes = {}\nfunction Shapes.point(x, y)\n\treturn { x = x, y = y }\nend\nreturn Shapes\n"),
            ("src/Board.luau", MODULE),
            ("src/start.server.luau", "print(os.time())\n"),
            ("rime.toml", ON),
        ],
    );
    let got = run(&dir);
    assert_eq!(got.messages(), ALL, "stderr: {}", got.stderr);
    assert!(got.reports.iter().all(|(file, _, _)| file == "Board.luau"), "{:?}", got.reports);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn only_shows_it_without_a_rime_toml_line() {
    let dir = fixture("only", &[("Board.luau", MODULE)]);
    let got = run_with(&dir, &["--only", "module-scope-computation"]);
    assert_eq!(got.messages(), ALL);
    assert!(got.reports.iter().all(|(_, _, severity)| severity == "warning"));
    std::fs::remove_dir_all(&dir).ok();

    // `--only` for another rule does not turn it on, whatever rime.toml says.
    let dir = fixture("only-other", &[("Board.luau", MODULE), ("rime.toml", ON)]);
    assert_eq!(run_with(&dir, &["--only", "unused-local"]).messages(), Vec::<&str>::new());
    std::fs::remove_dir_all(&dir).ok();

    // The project's own list still applies under `--only`.
    let toml = "[rules.module-scope-computation]\nlevel = \"off\"\nallow = [\"Shapes.*\", \"os.*\", \"print\"]\n";
    let dir = fixture("only-list", &[("Board.luau", MODULE), ("rime.toml", toml)]);
    assert_eq!(run(&dir).messages(), Vec::<&str>::new(), "level = off");
    assert_eq!(run_with(&dir, &["--only", "R0312"]).messages(), vec![ALL[2]]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_blanket_conservative_setting_needs_the_rules_own_to_stay_an_error() {
    // Every R0312 is flagged conservative (valid Luau, a project's
    // convention), so `[conservative] default` lowers it like any other;
    // the rule's own `conservative` is what keeps it failing the run.
    let lowered = "[conservative]\ndefault = \"warn\"\n[rules]\nmodule-scope-computation = \"error\"\n";
    let dir = fixture("conservative-default", &[("Board.luau", MODULE), ("rime.toml", lowered)]);
    let got = run(&dir);
    assert_eq!(got.messages(), ALL);
    assert_eq!(got.exit, Some(0));
    std::fs::remove_dir_all(&dir).ok();

    let kept = "[conservative]\ndefault = \"warn\"\n[rules]\nmodule-scope-computation = { level = \"error\", conservative = \"error\" }\n";
    let dir = fixture("conservative-own", &[("Board.luau", MODULE), ("rime.toml", kept)]);
    let got = run(&dir);
    assert_eq!(got.messages(), ALL);
    assert_eq!(got.exit, Some(1));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_bad_setting_is_an_error_not_a_silent_default() {
    for (toml, expected) in [
        // Options and no level: the rule would stay off and the list would
        // do nothing.
        ("[rules.module-scope-computation]\nallow = [\"require\"]\n", "off by default"),
        ("[rules.module-scope-computation]\nallow-loops = true\n", "off by default"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = \"require\"\n", "`allow` must be an array of strings"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [1]\n", "`allow` must be an array of strings"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\"\"]\n", "empty"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nscripts = \"*.server.lua\"\n", "`scripts` must be an array of strings"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow-loops = \"yes\"\n", "`allow-loops` must be true or false"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nallowed = [\"require\"]\n", "unknown key `allowed`"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\ndeny = \"print\"\n", "`deny` must be an array of strings"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nexclude = \"tools\"\n", "`exclude` must be an array of strings"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow-builtin = [\"roblox\"]\n", "unknown built-in list `roblox`"),
        ("[rules.module-scope-computation]\nexclude = [\"tools\"]\n", "off by default"),
        // A callee that could never match.
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\"build()\"]\n", "write `build`"),
        ("[rules.module-scope-computation]\nlevel = \"warn\"\nallow = [\" build\"]\n", "has a space"),
        // The options belong to this rule alone.
        ("[rules.unused-local]\nlevel = \"warn\"\nallow = [\"require\"]\n", "unknown key `allow`"),
    ] {
        let dir = fixture("bad", &[("Board.luau", MODULE), ("rime.toml", toml)]);
        let got = run(&dir);
        assert!(got.stderr.contains(expected), "toml: {toml}\nstderr: {}\nexpected: {expected}", got.stderr);
        assert_ne!(got.exit, Some(0), "{toml}");
        assert!(got.reports.is_empty(), "{toml}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
