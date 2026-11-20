//! Corpus harness. The corpus is the specification (plan §8).
//!
//! Discovers every `.luau` under `tests/`, runs the checker, and enforces the
//! bucket contract:
//!
//!   accept/               must produce zero diagnostics, and must RUN clean
//!   reject-unsound/       must produce >=1 non-conservative error
//!   reject-conservative/  must produce >=1 diagnostic, ALL conservative
//!
//! The last rule is the one that keeps the false-positive budget honest: if a
//! conservative rejection is ever emitted without `conservative: true`, it
//! lands in the wrong bucket and CI says so.
//!
//! Inline expectations, so a case and its expectation cannot drift:
//!
//! ```lua
//! --!rime
//! local t = {}
//! local f = table.freeze(t)
//! t.x = 1
//! --~ ERROR[R0041]: cannot write to a frozen table
//! --~ NOTE: frozen at line 3
//! ```
//!
//! A case that needs a rule's settings (what rime.toml would carry) says so
//! the same way; see `rule_options`:
//!
//! ```lua
//! --~ OPTION[R0312]: allow = Signal.new, *:IsServer
//! ```
//!
//! Full rendered output is additionally snapshotted via `insta`; run
//! `cargo insta review` to accept changes.

use libtest_mimic::{Arguments, Failed, Trial};
use std::path::{Path, PathBuf};

fn main() -> std::process::ExitCode {
    let args = Arguments::from_args();
    let root = corpus_root();

    let mut trials = Vec::new();
    trials.push(Trial::test("harness: expectation lines", expectation_lines_parse));
    for entry in walkdir::WalkDir::new(&root)
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path().to_path_buf();
        let name = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        // A directory containing main.luau is ONE multi-file case (cross-file
        // checking); its member files are not standalone cases.
        if entry.file_type().is_dir() && path.join("main.luau").is_file() {
            trials.push(Trial::test(name, move || run_dir_case(&path)));
            continue;
        }
        // Support files anywhere under a dir case (including nested
        // candidate dirs, e.g. 067's A/Maid.luau) are not standalone cases.
        let in_dir_case = path
            .ancestors()
            .skip(1)
            .take_while(|a| a.starts_with(&root))
            .any(|a| a.join("main.luau").is_file());
        if !entry.file_type().is_file()
            || !path.extension().is_some_and(|x| x == "luau")
            || in_dir_case
        {
            continue;
        }
        trials.push(Trial::test(name, move || run_case(&path)));
    }

    if trials.is_empty() {
        eprintln!("warning: no corpus cases found under {}", root.display());
    }

    libtest_mimic::run(&args, trials).exit_code()
}

#[path = "common/sibling.rs"]
mod sibling;
use sibling::SiblingResolver;

