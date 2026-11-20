//! Project-wide checking.
//!
//! Single-file checking stays `check_source`. This layer adds the cross-file
//! half of call conformance: every module's exported interface is extracted
//! once, `local X = require(<target>)` bindings are resolved through a
//! pluggable `Resolver`, and each file's calls through those bindings are
//! checked against the required module's signatures.
//!
//! The resolver is a trait on purpose: this crate stays Roblox-agnostic.
//! `rime-roblox` implements Rojo-project resolution (DataModel paths,
//! GetService, WaitForChild); the corpus harness implements a trivial
//! sibling-file resolver. Both interpret the SAME HIR expression shape.
//!
//! Re-export linkers (`return require(...)` — wally/rotriever package stubs)
//! are followed transitively, depth-capped.

use crate::timings::Timings;
use rime_diag::Diagnostic;
use rime_hir as hir;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The outcome of interpreting one `require(...)` argument. The four-way
/// split is what keeps the require rules honest: only `Missing` claims a
/// proven absence, and only `Dynamic` claims the shape itself is the
/// problem.
#[derive(Debug, Clone)]
pub enum Resolution {
    /// The require names this file.
    Found(PathBuf),
    /// The path was fully interpreted and nothing is at the destination.
    /// `described` renders the interpreted path for the diagnostic.
    Missing { described: String },
    /// A static path that leads outside the resolver's view (an unmounted
    /// DataModel subtree, runtime-created instances). Unverifiable — fails
    /// by default (R0503, conservative), with its
    /// own code so exactly this class can be downgraded.
    OutsideView,
    /// Not statically interpretable: computed targets, unsupported shapes,
    /// aliases the top-level env cannot see. The dialect bans these
    /// (R0502), conservatively — the resolver's own gaps land here too.
    Dynamic,
    /// The user vouched for this exact target (`[requires] trust` in
    /// rime.toml) and it did not resolve to a real file. Silence — the
    /// trust list is the explicit, greppable escape hatch.
    Trusted,
    /// A require style the resolver recognises and does not follow: a
    /// string that is not a Luau path, an alias nothing defines. Not a
    /// defect in the code and not a shape the resolver failed on, so it
    /// gets no diagnostic of its own; the run says it once (see
    /// `RequireNote`). Like `Dynamic`, it leaves unused-export cautious.
    Unfollowed(RequireNote),
}

/// Why a require was not followed, or what is worth saying about one that
/// was. These are counted per run and printed once, never once per call:
/// they describe how a codebase writes its requires, not a mistake at one
/// site.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RequireNote {
    /// `require("Name")`: a string with no `./`, `../` or `@` prefix. Luau
    /// has no meaning for it; a project's own loader gives it one.
    BareString,
    /// `require("@name/...")` where no `.luaurc` in reach defines `name`.
    UnknownAlias(String),
    /// `require("@name/...")` where `name` is defined and its folder is
    /// not on disk (packages not installed, a path in the home folder).
    AliasNotOnDisk(String),
    /// A relative string in an `init` file that resolves only relative to
    /// the file's own folder — the rule Luau had before `@self`. Resolved
    /// and followed; the note says the current rule reads it differently.
    InitRelative,
    /// A relative string in an `init` file that names one module under
    /// the current Luau rule and a different one under the older rule, and
    /// both exist. Not followed: either could be the wrong module.
    InitAmbiguous,
    /// A call through a `require` the file rebinds (`local require = ...`).
    Rebound,
}

/// An instance location mid-evaluation: an absolute DataModel path or a
/// file-relative walk from a script instance. Moved up from rime-roblox so
/// the evaluator (eval.rs) can compose path steps generically.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Loc {
    /// Absolute DataModel path (segments after `game`).
    Dm(Vec<String>),
    /// File-relative walk from `script`. Names the INSTANCE (extension
    /// stripped; `init.luau` collapsed to its directory), not a file on
    /// disk — final resolution happens through `resolve_loc`.
    Fs(PathBuf),
}

/// One execution context a script can run in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Ctx {
    Server,
    Client,
}

/// The set of contexts a file may run in — server-only, client-only, or
/// both (shared modules, and anything the resolver can't place).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CtxSet {
    pub server: bool,
    pub client: bool,
}

