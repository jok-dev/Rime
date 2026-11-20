//! Warm project state — the one pipeline behind `rime check` and `rime lsp`.
//!
//! `Session::new` is exactly the old `check_project_timed`: parse everything,
//! extract interfaces (linker chains chased), run the per-file pass with
//! `check_file`, run R0309 over the defining modules, finalize. What it adds
//! is that every intermediate stays resident per file, so `update`
//! can redo a slice of the work and report which files' diagnostics moved.

use crate::project::{
    linker_target, local_env, require_bindings, FileReport, RequireNote, Resolution, Resolver,
};
use crate::rules::calls::{self, ModuleInterface};
use crate::rules::cycles::{self, LoadEdge};
use crate::guard;
use crate::rules::exports::{self, Usage};
use crate::timings::{time_opt, Timings};
use rayon::prelude::*;
use rime_diag::{Diagnostic, Span};
use rime_hir as hir;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Everything held per file. `Default` = "known path, nothing computed".
#[derive(Default)]
pub(crate) struct FileState {
    pub src: String,
    /// `None` = parse failure; `pass2` then holds the R0900 fallback report.
    pub module: Option<hir::Module>,
    /// A linker's interface is its terminal's (old `interface_of`).
    pub interface: ModuleInterface,
    /// Files this linker chains through, in order, terminal last. Empty for
    /// a non-linker. Depth-capped at 9 entries like the old recursion.
    pub linker_chain: Vec<PathBuf>,
    /// (local name, raw require target, terminal after linker chase).
    pub deps: Vec<(String, PathBuf, PathBuf)>,
    pub degraded: bool,
    /// This file's R0309 usage contribution, keyed by terminal.
    pub usage: HashMap<PathBuf, Usage>,
    /// This file's member-name reads (degraded-mode global set).
    pub read_names: HashSet<String>,
    /// Where each name-shaped string literal in this file is written, as
    /// byte offsets (degraded-mode global set;
    /// `exports::collect_string_names`).
    pub string_names: HashMap<String, Vec<usize>>,
    /// Files the evaluator (eval.rs) read while resolving THIS file's
    /// `Dynamic` require sites — `Evaluator::take_deps()` at the end of
    /// `check_file`. `affected_for` reads them through `eval_consumers`.
    pub eval_deps: HashSet<PathBuf>,
    /// This file's resolved load-time requires — R0509's edges (pass 3
    /// walks the whole graph from these).
    pub load_edges: Vec<LoadEdge>,
    /// Requires in this file the run reports once, not per call: where
    /// each is, and why (see `RequireNote`). Never diagnostics.
    pub require_notes: Vec<(Span, RequireNote)>,
    /// R0901 when parsing this file crashed Rime. `module` is then `None`
    /// and `pass2` reports this instead of the R0900 fallback.
    pub parse_crash: Option<Diagnostic>,
    /// R0901 when extracting this file's interface crashed. `interface` is
    /// then the empty default, which consumers already treat as "nothing
    /// to check against".
    pub iface_crash: Option<Diagnostic>,
    /// The per-file pass crashed: `pass2` is the R0901 alone, and nothing
    /// this file would have contributed (deps, usage, read names, load
    /// edges) is known.
    pub crashed: bool,
    pub pass2: Vec<Diagnostic>,
    pub pass3: Vec<Diagnostic>,
    pub suppressed: usize,
    pub report: Vec<Diagnostic>,
}

/// One file's pass-2 result (the old per-worker `FileOut`).
pub(crate) struct FileOut {
    pub degraded: bool,
    /// See `FileState::crashed`; `failed` then holds the R0901.
    pub crashed: bool,
    /// `Some` = the parse-failure fallback report, already finalized.
    pub failed: Option<Vec<Diagnostic>>,
    pub diags: Vec<Diagnostic>,
    pub deps: Vec<(String, PathBuf, PathBuf)>,
    pub usage: HashMap<PathBuf, Usage>,
    pub read_names: HashSet<String>,
    pub string_names: HashMap<String, Vec<usize>>,
    /// See `FileState::eval_deps`.
    pub eval_deps: HashSet<PathBuf>,
    /// See `FileState::load_edges`.
    pub load_edges: Vec<LoadEdge>,
    /// See `FileState::require_notes`.
    pub require_notes: Vec<(Span, RequireNote)>,
    pub timings: Option<Timings>,
}

impl FileOut {
    /// The per-file pass crashed: the R0901 is the whole result.
    fn crashed(diagnostic: Diagnostic) -> Self {
        FileOut {
            degraded: false,
            crashed: true,
            failed: Some(vec![diagnostic]),
            diags: Vec::new(),
            deps: Vec::new(),
            usage: HashMap::new(),
            read_names: HashSet::new(),
            string_names: HashMap::new(),
            eval_deps: HashSet::new(),
            load_edges: Vec::new(),
            require_notes: Vec::new(),
            timings: None,
        }
    }
}

/// Parse `src`, containing a crash in the parser or the lowering: the file
/// then counts as unparsed and carries an R0901 in place of an R0900.
fn parse_guarded(path: &Path, src: &str) -> (Option<hir::Module>, Option<Diagnostic>) {
    let guarded = guard::catch(|| {
        guard::phase("parse", || {
            guard::fault("parse", path);
            rime_syntax::parse(src).ok().map(|mut module| {
                crate::project::mark_rebound_require(&mut module);
                module
            })
        })
    });
    match guarded {
        Ok(module) => (module, None),
        Err(crash) => (None, Some(crash.diagnostic("parsing", "none of this file was checked"))),
    }
}

