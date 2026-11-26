//! `rime` CLI. `rime check <paths>` runs the checks over files or
//! directories and renders diagnostics to the terminal, with per-rule
//! severity from rime.toml (see config.rs).
//!
//! `rime explain R0101` comes later (plan §1).

mod config;
mod discover;
mod json;
mod lsp;
mod notes;
mod template;
mod workspace;

use clap::{Parser, Subcommand};

// The per-file passes allocate heavily from every thread at once; the system
// allocator serializes them on one heap lock (parse + HIR drop were a large
// share of a run on a large codebase). mimalloc has per-thread heaps.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum OutputFormat {
    /// Human-readable terminal output (default)
    Text,
    /// Machine-readable JSON (schema v1) on stdout
    Json,
}

#[derive(Parser)]
#[command(name = "rime", version, about = "strict-dialect static checker for Luau")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check .luau/.lua files and print diagnostics
    Check {
        /// Files or directories to check
        paths: Vec<PathBuf>,
        /// Print machine-stable snapshot output instead of colour
        #[arg(long)]
        plain: bool,
        /// Path to rime.toml (default: nearest one walking up from cwd)
        #[arg(long)]
        config: Option<PathBuf>,
        /// The Rojo project file to read for cross-file checks (default:
        /// `project` in rime.toml, else the nearest folder at or above the
        /// first path that holds a `*.project.json`, where
        /// `default.project.json` wins)
        #[arg(long, value_name = "FILE")]
        project: Option<PathBuf>,
        /// Print a wall-clock breakdown (phases, rules, slowest files) to
        /// stderr after the summary line
        #[arg(long)]
        timings: bool,
        /// Output format. `json` prints one schema-v1 document to stdout;
        /// the summary line stays on stderr. Exit codes are unchanged.
        #[arg(long, value_enum, default_value = "text", conflicts_with = "plain")]
        format: OutputFormat,
        /// Skip running rime-rules/*.luau custom project rules (rule files
        /// are still loaded so their names resolve in rime.toml)
        #[arg(long)]
        no_custom_rules: bool,
        /// Also check files git ignores. By default, inside a git work
        /// tree, whatever a `.gitignore`, `.git/info/exclude` or the
        /// global excludes file matches is skipped. A path named on the
        /// command line is always walked, and files under a Rojo mount
        /// are still read for cross-file checks, though not reported on
        #[arg(long)]
        no_gitignore: bool,
        /// Report only these rules (comma-separated names or R#### codes;
        /// repeatable), at their default severity — rime.toml `[rules]`
        /// levels are ignored for them, so an `off` rule can be inspected.
        /// Everything else counts as ignored, except parse errors (R0900),
        /// which always show: an unparsed file is unchecked for every rule.
        /// `ignore`, `[requires]`, `[std]`, and inline allows still apply.
        #[arg(long, value_name = "RULES")]
        only: Vec<String>,
    },
    /// Run as a Language Server over stdio (used by the editor extension)
    Lsp,
    /// Print a rime.toml that names every setting with its current default
    /// (examples/rime.toml is this output, checked in)
    ConfigTemplate {
        /// Write the template to this path instead of stdout
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Developer tools (hidden)
    #[command(hide = true)]
    Dev {
        #[command(subcommand)]
        cmd: DevCommand,
    },
}