impl CtxSet {
    pub const BOTH: CtxSet = CtxSet { server: true, client: true };

    pub fn bits(self) -> impl Iterator<Item = Ctx> {
        [(self.server, Ctx::Server), (self.client, Ctx::Client)]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, c)| c)
    }
}

/// Read access to other modules' lowered HIR and source — what the
/// evaluator needs to chase exported values. Session implements it;
/// the corpus harness's fixtures satisfy it through Session too.
pub trait ModuleMap: Sync {
    fn hir(&self, path: &Path) -> Option<&hir::Module>;
    fn source(&self, path: &Path) -> Option<&str>;
}

/// Resolves the argument of a `require(...)` to the file it names.
///
/// `from` is the requiring file; `target` is the require argument as lowered
/// HIR (`script.Parent.X` chains, service accesses, `:WaitForChild("X")`
/// calls all arrive as `Expr::Index`/`Expr::Call` chains).
pub trait Resolver: Sync {
    /// `env` maps the requiring file's top-level locals to their initializer
    /// expressions — how `local RS = game:GetService("ReplicatedStorage");
    /// require(RS.Modules.X)` resolves without the resolver re-walking the
    /// module.
    fn resolve(
        &self,
        from: &Path,
        target: &hir::Expr,
        env: &HashMap<String, &hir::Expr>,
    ) -> Resolution;

    /// Something worth saying once per run about a require that DID
    /// resolve (`resolve` returned `Found`): today only
    /// `RequireNote::InitRelative`. Asked by the require rule alone, once
    /// per call site of a checked file. Default: nothing.
    fn note(&self, _from: &Path, _target: &hir::Expr) -> Option<RequireNote> {
        None
    }

    /// Same-name module candidates for a broken require's "did the module
    /// move?" note: path descriptions (in the same dialect `resolve` uses
    /// for `Missing::described`) of every module named `name`, from the
    /// requiring file's view. Default: none — a resolver without an index
    /// simply opts out of the note.
    ///
    /// Each description must be a Luau expression for that module: when
    /// there is exactly one, R0501 offers to write it into the `require`.
    fn candidates(&self, _from: &Path, _name: &str) -> Vec<String> {
        Vec::new()
    }

    /// Whether `candidate` (one of `candidates`' descriptions) can be
    /// required from everywhere `from` runs. A module under a server-only
    /// container is no fix for a file a client runs, so R0501 names such a
    /// candidate in its note and offers no edit. Default: reachable.
    fn candidate_reachable(&self, _from: &Path, _candidate: &str) -> bool {
        true
    }

    /// One instance step: `.Name`, `:FindFirstChild("Name")`,
    /// `:WaitForChild("Name")`. `Parent` pops. None = can't step.
    fn step(&self, _base: &Loc, _seg: &str) -> Option<Loc> {
        None
    }
    /// `game:GetService("X")`.
    fn service(&self, _name: &str) -> Option<Loc> {
        None
    }
    /// The instance a script file IS (`script`).
    fn instance_of(&self, _file: &Path) -> Option<Loc> {
        None
    }
    /// Loc → file. The final on-disk resolution; memoized resolver-side.
    fn resolve_loc(&self, _loc: &Loc) -> Resolution {
        Resolution::Dynamic
    }

    /// Whether the project provably CREATES the instance at `loc` — as
    /// opposed to `resolve_loc`, which answers the narrower "is it a module
    /// file". A folder is the case that separates them:
    /// `ServerStorage.ServerPackages` is a mount, so `FindFirstChild` on it
    /// never returns nil, but it is not requireable and `resolve_loc` calls
    /// it `Missing`. The evaluator needs presence, not requireability, to
    /// prune the nil guard that follows such a lookup; without it a nil
    /// candidate that cannot occur reaches the require.
    ///
    /// `false` means NO PROOF, never a claim of absence: instances get
    /// parented at runtime, so nothing on disk is never evidence that a
    /// child is missing. Default: no proofs at all.
    fn present(&self, _loc: &Loc) -> bool {
        false
    }

    /// The requiring file's execution context, lexically: script-class stem
    /// and mount. Shared modules are BOTH. A
    /// reaching-BFS can replace this behind the same signature later.
    fn context_of(&self, _file: &Path) -> CtxSet {
        CtxSet::BOTH
    }
}

