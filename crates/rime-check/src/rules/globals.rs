//! R0022–R0024: the globals family — selene's `undefined_variable`,
//! `unscoped_variables`, and `global_usage` lints.
//!
//!   R0022 undefined-variable  a READ that resolves to no enclosing scope,
//!                             no std name, and no global write anywhere
//!                             else in the file (selene parity: a file's
//!                             own global writes define the name — the
//!                             write is R0023's problem, the read is fine)
//!   R0023 unscoped-variable   a WRITE to a name with no `local` in any
//!                             enclosing scope — an implicit global.
//!                             `^_`-prefixed names are exempt (selene's
//!                             default `ignore_pattern`, hardcoded here;
//!                             no regex config)
//!   R0024 global-usage        any use of `_G`/`shared`, Luau's two ambient
//!                             mutable-global handles, not shadowed by a
//!                             local
//!
//! All three rules are implemented by this module. R0024's occurrences are
//! partitioned out of the event stream before R0022/R0023 classification
//! (see `check_module`) rather than added as another arm over the full
//! stream — the family is split across separate `classify_*` functions
//! (one loop each), so exclusivity (one occurrence, one diagnostic) has to
//! be structural, not a `continue` inside a shared loop. See
//! `classify_global_usage`'s doc comment for why `_G`'s `^_` prefix and
//! `shared`'s std-membership are not enough on their own.
//!
//! The same walk also feeds R0043 const-reassign (a write that resolves to
//! a `const` binding) — not a globals rule, but the same question asked of
//! the same scope chain; see `classify_const_reassign`.
//!
//! Why a module-level walk, not the per-body CFG (`crate::cfg`): whether a
//! name is global is a CROSS-BODY question — a name free in a closure may
//! resolve in an enclosing function's locals, and bodies are otherwise
//! checked independently (`crate::check_body` builds one CFG per body, with
//! no visibility into an enclosing body's scope). `cfg.rs`'s own
//! `target_write` already defers exactly this case to here (see its
//! comment). This module is a second, self-contained scope-stack walk over
//! the whole module for that reason — see the cross-reference with
//! `capture::free_names` at `walk_block` below for why it is not that
//! function wearing a different hat.
//!
//! Severity — settled on large-codebase survey evidence,
//! the same way the suspicious/style split and the UDim2-bounds kill were
//! settled. All three are `conservative`: every one of them flags
//! valid Luau.
//!
//! - R0022 `error`. A handful of first-party hits, none of them a rule bug:
//!   they name `PluginManager` (a plugin-only Roblox global absent from
//!   selene's generated roblox.yml too — that codebase suppresses it for
//!   selene as well) or are a genuinely undefined read. The false-positive
//!   class is framework-injected globals, which `[std] globals` declares away.
//! - R0023 `error`. Almost every first-party hit was a real defect —
//!   including a live one (a function whose parameter was dropped in a
//!   refactor, leaving the body defaulting a global no caller can set) and
//!   a name whose declaration-block siblings are all `local`. The rest
//!   are 2010-era free-model script style, which `local function` fixes.
//!   Not deliberate-intent-dominated, so it keeps error severity.
//! - R0024 `warning`. Every hit was deliberate: most in one Studio-plugin
//!   macro that carries `--# selene: allow(global_usage)` and a comment
//!   explaining that `_G` is what survives a Rojo re-sync, the rest in vendored
//!   test-harness code that keeps its environment in `_G`. Dominated by
//!   deliberate intent, so it takes the style-family downgrade.

use rime_diag::{Code, Diagnostic, Label};
use rime_hir as hir;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameUse {
    Read,
    Write,
}

/// One occurrence of a name that did not resolve in the lexical scope chain
/// at the point it was seen.
#[derive(Debug, Clone)]
struct NameEvent {
    name: String,
    span: hir::Span,
    use_: NameUse,
}

