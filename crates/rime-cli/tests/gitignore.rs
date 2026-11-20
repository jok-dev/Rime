//! `rime check` leaves out what git ignores.
//!
//! `rime check .` at the root of a repository used to walk everything
//! under it: build output, package caches, and other checkouts of the same
//! repository kept in a git-ignored folder. On a large codebase that was
//! most of the files and most of the diagnostics, none of them about the
//! code being worked on.
//!
//! The matching itself is tested against git in `src/discover.rs`; these
//! are the end-to-end promises.

use std::path::PathBuf;
use std::process::Command;

struct Repo {
    dir: PathBuf,
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| panic!("stdout is JSON ({e}): {}", self.stdout))
    }
}

impl Repo {
    /// A directory git would call a work tree. An empty `.git` folder is
    /// all Rime looks for; it never runs git.
    fn new(name: &str) -> Self {
        let repo = Self::bare(name);
        std::fs::create_dir_all(repo.dir.join(".git")).unwrap();
        repo
    }

    /// The same without `.git`.
    fn bare(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("rime-gitignore-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Repo { dir }
    }

    fn file(&self, rel: &str, src: &str) -> &Self {
        let path = self.dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, src).unwrap();
        self
    }

    fn check(&self, args: &[&str]) -> Run {
        self.check_in("", args)
    }

