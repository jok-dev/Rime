//! Cross-body name analysis — the capture machinery, shared by rules.
//!
//! `nested_closures` lists the closure bodies a block creates directly, each
//! paired with the `local function` name that refers to the closure itself
//! (when there is one). `free_names` walks a closure body — and its
//! transitively nested closures, which extend the same scope chain — and
//! collects every name that does not resolve inside the closure: those
//! escape to the enclosing body, or to a global. Free names split into two
//! sets: `reads` (any value-observing position, compound-assign targets
//! included — `x += 1` reads `x`) and `writes` (pure stores: plain
//! assignment targets and single-name `function f()` declarations).
//!
//! Name-based and over-approximate on purpose; every client only loses
//! precision, never soundness:
//!   - narrowing (R0201) and dead-store (R0104) treat any free name — read
//!     or written — as CAPTURED (`captured_names` is the union): a write
//!     mutates the alias, so over-poisoning disables a narrowing it did not
//!     strictly need to;
//!   - unused (R0013/14/15) counts only free READS as uses: a write-only
//!     captured local is dead state nothing observes (decided 2026-08-23;
//!     selene agrees). Over-counting reads still only ever silences.

use rime_hir as hir;
use std::collections::HashSet;

/// Names free in any closure created (transitively) inside `body`. Every
/// free name counts, self-reference included — for aliasing purposes a
/// `local function` IS an alias to itself. Clients that must NOT count
/// self-reference (R0015) run the walk themselves and seed the name.
pub(crate) fn captured_names(module: &hir::Module, body: &hir::Body) -> HashSet<String> {
    let mut nested = Vec::new();
    nested_closures(&body.block, &mut nested);
    let mut reads = HashSet::new();
    let mut writes = Vec::new();
    for (id, _) in nested {
        let closure = module.body(id);
        let mut scopes: Vec<HashSet<String>> =
            vec![closure.params.iter().map(|p| p.name.clone()).collect()];
        free_names(module, &closure.block, &mut scopes, &mut reads, &mut writes);
    }
    reads.extend(writes.into_iter().map(|(name, _)| name));
    reads
}

/// Names the closure `id` plainly assigns that do not resolve inside it:
/// the enclosing bodies' locals (or globals) a call to it can store to.
/// Closures nested inside it count as well — running the outer one may
/// run them. Name-based and over-approximate, like the rest of this file.
pub(crate) fn closure_writes(module: &hir::Module, id: hir::BodyId) -> Vec<String> {
    let closure = module.body(id);
    let mut scopes: Vec<HashSet<String>> =
        vec![closure.params.iter().map(|p| p.name.clone()).collect()];
    let mut reads = HashSet::new();
    let mut writes = Vec::new();
    free_names(module, &closure.block, &mut scopes, &mut reads, &mut writes);
    writes.into_iter().map(|(name, _)| name).collect()
}

/// Closure bodies created directly by this block's statements/expressions.
/// The `Option<String>` is the closure's own recursion name: `Some(f)` for
/// `local function f` (where `f` inside the body is self-reference), `None`
/// for expression closures and `function a.b()` declarations.
pub(crate) fn nested_closures(block: &hir::Block, out: &mut Vec<(hir::BodyId, Option<String>)>) {
    use hir::StmtKind as K;
    for stmt in &block.stmts {
        match &stmt.kind {
            K::Local { init, .. } => init.iter().for_each(|e| nested_in_expr(e, out)),
            K::Assign { targets, values } => {
                for t in targets {
                    if let hir::Target::Projection(e) = t {
                        nested_in_expr(e, out);
                    }
                }
                values.iter().for_each(|e| nested_in_expr(e, out));
            }
            K::CompoundAssign { target, value } => {
                if let hir::Target::Projection(e) = target {
                    nested_in_expr(e, out);
                }
                nested_in_expr(value, out);
            }
            K::Call(e) => nested_in_expr(e, out),
            K::Do(b) => nested_closures(b, out),
            K::If {
                cond,
                then,
                else_ifs,
                or_else,
            } => {
                nested_in_expr(cond, out);
                nested_closures(then, out);
                for (c, b) in else_ifs {
                    nested_in_expr(c, out);
                    nested_closures(b, out);
                }
                if let Some(b) = or_else {
                    nested_closures(b, out);
                }
            }
            K::While { cond, body } => {
                nested_in_expr(cond, out);
                nested_closures(body, out);
            }
            K::Repeat { body, until } => {
                nested_closures(body, out);
                nested_in_expr(until, out);
            }
            K::NumericFor {
                start,
                end,
                step,
                body,
                ..
            } => {
                nested_in_expr(start, out);
                nested_in_expr(end, out);
                if let Some(s) = step {
                    nested_in_expr(s, out);
                }
                nested_closures(body, out);
            }
            K::GenericFor { exprs, body, .. } => {
                exprs.iter().for_each(|e| nested_in_expr(e, out));
                nested_closures(body, out);
            }
            K::LocalFunction { decl, body } => out.push((*body, Some(decl.name.clone()))),
            K::FunctionDecl { body, .. } => out.push((*body, None)),
        }
    }
    if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
        exprs.iter().for_each(|e| nested_in_expr(e, out));
    }
}

