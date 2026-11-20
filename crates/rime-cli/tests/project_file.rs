//! Which Rojo project file `rime check` uses, and what it says about it.
//!
//! Rojo calls any `*.project.json` a project file; `default.project.json`
//! is only the name it looks for when given a folder. A project with
//! another name used to get no cross-file checking and no message.

use std::path::PathBuf;
use std::process::Command;

struct Dir {
    dir: PathBuf,
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Dir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rime-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Dir { dir }
    }

    fn file(&self, rel: &str, src: &str) -> &Self {
        let path = self.dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, src).unwrap();
        self
    }

    fn check(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_rime"))
            .arg("check")
            .arg("--plain")
            .args(args)
            .current_dir(&self.dir)
            .output()
            .expect("spawn rime");
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// A tree that mounts `folder` under ReplicatedStorage.
fn project(folder: &str) -> String {
    format!(
        r#"{{ "name": "fixture", "tree": {{ "$className": "DataModel",
            "ReplicatedStorage": {{ "Shared": {{ "$path": "{folder}" }} }} }} }}"#
    )
}

/// A require that only a project can prove broken.
const BROKEN: &str = "local Gone = require(script.Parent.Gone)\nreturn table.freeze({ Gone = Gone })\n";

fn lines_with<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
    text.lines().filter(|l| l.contains(needle)).collect()
}

#[test]
fn a_project_file_with_another_name_is_used_and_named() {
    let d = Dir::new("other-name");
    d.file("place.project.json", &project("src")).file("src/main.luau", BROKEN);
    let run = d.check(&["src"]);
    assert!(run.stdout.contains("R0501"), "cross-file checking must be on: {}\n{}", run.stdout, run.stderr);
    let named = lines_with(&run.stderr, "place.project.json");
    assert_eq!(named.len(), 1, "the project file in use is named once: {}", run.stderr);
    assert!(named[0].contains("using project file"), "{}", run.stderr);
}

#[test]
fn the_default_project_file_is_named_too() {
    let d = Dir::new("default-name");
    d.file("default.project.json", &project("src")).file("src/main.luau", BROKEN);
    let run = d.check(&["src"]);
    assert!(run.stdout.contains("R0501"), "{}\n{}", run.stdout, run.stderr);
    assert_eq!(lines_with(&run.stderr, "using project file").len(), 1, "{}", run.stderr);
    assert!(run.stderr.contains("default.project.json"), "{}", run.stderr);
}

#[test]
fn default_wins_over_other_names_in_the_same_folder() {
    let d = Dir::new("default-wins");
    d.file("default.project.json", &project("src"))
        .file("test.project.json", &project("nowhere"))
        .file("src/main.luau", BROKEN);
    let run = d.check(&["src"]);
    assert!(run.stdout.contains("R0501"), "{}\n{}", run.stdout, run.stderr);
    let named = lines_with(&run.stderr, "using project file");
    assert_eq!(named.len(), 1, "{}", run.stderr);
    assert!(named[0].contains("default.project.json"), "{}", run.stderr);
}

#[test]
fn several_project_files_and_no_default_are_not_guessed_between() {
    let d = Dir::new("several");
    d.file("lobby.project.json", &project("src"))
        .file("arena.project.json", &project("src"))
        .file("src/main.luau", BROKEN);
    let run = d.check(&["src"]);
    assert_eq!(run.code, 0, "no project, so nothing proves the require broken: {}\n{}", run.stdout, run.stderr);
    let off = lines_with(&run.stderr, "cross-file checking is off");
    assert_eq!(off.len(), 1, "said once: {}", run.stderr);
    assert!(off[0].contains("lobby.project.json") && off[0].contains("arena.project.json"), "{}", run.stderr);
    assert!(off[0].contains("--project"), "says how to pick one: {}", run.stderr);
    assert!(!run.stderr.contains("using project file"), "{}", run.stderr);
}

#[test]
fn the_project_flag_picks_a_file() {
    let d = Dir::new("flag");
    d.file("lobby.project.json", &project("src"))
        .file("arena.project.json", &project("other"))
        .file("src/main.luau", BROKEN)
        .file("other/main.luau", "return 1\n");
    let run = d.check(&["--project", "lobby.project.json", "src"]);
    assert!(run.stdout.contains("R0501"), "{}\n{}", run.stdout, run.stderr);
    let named = lines_with(&run.stderr, "using project file");
    assert_eq!(named.len(), 1, "{}", run.stderr);
    assert!(named[0].contains("lobby.project.json") && named[0].contains("--project"), "{}", run.stderr);
}