/// One lexical scope: each name declared in it, with the span of its
/// declaration when that declaration is a `const` (`None` for a `local`, a
/// parameter, a loop variable). A later declaration of the same name in the
/// same scope replaces the earlier one, as it does at runtime.
type Scope = HashMap<String, Option<hir::Span>>;

/// A write that resolved — to a `const`. The one kind of RESOLVED occurrence
/// the walk records; R0043's input (see `classify_const_reassign`).
#[derive(Debug, Clone)]
struct ConstWrite {
    name: String,
    /// The write: the assignment target, or the whole `function name()`
    /// declaration.
    span: hir::Span,
    /// The name in the `const` declaration the write reaches.
    declared: hir::Span,
}

/// Everything the one walk collects.
#[derive(Default)]
struct Walk {
    /// Unresolved names, R0022–R0024's input.
    events: Vec<NameEvent>,
    const_writes: Vec<ConstWrite>,
}

/// Entry point: one module-level walk collecting every unresolved name
/// occurrence (and every write to a `const`), then a classification pass
/// per rule.
///
/// `std` is unused by R0023 (a global WRITE needs no std-membership check)
/// — R0022 consumes it below to decide whether an unresolved READ names a
/// known std/Roblox global.
pub fn check_module(module: &hir::Module, std: &rime_std::StdNames, diags: &mut Vec<Diagnostic>) {
    let mut walk = Walk::default();
    walk_block(
        module,
        &module.body(module.root).block,
        &mut Vec::new(),
        &mut walk,
    );
    let Walk { events, const_writes } = walk;
    classify_const_reassign(&const_writes, diags);

    // R0024 owns every `_G`/`shared` occurrence exclusively: partitioned
    // out of the stream BEFORE R0022/R0023 classification, so one
    // occurrence yields exactly one diagnostic. This has to happen here,
    // not as a per-rule name check inside each `classify_*`, because
    // `_G`/`shared` are not reliably excluded otherwise: `_G`'s `^_`
    // prefix happens to dodge R0023 already, but `shared` carries no such
    // prefix, and std-membership (which silences R0022 reads) says nothing
    // about R0023 writes — `shared = {}` would otherwise double-report as
    // both R0023 (unscoped write) and R0024. See `classify_global_usage`.
    let (global_events, other_events): (Vec<NameEvent>, Vec<NameEvent>) = events
        .into_iter()
        .partition(|e| e.name == "_G" || e.name == "shared");

    // A file's own global writes define the name for R0022's purposes,
    // regardless of where in the file they occur relative to a read
    // (selene parity — see `classify_undefined_variable`'s doc comment).
    // Built from `other_events` (post-partition): `_G`/`shared` can never
    // be looked up here since they never appear in `other_events`, so
    // scoping this to the partitioned stream costs nothing and keeps the
    // partition's exclusivity visible at every use site.
    let written: HashSet<&str> = other_events
        .iter()
        .filter(|e| e.use_ == NameUse::Write)
        .map(|e| e.name.as_str())
        .collect();

    // Classification: each rule reads its own slice of the one walk above.
    classify_unscoped_variable(&other_events, diags);
    classify_undefined_variable(&other_events, &written, std, diags);
    classify_global_usage(&global_events, diags);
}

// --- the walk ---------------------------------------------------------------

/// Mirrors `capture::free_names` (cross-reference: see that function's doc
/// comment) — same scope-stack shape, same descend-into-closures-at-
/// creation-sites order — but records `(name, span, Read|Write)` per
/// occurrence instead of merging everything into one name set. Do NOT try
/// to reuse `free_names` directly: it collects bare names with no spans and
/// merges reads and writes, which is the wrong output shape for a rule that
/// must point at a site and tell a write from a read. Fold the two only if
/// a third client needs yet another output shape from the same traversal.
fn walk_block(
    module: &hir::Module,
    block: &hir::Block,
    scopes: &mut Vec<Scope>,
    out: &mut Walk,
) {
    scopes.push(Scope::new());
    walk_stmts(module, block, scopes, out);
    scopes.pop();
}

