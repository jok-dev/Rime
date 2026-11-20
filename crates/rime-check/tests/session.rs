//! Differential: a Session driven through edits must equal a fresh
//! check_project over the same in-memory files, after EVERY step.
//! Runs over every corpus directory case with the corpus's
//! own SiblingResolver.

#[path = "common/sibling.rs"]
mod sibling;
use sibling::SiblingResolver;

use rime_check::project::{check_project, FileReport, Resolver};
use rime_check::session::Session;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests").canonicalize().unwrap()
}

fn dir_cases() -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.join("main.luau").is_file() { out.push(p); } else { walk(&p, out); }
            }
        }
    }
    walk(&corpus_root(), &mut out);
    out.sort();
    out
}

fn load(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut files: Vec<(PathBuf, String)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "luau"))
        .map(|p| { let s = std::fs::read_to_string(&p).unwrap(); (p, s) })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

fn by_path(reports: Vec<FileReport>) -> BTreeMap<PathBuf, (Vec<rime_diag::Diagnostic>, usize)> {
    reports.into_iter().map(|r| (r.path, (r.diagnostics, r.suppressed))).collect()
}

/// Session state must equal a fresh run over `files`. `changed` must be
/// exactly the set of paths whose report differs from `before`.
fn assert_parity(
    step: &str,
    case: &Path,
    session: &Session<'_>,
    files: &[(PathBuf, String)],
    before: &BTreeMap<PathBuf, (Vec<rime_diag::Diagnostic>, usize)>,
    changed: &[PathBuf],
) -> BTreeMap<PathBuf, (Vec<rime_diag::Diagnostic>, usize)> {
    let fresh = by_path(check_project(files, &SiblingResolver));
    let got = by_path(session.reports());
    assert_eq!(got, fresh, "{}: session != fresh check after `{step}`", case.display());
    let mut expect_changed: Vec<&PathBuf> = got
        .iter()
        .filter(|(p, r)| before.get(*p) != Some(r))
        .map(|(p, _)| p)
        .chain(before.keys().filter(|p| !got.contains_key(*p)))
        .collect();
    expect_changed.sort();
    expect_changed.dedup();
    let mut changed: Vec<&PathBuf> = changed.iter().collect();
    changed.sort();
    assert_eq!(changed, expect_changed, "{}: changed-set after `{step}`", case.display());
    got
}

/// `src` with the FIRST `require(` retargeted at a module that does not
/// exist, or `None` when the file has no require at all. Shared with
/// `rime dev session-diff`'s mutation table (`main.rs`).
fn retargeted(src: &str) -> Option<String> {
    let at = src.find("require(")?;
    let mut out = src.to_string();
    out.replace_range(at..at + "require(".len(), "require(script.Parent.__rime_missing_");
    Some(out)
}

fn set(files: &mut Vec<(PathBuf, String)>, path: &Path, src: &str) {
    if let Some(f) = files.iter_mut().find(|(p, _)| p == path) {
        f.1 = src.to_string();
    } else {
        files.push((path.to_path_buf(), src.to_string()));
    }
}

#[test]
fn session_matches_fresh_check_after_every_edit() {
    let cases = dir_cases();
    assert!(!cases.is_empty(), "no corpus dir cases found");
    for case in cases {
        let mut files = load(&case);
        let originals = files.clone();
        let mut session = Session::new(
            files.clone(),
            Some(Box::new(SiblingResolver)),
            rime_std::StdNames::bundled().clone(),
            None,
        );
        // With an empty `before`, `assert_parity` treats every path as
        // changed, so the first call must pass every path as the
        // changed-set rather than `&[]`.
        let all: Vec<PathBuf> = files.iter().map(|f| f.0.clone()).collect();
        // `new` reports everything as changed relative to nothing; the
        // returned map IS the actual-state baseline (`assert_parity` returns
        // `got`, i.e. `by_path(session.reports())`).
        let mut before = assert_parity("new", &case, &session, &files, &BTreeMap::new(), &all);

        for (path, original) in &originals {
            let mut edits: Vec<(&str, String)> = vec![
                ("append statement", format!("{original}\nlocal __rime_probe = 1\n")),
                ("shift lines", format!("-- probe\n{original}")),
                ("parse error", format!("{original}\nlocal = \n")),
            ];
            // Point the first require at a module that does not exist: the
            // one edit shape that moves the require GRAPH rather than a
            // file's own text ("retarget a require to a missing
            // module and back" — "back" is the `restore` step below).
            if let Some(src) = retargeted(original) {
                edits.push(("retarget require", src));
            }
            edits.push(("restore", original.clone()));
            for (name, src) in edits {
                set(&mut files, path, &src);
                let changed = session.update(path, src);
                before = assert_parity(name, &case, &session, &files, &before, &changed);
            }
        }

        // Remove a dependency and put it back.
        let main = case.join("main.luau");
        if let Some((dep, dep_src)) = originals.iter().find(|(p, _)| *p != main).cloned() {
            files.retain(|(p, _)| *p != dep);
            let changed = session.remove(&dep);
            before = assert_parity("remove dep", &case, &session, &files, &before, &changed);
            files.push((dep.clone(), dep_src.clone()));
            let changed = session.add(dep.clone(), dep_src);
            before = assert_parity("add dep back", &case, &session, &files, &before, &changed);
        }

        // Full reset with one file changed, then back to the originals.
        let (main_path, main_src) = originals.iter().find(|(p, _)| *p == main).cloned().unwrap();
        set(&mut files, &main_path, &format!("{main_src}\nlocal __rime_reset = 2\n"));
        let changed = session.reset(files.clone(), Some(Box::new(SiblingResolver)), rime_std::StdNames::bundled().clone());
        before = assert_parity("reset changed", &case, &session, &files, &before, &changed);
        files = originals.clone();
        let changed = session.reset(files.clone(), Some(Box::new(SiblingResolver)), rime_std::StdNames::bundled().clone());
        let _ = assert_parity("reset originals", &case, &session, &files, &before, &changed);
    }
}

