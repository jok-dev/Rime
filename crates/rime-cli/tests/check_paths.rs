//! What `rime check` does with the paths it is given before it checks
//! anything: a path that does not exist is an error (exit 2, nothing
//! checked) and a path that holds nothing to check is a warning (exit 0).
//!
//! Both used to print "Rime checked 0 files" and exit 0 — in a blocking CI
//! step, a typo in the path was a permanent green.

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
        let dir = std::env::temp_dir().join(format!("rime-paths-{name}-{}", std::process::id()));
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

    fn mkdir(&self, rel: &str) -> &Self {
        std::fs::create_dir_all(self.dir.join(rel)).unwrap();
        self
    }

    fn check(&self, args: &[&str]) -> Run {
        let out = Command::new(env!("CARGO_BIN_EXE_rime"))
            .arg("check")
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

const BAD: &str = "print(definitely_undefined_name)\n";

#[test]
fn a_path_that_does_not_exist_is_an_error() {
    let d = Dir::new("missing");
    d.file("src/a.luau", "return 1\n");
    for args in [&["nope"][..], &["--format", "json", "nope"][..], &["--plain", "nope"][..]] {
        let run = d.check(args);
        assert_eq!(run.code, 2, "{args:?}: {}", run.stderr);
        assert!(run.stderr.contains("rime: cannot check `nope`: "), "{args:?}: {}", run.stderr);
        assert!(run.stdout.is_empty(), "{args:?}: nothing on stdout, not even an empty JSON document: {}", run.stdout);
        assert!(!run.stderr.contains("Rime checked"), "{args:?}: no summary, nothing ran: {}", run.stderr);
    }
}

#[test]
fn one_missing_path_stops_the_run_even_beside_good_ones() {
    // Checking the paths that do exist and exiting 0 or 1 would hide the
    // typo behind a result that looks complete.
    let d = Dir::new("mixed");
    d.file("src/a.luau", BAD);
    let run = d.check(&["src", "nope", "also-nope"]);
    assert_eq!(run.code, 2, "{}", run.stderr);
    assert!(run.stderr.contains("cannot check `nope`"), "{}", run.stderr);
    assert!(run.stderr.contains("cannot check `also-nope`"), "every bad path is named: {}", run.stderr);
    assert!(!run.stderr.contains("Rime checked"), "{}", run.stderr);
    assert!(run.stdout.is_empty(), "{}", run.stdout);

    // The same paths without the missing ones: checked as before.
    let run = d.check(&["src"]);
    assert_eq!(run.code, 1, "{}", run.stderr);
    assert!(run.stderr.contains("Rime checked 1 files"), "{}", run.stderr);
}

#[test]
fn an_existing_path_with_nothing_to_check_warns_and_passes() {
    let d = Dir::new("empty");
    d.mkdir("empty").file("docs/notes.txt", "hi\n").file("src/a.luau", "return 1\n");

    let run = d.check(&["empty"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("rime: warning: no .lua or .luau files under `empty`"), "{}", run.stderr);
    assert!(run.stderr.contains("Rime checked 0 files"), "{}", run.stderr);

    // A directory with files, none of them source.
    let run = d.check(&["docs"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("warning: no .lua or .luau files under `docs`"), "{}", run.stderr);

    // A file named outright that is not a source file is skipped by the
    // walk; that is said, not silent.
    let run = d.check(&["docs/notes.txt"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("warning: `docs/notes.txt` is not a .lua or .luau file"), "{}", run.stderr);

    // The warning is per path: an empty one beside a full one still warns.
    let run = d.check(&["src", "empty"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("no .lua or .luau files under `empty`"), "{}", run.stderr);
    assert!(run.stderr.contains("Rime checked 1 files"), "{}", run.stderr);

    // JSON output is unaffected: the warning is on stderr.
    let run = d.check(&["--format", "json", "empty"]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    let v: serde_json::Value = serde_json::from_str(&run.stdout).expect("stdout is still JSON");
    assert_eq!(v["summary"]["checked"], 0);
    assert!(run.stderr.contains("warning: no .lua or .luau files under `empty`"), "{}", run.stderr);
}

#[test]
fn no_warning_when_there_was_something_to_check() {
    let d = Dir::new("quiet");
    d.file("src/a.luau", "return 1\n");
    for args in [&["src"][..], &["src/a.luau"][..], &["."][..], &[][..]] {
        let run = d.check(args);
        assert_eq!(run.code, 0, "{args:?}: {}", run.stderr);
        assert!(!run.stderr.contains("rime: warning"), "{args:?}: {}", run.stderr);
        assert!(run.stderr.contains("Rime checked 1 files"), "{args:?}: {}", run.stderr);
    }
}

#[test]
fn the_default_path_warns_in_an_empty_directory() {
    // No path means `.`; an empty working directory is the same trap.
    let d = Dir::new("default");
    let run = d.check(&[]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stderr.contains("warning: no .lua or .luau files under `.`"), "{}", run.stderr);
}

#[test]
fn a_run_that_rime_toml_ignores_entirely_says_so() {
    let d = Dir::new("ignored");
    d.file("rime.toml", "ignore = [\"vendor\"]\n").file("vendor/v.luau", BAD);
    let run = d.check(&["vendor"]);
    assert_eq!(run.code, 0, "ignored findings do not fail the run: {}", run.stderr);
    assert!(run.stderr.contains("Rime checked 0 files"), "{}", run.stderr);
    assert!(
        run.stderr.contains("rime: warning: nothing was checked") && run.stderr.contains("`ignore` in rime.toml"),
        "{}",
        run.stderr
    );
}
