//! R0501 / R0502: require resolution — broken requires and dynamic requires.
//!
//! Project-mode only: without a resolver there is no claim to make, so
//! `check_source` never runs this. Every `require(...)` call site in the
//! module is interpreted through the project's `Resolver`; the four-way
//! `Resolution` decides:
//!
//! Unverifiable-at-check-time FAILS by default —
//! each class has its own code so rime.toml can downgrade them separately:
//!
//!   Found       -> silence (and the calls machinery gets an interface)
//!   Missing     -> R0501. The path was fully interpreted and nothing is
//!                  there — at runtime the require throws (or hangs on a
//!                  WaitForChild) the moment the module loads. Conservative:
//!                  a loader can clone instances in at runtime, and Rime
//!                  says so rather than claiming proof.
//!   OutsideView -> R0503, conservative. A static path into a subtree the
//!                  project does not mount (place-managed instances) — Rime
//!                  cannot verify it, so it fails until downgraded.
//!   Dynamic     -> R0502, conservative. Banned by dialect (settled: every
//!                  cross-file edge resolves statically — that is what call
//!                  conformance and cheap incrementality are built on), but
//!                  the resolver's own shape gaps land here too, so it must
//!                  never masquerade as a proven error.
//!
//! Stub requires (the lazy-require idiom, R0506 + R0501 variants):
//! `local X = false and require(P)` never executes — it documents a cyclic
//! dependency at the top of the file and gives the type checker something
//! to chew on, while a live `X = require(P)` somewhere else actually loads
//! the module. `nil and require(P)` is the same idiom. Consequences:
//!
//!   - a BROKEN stub whose live twin uses the SAME path stays silent — the
//!     live require reports the path once
//!   - a broken stub whose live twin uses a DIFFERENT path is stale
//!     documentation: R0501, note cites the live path
//!   - a broken stub with no live assignment is a dead reference: R0501,
//!     but the note must not claim a runtime throw — the guard means it
//!     never runs
//!   - a RESOLVING stub whose variable is never assigned but IS read at
//!     runtime is R0506: the module is never loaded and every read sees
//!     `false` (conservative — `false and require` doubles as a deliberate
//!     kill-switch, and Rime cannot read intent). No runtime reads means
//!     the stub is a type-only import (`type T = Var.T` — type positions
//!     are invisible to HIR) and stays silent.
//!
//! Matching is by variable NAME within the file (no scope resolution): a
//! shadowing local of the same name counts as an assignment, which errs
//! toward silence.
//!
//! `require` means the builtin only where the name reaches it. A file that
//! rebinds the name (`local require = ...`, a parameter) has its calls
//! through the new binding left alone: they are that file's own function.
//! `Module::is_require` answers it from Luau scoping; such calls come back
//! as `RequireNote::Rebound`, with no diagnostic.
//!
//! Once-per-run notes: a require the resolver recognises and does not
//! follow (`Resolution::Unfollowed`) is not a defect at that site and not
//! a shape Rime failed on. It gets no diagnostic; `check` hands the site
//! and the reason back in `notes`, and the run prints each reason once.
//! One-diagnostic-per-occurrence still holds for every defect: a module
//! that is not there is its own R0501 every time.

use crate::eval::{Evaluator, Outcome};
use crate::project::{RequireNote, Resolution, Resolver};
use rime_diag::{Code, Diagnostic, Label, Span, Suggestion};
use rime_hir as hir;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Which falsy literal guards a stub. Either is the idiom, and which one a
/// project writes is not reported; the kind is kept so the notes quote the
/// program as written. (0 and "" are truthy in Lua — these two are
/// exhaustive.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Guard {
    False,
    Nil,
}

impl Guard {
    /// The literal as written — both the guard text in notes
    /// (`guarded by \`nil and\``) and the value reads see (`always \`nil\``).
    fn text(self) -> &'static str {
        match self {
            Guard::False => "false",
            Guard::Nil => "nil",
        }
    }
}

/// A `false and require(...)` / `nil and require(...)` initializer: the
/// guarded require's call span, keyed back to the declared name by the
/// caller.
struct Stub {
    name: String,
    span: Span,
    guard: Guard,
}

/// One `X = require(...)` assignment, pre-resolved and pre-described while
/// the HIR borrow was available.
struct LiveRequire {
    name: String,
    /// The resolver's description when THIS path is also broken — the
    /// stub-suppression comparison key.
    missing_described: Option<String>,
    /// Source form of a static target chain, for the stale-stub note.
    path_text: Option<String>,
    span: Span,
}