/// A `return require(...)` cycle (`a` -> `b` -> `a`) makes `interface_and_chain`
/// push the starting file back onto its own chain within the 9-hop cap, which
/// can put `path` itself into the `chains[path]` set that `remove` hands to
/// `recompute_interfaces` as `linkers`. None of the corpus dir cases (54
/// today) define a require cycle, so this is not exercised by
/// `session_matches_fresh_check_after_every_edit` above:
/// `recompute_interfaces` must not panic on an unknown path.
#[test]
fn remove_survives_a_self_referential_linker_cycle() {
    let dir = std::env::temp_dir().join(format!("rime-session-cycle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let a = dir.join("a.luau");
    let b = dir.join("b.luau");
    let main = dir.join("main.luau");
    let a_src = "--!rime\nreturn require(script.Parent.b)\n".to_string();
    let b_src = "--!rime\nreturn require(script.Parent.a)\n".to_string();
    let main_src = "--!rime\nlocal A = require(script.Parent.a)\n".to_string();
    std::fs::write(&a, &a_src).unwrap();
    std::fs::write(&b, &b_src).unwrap();
    std::fs::write(&main, &main_src).unwrap();

    let mut files: Vec<(PathBuf, String)> = vec![
        (a.clone(), a_src.clone()),
        (b.clone(), b_src.clone()),
        (main.clone(), main_src.clone()),
    ];
    files.sort_by(|x, y| x.0.cmp(&y.0));

    let mut session = Session::new(
        files.clone(),
        Some(Box::new(SiblingResolver)),
        rime_std::StdNames::bundled().clone(),
        None,
    );
    let all: Vec<PathBuf> = files.iter().map(|f| f.0.clone()).collect();
    let mut before = assert_parity("cycle new", &dir, &session, &files, &BTreeMap::new(), &all);

    // Remove `a`: its own chain (a -> b -> a -> ...) can put `a` itself into
    // `chains[a]`, which `remove` reads as the `linkers` to recompute. This
    // must not panic.
    files.retain(|(p, _)| *p != a);
    let changed = session.remove(&a);
    before = assert_parity("cycle remove a", &dir, &session, &files, &before, &changed);

    // Put it back.
    files.push((a.clone(), a_src.clone()));
    files.sort_by(|x, y| x.0.cmp(&y.0));
    let changed = session.add(a.clone(), a_src);
    let _ = assert_parity("cycle add a back", &dir, &session, &files, &before, &changed);

    let _ = std::fs::remove_dir_all(&dir);
}

/// Editing a body without touching the export shape must re-run only that
/// file; changing the interface must re-run its consumers too.
#[test]
fn affected_set_follows_the_require_graph() {
    let case = corpus_root().join("calls/accept/041_crossfile_ok");
    let files = load(&case);
    let main = case.join("main.luau");
    let dep = files.iter().map(|(p, _)| p.clone()).find(|p| *p != main).expect("a dependency file");
    let dep_src = files.iter().find(|(p, _)| *p == dep).unwrap().1.clone();
    let mut session = Session::new(files, Some(Box::new(SiblingResolver)), rime_std::StdNames::bundled().clone(), None);

    // Body-only edit that leaves every declaration's span untouched: the
    // interface compares equal, only the dep itself re-runs. Inserted right
    // before the file's terminal `return` rather than appended after it —
    // Luau requires `return` to be a block's LAST statement, and every dep
    // with a non-open interface necessarily ends in one (`module_interface`
    // only reads a `Return` terminator), so appending past it is a parse
    // error, not a body-only edit.
    let return_at = dep_src.rfind("\nreturn ").expect("dep exports via a terminal return");
    let (before_return, from_return) = dep_src.split_at(return_at);
    session.update(&dep, format!("{before_return}\nlocal __rime_probe = 1{from_return}"));
    assert_eq!(session.last_affected(), &[dep.clone()]);

    // Shifting lines moves declaration spans: the interface differs, so the
    // consumer re-runs as well.
    session.update(&dep, format!("-- probe\n{dep_src}"));
    let mut affected = session.last_affected().to_vec();
    affected.sort();
    let mut expect = vec![dep.clone(), main.clone()];
    expect.sort();
    assert_eq!(affected, expect);
}

/// Retargeting a linker whose new terminal has an interface EQUAL to the old
/// one still has to re-run the linker's consumers: the terminal path itself
/// is what the require graph and R0309 attribution key on, so
/// `recompute_interfaces`'s `terminal_changed` arm — not interface
/// inequality — is what must put them in the affected set.
///
/// The two terminals are `x/t.luau` and `y/t.luau`: identical text AND
/// identical file stem, which is what makes their `ModuleInterface`s compare
/// equal. (`t1.luau`/`t2.luau` would differ in the interface's own `display`
/// and `file` fields and take the ordinary interface-changed path instead,
/// testing nothing about terminals.)
#[test]
fn linker_retarget_with_equal_interface_reruns_consumers() {
    let dir = std::env::temp_dir().join(format!("rime-session-retarget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("x")).unwrap();
    std::fs::create_dir_all(dir.join("y")).unwrap();

    let terminal =
        "--!rime\nlocal t = {}\n\nfunction t.f(): number\n\treturn 1\nend\n\nreturn t\n".to_string();
    let a = dir.join("a.luau");
    let a_src = "--!rime\nreturn require(script.Parent.x.t)\n".to_string();
    let xt = dir.join("x").join("t.luau");
    let yt = dir.join("y").join("t.luau");
    let main = dir.join("main.luau");
    let main_src = "--!rime\nlocal a = require(script.Parent.a)\n\nlocal main = {}\n\n\
                    function main.run(): number\n\treturn a.f()\nend\n\nreturn main\n"
        .to_string();
    std::fs::write(&a, &a_src).unwrap();
    std::fs::write(&xt, &terminal).unwrap();
    std::fs::write(&yt, &terminal).unwrap();
    std::fs::write(&main, &main_src).unwrap();

    let mut files: Vec<(PathBuf, String)> = vec![
        (a.clone(), a_src),
        (main.clone(), main_src),
        (xt.clone(), terminal.clone()),
        (yt.clone(), terminal),
    ];
    files.sort_by(|p, q| p.0.cmp(&q.0));

    let mut session = Session::new(
        files.clone(),
        Some(Box::new(SiblingResolver)),
        rime_std::StdNames::bundled().clone(),
        None,
    );
    let all: Vec<PathBuf> = files.iter().map(|f| f.0.clone()).collect();
    let before = assert_parity("retarget new", &dir, &session, &files, &BTreeMap::new(), &all);

    let retargeted = "--!rime\nreturn require(script.Parent.y.t)\n".to_string();
    set(&mut files, &a, &retargeted);
    let changed = session.update(&a, retargeted);
    assert!(
        session.last_affected().contains(&main),
        "the linker's consumer must re-run when its terminal moves: {:?}",
        session.last_affected()
    );
    let _ = assert_parity("retarget linker", &dir, &session, &files, &before, &changed);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A resolver rooted at `Loc::Fs` — the corpus `SiblingResolver` computes
/// `require(script.Parent.X)` paths directly and has no `step`/`instance_of`/
/// `resolve_loc` primitives yet (those land with the corpus resolver in a
/// later task), so the evaluator-driven fan-out below needs its own tiny
/// resolver. `resolve` always answers `Dynamic`: every require site in this
/// test goes through the evaluator, never the cheap path, so both the
/// static `require(script.Parent.Dep)` and the dynamic `require(M.Sub)`
/// exercise the SAME instance-walk primitives.
struct EvalFsResolver;

impl rime_check::project::Resolver for EvalFsResolver {
    fn resolve(
        &self,
        _from: &Path,
        _target: &rime_hir::Expr,
        _env: &std::collections::HashMap<String, &rime_hir::Expr>,
    ) -> rime_check::project::Resolution {
        rime_check::project::Resolution::Dynamic
    }

    fn step(
        &self,
        base: &rime_check::project::Loc,
        seg: &str,
    ) -> Option<rime_check::project::Loc> {
        let rime_check::project::Loc::Fs(p) = base else { return None };
        let mut p = p.clone();
        if seg == "Parent" {
            p.pop();
        } else {
            p.push(seg);
        }
        Some(rime_check::project::Loc::Fs(p))
    }

    fn instance_of(&self, file: &Path) -> Option<rime_check::project::Loc> {
        Some(rime_check::project::Loc::Fs(file.with_extension("")))
    }

    fn resolve_loc(&self, loc: &rime_check::project::Loc) -> rime_check::project::Resolution {
        let rime_check::project::Loc::Fs(p) = loc else {
            return rime_check::project::Resolution::Dynamic;
        };
        let candidate = p.with_extension("luau");
        if candidate.is_file() {
            rime_check::project::Resolution::Found(candidate)
        } else {
            rime_check::project::Resolution::Missing { described: p.display().to_string() }
        }
    }
}

/// The body-sensitivity case: `Dep.luau` exports
/// `M.Sub` as the return value of a local function `t`, whose body reads
/// `script.Parent.Target` — an instance chain the evaluator only resolves by
/// reading `Dep.luau`'s own HIR (`export_field`/`table_field` walking into
/// `t`'s body). `main.luau` requires `Dep` AND requires `M.Sub` — the second
/// require's target is only resolvable BECAUSE the evaluator read `Dep`'s
/// HIR, an edge `consumers` (require-target bindings) does not capture on
/// its own.
///
/// Editing `Target` -> `Targe2` (same byte length, so every span — and thus
/// `Dep`'s `ModuleInterface`, which is keyed by field name -> span — compares
/// EQUAL before and after) inside `t`'s body must still fan out to
/// `main.luau`: proof the fan-out comes from `eval_consumers`, not from the
/// existing interface-change path through `consumers`.
#[test]
fn eval_deps_fan_out_on_body_only_edit() {
    let dir = std::env::temp_dir().join(format!("rime-session-eval-deps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let main = dir.join("main.luau");
    let dep = dir.join("Dep.luau");
    let target = dir.join("Target.luau");

    let main_src =
        "--!rime\nlocal M = require(script.Parent.Dep)\nlocal S = require(M.Sub)\n".to_string();
    let dep_v1 = "--!rime\nlocal M = {}\nlocal function t() return script.Parent.Target end\nM.Sub = t()\nreturn M\n".to_string();
    let dep_v2 = "--!rime\nlocal M = {}\nlocal function t() return script.Parent.Targe2 end\nM.Sub = t()\nreturn M\n".to_string();
    let target_src = "--!rime\nreturn {}\n".to_string();

    std::fs::write(&main, &main_src).unwrap();
    std::fs::write(&dep, &dep_v1).unwrap();
    std::fs::write(&target, &target_src).unwrap();

    let files: Vec<(PathBuf, String)> =
        vec![(dep.clone(), dep_v1.clone()), (main.clone(), main_src.clone())];

    let mut session = Session::new(
        files,
        Some(Box::new(EvalFsResolver)),
        rime_std::StdNames::bundled().clone(),
        None,
    );

    let (report, _) = session.report(&main).expect("main is resident");
    assert!(
        !report.iter().any(|d| d.code == rime_diag::Code(502)),
        "fresh session: main.luau must have no R0502 — the evaluator resolves \
         M.Sub through Dep's HIR: {report:?}"
    );

    let changed = session.update(&dep, dep_v2);
    assert!(
        changed.contains(&main),
        "a body-only edit to Dep.luau (interface-invariant) must still fan \
         out to main.luau via eval_consumers: changed = {changed:?}"
    );

    let (report, _) = session.report(&main).expect("main is resident");
    assert!(
        report.iter().any(|d| d.code == rime_diag::Code(501)),
        "main.luau must now report R0501 — M.Sub resolves to the now-missing \
         Targe2.luau: {report:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A gap a review caught: an eval dependency that is neither
/// a dependent's own `require()` target (`consumers`) nor a linker hop
/// (`chains`) — an INTERMEDIATE `require(...)` the evaluator only reaches by
/// chasing through a THIRD file's exported field. `Dep.luau`'s `Sub` field
/// is itself the value of `require(script.Parent.X).Y`: `X.luau` only lands
/// in `main.luau`'s `eval_deps` — recorded while `main.luau` resolves its
/// OWN `require(M.Sub)`, which chases into `Dep.luau`'s HIR and from there
/// into `X.luau`'s — never in `main.luau`'s `deps` (it never requires `X`
/// directly) or `chains` (`Dep.luau` isn't a linker). Removing `X.luau`
/// (deleted from disk first, so `resolve_loc` misses) must still re-run
/// `main.luau` through `eval_consumers`, or its report goes stale.
#[test]
fn eval_deps_fan_out_on_remove_of_intermediate_dep() {
    let dir = std::env::temp_dir()
        .join(format!("rime-session-eval-deps-remove-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let main = dir.join("main.luau");
    let dep = dir.join("Dep.luau");
    let x = dir.join("X.luau");
    let target = dir.join("Target.luau");

    let main_src =
        "--!rime\nlocal M = require(script.Parent.Dep)\nlocal S = require(M.Sub)\n".to_string();
    let dep_src = "--!rime\nlocal M = {}\nM.Sub = require(script.Parent.X).Y\nreturn M\n".to_string();
    let x_src = "--!rime\nlocal N = {}\nN.Y = script.Parent.Target\nreturn N\n".to_string();
    let target_src = "--!rime\nreturn {}\n".to_string();

    std::fs::write(&main, &main_src).unwrap();
    std::fs::write(&dep, &dep_src).unwrap();
    std::fs::write(&x, &x_src).unwrap();
    std::fs::write(&target, &target_src).unwrap();

    let files: Vec<(PathBuf, String)> = vec![
        (dep.clone(), dep_src.clone()),
        (main.clone(), main_src.clone()),
        (x.clone(), x_src.clone()),
    ];

    let mut session = Session::new(
        files,
        Some(Box::new(EvalFsResolver)),
        rime_std::StdNames::bundled().clone(),
        None,
    );

    let (report, _) = session.report(&main).expect("main is resident");
    assert!(
        !report.iter().any(|d| d.code == rime_diag::Code(501) || d.code == rime_diag::Code(502)),
        "fresh session: main.luau must resolve fully through Dep -> X -> Target: {report:?}"
    );

    // `X.luau` never becomes a require target of `main.luau` itself — only
    // `Dep.luau` calls `require(script.Parent.X)`, mid-eval, while
    // `main.luau`'s OWN evaluator chases `M.Sub` — and `Dep.luau` isn't a
    // linker, so neither `consumers` nor `chains` would ever widen
    // `main.luau` into the affected set here; only `eval_consumers` does.
    std::fs::remove_file(&x).unwrap();
    let changed = session.remove(&x);
    assert!(
        changed.contains(&main),
        "removing an intermediate eval dep must still fan out to main.luau \
         via eval_consumers: changed = {changed:?}"
    );

    let (report, _) = session.report(&main).expect("main is resident");
    assert!(
        report.iter().any(|d| d.code == rime_diag::Code(502)),
        "main.luau must now report R0502 — require(M.Sub) can no longer be \
         resolved now that X.luau is gone: {report:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// `SiblingResolver::resolve_loc`'s
/// `Missing::described` must render the FULL dotted segment chain, not just
/// the terminal file name — a two-hop instance miss (`<case>/A/B`, neither
/// `A/B.luau` nor `A` created on disk — `resolve_loc` only ever inspects
/// the leaf, `Loc::Fs` paths carry no on-disk requirement for their
/// intermediate segments) must describe as `script.Parent.A.B`, matching
/// `resolve`'s own dialect for the same two-segment chain, not the
/// pre-fix `script.Parent.B`.
#[test]
fn resolve_loc_missing_renders_full_segment_chain() {
    let dir = std::env::temp_dir()
        .join(format!("rime-sibling-resolve-loc-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Only `main.luau` needs to exist — it marks `dir` as the case dir
    // `display_rel` walks up to find; `A/B.luau` is deliberately never
    // created, so the two-hop instance below stays Missing.
    std::fs::write(dir.join("main.luau"), "--!rime\nreturn {}\n").unwrap();

    let inst = dir.join("A").join("B");
    let resolution =
        SiblingResolver.resolve_loc(&rime_check::project::Loc::Fs(inst));
    match resolution {
        rime_check::project::Resolution::Missing { described } => {
            assert_eq!(
                described, "script.Parent.A.B",
                "a two-hop miss must render the full chain, not just the leaf"
            );
        }
        other => panic!("expected Resolution::Missing, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir);
}
