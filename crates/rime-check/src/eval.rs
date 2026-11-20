//! Require-target evaluator.
//!
//! This module resolves a `require(...)` argument that today's cheap
//! whole-expr `Resolver::resolve` gives up on (`Resolution::Dynamic`) by
//! walking the HIR expression through a scoped, lazy `Value` domain:
//! DataModel/file instance chains (`RS.Modules.A`), `or`-fallbacks, and
//! `RunService:IsServer()`/`IsClient()` context truth. It only ever runs on
//! the `Dynamic` slow path — the existing fast path is untouched.
//!
//! The core is `Value`, scoped frames (with poisoning on rebind), the
//! `Inst`-chain walk, and `or`. Two arms cross module boundaries:
//!   - `require(e)` itself, in the plain-call arm — resolves the argument
//!     to instances, then `Resolver::resolve_loc` to a file, producing
//!     `Value::Export(file)`;
//!   - `step_value` on `Value::Export` — cross-module field lookup
//!     (`export_field`/`table_field`), chasing linker (`return
//!     require(x)`) re-exports and `return script` instances too.
//!
//! Function returns: a plain (non-`require`) call whose callee
//! `Name` resolves in scope to a same-module `Bind::Fn` walks that
//! function's own body for its return-value candidates (`call_returns` /
//! `block_returns`) — branch-aware (`truth`/`value_truth` prune dead `if`
//! arms, including `RunService:IsServer()`/`IsClient()` ctx truth and
//! `x == nil`/`x ~= nil` guards), with a cycle guard (`Cx::visited`) for
//! recursion and loops poisoned to `Unknown` rather than modeled.
//!
//! Never guess: any shape not modeled here degrades to `Value::Unknown`,
//! never to a wrong path.
//!
//! `export_root` is also called from `rules::calls::module_interface`
//! (so the evaluator and the member-existence extractor never disagree on
//! "what does this module export"). The real entry point
//! (`eval_require_target`) is called from `check_file`'s
//! own require-resolution loop and `rules::requires::check` — both re-ask
//! the evaluator on the SAME site whenever cheap `Resolver::resolve` gives
//! up with `Resolution::Dynamic`, so they always agree on its outcome.

use crate::project::{Ctx, Loc, ModuleMap, Resolution, Resolver};
use rime_hir as hir;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// A require-target candidate mid-evaluation. Candidate sets are
/// `Vec<Value>` throughout, deduped by equality and capped at 8 — see
/// `dedup_cap`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    /// A DataModel/file instance location.
    Inst(Loc),
    /// The exported value of `require(<PathBuf>)`.
    Export(PathBuf),
    /// Provably nil on a live path; `why` renders in the R0502 note
    /// ("locateModules can return nil at <file>:<line>").
    Nil { why: String },
    /// `RunService:IsServer()` / `IsClient()` truth. Never a require target
    /// on its own; if a `Bool` ever reaches candidate-set collapse,
    /// treat it as `Unknown` there — not this module's concern yet.
    Bool(bool),
    /// Not modeled, or ambiguous — the poison value. Never guess.
    Unknown,
}

/// Walks HIR expressions to `Value` candidate sets. Read-only over the
/// resolver and other modules' resident HIR; safe to share across the
/// parallel per-file pass (both are `Sync`).
pub(crate) struct Evaluator<'a> {
    resolver: &'a dyn Resolver,
    /// Cross-module HIR/source access, for the `Export` field lookup.
    modules: &'a dyn ModuleMap,
    /// Files whose HIR/source this evaluator read — drained into
    /// `FileOut.eval_deps`.
    deps: RefCell<HashSet<PathBuf>>,
    /// CPU spent inside `eval_require_target`, summed over every call on
    /// this evaluator; `check_file` folds it into `--timings` as the
    /// `eval (nested)` CPU phase. `None` — the default — means the run is
    /// untimed and the clock reads are skipped entirely, as at every other
    /// instrumentation site (timings.rs module doc). `Cell`, not `RefCell`:
    /// an evaluator is built per `check_file` call and never crosses a
    /// thread, and `Duration` is `Copy`.
    spent: Cell<Option<Duration>>,
}

/// The collapse of a require site's per-ctx `Value` candidate sets into a
/// diagnostic-ready outcome. `collapse` is the only producer.
#[derive(Debug)]
pub(crate) enum Outcome {
    /// Fully resolved through resolve_loc — Found/Missing/OutsideView/Trusted.
    Resolved(Resolution),
    /// A live branch can hand require a nil → R0502 + note.
    NilCapable { why: String },
    /// Every candidate resolves but the files' sources differ → R0502 + note.
    Diverged { files: Vec<String> },
    /// At least one Unknown → plain R0502, exactly today's message.
    Opaque,
}

/// What a module's own `return` statement names as its export — `return T`
/// / `return table.freeze(T)` / `return script` / anything else. Extracted
/// from `rules::calls::module_interface`'s original match so the
/// evaluator's cross-module field lookup and the member-existence
/// extractor share one interpretation; `module_interface` now calls this
/// instead of duplicating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExportRoot {
    /// `return T` / `return table.freeze(T)` — a named local table.
    Table(String),
    /// `return script` — the module's own instance; members are child
    /// instances, not statically enumerable from the HIR.
    Instance,
    /// No return, a bare literal, or an unnamed/unrecognized call.
    None,
}

/// See `ExportRoot`.
pub(crate) fn export_root(module: &hir::Module) -> ExportRoot {
    let root = module.body(module.root);
    let Some(hir::Terminator::Return { exprs, .. }) = &root.block.terminator else {
        return ExportRoot::None;
    };
    let exported = match exprs.first() {
        Some(hir::Expr::Name { name, .. }) => name.clone(),
        Some(e) => match crate::rules::calls::freeze_wrapped_name(e) {
            Some(n) => n.to_string(),
            None => return ExportRoot::None,
        },
        None => return ExportRoot::None,
    };
    if exported == "script" {
        return ExportRoot::Instance;
    }
    ExportRoot::Table(exported)
}

impl<'a> Evaluator<'a> {
    pub(crate) fn new(resolver: &'a dyn Resolver, modules: &'a dyn ModuleMap) -> Self {
        Self { resolver, modules, deps: RefCell::new(HashSet::new()), spent: Cell::new(None) }
    }

    /// Arm (`on`) or leave off the `eval_require_target` stopwatch — see
    /// `spent`. Off is the default, so only `--timings` runs pay for it.
    pub(crate) fn with_timing(self, on: bool) -> Self {
        if on {
            self.spent.set(Some(Duration::ZERO));
        }
        self
    }

    /// Accumulated `eval_require_target` CPU, or `None` when untimed.
    /// A timed evaluator that never had to evaluate anything answers
    /// `Some(ZERO)`, so the phase's file count matches the other per-file
    /// phases'.
    pub(crate) fn spent(&self) -> Option<Duration> {
        self.spent.get()
    }

    pub(crate) fn take_deps(&self) -> HashSet<PathBuf> {
        self.deps.borrow_mut().drain().collect()
    }

    /// The one entry: a require target that cheap `Resolver::resolve` gave
    /// up on (`Resolution::Dynamic`). `from` is the requiring file; `module`
    /// is that file's own HIR, for its top-level frame; `target` is the
    /// require argument, unevaluated. Runs once per bit of `from`'s
    /// `context_of` (a shared module runs once per ctx it can
    /// execute in) and collapses the per-ctx candidate sets through
    /// `collapse`.
    pub(crate) fn eval_require_target(
        &self,
        from: &Path,
        module: &hir::Module,
        target: &hir::Expr,
    ) -> Outcome {
        let start = self.spent.get().map(|_| Instant::now());
        let mut per_ctx = Vec::new();
        for ctx in self.resolver.context_of(from).bits() {
            let mut cx = Cx { ctx, depth: 0, visited: HashSet::new() };
            // The site's own lexical position is not modeled — a require
            // can sit in any body or nested block, and this is the ROOT
            // frame — so anything a nearer scope could shadow is poisoned
            // here (`frame_of_unpositioned`).
            let scope = Scope {
                file: from,
                module,
                frames: vec![frame_of_unpositioned(&module.body(module.root).block, module)],
            };
            per_ctx.push((ctx, self.eval(&mut cx, &scope, target)));
        }
        let out = self.collapse(per_ctx);
        if let (Some(sofar), Some(start)) = (self.spent.get(), start) {
            self.spent.set(Some(sofar + start.elapsed()));
        }
        out
    }

    /// The inner eval, exposed for unit tests; `eval_require_target`
    /// above is the real entry. Depth-capped at 16.
    fn eval(&self, cx: &mut Cx, scope: &Scope<'_>, e: &hir::Expr) -> Vec<Value> {
        if cx.depth > 16 {
            return vec![Value::Unknown];
        }
        cx.depth += 1;
        let out = self.eval_inner(cx, scope, e);
        cx.depth -= 1;
        out
    }

    fn eval_inner(&self, cx: &mut Cx, scope: &Scope<'_>, e: &hir::Expr) -> Vec<Value> {
        use hir::Expr as E;
        match e {
            // An explicit `nil` literal — reached through `return nil`, an
            // alias's initializer, etc. (`block_returns`'s
            // terminator match hands a `return`'s first expr straight to
            // `eval`, whether it's `nil` or anything else — only a BARE
            // `return` with no expr at all skips this, going through
            // `nil_why` directly instead).
            E::Literal { prim: hir::Prim::Nil, span, .. } => {
                vec![Value::Nil { why: self.nil_why(scope, span) }]
            }
            E::Name { name, .. } => {
                if name == "game" {
                    return vec![Value::Inst(Loc::Dm(Vec::new()))];
                }
                if name == "script" {
                    return match self.resolver.instance_of(scope.file) {
                        Some(l) => vec![Value::Inst(l)],
                        None => vec![Value::Unknown],
                    };
                }
                for (i, fr) in scope.frames.iter().enumerate().rev() {
                    match fr.0.get(name.as_str()) {
                        // The initializer is an expression at the BINDING's
                        // site, not at this reference's — so it evaluates
                        // against the frames that were in scope THERE
                        // (`truncated(i)`), never against the reader's own
                        // deeper stack. Evaluating it in the current stack
                        // is dynamic scoping: a callee that rebinds a name
                        // the initializer mentions would silently retarget
                        // an outer binding's value.
                        Some(Bind::Init(init)) => {
                            return self.eval(cx, &scope.truncated(i), init)
                        }
                        Some(Bind::Fn(_)) | Some(Bind::Poison) => return vec![Value::Unknown],
                        None => {}
                    }
                }
                vec![Value::Unknown] // global / not modeled
            }
            E::Index { base, key, name, .. } => {
                let seg = match (name, key) {
                    (Some(n), _) => n.clone(),
                    (None, Some(k)) => match literal_text(k) {
                        Some(t) => t,
                        None => return vec![Value::Unknown],
                    },
                    _ => return vec![Value::Unknown],
                };
                let vs = self.eval(cx, scope, base);
                dedup_cap(vs.into_iter().flat_map(|v| self.step_value(cx, v, &seg)).collect())
            }
            E::Call { base, args, method, .. } => match method.as_deref() {
                // `game:GetService("X")` — base must eval to Inst(Dm([])),
                // the DataModel root; only game has services.
                Some("GetService") => {
                    let Some(name) = args.first().and_then(literal_text) else {
                        return vec![Value::Unknown];
                    };
                    let vs = self.eval(cx, scope, base);
                    dedup_cap(
                        vs.into_iter()
                            .map(|v| match v {
                                Value::Inst(Loc::Dm(p)) if p.is_empty() => {
                                    match self.resolver.service(&name) {
                                        Some(loc) => Value::Inst(loc),
                                        None => Value::Unknown,
                                    }
                                }
                                _ => Value::Unknown,
                            })
                            .collect(),
                    )
                }
                Some("WaitForChild") | Some("FindFirstChild") => {
                    let Some(seg) = args.first().and_then(literal_text) else {
                        return vec![Value::Unknown];
                    };
                    let vs = self.eval(cx, scope, base);
                    dedup_cap(vs.into_iter().flat_map(|v| self.step_value(cx, v, &seg)).collect())
                }
                // A clone is a fresh instance of the SAME source (a
                // separate require-cache entry) — the edge is statically
                // knowable, identity is not resolution's concern.
                Some("Clone") if args.is_empty() => self.eval(cx, scope, base),
                Some("IsServer") if self.is_run_service(cx, scope, base) => {
                    vec![Value::Bool(cx.ctx == Ctx::Server)]
                }
                Some("IsClient") if self.is_run_service(cx, scope, base) => {
                    vec![Value::Bool(cx.ctx == Ctx::Client)]
                }
                None => match &**base {
                    // `require(e)`: eval the argument, turn every resolved
                    // instance into the module it names. Anything that
                    // doesn't resolve to a real file (Missing/OutsideView/
                    // Dynamic/Trusted, or a candidate that wasn't even an
                    // instance) is Unknown — never guess a target.
                    callee if scope.module.is_require(callee) && args.len() == 1 => {
                        let vs = self.eval(cx, scope, &args[0]);
                        dedup_cap(
                            vs.into_iter()
                                .map(|v| match v {
                                    Value::Inst(loc) => match self.resolver.resolve_loc(&loc) {
                                        Resolution::Found(f) => Value::Export(f),
                                        _ => Value::Unknown,
                                    },
                                    Value::Export(_)
                                    | Value::Nil { .. }
                                    | Value::Bool(_)
                                    | Value::Unknown => Value::Unknown,
                                })
                                .collect(),
                        )
                    }
                    // Plain call `f(...)`: a callee `Name` resolving to a
                    // same-module `Bind::Fn` gets its return-value
                    // candidates walked (`call_returns`); anything
                    // else (a global, a table-field call, a param/rebound
                    // name) is not modeled.
                    // The callee's body is lexically inside the frame that
                    // BOUND it, so it is walked under `truncated(i)` — the
                    // same binding-site rule `Bind::Init` follows above.
                    hir::Expr::Name { name, .. } => match self.lookup_fn(scope, name) {
                        Some((body_id, i)) => {
                            self.call_returns(cx, &scope.truncated(i), body_id)
                        }
                        None => vec![Value::Unknown],
                    },
                    _ => vec![Value::Unknown],
                },
                _ => vec![Value::Unknown],
            },
            E::BinOp { op: hir::BinOp::Or, lhs, rhs } => {
                let l = self.eval(cx, scope, lhs);
                let mut out = Vec::new();
                for v in l {
                    match self.non_nil(&v) {
                        Some(true) => out.push(v),
                        Some(false) => out.extend(self.eval(cx, scope, rhs)),
                        None => out.push(Value::Unknown),
                    }
                }
                dedup_cap(out)
            }
            _ => vec![Value::Unknown],
        }
    }