// `pub(crate)`, not `pub` like every other rule module's `check` — the
// `eval: Option<&Evaluator<'_>>` parameter carries `eval.rs`'s deliberately
// `pub(crate)` `Evaluator` (an internal implementation detail, not part of
// this crate's public surface), and `rules` is itself a `pub mod`, so a
// `pub fn` here would leak a `pub(crate)` type through a public signature
// (`private_interfaces` lint). Matches actual reachability: `session.rs` is
// this function's only caller, in-crate.
pub(crate) fn check(
    module: &hir::Module,
    path: &Path,
    resolver: &dyn Resolver,
    eval: Option<&Evaluator<'_>>,
    env: &HashMap<String, &hir::Expr>,
    src: &str,
    diags: &mut Vec<Diagnostic>,
    notes: &mut Vec<(Span, RequireNote)>,
) {
    let mut sites = Vec::new();
    let mut enum_vars = Vec::new();
    let mut walk = Walk { module, enum_vars: &mut enum_vars, out: &mut sites, rebound: Vec::new() };
    for body in &module.bodies {
        walk.block(&body.block);
        debug_assert!(walk.enum_vars.is_empty());
    }
    notes.extend(walk.rebound.into_iter().map(|span| (span, RequireNote::Rebound)));

    // Stub declarations, every assigned name, and every `X = require(...)`
    // live assignment — the three facts the stub rules pivot on.
    let mut stubs: Vec<Stub> = Vec::new();
    let mut assigned: HashSet<String> = HashSet::new();
    let mut reads: HashSet<String> = HashSet::new();
    let mut live: Vec<LiveRequire> = Vec::new();
    for body in &module.bodies {
        hir::visit::exprs(body, &mut |e| {
            if let hir::Expr::Name { name, .. } = e {
                reads.insert(name.clone());
            }
        });
        hir::visit::stmts(body, &mut |stmt| match &stmt.kind {
            hir::StmtKind::Local { decls, init } => {
                for (decl, e) in decls.iter().zip(init) {
                    if let Some((span, guard)) = stub_guard(module, e) {
                        stubs.push(Stub {
                            name: decl.name.clone(),
                            span,
                            guard,
                        });
                    }
                }
            }
            hir::StmtKind::Assign { targets, values } => {
                for (i, t) in targets.iter().enumerate() {
                    let hir::Target::Name { name, .. } = t else {
                        continue;
                    };
                    assigned.insert(name.clone());
                    if let Some((target, span)) =
                        values.get(i).and_then(|v| direct_require(module, v))
                    {
                        // Resolve and describe now: the visitor's borrows
                        // cannot leave the closure. Re-asks the evaluator on
                        // Dynamic too (`resolve_with_eval`), same as the
                        // main sites loop below — a stale-stub live path
                        // that the evaluator resolves to Missing must still
                        // get its `missing_described`, or the stub-vs-live
                        // comparison below would silently stop matching it.
                        let (resolution, _) =
                            resolve_with_eval(resolver, path, module, target, env, eval);
                        let missing_described = match resolution {
                            Resolution::Missing { described } => Some(described),
                            _ => None,
                        };
                        live.push(LiveRequire {
                            name: name.clone(),
                            missing_described,
                            path_text: describe(target),
                            span,
                        });
                    }
                }
            }
            hir::StmtKind::CompoundAssign {
                target: hir::Target::Name { name, .. },
                ..
            } => {
                assigned.insert(name.clone());
            }
            _ => {}
        });
    }

    for (target, span, is_enum) in sites {
        let stub = stubs.iter().find(|s| s.span == span);
        let (resolution, eval_note) = resolve_with_eval(resolver, path, module, target, env, eval);
        match resolution {
            Resolution::Found(_) | Resolution::Trusted => {
                if let Some(note) = resolver.note(path, target) {
                    notes.push((span.clone(), note));
                }
                if let Some(s) = stub {
                    if !assigned.contains(s.name.as_str())
                        && reads.contains(s.name.as_str())
                    {
                        diags.push(never_loaded_diag(&s.name, &span, s.guard));
                    }
                }
            }
            Resolution::Missing { described } => match stub {
                None => {
                    let name = described.rsplit('.').next().unwrap_or("");
                    let found = if name.is_empty() {
                        Vec::new()
                    } else {
                        resolver.candidates(path, name)
                    };
                    let reachable = match found.as_slice() {
                        [only] => resolver.candidate_reachable(path, only),
                        _ => false,
                    };
                    let mut d = missing_diag(&described, &span, &found, reachable);
                    if let Some(note) = &eval_note {
                        d = d.with_note(note.clone());
                    }
                    diags.push(d);
                }
                Some(s) => {
                    let mine: Vec<_> =
                        live.iter().filter(|l| l.name == s.name).collect();
                    let same_path = mine
                        .iter()
                        .any(|l| l.missing_described.as_deref() == Some(&described));
                    if same_path {
                        // The live require reports the broken path itself;
                        // a second error on its documentation is noise.
                    } else if let Some((live_path, line)) =
                        mine.iter().find_map(|l| {
                            l.path_text
                                .as_deref()
                                .map(|d| (d, line_of(src, l.span.start)))
                        })
                    {
                        diags.push(stale_stub_diag(
                            &described, &span, &s.name, live_path, line, s.guard,
                        ));
                    } else {
                        diags.push(dead_stub_diag(&described, &span, s.guard));
                    }
                }
            },
            Resolution::OutsideView => diags.push(outside_diag(&span)),
            Resolution::Unfollowed(note) => notes.push((span, note)),
            Resolution::Dynamic => diags.push(if is_enum {
                enumeration_diag(&span)
            } else {
                dynamic_diag(&span, eval_note.as_deref())
            }),
        }
    }
}