pub struct Session<'r> {
    resolver: Option<Box<dyn Resolver + 'r>>,
    std: rime_std::StdNames,
    /// rime.toml's settings for the rules that take their own.
    options: crate::RuleOptions,
    /// Insertion order — the CLI's output order.
    order: Vec<PathBuf>,
    files: HashMap<PathBuf, FileState>,
    /// raw require target → files that bind it.
    consumers: HashMap<PathBuf, HashSet<PathBuf>>,
    /// Reverse of `FileState::eval_deps`: a file → files whose evaluator run
    /// read ITS HIR while resolving a `Dynamic` require site. Maintained
    /// exactly like `consumers`, keyed off `eval_deps` instead of `deps`.
    eval_consumers: HashMap<PathBuf, HashSet<PathBuf>>,
    /// file → linkers whose chain passes through it.
    chains: HashMap<PathBuf, HashSet<PathBuf>>,
    failed_parses: usize,
    degraded_files: usize,
    /// Files whose per-file pass crashed (`FileState::crashed`). Like a
    /// failed parse, one of these means usage is incomplete, so R0309 is
    /// off while it is non-zero.
    crashed_files: usize,
    /// R0901s for crashes in whole-project work that belongs to no file
    /// (the require-cycle search). Replaced on every pass 3.
    project_errors: Vec<Diagnostic>,
    /// Files whose `iface_crash` changed since the last finalize — their
    /// report moved without pass 2 or pass 3 saying so.
    iface_dirty: Vec<PathBuf>,
    /// The affected set computed by the most recent `update`/`add`/`remove`
    /// (before `rerun` widens it with pass2/pass3 changes). Exposed only for
    /// tests.
    last_affected: Vec<PathBuf>,
}

impl<'r> Session<'r> {
    pub fn new(
        files: Vec<(PathBuf, String)>,
        resolver: Option<Box<dyn Resolver + 'r>>,
        std: rime_std::StdNames,
        timings: Option<&mut Timings>,
    ) -> Self {
        Self::with_options(files, resolver, std, crate::RuleOptions::default(), timings)
    }

    /// `new` under a project's rule settings (rime.toml).
    pub fn with_options(
        files: Vec<(PathBuf, String)>,
        resolver: Option<Box<dyn Resolver + 'r>>,
        std: rime_std::StdNames,
        options: crate::RuleOptions,
        mut timings: Option<&mut Timings>,
    ) -> Self {
        let mut s = Session {
            resolver,
            std,
            options,
            order: Vec::new(),
            files: HashMap::new(),
            consumers: HashMap::new(),
            eval_consumers: HashMap::new(),
            chains: HashMap::new(),
            failed_parses: 0,
            degraded_files: 0,
            crashed_files: 0,
            project_errors: Vec::new(),
            iface_dirty: Vec::new(),
            last_affected: Vec::new(),
        };
        let timed = timings.is_some();

        // ---- pass 1: parse (LPT schedule, as before) ----------------------
        let parse_start = timed.then(Instant::now);
        let mut order: Vec<usize> = (0..files.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(files[i].1.len()));
        let next = AtomicUsize::new(0);
        type Parsed = (usize, Option<hir::Module>, Option<Diagnostic>, Option<Duration>);
        let per_thread: Vec<Vec<Parsed>> = rayon::broadcast(|_| {
            let mut out = Vec::new();
            while let Some(&i) = order.get(next.fetch_add(1, Ordering::Relaxed)) {
                let start = timed.then(Instant::now);
                let (module, crash) = parse_guarded(&files[i].0, &files[i].1);
                out.push((i, module, crash, start.map(|s| s.elapsed())));
            }
            out
        });
        let mut parsed: Vec<(Option<hir::Module>, Option<Diagnostic>, Option<Duration>)> =
            (0..files.len()).map(|_| (None, None, None)).collect();
        for (i, module, crash, dur) in per_thread.into_iter().flatten() {
            parsed[i] = (module, crash, dur);
        }
        let mut parse_cpu = Duration::ZERO;
        for ((path, src), (module, parse_crash, dur)) in files.into_iter().zip(parsed) {
            if let (Some(t), Some(dur)) = (timings.as_deref_mut(), dur) {
                t.record_file(&path, dur);
                parse_cpu += dur;
            }
            if module.is_none() {
                s.failed_parses += 1;
            }
            s.order.push(path.clone());
            s.files.insert(path, FileState { src, module, parse_crash, ..Default::default() });
        }
        if let (Some(t), Some(start)) = (timings.as_deref_mut(), parse_start) {
            t.record("parse + lower", start.elapsed());
            let mut cpu = Timings::default();
            cpu.record("parse", parse_cpu);
            t.merge_cpu(cpu);
        }

        // ---- interfaces + linker chains ----------------------------------
        let iface_start = timed.then(Instant::now);
        s.recompute_interfaces(None);
        if let (Some(t), Some(start)) = (timings.as_deref_mut(), iface_start) {
            t.record("interfaces", start.elapsed());
        }

        // ---- pass 2 for every file ---------------------------------------
        let check_start = timed.then(Instant::now);
        let all: Vec<PathBuf> = s.order.clone();
        s.run_pass2(&all, timings.as_deref_mut());
        if let (Some(t), Some(start)) = (timings.as_deref_mut(), check_start) {
            t.record("check files", start.elapsed());
        }

        // ---- pass 3 + finalize -------------------------------------------
        time_opt(&mut timings, "rule: exports", || s.run_pass3());
        let all: Vec<PathBuf> = s.order.clone();
        time_opt(&mut timings, "finalize diags", || s.finalize(&all));
        s.iface_dirty.clear();
        s
    }