    /// One instance/field step on a value: `Inst` steps through the
    /// resolver (`.Name`, `:FindFirstChild`/`:WaitForChild`, `Parent`);
    /// `Export` field lookup chases into the exporting module's own HIR
    /// (`export_field`) — table-literal / `.field =` / `self.field =`
    /// writes, `return script` children, and linker (`return require(x)`)
    /// re-export chains; nothing else can be stepped.
    fn step_value(&self, cx: &mut Cx, v: Value, seg: &str) -> Vec<Value> {
        match v {
            Value::Inst(l) => match self.resolver.step(&l, seg) {
                Some(loc) => vec![Value::Inst(loc)],
                None => vec![Value::Unknown],
            },
            Value::Export(f) => self.export_field(cx, &f, seg),
            Value::Nil { .. } | Value::Unknown | Value::Bool(_) => vec![Value::Unknown],
        }
    }

    /// Field lookup on a required module's export: depth-capped the same
    /// way `eval` is (each hop through the linker chase must respect
    /// `cx.depth`) — the real work is `export_field_inner`.
    fn export_field(&self, cx: &mut Cx, file: &Path, field: &str) -> Vec<Value> {
        if cx.depth > 16 {
            return vec![Value::Unknown];
        }
        cx.depth += 1;
        let out = self.export_field_inner(cx, file, field);
        cx.depth -= 1;
        out
    }

    fn export_field_inner(&self, cx: &mut Cx, file: &Path, field: &str) -> Vec<Value> {
        // Recorded BEFORE the HIR lookup, not after: a file with no HIR
        // right now (a parse error mid-edit, say) is still read here — if
        // it later parses again, that outcome can change, so the
        // dependency edge must exist even on this early-return path. Bug
        // fix (corpus case 090's incremental-parity test):
        // recording it only after a successful `hir()` lookup dropped the
        // edge entirely whenever the dependency was mid-broken, so a
        // session step that fixed it back up never re-ran the consumer.
        self.deps.borrow_mut().insert(file.to_path_buf());
        let Some(module) = self.modules.hir(file) else {
            return vec![Value::Unknown];
        };

        // Linker stub: the module's WHOLE export is `return require(x)` —
        // chase to the terminal module and re-ask there, rather than
        // treating `x` itself as an exported value.
        if let Some(t) = crate::project::linker_target(module) {
            let scope = Scope {
                file,
                module,
                frames: vec![frame_of(&module.body(module.root).block, module)],
            };
            let mut out = Vec::new();
            for v in self.eval(cx, &scope, t) {
                match v {
                    Value::Inst(loc) => match self.resolver.resolve_loc(&loc) {
                        Resolution::Found(f2) => out.extend(self.export_field(cx, &f2, field)),
                        _ => out.push(Value::Unknown),
                    },
                    Value::Export(f2) => out.extend(self.export_field(cx, &f2, field)),
                    Value::Nil { .. } | Value::Bool(_) | Value::Unknown => {
                        out.push(Value::Unknown)
                    }
                }
            }
            return dedup_cap(out);
        }

        match export_root(module) {
            // `return script`: fields are child instances, navigated
            // through the resolver — never statically enumerable from HIR
            // alone.
            ExportRoot::Instance => {
                match self.resolver.instance_of(file).and_then(|l| self.resolver.step(&l, field)) {
                    Some(l) => vec![Value::Inst(l)],
                    None => vec![Value::Unknown],
                }
            }
            ExportRoot::Table(t) => self.table_field(cx, file, module, &t, field),
            ExportRoot::None => vec![Value::Unknown],
        }
    }

    /// One field's value on the exported table `t` of `module` (file
    /// `file`) — the three modeled write forms: a table-literal
    /// entry (`local T = { field = e }`), a top-level `T.field = e`, or a
    /// `self.field = e` inside one of `T`'s own methods
    /// (`function T:m()` / `function T.m(self)`). Every match is a
    /// candidate write; never guess: zero writes, or more than one whose
    /// RHS structurally differs (`project::expr_eq`), degrades to
    /// `Unknown`. The one surviving write's RHS evaluates in the DEFINING
    /// module's own scope — the top-level frame, plus (form 3 only) that
    /// method body's frame, since its locals may feed the RHS — never the
    /// requiring file's scope.
    fn table_field(
        &self,
        cx: &mut Cx,
        file: &Path,
        module: &hir::Module,
        t: &str,
        field: &str,
    ) -> Vec<Value> {
        /// One candidate write to `t.field`: its RHS, and — for a form-3
        /// `self.field = e` write — the method body whose params/locals the
        /// RHS may reference.
        struct Write<'m> {
            rhs: &'m hir::Expr,
            method_body: Option<hir::BodyId>,
        }

        // A rebound export root — a second `local T`, a bare `T = ...`
        // write anywhere, or a multi-assign naming `T` as a target — means
        // the table the module RETURNS may not be the one the recorded
        // writes touched: every write below is suspect, whatever its
        // shape. Never guess.
        if export_root_rebound(module, t) {
            return vec![Value::Unknown];
        }

        let root = module.body(module.root);
        let mut writes: Vec<Write> = Vec::new();
        // A write to `t.field` whose value this evaluator cannot pair with
        // its target: a multi-assign (`x, T.field = f()`) whose target and
        // value lists don't line up 1:1. The positional zip below would
        // DROP such a target silently, leaving the write invisible and some
        // other write looking like the module's only one — a value runtime
        // overwrites. Never guess: one of
        // these degrades the whole lookup.
        let mut unpaired = false;

        // Form 1: table-literal entry, top-level `local T = { field = e }`
        // only — not a nested/reassigned `T`.
        for stmt in &root.block.stmts {
            let hir::StmtKind::Local { decls, init } = &stmt.kind else { continue };
            for (d, e) in decls.iter().zip(init) {
                if d.name != t {
                    continue;
                }
                if let hir::Expr::Table(fields) = e {
                    for value in named_field_values(fields, field) {
                        writes.push(Write { rhs: value, method_body: None });
                    }
                }
            }
        }

        // Form 2: `T.field = e`, in any body. `stmts_lifetime`, not
        // `hir::visit::stmts` — the RHS `&Expr` pushed into `writes` must
        // outlive this closure call (`hir::visit`'s generic `impl
        // FnMut(&Stmt)` signature is a fresh higher-ranked lifetime per
        // call, too short for that; see `stmts_lifetime`'s doc).
        for body in &module.bodies {
            stmts_lifetime(&body.block, &mut |stmt| {
                let hir::StmtKind::Assign { targets, values } = &stmt.kind else { return };
                if targets.len() != values.len() {
                    unpaired |= targets.iter().any(|x| field_write_base(x, field) == Some(t));
                    return;
                }
                for (target, value) in targets.iter().zip(values) {
                    if field_write_base(target, field) == Some(t) {
                        writes.push(Write { rhs: value, method_body: None });
                    }
                }
            });
        }

        // Form 3: `self.field = e` inside one of `T`'s own methods. Finding
        // the method bodies only extracts owned `BodyId`s (no reference
        // escapes), so `hir::visit::stmts` is fine here; walking each one
        // for its `self.field = e` writes needs `stmts_lifetime` again.
        let mut method_bodies: Vec<hir::BodyId> = Vec::new();
        for body in &module.bodies {
            hir::visit::stmts(body, &mut |stmt| {
                if let hir::StmtKind::FunctionDecl { root: r, body: body_id, .. } = &stmt.kind {
                    if r == t {
                        method_bodies.push(*body_id);
                    }
                }
            });
        }
        for body_id in method_bodies {
            let mbody = module.body(body_id);
            stmts_lifetime(&mbody.block, &mut |stmt| {
                let hir::StmtKind::Assign { targets, values } = &stmt.kind else { return };
                if targets.len() != values.len() {
                    unpaired |= targets.iter().any(|x| field_write_base(x, field) == Some("self"));
                    return;
                }
                for (target, value) in targets.iter().zip(values) {
                    if field_write_base(target, field) == Some("self") {
                        writes.push(Write { rhs: value, method_body: Some(body_id) });
                    }
                }
            });
        }

        // Never guess: no write, writes that disagree, and a write whose
        // value could not be paired with its target all degrade.
        if unpaired {
            return vec![Value::Unknown];
        }
        let mut distinct: Vec<&Write> = Vec::new();
        for w in &writes {
            if !distinct.iter().any(|d| crate::project::expr_eq(d.rhs, w.rhs)) {
                distinct.push(w);
            }
        }
        if distinct.len() != 1 {
            return vec![Value::Unknown];
        }
        let w = distinct[0];