/// Structural equality for alias initializers — the identical-re-declaration
/// exception below. Conservative: shapes it does not model compare unequal,
/// so unknown constructs degrade to poisoning, never to a wrong match.
/// Spans are ignored on purpose (the two declarations ARE at different
/// sites); literal identity is prim + text.
pub(crate) fn expr_eq(a: &hir::Expr, b: &hir::Expr) -> bool {
    use hir::Expr as E;
    match (a, b) {
        (E::Name { name: an, .. }, E::Name { name: bn, .. }) => an == bn,
        (
            E::Index { base: ab, key: ak, name: an, .. },
            E::Index { base: bb, key: bk, name: bn, .. },
        ) => {
            an == bn
                && match (ak, bk) {
                    (None, None) => true,
                    (Some(x), Some(y)) => expr_eq(x, y),
                    _ => false,
                }
                && expr_eq(ab, bb)
        }
        (
            E::Call { base: ab, args: aa, method: am, .. },
            E::Call { base: bb, args: ba, method: bm, .. },
        ) => {
            am == bm
                && aa.len() == ba.len()
                && expr_eq(ab, bb)
                && aa.iter().zip(ba).all(|(x, y)| expr_eq(x, y))
        }
        (
            E::Literal { prim: ap, text: at, .. },
            E::Literal { prim: bp, text: bt, .. },
        ) => ap == bp && at == bt,
        _ => false,
    }
}

/// `local N = <expr>` initializers for alias resolution, collected from the
/// WHOLE module — service aliases live inside functions too
/// (`local RS = game:GetService(...)` in a handler). Scope is not modelled;
/// instead a name declared more than once anywhere is dropped entirely, so
/// the env never resolves an alias to the wrong initializer — ambiguity
/// degrades to Dynamic, never to a wrong path. One exception: re-declaring
/// a name with a STRUCTURALLY IDENTICAL initializer keeps it (corpus 058;
/// differing initializers still poison, corpus 059). Real scoped
/// environments arrive with the require-target evaluator, not here.
pub(crate) fn local_env(module: &hir::Module) -> HashMap<String, &hir::Expr> {
    let mut seen: HashMap<String, Option<&hir::Expr>> = HashMap::new();
    for body in &module.bodies {
        // Parameters are bindings with unknowable values — any alias name
        // they shadow must never resolve through the outer initializer.
        for p in &body.params {
            seen.insert(p.name.clone(), None);
        }
        collect_locals(&body.block, &mut seen);
    }
    seen.into_iter()
        .filter_map(|(k, v)| v.map(|e| (k, e)))
        .collect()
}

fn collect_locals<'m>(block: &'m hir::Block, seen: &mut HashMap<String, Option<&'m hir::Expr>>) {
    use hir::StmtKind as K;
    for stmt in &block.stmts {
        match &stmt.kind {
            K::Local { decls, init } => {
                for (i, d) in decls.iter().enumerate() {
                    let existing = seen.get(&d.name).copied();
                    match (existing, init.get(i)) {
                        // Identical re-declaration (the boot-script idiom:
                        // the same alias bound at module level AND inside a
                        // function): keep it. Identical initializers cannot
                        // eval to different places — every name they mention
                        // is itself either consistently bound or poisoned.
                        (Some(Some(prev)), Some(e)) if expr_eq(prev, e) => {}
                        // Second declaration with a different (or absent)
                        // initializer: poison the name.
                        (Some(_), _) => {
                            seen.insert(d.name.clone(), None);
                        }
                        (None, Some(e)) => {
                            seen.insert(d.name.clone(), Some(e));
                        }
                        (None, None) => {
                            seen.insert(d.name.clone(), None);
                        }
                    }
                }
            }
            K::Do(b) => collect_locals(b, seen),
            K::If { then, else_ifs, or_else, .. } => {
                collect_locals(then, seen);
                for (_, b) in else_ifs {
                    collect_locals(b, seen);
                }
                if let Some(b) = or_else {
                    collect_locals(b, seen);
                }
            }
            K::While { body, .. } | K::Repeat { body, .. } => collect_locals(body, seen),
            K::NumericFor { var, body, .. } => {
                seen.insert(var.name.clone(), None);
                collect_locals(body, seen);
            }
            K::GenericFor { vars, body, .. } => {
                for v in vars {
                    seen.insert(v.name.clone(), None);
                }
                collect_locals(body, seen);
            }
            // Loop variables / function names shadowing an alias name would
            // be a second binding; poison conservatively.
            K::LocalFunction { decl, .. } => {
                seen.insert(decl.name.clone(), None);
            }
            _ => {}
        }
    }
}