fn nested_in_expr(expr: &hir::Expr, out: &mut Vec<(hir::BodyId, Option<String>)>) {
    use hir::Expr as E;
    match expr {
        E::Function(id) => out.push((*id, None)),
        E::Call { base, args, .. } => {
            nested_in_expr(base, out);
            args.iter().for_each(|e| nested_in_expr(e, out));
        }
        E::Index { base, key, .. } => {
            nested_in_expr(base, out);
            if let Some(k) = key {
                nested_in_expr(k, out);
            }
        }
        E::BinOp { lhs, rhs, .. } => {
            nested_in_expr(lhs, out);
            nested_in_expr(rhs, out);
        }
        E::Not(inner) | E::UnOp { operand: inner, .. } => nested_in_expr(inner, out),
        E::Table(fields) => {
            for f in fields {
                match f {
                    hir::TableField::Keyed { key, value } => {
                        nested_in_expr(key, out);
                        nested_in_expr(value, out);
                    }
                    hir::TableField::Named { value, .. } => nested_in_expr(value, out),
                    hir::TableField::Item(e) => nested_in_expr(e, out),
                }
            }
        }
        E::IfExpr(arms) => {
            for (cond, val) in arms {
                if let Some(c) = cond {
                    nested_in_expr(c, out);
                }
                nested_in_expr(val, out);
            }
        }
        E::Composite(parts) => parts.iter().for_each(|e| nested_in_expr(e, out)),
        E::Name { .. } | E::Literal { .. } | E::Leaf => {}
    }
}