        // Neither frame models the WRITE's own lexical position (it was
        // matched anywhere in the module / anywhere in the method), so both
        // are built the conservative way — see `frame_of_unpositioned`.
        let mut frames = vec![frame_of_unpositioned(&root.block, module)];
        if let Some(bid) = w.method_body {
            frames.push(body_frame_unpositioned(module.body(bid), module));
        }
        let scope = Scope { file, module, frames };
        self.eval(cx, &scope, w.rhs)
    }

    /// Whether `base` evals (recursively, cheap — reuses `eval`) to the
    /// `RunService` instance. Guards `IsServer`/`IsClient` so only calls on
    /// the actual RunService produce a context truth value.
    fn is_run_service(&self, cx: &mut Cx, scope: &Scope<'_>, base: &hir::Expr) -> bool {
        self.eval(cx, scope, base)
            .iter()
            .any(|v| matches!(v, Value::Inst(Loc::Dm(p)) if p.len() == 1 && p[0] == "RunService"))
    }

    /// The `Bind::Fn` body a plain call's callee `Name` resolves to, in
    /// scope — same shadowing rule the `Name` value-eval arm uses (the
    /// walk stops at the FIRST binding found, innermost frame first): a
    /// nearer `Init`/`Poison` binding of the same name shadows a `Fn` bound
    /// further out, and the search stops there rather than falling through
    /// to it. `None` for a global name, or a name bound to anything other
    /// than a function. The frame INDEX comes back with it: the body is
    /// lexically inside that frame's block, so the caller walks it under
    /// `Scope::truncated(i)`, not under its own (possibly deeper) stack.
    fn lookup_fn(&self, scope: &Scope<'_>, name: &str) -> Option<(hir::BodyId, usize)> {
        for (i, fr) in scope.frames.iter().enumerate().rev() {
            match fr.0.get(name) {
                Some(Bind::Fn(body_id)) => return Some((*body_id, i)),
                Some(Bind::Init(_)) | Some(Bind::Poison) => return None,
                None => {}
            }
        }
        None
    }

    /// A plain call's return-value candidates: the callee
    /// body's own `block_returns` walk, plus an implicit `Nil` if control
    /// can fall out the end of the function. Same-module only — the caller
    /// resolves `body_id` from its own scope, never across a `require`
    /// boundary. Cycle guard: `cx.visited` records (file, body) pairs
    /// currently being walked; hitting one already on the stack — direct
    /// or mutual recursion — degrades to `Unknown` rather than looping
    /// forever.
    fn call_returns<'m>(
        &self,
        cx: &mut Cx,
        scope: &Scope<'m>,
        body_id: hir::BodyId,
    ) -> Vec<Value> {
        let key = (scope.file.to_path_buf(), body_id);
        if !cx.visited.insert(key.clone()) {
            return vec![Value::Unknown];
        }
        let body = scope.module.body(body_id);
        // Callee scope: the caller's own frame chain, so a top-level local
        // read as an upvalue from inside the callee still resolves, with
        // the callee body's own params (Poison, v1 — caller backtracking
        // plugs in here later) and locals layered on top via `body_frame`.
        let mut frames = scope.frames.clone();
        frames.push(body_frame(body, scope.module));
        let inner = Scope { file: scope.file, module: scope.module, frames };
        let (mut vals, falls_through) = self.block_returns(cx, &inner, &body.block);
        if falls_through {
            vals.push(Value::Nil {
                why: format!(
                    "implicit nil return at end of function ({})",
                    display_file(scope.file)
                ),
            });
        }
        cx.visited.remove(&key);
        dedup_cap(vals)
    }

    /// The branch-aware walk over one block's statements:
    /// returns the candidate return VALUES reachable from `block`, and
    /// whether control can also fall out the end of it (past the last
    /// statement, with no `return` in the way). An `If`'s live arms are
    /// governed by `truth`: a provably-false arm is dead (skipped); a
    /// provably-true arm is the ONLY one that runs — later arms (and any
    /// `else`) are dead too, and if that arm doesn't fall through, neither
    /// does the whole `If` (return early). Symmetrically, if EVERY explicit
    /// arm is provably false, `or_else` (when present) is the only branch
    /// that can run, and its own fallthrough decides the `If`'s the same
    /// way. An arm whose truth is unproven contributes its candidates as a
    /// "maybe" and does not, on its own, decide anything. A loop is not
    /// modeled: `Unknown` if it contains a `return` anywhere inside
    /// (`contains_return`), nothing otherwise — never treated as
    /// definitely returning or definitely not.
    fn block_returns<'m>(
        &self,
        cx: &mut Cx,
        scope: &Scope<'m>,
        block: &'m hir::Block,
    ) -> (Vec<Value>, bool) {
        let mut vals = Vec::new();
        for stmt in &block.stmts {
            match &stmt.kind {
                hir::StmtKind::If { cond, then, else_ifs, or_else } => {
                    // Whether a truth-true arm (or a certain `or_else`,
                    // below) already consumed control for this `If`.
                    let mut decided = false;
                    // Whether every arm checked so far is provably false —
                    // while this holds, `or_else` (if reached) is the ONLY
                    // branch that can run.
                    let mut all_dead = true;
                    let mut arms: Vec<(&hir::Expr, &hir::Block)> = vec![(cond, then)];
                    arms.extend(else_ifs.iter().map(|(c, b)| (c, b)));
                    for (c, b) in arms {
                        match self.truth(cx, scope, c) {
                            Some(false) => continue, // arm dead
                            Some(true) => {
                                let (v, ft) = self.block_returns(cx, &self.nested(scope, b), b);
                                vals.extend(v);
                                if !ft {
                                    return (dedup_cap(vals), false);
                                }
                                decided = true;
                                break; // later arms dead
                            }
                            None => {
                                // Maybe taken: contributes candidates,
                                // doesn't decide anything on its own.
                                let (v, _ft) = self.block_returns(cx, &self.nested(scope, b), b);
                                vals.extend(v);
                                all_dead = false;
                            }
                        }
                    }
                    if !decided {
                        if let Some(e) = or_else {
                            let (v, ft) = self.block_returns(cx, &self.nested(scope, e), e);
                            vals.extend(v);
                            if all_dead && !ft {
                                return (dedup_cap(vals), false);
                            }
                        }
                    }
                    // Otherwise control continues past the `If`.
                }
                hir::StmtKind::Do(b) => {
                    let (v, ft) = self.block_returns(cx, &self.nested(scope, b), b);
                    vals.extend(v);
                    if !ft {
                        return (dedup_cap(vals), false);
                    }
                }
                hir::StmtKind::While { body, .. }
                | hir::StmtKind::Repeat { body, .. }
                | hir::StmtKind::NumericFor { body, .. }
                | hir::StmtKind::GenericFor { body, .. } => {
                    // Loops are not modeled: a return anywhere inside
                    // poisons; no return, nothing contributed.
                    if contains_return(body) {
                        vals.push(Value::Unknown);
                    }
                }
                _ => {}
            }
        }
        match &block.terminator {
            Some(hir::Terminator::Return { exprs, span }) => {
                match exprs.first() {
                    // Multiple return values: only the first expr matters
                    // (a require target is single-valued).
                    None => vals.push(Value::Nil { why: self.nil_why(scope, span) }),
                    Some(e) => vals.extend(self.eval(cx, scope, e)),
                }
                (dedup_cap(vals), false)
            }
            _ => (dedup_cap(vals), true),
        }
    }

    /// The scope INSIDE a nested block: the enclosing frame stack plus that
    /// block's own `local`s. A Lua block is a lexical scope, and `frame_of`
    /// deliberately only scans a block's own statements — so without this
    /// push, a name declared inside an `if` arm is unbound everywhere in
    /// that arm and evaluates to `Unknown`. A package that finds its other
    /// half at run time has exactly that shape:
    ///
    /// ```lua
    /// if onServer then
    ///     local half = serverHalf()
    ///     if not half then return nil end           -- must prune
    ///     return half.Modules
    /// end
    /// ```
    ///
    /// The arm's CONDITION still evaluates in the enclosing scope (it is
    /// outside the block) — only statements within the block see this frame.
    fn nested<'m>(&self, scope: &Scope<'m>, block: &'m hir::Block) -> Scope<'m> {
        let mut frames = scope.frames.clone();
        frames.push(frame_of(block, scope.module));
        Scope { file: scope.file, module: scope.module, frames }
    }

    /// Some(true/false) when `cond` provably decides; `None` = unknown.
    /// `x == nil` / `x ~= nil` (nil literal on either side) read as the
    /// nil-check they are, through `value_is_nil` — nil-ness, not inverted
    /// truthiness; anything else (a bare `if x` / `if
    /// RunService:IsServer()`) falls to `value_truth`.
    fn truth(&self, cx: &mut Cx, scope: &Scope<'_>, cond: &hir::Expr) -> Option<bool> {
        use hir::{BinOp, Expr as E};
        match cond {
            E::Not(e) => self.truth(cx, scope, e).map(|b| !b),
            E::BinOp { op: BinOp::Eq, lhs, rhs } if is_nil_literal(rhs) => {
                self.value_is_nil(cx, scope, lhs)
            }
            E::BinOp { op: BinOp::Eq, lhs, rhs } if is_nil_literal(lhs) => {
                self.value_is_nil(cx, scope, rhs)
            }
            E::BinOp { op: BinOp::Ne, lhs, rhs } if is_nil_literal(rhs) => {
                self.value_is_nil(cx, scope, lhs).map(|b| !b)
            }
            E::BinOp { op: BinOp::Ne, lhs, rhs } if is_nil_literal(lhs) => {
                self.value_is_nil(cx, scope, rhs).map(|b| !b)
            }
            _ => self.value_truth(cx, scope, cond),
        }
    }

    /// Truthiness of a value expression: provable only for a SINGLE
    /// candidate with a proof — `Bool` reads its own truth directly (a
    /// `RunService:IsServer()`/`IsClient()` ctx check), anything else goes
    /// through `non_nil` (a bare `if x` is truthy iff `x` is non-nil, which
    /// is not quite Lua's real truthiness — `false` isn't nil — but no
    /// modeled `Value` is ever the boolean `false` outside `Bool` itself,
    /// so the two coincide here). More than one candidate, or a candidate
    /// without a proof, is unprovable.
    fn value_truth(&self, cx: &mut Cx, scope: &Scope<'_>, e: &hir::Expr) -> Option<bool> {
        let vals = self.eval(cx, scope, e);
        let [v] = vals.as_slice() else { return None };
        match v {
            Value::Bool(b) => Some(*b),
            _ => self.non_nil(v),
        }
    }

    /// Whether an expression is provably nil — what `x == nil` / `x ~= nil`
    /// actually ask, which is NOT the negation of truthiness: `false ~= nil`
    /// in Lua, so a `Bool` answers "not nil" whichever way it points.
    /// Routing these arms through `value_truth` read `onServer == nil`
    /// under the client context (where `onServer` is `Bool(false)`) as
    /// PROVABLY TRUE and pruned the wrong branch.
    /// Same single-candidate-with-a-proof rule as
    /// `value_truth`; everything else is `non_nil`, negated.
    fn value_is_nil(&self, cx: &mut Cx, scope: &Scope<'_>, e: &hir::Expr) -> Option<bool> {
        let vals = self.eval(cx, scope, e);
        let [v] = vals.as_slice() else { return None };
        match v {
            Value::Bool(_) => Some(false),
            _ => self.non_nil(v).map(|nn| !nn),
        }
    }

    /// A bare `return`'s nil-why note: `"<file>:<line> returns nil"`, the
    /// line rendered from `scope.module`'s own source the same
    /// newline-count way `requires.rs::line_of` does. Falls back to just
    /// the file when the source isn't resident (defensive only — every
    /// scope this evaluator builds walks a module it just read the source
    /// of, to lower it to HIR in the first place). `<file>` is the bare
    /// filename (`display_file`), not the full path — see there.
    fn nil_why(&self, scope: &Scope<'_>, span: &hir::Span) -> String {
        let file = display_file(scope.file);
        match self.modules.source(scope.file) {
            Some(src) => format!("{file}:{} returns nil", line_of(src, span.start)),
            None => format!("{file} returns nil"),
        }
    }

    /// Non-nil proof for a value. `Inst` counts as PROVEN present only
    /// when it RESOLVES (`FindFirstChild` that resolves statically
    /// = non-nil; unresolved is Unknown-presence, never nil — a runtime-
    /// parented instance may still exist).
    ///
    /// "Resolves" is two questions, not one: `resolve_loc` answers "is this
    /// a module file", `Resolver::present` answers "does the project create
    /// this instance at all". A folder (`ServerStorage.ServerPackages`)
    /// only passes the second — and a `FindFirstChild` for a folder is
    /// exactly as non-nil as one for a module. Neither
    /// proof means unproven, never nil.
    fn non_nil(&self, v: &Value) -> Option<bool> {
        match v {
            Value::Inst(loc) => match self.resolver.resolve_loc(loc) {
                Resolution::Found(_) => Some(true),
                _ if self.resolver.present(loc) => Some(true),
                _ => None,
            },
            Value::Export(_) => Some(true), // a successful require is non-nil
            Value::Nil { .. } => Some(false),
            Value::Bool(b) => Some(*b),
            Value::Unknown => None,
        }
    }

    /// Collapses a require site's per-ctx candidate `Value` sets into an
    /// `Outcome`, in this order:
    ///
    ///   1. any `Unknown` anywhere → `Opaque`. A `Bool` reaching here is
    ///      never a require target (see `Value::Bool`) — same
    ///      treatment as `Unknown`.
    ///   2. else any `Nil` anywhere → `NilCapable`, first `why` (input
    ///      order: ctx entries as given, values within each in order) wins.
    ///   3. every remaining candidate is `Inst`/`Export` — map each through
    ///      `resolve_loc` (`Export(f)` is already a resolved file, so it
    ///      goes straight to `Found(f)`, skipping `resolve_loc`). Any
    ///      result that is not `Found` (`Missing`/`OutsideView`, and
    ///      `Trusted`/`Dynamic` the same way, since `Outcome::Resolved`'s
    ///      own doc lists `Trusted` as something it wraps) wins
    ///      immediately, first in input order.
    ///   4. one distinct `Found` file → `Resolved(Found(_))`.
    ///   5. several, but every file's `ModuleMap::source` agrees
    ///      byte-for-byte → `Resolved(Found(canonical))`; canonical = the
    ///      first `Found` whose owning ctx entry is `Ctx::Server`, else the
    ///      first `Found` overall (both input order).
    ///   6. otherwise (sources differ, or a source is unavailable) →
    ///      `Diverged`, files listed ctx-order — server entries before
    ///      client (arbitrary but deterministic; chosen over a path sort
    ///      since candidate order already carries it) — with their common
    ///      path prefix stripped for display.
    pub(crate) fn collapse(&self, per_ctx: Vec<(Ctx, Vec<Value>)>) -> Outcome {
        // 1: any Unknown/Bool anywhere -> Opaque.
        let any_opaque = per_ctx
            .iter()
            .any(|(_, vs)| vs.iter().any(|v| matches!(v, Value::Unknown | Value::Bool(_))));
        if any_opaque {
            return Outcome::Opaque;
        }

        // 2: first Nil (input order) -> NilCapable.
        for (_, vs) in &per_ctx {
            for v in vs {
                if let Value::Nil { why } = v {
                    return Outcome::NilCapable { why: why.clone() };
                }
            }
        }

        // 3: only Inst/Export remain now — resolve each, tagged with its
        // owning ctx for step 5's canonical pick. Any non-Found result
        // wins immediately.
        let mut resolved: Vec<(Ctx, Resolution)> = Vec::new();
        for (ctx, vs) in &per_ctx {
            for v in vs {
                let r = match v {
                    Value::Inst(loc) => self.resolver.resolve_loc(loc),
                    Value::Export(f) => Resolution::Found(f.clone()),
                    Value::Nil { .. } | Value::Unknown | Value::Bool(_) => {
                        unreachable!("ruled out by steps 1-2 above")
                    }
                };
                resolved.push((*ctx, r));
            }
        }
        for (_, r) in &resolved {
            if !matches!(r, Resolution::Found(_)) {
                return Outcome::Resolved(r.clone());
            }
        }

        // Only `Found` survives past here.
        let files: Vec<(Ctx, PathBuf)> = resolved
            .into_iter()
            .map(|(ctx, r)| match r {
                Resolution::Found(f) => (ctx, f),
                _ => unreachable!("non-Found already returned above"),
            })
            .collect();

        let mut distinct: Vec<PathBuf> = Vec::new();
        for (_, f) in &files {
            if !distinct.contains(f) {
                distinct.push(f.clone());
            }
        }

        match distinct.len() {
            // No candidates at all — defensive only; every `eval` call
            // returns at least one `Value`, so a real per_ctx never gets
            // here. Same conservative degrade as an outright Unknown.
            0 => Outcome::Opaque,
            1 => Outcome::Resolved(Resolution::Found(distinct.into_iter().next().unwrap())),
            _ => {
                // 5: several distinct files — byte-identical sources still
                // collapse to one, the Server-ctx candidate canonical.
                //
                // Bug fix (corpus case 087/088): the
                // Found-vs-Diverged answer here is a function of these
                // files' CONTENT (byte-identity), not just their existence
                // — unlike a plain `resolve_loc` existence check, which the
                // session's ordinary (non-eval) `deps` already covers. Never
                // read via `export_field_inner` (these are bare instance
                // candidates, not `require`d modules), so nothing else
                // records them; without this, editing a candidate whose
                // content the comparison actually reads leaves the
                // requiring file's stale Found/Diverged diagnostic
                // uninvalidated (`Session::eval_consumers` never learns the
                // edge).
                self.deps.borrow_mut().extend(distinct.iter().cloned());
                let sources: Vec<Option<&str>> =
                    distinct.iter().map(|f| self.modules.source(f)).collect();
                let all_same = matches!(sources.first(), Some(Some(_)))
                    && sources.iter().all(|s| *s == sources[0]);
                if all_same {
                    let canonical = files
                        .iter()
                        .find(|(ctx, _)| *ctx == Ctx::Server)
                        .or_else(|| files.first())
                        .map(|(_, f)| f.clone())
                        .expect("distinct.len() > 1 implies files non-empty");
                    return Outcome::Resolved(Resolution::Found(canonical));
                }

                // 6: sources differ (or one is unavailable) — Diverged,
                // display files in ctx-order (server first). When the
                // per-ctx candidate SETS differ (a genuine server/client
                // split — the real-world case), each file carries the ctx
                // that lands on it; when both bits carry the same union
                // (corpus 088's Unknown-cond shape), a label would claim a
                // split that does not exist, so none renders.
                let mut ordered: Vec<PathBuf> = Vec::new();
                for want in [Ctx::Server, Ctx::Client] {
                    for (ctx, f) in &files {
                        if *ctx == want && !ordered.contains(f) {
                            ordered.push(f.clone());
                        }
                    }
                }
                let ctxs_of = |f: &PathBuf| -> Vec<Ctx> {
                    [Ctx::Server, Ctx::Client]
                        .into_iter()
                        .filter(|c| files.iter().any(|(fc, ff)| fc == c && ff == f))
                        .collect()
                };
                let sets: Vec<Vec<Ctx>> = ordered.iter().map(ctxs_of).collect();
                let uniform = sets.windows(2).all(|w| w[0] == w[1]);
                let mut names = strip_common_prefix(&ordered);
                if !uniform {
                    for (name, set) in names.iter_mut().zip(&sets) {
                        let label = set
                            .iter()
                            .map(|c| match c {
                                Ctx::Server => "server",
                                Ctx::Client => "client",
                            })
                            .collect::<Vec<_>>()
                            .join("+");
                        name.push_str(&format!(" ({label})"));
                    }
                }
                Outcome::Diverged { files: names }
            }
        }
    }
}