/// `walk_block`'s body without its own push/pop — factored out so `Repeat`
/// can walk the loop body's statements, keep that scope open, evaluate
/// `until` inside it, and pop exactly once (see the `Repeat` arm below).
fn walk_stmts(
    module: &hir::Module,
    block: &hir::Block,
    scopes: &mut Vec<Scope>,
    out: &mut Walk,
) {
    for stmt in &block.stmts {
        walk_stmt(module, stmt, scopes, out);
    }
    if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
        exprs.iter().for_each(|e| expr(module, e, scopes, out));
    }
}

fn walk_stmt(
    module: &hir::Module,
    stmt: &hir::Stmt,
    scopes: &mut Vec<Scope>,
    out: &mut Walk,
) {
    use hir::StmtKind as K;
    match &stmt.kind {
        K::Local { decls, init } => {
            init.iter().for_each(|e| expr(module, e, scopes, out));
            for d in decls {
                declare(scopes, d);
            }
        }
        K::Assign { targets, values } => {
            values.iter().for_each(|e| expr(module, e, scopes, out));
            targets
                .iter()
                .for_each(|t| target_write(module, t, scopes, out));
        }
        K::CompoundAssign { target, value } => {
            // Write only — a compound assign reads-then-writes at runtime,
            // but this occurrence gets exactly one event, and so at most
            // one diagnostic if it classifies as an unscoped global. See
            // the module doc's walker-requirements note and corpus case
            // tests/globals/reject-conservative/003-compound-assign-global.luau.
            expr(module, value, scopes, out);
            target_write(module, target, scopes, out);
        }
        K::Call(e) => expr(module, e, scopes, out),
        K::Do(b) => walk_block(module, b, scopes, out),
        K::If {
            cond,
            then,
            else_ifs,
            or_else,
        } => {
            expr(module, cond, scopes, out);
            walk_block(module, then, scopes, out);
            for (c, b) in else_ifs {
                expr(module, c, scopes, out);
                walk_block(module, b, scopes, out);
            }
            if let Some(b) = or_else {
                walk_block(module, b, scopes, out);
            }
        }
        K::While { cond, body } => {
            expr(module, cond, scopes, out);
            walk_block(module, body, scopes, out);
        }
        K::Repeat { body, until } => {
            // Real Lua: `until` sees the body's own locals. Walk the body's
            // statements WITHOUT popping its scope, evaluate `until` inside
            // it, then pop once. Contrast `free_names`, which pops the body
            // scope before walking `until` — a harmless over-approximation
            // for capture (it can only over-poison a narrowing or over-count
            // a use), but a real false undefined-variable/global-write here.
            // Locked by tests/globals/accept/002-locals-upvalues-repeat.luau.
            scopes.push(Scope::new());
            walk_stmts(module, body, scopes, out);
            expr(module, until, scopes, out);
            scopes.pop();
        }
        K::NumericFor {
            var,
            start,
            end,
            step,
            body,
        } => {
            expr(module, start, scopes, out);
            expr(module, end, scopes, out);
            if let Some(s) = step {
                expr(module, s, scopes, out);
            }
            scopes.push(Scope::from([(var.name.clone(), None)]));
            walk_block(module, body, scopes, out);
            scopes.pop();
        }
        K::GenericFor { vars, exprs, body } => {
            exprs.iter().for_each(|e| expr(module, e, scopes, out));
            scopes.push(vars.iter().map(|v| (v.name.clone(), None)).collect());
            walk_block(module, body, scopes, out);
            scopes.pop();
        }
        K::LocalFunction { decl, body } => {
            // Bind the name BEFORE walking the closure body so self-
            // recursion resolves (`local function f() f() end`).
            declare(scopes, decl);
            let closure = module.body(*body);
            scopes.push(param_scope(closure, false));
            walk_block(module, &closure.block, scopes, out);
            scopes.pop();
        }
        K::FunctionDecl {
            root,
            is_method,
            assigns_root,
            body,
            ..
        } => {
            // `assigns_root` is true only for the bare single-name form
            // (`path` empty — see rime-syntax's `FunctionDeclaration`
            // lowering); it never co-occurs with `is_method`, since a
            // method decl always carries at least the method-name path
            // segment, so `path` is never empty for one.
            if *assigns_root {
                // `function a() end` sugars to `a = function() end` — a
                // WRITE to `a`, not a read. `FunctionDecl` carries no
                // per-name span (only the whole-declaration `stmt.span`);
                // `rules::calls`'s exported-function-reassign rule points
                // at the same span for the same reason.
                touch(scopes, root, &stmt.span, NameUse::Write, out);
            } else {
                // `function a.b.c()` / `function a:m()` — projects into
                // `a` to reach the declared slot, which READS the root.
                touch(scopes, root, &stmt.span, NameUse::Read, out);
            }
            let closure = module.body(*body);
            scopes.push(param_scope(closure, *is_method));
            walk_block(module, &closure.block, scopes, out);
            scopes.pop();
        }
    }
}