/// Names read or written in `block` that do not resolve in the scope chain
/// rooted at the closure — those escape to the enclosing body (or a global;
/// the over-approximation is deliberate). `reads` collects every
/// value-observing occurrence (compound-assign targets included); `writes`
/// collects pure stores WITH their spans (plain assignment targets at the
/// target-name span, single-name `function f()` declarations at the
/// statement span) — R0016 points at each dead store, so sites matter.
/// Nested closures extend the same chain, so a doubly-nested capture
/// propagates and a middle-bound name does not. Callers seed `scopes` with
/// the closure's params (and its own recursion name, when self-reference
/// should not count).
///
/// Cross-reference: `rules::globals`'s module-level walker (`walk_block` in
/// `rules/globals.rs`) mirrors this exact scope-stack shape — same
/// descend-into-closures-at-creation-sites order — for a different client
/// that needs per-occurrence spans and Read/Write kind, not a merged name
/// set. It does NOT call this function directly; see its own doc comment
/// for the differences (repeat/until scope timing is the one that actually
/// changes behavior, not just shape) and fold the two only if a third
/// client turns up wanting yet another output shape.
pub(crate) fn free_names(
    module: &hir::Module,
    block: &hir::Block,
    scopes: &mut Vec<HashSet<String>>,
    reads: &mut HashSet<String>,
    writes: &mut Vec<(String, hir::Span)>,
) {
    use hir::StmtKind as K;

    fn resolve(scopes: &[HashSet<String>], name: &str) -> bool {
        scopes.iter().any(|s| s.contains(name))
    }
    fn touch(scopes: &[HashSet<String>], name: &str, into: &mut HashSet<String>) {
        if !resolve(scopes, name) {
            into.insert(name.to_string());
        }
    }
    fn record_write(
        scopes: &[HashSet<String>],
        name: &str,
        span: &hir::Span,
        writes: &mut Vec<(String, hir::Span)>,
    ) {
        if !resolve(scopes, name) {
            writes.push((name.to_string(), span.clone()));
        }
    }
    fn expr(
        module: &hir::Module,
        e: &hir::Expr,
        scopes: &mut Vec<HashSet<String>>,
        reads: &mut HashSet<String>,
        writes: &mut Vec<(String, hir::Span)>,
    ) {
        use hir::Expr as E;
        match e {
            E::Name { name, .. } => touch(scopes, name, reads),
            E::Function(id) => {
                let closure = module.body(*id);
                scopes.push(closure.params.iter().map(|p| p.name.clone()).collect());
                free_names(module, &closure.block, scopes, reads, writes);
                scopes.pop();
            }
            E::Call { base, args, .. } => {
                expr(module, base, scopes, reads, writes);
                args.iter()
                    .for_each(|a| expr(module, a, scopes, reads, writes));
            }
            E::Index { base, key, .. } => {
                expr(module, base, scopes, reads, writes);
                if let Some(k) = key {
                    expr(module, k, scopes, reads, writes);
                }
            }
            E::BinOp { lhs, rhs, .. } => {
                expr(module, lhs, scopes, reads, writes);
                expr(module, rhs, scopes, reads, writes);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => {
                expr(module, inner, scopes, reads, writes)
            }
            E::Table(fields) => {
                for f in fields {
                    match f {
                        hir::TableField::Keyed { key, value } => {
                            expr(module, key, scopes, reads, writes);
                            expr(module, value, scopes, reads, writes);
                        }
                        hir::TableField::Named { value, .. } => {
                            expr(module, value, scopes, reads, writes)
                        }
                        hir::TableField::Item(e) => expr(module, e, scopes, reads, writes),
                    }
                }
            }
            E::IfExpr(arms) => {
                for (cond, val) in arms {
                    if let Some(c) = cond {
                        expr(module, c, scopes, reads, writes);
                    }
                    expr(module, val, scopes, reads, writes);
                }
            }
            E::Composite(parts) => parts
                .iter()
                .for_each(|p| expr(module, p, scopes, reads, writes)),
            E::Literal { .. } | E::Leaf => {}
        }
    }

    scopes.push(HashSet::new());
    for stmt in &block.stmts {
        match &stmt.kind {
            K::Local { decls, init } => {
                init.iter()
                    .for_each(|e| expr(module, e, scopes, reads, writes));
                for d in decls {
                    scopes.last_mut().unwrap().insert(d.name.clone());
                }
            }
            K::Assign { targets, values } => {
                values
                    .iter()
                    .for_each(|e| expr(module, e, scopes, reads, writes));
                for t in targets {
                    match t {
                        // A plain assignment target is a pure store.
                        hir::Target::Name { name, span } => {
                            record_write(scopes, name, span, writes)
                        }
                        // `t.x = v` READS `t` (and any bracket key).
                        hir::Target::Projection(e) => expr(module, e, scopes, reads, writes),
                    }
                }
            }
            K::CompoundAssign { target: t, value } => {
                expr(module, value, scopes, reads, writes);
                match t {
                    // `x += 1` reads `x` before storing.
                    hir::Target::Name { name, .. } => touch(scopes, name, reads),
                    hir::Target::Projection(e) => expr(module, e, scopes, reads, writes),
                }
            }
            K::Call(e) => expr(module, e, scopes, reads, writes),
            K::Do(b) => free_names(module, b, scopes, reads, writes),
            K::If {
                cond,
                then,
                else_ifs,
                or_else,
            } => {
                expr(module, cond, scopes, reads, writes);
                free_names(module, then, scopes, reads, writes);
                for (c, b) in else_ifs {
                    expr(module, c, scopes, reads, writes);
                    free_names(module, b, scopes, reads, writes);
                }
                if let Some(b) = or_else {
                    free_names(module, b, scopes, reads, writes);
                }
            }
            K::While { cond, body } => {
                expr(module, cond, scopes, reads, writes);
                free_names(module, body, scopes, reads, writes);
            }
            K::Repeat { body, until } => {
                // Approximation: the until-condition's shared scope with the
                // body is not modelled here; body locals it references count
                // as free. Over-poisoning only.
                free_names(module, body, scopes, reads, writes);
                expr(module, until, scopes, reads, writes);
            }
            K::NumericFor {
                var,
                start,
                end,
                step,
                body,
            } => {
                expr(module, start, scopes, reads, writes);
                expr(module, end, scopes, reads, writes);
                if let Some(s) = step {
                    expr(module, s, scopes, reads, writes);
                }
                scopes.push(HashSet::from([var.name.clone()]));
                free_names(module, body, scopes, reads, writes);
                scopes.pop();
            }
            K::GenericFor { vars, exprs, body } => {
                exprs
                    .iter()
                    .for_each(|e| expr(module, e, scopes, reads, writes));
                scopes.push(vars.iter().map(|v| v.name.clone()).collect());
                free_names(module, body, scopes, reads, writes);
                scopes.pop();
            }
            K::LocalFunction { decl, body } => {
                scopes.last_mut().unwrap().insert(decl.name.clone());
                let closure = module.body(*body);
                scopes.push(closure.params.iter().map(|p| p.name.clone()).collect());
                free_names(module, &closure.block, scopes, reads, writes);
                scopes.pop();
            }
            K::FunctionDecl {
                root,
                assigns_root,
                body,
                ..
            } => {
                if *assigns_root {
                    // `function f() end` is `f = function`: a pure store.
                    // No span for the bare name — the statement's will do.
                    record_write(scopes, root, &stmt.span, writes);
                }
                let closure = module.body(*body);
                scopes.push(closure.params.iter().map(|p| p.name.clone()).collect());
                free_names(module, &closure.block, scopes, reads, writes);
                scopes.pop();
            }
        }
    }
    if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
        exprs
            .iter()
            .for_each(|e| expr(module, e, scopes, reads, writes));
    }
    scopes.pop();
}