/// One checked file's result.
pub struct FileReport {
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
    /// Diagnostics removed by inline `-- rime: allow(...)` directives — the
    /// CLI folds these into its ignored count.
    pub suppressed: usize,
}

/// Check a whole set of files with cross-file call conformance.
///
/// Two passes: extract every file's interface (following re-export linkers),
/// then check every file with its require bindings resolved against those
/// interfaces. CSTs are dropped at lowering — only HIR is retained, which is
/// what keeps this within memory budget at repo scale.
pub fn check_project(
    files: &[(PathBuf, String)],
    resolver: &dyn Resolver,
) -> Vec<FileReport> {
    check_project_timed(files, resolver, None, rime_std::StdNames::bundled())
}

/// `check_project` with wall-clock accounting for `--timings`. `None` runs
/// with zero instrumentation overhead. `std` is the globals family's known-
/// name set (rime.toml's `[std]`, via `Config::build_std_names`, or the
/// bundled Luau+Roblox set) — threaded into pass 2's `rules::globals`
/// call below, same as `check_source_with`.
pub fn check_project_timed(
    files: &[(PathBuf, String)],
    resolver: &dyn Resolver,
    mut timings: Option<&mut Timings>,
    std: &rime_std::StdNames,
) -> Vec<FileReport> {
    // One forwarding shim so the borrowed resolver can live in the Session's
    // `Box<dyn Resolver + 'r>` slot. Forwards EVERY trait method, not just
    // `resolve`/`candidates` — the evaluator (eval.rs) calls `step`/
    // `service`/`instance_of`/`resolve_loc`/`present`/`context_of` through
    // whatever resolver `Session` holds, so a shim that only forwarded the
    // original two would silently opt every caller of `check_project`/
    // `check_project_timed` out of every evaluator primitive, even when the
    // caller's own `Resolver` impl provides them. Those callers are the
    // corpus harness and the session differential today (the CLI and the
    // LSP build `Session::new` themselves), which is exactly why the
    // forwarding has to be exhaustive: a missed method here is a silent
    // divergence between what the corpus proves and what ships. ADD EVERY
    // NEW TRAIT METHOD HERE.
    struct Borrowed<'a>(&'a dyn Resolver);
    impl Resolver for Borrowed<'_> {
        fn resolve(&self, from: &Path, target: &hir::Expr, env: &HashMap<String, &hir::Expr>) -> Resolution {
            self.0.resolve(from, target, env)
        }
        fn note(&self, from: &Path, target: &hir::Expr) -> Option<RequireNote> {
            self.0.note(from, target)
        }
        fn candidates(&self, from: &Path, name: &str) -> Vec<String> {
            self.0.candidates(from, name)
        }
        fn candidate_reachable(&self, from: &Path, candidate: &str) -> bool {
            self.0.candidate_reachable(from, candidate)
        }
        fn step(&self, base: &Loc, seg: &str) -> Option<Loc> {
            self.0.step(base, seg)
        }
        fn service(&self, name: &str) -> Option<Loc> {
            self.0.service(name)
        }
        fn instance_of(&self, file: &Path) -> Option<Loc> {
            self.0.instance_of(file)
        }
        fn resolve_loc(&self, loc: &Loc) -> Resolution {
            self.0.resolve_loc(loc)
        }
        fn present(&self, loc: &Loc) -> bool {
            self.0.present(loc)
        }
        fn context_of(&self, file: &Path) -> CtxSet {
            self.0.context_of(file)
        }
    }
    let session = crate::session::Session::new(
        files.to_vec(),
        Some(Box::new(Borrowed(resolver))),
        std.clone(),
        timings.as_deref_mut(),
    );
    let reports = session.reports();
    session.drop_parallel(timings);
    reports
}