    /// (interface, chain) for `path`, following `return require(...)` linkers.
    /// Same depth cap as the old `interface_of`/`resolve_linker_chain`: nine
    /// levels, then an empty interface.
    fn interface_and_chain(&self, path: &Path) -> (ModuleInterface, Vec<PathBuf>) {
        let mut chain = Vec::new();
        let mut cur = path.to_path_buf();
        for _ in 0..=8 {
            let Some(state) = self.files.get(&cur) else {
                return (ModuleInterface::default(), chain);
            };
            let Some(module) = &state.module else {
                return (ModuleInterface::default(), chain);
            };
            let Some(target) = linker_target(module) else {
                let display = cur
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| cur.display().to_string());
                let file = cur
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| cur.display().to_string());
                return (calls::module_interface(module, &display, &file, &state.src), chain);
            };
            let Some(resolver) = self.resolver.as_deref() else {
                return (ModuleInterface::default(), chain);
            };
            let env = local_env(module);
            match resolver.resolve(&cur, target, &env) {
                Resolution::Found(dep) => {
                    chain.push(dep.clone());
                    cur = dep;
                }
                _ => return (ModuleInterface::default(), chain),
            }
        }
        (ModuleInterface::default(), chain)
    }

    /// Recompute interfaces for `only` (or every file) and rebuild the
    /// `chains` reverse map for them. Returns the paths whose interface or
    /// terminal changed.
    fn recompute_interfaces(&mut self, only: Option<&[PathBuf]>) -> Vec<PathBuf> {
        let targets: Vec<PathBuf> = match only {
            Some(p) => p.to_vec(),
            None => self.order.clone(),
        };
        // `self.interface_and_chain` only needs `&self`, but calling it from
        // inside a `Fn` closure fanned out by rayon requires reborrowing
        // `self` (a `&mut Session`) as `&Session` up front — a `&mut`
        // reference cannot be captured by a closure invoked more than once.
        let s: &Session<'_> = &*self;
        type Computed = (PathBuf, ModuleInterface, Vec<PathBuf>, Option<Diagnostic>);
        let computed: Vec<Computed> = targets
            .par_iter()
            .map(|p| {
                let guarded = guard::catch(|| {
                    guard::fault("interface", p);
                    guard::phase("interfaces", || s.interface_and_chain(p))
                });
                match guarded {
                    Ok((iface, chain)) => (p.clone(), iface, chain, None),
                    // An empty interface is what an unparsed file has:
                    // consumers find nothing to check their uses against.
                    Err(crash) => (
                        p.clone(),
                        ModuleInterface::default(),
                        Vec::new(),
                        Some(crash.diagnostic(
                            "extracting the interface of",
                            "other files' uses of this module were not checked against it",
                        )),
                    ),
                }
            })
            .collect();
        let mut changed = Vec::new();
        for (path, iface, chain, crash) in computed {
            // `path` can be absent here: a caller's `only` list can name a
            // path that was removed from `self.files` in this same call
            // (e.g. `remove` hands in linkers reached through a require
            // cycle that resolves back to the just-removed file within the
            // 9-hop cap). An unknown path
            // contributes nothing: no `changed` entry, no `chains` update.
            let Some(state) = self.files.get_mut(&path) else { continue };
            let terminal_changed = state.linker_chain.last() != chain.last();
            if state.interface != iface || terminal_changed {
                changed.push(path.clone());
            }
            for x in &state.linker_chain {
                if let Some(set) = self.chains.get_mut(x) {
                    set.remove(&path);
                }
            }
            for x in &chain {
                self.chains.entry(x.clone()).or_default().insert(path.clone());
            }
            state.interface = iface;
            state.linker_chain = chain;
            if state.iface_crash != crash {
                state.iface_crash = crash;
                self.iface_dirty.push(path);
            }
        }
        changed
    }

    /// Terminal of a raw require target (old `resolve_linker_chain`).
    fn terminal_of(&self, raw: &Path) -> PathBuf {
        self.files
            .get(raw)
            .and_then(|f| f.linker_chain.last().cloned())
            .unwrap_or_else(|| raw.to_path_buf())
    }

    /// Pass 2 for `paths` in parallel, folded into `files` and the consumer
    /// edges in `paths` order. Returns the paths whose `pass2` changed.
    fn run_pass2(&mut self, paths: &[PathBuf], mut timings: Option<&mut Timings>) -> Vec<PathBuf> {
        let timed = timings.is_some();
        // Same reborrow-before-fan-out shape as `recompute_interfaces`:
        // `check_file` takes `&Session`, so a `&mut Session` cannot be
        // captured directly by the parallel closure.
        let s: &Session<'_> = &*self;
        let outs: Vec<FileOut> = paths
            .par_iter()
            .map(|p| {
                let guarded = guard::catch(|| {
                    guard::fault("check", p);
                    check_file(p, s, timed)
                });
                guarded.unwrap_or_else(|crash| {
                    FileOut::crashed(crash.diagnostic(
                        "checking",
                        "none of this file's findings are known, and whole-project rules ran \
                         without it (unused-export is off for this run)",
                    ))
                })
            })
            .collect();
        let mut changed = Vec::new();
        for (path, out) in paths.iter().zip(outs) {
            if let (Some(t), Some(local)) = (timings.as_deref_mut(), out.timings) {
                t.merge_cpu(local);
            }
            let state = self.files.get_mut(path).expect("known path");
            for (_, raw, _) in &state.deps {
                if let Some(set) = self.consumers.get_mut(raw) {
                    set.remove(path);
                }
            }
            for (_, raw, _) in &out.deps {
                self.consumers.entry(raw.clone()).or_default().insert(path.clone());
            }
            for dep in &state.eval_deps {
                if let Some(set) = self.eval_consumers.get_mut(dep) {
                    set.remove(path);
                }
            }
            for dep in &out.eval_deps {
                self.eval_consumers.entry(dep.clone()).or_default().insert(path.clone());
            }
            match (state.degraded, out.degraded) {
                (false, true) => self.degraded_files += 1,
                (true, false) => self.degraded_files -= 1,
                _ => {}
            }
            match (state.crashed, out.crashed) {
                (false, true) => self.crashed_files += 1,
                (true, false) => self.crashed_files -= 1,
                _ => {}
            }
            state.crashed = out.crashed;
            let new_pass2 = out.failed.unwrap_or(out.diags);
            if state.pass2 != new_pass2 {
                changed.push(path.clone());
            }
            state.pass2 = new_pass2;
            state.deps = out.deps;
            state.degraded = out.degraded;
            state.usage = out.usage;
            state.read_names = out.read_names;
            state.string_names = out.string_names;
            state.eval_deps = out.eval_deps;
            state.load_edges = out.load_edges;
            state.require_notes = out.require_notes;
        }
        changed
    }

    /// R0309 over every defining module and R0509 over the load-time
    /// require graph, from resident state. Returns the paths whose `pass3`
    /// changed.
    fn run_pass3(&mut self) -> Vec<PathBuf> {
        // R0509 first: whole-graph, serial, O(files + edges). Unlike R0309
        // it is not gated on every file parsing — a file that failed to
        // parse has no edges, and cannot invent a cycle for anyone else.
        let cycle_search = {
            let edges: HashMap<&Path, &[LoadEdge]> = self
                .files
                .iter()
                .map(|(p, f)| (p.as_path(), f.load_edges.as_slice()))
                .collect();
            guard::catch(|| {
                guard::fault("cycles", Path::new(""));
                guard::phase("rule: cycles", || cycles::check(&self.order, &edges))
            })
        };
        // The search covers the whole graph, so a crash in it has no file
        // to sit on: it is the session's own error, and no file gets R0509.
        let cycle_diags = match cycle_search {
            Ok(diags) => {
                self.project_errors.clear();
                diags
            }
            Err(crash) => {
                self.project_errors = vec![crash.project_diagnostic(
                    "searching the project for require cycles",
                    "circular-require (R0509) reported nothing in this run",
                )];
                HashMap::new()
            }
        };
        let mut usage: HashMap<PathBuf, Usage> = HashMap::new();
        for path in &self.order {
            for (dep, u) in &self.files[path].usage {
                let merged = usage.entry(dep.clone()).or_default();
                merged.used.extend(u.used.iter().cloned());
                merged.poisoned |= u.poisoned;
            }
        }
        let global_names: Option<exports::GlobalNames> = (self.degraded_files > 0).then(|| {
            let mut names = exports::GlobalNames::default();
            // Files are numbered by their place in `order`, which is what
            // `locate` below turns back into a path.
            for (file, path) in self.order.iter().enumerate() {
                let state = &self.files[path];
                names.reads.extend(state.read_names.iter().cloned());
                for (name, offsets) in &state.string_names {
                    names
                        .strings
                        .entry(name.clone())
                        .or_default()
                        .extend(offsets.iter().map(|&offset| (file, offset)));
                }
            }
            names
        });
        let locate = |file: usize, offset: usize| {
            let path = &self.order[file];
            rime_diag::Site { path: path.clone(), line: line_of(&self.files[path].src, offset) }
        };
        // A file whose per-file pass crashed contributed no usage, exactly
        // like one that did not parse: every export it uses would read as
        // unused.
        let enabled = self.failed_parses == 0 && self.crashed_files == 0;
        let computed: Vec<(PathBuf, Vec<Diagnostic>)> = self
            .order
            .par_iter()
            .enumerate()
            .map(|(file, path)| {
                let mut diags = Vec::new();
                if let Some(c) = cycle_diags.get(path) {
                    diags.extend(c.iter().cloned());
                }
                let unused = guard::catch(|| {
                    guard::fault("exports", path);
                    guard::phase("rule: exports", || {
                        let mut out = Vec::new();
                        let state = &self.files[path];
                        let Some(module) = &state.module else { return out };
                        if !enabled || linker_target(module).is_some() {
                            return out;
                        }
                        if let Some(u) = usage.get(path) {
                            let degraded = global_names
                                .as_ref()
                                .map(|names| exports::Degraded { names, file, locate: &locate });
                            exports::check_unused(
                                &state.interface,
                                u,
                                degraded.as_ref(),
                                module,
                                &mut out,
                            );
                        }
                        out
                    })
                });
                match unused {
                    Ok(out) => diags.extend(out),
                    Err(crash) => diags.push(crash.diagnostic(
                        "checking the exports of",
                        "unused-export (R0309) was not checked for this file",
                    )),
                }
                (path.clone(), diags)
            })
            .collect();
        let mut changed = Vec::new();
        for (path, diags) in computed {
            let state = self.files.get_mut(&path).expect("known path");
            if state.pass3 != diags {
                changed.push(path.clone());
            }
            state.pass3 = diags;
        }
        changed
    }

    /// allow directives + ordering for `paths`. Returns the paths whose
    /// `report` changed.
    fn finalize(&mut self, paths: &[PathBuf]) -> Vec<PathBuf> {
        let mut changed = Vec::new();
        for path in paths {
            let state = self.files.get_mut(path).expect("known path");
            let (report, suppressed) = if state.module.is_none() {
                (state.pass2.clone(), 0)
            } else {
                let mut diags = state.pass2.clone();
                diags.extend(state.pass3.iter().cloned());
                diags.extend(state.iface_crash.iter().cloned());
                // R0901 is about Rime, not about this file's code: no allow
                // directive in the file reaches it.
                let (internal, findings): (Vec<Diagnostic>, Vec<Diagnostic>) =
                    diags.into_iter().partition(Diagnostic::is_internal_error);
                let src = state.src.as_str();
                let applied = guard::catch(|| {
                    guard::fault("finalize", path);
                    guard::phase("allow directives", || rime_diag::allow::apply(src, findings))
                });
                match applied {
                    Ok(applied) => {
                        let mut kept = applied.kept;
                        kept.extend(internal);
                        (rime_diag::finalize(kept), applied.suppressed)
                    }
                    Err(crash) => {
                        let mut kept = internal;
                        kept.push(crash.diagnostic(
                            "applying the allow directives of",
                            "this file's findings are withheld: its allow directives could \
                             not be applied to them",
                        ));
                        (rime_diag::finalize(kept), 0)
                    }
                }
            };
            if state.report != report || state.suppressed != suppressed {
                changed.push(path.clone());
            }
            state.report = report;
            state.suppressed = suppressed;
        }
        changed
    }

    /// R0901s for crashes in whole-project work, which no file's report
    /// carries (see the field). Per-file crashes are in that file's report.
    pub fn project_errors(&self) -> &[Diagnostic] {
        &self.project_errors
    }

    pub fn report(&self, path: &Path) -> Option<(&[Diagnostic], usize)> {
        self.files.get(path).map(|f| (f.report.as_slice(), f.suppressed))
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.order.iter().map(PathBuf::as_path)
    }

    pub fn reports(&self) -> Vec<FileReport> {
        self.order
            .iter()
            .map(|p| {
                let f = &self.files[p];
                FileReport { path: p.clone(), diagnostics: f.report.clone(), suppressed: f.suppressed }
            })
            .collect()
    }

    /// The requires in `path` that the run reports once instead of per
    /// call, in source order: the call's span and why. Empty for a file
    /// with none, and for every file when there is no resolver.
    pub fn require_notes(&self, path: &Path) -> &[(Span, RequireNote)] {
        self.files.get(path).map_or(&[], |f| f.require_notes.as_slice())
    }

    /// A file's source text, for rendering diagnostics against `report()`.
    pub fn source(&self, path: &Path) -> Option<&str> {
        self.files.get(path).map(|f| f.src.as_str())
    }

    /// A file's HIR, for consumers that run their own passes over it (the
    /// CLI's custom-rule pass). None for a file that failed to parse.
    pub fn module(&self, path: &Path) -> Option<&hir::Module> {
        self.files.get(path)?.module.as_ref()
    }

    /// Drop the HIR in parallel — freeing many modules serially costs more
    /// than the check pass on Windows. Call after `reports()`/`source()`;
    /// consumes the session.
    pub fn drop_parallel(self, mut timings: Option<&mut Timings>) {
        let files = self.files;
        time_opt(&mut timings, "drop hir", || files.into_par_iter().for_each(drop));
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    /// A held file's resident state — for go-to-definition (`definition.rs`),
    /// which reads `module`/`deps`/`interface` without re-running any pass.
    pub(crate) fn file_state(&self, path: &Path) -> Option<&FileState> {
        self.files.get(path)
    }

    /// The require resolver, for go-to-definition's `require(...)` jump.
    pub(crate) fn resolver_ref(&self) -> Option<&(dyn Resolver + 'r)> {
        self.resolver.as_deref()
    }

    #[doc(hidden)]
    pub fn last_affected(&self) -> &[PathBuf] {
        &self.last_affected
    }

    /// Re-read `path` from `src`. Returns every path whose report changed.
    pub fn update(&mut self, path: &Path, src: String) -> Vec<PathBuf> {
        if !self.files.contains_key(path) {
            return self.add(path.to_path_buf(), src);
        }
        let (module, parse_crash) = parse_guarded(path, &src);
        {
            let state = self.files.get_mut(path).expect("known path");
            match (state.module.is_none(), module.is_none()) {
                (false, true) => self.failed_parses += 1,
                (true, false) => self.failed_parses -= 1,
                _ => {}
            }
            state.src = src;
            state.module = module;
            state.parse_crash = parse_crash;
        }
        let affected = self.affected_for(path);
        self.rerun(affected)
    }

    /// Insert a file the session does not hold. A path it already holds is
    /// an `update`, not a second insertion — adding twice would duplicate
    /// the `order` entry and leak the old state's consumer/chain edges.
    pub fn add(&mut self, path: PathBuf, src: String) -> Vec<PathBuf> {
        if self.files.contains_key(&path) {
            return self.update(&path, src);
        }
        let (module, parse_crash) = parse_guarded(&path, &src);
        if module.is_none() {
            self.failed_parses += 1;
        }
        self.order.push(path.clone());
        self.files.insert(path.clone(), FileState { src, module, parse_crash, ..Default::default() });
        let mut affected = self.affected_for(&path);
        // A consumer whose stale require pointed at this path now resolves.
        if let Some(set) = self.consumers.get(&path) {
            affected.extend(set.iter().cloned());
        }
        let mut changed = self.rerun(affected);
        // `path` is brand new to resident state: its report is diffed
        // against a just-`Default`-initialized `FileState`, so a fresh
        // report that happens to equal the empty default (no diagnostics)
        // would otherwise vanish from `changed` even though the file went
        // from absent to present. Always report it, mirroring `remove`.
        if !changed.contains(&path) {
            changed.push(path);
        }
        changed
    }

    pub fn remove(&mut self, path: &Path) -> Vec<PathBuf> {
        let Some(state) = self.files.remove(path) else { return Vec::new() };
        self.order.retain(|p| p != path);
        if state.module.is_none() {
            self.failed_parses -= 1;
        }
        if state.degraded {
            self.degraded_files -= 1;
        }
        if state.crashed {
            self.crashed_files -= 1;
        }
        for (_, raw, _) in &state.deps {
            if let Some(set) = self.consumers.get_mut(raw) {
                set.remove(path);
            }
        }
        for dep in &state.eval_deps {
            if let Some(set) = self.eval_consumers.get_mut(dep) {
                set.remove(path);
            }
        }
        for x in &state.linker_chain {
            if let Some(set) = self.chains.get_mut(x) {
                set.remove(path);
            }
        }
        let mut affected: HashSet<PathBuf> = self.consumers.remove(path).unwrap_or_default();
        // Body-sensitivity, same as `affected_for`: a file whose eval read
        // THIS file's HIR (an intermediate `require(...)` chased mid-eval,
        // never necessarily this file's own require target or a linker hop)
        // must re-run too — its cached resolution silently goes stale
        // otherwise. Snapshotted via `remove` (not `get`) right alongside
        // `consumers`/`chains` above, before either the `linkers` interface
        // recompute or `rerun`'s own edge rewrite can touch it.
        affected.extend(self.eval_consumers.remove(path).into_iter().flatten());
        let linkers = self.chains.remove(path).unwrap_or_default();
        let linkers: Vec<PathBuf> = linkers.into_iter().collect();
        for l in self.recompute_interfaces(Some(&linkers)) {
            affected.extend(self.consumers.get(&l).into_iter().flatten().cloned());
        }
        // `rerun` filters `affected` against `self.files` itself (a path can
        // legitimately name the just-removed file here), so no retain needed.
        let mut changed = self.rerun(affected);
        changed.push(path.to_path_buf());
        changed
    }

    /// Replace the rule settings. Nothing is rechecked here: follow with
    /// `reset`, which rechecks every file.
    pub fn set_options(&mut self, options: crate::RuleOptions) {
        self.options = options;
    }

    /// Full warm recheck against a new file set / resolver / std. Modules
    /// whose text is unchanged are reused, not reparsed. The rule settings
    /// carry over (`set_options` changes them).
    pub fn reset(
        &mut self,
        files: Vec<(PathBuf, String)>,
        resolver: Option<Box<dyn Resolver + 'r>>,
        std: rime_std::StdNames,
    ) -> Vec<PathBuf> {
        let old_reports: HashMap<PathBuf, (Vec<Diagnostic>, usize)> = self
            .files
            .iter()
            .map(|(p, f)| (p.clone(), (f.report.clone(), f.suppressed)))
            .collect();
        let mut cache: HashMap<PathBuf, FileState> = std::mem::take(&mut self.files);
        let mut fresh = Session {
            resolver,
            std,
            options: std::mem::take(&mut self.options),
            order: Vec::new(),
            files: HashMap::new(),
            consumers: HashMap::new(),
            eval_consumers: HashMap::new(),
            chains: HashMap::new(),
            failed_parses: 0,
            degraded_files: 0,
            crashed_files: 0,
            project_errors: Vec::new(),
            iface_dirty: Vec::new(),
            last_affected: Vec::new(),
        };
        // Parse only what changed; the LPT schedule of `new` is not worth
        // reproducing for a handful of files. Reuse-vs-reparse is decided
        // per path below (read-only against `cache`); the parallel part is
        // the `to_parse` fan-out that follows.
        let mut to_parse: Vec<usize> = Vec::new();
        let mut states: Vec<(PathBuf, FileState)> = Vec::with_capacity(files.len());
        for (i, (path, src)) in files.into_iter().enumerate() {
            match cache.remove(&path) {
                Some(old) if old.src == src => states.push((
                    path,
                    FileState {
                        src,
                        module: old.module,
                        parse_crash: old.parse_crash,
                        ..Default::default()
                    },
                )),
                _ => {
                    to_parse.push(i);
                    states.push((path, FileState { src, ..Default::default() }))
                }
            }
        }
        let modules: Vec<(usize, (Option<hir::Module>, Option<Diagnostic>))> = to_parse
            .par_iter()
            .map(|&i| (i, parse_guarded(&states[i].0, &states[i].1.src)))
            .collect();
        for (i, (module, parse_crash)) in modules {
            states[i].1.module = module;
            states[i].1.parse_crash = parse_crash;
        }
        for (path, state) in states {
            if state.module.is_none() {
                fresh.failed_parses += 1;
            }
            fresh.order.push(path.clone());
            fresh.files.insert(path, state);
        }
        fresh.recompute_interfaces(None);
        let all = fresh.order.clone();
        fresh.run_pass2(&all, None);
        fresh.run_pass3();
        fresh.finalize(&all);
        fresh.iface_dirty.clear();
        *self = fresh;
        let mut changed: Vec<PathBuf> = self
            .order
            .iter()
            .filter(|p| {
                let f = &self.files[*p];
                old_reports.get(*p) != Some(&(f.report.clone(), f.suppressed))
            })
            .cloned()
            .collect();
        changed.extend(old_reports.keys().filter(|p| !self.files.contains_key(*p)).cloned());
        changed
    }

    /// The affected set: the edited file, its consumers when its interface
    /// or terminal changed, and the consumers of every linker whose chain
    /// runs through it and whose interface changed as a result.
    fn affected_for(&mut self, path: &Path) -> HashSet<PathBuf> {
        let mut affected: HashSet<PathBuf> = HashSet::new();
        affected.insert(path.to_path_buf());
        let own = self.recompute_interfaces(Some(&[path.to_path_buf()]));
        if !own.is_empty() {
            affected.extend(self.consumers.get(path).into_iter().flatten().cloned());
        }
        let through: Vec<PathBuf> = self.chains.get(path).into_iter().flatten().cloned().collect();
        for l in self.recompute_interfaces(Some(&through)) {
            affected.extend(self.consumers.get(&l).into_iter().flatten().cloned());
        }
        // Body-sensitivity, unconditional: a file whose eval read `path`'s
        // HIR (`eval_deps`) can see a different require outcome from a pure
        // body edit that never touches `path`'s interface — the ordinary
        // interface-diff widening above would miss it entirely.
        // Never gated on `own`/`through` changing.
        affected.extend(self.eval_consumers.get(path).into_iter().flatten().cloned());
        affected.retain(|p| self.files.contains_key(p));
        affected
    }

    /// Re-runs the passes for an affected set; returns changed reports.
    fn rerun(&mut self, affected: HashSet<PathBuf>) -> Vec<PathBuf> {
        let mut affected: Vec<PathBuf> = affected.into_iter().collect();
        // Every caller's `affected` set may name a path no longer in
        // `self.files` (e.g. `remove`'s own consumers/chains snapshots are
        // taken around the removal) — filter here so `run_pass2`/`finalize`
        // never see an unknown path.
        affected.retain(|p| self.files.contains_key(p));
        affected.sort();
        self.last_affected = affected.clone();
        let mut touched: HashSet<PathBuf> = affected.iter().cloned().collect();
        touched.extend(self.run_pass2(&affected, None));
        touched.extend(self.run_pass3());
        // Interface crashes that came or went since the last finalize: the
        // callers recomputed interfaces before handing `affected` over.
        touched.extend(std::mem::take(&mut self.iface_dirty));
        touched.retain(|p| self.files.contains_key(p));
        let mut touched: Vec<PathBuf> = touched.into_iter().collect();
        touched.sort();
        self.finalize(&touched)
    }
}