#[test]
fn the_config_key_picks_a_file_and_the_flag_beats_it() {
    let d = Dir::new("config-key");
    d.file("rime.toml", "project = \"lobby.project.json\"\n")
        .file("lobby.project.json", &project("src"))
        .file("arena.project.json", &project("src"))
        .file("src/main.luau", BROKEN);
    let run = d.check(&["src"]);
    assert!(run.stdout.contains("R0501"), "{}\n{}", run.stdout, run.stderr);
    let named = lines_with(&run.stderr, "using project file");
    assert_eq!(named.len(), 1, "{}", run.stderr);
    assert!(named[0].contains("lobby.project.json") && named[0].contains("rime.toml"), "{}", run.stderr);

    let run = d.check(&["--project", "arena.project.json", "src"]);
    let named = lines_with(&run.stderr, "using project file");
    assert_eq!(named.len(), 1, "{}", run.stderr);
    assert!(named[0].contains("arena.project.json"), "{}", run.stderr);
}

#[test]
fn a_named_project_file_that_is_not_there_stops_the_run() {
    let d = Dir::new("flag-missing");
    d.file("src/main.luau", "return 1\n");
    let run = d.check(&["--project", "nope.project.json", "src"]);
    assert_ne!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("nope.project.json"), "{}", run.stderr);
    assert!(!run.stderr.contains("Rime checked"), "nothing ran: {}", run.stderr);

    d.file("rime.toml", "project = \"gone.project.json\"\n");
    let run = d.check(&["src"]);
    assert_ne!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("gone.project.json"), "{}", run.stderr);
}

#[test]
fn no_project_file_is_said_once() {
    let d = Dir::new("none");
    d.file("src/main.luau", BROKEN).file("src/other.luau", BROKEN);
    let run = d.check(&["src"]);
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
    let off = lines_with(&run.stderr, "cross-file checking is off");
    assert_eq!(off.len(), 1, "once per run, not once per file: {}", run.stderr);
    assert!(off[0].contains("*.project.json"), "{}", run.stderr);
    assert!(off[0].contains("--project"), "{}", run.stderr);
}

#[test]
fn a_project_file_rojo_accepts_with_comments_and_optional_paths_loads() {
    let d = Dir::new("jsonc");
    d.file(
        "place.project.json",
        r#"{
            // Rojo reads project files as JSON with comments.
            "name": "fixture",
            "tree": {
                "$className": "DataModel",
                "ReplicatedStorage": {
                    "Shared": { "$path": { "optional": "src" } },
                    "Absent": { "$path": { "optional": "not-there" } },
                },
            },
        }"#,
    )
    .file("src/main.luau", "local Dep = require(game.ReplicatedStorage.Shared.dep)\nreturn table.freeze({ Dep = Dep })\n")
    .file("src/dep.luau", "return table.freeze({})\n");
    let run = d.check(&["src"]);
    assert!(run.stderr.contains("using project file"), "{}", run.stderr);
    assert_eq!(run.code, 0, "the optional mount resolves: {}\n{}", run.stdout, run.stderr);
}

#[test]
fn a_path_that_names_a_project_file_is_that_project() {
    let d = Dir::new("path-to-project");
    d.file(
        "place.project.json",
        r#"{ "name": "fixture", "tree": { "$className": "DataModel",
            "ReplicatedStorage": { "Kit": { "$path": "kit/kit.project.json" } } } }"#,
    )
    .file("kit/kit.project.json", r#"{ "name": "kit", "tree": { "$path": "lib" } }"#)
    .file("kit/lib/init.luau", "return table.freeze({})\n")
    .file("kit/lib/part.luau", "return table.freeze({})\n")
    .file(
        "src/main.luau",
        "local Kit = require(game.ReplicatedStorage.Kit)\nlocal Part = require(game.ReplicatedStorage.Kit.part)\n\
         local Gone = require(game.ReplicatedStorage.Kit.gone)\nreturn table.freeze({ Kit = Kit, Part = Part, Gone = Gone })\n",
    );
    let run = d.check(&["src"]);
    assert_eq!(lines_with(&run.stdout, "R0501").len(), 1, "only `gone` is missing: {}\n{}", run.stdout, run.stderr);
    assert_eq!(lines_with(&run.stdout, "R050").len(), 1, "{}", run.stdout);
}
