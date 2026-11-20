//! Requires written as strings, end to end through `rime check`.
//!
//! A string that resolves is followed like an instance path. A string Rime
//! cannot follow (a loader's `require("Name")`, an alias nothing defines)
//! is said once per run on stderr, not once per call as an error. A file
//! that rebinds `require` has its calls through the new binding left alone.

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
        let dir = std::env::temp_dir().join(format!("rime-strings-{name}-{}", std::process::id()));
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

    fn project(&self) -> &Self {
        self.file(
            "default.project.json",
            r#"{ "name": "fixture", "tree": { "$className": "DataModel",
                "ReplicatedStorage": { "Shared": { "$path": "src" } } } }"#,
        )
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

    fn plain(&self) -> Run {
        self.check(&["--plain", "src"])
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

const DEP: &str = "local dep = {}\n\nfunction dep.grant(player: string, amount: number, reason: string): boolean\n\
                   \tprint(player, amount, reason)\n\treturn true\nend\n\nreturn table.freeze(dep)\n";

fn count(text: &str, needle: &str) -> usize {
    text.lines().filter(|l| l.contains(needle)).count()
}

fn notes(run: &Run) -> Vec<&str> {
    run.stderr.lines().filter(|l| l.starts_with("rime: note:")).collect()
}

#[test]
fn relative_strings_resolve_and_carry_cross_file_checks() {
    let d = Dir::new("relative");
    d.project()
        .file("src/dep.luau", DEP)
        .file("src/sub/user.luau", "local Dep = require(\"../dep\")\nDep.grant(\"a\", 1)\nreturn table.freeze({})\n")
        .file("src/main.luau", "local Dep = require(\"./dep\")\nDep.grant(\"a\", 1, \"b\")\nreturn table.freeze({})\n");
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R0502"), 0, "a string that resolves is not dynamic: {}", run.stdout);
    assert_eq!(count(&run.stdout, "R0301"), 1, "the call is checked against dep.luau: {}", run.stdout);
    assert!(notes(&run).is_empty(), "nothing unfollowed: {}", run.stderr);
}

#[test]
fn a_relative_string_to_nothing_is_a_broken_require() {
    let d = Dir::new("relative-missing");
    d.project().file("src/main.luau", "local Gone = require(\"./gone\")\nreturn table.freeze({ Gone = Gone })\n");
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R0501"), 1, "{}", run.stdout);
    assert_eq!(count(&run.stdout, "R0502"), 0, "{}", run.stdout);
    assert_eq!(run.code, 1, "{}", run.stderr);
}

#[test]
fn init_files_follow_the_luau_rule_and_self() {
    // `pack/init.luau` IS the module `pack`: `./x` is beside the folder,
    // `@self/x` is inside it.
    let d = Dir::new("init");
    d.project()
        .file("src/dep.luau", DEP)
        .file("src/pack/inner.luau", DEP)
        .file(
            "src/pack/init.luau",
            "local Dep = require(\"./dep\")\nlocal Inner = require(\"@self/inner\")\n\
             Dep.grant(\"a\", 1, \"b\")\nInner.grant(\"a\", 1, \"b\")\nreturn table.freeze({})\n",
        )
        .file("src/main.luau", "local Pack = require(\"./pack\")\nreturn table.freeze({ Pack = Pack })\n");
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R050"), 0, "{}", run.stdout);
    assert!(notes(&run).is_empty(), "{}", run.stderr);
}

#[test]
fn luaurc_aliases_resolve() {
    let d = Dir::new("alias");
    d.project()
        .file(".luaurc", "{\n  // comments and trailing commas are allowed\n  \"aliases\": { \"Lib\": \"./src/lib\", },\n}\n")
        .file("src/lib/dep.luau", DEP)
        .file(
            "src/main.luau",
            "local Dep = require(\"@Lib/dep\")\nlocal Gone = require(\"@lib/gone\")\nDep.grant(\"a\", 1)\n\
             return table.freeze({ Gone = Gone })\n",
        );
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R0301"), 1, "checked through the alias: {}", run.stdout);
    assert_eq!(count(&run.stdout, "R0501"), 1, "the alias folder exists, the module does not: {}", run.stdout);
    assert_eq!(count(&run.stdout, "R0502"), 0, "{}", run.stdout);
}

#[test]
fn an_init_file_looks_for_aliases_from_the_folder_above_its_own() {
    // `pkg/init.luau` IS the module `pkg`, and Luau starts the `.luaurc`
    // search at a module's parent: the `.luaurc` inside `pkg` is not in
    // reach of the init file, and is in reach of every other file there.
    let root_leaf = "local leaf = {}\n\nfunction leaf.fromRoot() end\n\nreturn table.freeze(leaf)\n";
    let inner_leaf = "local leaf = {}\n\nfunction leaf.fromInner() end\n\nreturn table.freeze(leaf)\n";
    let d = Dir::new("init-alias");
    d.project()
        .file(".luaurc", r#"{ "aliases": { "Dup": "./src/root" } }"#)
        .file("src/pkg/.luaurc", r#"{ "aliases": { "Dup": "./inner", "Inner": "./inner" } }"#)
        .file("src/root/leaf.luau", root_leaf)
        .file("src/pkg/inner/leaf.luau", inner_leaf)
        .file(
            "src/pkg/init.luau",
            "local Dup = require(\"@Dup/leaf\")\nlocal Inner = require(\"@Inner/leaf\")\nDup.fromRoot()\n\
             return table.freeze({ Inner = Inner })\n",
        )
        .file(
            "src/pkg/plain.luau",
            "local Dup = require(\"@Dup/leaf\")\nlocal Inner = require(\"@Inner/leaf\")\nDup.fromInner()\n\
             Inner.fromInner()\nreturn table.freeze({})\n",
        );
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R0306"), 0, "`@Dup` from the init file is the root's: {}", run.stdout);
    assert_eq!(count(&run.stdout, "R050"), 0, "{}", run.stdout);
    let notes = notes(&run);
    assert_eq!(notes.len(), 1, "`@Inner` is not defined for the init file: {}", run.stderr);
    assert!(notes[0].contains("@inner"), "{}", notes[0]);
    assert!(notes[0].contains("1 require in 1 file"), "{}", notes[0]);
}

#[test]
fn letter_case_is_checked_on_every_part_of_a_path() {
    // Windows and macOS open `util/helper.luau` for `Util/helper`; Roblox
    // and Luau on Linux do not. The answer is the strict one on every
    // system, for a folder in the middle of a path as for the last name.
    let d = Dir::new("case-parts");
    d.project()
        .file(".luaurc", r#"{ "aliases": { "Shared": "./src/shared" } }"#)
        .file("src/shared/alpha.luau", DEP)
        .file("src/shared/util/helper.luau", DEP)
        .file(
            "src/server/wrong.luau",
            "local A = require(\"@Shared/Alpha\")\nlocal B = require(\"@Shared/Util/helper\")\n\
             local C = require(\"../Shared/alpha\")\nlocal D = require(script.Parent.Parent.Shared.alpha)\n\
             return table.freeze({ A = A, B = B, C = C, D = D })\n",
        )
        .file(
            "src/server/right.luau",
            "local A = require(\"@Shared/alpha\")\nlocal B = require(\"@shared/util/helper\")\n\
             local C = require(\"../shared/alpha\")\nlocal D = require(script.Parent.Parent.shared.alpha)\n\
             A.grant(\"a\", 1)\nB.grant(\"a\", 1)\nC.grant(\"a\", 1)\nD.grant(\"a\", 1)\nreturn table.freeze({})\n",
        );
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R0501"), 4, "each wrong spelling is a broken require: {}", run.stdout);
    for wrong in ["@Shared/Alpha", "@Shared/Util/helper", "../Shared/alpha"] {
        assert!(run.stdout.contains(wrong), "{wrong} is reported: {}", run.stdout);
    }
    assert_eq!(count(&run.stdout, "R0301"), 4, "each right spelling is followed: {}", run.stdout);
    assert!(notes(&run).is_empty(), "{}", run.stderr);
}

#[test]
fn an_alias_value_is_spelled_as_on_disk() {
    // `./SRC/lib` names the folder `src/lib` on Windows and macOS and
    // nothing on Linux. On every system it is an alias to a path not on disk.
    let d = Dir::new("case-alias");
    d.project()
        .file(".luaurc", r#"{ "aliases": { "Lib": "./SRC/lib", "Good": "./src/lib" } }"#)
        .file("src/lib/dep.luau", DEP)
        .file(
            "src/main.luau",
            "local A = require(\"@Lib/dep\")\nlocal B = require(\"@Good/dep\")\nB.grant(\"a\", 1)\n\
             return table.freeze({ A = A })\n",
        );
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R0301"), 1, "the alias spelled as on disk is followed: {}", run.stdout);
    assert_eq!(count(&run.stdout, "R050"), 0, "{}", run.stdout);
    let notes = notes(&run);
    assert_eq!(notes.len(), 1, "{}", run.stderr);
    assert!(notes[0].contains("@lib") && notes[0].contains("not on"), "{}", notes[0]);
    assert!(notes[0].contains("1 require in 1 file"), "{}", notes[0]);
}

#[test]
fn strings_rime_cannot_follow_are_said_once() {
    let d = Dir::new("unfollowed");
    d.project()
        .file("src/a.luau", "local One = require(\"One\")\nlocal Two = require(\"Two\")\nreturn table.freeze({ One = One, Two = Two })\n")
        .file("src/b.luau", "local One = require(\"One\")\nlocal Kit = require(\"@kit/part\")\nreturn table.freeze({ One = One, Kit = Kit })\n");
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R050"), 0, "no error per require: {}", run.stdout);
    let notes = notes(&run);
    assert_eq!(notes.len(), 2, "one line per kind: {}", run.stderr);
    let bare = notes.iter().find(|l| l.contains("require(\"Name\")")).unwrap_or_else(|| panic!("{}", run.stderr));
    assert!(bare.contains("3 requires in 2 files"), "{bare}");
    let alias = notes.iter().find(|l| l.contains("@kit")).unwrap_or_else(|| panic!("{}", run.stderr));
    assert!(alias.contains("1 require in 1 file"), "{alias}");
    assert!(alias.contains(".luaurc"), "{alias}");
    assert_eq!(run.code, 0, "{}\n{}", run.stdout, run.stderr);
}

#[test]
fn json_output_carries_the_notes() {
    let d = Dir::new("json");
    d.project().file("src/a.luau", "local One = require(\"One\")\nreturn table.freeze({ One = One })\n");
    let run = d.check(&["--format", "json", "src"]);
    let v: serde_json::Value = serde_json::from_str(&run.stdout).unwrap_or_else(|e| panic!("{e}: {}", run.stdout));
    let notes = v["notes"].as_array().unwrap_or_else(|| panic!("no notes array: {}", run.stdout));
    assert_eq!(notes.len(), 1, "{}", run.stdout);
    assert!(notes[0].as_str().unwrap().contains("1 require in 1 file"), "{}", run.stdout);
    assert_eq!(v["diagnostics"].as_array().unwrap().len(), 0, "{}", run.stdout);
}

#[test]
fn a_rebound_require_is_left_alone() {
    let d = Dir::new("rebound");
    d.project()
        .file("src/loader.luau", "return function(name: string)\n\treturn name\nend\n")
        .file(
            "src/a.luau",
            "local require = require(script.Parent.loader)\nlocal One = require(\"One\")\nlocal Two = require(\"Two\")\n\
             return table.freeze({ One = One, Two = Two })\n",
        )
        .file(
            "src/b.luau",
            "local function run(require)\n\treturn require(\"Three\")\nend\nreturn table.freeze({ run = run })\n",
        );
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R050"), 0, "{}", run.stdout);
    let notes = notes(&run);
    assert_eq!(notes.len(), 1, "{}", run.stderr);
    assert!(notes[0].contains("3 requires in 2 files"), "{}", notes[0]);
    assert!(notes[0].contains("local require"), "{}", notes[0]);
}

/// `count` copies of `part/`.
fn deep(part: &str, count: usize) -> String {
    format!("{part}/").repeat(count)
}

#[test]
fn a_path_of_very_many_parts_costs_time_in_proportion() {
    // 16,000 parts in each of two requires, both just under the longest
    // path an OS takes. Building the path one part
    // at a time re-read the whole path at each step, so this took 10.9 s
    // and four times as many parts took 144.5 s.
    let d = Dir::new("many-parts");
    d.project()
        .file(".luaurc", r#"{ "aliases": { "x": "./src" } }"#)
        .file(
            "src/main.luau",
            &format!(
                "local A = require(\"@x/{}lib\")\nlocal B = require(\"./{}lib\")\nlocal C = require(\"@game/ReplicatedStorage/Shared/{}lib\")\n\
                 return {{ A = A, B = B, C = C }}\n",
                deep("a", 16_000),
                deep("b", 16_000),
                deep("c", 16_000),
            ),
        );
    let started = std::time::Instant::now();
    let run = d.plain();
    let took = started.elapsed();
    assert_eq!(count(&run.stdout, "R0501"), 3, "nothing is there: {}", run.stderr);
    assert!(took.as_secs_f64() < 3.0, "took {took:?}");
}

#[test]
fn a_string_longer_than_any_path_is_not_walked() {
    // Past 32,767 characters no file system holds the path, so there is
    // nothing to look for: one R0501 that does not print the whole string.
    let d = Dir::new("too-long");
    d.project()
        .file(".luaurc", &format!(r#"{{ "aliases": {{ "far": "./{}src", "near": "./src" }} }}"#, deep("a", 100_000)))
        .file("src/lib.luau", "return {}\n")
        .file(
            "src/main.luau",
            &format!(
                "local A = require(\"./{}lib\")\nlocal B = require(\"@near/{}lib\")\nlocal C = require(\"@far/lib\")\n\
                 return {{ A = A, B = B, C = C }}\n",
                deep("b", 100_000),
                deep("c", 100_000),
            ),
        );
    let started = std::time::Instant::now();
    let run = d.plain();
    let took = started.elapsed();
    assert_eq!(count(&run.stdout, "R0501"), 2, "{}", run.stderr);
    assert!(run.stdout.contains("no path is 200005 characters long"), "{}", &run.stdout[..run.stdout.len().min(2000)]);
    assert!(run.stdout.len() < 20_000, "the string is not printed whole: {} bytes", run.stdout.len());
    let notes = notes(&run);
    assert_eq!(notes.len(), 1, "{}", run.stderr);
    assert!(notes[0].contains("@far"), "an alias value that long is not on disk: {}", notes[0]);
    assert!(took.as_secs_f64() < 3.0, "took {took:?}");
}

#[test]
fn local_require_equals_require_is_still_the_builtin() {
    // `local require = require` renames nothing, so
    // the broken require below it is still reported and nothing is said
    // about a rebinding.
    let d = Dir::new("rebound-to-itself");
    d.project().file("src/lib.luau", "return { ok = function() end }\n").file(
        "src/localized.luau",
        "local require = require\nlocal lib = require(script.Parent.lib)\n\
         local gone = require(script.Parent.gone)\nlocal other = require(\"./alsoGone\")\n\
         return { lib = lib, gone = gone, other = other }\n",
    );
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R0501"), 2, "{}", run.stdout);
    assert!(notes(&run).is_empty(), "the name keeps its meaning: {}", run.stderr);
    assert_eq!(run.code, 1, "{}", run.stderr);
}

#[test]
fn the_builtin_require_that_feeds_a_rebinding_is_still_checked() {
    // `local require = require(X)`: the right-hand `require` is the
    // builtin (the new local is not in scope yet).
    let d = Dir::new("rebound-rhs");
    d.project().file(
        "src/a.luau",
        "local require = require(script.Parent.gone)\nlocal One = require(\"One\")\nreturn table.freeze({ One = One })\n",
    );
    let run = d.plain();
    assert_eq!(count(&run.stdout, "R0501"), 1, "{}", run.stdout);
    assert_eq!(count(&run.stdout, "R0502"), 0, "{}", run.stdout);
}