#[derive(Subcommand)]
enum DevCommand {
    /// Drive a Session through random edits over a whole project and compare
    /// to a fresh check after each one.
    SessionDiff {
        dir: PathBuf,
        #[arg(long, default_value_t = 50)]
        edits: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
}

/// `rime check` exit status when Rime itself failed: an R0901 was reported
/// (a crash contained to one file or rule — the rest of the run finished
/// and printed) or the run died outside any such boundary. It wins over
/// 1: the findings of a run with a hole in it are not a verdict. 101 is
/// what an uncaught Rust panic already exits with, and what rustc uses for
/// an internal compiler error, so "Rime crashed" is one code whether or
/// not the crash was contained. 0 = clean, 1 = error-severity findings
/// (also a bad rime.toml or `--only`), 2 = bad command line (clap).
const EXIT_INTERNAL_ERROR: u8 = 101;

/// `rime check` exit status when the command could not be carried out as
/// written: a path to check that does not exist. The same 2 clap exits
/// with for a malformed command line, and for the same reason — nothing
/// was checked, so neither 0 ("clean") nor 1 ("findings") is true.
const EXIT_USAGE: u8 = 2;

/// Stack size of every thread that parses or checks: the rayon workers, the
/// thread `check` itself runs on, and the language server's worker.
///
/// The parser (full_moon) and the passes over its tree are recursive, and
/// the frames are large. Measured on Windows with full_moon 2.2, one level
/// of nested closure cost about 50 KB of stack in a release build and
/// about 430 KB in a debug build: on the platform defaults (2 MiB for a
/// spawned thread, 1 MiB for the Windows main thread) a release build died
/// at 39 nested closures or 68 nested `if`s, and a debug build died on
/// ordinary source files (five or six levels of nesting). full_moon 3.0
/// boxes its AST nodes and roughly halves that in release and thirds it in
/// debug, which moves the cliff and does not remove it. A stack overflow
/// is an abort: nothing can catch it, the whole run is lost and the
/// message names no file. So the only defence is room.
///
/// This is address space, not memory: the OS commits a stack page when it
/// is first touched, so the unused part of each stack costs nothing.
///
/// With full_moon 3.0, 256 MiB holds 8,000 nested closures in a release
/// build (10,000 overflows) — Luau's own parser refuses anything nested
/// deeper than 1000 — and 1,700 in a debug build (2,200 overflows). With
/// 2.2 those were 4,800 / 5,200 and 570 / 620. tests/deep_nesting.rs pins
/// a floor under both.
const WORK_STACK_BYTES: usize = 256 * 1024 * 1024;

/// `WORK_STACK_BYTES`, or `RUST_MIN_STACK` when that asks for more — the
/// standard knob keeps working as an escape hatch for a file that needs it.
fn work_stack_bytes() -> usize {
    std::env::var("RUST_MIN_STACK")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .map_or(WORK_STACK_BYTES, |asked| asked.max(WORK_STACK_BYTES))
}

/// Give the global rayon pool `work_stack_bytes()` stacks. Must run before
/// anything touches rayon: the pool is built once, on first use.
fn init_worker_pool() {
    // An error means the OS refused the threads (or a pool already exists,
    // which cannot happen this early). Either way rayon then builds its
    // default pool on first use: smaller stacks, but a run.
    let _ = rayon::ThreadPoolBuilder::new().stack_size(work_stack_bytes()).build_global();
}

/// Run `f` on a thread with a `work_stack_bytes()` stack and return its
/// result. The main thread's own stack size is fixed at link time (1 MiB
/// on Windows), and the serial parts of a run happen on the thread that
/// started it. If the OS refuses the thread, `f` runs right here instead.
fn on_work_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    let job = std::sync::Mutex::new(Some(f));
    let take = || job.lock().unwrap_or_else(|e| e.into_inner()).take();
    let spawned = std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("rime-main".into())
            .stack_size(work_stack_bytes())
            .spawn_scoped(scope, || take().map(|f| f()));
        match worker {
            Ok(worker) => match worker.join() {
                Ok(out) => out,
                Err(payload) => std::panic::resume_unwind(payload),
            },
            Err(_) => None,
        }
    });
    match spawned {
        Some(out) => out,
        None => take().expect("the job is only taken by whichever thread runs it")(),
    }
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { paths, plain, config, project, timings, format, no_custom_rules, no_gitignore, only } => {
            init_worker_pool();
            on_work_stack(|| {
                // Last line of defence: per-file crashes are contained inside
                // the run (rime_check::guard); this is for everything else —
                // loading the workspace, the resolver, the summary itself.
                let run = rime_check::guard::catch(|| {
                    check(paths, plain, config, project, timings, format, no_custom_rules, no_gitignore, only)
                });
                run.unwrap_or_else(|crash| {
                    eprint_internal_error(&crash.project_diagnostic(
                        "running the check",
                        "the run did not finish: nothing it printed is a complete result",
                    ));
                    std::process::ExitCode::from(EXIT_INTERNAL_ERROR)
                })
            })
        }
        Command::Lsp => {
            init_worker_pool();
            lsp::run()
        }
        Command::ConfigTemplate { out } => config_template(out),
        Command::Dev { cmd } => match cmd {
            DevCommand::SessionDiff { dir, edits, seed } => {
                init_worker_pool();
                on_work_stack(|| session_diff(dir, edits, seed))
            }
        },
    }
}