fn expr(
    module: &hir::Module,
    e: &hir::Expr,
    scopes: &mut Vec<Scope>,
    out: &mut Walk,
) {
    use hir::Expr as E;
    match e {
        E::Name { name, span } => touch(scopes, name, span, NameUse::Read, out),
        E::Function(id) => {
            let closure = module.body(*id);
            scopes.push(param_scope(closure, false));
            walk_block(module, &closure.block, scopes, out);
            scopes.pop();
        }
        E::Call { base, args, .. } => {
            expr(module, base, scopes, out);
            args.iter().for_each(|a| expr(module, a, scopes, out));
        }
        E::Index { base, key, .. } => {
            expr(module, base, scopes, out);
            if let Some(k) = key {
                expr(module, k, scopes, out);
            }
        }
        E::BinOp { lhs, rhs, .. } => {
            expr(module, lhs, scopes, out);
            expr(module, rhs, scopes, out);
        }
        E::Not(inner) | E::UnOp { operand: inner, .. } => expr(module, inner, scopes, out),
        E::Table(fields) => {
            for f in fields {
                match f {
                    hir::TableField::Keyed { key, value } => {
                        expr(module, key, scopes, out);
                        expr(module, value, scopes, out);
                    }
                    hir::TableField::Named { value, .. } => expr(module, value, scopes, out),
                    hir::TableField::Item(e) => expr(module, e, scopes, out),
                }
            }
        }
        E::IfExpr(arms) => {
            for (cond, val) in arms {
                if let Some(c) = cond {
                    expr(module, c, scopes, out);
                }
                expr(module, val, scopes, out);
            }
        }
        E::Composite(parts) => parts.iter().for_each(|p| expr(module, p, scopes, out)),
        E::Literal { .. } | E::Leaf => {}
    }
}

fn target_write(
    module: &hir::Module,
    t: &hir::Target,
    scopes: &mut Vec<Scope>,
    out: &mut Walk,
) {
    match t {
        hir::Target::Name { name, span } => touch(scopes, name, span, NameUse::Write, out),
        // `t.x = v` mutates a table, it does not write a bare name — but it
        // DOES read `t` (and any bracket key), same as `free_names::target`.
        hir::Target::Projection(e) => expr(module, e, scopes, out),
    }
}

/// Bind a declared name in the innermost scope.
fn declare(scopes: &mut [Scope], decl: &hir::LocalDecl) {
    let declared = decl.is_const.then(|| decl.span.clone());
    scopes.last_mut().unwrap().insert(decl.name.clone(), declared);
}