/// Cheap `Resolver::resolve`, with the evaluator re-asked when it
/// gives up (`Resolution::Dynamic`) and an evaluator is available — the ONE
/// place both call sites in `check` (the live-assign Missing probe and the
/// main sites loop) ask, so they always land on the same outcome for the
/// same site. `eval` is `None` for callers that don't care (no project, or
/// a test harness that doesn't wire one) — that path is `resolver.resolve`
/// unchanged.
///
/// `Outcome::Resolved(r)` collapses straight to `r`: `Trusted`/`Found`/
/// `Missing`/`OutsideView` are handled by the caller's existing
/// `Resolution` match exactly like a cheap resolve would, and
/// `Resolution::Dynamic` itself (the evaluator can land there too — a
/// `Bool`/`Unknown` candidate mid-collapse) falls through to the same
/// `Resolution::Dynamic` arm as `NilCapable`/`Diverged`/`Opaque`, just
/// without a note — identical to today's plain `dynamic_diag`.
/// `NilCapable`/`Diverged` carry their explanation onward as `eval_note`,
/// for `dynamic_diag`'s `extra_note`; `Opaque` carries none.
fn resolve_with_eval(
    resolver: &dyn Resolver,
    path: &Path,
    module: &hir::Module,
    target: &hir::Expr,
    env: &HashMap<String, &hir::Expr>,
    eval: Option<&Evaluator<'_>>,
) -> (Resolution, Option<String>) {
    let resolution = resolver.resolve(path, target, env);
    match (resolution, eval) {
        (Resolution::Dynamic, Some(ev)) => match ev.eval_require_target(path, module, target) {
            // A Missing PROVEN by the evaluator gets a provenance note: the
            // rendered path was interpreted through another module's
            // exported values, not read off a literal chain — without the
            // note a reader can't tell an evaluated resolution from a cheap
            // one, and a false R0501 here would be unreportable.
            Outcome::Resolved(r @ Resolution::Missing { .. }) => (
                r,
                Some(
                    "path interpreted through module-exported values \
                     (see the required module's export)"
                        .to_string(),
                ),
            ),
            Outcome::Resolved(r) => (r, None),
            Outcome::NilCapable { why } => {
                (Resolution::Dynamic, Some(format!("target can be nil: {why}")))
            }
            Outcome::Diverged { files } => (
                Resolution::Dynamic,
                Some(format!("resolves to {}, which differ", files.join(" / "))),
            ),
            Outcome::Opaque => (Resolution::Dynamic, None),
        },
        (r, _) => (r, None),
    }
}

/// `false and require(...)` / `nil and require(...)` — the guarded-stub
/// initializer shapes. Returns the require's call span and the guard kind.
fn stub_guard(module: &hir::Module, e: &hir::Expr) -> Option<(Span, Guard)> {
    let hir::Expr::BinOp { op: hir::BinOp::And, lhs, rhs } = e else {
        return None;
    };
    let guard = match &**lhs {
        hir::Expr::Literal { prim: hir::Prim::Boolean, text: Some(t), .. } if t == "false" => {
            Guard::False
        }
        hir::Expr::Literal { prim: hir::Prim::Nil, .. } => Guard::Nil,
        _ => return None,
    };
    direct_require(module, rhs).map(|(_, span)| (span, guard))
}

