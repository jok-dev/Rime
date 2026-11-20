//! Every edit the corpus produces, applied.
//!
//! A `Suggestion` is a span and a replacement. Editors and agents apply it
//! without reading it, so each one owes this much: the file still parses,
//! the diagnostic it came from is gone, and no other diagnostic took its
//! place. This walks the whole corpus and applies every suggestion of every
//! diagnostic, so a new rule's edits are held to that the day its corpus
//! case lands.
//!
//! It is a floor, not a proof. An edit can pass all three and still change
//! what the code means (`typetype(x) == "number"` parses, and Rime has no
//! rule for an unknown global), so a rule that builds an edit also tests
//! the exact text it produces, next to the rule.
//!
//! Advice has no place in `suggestions` (it goes in `Diagnostic::help`), so
//! an entry here that only deletes text is checked like any other edit.

use rime_diag::Diagnostic;
use std::path::{Path, PathBuf};

#[path = "common/sibling.rs"]
mod sibling;
use sibling::SiblingResolver;

/// One corpus case: the files checked together, and which one is judged.
struct Case {
    name: String,
    files: Vec<(PathBuf, String)>,
    main: usize,
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests")
        .canonicalize()
        .expect("tests/ directory")
}

/// The same cases `corpus.rs` runs: a directory holding `main.luau` is one
/// multi-file case, every other `.luau` outside such a directory is a
/// single-file case.
fn cases() -> Vec<Case> {
    let root = corpus_root();
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(&root).into_iter().filter_map(Result::ok) {
        let path = entry.path().to_path_buf();
        let name = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy().into_owned();
        if entry.file_type().is_dir() && path.join("main.luau").is_file() {
            let mut files = Vec::new();
            for member in std::fs::read_dir(&path).unwrap() {
                let p = member.unwrap().path();
                if p.extension().is_some_and(|x| x == "luau") {
                    let src = std::fs::read_to_string(&p).unwrap();
                    files.push((p, src));
                }
            }
            files.sort_by(|a, b| a.0.cmp(&b.0));
            let main = files.iter().position(|(p, _)| p.ends_with("main.luau")).unwrap();
            out.push(Case { name, files, main });
            continue;
        }
        let in_dir_case = path
            .ancestors()
            .skip(1)
            .take_while(|a| a.starts_with(&root))
            .any(|a| a.join("main.luau").is_file());
        if entry.file_type().is_file() && path.extension().is_some_and(|x| x == "luau") && !in_dir_case {
            let src = std::fs::read_to_string(&path).unwrap();
            out.push(Case { name, files: vec![(path, src)], main: 0 });
        }
    }
    out
}

/// The judged file's diagnostics, with `src` standing in for its text.
fn diagnostics(case: &Case, src: &str) -> Vec<Diagnostic> {
    if case.files.len() == 1 {
        return rime_check::check_source(src);
    }
    let mut files = case.files.clone();
    files[case.main].1 = src.to_string();
    let main_path = files[case.main].0.clone();
    rime_check::project::check_project(&files, &SiblingResolver)
        .into_iter()
        .find(|r| r.path == main_path)
        .map(|r| r.diagnostics)
        .unwrap_or_default()
}

#[test]
fn every_corpus_edit_parses_and_removes_its_diagnostic() {
    let mut applied = 0usize;
    let mut failures = Vec::new();
    for case in cases() {
        let src = &case.files[case.main].1;
        let before = diagnostics(&case, src);
        for d in &before {
            for s in &d.suggestions {
                applied += 1;
                let what = format!("{} {} `{}`", case.name, d.code, s.message);
                println!("{what}");
                let Some(edited) = rime_check::edits::apply(src, s) else {
                    failures.push(format!("{what}: the span is not inside the file"));
                    continue;
                };
                if rime_syntax::parse(&edited).is_err() {
                    failures.push(format!("{what}: the result does not parse\n{edited}"));
                    continue;
                }
                let count = |diags: &[Diagnostic], code| diags.iter().filter(|x| x.code == code).count();
                let after = diagnostics(&case, &edited);
                if count(&after, d.code) >= count(&before, d.code) {
                    failures.push(format!("{what}: {} is still reported\n{edited}", d.code));
                }
                // A fix that trades its diagnostic for another is not one.
                for new in &after {
                    if count(&after, new.code) > count(&before, new.code) {
                        failures.push(format!("{what}: the result is reported as {}\n{edited}", new.code));
                        break;
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} edit(s) failed:\n\n{}", failures.len(), failures.join("\n\n"));
    // The walk found the corpus and the corpus has edits in it; without
    // this an empty walk would pass.
    assert!(applied >= 10, "only {applied} edits found in the corpus");
    println!("{applied} corpus edits applied");
}