/// The binding `name` refers to here — the innermost scope that declares
/// it wins. `None`: no scope does. `Some(None)`: an assignable binding.
/// `Some(Some(span))`: a `const`, declared at `span`.
fn resolve<'s>(scopes: &'s [Scope], name: &str) -> Option<&'s Option<hir::Span>> {
    scopes.iter().rev().find_map(|s| s.get(name))
}

/// Record `name` as unresolved, unfiltered — `_G`/`shared` and `_`-prefixed
/// names are recorded here too; nothing about a name changes whether it is
/// collected. Filtering happens downstream: `check_module` partitions
/// `_G`/`shared` out for R0024 before classification even starts, and
/// R0023/R0022 apply their own filters (the `^_` exemption, std-membership)
/// over what's left.
///
/// A name that DOES resolve is recorded in one case only: a write whose
/// binding is a `const` (R0043).
fn touch(
    scopes: &[Scope],
    name: &str,
    span: &hir::Span,
    use_: NameUse,
    out: &mut Walk,
) {
    match resolve(scopes, name) {
        None => out.events.push(NameEvent {
            name: name.to_string(),
            span: span.clone(),
            use_,
        }),
        Some(Some(declared)) if use_ == NameUse::Write => out.const_writes.push(ConstWrite {
            name: name.to_string(),
            span: span.clone(),
            declared: declared.clone(),
        }),
        Some(_) => {}
    }
}

/// A closure's initial scope: its declared parameters, plus a synthesized
/// `self` for method bodies. `function t:m(...)` declares an implicit
/// `self` that full-moon's parameter list (and so `body.params`) does not
/// carry — `rules::calls` already relies on the same fact (its
/// `explicit_self` check treats a literal `self`/`_self` first parameter as
/// a different, deliberate spelling, not the implicit one it never sees).
fn param_scope(closure: &hir::Body, is_method: bool) -> Scope {
    let mut scope: Scope = closure.params.iter().map(|p| (p.name.clone(), None)).collect();
    if is_method {
        scope.insert("self".to_string(), None);
    }
    scope
}

// --- classification ----------------------------------------------------

/// R0043 const-reassign: a write whose target resolves to a `const`
/// binding — `x = e`, `x += e`, one target of `a, x = …`, and
/// `function x() end` (sugar for `x = function() end`), from the declaring
/// function or from any closure inside it. ONE diagnostic per write.
///
/// Not conservative. Luau's parser rejects each of these forms ("Variable
/// 'x' is constant and may not be reassigned"; `Parser.cpp` checks the
/// target in `parseAssignment`, `parseCompoundAssignment` and
/// `parseFunctionStat`, upvalues included), and a parse error fails the
/// whole script, so no run of the file can make the write legal.
///
/// A write THROUGH a const (`t.x = 1`, `t[k] = v`) is not a write TO it:
/// `target_write` routes those to `expr`, as reads of `t`.
///
/// Lives here rather than in a module of its own because the question
/// "which declaration does this write reach?" is the scope walk above; a
/// second walk would have to stay in step with this one case by case
/// (`repeat … until`, method `self`, declaration order).
fn classify_const_reassign(writes: &[ConstWrite], diags: &mut Vec<Diagnostic>) {
    for w in writes {
        diags.push(
            Diagnostic::error(
                Code(43),
                format!("cannot assign to `{}`, it is declared `const`", w.name),
            )
            .with_label(Label::primary(w.span.clone(), "assigned here"))
            .with_label(Label::secondary(
                w.declared.clone(),
                format!("`{}` declared `const` here", w.name),
            ))
            .with_note(
                "Luau refuses to compile a file that assigns to a `const` binding, \
                 so the whole script fails to load, not only this statement",
            )
            .with_help(format!(
                "declare `{}` with `local` if it has to change, or keep the new value \
                 under another name",
                w.name
            )),
        );
    }
}