/// One `eval` call tree's shared state: the ctx bit being evaluated (a
/// shared module runs once per bit), recursion depth (cap 16),
/// and a cycle guard for cross-module/cross-function recursion.
struct Cx {
    ctx: Ctx,
    depth: usize,
    visited: HashSet<(PathBuf, usize)>, // usize = BodyId index
}

/// One file's evaluation scope: the file being evaluated (for `script`),
/// its module (for descending into other bodies), and a stack of
/// lexical frames, innermost last.
struct Scope<'m> {
    file: &'m Path,
    module: &'m hir::Module,
    frames: Vec<Frame<'m>>,
}

impl<'m> Scope<'m> {
    /// The scope as it stood at frame `i` — everything nested INSIDE that
    /// frame dropped. A binding's initializer, and a `Bind::Fn`'s body, are
    /// code at the binding's own site: they must see the frames that were
    /// open there, not whatever the reader has since pushed on top.
    /// Frames are small name→`Bind` maps
    /// of borrowed references, so the clone is cheap.
    fn truncated(&self, i: usize) -> Scope<'m> {
        Scope { file: self.file, module: self.module, frames: self.frames[..=i].to_vec() }
    }
}

/// What a name in a frame is bound to. `Clone`/`Copy`: `call_returns`
/// clones a caller's whole frame stack into the callee's scope so
/// an upvalue read (a top-level local referenced from inside a called
/// function) resolves through it.
#[derive(Clone, Copy)]
enum Bind<'m> {
    /// A `local` with a known initializer expression.
    Init(&'m hir::Expr),
    /// A `local function` / `function f()` — not modeled as a value
    /// (a *call* of it is evaluated, not the function itself as a value).
    Fn(hir::BodyId),
    /// Unknowable: a parameter, a rebound name, or anything else this
    /// module declines to track through. Never guess past this.
    Poison,
}

/// One lexical scope's bindings, name -> `Bind`. `Clone` for the same
/// reason `Bind` is (see there).
#[derive(Clone)]
struct Frame<'m>(HashMap<&'m str, Bind<'m>>);

/// Build a frame from one block's own statements: a `local` binds its
/// initializer; a second binding of the same name (a second `local`
/// declaration, or no initializer) poisons it — no SSA.
///
/// The block-local scan
/// above is not enough on its own — a write to a bound name inside a
/// NESTED block (`local t = A; if c then t = B end`) or inside a closure
/// body (a capture write) must also poison the binding, or eval would
/// resolve through a stale initializer, violating "ambiguity degrades to
/// Unknown, never to a wrong path". So after the block-local pass, every
/// `Assign`/`CompoundAssign` anywhere in the WHOLE MODULE — every body,
/// `hir::visit::stmts` walks each body's own nested blocks too, and
/// closures are separate arena bodies — that name-matches a bound name
/// poisons it. This over-poisons same-named locals in unrelated scopes;
/// acceptable conservatism, the same instinct as
/// `project.rs::local_env`'s alias poisoning.
fn frame_of<'m>(block: &'m hir::Block, module: &'m hir::Module) -> Frame<'m> {
    // A plain fn, not a closure: a closure capturing `&mut f` would hold
    // that borrow for the closure's whole lifetime (every loop iteration
    // that might call it), blocking the direct `f.get_mut` calls below in
    // the SAME loop. Taking `f` as an explicit parameter borrows it only
    // for the duration of each call.
    fn bind<'m>(f: &mut HashMap<&'m str, Bind<'m>>, name: &'m str, b: Bind<'m>) {
        // Second binding of the same name in one block: poison (no SSA).
        f.entry(name).and_modify(|e| *e = Bind::Poison).or_insert(b);
    }
    let mut f: HashMap<&str, Bind> = HashMap::new();
    for stmt in &block.stmts {
        match &stmt.kind {
            hir::StmtKind::Local { decls, init } => {
                for (i, d) in decls.iter().enumerate() {
                    match init.get(i) {
                        Some(e) => bind(&mut f, &d.name, Bind::Init(e)),
                        None => bind(&mut f, &d.name, Bind::Poison),
                    }
                }
            }
            hir::StmtKind::LocalFunction { decl, body } => {
                bind(&mut f, &decl.name, Bind::Fn(*body))
            }
            hir::StmtKind::FunctionDecl { root, path, assigns_root, body, .. }
                if *assigns_root && path.is_empty() =>
            {
                bind(&mut f, root, Bind::Fn(*body))
            }
            hir::StmtKind::Assign { targets, .. } => {
                for t in targets {
                    if let hir::Target::Name { name, .. } = t {
                        if let Some(b) = f.get_mut(name.as_str()) {
                            *b = Bind::Poison;
                        }
                        // A name not locally bound is a global write — not ours.
                    }
                }
            }
            hir::StmtKind::CompoundAssign { target: hir::Target::Name { name, .. }, .. } => {
                if let Some(b) = f.get_mut(name.as_str()) {
                    *b = Bind::Poison;
                }
            }
            _ => {}
        }
    }

    // The whole-module nested-write / closure-capture poison scan (ruling
    // above).
    for body in &module.bodies {
        hir::visit::stmts(body, &mut |stmt| match &stmt.kind {
            hir::StmtKind::Assign { targets, .. } => {
                for t in targets {
                    if let hir::Target::Name { name, .. } = t {
                        if let Some(b) = f.get_mut(name.as_str()) {
                            *b = Bind::Poison;
                        }
                    }
                }
            }
            hir::StmtKind::CompoundAssign { target: hir::Target::Name { name, .. }, .. } => {
                if let Some(b) = f.get_mut(name.as_str()) {
                    *b = Bind::Poison;
                }
            }
            _ => {}
        });
    }

    Frame(f)
}

/// A frame for a block whose evaluated expression's lexical POSITION the
/// frame stack does not model — the two sites that have no position to
/// model:
///
///   - `eval_require_target`: a require site can sit in any body or nested
///     block of the module, and only the module's ROOT frame is built for
///     it;
///   - `table_field`: a form-2 `T.field = e` write is matched anywhere in
///     the defining module, and its RHS evaluates in that module's root
///     frame (plus, form 3, the method body's own).
///
/// At such a site any name that is bound MORE THAN ONCE in the module
/// might be shadowed at the real position by a binding this frame cannot
/// see, so resolving it through this frame's initializer would be a
/// confident WRONG answer rather than a missed one. Poison it — the same
/// conservatism `project.rs::local_env`/`collect_locals` already applies
/// for the cheap alias env, down to that env's one exception (a name
/// re-declared with a STRUCTURALLY IDENTICAL initializer keeps resolving:
/// whichever binding actually wins at the real position, it evaluates to
/// the same thing, because any name the shared initializer mentions is
/// itself either consistently bound or poisoned). So the evaluator is
/// never LESS conservative than the fast path it exists to improve on, and
/// never more so either — corpus 058's boot-script idiom (the same alias
/// bound at module level AND inside a function) still resolves.
///
/// Frames built where the stack IS the real lexical chain (`nested`,
/// `body_frame` under `call_returns`) keep plain `frame_of`'s precision:
/// there a shadowing inner binding is pushed as its own frame and wins on
/// its own, so nothing has to be given up.
fn frame_of_unpositioned<'m>(block: &'m hir::Block, module: &'m hir::Module) -> Frame<'m> {
    let mut frame = frame_of(block, module);
    let bindings = module_bindings(module);
    for (name, b) in frame.0.iter_mut() {
        let safe = bindings
            .get(*name)
            .is_some_and(|x| x.count == 1 || x.same_init.is_some());
        if !safe {
            *b = Bind::Poison;
        }
    }
    frame
}

/// What `module_bindings` records about one name.
struct NameBindings<'m> {
    /// How many times the name is bound anywhere in the module. Exactly
    /// one binding means nothing can shadow it, anywhere.
    count: usize,
    /// `Some(e)` while EVERY one of those bindings is a `local` whose
    /// initializer is structurally equal to `e` (`project::expr_eq`);
    /// `None` as soon as one is anything else — a parameter, a `for`
    /// variable, a function declaration, an initializer-less `local`, or a
    /// differing initializer. This is `collect_locals`' identical-
    /// re-declaration exception, in the evaluator's dialect.
    same_init: Option<&'m hir::Expr>,
}