/// A bare `require(<target>)` call on the builtin: its target and call span.
fn direct_require<'e>(module: &hir::Module, e: &'e hir::Expr) -> Option<(&'e hir::Expr, Span)> {
    let hir::Expr::Call { base, args, method: None, span, .. } = e else {
        return None;
    };
    if !module.is_require(base) {
        return None;
    }
    Some((args.first()?, span.clone()))
}

/// Source form of a static `a.b.c` chain, for citing the live require's
/// path in a note. Non-static shapes (computed keys, calls) yield None.
fn describe(e: &hir::Expr) -> Option<String> {
    match e {
        hir::Expr::Name { name, .. } => Some(name.clone()),
        hir::Expr::Index { base, key: None, name: Some(n), .. } => {
            Some(format!("{}.{}", describe(base)?, n))
        }
        _ => None,
    }
}

fn line_of(src: &str, offset: usize) -> usize {
    src.as_bytes()[..offset.min(src.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

const MOUNTS_NOTE: &str =
    "the path was followed through the project mounts and no module \
     file is at the destination; if something puts an instance there \
     at runtime, Rime cannot see it — downgrade `broken-require` if \
     this pattern is deliberate";

/// `reachable`: a lone candidate can be required from everywhere this file
/// runs, so its path may be written over the broken one.
fn missing_diag(
    described: &str,
    span: &Span,
    candidates: &[String],
    reachable: bool,
) -> Diagnostic {
    let name = described.rsplit('.').next().unwrap_or(described);
    let mut d = Diagnostic::error(Code(501), "cannot resolve this require")
        .with_label(Label::primary(
            span.clone(),
            format!("nothing exists at `{described}`"),
        ))
        .conservative(MOUNTS_NOTE)
        .with_note(
            "if nothing creates it, this require throws (or waits forever) \
             the moment the module loads",
        )
        .with_help("fix the path, or add the missing module to the project");
    match candidates {
        [] => {}
        // Exactly one module carries the name: name it, and offer a real
        // replacement (the span covers `(target)`, parens included).
        [only] if reachable => {
            d = d
                .with_note(format!(
                    "a module named `{name}` exists at `{only}` — the path \
                     may have rotted, not the module"
                ))
                .with_suggestion(Suggestion {
                    message: format!("replace the path with `{only}`"),
                    span: span.clone(),
                    replacement: format!("({only})"),
                });
        }
        // The one module with the name is where this file cannot load it
        // from (server-only, and this file can run on a client). Name it;
        // writing its path here would only move the failure to runtime.
        [only] => {
            d = d.with_note(format!(
                "a module named `{name}` exists at `{only}`, but this file \
                 cannot require it from everywhere it runs — no edit is \
                 offered"
            ));
        }
        // Several candidates: list them, fix nothing — a wrong guess
        // written into the file is worse than no fix.
        many => {
            let list = many
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ");
            d = d.with_note(format!(
                "modules named `{name}` exist at {list} — the path may have \
                 rotted, not the module"
            ));
        }
    }
    d
}

fn stale_stub_diag(
    described: &str,
    span: &Span,
    name: &str,
    live_path: &str,
    live_line: usize,
    guard: Guard,
) -> Diagnostic {
    Diagnostic::error(Code(501), "cannot resolve this require")
        .with_label(Label::primary(
            span.clone(),
            format!("nothing exists at `{described}`"),
        ))
        .conservative(MOUNTS_NOTE)
        .with_note(format!(
            "guarded by `{} and`, this require never runs — it is a stub \
             for `{name}`, but the live require at line {live_line} loads \
             `{live_path}`, not this path",
            guard.text()
        ))
        .with_help("update the stub to match the live require")
}

fn dead_stub_diag(described: &str, span: &Span, guard: Guard) -> Diagnostic {
    Diagnostic::error(Code(501), "cannot resolve this require")
        .with_label(Label::primary(
            span.clone(),
            format!("nothing exists at `{described}`"),
        ))
        .conservative(MOUNTS_NOTE)
        .with_note(format!(
            "guarded by `{} and`, this require never runs — nothing \
             throws at runtime, but the path points at nothing: a dead \
             reference",
            guard.text()
        ))
        .with_help("fix the path, or delete the stub")
}

fn never_loaded_diag(name: &str, span: &Span, guard: Guard) -> Diagnostic {
    Diagnostic::error(
        Code(506),
        format!(
            "this module is never loaded — `{name}` is always `{}`",
            guard.text()
        ),
    )
    .with_label(Label::primary(
        span.clone(),
        format!("guarded by `{} and`, this require never runs", guard.text()),
    ))
    .conservative(format!(
        "the lazy-require idiom needs a live assignment (`{name} = \
         require(...)`) somewhere in the file; there is none, so every use \
         of `{name}` sees `{}`",
        guard.text()
    ))
    .with_help(
        "add the live require, or delete the stub if the feature \
         is dead",
    )
}

fn outside_diag(span: &Span) -> Diagnostic {
    Diagnostic::error(Code(503), "require outside the project's view")
        .with_label(Label::primary(
            span.clone(),
            "this path leads into a subtree the project does not mount",
        ))
        .conservative(
            "Rime can only verify requires within the Rojo mounts; \
             unverifiable-at-check-time fails by default — downgrade \
             `require-outside-project` if this subtree is managed elsewhere",
        )
        .with_help(
            "mount the subtree in default.project.json, or move the \
             module under a mounted path",
        )
}

fn enumeration_diag(span: &Span) -> Diagnostic {
    Diagnostic::error(
        Code(508),
        "require target is a loop variable over a folder's children",
    )
    .with_label(Label::primary(
        span.clone(),
        "each child of the folder is required here",
    ))
    .conservative(
        "the loop requires whatever the folder holds at runtime; Rime does \
         not yet enumerate mounted folders statically — downgrade \
         `enumeration-require` in rime.toml if this registry pattern is \
         deliberate",
    )
    .with_help(
        "keep the loop and downgrade `enumeration-require`, or \
         require each module statically",
    )
}

/// `extra_note`: the evaluator's own explanation when it re-ran a `Dynamic`
/// site and landed on `NilCapable`/`Diverged` (`resolve_with_eval`) —
/// appended as one more note, after the escape-hatch note below. `None` for
/// every pre-evaluator caller, and for `Opaque`/a plain unresolved `Dynamic`.
fn dynamic_diag(span: &Span, extra_note: Option<&str>) -> Diagnostic {
    let d = Diagnostic::error(Code(502), "require target is not statically resolvable")
        .with_label(Label::primary(
            span.clone(),
            "Rime cannot follow this require",
        ))
        .conservative(
            "every cross-file edge must resolve statically — that is what \
             cross-file checking is built on; a computed target (or a shape \
             the resolver does not model) breaks the graph here",
        )
        // R0502 only: "dynamic on purpose" is a normal answer here (loader
        // loops, content-keyed modules), so the diagnostic teaches its own
        // escape hatch. R0501 deliberately does NOT — advertising allow on
        // broken paths invites suppressing rot.
        .with_note(
            "dynamic on purpose (a loader over children, content-keyed \
             modules)? keep it and add `-- rime: allow(dynamic-require, \
             \"why\")` on or above this line — the reason is mandatory",
        )
        .with_help(
            "require each candidate statically and pick between the \
             bound results",
        );
    match extra_note {
        Some(note) => d.with_note(note),
        None => d,
    }
}

/// Every `require(<target>)` call site, with the call span. A bare
/// `require()` with no argument is nonsense; it surfaces as Dynamic via a
/// `Leaf` target.
type Site<'m> = (&'m hir::Expr, Span, bool);

/// `for _, v in <folder>:GetChildren() do` — the enumeration iterator
/// shapes: a bare GetChildren/GetDescendants method call, optionally
/// wrapped in `ipairs(...)`/`pairs(...)`. Anything looser stays R0502.
fn is_children_enum(exprs: &[hir::Expr]) -> bool {
    fn children_call(e: &hir::Expr) -> bool {
        matches!(e, hir::Expr::Call { method: Some(m), args, .. }
            if (m == "GetChildren" || m == "GetDescendants") && args.is_empty())
    }
    let Some(first) = exprs.first() else {
        return false;
    };
    children_call(first)
        || matches!(first, hir::Expr::Call { base, args, method: None, .. }
            if matches!(&**base, hir::Expr::Name { name, .. }
                if name == "ipairs" || name == "pairs")
                && args.len() == 1
                && children_call(&args[0]))
}

/// The site walk's state: what it collects, and the module it asks whether
/// a `require` name is the builtin.
struct Walk<'m, 'w> {
    module: &'m hir::Module,
    enum_vars: &'w mut Vec<String>,
    out: &'w mut Vec<Site<'m>>,
    /// Call spans of calls through a rebound `require`.
    rebound: Vec<Span>,
}

impl<'m> Walk<'m, '_> {
    fn block(&mut self, block: &'m hir::Block) {
        use hir::StmtKind as K;
        for stmt in &block.stmts {
            match &stmt.kind {
                K::Local { init, .. } => init.iter().for_each(|e| self.expr(e)),
                K::Assign { targets, values } => {
                    for t in targets {
                        if let hir::Target::Projection(e) = t {
                            self.expr(e);
                        }
                    }
                    values.iter().for_each(|e| self.expr(e));
                }
                K::CompoundAssign { target, value } => {
                    if let hir::Target::Projection(e) = target {
                        self.expr(e);
                    }
                    self.expr(value);
                }
                K::Call(e) => self.expr(e),
                K::Do(b) => self.block(b),
                K::If { cond, then, else_ifs, or_else } => {
                    self.expr(cond);
                    self.block(then);
                    for (c, b) in else_ifs {
                        self.expr(c);
                        self.block(b);
                    }
                    if let Some(b) = or_else {
                        self.block(b);
                    }
                }
                K::While { cond, body } => {
                    self.expr(cond);
                    self.block(body);
                }
                K::Repeat { body, until } => {
                    self.block(body);
                    self.expr(until);
                }
                K::NumericFor { start, end, step, body, .. } => {
                    self.expr(start);
                    self.expr(end);
                    if let Some(s) = step {
                        self.expr(s);
                    }
                    self.block(body);
                }
                K::GenericFor { vars, exprs, body } => {
                    exprs.iter().for_each(|e| self.expr(e));
                    // Loop vars over a children enumeration mark their requires
                    // as R0508 registry sites. A shadowing inner `local` of the
                    // same name inside the body is not modelled — that misfile
                    // only ever downgrades an R0502 to the (also conservative)
                    // R0508, never invents a resolution.
                    let is_enum = is_children_enum(exprs);
                    if is_enum {
                        vars.iter().for_each(|v| self.enum_vars.push(v.name.clone()));
                    }
                    self.block(body);
                    if is_enum {
                        self.enum_vars.truncate(self.enum_vars.len() - vars.len());
                    }
                }
                // Nested bodies are iterated by `check`'s body loop.
                K::LocalFunction { .. } | K::FunctionDecl { .. } => {}
            }
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            exprs.iter().for_each(|e| self.expr(e));
        }
    }

    fn expr(&mut self, expr: &'m hir::Expr) {
        use hir::Expr as E;
        match expr {
            E::Call { base, args, method: None, span, .. } => {
                if self.module.is_require(base) {
                    static NO_ARG: hir::Expr = hir::Expr::Leaf;
                    let target = args.first().unwrap_or(&NO_ARG);
                    let is_enum = matches!(target, E::Name { name, .. }
                        if self.enum_vars.iter().any(|v| v == name));
                    self.out.push((target, span.clone(), is_enum));
                } else if self.module.is_rebound_require(base) {
                    self.rebound.push(span.clone());
                }
                self.expr(base);
                args.iter().for_each(|a| self.expr(a));
            }
            E::Call { base, args, .. } => {
                self.expr(base);
                args.iter().for_each(|a| self.expr(a));
            }
            E::Index { base, key, .. } => {
                self.expr(base);
                if let Some(k) = key {
                    self.expr(k);
                }
            }
            E::BinOp { lhs, rhs, .. } => {
                self.expr(lhs);
                self.expr(rhs);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => self.expr(inner),
            E::Table(fields) => {
                for f in fields {
                    match f {
                        hir::TableField::Keyed { key, value } => {
                            self.expr(key);
                            self.expr(value);
                        }
                        hir::TableField::Named { value, .. } => self.expr(value),
                        hir::TableField::Item(e) => self.expr(e),
                    }
                }
            }
            E::IfExpr(arms) => {
                for (cond, val) in arms {
                    if let Some(c) = cond {
                        self.expr(c);
                    }
                    self.expr(val);
                }
            }
            E::Composite(parts) => parts.iter().for_each(|p| self.expr(p)),
            E::Name { .. } | E::Function(_) | E::Literal { .. } | E::Leaf => {}
        }
    }
}