/// Read access to other files' resident HIR/source for the evaluator
/// (eval.rs) — `check_file` builds one `Evaluator` per call from `&Session`
/// itself, so its cross-module field lookup (`export_field`) can chase a
/// `require`d module's own HIR without a second index.
impl crate::project::ModuleMap for Session<'_> {
    fn hir(&self, path: &Path) -> Option<&hir::Module> {
        self.files.get(path)?.module.as_ref()
    }
    fn source(&self, path: &Path) -> Option<&str> {
        Some(self.files.get(path)?.src.as_str())
    }
}

/// 1-based line of a byte offset — the newline-count idiom of
/// `requires.rs::line_of`, for a place in a file other than the one a
/// diagnostic is reported in.
fn line_of(src: &str, offset: usize) -> usize {
    src.as_bytes()[..offset.min(src.len())].iter().filter(|&&b| b == b'\n').count() + 1
}

/// The per-file pass (the old pass-2 closure). Reads only shared state.
pub(crate) fn check_file(path: &Path, s: &Session<'_>, timed: bool) -> FileOut {
    let mut local = timed.then(Timings::default);
    let mut lt = local.as_mut();
    let file_start = timed.then(Instant::now);
    let state = &s.files[path];
    let src = state.src.as_str();
    let std = &s.std;
    let Some(module) = &state.module else {
        return FileOut {
            degraded: false,
            crashed: false,
            // A parser crash is reported as itself; re-parsing for the
            // R0900 text would only crash again.
            failed: Some(match &state.parse_crash {
                Some(crash) => vec![crash.clone()],
                None => crate::check_source_configured(src, Some(path), std, &s.options),
            }),
            diags: Vec::new(),
            deps: Vec::new(),
            usage: HashMap::new(),
            read_names: HashSet::new(),
            string_names: HashMap::new(),
            eval_deps: HashSet::new(),
            load_edges: Vec::new(),
            require_notes: Vec::new(),
            timings: local,
        };
    };
    let Some(resolver) = s.resolver.as_deref() else {
        // No project: exactly `check_source_with`, but unfinalized so the
        // shared finalize step applies allow directives once.
        let mut diags = Vec::new();
        for body in &module.bodies {
            crate::check_body(module, body, src, &mut diags, &mut lt);
        }
        let names = crate::scope::resolve(module);
        calls::check_module(module, &names, &mut diags);
        crate::rules::must_use::check_module(module, &names, &mut diags);
        crate::rules::globals::check_module(module, std, &mut diags);
        crate::rules::retention::check_module(module, src, &s.options.retention, &mut diags);
        if let Some(options) = &s.options.module_scope {
            crate::rules::module_scope::check(module, &names, Some(path), src, options, &mut diags);
        }
        if let (Some(t), Some(start)) = (lt, file_start) {
            t.record_file(path, start.elapsed());
        }
        return FileOut {
            degraded: false,
            crashed: false,
            failed: None,
            diags,
            deps: Vec::new(),
            usage: HashMap::new(),
            read_names: HashSet::new(),
            string_names: HashMap::new(),
            eval_deps: HashSet::new(),
            load_edges: Vec::new(),
            require_notes: Vec::new(),
            timings: local,
        };
    };

    let env = time_opt(&mut lt, "alias env", || local_env(module));
    let eval = crate::eval::Evaluator::new(resolver, s).with_timing(timed);
    let mut externals: HashMap<String, &ModuleInterface> = HashMap::new();
    let mut usage_bindings: HashMap<String, PathBuf> = HashMap::new();
    let mut usage: HashMap<PathBuf, Usage> = HashMap::new();
    let mut deps = Vec::new();
    let mut degraded = exports::stray_require(module);
    time_opt(&mut lt, "resolve requires", || {
        for (local, target) in require_bindings(module) {
            let dep = match resolver.resolve(path, target, &env) {
                Resolution::Found(dep) => dep,
                Resolution::Missing { .. } | Resolution::OutsideView | Resolution::Trusted => continue,
                // Recognised and not followed (a loader's `require("Name")`,
                // an alias nothing defines): whatever module it loads is
                // used through a binding usage collection cannot attribute,
                // exactly like an unresolved Dynamic. The require rule
                // records the note; no diagnostic.
                Resolution::Unfollowed(_) => {
                    degraded = true;
                    continue;
                }
                // Cheap resolve gave up — re-ask the evaluator before
                // degrading. A `Found` it comes back with joins the SAME
                // downstream (deps edge, usage_bindings, externals lookup)
                // as a cheap Found; Missing/OutsideView/Trusted are left to
                // `rules::requires::check`'s diagnostics (same as the cheap
                // arms above). Everything else — NilCapable/Diverged/Opaque,
                // AND the evaluator landing on `Resolved(Dynamic)` itself
                // (never wildcarded together with Trusted/Missing/
                // OutsideView above: those are proven outcomes the cheap
                // path already treats as "nothing to add here", Dynamic
                // isn't) — degrades exactly like an unresolved Dynamic
                // always has.
                Resolution::Dynamic => match eval.eval_require_target(path, module, target) {
                    crate::eval::Outcome::Resolved(Resolution::Found(dep)) => dep,
                    crate::eval::Outcome::Resolved(
                        Resolution::Missing { .. } | Resolution::OutsideView | Resolution::Trusted,
                    ) => continue,
                    crate::eval::Outcome::Resolved(Resolution::Dynamic | Resolution::Unfollowed(_))
                    | crate::eval::Outcome::NilCapable { .. }
                    | crate::eval::Outcome::Diverged { .. }
                    | crate::eval::Outcome::Opaque => {
                        degraded = true;
                        continue;
                    }
                },
            };
            let terminal = s.terminal_of(&dep);
            deps.push((local.clone(), dep.clone(), terminal.clone()));
            match usage_bindings.get(&local) {
                Some(prev) if *prev != terminal => {
                    usage.entry(prev.clone()).or_default().poisoned = true;
                    usage.entry(terminal.clone()).or_default().poisoned = true;
                }
                _ => {
                    usage_bindings.insert(local.clone(), terminal);
                }
            }
            if let Some(iface) = s.files.get(&dep).map(|f| &f.interface) {
                if !iface.functions.is_empty() || !iface.fields.is_empty() || iface.closed {
                    externals.insert(local, iface);
                }
            }
        }
    });
    time_opt(&mut lt, "rule: exports (usage)", || {
        exports::collect_usage(module, &usage_bindings, &mut usage)
    });
    // R0509's edges: every load-time require site that resolves to a file.
    // Same cheap-then-evaluator resolution as the deps loop above, but a
    // site that does not land on a file is simply not an edge — its own
    // code (R0501/R0502/R0503) already speaks for it, and nothing degrades.
    let load_edges: Vec<LoadEdge> = time_opt(&mut lt, "rule: cycles (sites)", || {
        cycles::load_sites(module)
            .into_iter()
            .filter_map(|site| {
                let target = match resolver.resolve(path, site.target, &env) {
                    Resolution::Found(dep) => dep,
                    Resolution::Dynamic => match eval.eval_require_target(path, module, site.target) {
                        crate::eval::Outcome::Resolved(Resolution::Found(dep)) => dep,
                        _ => return None,
                    },
                    _ => return None,
                };
                Some(LoadEdge { target, span: site.span, conditional: site.conditional })
            })
            .collect()
    });
    let mut read_names = HashSet::new();
    exports::collect_read_names(module, &mut read_names);
    let mut string_names = HashMap::new();
    exports::collect_string_names(module, &mut string_names);

    let mut diags = Vec::new();
    for body in &module.bodies {
        crate::check_body(module, body, src, &mut diags, &mut lt);
    }
    let names = time_opt(&mut lt, "name resolution", || crate::scope::resolve(module));
    time_opt(&mut lt, "rule: calls", || calls::check_module_with(module, &names, &externals, &mut diags));
    time_opt(&mut lt, "rule: must-use", || {
        crate::rules::must_use::check_module(module, &names, &mut diags)
    });
    time_opt(&mut lt, "rule: members", || {
        crate::rules::members::check_field_reads(module, &externals, &mut diags)
    });
    time_opt(&mut lt, "rule: globals", || {
        crate::rules::globals::check_module(module, std, &mut diags)
    });
    time_opt(&mut lt, "rule: retention", || {
        crate::rules::retention::check_module(module, src, &s.options.retention, &mut diags)
    });
    let mut require_notes = Vec::new();
    time_opt(&mut lt, "rule: requires", || {
        crate::rules::requires::check(
            module,
            path,
            resolver,
            Some(&eval),
            &env,
            src,
            &mut diags,
            &mut require_notes,
        )
    });
    // A call through a rebound `require` loads whatever the file's own
    // function decides; its result is used through a binding nothing here
    // can attribute.
    degraded |= require_notes.iter().any(|(_, n)| *n == RequireNote::Rebound);
    time_opt(&mut lt, "rule: naming", || crate::rules::naming::check(module, path, &mut diags));
    if let Some(options) = &s.options.module_scope {
        time_opt(&mut lt, "rule: module-scope", || {
            crate::rules::module_scope::check(module, &names, Some(path), src, options, &mut diags)
        });
    }
    let eval_deps = eval.take_deps();
    // NESTED phase, unlike every other one here: the evaluator runs inside
    // "resolve requires" and inside "rule: requires", so this duration is
    // already counted in those two and must not be read as a share of the
    // CPU total. The name says so; it is here to answer "what does the
    // export-value evaluator cost on a whole project".
    if let (Some(t), Some(spent)) = (lt.as_mut(), eval.spent()) {
        t.record("eval (nested)", spent);
    }
    if let (Some(t), Some(start)) = (lt, file_start) {
        t.record_file(path, start.elapsed());
    }
    FileOut {
        degraded,
        crashed: false,
        failed: None,
        diags,
        deps,
        usage,
        read_names,
        string_names,
        eval_deps,
        load_edges,
        require_notes,
        timings: local,
    }
}