/// Every name BOUND anywhere in the module, and how: `local` declarations,
/// `local function`, root-assigning `function f()`, numeric/generic `for`
/// variables, and every parameter of every body — the same binding forms
/// `project.rs::collect_locals` (plus `local_env`'s parameter pass) treats
/// as shadowing risks. `frame_of_unpositioned` is the only reader.
fn module_bindings<'m>(module: &'m hir::Module) -> HashMap<&'m str, NameBindings<'m>> {
    let mut out: HashMap<&str, NameBindings> = HashMap::new();
    fn bind<'m>(
        out: &mut HashMap<&'m str, NameBindings<'m>>,
        name: &'m str,
        init: Option<&'m hir::Expr>,
    ) {
        let e = out.entry(name).or_insert(NameBindings { count: 0, same_init: init });
        e.count += 1;
        if e.count > 1 {
            e.same_init = match (e.same_init, init) {
                (Some(a), Some(b)) if crate::project::expr_eq(a, b) => Some(a),
                _ => None,
            };
        }
    }
    for body in &module.bodies {
        for p in &body.params {
            bind(&mut out, p.name.as_str(), None);
        }
        // `stmts_lifetime`, not `hir::visit::stmts`: the recorded `&str`
        // keys and `&Expr` initializers must outlive the closure call (see
        // `stmts_lifetime`'s doc). It walks this body's own nested blocks
        // but never enters another arena body — and the loop above visits
        // every body already, so nothing is counted twice.
        stmts_lifetime(&body.block, &mut |stmt| match &stmt.kind {
            hir::StmtKind::Local { decls, init } => {
                for (i, d) in decls.iter().enumerate() {
                    bind(&mut out, d.name.as_str(), init.get(i));
                }
            }
            hir::StmtKind::LocalFunction { decl, .. } => {
                bind(&mut out, decl.name.as_str(), None);
            }
            hir::StmtKind::FunctionDecl { root, path, assigns_root, .. }
                if *assigns_root && path.is_empty() =>
            {
                bind(&mut out, root.as_str(), None);
            }
            hir::StmtKind::NumericFor { var, .. } => {
                bind(&mut out, var.name.as_str(), None);
            }
            hir::StmtKind::GenericFor { vars, .. } => {
                for v in vars {
                    bind(&mut out, v.name.as_str(), None);
                }
            }
            _ => {}
        });
    }
    out
}

/// Whether a module's export root `t` is ever rebound: declared more than
/// once (`module_bindings` counts every `local`, `local function`,
/// root-assigning `function`, loop variable, and parameter of that name),
/// or named as a bare target of any `Assign`/`CompoundAssign` — including
/// an under-valued multi-assign the positional zips cannot pair. Any of
/// these makes `table_field`'s recorded writes unreliable: the returned
/// table and the written table may differ.
fn export_root_rebound(module: &hir::Module, t: &str) -> bool {
    if module_bindings(module).get(t).is_some_and(|b| b.count > 1) {
        return true;
    }
    let mut rebound = false;
    for body in &module.bodies {
        hir::visit::stmts(body, &mut |stmt| match &stmt.kind {
            hir::StmtKind::Assign { targets, .. } => {
                rebound |= targets
                    .iter()
                    .any(|x| matches!(x, hir::Target::Name { name, .. } if name == t));
            }
            hir::StmtKind::CompoundAssign { target: hir::Target::Name { name, .. }, .. } => {
                rebound |= name == t;
            }
            _ => {}
        });
    }
    rebound
}

/// A function body's frame: every parameter binds `Poison` — its value is
/// never knowable from the callee's own scope, the same treatment
/// `project.rs::local_env` gives params shadowing an alias name — layered
/// under the body's own block frame (built, and whole-module poison-
/// scanned, by `frame_of`). Pushed per callee body when
/// function-return eval descends into a call.
fn body_frame<'m>(body: &'m hir::Body, module: &'m hir::Module) -> Frame<'m> {
    with_params(frame_of(&body.block, module), body)
}

/// `body_frame` for a position the frame stack does not model — see
/// `frame_of_unpositioned`. `table_field`'s form-3 (`self.field = e`)
/// method frame is the one caller: the write can sit in any nested block
/// of the method, not just its top level.
fn body_frame_unpositioned<'m>(body: &'m hir::Body, module: &'m hir::Module) -> Frame<'m> {
    with_params(frame_of_unpositioned(&body.block, module), body)
}

fn with_params<'m>(mut frame: Frame<'m>, body: &'m hir::Body) -> Frame<'m> {
    for p in &body.params {
        frame.0.insert(p.name.as_str(), Bind::Poison);
    }
    frame
}

/// Dedup by equality (candidate-set convention); past 8 distinct
/// candidates the set is unusable — collapse to a single `Unknown`.
fn dedup_cap(v: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for val in v {
        if !out.contains(&val) {
            out.push(val);
        }
    }
    if out.len() > 8 {
        vec![Value::Unknown]
    } else {
        out
    }
}

/// Display form for a `Diverged` file list: each path with the common
/// path-component prefix of the whole set stripped, `to_string_lossy`'d
/// (relative-ized like `Missing::described`). Order is preserved; callers pick the order (ctx-order, here).
fn strip_common_prefix(paths: &[PathBuf]) -> Vec<String> {
    if paths.len() <= 1 {
        return paths.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    }
    let comps: Vec<Vec<std::path::Component>> =
        paths.iter().map(|p| p.components().collect()).collect();
    let mut prefix_len = comps[0].len();
    for c in &comps[1..] {
        let matching = comps[0].iter().zip(c).take_while(|(a, b)| a == b).count();
        prefix_len = prefix_len.min(matching);
    }
    // Every path keeps at least its last component: when one path is a
    // component-prefix of another, the whole shorter path is "common" and
    // would otherwise render as an empty string.
    for c in &comps {
        prefix_len = prefix_len.min(c.len().saturating_sub(1));
    }
    comps
        .iter()
        .map(|c| c[prefix_len..].iter().collect::<PathBuf>().to_string_lossy().into_owned())
        .collect()
}

/// Every statement in `block`, including nested blocks (if/loop arms, `do`
/// blocks — the same set `hir::visit::stmts_in_block` covers; a nested
/// function body is not entered), but with the block's OWN lifetime `'m`
/// tied through to the closure. `hir::visit::stmts`'s public signature is
/// generic (`impl FnMut(&Stmt)`), which Rust desugars to a fresh
/// higher-ranked lifetime per call — fine when the closure only reads a
/// statement transiently (as `frame_of`'s poison scan does), but too short
/// to stash a matched write's `&'m Expr` past the call, which
/// `table_field` needs to do. A hand-rolled walk here is the fix.
fn stmts_lifetime<'m>(block: &'m hir::Block, f: &mut impl FnMut(&'m hir::Stmt)) {
    for stmt in &block.stmts {
        f(stmt);
        match &stmt.kind {
            hir::StmtKind::Do(b)
            | hir::StmtKind::While { body: b, .. }
            | hir::StmtKind::Repeat { body: b, .. }
            | hir::StmtKind::NumericFor { body: b, .. }
            | hir::StmtKind::GenericFor { body: b, .. } => stmts_lifetime(b, f),
            hir::StmtKind::If { then, else_ifs, or_else, .. } => {
                stmts_lifetime(then, f);
                for (_, b) in else_ifs {
                    stmts_lifetime(b, f);
                }
                if let Some(b) = or_else {
                    stmts_lifetime(b, f);
                }
            }
            _ => {}
        }
    }
}

/// A table literal's `Named` field values matching `name` — every match
/// (ordinarily 0 or 1; a malformed/duplicate literal can have more, and
/// every one becomes a candidate write for `table_field`'s agreement
/// check). `definition.rs::named_fields` returns every field's span+expr;
/// this only needs one field's value expressions.
fn named_field_values<'m>(fields: &'m [hir::TableField], name: &str) -> Vec<&'m hir::Expr> {
    fields
        .iter()
        .filter_map(|f| match f {
            hir::TableField::Named { name: n, value, .. } if n == name => Some(value),
            _ => None,
        })
        .collect()
}

/// If `target` is `<base>.<field> = ...` (a dot projection naming `field`
/// exactly, not a bracket key), the base name — `None` for anything else
/// (a bracket/computed key, a different field, a plain-name target).
/// Mirrors `definition.rs::static_field_write`'s shape match but returns
/// the write's BASE name, not the field's own span.
fn field_write_base<'m>(target: &'m hir::Target, field: &str) -> Option<&'m str> {
    let hir::Target::Projection(hir::Expr::Index { base, key: None, name: Some(n), .. }) = target
    else {
        return None;
    };
    if n != field {
        return None;
    }
    match &**base {
        hir::Expr::Name { name, .. } => Some(name.as_str()),
        _ => None,
    }
}

/// String-literal text of an `Expr::Literal`. A copy of rime-roblox's
/// private helper of the same name and shape: rime-check cannot depend on
/// rime-roblox, which depends on it.
fn literal_text(e: &hir::Expr) -> Option<String> {
    // Prim guard: numbers and booleans carry text too, and `t[1]` must not
    // resolve as a child named "1".
    let hir::Expr::Literal { prim: hir::Prim::String, text: Some(t), .. } = e else {
        return None;
    };
    Some(t.clone())
}

/// Whether `e` is the literal `nil` — `truth`'s `x == nil` / `x ~= nil`
/// nil-check recognition (either operand order).
fn is_nil_literal(e: &hir::Expr) -> bool {
    matches!(e, hir::Expr::Literal { prim: hir::Prim::Nil, .. })
}

/// Whether `block`, or any block nested inside it (if/elseif/else arms, a
/// `do` block, a nested loop's own body), can reach a
/// `Terminator::Return`. Mirrors the same block-recursion shape as
/// `stmts_lifetime` above (`hir::visit`'s public walkers don't expose
/// terminators, only statements) — a nested function body (`Expr::Function`
/// / `LocalFunction` / `FunctionDecl`) is a separate arena body and is NOT
/// entered, same boundary every walker in this file respects. Loops
/// are not modeled beyond this: any reachable `return` inside one
/// poisons the whole loop to `Unknown`.
fn contains_return(block: &hir::Block) -> bool {
    if matches!(block.terminator, Some(hir::Terminator::Return { .. })) {
        return true;
    }
    block.stmts.iter().any(|stmt| match &stmt.kind {
        hir::StmtKind::Do(b)
        | hir::StmtKind::While { body: b, .. }
        | hir::StmtKind::Repeat { body: b, .. }
        | hir::StmtKind::NumericFor { body: b, .. }
        | hir::StmtKind::GenericFor { body: b, .. } => contains_return(b),
        hir::StmtKind::If { then, else_ifs, or_else, .. } => {
            contains_return(then)
                || else_ifs.iter().any(|(_, b)| contains_return(b))
                || or_else.as_ref().is_some_and(contains_return)
        }
        _ => false,
    })
}

/// 1-based line number of byte `offset` in `src` — the same newline-count
/// idiom as `requires.rs::line_of`, copied rather than shared since that
/// one is private to its module and this crate has no shared "source
/// position" utility yet.
fn line_of(src: &str, offset: usize) -> usize {
    src.as_bytes()[..offset.min(src.len())].iter().filter(|&&b| b == b'\n').count() + 1
}