/// The require target of a linker stub — a module whose ENTIRE export is
/// `return require(<target>)` (wally/rotriever package stubs).
pub(crate) fn linker_target(module: &hir::Module) -> Option<&hir::Expr> {
    let root = module.body(module.root);
    let hir::Terminator::Return { exprs, .. } = root.block.terminator.as_ref()? else {
        return None;
    };
    let hir::Expr::Call { base, args, .. } = exprs.first()? else {
        return None;
    };
    if !module.is_require(base) {
        return None;
    }
    args.first()
}

/// Fill `Module::rebound_require`: every `require` name that reaches a
/// local the file declares. Run once on a freshly parsed module, before
/// anything asks `Module::is_require`.
///
/// Scoping is Luau's (`scope::resolve`): in `local require = require(x)`
/// the right-hand name is still the builtin, and only the names after the
/// statement reach the new local. A file that declares nothing named
/// `require` — nearly all of them — pays one walk over its declarations
/// and no name resolution.
///
/// `local require = require` is not a rebinding: the local holds the
/// builtin, so the name means what it meant (a common habit, to make the
/// global a local). Such a local, and one initialized from one, stays out
/// of the set as long as the file never stores anything else in it.
///
/// Not covered: a write to the GLOBAL (`require = f`, `function require()`).
/// Such a file's calls still read as the builtin.
pub fn mark_rebound_require(module: &mut hir::Module) {
    use hir::StmtKind as K;
    let mut declares = module.bodies.iter().any(|b| b.params.iter().any(|p| p.name == "require"));
    if !declares {
        for body in &module.bodies {
            hir::visit::stmts(body, &mut |stmt| {
                declares |= match &stmt.kind {
                    K::Local { decls, .. } => decls.iter().any(|d| d.name == "require"),
                    K::LocalFunction { decl, .. } => decl.name == "require",
                    K::NumericFor { var, .. } => var.name == "require",
                    K::GenericFor { vars, .. } => vars.iter().any(|v| v.name == "require"),
                    _ => false,
                };
            });
            if declares {
                break;
            }
        }
    }
    if !declares {
        return;
    }
    let names = crate::scope::resolve(module);
    // The locals that are the builtin under its own name: declared as
    // `local require = require` where the right-hand name is the builtin
    // (the global, or another such local), and written nowhere else. To a
    // fixed point, since one can be initialized from another that a later
    // body declares.
    let mut builtin: std::collections::HashSet<crate::scope::Binding> = std::collections::HashSet::new();
    loop {
        let before = builtin.len();
        for body in &module.bodies {
            hir::visit::stmts(body, &mut |stmt| {
                let K::Local { decls, init } = &stmt.kind else { return };
                for (decl, value) in decls.iter().zip(init) {
                    let hir::Expr::Name { name, span } = value else { continue };
                    if decl.name != "require" || name != "require" {
                        continue;
                    }
                    let from_builtin = !names.is_local(span) || builtin.contains(&names.binding(name, span));
                    let written_once = names.var(&decl.name, &decl.span).is_some_and(|v| v.writes.len() == 1);
                    if from_builtin && written_once {
                        builtin.insert(names.binding(&decl.name, &decl.span));
                    }
                }
            });
        }
        if builtin.len() == before {
            break;
        }
    }
    let mut rebound = std::collections::HashSet::new();
    for body in &module.bodies {
        hir::visit::exprs(body, &mut |e| {
            if let hir::Expr::Name { name, span } = e {
                if name == "require" && names.is_local(span) && !builtin.contains(&names.binding(name, span)) {
                    rebound.insert(span.start);
                }
            }
        });
    }
    module.rebound_require = rebound;
}

/// Top-level `local X = require(<target>)` bindings.
pub(crate) fn require_bindings(module: &hir::Module) -> Vec<(String, &hir::Expr)> {
    let root = module.body(module.root);
    let mut out = Vec::new();
    for stmt in &root.block.stmts {
        let hir::StmtKind::Local { decls, init } = &stmt.kind else { continue };
        for (d, e) in decls.iter().zip(init) {
            if let hir::Expr::Call { base, args, .. } = e {
                if module.is_require(base) {
                    if let Some(target) = args.first() {
                        out.push((d.name.clone(), target));
                    }
                }
            }
        }
    }
    out
}
