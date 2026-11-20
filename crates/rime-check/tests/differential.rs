//! Differential runner (plan §8): the test that tests the claim.
//!
//! The theorem Rime sells is "checks clean => cannot raise a runtime type
//! error". The corpus harness checks our beliefs about the checker; this
//! harness checks the claim itself, by EXECUTING every `tests/accept/` case
//! under real Luau (mlua, vendored) and failing if execution raises anything.
//! Ground truth over beliefs.
//!
//! Contract per accept case:
//!
//!   1. real Luau must execute the file top-to-bottom without error
//!      (accept cases are self-contained modules ending in
//!      `return table.freeze({...})`, so running the top level is enough);
//!   2. Rime must accept it (zero diagnostics) — belt and suspenders with
//!      corpus.rs, but cheap, and it lets THIS harness name the real problem:
//!      when exactly one of (1)/(2) holds, the checker and the runtime
//!      disagree about which of them is wrong.
//!
//! Nothing is stripped from the source: the `--!rime` pragma and `--~`
//! expectation lines are legal Luau comments and go to the VM verbatim.
//!
//! Skip mechanism for engine-touching cases (`Instance` etc., which cannot
//! run outside Roblox): a line reading exactly `--!no-exec` in the leading
//! comment block (it cannot be the FIRST line — corpus.rs requires `--!rime`
//! there) marks a case static-only; its trial is registered but ignored, so
//! the skip stays visible in the test output. No current accept case needs
//! it.
//!
//! Future work (plan §8): mutation cases — deliberately break an accept case
//! (e.g. remove the freeze) and assert the runtime DOES throw, proving this
//! harness would have caught the regression.

use libtest_mimic::{Arguments, Failed, Trial};
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    let args = Arguments::from_args();
    let root = corpus_root();

    let mut trials = Vec::new();
    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "luau"))
    {
        let path = entry.path().to_path_buf();
        if !in_accept_bucket(&path) {
            continue; // reject buckets are corpus.rs territory; nothing to execute
        }
        let name = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        // Static-only opt-out: decided at discovery time so the case shows up
        // as "ignored" rather than silently passing.
        let static_only = std::fs::read_to_string(&path)
            .is_ok_and(|src| has_no_exec_optout(&src));
        trials.push(
            Trial::test(name, move || run_case(&path)).with_ignored_flag(static_only),
        );
    }

    let file_trials = trials.len();

    // Self-check for the skip scanner, registered as a real trial: with
    // `harness = false` a `#[cfg(test)] mod` never runs, and the scanner is
    // load-bearing (a broken one silently executes engine cases — or skips
    // nothing).
    trials.push(Trial::test("self::no_exec_optout_scanner", || {
        let cases: &[(&str, bool)] = &[
            ("--!rime\n--!no-exec\nlocal x = 1\n", true),
            ("--!rime\n\n  --!no-exec\nlocal x = 1\n", true),
            // past the leading comment block: not an opt-out
            ("--!rime\nlocal x = 1\n--!no-exec\n", false),
            ("--!rime\nlocal x = 1\n", false),
        ];
        for (src, want) in cases {
            if has_no_exec_optout(src) != *want {
                return Err(format!(
                    "has_no_exec_optout returned {} for {src:?}",
                    !want
                )
                .into());
            }
        }
        Ok(())
    }));

    if file_trials == 0 {
        eprintln!(
            "warning: no accept cases found under {} — the differential \
             runner has nothing to execute",
            root.display()
        );
    }

    libtest_mimic::run(&args, trials).exit_code()
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests")
        .canonicalize()
        .expect("tests/ directory")
}

/// Component-wise, not string `contains`: path separators differ across
/// platforms and Windows canonical paths carry a `\\?\` prefix (same
/// rationale as `bucket_of` in corpus.rs).
fn in_accept_bucket(path: &Path) -> bool {
    path.components()
        .any(|c| c.as_os_str().to_str() == Some("accept"))
}

/// True if a line in the leading comment block reads exactly `--!no-exec`.
/// The block is the run of comment/blank lines at the top of the file; the
/// scan stops at the first line of actual code so an opt-out cannot hide in
/// the body.
fn has_no_exec_optout(src: &str) -> bool {
    for line in src.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if !t.starts_with("--") {
            return false; // first code line ends the leading block
        }
        if t == "--!no-exec" {
            return true;
        }
    }
    false
}

fn run_case(path: &Path) -> Result<(), Failed> {
    let src = std::fs::read_to_string(path)?;

    let diags = rime_check::check_source(&src);
    let exec = execute_under_luau(path, &src);

    match (diags.is_empty(), exec) {
        // Checker clean, runtime clean: the theorem holds on this case.
        (true, Ok(())) => Ok(()),

        // Checker clean, runtime raised: the theorem is violated. The runtime
        // is ground truth, so the checker (or the case's placement) is wrong.
        (true, Err(e)) => Err(format!(
            "THEOREM VIOLATION: Rime accepts this module, but real Luau raised \
             an error executing it. The checker and the runtime disagree about \
             which one is wrong — and the runtime is ground truth.\n\
             Luau error:\n{e}"
        )
        .into()),

        // Checker rejects, runtime clean: disagreement the other way round.
        (false, Ok(())) => Err(format!(
            "checker/runtime disagreement: Rime rejects this accept/ case with \
             {} diagnostic(s), but real Luau executes it cleanly. Either a rule \
             is over-tight (fix it, or flag it conservative and move the case \
             to reject-conservative/) or the case is mis-bucketed.\n{}",
            diags.len(),
            summarize(&diags)
        )
        .into()),

        // Both reject: the case simply does not belong in accept/.
        (false, Err(e)) => Err(format!(
            "case is in accept/ but BOTH Rime and real Luau reject it — it \
             does not belong in this bucket.\nRime ({} diagnostic(s)):\n{}\
             Luau error:\n{e}",
            diags.len(),
            summarize(&diags)
        )
        .into()),
    }
}

/// Load and execute the module top-level under a fresh Luau VM. Fresh per
/// case: accept cases are self-contained modules, and isolation means one
/// case cannot pollute globals for another. The source goes in verbatim —
/// `--!rime` / `--~` lines are ordinary comments to Luau.
fn execute_under_luau(path: &Path, src: &str) -> Result<(), String> {
    let lua = mlua::Lua::new();
    let chunk_name = format!(
        "@{}",
        path.file_name().map_or_else(
            || path.to_string_lossy().into_owned(),
            |n| n.to_string_lossy().into_owned(),
        )
    );
    lua.load(src)
        .set_name(chunk_name)
        .exec()
        .map_err(|e| e.to_string())
}

fn summarize(diags: &[rime_diag::Diagnostic]) -> String {
    diags
        .iter()
        .map(|d| format!("  {}: {}\n", d.code, d.message))
        .collect()
}