/// Bare filename for a `why` note (`nil_why`, `call_returns`'s
/// implicit-fallthrough push): `scope.file` is whatever absolute filesystem
/// path the resolver built it from (a project on disk: canonicalized from
/// disk, `\\?\`-prefixed on Windows; the corpus harness: the case dir's own
/// canonicalized absolute path) — baking the FULL path into a diagnostic
/// message would make the note, and the insta snapshot pinning it, depend
/// on where the project happens to be checked out. The bare filename is
/// portable and still names the module; `requires.rs::stale_stub_diag`
/// makes the same "no full path in a note" call for its own live-require
/// citation (a bare line number there, since it is always the same file).
fn display_file(file: &Path) -> std::borrow::Cow<'_, str> {
    match file.file_name() {
        Some(n) => n.to_string_lossy(),
        None => file.to_string_lossy(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock resolver — a filesystem-free instance tree:
    /// `game.ReplicatedStorage.Modules.{A,B}`, files
    /// `/rs/Modules/{A,B}.luau`.
    struct MockResolver;
    impl Resolver for MockResolver {
        fn resolve(
            &self,
            _: &Path,
            _: &hir::Expr,
            _: &HashMap<String, &hir::Expr>,
        ) -> Resolution {
            Resolution::Dynamic
        }
        fn step(&self, base: &Loc, seg: &str) -> Option<Loc> {
            let Loc::Dm(p) = base else { return None };
            let mut p = p.clone();
            if seg == "Parent" {
                p.pop()?;
            } else {
                p.push(seg.to_string());
            }
            Some(Loc::Dm(p))
        }
        fn service(&self, name: &str) -> Option<Loc> {
            Some(Loc::Dm(vec![name.to_string()]))
        }
        fn resolve_loc(&self, loc: &Loc) -> Resolution {
            let Loc::Dm(p) = loc else { return Resolution::Dynamic };
            match p.join(".").as_str() {
                "ReplicatedStorage.Modules.A" => {
                    Resolution::Found(PathBuf::from("/rs/Modules/A.luau"))
                }
                "ReplicatedStorage.Modules.B" => {
                    Resolution::Found(PathBuf::from("/rs/Modules/B.luau"))
                }
                _ => Resolution::Missing { described: p.join(".") },
            }
        }
        // `ReplicatedStorage.Folders` is a CONTAINER: it exists, but it is
        // not a module, so `resolve_loc` above calls it `Missing`. This is
        // the folder/module split `non_nil` has to see through: a packages
        // folder that a project mounts under a service.
        fn present(&self, loc: &Loc) -> bool {
            matches!(loc, Loc::Dm(p) if p.join(".") == "ReplicatedStorage.Folders")
        }
        // The instance a file IS — the inverse of `resolve_loc` above, for
        // `return script` (`export_script_return`).
        fn instance_of(&self, file: &Path) -> Option<Loc> {
            if file == a_file() {
                Some(Loc::Dm(vec![
                    "ReplicatedStorage".to_string(),
                    "Modules".to_string(),
                    "A".to_string(),
                ]))
            } else if file == b_file() {
                Some(Loc::Dm(vec![
                    "ReplicatedStorage".to_string(),
                    "Modules".to_string(),
                    "B".to_string(),
                ]))
            } else {
                None
            }
        }
    }

    struct EmptyMap;
    impl ModuleMap for EmptyMap {
        fn hir(&self, _: &Path) -> Option<&hir::Module> {
            None
        }
        fn source(&self, _: &Path) -> Option<&str> {
            None
        }
    }

    /// A `ModuleMap` that parses and serves modules by path — the deps
    /// the cross-module field lookup (`export_field`/`table_field`)
    /// reads.
    #[derive(Default)]
    struct MockMap {
        modules: HashMap<PathBuf, hir::Module>,
        sources: HashMap<PathBuf, String>,
    }

    impl MockMap {
        fn new() -> Self {
            Self::default()
        }

        fn insert(&mut self, path: PathBuf, src: &str) {
            self.modules.insert(path.clone(), parse(src));
            self.sources.insert(path, src.to_string());
        }
    }

    impl ModuleMap for MockMap {
        fn hir(&self, path: &Path) -> Option<&hir::Module> {
            self.modules.get(path)
        }
        fn source(&self, path: &Path) -> Option<&str> {
            self.sources.get(path).map(String::as_str)
        }
    }

    /// A `ModuleMap` whose `source` serves fixed strings from a table —
    /// `collapse`'s byte-identity (step 5) and diverged (step 6) tests need
    /// `source()` to answer for specific files without a real HIR/source
    /// pair backing them.
    struct FixedMap(HashMap<PathBuf, &'static str>);
    impl ModuleMap for FixedMap {
        fn hir(&self, _: &Path) -> Option<&hir::Module> {
            None
        }
        fn source(&self, path: &Path) -> Option<&str> {
            self.0.get(path).copied()
        }
    }

    fn parse(src: &str) -> hir::Module {
        rime_syntax::parse(src).expect("test source must parse")
    }

    fn a_file() -> PathBuf {
        PathBuf::from("/rs/Modules/A.luau")
    }

    fn b_file() -> PathBuf {
        PathBuf::from("/rs/Modules/B.luau")
    }

    fn rs_a() -> Value {
        Value::Inst(Loc::Dm(vec![
            "ReplicatedStorage".to_string(),
            "Modules".to_string(),
            "A".to_string(),
        ]))
    }

    fn rs_b() -> Value {
        Value::Inst(Loc::Dm(vec![
            "ReplicatedStorage".to_string(),
            "Modules".to_string(),
            "B".to_string(),
        ]))
    }

    /// Evaluate a bare reference to `name` in the module's top-level scope
    /// — the same thing evaluating a use-site of that name in the source
    /// would do (the `Name` arm forwards straight to the binding's
    /// initializer).
    fn eval_name(module: &hir::Module, name: &str) -> Vec<Value> {
        let root = module.body(module.root);
        let frame = frame_of(&root.block, module);
        eval_name_in(module, vec![frame], name)
    }

    /// Same, but in the scope of the module's first `local function` /
    /// `function f()` body (for testing param poisoning).
    fn eval_name_in_first_fn(module: &hir::Module, name: &str) -> Vec<Value> {
        let root = module.body(module.root);
        let body_id = root
            .block
            .stmts
            .iter()
            .find_map(|s| match &s.kind {
                hir::StmtKind::LocalFunction { body, .. } => Some(*body),
                hir::StmtKind::FunctionDecl { body, .. } => Some(*body),
                _ => None,
            })
            .expect("test source must declare a function");
        let frame = body_frame(module.body(body_id), module);
        eval_name_in(module, vec![frame], name)
    }

    fn eval_name_in(module: &hir::Module, frames: Vec<Frame<'_>>, name: &str) -> Vec<Value> {
        eval_name_in_with_modules(module, frames, &EmptyMap, Ctx::Server, name)
    }

    /// Same as `eval_name_in`, but with a caller-supplied `ModuleMap` and
    /// `Ctx` — the require-target's cross-module tests (`export_*`) need
    /// dep modules resident behind `MockMap` rather than the always-empty
    /// `EmptyMap`, and the function-return ctx-split tests (`fn_ctx_branch`,
    /// `fn_upvalue_is_server`) need both `Server` and `Client` runs of the
    /// same module.
    fn eval_name_in_with_modules(
        module: &hir::Module,
        frames: Vec<Frame<'_>>,
        modules: &dyn ModuleMap,
        ctx: Ctx,
        name: &str,
    ) -> Vec<Value> {
        let file = PathBuf::from("/rs/Modules/Test.luau");
        let scope = Scope { file: &file, module, frames };
        let resolver = MockResolver;
        let evaluator = Evaluator::new(&resolver, modules);
        let mut cx = Cx { ctx, depth: 0, visited: HashSet::new() };
        let name_expr = hir::Expr::Name { name: name.to_string(), span: 0..0 };
        evaluator.eval(&mut cx, &scope, &name_expr)
    }

    /// Evaluate a bare reference to `name` at `requiring_src`'s top level,
    /// with `modules` behind the require target as dep HIR — the shape
    /// every `export_*` test shares: parse the requiring module, build its
    /// top-level frame, eval.
    fn eval_require_field(requiring_src: &str, modules: &dyn ModuleMap, name: &str) -> Vec<Value> {
        let module = parse(requiring_src);
        let frame = frame_of(&module.body(module.root).block, &module);
        eval_name_in_with_modules(&module, vec![frame], modules, Ctx::Server, name)
    }

    /// Same as `eval_name`, but under a caller-chosen `Ctx` — see
    /// `eval_name_in_with_modules`.
    fn eval_name_with_ctx(module: &hir::Module, ctx: Ctx, name: &str) -> Vec<Value> {
        let root = module.body(module.root);
        let frame = frame_of(&root.block, module);
        eval_name_in_with_modules(module, vec![frame], &EmptyMap, ctx, name)
    }

    #[test]
    fn eval_dm_chain() {
        let module = parse(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = RS.Modules.A
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a()]);
    }

    #[test]
    fn eval_ffc() {
        let module = parse(
            r#"
            local t = game:GetService("ReplicatedStorage"):FindFirstChild("Modules")
            "#,
        );
        assert_eq!(
            eval_name(&module, "t"),
            vec![Value::Inst(Loc::Dm(vec![
                "ReplicatedStorage".to_string(),
                "Modules".to_string(),
            ]))]
        );
    }

    #[test]
    fn eval_or_first_resolves() {
        let module = parse(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = RS.Modules.A or RS.Modules.B
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a()]);
    }

    #[test]
    fn eval_or_lhs_unknown() {
        let module = parse(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = unknownGlobal or RS.Modules.A
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![Value::Unknown]);
    }

    #[test]
    fn eval_rebound_poison() {
        let module = parse(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = RS.Modules.A
            t = RS.Modules.B
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![Value::Unknown]);
    }

    #[test]
    fn eval_param_poison() {
        let module = parse(
            r#"
            local function f(t)
                local x = t
            end
            "#,
        );
        assert_eq!(eval_name_in_first_fn(&module, "x"), vec![Value::Unknown]);
    }

    #[test]
    fn eval_depth_cap() {
        // 20-deep alias chain: t1 = t2 = ... = t20 = RS.Modules.A. The
        // depth cap (16) trips well before t20's real initializer is ever
        // reached, proving the cap — not global fallback — produced the
        // Unknown.
        let mut src = String::from(
            "local RS = game:GetService(\"ReplicatedStorage\")\nlocal t20 = RS.Modules.A\n",
        );
        for i in (1..20).rev() {
            src.push_str(&format!("local t{i} = t{}\n", i + 1));
        }
        let module = parse(&src);
        assert_eq!(eval_name(&module, "t1"), vec![Value::Unknown]);
    }

    /// A rebind
    /// inside a NESTED block must poison too, not just a rebind at the
    /// frame's own top level.
    #[test]
    fn eval_nested_rebind_poison() {
        let module = parse(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = RS.Modules.A
            if unknown then
                t = RS.Modules.B
            end
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![Value::Unknown]);
    }

    // ---- require(e) -> Export, cross-module field lookup ----

    /// `M.field = e` — a static field write anywhere in the module.
    #[test]
    fn export_static_field() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local M = {}\nM.Sub = game:GetService(\"ReplicatedStorage\").Modules.A\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![rs_a()]);
    }

    /// `local M = { field = e }` — a table-literal entry.
    #[test]
    fn export_table_literal_field() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local M = { Sub = game:GetService(\"ReplicatedStorage\").Modules.A }\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![rs_a()]);
    }

    /// `self.field = e` inside `function M:init()`.
    #[test]
    fn export_self_field() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local M = {}\nfunction M:init()\n  self.Sub = game:GetService(\"ReplicatedStorage\").Modules.A\nend\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![rs_a()]);
    }

    /// A module may call the table it returns `self`. At the top level that
    /// is an ordinary local, not a method receiver, and its field writes
    /// are the module's exports like any other name's.
    #[test]
    fn export_root_named_self() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local self = {}\nself.Sub = game:GetService(\"ReplicatedStorage\").Modules.A\nreturn self",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![rs_a()]);
    }

    /// The table a module returns may start as `setmetatable({}, X)` instead
    /// of a bare `{}`. Its field writes are the module's exports all the same.
    #[test]
    fn export_root_from_setmetatable() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local Proto = {}\nProto.__index = Proto\nlocal M = setmetatable({}, Proto)\nM.Sub = game:GetService(\"ReplicatedStorage\").Modules.A\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![rs_a()]);
    }

    /// `:FindFirstChild("Name")` steps to a child the way `.Name` does, on
    /// the instance another module returns and on `script` itself.
    #[test]
    fn ffc_on_a_returned_instance_and_on_script() {
        let sub = Value::Inst(Loc::Dm(vec![
            "ReplicatedStorage".to_string(),
            "Modules".to_string(),
            "A".to_string(),
            "Sub".to_string(),
        ]));

        let mut modules = MockMap::new();
        modules.insert(a_file(), "return script");
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A):FindFirstChild("Sub")
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![sub.clone()]);

        let mut modules = MockMap::new();
        modules.insert(a_file(), "local M = {}\nM.Sub = script:FindFirstChild(\"Sub\")\nreturn M");
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![sub]);
    }

    /// `return script` — fields are child instances, stepped through the
    /// resolver, not discovered from HIR.
    #[test]
    fn export_script_return() {
        let mut modules = MockMap::new();
        modules.insert(a_file(), "return script");
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        let expected = Value::Inst(Loc::Dm(vec![
            "ReplicatedStorage".to_string(),
            "Modules".to_string(),
            "A".to_string(),
            "Sub".to_string(),
        ]));
        assert_eq!(eval_require_field(src, &modules, "t"), vec![expected]);
    }

    /// Two `M.Sub` writes with differing RHS: never guess.
    #[test]
    fn export_conflicting_writes() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local M = {}\nM.Sub = game:GetService(\"ReplicatedStorage\").Modules.A\nM.Sub = game:GetService(\"ReplicatedStorage\").Modules.B\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![Value::Unknown]);
    }

    /// `local M = { Sub = <A> }` then a bare `M = ...` reassignment: the
    /// literal is stale at runtime — the table the module RETURNS may not
    /// be the one carrying the literal. Never guess.
    #[test]
    fn export_root_reassigned_poisons() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local M = { Sub = game:GetService(\"ReplicatedStorage\").Modules.A }\nM = somethingElse()\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![Value::Unknown]);
    }

    /// A second `local M` re-declaration: `return M` names the SECOND
    /// binding; the first one's literal must not answer for it.
    #[test]
    fn export_root_redeclared_poisons() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local M = { Sub = game:GetService(\"ReplicatedStorage\").Modules.A }\nlocal M = {}\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![Value::Unknown]);
    }

    /// An under-valued multi-assign onto the export root (`x, M = f()`):
    /// the positional zip cannot pair `M` with a value, but the write is
    /// still a rebinding — the literal above it is stale.
    #[test]
    fn export_root_undervalued_multiassign_poisons() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local M = { Sub = game:GetService(\"ReplicatedStorage\").Modules.A }\nlocal x\nx, M = somethingElse()\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![Value::Unknown]);
    }

    /// A linker stub (`return require(x)`, the wally/rotriever package-stub
    /// idiom) chases to the terminal module and re-asks there.
    #[test]
    fn export_linker_chase() {
        let mut modules = MockMap::new();
        modules.insert(
            b_file(),
            "return require(game:GetService(\"ReplicatedStorage\").Modules.A)",
        );
        modules.insert(
            a_file(),
            "local M = {}\nM.Sub = game:GetService(\"ReplicatedStorage\").Modules.A\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.B).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![rs_a()]);
    }

    /// `return table.freeze(M)` — the export name is still `M`.
    #[test]
    fn export_freeze_wrapped() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local M = {}\nM.Sub = game:GetService(\"ReplicatedStorage\").Modules.A\nreturn table.freeze(M)",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![rs_a()]);
    }

    // ---- collapse ----

    fn collapse_with(
        resolver: &dyn Resolver,
        modules: &dyn ModuleMap,
        per_ctx: Vec<(Ctx, Vec<Value>)>,
    ) -> Outcome {
        Evaluator::new(resolver, modules).collapse(per_ctx)
    }

    #[test]
    fn collapse_unknown_wins() {
        let per_ctx = vec![(Ctx::Server, vec![rs_a(), Value::Unknown])];
        let outcome = collapse_with(&MockResolver, &EmptyMap, per_ctx);
        assert!(matches!(outcome, Outcome::Opaque), "expected Opaque, got {outcome:?}");
    }

    #[test]
    fn collapse_nil_wins_over_resolved() {
        let why = "locateModules can return nil".to_string();
        let per_ctx = vec![(Ctx::Server, vec![rs_a(), Value::Nil { why }])];
        let outcome = collapse_with(&MockResolver, &EmptyMap, per_ctx);
        match outcome {
            Outcome::NilCapable { why } => assert_eq!(why, "locateModules can return nil"),
            other => panic!("expected NilCapable, got {other:?}"),
        }
    }

    #[test]
    fn collapse_single() {
        let per_ctx = vec![(Ctx::Server, vec![rs_a()]), (Ctx::Client, vec![rs_a()])];
        let outcome = collapse_with(&MockResolver, &EmptyMap, per_ctx);
        match outcome {
            Outcome::Resolved(Resolution::Found(f)) => assert_eq!(f, a_file()),
            other => panic!("expected Resolved(Found(A)), got {other:?}"),
        }
    }

    #[test]
    fn collapse_identical_sources() {
        let modules = FixedMap(HashMap::from([(a_file(), "return {}"), (b_file(), "return {}")]));
        let per_ctx = vec![(Ctx::Server, vec![rs_a()]), (Ctx::Client, vec![rs_b()])];
        let outcome = collapse_with(&MockResolver, &modules, per_ctx);
        match outcome {
            // Canonical = the Server-ctx candidate: A.
            Outcome::Resolved(Resolution::Found(f)) => assert_eq!(f, a_file()),
            other => panic!("expected Resolved(Found(A)), got {other:?}"),
        }
    }

    /// Pins "canonical = the Server-ctx candidate" as distinct from
    /// "canonical = whichever candidate is positionally first": Client is
    /// listed before Server here, sources still byte-identical, so a
    /// regression to naive `files.first()` (dropping the ctx preference)
    /// would pick B (Client's file) and fail this test — unlike
    /// `collapse_identical_sources` above, where Server already happens to
    /// be positionally first and so can't tell the two policies apart.
    #[test]
    fn collapse_identical_sources_server_not_first() {
        let modules = FixedMap(HashMap::from([(a_file(), "return {}"), (b_file(), "return {}")]));
        let per_ctx = vec![(Ctx::Client, vec![rs_b()]), (Ctx::Server, vec![rs_a()])];
        let outcome = collapse_with(&MockResolver, &modules, per_ctx);
        match outcome {
            // Canonical = the Server-ctx candidate: A — even though B
            // (Client) was listed first.
            Outcome::Resolved(Resolution::Found(f)) => assert_eq!(f, a_file()),
            other => panic!("expected Resolved(Found(A)), got {other:?}"),
        }
    }

    #[test]
    fn collapse_diverged() {
        let modules = FixedMap(HashMap::from([(a_file(), "return 1"), (b_file(), "return 2")]));
        let per_ctx = vec![(Ctx::Server, vec![rs_a()]), (Ctx::Client, vec![rs_b()])];
        let outcome = collapse_with(&MockResolver, &modules, per_ctx);
        match outcome {
            // ctx-order (server first) with the common `/rs/Modules/`
            // prefix stripped. The per-ctx candidate SETS differ (server
            // lands A, client lands B), so each file carries the ctx that
            // lands on it.
            Outcome::Diverged { files } => {
                assert_eq!(
                    files,
                    vec!["A.luau (server)".to_string(), "B.luau (client)".to_string()]
                )
            }
            other => panic!("expected Diverged, got {other:?}"),
        }
    }

    /// Both ctx bits carry the SAME candidate set (one Unknown-cond union
    /// evaluated under both bits — corpus 088's shape): a ctx label would
    /// claim a client/server split that does not exist, so none renders.
    #[test]
    fn collapse_diverged_same_sets_unlabeled() {
        let modules = FixedMap(HashMap::from([(a_file(), "return 1"), (b_file(), "return 2")]));
        let per_ctx = vec![
            (Ctx::Server, vec![rs_a(), rs_b()]),
            (Ctx::Client, vec![rs_a(), rs_b()]),
        ];
        let outcome = collapse_with(&MockResolver, &modules, per_ctx);
        match outcome {
            Outcome::Diverged { files } => {
                assert_eq!(files, vec!["A.luau".to_string(), "B.luau".to_string()])
            }
            other => panic!("expected Diverged, got {other:?}"),
        }
    }

    /// One candidate path being a component-prefix of the other must not
    /// render as an empty string (`.max(1)`
    /// floor — every path keeps at least its last component).
    #[test]
    fn strip_common_prefix_keeps_prefix_path_nonempty() {
        let out = strip_common_prefix(&[
            PathBuf::from("/rs/Modules"),
            PathBuf::from("/rs/Modules/A.luau"),
        ]);
        assert!(out.iter().all(|s| !s.is_empty()), "empty render in {out:?}");
    }

    #[test]
    fn collapse_missing_wins() {
        let nonexistent = Value::Inst(Loc::Dm(vec![
            "ReplicatedStorage".to_string(),
            "Modules".to_string(),
            "Nonexistent".to_string(),
        ]));
        let per_ctx = vec![(Ctx::Server, vec![nonexistent])];
        let outcome = collapse_with(&MockResolver, &EmptyMap, per_ctx);
        match outcome {
            Outcome::Resolved(Resolution::Missing { described }) => {
                assert_eq!(described, "ReplicatedStorage.Modules.Nonexistent")
            }
            other => panic!("expected Resolved(Missing), got {other:?}"),
        }
    }

    // ---- function-return eval: branches, ctx truth, guard pruning,
    // cycles ----

    #[test]
    fn fn_return_simple() {
        let module = parse(
            r#"
            local function get()
                return game:GetService("ReplicatedStorage").Modules.A
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a()]);
    }

    /// A ctx-decided `if/else`, both arms explicitly returning, with no
    /// trailing statement after the `if` — the block's own terminator is
    /// `None` (the last STATEMENT is the `If`, not a bare `return`), so
    /// getting `[Inst(B)]` alone under `Client` (not `[Inst(B), Nil{..}]`)
    /// depends on `block_returns` recognizing that once `then` is proven
    /// false with no `else_ifs`, `or_else` is the ONLY branch that can run
    /// — same as a proven-true arm.
    #[test]
    fn fn_ctx_branch() {
        let module = parse(
            r#"
            local RunService = game:GetService("RunService")
            local function get()
                if RunService:IsServer() then
                    return game:GetService("ReplicatedStorage").Modules.A
                else
                    return game:GetService("ReplicatedStorage").Modules.B
                end
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name_with_ctx(&module, Ctx::Server, "t"), vec![rs_a()]);
        assert_eq!(eval_name_with_ctx(&module, Ctx::Client, "t"), vec![rs_b()]);
    }

    /// The same decision spread over `if`/`elseif` with no `else`: a dead
    /// first arm must not hide the `elseif` behind it, and a live arm that
    /// returns must stop the function's fall-through `nil` from joining
    /// the candidates.
    #[test]
    fn fn_ctx_elseif_branch() {
        let module = parse(
            r#"
            local RunService = game:GetService("RunService")
            local function get()
                if RunService:IsClient() then
                    return game:GetService("ReplicatedStorage").Modules.B
                elseif RunService:IsServer() then
                    return game:GetService("ReplicatedStorage").Modules.A
                end
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name_with_ctx(&module, Ctx::Server, "t"), vec![rs_a()]);
        assert_eq!(eval_name_with_ctx(&module, Ctx::Client, "t"), vec![rs_b()]);
    }

    #[test]
    fn fn_guard_prunes() {
        let module = parse(
            r#"
            local function get()
                local s = game:GetService("ReplicatedStorage").Modules.A
                if s == nil then
                    return nil
                end
                return s
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a()]);
    }

    /// Same shape as `fn_guard_prunes`, but the chain misses
    /// (`resolve_loc` gives `Missing`) — `s`'s non-nil-ness is unprovable,
    /// so `truth(s == nil)` is `None` and BOTH arms (the guard's `return
    /// nil` and the fallthrough `return s`) contribute.
    #[test]
    fn fn_guard_unpruned() {
        let module = parse(
            r#"
            local function get()
                local s = game:GetService("ReplicatedStorage").Modules.Missing
                if s == nil then
                    return nil
                end
                return s
            end
            local t = get()
            "#,
        );
        let vals = eval_name(&module, "t");
        assert!(
            vals.iter().any(|v| matches!(v, Value::Nil { .. })),
            "expected a Nil candidate, got {vals:?}"
        );
    }

    #[test]
    fn fn_unknown_cond_unions() {
        let module = parse(
            r#"
            local function get()
                if someGlobal then
                    return game:GetService("ReplicatedStorage").Modules.A
                end
                return game:GetService("ReplicatedStorage").Modules.B
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a(), rs_b()]);
    }

    #[test]
    fn fn_fallthrough_nil() {
        let module = parse(
            r#"
            local function get()
                if someGlobal then
                    return game:GetService("ReplicatedStorage").Modules.A
                end
            end
            local t = get()
            "#,
        );
        let vals = eval_name(&module, "t");
        assert_eq!(vals.len(), 2, "expected [Inst(A), Nil{{..}}], got {vals:?}");
        assert_eq!(vals[0], rs_a());
        assert!(matches!(&vals[1], Value::Nil { .. }), "expected trailing Nil, got {vals:?}");
    }

    /// `f` and `g` call each other; the cycle guard (`Cx::visited`) must
    /// degrade the re-entrant call to `Unknown` rather than looping
    /// forever.
    #[test]
    fn fn_recursion_poisons() {
        let module = parse(
            r#"
            local function f()
                return g()
            end
            function g()
                return f()
            end
            local t = f()
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![Value::Unknown]);
    }

    #[test]
    fn fn_loop_return_poisons() {
        let module = parse(
            r#"
            local function get()
                for i = 1, 10 do
                    return game:GetService("ReplicatedStorage").Modules.A
                end
            end
            local t = get()
            "#,
        );
        let vals = eval_name(&module, "t");
        assert!(vals.contains(&Value::Unknown), "expected Unknown from the loop return, got {vals:?}");
    }

    /// `onServer` is a module-top-level local, read as an UPVALUE from
    /// inside `get`'s body — `call_returns` must thread the caller's own
    /// frame chain into the callee scope for this to resolve at all
    /// (otherwise it is an unbound name and falls back to global/Unknown).
    #[test]
    fn fn_upvalue_is_server() {
        let module = parse(
            r#"
            local RunService = game:GetService("RunService")
            local onServer = RunService:IsServer()
            local function get()
                if onServer then
                    return game:GetService("ReplicatedStorage").Modules.A
                end
                return game:GetService("ReplicatedStorage").Modules.B
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name_with_ctx(&module, Ctx::Server, "t"), vec![rs_a()]);
        assert_eq!(eval_name_with_ctx(&module, Ctx::Client, "t"), vec![rs_b()]);
    }

    /// A `local` declared INSIDE an `if` arm is in scope for the rest of
    /// that arm. `frame_of` only scans a block's own statements, so
    /// `block_returns` has to push the arm's own frame (`nested`) as it
    /// descends — without it `s` is an unbound global and the arm returns
    /// `Unknown` (a package linker may declare the value it
    /// returns inside the branch).
    #[test]
    fn fn_block_local_in_scope() {
        let module = parse(
            r#"
            local function get()
                if someGlobal then
                    local s = game:GetService("ReplicatedStorage").Modules.A
                    return s
                end
                return game:GetService("ReplicatedStorage").Modules.B
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a(), rs_b()]);
    }

    /// The same scoping, one rung further: the guard that tests the
    /// block-local must prune through it.
    #[test]
    fn fn_block_local_guard_prunes() {
        let module = parse(
            r#"
            local function get()
                if someGlobal then
                    local s = game:GetService("ReplicatedStorage").Modules.A
                    if s == nil then
                        return nil
                    end
                    return s
                end
                return game:GetService("ReplicatedStorage").Modules.B
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a(), rs_b()]);
    }

    /// Shadowing, the case that makes the missing frame a WRONG path
    /// rather than merely an unknown one: before `nested`, the inner
    /// `local s` was invisible and `s` inside the arm resolved to the
    /// OUTER `s`'s initializer — a confident answer, and the wrong file.
    #[test]
    fn fn_block_local_shadows_outer() {
        let module = parse(
            r#"
            local function get()
                local s = game:GetService("ReplicatedStorage").Modules.B
                if someGlobal then
                    local s = game:GetService("ReplicatedStorage").Modules.A
                    return s
                end
                return s
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a(), rs_b()]);
    }

    /// A `Resolver` that implements only the required method — what the
    /// corpus's sibling resolver and the LSP's get. `present` must opt out
    /// as "no proof", and that must stay INERT: `non_nil` reads it as
    /// unproven (`None`), never as a claim of absence (`Some(false)`).
    #[test]
    fn present_defaults_to_no_proof() {
        struct Bare;
        impl Resolver for Bare {
            fn resolve(
                &self,
                _: &Path,
                _: &hir::Expr,
                _: &HashMap<String, &hir::Expr>,
            ) -> Resolution {
                Resolution::Dynamic
            }
        }
        let loc = Loc::Dm(vec!["ReplicatedStorage".to_string(), "Folders".to_string()]);
        assert!(!Bare.present(&loc));
        let ev = Evaluator::new(&Bare, &EmptyMap);
        assert_eq!(ev.non_nil(&Value::Inst(loc)), None);
    }

    /// A CONTAINER instance (a folder) is present but not requireable:
    /// `resolve_loc` says `Missing`, `Resolver::present` says it is there.
    /// The `or` must therefore take its left side, and the `== nil` guard
    /// must prune, as for a mounted packages folder that is looked up by
    /// name. Without the presence oracle both
    /// steps degrade and a `Nil` candidate reaches the require.
    #[test]
    fn fn_guard_prunes_on_present_container() {
        let module = parse(
            r#"
            local function get()
                local sp = game:GetService("ReplicatedStorage").Folders
                    or game:GetService("ReplicatedStorage").Modules.B
                if sp == nil then
                    return nil
                end
                return game:GetService("ReplicatedStorage").Modules.A
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a()]);
    }

    /// The `--timings` stopwatch: off by default so an untimed
    /// run never reads the clock, armed by `with_timing(true)`, and then
    /// monotonically accumulating across calls. A timed evaluator answers
    /// `Some(ZERO)` before it has evaluated anything, so `check_file` can
    /// record the phase for every file and keep the phase's file count
    /// comparable with the other per-file phases'.
    #[test]
    fn eval_timing_is_opt_in_and_accumulates() {
        let module = parse(r#"local RS = game:GetService("ReplicatedStorage")"#);
        let target = hir::Expr::Name { name: "RS".to_string(), span: 0..0 };
        let file = PathBuf::from("/rs/Modules/Test.luau");

        let off = Evaluator::new(&MockResolver, &EmptyMap);
        assert_eq!(off.spent(), None);
        off.eval_require_target(&file, &module, &target);
        assert_eq!(off.spent(), None, "an untimed run must never read the clock");

        let on = Evaluator::new(&MockResolver, &EmptyMap).with_timing(true);
        assert_eq!(on.spent(), Some(Duration::ZERO));
        on.eval_require_target(&file, &module, &target);
        let first = on.spent().expect("armed");
        on.eval_require_target(&file, &module, &target);
        assert!(on.spent().expect("armed") >= first, "must accumulate, not replace");

        // `with_timing(false)` is the same as not calling it at all — the
        // shape `check_file` uses when the run has no `--timings`.
        assert_eq!(Evaluator::new(&MockResolver, &EmptyMap).with_timing(false).spent(), None);
    }

    // ---- lexical scope position ----

    /// The first `require(<target>)` argument anywhere in the module — the
    /// require SITE `check_file`/`requires::check` hand to
    /// `eval_require_target`. Every site below deliberately sits inside a
    /// nested body or block: the evaluator only ever builds the module's
    /// ROOT frame for a require site, so a name that site's own scope
    /// rebinds is invisible there and MUST be poisoned rather than
    /// resolved through the outer initializer.
    fn require_arg(module: &hir::Module) -> &hir::Expr {
        fn is_require(e: &hir::Expr) -> Option<&hir::Expr> {
            let hir::Expr::Call { base, args, method: None, .. } = e else { return None };
            match &**base {
                hir::Expr::Name { name, .. } if name == "require" => args.first(),
                _ => None,
            }
        }
        for body in &module.bodies {
            let mut found: Option<&hir::Expr> = None;
            stmts_lifetime(&body.block, &mut |stmt| {
                if found.is_some() {
                    return;
                }
                let exprs: &[hir::Expr] = match &stmt.kind {
                    hir::StmtKind::Local { init, .. } => init,
                    hir::StmtKind::Assign { values, .. } => values,
                    hir::StmtKind::Call(e) => std::slice::from_ref(e),
                    _ => &[],
                };
                found = exprs.iter().find_map(is_require);
            });
            if let Some(e) = found {
                return e;
            }
        }
        panic!("test source must contain a require(...)")
    }

    /// End-to-end `eval_require_target` on the module's own require site.
    fn require_outcome(src: &str) -> Outcome {
        let module = parse(src);
        let target = require_arg(&module);
        let file = PathBuf::from("/rs/Modules/Test.luau");
        let resolver = MockResolver;
        Evaluator::new(&resolver, &EmptyMap).eval_require_target(&file, &module, target)
    }

    /// (i) A PARAMETER shadowing a top-level alias. Before the fix the root
    /// frame answered `RS` with the service handle and the site resolved
    /// confidently to `A.luau` — a wrong file, not merely a missed one.
    #[test]
    fn require_site_param_shadow_poisons() {
        let outcome = require_outcome(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            local function f(RS)
                local m = require(RS.Modules.A)
                return m
            end
            return f
            "#,
        );
        assert!(matches!(outcome, Outcome::Opaque), "expected Opaque, got {outcome:?}");
    }

    /// (ii) An inner `local` shadowing a top-level alias.
    #[test]
    fn require_site_nested_local_shadow_poisons() {
        let outcome = require_outcome(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            local function f()
                local RS = game:GetService("ServerStorage")
                local m = require(RS.Modules.A)
                return m
            end
            return f
            "#,
        );
        assert!(matches!(outcome, Outcome::Opaque), "expected Opaque, got {outcome:?}");
    }

    /// (iii) A `for` variable shadowing a top-level alias.
    #[test]
    fn require_site_for_var_shadow_poisons() {
        let outcome = require_outcome(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            for _, RS in pairs(folders) do
                local m = require(RS.Modules.A)
            end
            "#,
        );
        assert!(matches!(outcome, Outcome::Opaque), "expected Opaque, got {outcome:?}");
    }

    /// The one exception the fast-path alias env makes, mirrored here so
    /// the evaluator is not MORE conservative than it either: the
    /// boot-script idiom (corpus 058) — the same alias declared at module
    /// level AND inside a function, with a structurally identical
    /// initializer. Whichever binding wins at the site's real position it
    /// evaluates to the same instance, so the site still resolves.
    /// Contrast `require_site_nested_local_shadow_poisons`, where the two
    /// initializers name different services.
    #[test]
    fn require_site_identical_redeclaration_still_resolves() {
        let outcome = require_outcome(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            local function f()
                local RS = game:GetService("ReplicatedStorage")
                local m = require(RS.Modules.A)
                return m
            end
            return f
            "#,
        );
        match outcome {
            Outcome::Resolved(Resolution::Found(f)) => assert_eq!(f, a_file()),
            other => panic!("expected Resolved(Found(A)), got {other:?}"),
        }
    }

    /// (iv) A form-2 cross-module field write sitting INSIDE a function
    /// whose own `local` shadows the alias its RHS reads. The write's RHS
    /// evaluates in the defining module's root frame, which does not model
    /// the write's position — so the shadowed name has to be poisoned
    /// there (the sanctioned minimum; the true answer is ServerStorage's
    /// `A`, never ReplicatedStorage's).
    #[test]
    fn export_write_in_fn_with_shadowing_local_poisons() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local RS = game:GetService(\"ReplicatedStorage\")\nlocal M = {}\nlocal function init()\n  local RS = game:GetService(\"ServerStorage\")\n  M.Sub = RS.Modules.A\nend\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![Value::Unknown]);
    }

    /// (v) The positive half: a binding's initializer is an expression at
    /// the BINDING's site, so it must evaluate against the frames in scope
    /// THERE — not against the reader's own deeper stack. `root`'s init
    /// mentions `RS`; the callee that reads `root` rebinds `RS` to a
    /// different service. Re-evaluating the init in the callee's frames
    /// (the old behaviour) answers ServerStorage — dynamic scoping, and a
    /// wrong file. The top-level env is the only correct answer, and this
    /// is the shape `fn_upvalue_is_server` depends on.
    #[test]
    fn init_evaluates_in_its_own_binding_scope() {
        let module = parse(
            r#"
            local RS = game:GetService("ReplicatedStorage")
            local root = RS.Modules.A
            local function get()
                local RS = game:GetService("ServerStorage")
                return root
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name(&module, "t"), vec![rs_a()]);
    }

    // ---- multi-target Assign ----

    /// `x, M.Sub = f()` — the target and value lists do not line up, so the
    /// positional zip would drop the `M.Sub` write entirely and leave the
    /// table-literal entry looking like the module's ONLY write to `Sub`.
    /// Runtime overwrites it; never resolve confidently through it.
    #[test]
    fn export_under_valued_multi_assign_poisons() {
        let mut modules = MockMap::new();
        modules.insert(
            a_file(),
            "local M = { Sub = game:GetService(\"ReplicatedStorage\").Modules.A }\nlocal x\nx, M.Sub = getBoth()\nreturn M",
        );
        let src = r#"
            local RS = game:GetService("ReplicatedStorage")
            local t = require(RS.Modules.A).Sub
        "#;
        assert_eq!(eval_require_field(src, &modules, "t"), vec![Value::Unknown]);
    }

    // ---- nil comparison vs. Bool ----

    /// `false ~= nil` in Lua. Under `Client`, `onServer` is `Bool(false)`
    /// — so `onServer == nil` is provably FALSE and the `else` is the live
    /// branch. Reading Bool falsiness as nil-ness (the old `value_truth`
    /// route) proved the guard TRUE and pruned the wrong arm.
    #[test]
    fn nil_comparison_does_not_read_bool_false_as_nil() {
        let module = parse(
            r#"
            local RunService = game:GetService("RunService")
            local onServer = RunService:IsServer()
            local function get()
                if onServer == nil then
                    return game:GetService("ReplicatedStorage").Modules.A
                else
                    return game:GetService("ReplicatedStorage").Modules.B
                end
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name_with_ctx(&module, Ctx::Client, "t"), vec![rs_b()]);
        assert_eq!(eval_name_with_ctx(&module, Ctx::Server, "t"), vec![rs_b()]);
    }

    /// The mirror: `~= nil` on a `Bool(false)` is TRUE, so its arm is the
    /// live one.
    #[test]
    fn non_nil_comparison_holds_for_bool_false() {
        let module = parse(
            r#"
            local RunService = game:GetService("RunService")
            local onServer = RunService:IsServer()
            local function get()
                if onServer ~= nil then
                    return game:GetService("ReplicatedStorage").Modules.A
                end
                return game:GetService("ReplicatedStorage").Modules.B
            end
            local t = get()
            "#,
        );
        assert_eq!(eval_name_with_ctx(&module, Ctx::Client, "t"), vec![rs_a()]);
        assert_eq!(eval_name_with_ctx(&module, Ctx::Server, "t"), vec![rs_a()]);
    }

    /// `export_root` and `rules::calls::exported_name` must not disagree
    /// about what a module exports: `return script` is an INSTANCE export,
    /// not a table named `script`.
    #[test]
    fn exported_name_agrees_with_export_root() {
        let script = parse("return script");
        assert_eq!(export_root(&script), ExportRoot::Instance);
        assert_eq!(crate::rules::calls::exported_name(&script), None);

        let table = parse("local M = {}\nreturn M");
        assert_eq!(export_root(&table), ExportRoot::Table("M".to_string()));
        assert_eq!(crate::rules::calls::exported_name(&table), Some("M".to_string()));

        let none = parse("return 1");
        assert_eq!(export_root(&none), ExportRoot::None);
        assert_eq!(crate::rules::calls::exported_name(&none), None);
    }
}