/// R0023: a WRITE event to a name with no `local` in any enclosing scope,
/// except selene's default `^_` exemption (a plain `starts_with`, no
/// regex/config). ONE diagnostic PER WRITE,
/// selene-style (the third form: dedup-to-first-site and packed secondary
/// labels were both tried and rejected — editors jump to a diagnostic's
/// primary file:line, so every site must be its own diagnostic; the count
/// note keeps the sites connected).
fn classify_unscoped_variable(events: &[NameEvent], diags: &mut Vec<Diagnostic>) {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for e in events {
        if e.use_ == NameUse::Write && !e.name.starts_with('_') {
            *counts.entry(&e.name).or_default() += 1;
        }
    }
    for e in events {
        if e.use_ != NameUse::Write || e.name.starts_with('_') {
            continue;
        }
        let d = Diagnostic::error(
            Code(23),
            format!("global write: `{}` is not declared `local`", e.name),
        )
        .with_label(Label::primary(
            e.span.clone(),
            "no `local` declaration in any enclosing scope",
        ))
        .conservative(
            "assigning to an undeclared name silently creates a global at \
             runtime — legal Luau, but almost always a forgotten `local`",
        );
        diags.push(d);
    }
}

/// R0022: every READ event for a name that is neither a known std/Roblox
/// global (`std`, bundled Luau+Roblox — see `rime_std::StdNames::bundled`)
/// nor written anywhere else in the file (`written`, built by
/// `check_module` from this same event stream). The write-defines-the-name
/// check is deliberately order-insensitive — a read before its file's own
/// global write is still silent, matching selene, and matching case
/// `tests/globals/reject-conservative/006-read-after-global-write.luau`,
/// which writes and then reads `total` and expects exactly the R0023
/// diagnostic, not a second R0022 on the read.
fn classify_undefined_variable(
    events: &[NameEvent],
    written: &HashSet<&str>,
    std: &rime_std::StdNames,
    diags: &mut Vec<Diagnostic>,
) {
    for e in events {
        if e.use_ == NameUse::Read && !std.contains(&e.name) && !written.contains(e.name.as_str()) {
            diags.push(
                Diagnostic::error(Code(22), format!("`{}` is not defined", e.name))
                    .with_label(Label::primary(
                        e.span.clone(),
                        "not a local, not a known global",
                    ))
                    .conservative("code that runs earlier could create this global at runtime"),
            );
        }
    }
}

/// R0024: every occurrence (Read or Write, unfiltered) in `global_events` —
/// which `check_module` has already restricted to exactly the names `_G`
/// and `shared`, Luau's two ambient mutable-global handles (selene's
/// `global_usage` flags `_G`; the Roblox std adds `shared`). This flags
/// EVERY use, including a plain read (`print(_G.x)`) — not just writes —
/// which is why the partition happens upstream instead of adding a name
/// check to `classify_unscoped_variable` (writes-only) or
/// `classify_undefined_variable` (reads-only): neither loop alone sees
/// both use kinds for a name. A local `_G`/`shared` shadows and silences,
/// same as R0022/R0023 — enforced upstream too, since `touch` never
/// records a resolved name as an event at all (see
/// `tests/globals/accept/006-shadowed-g.luau`).
///
/// Warning, not error, alone in this family: reaching for `_G`/`shared` at
/// all is a deliberate act, and a large-codebase survey found it deliberate
/// every time it occurred (module doc above).
fn classify_global_usage(global_events: &[NameEvent], diags: &mut Vec<Diagnostic>) {
    for e in global_events {
        diags.push(
            Diagnostic::warning(Code(24), format!("use of `{}` global state", e.name))
                .with_label(Label::primary(e.span.clone(), "global mutable state"))
                .conservative("deliberate `_G`/`shared` use is valid Luau"),
        );
    }
}