/// `rime config-template [--out PATH]`: the generated rime.toml (see
/// `template.rs`) to stdout, or written to `PATH` with nothing on stdout.
fn config_template(out: Option<PathBuf>) -> std::process::ExitCode {
    let text = template::render();
    match out {
        None => {
            print!("{text}");
            std::process::ExitCode::SUCCESS
        }
        Some(path) => match std::fs::write(&path, text) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("rime: could not write {}: {e}", path.display());
                std::process::ExitCode::FAILURE
            }
        },
    }
}

/// Drive a warm `Session` through `edits` random mutations over the real
/// project at `dir`, comparing its reports against a fresh `Session::new`
/// after each one. The corpus differential proves this on small fixtures;
/// this proves it at scale on a real repo. A
/// fresh `RojoResolver` per comparison is deliberate — its caches memoize
/// filesystem state that must not leak between runs.
fn session_diff(dir: PathBuf, edits: usize, seed: u64) -> std::process::ExitCode {
    let root = dir.canonicalize().unwrap_or(dir);
    let ws = workspace::Workspace::load(std::slice::from_ref(&root), None, None, &root, true, None).expect("load");
    let project_file = ws.project.file().map(Path::to_path_buf);
    let originals = ws.files.clone();
    if originals.is_empty() {
        eprintln!("rime: no .lua/.luau files under {}", root.display());
        return std::process::ExitCode::SUCCESS;
    }
    let paths_only: Vec<PathBuf> = originals.iter().map(|(p, _)| p.clone()).collect();
    let make_resolver = |_root: &Path, config: &config::Config| -> Option<Box<dyn rime_check::project::Resolver>> {
        let project = rime_roblox::RojoProject::load_file(project_file.as_deref()?).ok()?;
        // Seed the mount listing from the walk already done above — the
        // same `seed_walk` call `Workspace::load` makes for the warm
        // session's own resolver. Without it, this fresh RojoProject's
        // per-mount listing is a lazy `OnceLock`: on a whole project,
        // `recompute_interfaces`'s parallel pass has many linker files
        // racing to trigger it at once, the rest block on the lock while
        // the winner's own recursive walk starves for rayon workers to
        // steal onto (every other worker is parked on the same lock, not
        // stealing) — minutes, not milliseconds, on a large project
        // (found running this at scale on a large codebase).
        project.seed_walk(&[root.to_path_buf()], &paths_only);
        Some(Box::new(rime_roblox::RojoResolver::new(project).with_trusted(config.trusted_requires())))
    };
    let (mut session, config, _) = ws.session(None);
    let mut files = originals.clone();
    // xorshift — deterministic, no dep.
    let mut state = seed.max(1);
    let mut rng = move || { state ^= state << 13; state ^= state >> 7; state ^= state << 17; state };
    let mut failures = 0;
    let mut retargets = 0usize;
    for i in 0..edits {
        let idx = (rng() as usize) % files.len();
        let (path, original) = files[idx].clone();
        let (kind, src) = match rng() % 5 {
            0 => ("append", format!("{original}\nlocal __rime_probe = 1\n")),
            1 => ("prepend", format!("-- probe\n{original}")),
            2 => ("parse-error", format!("{original}\nlocal = \n")),
            // Move the require GRAPH, not just a file's text: the first
            // require goes to a module that does not exist. The `restore`
            // arm below is what puts it back.
            3 => match original.find("require(") {
                Some(at) => {
                    let mut s = original.clone();
                    s.replace_range(at..at + "require(".len(), "require(script.Parent.__rime_missing_");
                    retargets += 1;
                    ("retarget", s)
                }
                None => ("retarget(no-op)", original.clone()),
            },
            _ => (
                "restore",
                originals.iter().find(|(p, _)| *p == path).map(|(_, s)| s.clone()).unwrap_or(original),
            ),
        };
        files[idx].1 = src.clone();
        let t = std::time::Instant::now();
        let changed = session.update(&path, src);
        let took = t.elapsed();
        let fresh = rime_check::session::Session::with_options(files.clone(), make_resolver(&root, &config), config.build_std_names().unwrap(), config.rule_options(), None).reports();
        let mine = session.reports();
        let ok = fresh.len() == mine.len()
            && fresh.iter().zip(&mine).all(|(a, b)| a.path == b.path && a.diagnostics == b.diagnostics && a.suppressed == b.suppressed);
        eprintln!("edit {i:>3} {:<60} {kind:<15} {:>7.1?}  affected {:>5}  changed {:>4}  {}",
            path.file_name().unwrap().to_string_lossy(), took, session.last_affected().len(), changed.len(),
            if ok { "ok" } else { "MISMATCH" });
        if !ok {
            failures += 1;
            // Undiagnosable otherwise: find and dump the first file where
            // the two report lists disagree.
            let n = fresh.len().max(mine.len());
            let mut shown = false;
            for j in 0..n {
                let a = fresh.get(j);
                let b = mine.get(j);
                let differs = match (a, b) {
                    (Some(a), Some(b)) => {
                        a.path != b.path || a.diagnostics != b.diagnostics || a.suppressed != b.suppressed
                    }
                    _ => true,
                };
                if !differs {
                    continue;
                }
                eprintln!(
                    "  first differing file: fresh={:?} session={:?}",
                    a.map(|r| r.path.display().to_string()),
                    b.map(|r| r.path.display().to_string())
                );
                if let Some(a) = a {
                    eprintln!("  fresh diagnostics ({}):", a.diagnostics.len());
                    for d in &a.diagnostics {
                        eprintln!("    ({}, {:?}, {:?})", d.code, d.primary_span(), d.message);
                    }
                }
                if let Some(b) = b {
                    eprintln!("  session diagnostics ({}):", b.diagnostics.len());
                    for d in &b.diagnostics {
                        eprintln!("    ({}, {:?}, {:?})", d.code, d.primary_span(), d.message);
                    }
                }
                shown = true;
                break;
            }
            if !shown {
                eprintln!("  (no differing report found — check suppressed counts / ordering)");
            }
        }
    }
    eprintln!("{edits} edits, {retargets} of them require-retargets, {failures} mismatches");
    if failures == 0 { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

/// An R0901 that belongs to no file, on stderr in the plain diagnostic
/// shape. Stderr because stdout may be a JSON document, and a file-less
/// entry has no place in its `diagnostics`.
fn eprint_internal_error(d: &rime_diag::Diagnostic) {
    eprint!("rime: {}", rime_diag::render::to_snapshot(d, ""));
}

/// One diagnostic, ready to print or to collect.
enum Rendered {
    Json(json::Diagnostic),
    Text(String),
}

/// `show` writes the path of another file the way `path` is written.
fn render(
    format: OutputFormat,
    plain: bool,
    path: &str,
    d: &rime_diag::Diagnostic,
    src: &str,
    show: &dyn Fn(&Path) -> String,
) -> Rendered {
    match format {
        OutputFormat::Json => Rendered::Json(json::Diagnostic::from_diag(path, d, src, show)),
        OutputFormat::Text if plain => {
            Rendered::Text(format!("{}: {}", path, rime_diag::render::to_snapshot(d, src)))
        }
        // ariadne hangs the path and the notes off a label; given none it
        // prints the headline alone. A diagnostic about a whole file (an
        // R0901) is all notes, so it takes the plain form here too.
        OutputFormat::Text if d.labels.is_empty() => {
            Rendered::Text(format!("{}: {}\n", path, rime_diag::render::to_snapshot(d, src)))
        }
        OutputFormat::Text => {
            Rendered::Text(format!("{}\n", rime_diag::render::to_terminal(d, path, src)))
        }
    }
}

fn check(
    paths: Vec<PathBuf>,
    plain: bool,
    config_path: Option<PathBuf>,
    project_file: Option<PathBuf>,
    want_timings: bool,
    format: OutputFormat,
    no_custom_rules: bool,
    no_gitignore: bool,
    only: Vec<String>,
) -> std::process::ExitCode {
    use rime_check::guard;
    guard::fault("run", Path::new(""));
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // A path that is not there is a mistake in the command, not an empty
    // result: walked as "nothing", it would print "checked 0 files" and
    // pass. Every bad path is named, and nothing is checked.
    let roots = if paths.is_empty() { vec![PathBuf::from(".")] } else { paths };
    let unusable: Vec<(&PathBuf, std::io::Error)> =
        roots.iter().filter_map(|p| std::fs::metadata(p).err().map(|e| (p, e))).collect();
    if !unusable.is_empty() {
        for (path, error) in &unusable {
            eprintln!("rime: cannot check `{}`: {error}", path.display());
        }
        return std::process::ExitCode::from(EXIT_USAGE);
    }
    let mut timings = want_timings.then(rime_check::timings::Timings::default);
    let started = std::time::Instant::now();

    // Custom rules register their names before rime.toml is parsed, so
    // `[rules] my-rule = "off"` resolves. The project root for
    // rules is rime.toml's directory; no rime.toml, no custom rules.
    let rules_root: Option<PathBuf> = config::Config::discover_path(config_path.as_deref(), &cwd)
        .and_then(|p| p.parent().map(Path::to_path_buf))
        // `--config rime.toml` has parent "" — resolve relative parents against cwd.
        .map(|d| if d.as_os_str().is_empty() { cwd.clone() } else if d.is_absolute() { d } else { cwd.join(d) })
        .map(|d| d.canonicalize().unwrap_or(d));
    let mut rule_file_diags: Vec<rime_custom::RuleDiag> = Vec::new();
    let mut engine: Option<rime_custom::Engine> = None;
    let mut rule_phase_names: Vec<&'static str> = Vec::new();
    if let Some(root) = &rules_root {
        let load_start = std::time::Instant::now();
        // Always load: rule NAMES must be registered before rime.toml is
        // parsed or its `[rules]` keys cannot resolve. `--no-custom-rules`
        // skips running them, not knowing them.
        let report = rime_custom::Engine::load(root);
        if !no_custom_rules {
            rule_file_diags.extend(report.diagnostics);
            if let Some(e) = report.engine {
                rule_phase_names = e
                    .rule_ids()
                    .iter()
                    .map(|id| &*Box::leak(format!("custom: {id}").into_boxed_str()))
                    .collect();
                engine = Some(e);
            }
        }
        if let Some(t) = timings.as_mut() {
            t.record("load custom rules", load_start.elapsed());
        }
    }

    let mut ws = match workspace::Workspace::load(
        &roots,
        config_path.as_deref(),
        project_file.as_deref(),
        &cwd,
        !no_gitignore,
        timings.as_mut(),
    ) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("rime: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // A custom rule sees each file's path, and matches its `files` globs,
    // relative to one directory for the whole run: `[custom-rules] root` when
    // rime.toml sets it, else rime.toml's own directory. A file outside it is
    // not checked by custom rules, and the run says how many were left out.
    let rule_paths_base: Option<PathBuf> = rules_root
        .as_ref()
        .map(|root| ws.config.custom_rules_root().map_or_else(|| root.clone(), Path::to_path_buf));
    let mut outside_rule_paths_base = 0usize;
    // A path that exists but holds nothing to check is not an error (an
    // empty directory is a legitimate thing to point at, and Selene and
    // StyLua both exit 0 on one), but it is said out loud: "0 files" in
    // the summary line is easy to read past.
    for root in &ws.empty_roots {
        let path = root.path.display();
        if !root.path.is_dir() {
            eprintln!("rime: warning: `{path}` is not a .lua or .luau file, so it was not checked");
        } else if root.git_ignored > 0 {
            eprintln!(
                "rime: warning: no .lua or .luau files under `{path}` that git does not ignore \
                 (--no-gitignore checks ignored files too)"
            );
        } else {
            eprintln!("rime: warning: no .lua or .luau files under `{path}`");
        }
    }
    let warned_empty = !ws.empty_roots.is_empty();
    // `--only` resolves names the same way rime.toml keys do; custom rule
    // names are registered above, so they work here too.
    for spec in &only {
        match config::parse_only(spec) {
            Ok(codes) => ws.config.restrict_to(codes),
            Err(e) => {
                eprintln!("rime: --only: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }

    // After `--only`: it decides whether the rule the settings belong to
    // runs at all.
    for warning in ws.config.rule_option_warnings() {
        eprintln!("rime: warning: {warning}");
    }

    // After `--only` too: a file outside the rule paths' directory only
    // counts as missed when some custom rule could have reported on it.
    let any_custom_rule_reports =
        engine.as_ref().is_some_and(|e| e.rule_codes().into_iter().any(|code| ws.config.reports(code)));

    // Paths render relative to the invocation directory (clickable in
    // editors, matches clippy/tsc); files outside it fall back to absolute,
    // minus the Windows `\\?\` verbatim prefix either way.
    let canon_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
    let display_path = move |path: &Path| -> String { workspace::show_path(path, &canon_cwd) };

    // Which project file the run reads, or that it reads none: with none
    // there are no cross-file checks, and a run that quietly did less
    // looks the same as a clean one. Said before any finding.
    let project_line = ws.project.line(&display_path);
    eprintln!("rime: {project_line}");
    let cross_file_off = ws.project.file().is_none();
    let mut require_notes = notes::RequireNotes::default();

    let mut checked = 0usize;
    let mut errors = 0usize;
    let mut warnings = 0usize;
    let mut ignored = 0usize;
    let mut ignored_file_diags = 0usize;
    // Diagnostics dropped by inline `-- rime: allow(...)` — folded into the
    // ignored count. Separate variable: `emit` holds the borrow on `ignored`.
    let mut allowed = 0usize;
    let mut failing = false;
    // R0901s reported, per file or for the project. Any at all changes the
    // exit status (`EXIT_INTERNAL_ERROR`).
    let mut internal_errors = 0usize;
    // The custom-rule pass crashed (a panic in the bridge, not a rule that
    // threw — that is R0025). The VM's state is unknown after that, so the
    // pass is off for the rest of the run.
    let mut custom_rules_crashed = false;
    let mut json_diags: Vec<json::Diagnostic> = Vec::new();
    // Accumulated locally: `timings` must stay free for the ignore-filter
    // phase and `drop_parallel` below while this closure is alive.
    let mut render_time = std::time::Duration::ZERO;
    // Project mode: a Rojo project was found and the session got a
    // resolver. Reports then cover the whole project (interfaces need it),
    // so they get filtered back down to the requested files below. In
    // no-project mode the walk IS the request — `Workspace::load` already
    // dropped ignored files before reading, so every session report is one
    // to emit.
    let project_mode = ws.resolver.is_some();
    let (session, config, requested) = ws.session(timings.as_mut());
    let mut emit = |path: &Path, src: &str, diags: Vec<rime_diag::Diagnostic>| {
        let render_start = std::time::Instant::now();
        checked += 1;
        let (diags, dropped) = config.apply(diags);
        ignored += dropped;
        for d in &diags {
            match d.severity {
                rime_diag::Severity::Error => errors += 1,
                rime_diag::Severity::Warning => warnings += 1,
            }
        }
        failing |= config::has_errors(&diags);
        internal_errors += diags.iter().filter(|d| d.is_internal_error()).count();
        let path_str = display_path(path);
        let mut put = |rendered: Rendered| match rendered {
            Rendered::Json(d) => json_diags.push(d),
            Rendered::Text(text) => print!("{text}"),
        };
        // Rendering slices the source by span; a span the renderer cannot
        // take must not cost the finding, or every file after it.
        let mut render_crashes: Vec<rime_diag::Diagnostic> = Vec::new();
        for d in &diags {
            let rendered = guard::catch(|| {
                guard::fault("render", path);
                guard::phase("render", || render(format, plain, &path_str, d, src, &display_path))
            });
            match rendered {
                Ok(rendered) => put(rendered),
                Err(crash) => {
                    // Still report the finding — without the spans that
                    // broke the renderer, in the form that needs no source.
                    let bare = rime_diag::Diagnostic {
                        labels: Vec::new(),
                        suggestions: Vec::new(),
                        ..d.clone()
                    };
                    put(match format {
                        OutputFormat::Json => render(format, plain, &path_str, &bare, src, &display_path),
                        OutputFormat::Text => render(format, true, &path_str, &bare, src, &display_path),
                    });
                    render_crashes.push(crash.diagnostic(
                        "rendering a diagnostic for",
                        &format!(
                            "the {} report for this file is shown without its labels and source excerpt",
                            d.code
                        ),
                    ));
                }
            }
        }
        for d in &render_crashes {
            errors += 1;
            internal_errors += 1;
            // No labels on an R0901, so nothing here touches the source.
            put(match format {
                OutputFormat::Json => render(format, plain, &path_str, d, "", &display_path),
                OutputFormat::Text => render(format, true, &path_str, d, "", &display_path),
            });
        }
        render_time += render_start.elapsed();
    };

    let reports = session.reports();
    let ignore_start = std::time::Instant::now();
    let ignored_flags: Vec<bool> = reports.iter().map(|r| config.ignored(&r.path)).collect();
    if let Some(t) = timings.as_mut() {
        t.record("ignore filter", ignore_start.elapsed());
    }
    for (report, is_ignored) in reports.into_iter().zip(ignored_flags) {
        let unrequested = project_mode && !requested.contains(&report.path);
        if unrequested || is_ignored {
            // A file the run was not asked about, or one `ignore` covers:
            // its findings stay out. A crash on it does not — the file
            // still fed the interfaces and whole-project rules that the
            // reported files were checked against.
            let (internal, findings): (Vec<_>, Vec<_>) =
                report.diagnostics.into_iter().partition(|d| d.is_internal_error());
            if !unrequested {
                // The file was checked and skipped by `ignore` — its
                // kept-severity diagnostics count as ignored. Separate
                // counter: `emit` holds the borrow on `ignored`.
                let (kept, _) = config.apply(findings);
                ignored_file_diags += kept.len();
            }
            if !internal.is_empty() {
                emit(&report.path, session.source(&report.path).unwrap_or(""), internal);
            }
            continue;
        }
        allowed += report.suppressed;
        let src = session.source(&report.path).unwrap_or("");
        let file_notes = session.require_notes(&report.path);
        if !file_notes.is_empty() {
            require_notes.add_file(&display_path(&report.path), src, file_notes);
        }
        let mut diags = report.diagnostics;
        if let (Some(engine), Some(base), false) = (engine.as_mut(), rule_paths_base.as_ref(), custom_rules_crashed) {
            let rel = report.path.strip_prefix(base).ok();
            if rel.is_none() && any_custom_rule_reports {
                outside_rule_paths_base += 1;
            }
            if let (Some(module), Some(rel)) = (session.module(&report.path), rel) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                let ran = guard::catch(|| {
                    guard::fault("custom", &report.path);
                    guard::phase("custom rules", || engine.check_file(&rel, module, src))
                });
                match ran {
                    Ok(out) => {
                        let applied = rime_diag::allow::apply(src, out.diagnostics);
                        allowed += applied.suppressed;
                        diags.extend(applied.kept);
                        rule_file_diags.extend(out.rule_errors);
                        if let Some(t) = timings.as_mut() {
                            for (i, dur) in out.timings {
                                t.record(rule_phase_names[i], dur);
                            }
                        }
                    }
                    Err(crash) => {
                        custom_rules_crashed = true;
                        diags.push(crash.diagnostic(
                            "running the project's custom rules on",
                            "custom rules (rime-rules/) did not run on this file or on any \
                             file after it in this run",
                        ));
                    }
                }
            }
        }
        emit(&report.path, src, rime_diag::finalize(diags));
    }

    // Crashes in whole-project work have no file to be listed under; they
    // go to stderr. Counted apart from `internal_errors`, which `emit`
    // holds the borrow on.
    let mut project_internal_errors = 0usize;
    for d in session.project_errors() {
        project_internal_errors += 1;
        eprint_internal_error(d);
    }

    // Files custom rules could not see: said once, so a rule that matches
    // nothing because of where rime.toml lives is not a silent pass.
    if let (Some(base), true) = (rule_paths_base.as_ref(), outside_rule_paths_base > 0) {
        let files = if outside_rule_paths_base == 1 { "file" } else { "files" };
        // The directory the run started in shows as an empty path.
        let shown = display_path(base);
        let shown = if shown.is_empty() { ".".to_string() } else { shown };
        eprintln!(
            "rime: note: custom rules did not run on {outside_rule_paths_base} {files} outside `{shown}`, the \
             directory their paths are relative to. Set `[custom-rules] root` in rime.toml to a directory above them."
        );
    }

    // Throwing rules report once per (rule, error line) for the whole run,
    // not once per file — the engine holds them until now.
    if let Some(e) = engine.as_mut() {
        match guard::catch(|| guard::phase("custom rules", || e.finish())) {
            Ok(diags) => rule_file_diags.extend(diags),
            Err(crash) => {
                project_internal_errors += 1;
                eprint_internal_error(&crash.project_diagnostic(
                    "collecting the custom rules' own errors",
                    "a rime-rules/ file that threw may be missing its custom-rule-error (R0025)",
                ));
            }
        }
    }

    // Rule-file problems (load failures, throwing checks) render like any
    // file's diagnostics, anchored on the rule file.
    let mut by_file: std::collections::BTreeMap<PathBuf, (String, Vec<rime_diag::Diagnostic>)> =
        std::collections::BTreeMap::new();
    for rd in rule_file_diags {
        let entry = by_file.entry(rd.path).or_insert_with(|| (rd.src, Vec::new()));
        entry.1.push(rd.diagnostic);
    }
    for (path, (src, diags)) in by_file {
        emit(&path, &src, rime_diag::finalize(diags));
    }
    session.drop_parallel(timings.as_mut());

    // Requires the run did not follow: one sentence per kind, not one
    // report per call (`notes.rs`). They change neither counts nor the
    // exit status: nothing here is a defect in the code.
    let note_lines = require_notes.lines();
    for line in &note_lines {
        eprintln!("rime: note: {line}");
    }

    let total = started.elapsed();
    let secs = total.as_secs_f64();
    let ignored = ignored + ignored_file_diags + allowed;
    let internal_errors = internal_errors + project_internal_errors;
    eprintln!(
        "Rime checked {checked} files, produced {errors} errors, {warnings} warnings, \
         {ignored} ignored in {secs:.1}s"
    );
    if checked == 0 && !warned_empty {
        eprintln!(
            "rime: warning: nothing was checked: every source file under the given paths is \
             excluded by `ignore` in rime.toml, or could not be read"
        );
    }
    if internal_errors > 0 {
        eprintln!(
            "rime: {internal_errors} internal error{} (R0901) — Rime crashed on part of this run, \
             so its results are incomplete; exiting with status {EXIT_INTERNAL_ERROR}",
            if internal_errors == 1 { "" } else { "s" }
        );
    }

    if format == OutputFormat::Json {
        let out = json::Output {
            version: 1,
            diagnostics: json_diags,
            summary: json::Summary { checked, errors, warnings, ignored, internal_errors },
            notes: cross_file_off.then_some(project_line).into_iter().chain(note_lines).collect(),
        };
        println!("{}", serde_json::to_string_pretty(&out).expect("serialize"));
    }

    if let Some(t) = timings.as_mut() {
        t.record("config apply + render", render_time);
        // Wall phases sum to the run; CPU phases (per-file buckets summed
        // across worker threads) show relative rule cost — see timings.rs.
        let (mut wall, mut cpu): (Vec<_>, Vec<_>) = t.phases().iter().partition(|p| !p.cpu);
        wall.sort_by(|a, b| b.total.cmp(&a.total));
        cpu.sort_by(|a, b| b.total.cmp(&a.total));
        let accounted: std::time::Duration = wall.iter().map(|p| p.total).sum();
        eprintln!("\ntiming breakdown (total {secs:.2}s, biggest first):");
        for p in &wall {
            let pct = p.total.as_secs_f64() / total.as_secs_f64() * 100.0;
            eprintln!("  {:<22} {:>8.3}s  {:>5.1}%", p.name, p.total.as_secs_f64(), pct);
        }
        let rest = total.saturating_sub(accounted).as_secs_f64();
        eprintln!("  {:<22} {:>8.3}s  {:>5.1}%", "(unaccounted)", rest, rest / secs * 100.0);
        if !cpu.is_empty() {
            let cpu_total: std::time::Duration = cpu.iter().map(|p| p.total).sum();
            let cpu_secs = cpu_total.as_secs_f64();
            eprintln!(
                "per-file work (CPU time summed over {} threads, {cpu_secs:.2}s):",
                rayon::current_num_threads()
            );
            for p in &cpu {
                let pct = p.total.as_secs_f64() / cpu_secs * 100.0;
                eprintln!("  {:<22} {:>8.3}s  {:>5.1}%", p.name, p.total.as_secs_f64(), pct);
            }
        }
        let slowest = t.slowest_files(5);
        if !slowest.is_empty() {
            eprintln!("slowest files (parse + check):");
            for (path, dur) in slowest {
                eprintln!("  {:>8.3}s  {}", dur.as_secs_f64(), display_path(path));
            }
        }
    }
    if internal_errors > 0 {
        std::process::ExitCode::from(EXIT_INTERNAL_ERROR)
    } else if failing {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