    fn check_in(&self, cwd: &str, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_rime"))
            .arg("check")
            .args(args)
            .current_dir(self.dir.join(cwd))
            // The global excludes file of whoever runs the tests is not
            // part of the fixture: point every place it is looked up from
            // (`~/.gitconfig`, `$XDG_CONFIG_HOME/git/`) at nothing.
            .env("HOME", self.dir.join("no-such-home"))
            .env("USERPROFILE", self.dir.join("no-such-home"))
            .env("XDG_CONFIG_HOME", self.dir.join("no-such-home").join(".config"))
            .output()
            .expect("spawn rime");
        Run {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        }
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

/// Is there a diagnostic with `code` on a file whose path ends in `file`?
fn has(v: &serde_json::Value, file: &str, code: &str) -> bool {
    v["diagnostics"].as_array().expect("diagnostics array").iter().any(|d| {
        d["code"] == code && d["path"].as_str().is_some_and(|f| f.replace('\\', "/").ends_with(file))
    })
}

fn files_reported(v: &serde_json::Value) -> Vec<String> {
    let mut files: Vec<String> = v["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .map(|d| d["path"].as_str().unwrap_or_default().replace('\\', "/"))
        .collect();
    files.sort();
    files.dedup();
    files
}

const BAD: &str = "print(definitely_undefined_name)\n";
const GOOD: &str = "return 1\n";

#[test]
fn what_git_ignores_is_not_checked() {
    let r = Repo::new("skip");
    r.file(".gitignore", "scratch/\n*.gen.luau\n")
        .file("src/a.luau", GOOD)
        .file("src/b.gen.luau", BAD)
        .file("scratch/s.luau", BAD)
        // Another checkout of the same repository, in an ignored folder.
        .file("scratch/worktrees/copy/.git", "gitdir: elsewhere\n")
        .file("scratch/worktrees/copy/src/a.luau", BAD);

    for args in [&["."][..], &[][..]] {
        let run = r.check(args);
        assert_eq!(run.code, 0, "{args:?}: {}", run.stderr);
        assert!(run.stderr.contains("Rime checked 1 files"), "{args:?}: {}", run.stderr);
        assert!(!run.stderr.contains("rime: warning"), "{args:?}: {}", run.stderr);
    }
    let run = r.check(&["--format", "json", "."]);
    let v = run.json();
    assert_eq!(v["summary"]["checked"], 1, "{v}");
    assert!(v["diagnostics"].as_array().unwrap().is_empty(), "{v}");
}

#[test]
fn no_gitignore_checks_them_again() {
    let r = Repo::new("optout");
    r.file(".gitignore", "scratch/\n").file("src/a.luau", GOOD).file("scratch/s.luau", BAD);

    let run = r.check(&["--no-gitignore", "--format", "json", "."]);
    let v = run.json();
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(v["summary"]["checked"], 2, "{v}");
    assert!(has(&v, "scratch/s.luau", "R0022"), "{v}");
}

#[test]
fn a_path_named_on_the_command_line_is_checked_even_when_git_ignores_it() {
    // Naming it is asking for it. Only what is found by walking a
    // directory is filtered.
    let r = Repo::new("named");
    r.file(".gitignore", "scratch/\none.luau\n")
        .file("src/a.luau", GOOD)
        .file("scratch/s.luau", BAD)
        .file("src/one.luau", BAD);

    let run = r.check(&["--format", "json", "scratch"]);
    assert_eq!(run.code, 1, "an ignored folder, named: {}", run.stderr);
    assert!(has(&run.json(), "scratch/s.luau", "R0022"), "{}", run.stdout);

    let run = r.check(&["--format", "json", "src/one.luau"]);
    assert_eq!(run.code, 1, "an ignored file, named: {}", run.stderr);
    assert!(has(&run.json(), "src/one.luau", "R0022"), "{}", run.stdout);

    // Walked rather than named, the same file is left out.
    let run = r.check(&["--format", "json", "src"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(run.json()["summary"]["checked"], 1, "{}", run.stdout);
}

#[test]
fn a_root_below_the_repository_top_still_follows_the_gitignores_above_it() {
    let r = Repo::new("below");
    r.file(".gitignore", "generated/\n")
        .file("pkg/.gitignore", "local.luau\n")
        .file("pkg/src/a.luau", GOOD)
        .file("pkg/src/local.luau", BAD)
        .file("pkg/src/generated/g.luau", BAD);

    // From the top, naming a subfolder; and from inside the subfolder.
    for (cwd, args) in [("", &["pkg/src"][..]), ("pkg/src", &["."][..]), ("pkg", &[][..])] {
        let run = r.check_in(cwd, args);
        assert_eq!(run.code, 0, "cwd `{cwd}` {args:?}: {}", run.stderr);
        assert!(run.stderr.contains("Rime checked 1 files"), "cwd `{cwd}` {args:?}: {}", run.stderr);
    }
}

#[test]
fn a_gitignore_outside_a_repository_is_not_followed() {
    // git gives a `.gitignore` meaning only inside a work tree. Outside
    // one it is a file like any other, and skipping on its say-so would
    // hide code nobody asked to hide.
    let r = Repo::bare("norepo");
    r.file(".gitignore", "scratch/\n").file("src/a.luau", GOOD).file("scratch/s.luau", BAD);

    let run = r.check(&["--format", "json", "."]);
    let v = run.json();
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(v["summary"]["checked"], 2, "{v}");
    assert!(has(&v, "scratch/s.luau", "R0022"), "{v}");
}

const PROJECT: &str = r#"{ "name": "t", "tree": { "$className": "DataModel", "ReplicatedStorage": {
    "$className": "ReplicatedStorage",
    "Source": { "$path": "src" },
    "Packages": { "$path": "Packages" } } } }"#;
const LIB: &str =
    "local Lib = {}\nfunction Lib.two(a: number, b: number) return a + b end\nprint(undefined_in_packages)\nreturn Lib\n";
const MAIN: &str = "local Lib = require(script.Parent.Parent.Packages.Lib)\nprint(Lib.two(1))\n";

/// Installed packages are git-ignored in nearly every Rojo repository and
/// required from nearly every file. Leaving them out of the walk must not
/// leave them out of the project: the cross-file checks still need to know
/// what they export.
#[test]
fn a_git_ignored_rojo_mount_is_still_read_but_not_reported_on() {
    let r = Repo::new("mount");
    r.file("default.project.json", PROJECT)
        .file(".gitignore", "Packages\nscratch/\n")
        .file("Packages/Lib.luau", LIB)
        .file("src/Main.luau", MAIN)
        .file("scratch/s.luau", BAD);

    let run = r.check(&["--format", "json", "."]);
    let v = run.json();
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(has(&v, "src/Main.luau", "R0301"), "the call was checked against the package's signature: {v}");
    assert_eq!(files_reported(&v), ["src/Main.luau"], "nothing reported on the ignored files: {v}");
    assert_eq!(v["summary"]["checked"], 1, "{v}");
    assert_eq!(v["summary"]["errors"], 1, "{v}");

    // Checking only `src` gives the same answer: `.` is now `src` plus
    // nothing, where it used to be `src` plus everything ignored.
    let src = r.check(&["--format", "json", "src"]).json();
    assert_eq!(src["diagnostics"], v["diagnostics"]);

    // Opted out, the package and the scratch folder are reported on too.
    let run = r.check(&["--no-gitignore", "--format", "json", "."]);
    let v = run.json();
    assert_eq!(files_reported(&v), ["Packages/Lib.luau", "scratch/s.luau", "src/Main.luau"], "{v}");
    assert!(has(&v, "src/Main.luau", "R0301"), "{v}");
    assert_eq!(v["summary"]["checked"], 3, "{v}");
}

/// `ignore` in rime.toml and `.gitignore` are separate lists with separate
/// effects, and neither changes what the other does.
#[test]
fn rime_toml_ignore_and_gitignore_each_do_their_own_thing() {
    let r = Repo::new("both");
    r.file("default.project.json", PROJECT)
        .file("rime.toml", "ignore = [\"src/vendor\", \"scratch/kept\"]\n")
        .file(".gitignore", "scratch/\n")
        .file("Packages/Lib.luau", "return {}\n")
        .file("src/Main.luau", GOOD)
        .file("src/vendor/v.luau", BAD)
        .file("scratch/s.luau", BAD)
        .file("scratch/kept/k.luau", BAD);

    // rime.toml's `ignore`: checked, counted as ignored, not reported.
    // .gitignore: never looked at, so not counted at all.
    let run = r.check(&["--format", "json", "."]);
    let v = run.json();
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(v["diagnostics"].as_array().unwrap().is_empty(), "{v}");
    assert_eq!(v["summary"]["ignored"], 1, "only `src/vendor`: {v}");

    // --no-gitignore brings back what git ignored, and rime.toml's list
    // then applies to it like to anything else.
    let run = r.check(&["--no-gitignore", "--format", "json", "."]);
    let v = run.json();
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert_eq!(files_reported(&v), ["scratch/s.luau"], "{v}");
    assert_eq!(v["summary"]["ignored"], 2, "`src/vendor` and `scratch/kept`: {v}");

    // Naming a git-ignored folder does not get past rime.toml either.
    let run = r.check(&["--format", "json", "scratch"]);
    let v = run.json();
    assert_eq!(files_reported(&v), ["scratch/s.luau"], "{v}");
    assert_eq!(v["summary"]["ignored"], 1, "{v}");
}

#[test]
fn a_folder_where_everything_is_git_ignored_says_why_nothing_was_checked() {
    let r = Repo::new("allignored");
    r.file(".gitignore", "out/\n").file("out/a.luau", BAD).file("out/b.luau", BAD);

    let run = r.check(&["."]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("Rime checked 0 files"), "{}", run.stderr);
    assert!(
        run.stderr.contains("rime: warning: no .lua or .luau files under `.` that git does not ignore")
            && run.stderr.contains("--no-gitignore"),
        "{}",
        run.stderr
    );

    // The hint is true: the flag finds them.
    let run = r.check(&["--no-gitignore", "."]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(run.stderr.contains("Rime checked 2 files"), "{}", run.stderr);
}