/// R0022 for `...`: a read of `...` in a function whose parameter list
/// does not declare it. Per body, off `hir::SyntaxNotes::vararg_reads`.
///
/// Selene files this under `undefined_variable` (`...` is not defined),
/// which is why it is R0022 here and not a rule of its own. Unlike every
/// other R0022 it is NOT conservative: Luau refuses to compile the file
/// ("Cannot use '...' outside of a vararg function"), so no code that runs
/// earlier can make the read mean something.
///
/// The top level of a file is variadic (`has_varargs` on the root body),
/// blocks nested in it included. Selene accepts `...` only in the file's
/// outermost scope and reports `do local x = ... end` at the top level;
/// that code is valid and Rime does not follow it there.
pub fn check_varargs(body: &hir::Body, diags: &mut Vec<Diagnostic>) {
    if body.has_varargs {
        return;
    }
    for span in &body.notes.vararg_reads {
        diags.push(
            Diagnostic::error(Code(22), "`...` is not defined in this function")
                .with_label(Label::primary(
                    span.clone(),
                    "this function's parameter list has no `...`",
                ))
                .with_note(
                    "Luau refuses to compile the file: `...` can be read only in the \
                     function that declares it, and at the top level of a file"
                        .to_string(),
                )
                .with_help(
                    "add `...` to this function's parameters; an enclosing function's `...` \
                     does not reach in here, so capture it outside first \
                     (`local args = table.pack(...)`)",
                ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1-based lines of the R0022 `...` reports over every body of `src`,
    /// and whether any of them is marked conservative.
    fn vararg_report_lines(src: &str) -> (Vec<usize>, bool) {
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        for body in &module.bodies {
            check_varargs(body, &mut diags);
        }
        let mut lines: Vec<usize> = diags
            .iter()
            .map(|d| {
                assert_eq!(d.code.to_string(), "R0022");
                src[..d.primary_span().start].matches('\n').count() + 1
            })
            .collect();
        lines.sort_unstable();
        (lines, diags.iter().any(|d| d.conservative))
    }

    // Selene's own fixture for this (`undefined_variable/varargs.lua`): it
    // reports lines 4 and 8, the two reads in functions without `...`.
    #[test]
    fn varargs_selene_fixture() {
        let src = "\
local args = ...

return function()
    local shouldntExist = ...
    return function(...)
        local shouldExist = ...
        return function()
            local alsoShouldntExist = ...
        end
    end
end
";
        let (lines, any_conservative) = vararg_report_lines(src);
        assert_eq!(lines, vec![4, 8]);
        // Luau refuses to compile the file ("Cannot use '...' outside of a
        // vararg function"), so nothing at runtime can make this right.
        assert!(!any_conservative);
    }

    // One report per read, not one per function.
    #[test]
    fn varargs_every_read_is_reported() {
        let src = "\
local M = {}
function M.plain()
\tprint(...)
\treturn select(\"#\", ...), { ... }
end
function M:method()
\treturn ...
end
return M
";
        assert_eq!(vararg_report_lines(src).0, vec![3, 4, 4, 7]);
    }

    // Where Luau allows `...`: the top level of a file — its nested blocks
    // included, which Selene reports (it accepts `...` only in the file's
    // outermost scope, a wrong report on valid code) — and anywhere in a
    // function that declares it.
    #[test]
    fn varargs_silent_where_luau_allows_them() {
        let src = "\
local first = ...
do
\tlocal again = ...
\tprint(again)
end
if first then
\tprint(...)
end
local function sum(...: number)
\tfor _, n in { ... } do
\t\tif n > 0 then
\t\t\tprint(select(\"#\", ...))
\t\tend
\tend
end
return sum
";
        assert_eq!(vararg_report_lines(src).0, Vec::<usize>::new());
    }

    // Every write to a never-localized name is its own diagnostic, in
    // source order (the third form: dedup-to-first and packed secondary
    // labels were both tried and rejected — the primary file:line is what
    // editors jump to, so every site needs its own).
    #[test]
    fn repeated_global_write_reports_every_site() {
        let src = "\
local function f(a, b)
\tplayer = a
\tif not player then
\t\tplayer = b
\tend
\treturn player
end
return f
";
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        check_module(&module, rime_std::StdNames::bundled(), &mut diags);
        let r23: Vec<_> = diags
            .iter()
            .filter(|d| d.code.to_string() == "R0023")
            .collect();
        assert_eq!(r23.len(), 2, "want one R0023 per write, got {}", r23.len());
        let first_write = src.find("player = a").unwrap();
        let second_write = src.find("player = b").unwrap();
        assert_eq!(r23[0].labels[0].span.start, first_write);
        assert_eq!(r23[1].labels[0].span.start, second_write);
        for d in &r23 {
            assert_eq!(d.labels.len(), 1, "no packed labels: {:?}", d.labels);
        }
    }

    fn with_code<'d>(diags: &'d [Diagnostic], code: &str) -> Vec<&'d Diagnostic> {
        diags.iter().filter(|d| d.code.to_string() == code).collect()
    }

    // One R0043 per write that reaches a `const`, primary label at the
    // write, secondary at the declaration it reaches — and no R0023 for
    // the same occurrence (the name resolves; it is not a global write).
    #[test]
    fn const_reassign_reports_every_write_and_points_at_the_declaration() {
        let src = "\
const limit = 1
local function f()
\tlimit = 2
\tlimit += 3
end
return f
";
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        check_module(&module, rime_std::StdNames::bundled(), &mut diags);
        let r43 = with_code(&diags, "R0043");
        assert_eq!(r43.len(), 2, "want one R0043 per write, got {diags:?}");
        assert_eq!(r43[0].labels[0].span.start, src.find("limit = 2").unwrap());
        assert_eq!(r43[1].labels[0].span.start, src.find("limit += 3").unwrap());
        for d in &r43 {
            assert!(!d.conservative, "Luau rejects the file: not a conservative finding");
            assert_eq!(d.labels.len(), 2, "write + declaration: {:?}", d.labels);
            assert_eq!(d.labels[1].span, 6..11, "the declaration's name");
        }
        assert!(with_code(&diags, "R0023").is_empty(), "{diags:?}");
        assert!(with_code(&diags, "R0022").is_empty(), "{diags:?}");
    }

    // The innermost binding of the name decides: a `local` that shadows a
    // const takes the write, and a const that shadows a local refuses it.
    #[test]
    fn const_reassign_follows_the_innermost_binding() {
        let src = "\
const a = 1
local b = 1
do
\tlocal a = 2
\ta = 3
\tconst b = 4
\tb = 5
end
b = 6
return a + b
";
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        check_module(&module, rime_std::StdNames::bundled(), &mut diags);
        let r43 = with_code(&diags, "R0043");
        assert_eq!(r43.len(), 1, "{diags:?}");
        assert_eq!(r43[0].labels[0].span.start, src.find("b = 5").unwrap());
        assert_eq!(r43[0].labels[1].span.start, src.find("b = 4").unwrap());
    }

    // `function name() end` assigns `name`; when `name` is a const (a
    // `const function` included) that is a reassignment like any other.
    #[test]
    fn const_reassign_sees_function_declarations() {
        let src = "\
const function f()
\treturn 1
end
function f()
\treturn 2
end
return f
";
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        check_module(&module, rime_std::StdNames::bundled(), &mut diags);
        let r43 = with_code(&diags, "R0043");
        assert_eq!(r43.len(), 1, "{diags:?}");
        assert_eq!(r43[0].labels[0].span.start, src.find("function f()\n\treturn 2").unwrap());
    }

    // A table held by a const is still a table: writes THROUGH the name
    // are not writes TO it.
    #[test]
    fn const_reassign_ignores_field_and_index_writes() {
        let src = "\
const t = {}
t.x = 1
t[2] = 3
t.x += 1
return t
";
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        check_module(&module, rime_std::StdNames::bundled(), &mut diags);
        assert!(diags.is_empty(), "{diags:?}");
    }
}