/// One directory = one case: all member .luau files are checked as a project;
/// expectations, bucket contract, and the snapshot judge main.luau's
/// diagnostics only (dep files must stay clean — a dirty dep fails loudly).
fn run_dir_case(dir: &Path) -> Result<(), Failed> {
    let bucket = bucket_of(dir)?;
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.extension().is_some_and(|x| x == "luau") {
            let src = std::fs::read_to_string(&p)?;
            files.push((p, src));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let main_path = dir.join("main.luau");
    let main_src = files
        .iter()
        .find(|(p, _)| p == &main_path)
        .map(|(_, s)| s.clone())
        .ok_or("dir case must contain main.luau")?;
    if !main_src.starts_with("--!rime") {
        return Err("main.luau must begin with the `--!rime` pragma".into());
    }

    let reports = rime_check::project::check_project(&files, &SiblingResolver);

    let mut main_diags = Vec::new();
    for r in &reports {
        if r.path == main_path {
            main_diags = r.diagnostics.clone();
        } else if !r.diagnostics.is_empty() {
            return Err(format!(
                "dep file {} produced diagnostics:\n{}",
                r.path.display(),
                render_all(&r.diagnostics, files.iter().find(|(p, _)| *p == r.path).map(|(_, s)| s.as_str()).unwrap_or(""))
            )
            .into());
        }
    }

    judge(bucket, &main_diags, &main_src, &parse_expectations(&main_src))?;

    let rendered = render_all(&main_diags, &main_src);
    let name = dir.file_name().unwrap().to_string_lossy().into_owned();
    insta::with_settings!({
        snapshot_path => dir.parent().unwrap().join("snapshots"),
        prepend_module_to_snapshot => false,
        omit_expression => true,
    }, {
        insta::assert_snapshot!(name, rendered);
    });
    Ok(())
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests")
        .canonicalize()
        .expect("tests/ directory")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Accept,
    RejectUnsound,
    RejectConservative,
}

fn bucket_of(path: &Path) -> Result<Bucket, Failed> {
    // Component-wise, not string `contains`: path separators differ across
    // platforms and Windows canonical paths carry a `\\?\` prefix.
    for comp in path.components() {
        match comp.as_os_str().to_str() {
            Some("accept") => return Ok(Bucket::Accept),
            Some("reject-unsound") => return Ok(Bucket::RejectUnsound),
            Some("reject-conservative") => return Ok(Bucket::RejectConservative),
            _ => {}
        }
    }
    Err(format!("case is not in a known bucket: {}", path.display()).into())
}

/// One `--~` expectation line.
#[derive(Debug, PartialEq, Eq)]
struct Expectation {
    kind: String,
    code: Option<String>,
    text: String,
}

fn parse_expectations(src: &str) -> Vec<Expectation> {
    let mut out = Vec::new();
    for line in src.lines() {
        let Some(rest) = line.trim_start().strip_prefix("--~") else {
            continue;
        };
        let rest = rest.trim();
        // ERROR[R0041]: message   |   NOTE: message
        let (head, text) = match rest.split_once(':') {
            Some((h, t)) => (h.trim(), t.trim().to_string()),
            None => (rest, String::new()),
        };
        let (kind, code) = match head.split_once('[') {
            Some((k, c)) => (
                k.trim().to_string(),
                Some(c.trim_end_matches(']').trim().to_string()),
            ),
            None => (head.to_string(), None),
        };
        out.push(Expectation { kind, code, text });
    }
    out
}

/// Every case's verdict rests on `parse_expectations`, so it is a trial of
/// its own: a slip in it would change what the cases assert without turning
/// any of them red.
fn expectation_lines_parse() -> Result<(), Failed> {
    let src = "--!rime\n\
               local x = 1\n\
               \t--~ ERROR[R0041]: cannot write: frozen\n\
               --~ NOTE: frozen at line 3\n\
               --~ OPTION[R0312]: on\n\
               local y = 2 --~ WARNING[R0013]: not at the start of its line\n";
    let line = |kind: &str, code: Option<&str>, text: &str| Expectation {
        kind: kind.to_string(),
        code: code.map(str::to_string),
        text: text.to_string(),
    };
    let want = vec![
        line("ERROR", Some("R0041"), "cannot write: frozen"),
        line("NOTE", None, "frozen at line 3"),
        line("OPTION", Some("R0312"), "on"),
    ];
    let got = parse_expectations(src);
    if got != want {
        return Err(format!("expected {want:#?}\ngot {got:#?}").into());
    }
    Ok(())
}

/// The rule settings a case asks for, in place of a rime.toml:
///
/// ```lua
/// --~ OPTION[R0312]: on
/// --~ OPTION[R0312]: allow = Signal.new, *:IsServer
/// --~ OPTION[R0312]: allow-builtin = luau
/// --~ OPTION[R0312]: deny = Random.new
/// --~ OPTION[R0312]: allow-loops = true
/// --~ OPTION[R0312]: scripts = *.server.luau
/// --~ OPTION[R0312]: exclude = *.spec.luau
/// ```
///
/// Any R0312 line turns the rule on (it is off by default); `on` alone runs
/// it on its defaults. A list is comma-separated; `allow-builtin = none` is
/// the empty one. Path patterns can only be file names: a case has no
/// rime.toml to be below.
fn rule_options(expectations: &[Expectation]) -> Result<rime_check::RuleOptions, Failed> {
    let mut out = rime_check::RuleOptions::default();
    for exp in expectations.iter().filter(|e| e.kind == "OPTION") {
        if exp.code.as_deref() != Some("R0312") {
            return Err(format!("no rule options are known for {:?}", exp.code).into());
        }
        let options = out.module_scope.get_or_insert_with(Default::default);
        if exp.text == "on" {
            continue;
        }
        let (key, value) = exp
            .text
            .split_once('=')
            .ok_or_else(|| format!("OPTION must read `key = value` or `on`: {}", exp.text))?;
        let list = || value.split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        match key.trim() {
            "allow" => options.allow = list().collect(),
            "deny" => options.deny = list().collect(),
            "scripts" => options.scripts = list().collect(),
            "exclude" => options.exclude = list().collect(),
            "allow-builtin" => {
                use rime_check::rules::module_scope::Builtin;
                options.allow_builtin = list()
                    .filter(|name| name != "none")
                    .map(|name| Builtin::from_name(&name).ok_or(format!("unknown built-in list `{name}`")))
                    .collect::<Result<_, _>>()?;
            }
            "allow-loops" => {
                options.allow_loops = value
                    .trim()
                    .parse()
                    .map_err(|_| format!("allow-loops must be true or false: {}", exp.text))?
            }
            other => return Err(format!("unknown R0312 option `{other}`").into()),
        }
    }
    Ok(out)
}

fn run_case(path: &Path) -> Result<(), Failed> {
    let bucket = bucket_of(path)?;
    let src = std::fs::read_to_string(path)?;

    if !src.starts_with("--!rime") {
        return Err("case must begin with the `--!rime` pragma".into());
    }

    let expectations = parse_expectations(&src);
    let options = rule_options(&expectations)?;
    let diags = rime_check::check_source_configured(
        &src,
        Some(path),
        rime_std::StdNames::bundled(),
        &options,
    );
    judge(bucket, &diags, &src, &expectations)?;

    // --- snapshot ---------------------------------------------------------
    let rendered = render_all(&diags, &src);
    let name = path.file_stem().unwrap().to_string_lossy().into_owned();
    insta::with_settings!({
        snapshot_path => path.parent().unwrap().join("snapshots"),
        prepend_module_to_snapshot => false,
        omit_expression => true,
    }, {
        insta::assert_snapshot!(name, rendered);
    });

    Ok(())
}

/// Bucket contract + inline expectations, shared by single-file and dir cases.
fn judge(
    bucket: Bucket,
    diags: &[rime_diag::Diagnostic],
    src: &str,
    expectations: &[Expectation],
) -> Result<(), Failed> {
    match bucket {
        Bucket::Accept => {
            if !diags.is_empty() {
                return Err(format!(
                    "expected accept/, got {} diagnostic(s):\n{}",
                    diags.len(),
                    render_all(diags, src)
                )
                .into());
            }
            if expectations.iter().any(|e| e.kind != "OPTION") {
                return Err("accept/ cases must not carry `--~` expectations".into());
            }
        }
        Bucket::RejectUnsound => {
            if diags.is_empty() {
                return Err("expected at least one diagnostic".into());
            }
            if diags.iter().all(|d| d.conservative) {
                return Err(
                    "all diagnostics were conservative — this case belongs in \
                     reject-conservative/, or the rule is mis-flagged"
                        .into(),
                );
            }
        }
        Bucket::RejectConservative => {
            if diags.is_empty() {
                return Err("expected at least one diagnostic".into());
            }
            if let Some(d) = diags.iter().find(|d| !d.conservative) {
                return Err(format!(
                    "diagnostic {} is not marked conservative, but this case is \
                     valid code we reject on purpose — either mark it \
                     conservative or move the case to reject-unsound/",
                    d.code
                )
                .into());
            }
        }
    }

    for exp in expectations {
        if exp.kind != "ERROR" && exp.kind != "WARNING" {
            continue; // NOTE/HELP are checked via the snapshot only
        }
        let matched = diags.iter().any(|d| {
            exp.code.as_deref().is_none_or(|c| d.code.to_string() == c)
                && d.message.contains(&exp.text)
        });
        if !matched {
            return Err(format!(
                "unmet expectation: {} {:?}: {}\ngot:\n{}",
                exp.kind,
                exp.code,
                exp.text,
                render_all(diags, src)
            )
            .into());
        }
    }
    Ok(())
}

fn render_all(diags: &[rime_diag::Diagnostic], src: &str) -> String {
    if diags.is_empty() {
        return "(no diagnostics)\n".to_string();
    }
    diags
        .iter()
        .map(|d| rime_diag::render::to_snapshot(d, src))
        .collect::<Vec<_>>()
        .join("\n")
}
