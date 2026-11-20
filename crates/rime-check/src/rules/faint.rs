//! R0017: faint local — every read of the binding only feeds the binding
//! itself, so nothing ever consumes its value (decided 2026-08-23; the
//! survey reference lives in `rime-bench/src/bin/faint_survey.rs`; on a
//! large codebase every spot-check was real). A
//! local is FAINT when it has at least one read and every read is
//! self-feeding:
//!
//!   - a read in the RHS of an assignment back to the same local
//!     (`count += 1`, `s = s .. x` — the accumulator shape), or
//!   - a read in the condition of a self-guard `if`: no elseifs, no else,
//!     no terminator, and a body that only assigns the same local from
//!     effect-free expressions (`if not x then x = default end`).
//!
//! Loop conditions never count (they control how often effectful code
//! runs), reads inside call arguments never count (the value escapes), and
//! `x.field = v` reads x's value as a table reference. Differences from
//! the survey detector: capture is decided by `capture::captured_names`
//! (precise transitive walk) instead of a name-in-any-other-body scan, and
//! the root body is INCLUDED — module scope is checked everywhere in the
//! unused family since v2 (2026-08-23).
//!
//! Division of labor: zero-read locals are R0013/R0016's territory; a
//! faint local's guarded fallback store would otherwise draw a misleading
//! loop-carried R0104 (the guard-in-loop case), so
//! `check` returns the faint declarations and `dead_store` skips them:
//! one defect, one diagnostic, at the declaration.

use crate::capture;
use rime_diag::{Code, Diagnostic, Label, Span};
use rime_hir as hir;
use std::collections::HashSet;

const SHAPE_ACCUM: u8 = 1;
const SHAPE_GUARD: u8 = 2;
const SHAPE_GUARD_IN_LOOP: u8 = 4;

/// Flags faint locals in `body`, pushing one R0017 per finding at the
/// declaration. Returns the declaration-span starts of the flagged locals
/// so `dead_store` can suppress its R0104 on the same bindings.
pub fn check(module: &hir::Module, body: &hir::Body, diags: &mut Vec<Diagnostic>) -> HashSet<usize> {
    let captured = capture::captured_names(module, body);
    let mut w = Walker { decls: Vec::new(), scopes: vec![Vec::new()], loop_depth: 0 };
    for p in &body.params {
        w.declare(&p.name, &p.span, false);
    }
    w.walk_block(&body.block);

    let mut faint = HashSet::new();
    for d in &w.decls {
        if !d.candidate
            || d.external
            || d.shapes == 0
            || d.name.starts_with('_')
            || captured.contains(&d.name)
        {
            continue;
        }
        faint.insert(d.span.start);
        diags.push(diag(&d.name, d.span.clone(), d.shapes));
    }
    faint
}

fn diag(name: &str, decl_span: Span, shapes: u8) -> Diagnostic {
    let note = if shapes & (SHAPE_GUARD | SHAPE_GUARD_IN_LOOP) != 0 {
        format!(
            "its only reads guard its own fallback assignment; nothing ever \
             consumes the value — the code that used `{name}` may have been \
             deleted"
        )
    } else {
        format!(
            "it only accumulates into itself; nothing ever consumes the result"
        )
    };
    Diagnostic::warning(
        Code(17),
        format!("`{name}` only feeds itself; its value is never used"),
    )
    .with_label(Label::primary(
        decl_span.clone(),
        format!("`{name}` declared here"),
    ))
    .with_note(note)
    .conservative(
        "every read of this binding flows back into the binding; Rime cannot \
         tell abandoned code from work-in-progress",
    )
    .with_help(format!(
        "use the value, or delete `{name}` and its self-feeding statements"
    ))
}

struct DeclState {
    name: String,
    span: Span,
    candidate: bool,
    external: bool,
    shapes: u8,
}

/// Reads resolving to the carried decl feed that decl itself; everything
/// else a read touches is an external use of whatever it resolves to.
#[derive(Clone, Copy)]
enum Ctx {
    External,
    Internal(usize, u8),
}

struct Walker {
    decls: Vec<DeclState>,
    scopes: Vec<Vec<(String, usize)>>,
    loop_depth: usize,
}

impl Walker {
    fn declare(&mut self, name: &str, span: &Span, candidate: bool) {
        let id = self.decls.len();
        self.decls.push(DeclState {
            name: name.to_string(),
            span: span.clone(),
            candidate,
            external: false,
            shapes: 0,
        });
        self.scopes.last_mut().unwrap().push((name.to_string(), id));
    }

    fn resolve(&self, name: &str) -> Option<usize> {
        for frame in self.scopes.iter().rev() {
            for (n, id) in frame.iter().rev() {
                if n == name {
                    return Some(*id);
                }
            }
        }
        None
    }

    fn walk_block(&mut self, block: &hir::Block) {
        self.scopes.push(Vec::new());
        self.walk_stmts(block);
        self.scopes.pop();
    }

    fn walk_stmts(&mut self, block: &hir::Block) {
        for stmt in &block.stmts {
            self.walk_stmt(stmt);
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            for e in exprs {
                self.classify(e, Ctx::External);
            }
        }
    }

    fn walk_stmt(&mut self, stmt: &hir::Stmt) {
        use hir::StmtKind as K;
        match &stmt.kind {
            K::Local { decls, init } => {
                // Initializers evaluate before the names bind.
                for e in init {
                    self.classify(e, Ctx::External);
                }
                for d in decls {
                    self.declare(&d.name, &d.span, true);
                }
            }
            K::Assign { targets, values } => {
                let single_self = match targets.as_slice() {
                    [hir::Target::Name { name, .. }] => self.resolve(name),
                    _ => None,
                };
                let ctx = match single_self {
                    Some(id) => Ctx::Internal(id, SHAPE_ACCUM),
                    None => Ctx::External,
                };
                for t in targets {
                    if let hir::Target::Projection(e) = t {
                        self.classify(e, Ctx::External);
                    }
                }
                for v in values {
                    self.classify(v, ctx);
                }
            }
            K::CompoundAssign { target, value } => match target {
                hir::Target::Name { name, .. } => match self.resolve(name) {
                    Some(id) => {
                        // `x += e` reads x — a self-feeding read.
                        self.decls[id].shapes |= SHAPE_ACCUM;
                        self.classify(value, Ctx::Internal(id, SHAPE_ACCUM));
                    }
                    None => self.classify(value, Ctx::External),
                },
                hir::Target::Projection(e) => {
                    self.classify(e, Ctx::External);
                    self.classify(value, Ctx::External);
                }
            },
            K::Call(e) => self.classify(e, Ctx::External),
            K::Do(b) => self.walk_block(b),
            K::If { cond, then, else_ifs, or_else } => {
                let guard =
                    self_guard_target(then, else_ifs, or_else).and_then(|n| self.resolve(&n));
                match guard {
                    Some(id) => {
                        let shape = if self.loop_depth > 0 {
                            SHAPE_GUARD_IN_LOOP
                        } else {
                            SHAPE_GUARD
                        };
                        self.classify(cond, Ctx::Internal(id, shape));
                        self.walk_block(then);
                    }
                    None => {
                        self.classify(cond, Ctx::External);
                        self.walk_block(then);
                        for (c, b) in else_ifs {
                            self.classify(c, Ctx::External);
                            self.walk_block(b);
                        }
                        if let Some(b) = or_else {
                            self.walk_block(b);
                        }
                    }
                }
            }
            K::While { cond, body } => {
                // Loop conditions control how often effectful code runs —
                // never a self-feeding read.
                self.classify(cond, Ctx::External);
                self.loop_depth += 1;
                self.walk_block(body);
                self.loop_depth -= 1;
            }
            K::Repeat { body, until } => {
                self.loop_depth += 1;
                self.scopes.push(Vec::new());
                self.walk_stmts(body); // until-cond sees body locals
                self.classify(until, Ctx::External);
                self.scopes.pop();
                self.loop_depth -= 1;
            }
            K::NumericFor { var, start, end, step, body } => {
                self.classify(start, Ctx::External);
                self.classify(end, Ctx::External);
                if let Some(s) = step {
                    self.classify(s, Ctx::External);
                }
                self.loop_depth += 1;
                self.scopes.push(Vec::new());
                self.declare(&var.name, &var.span, false);
                self.walk_block(body);
                self.scopes.pop();
                self.loop_depth -= 1;
            }
            K::GenericFor { vars, exprs, body } => {
                for e in exprs {
                    self.classify(e, Ctx::External);
                }
                self.loop_depth += 1;
                self.scopes.push(Vec::new());
                for v in vars {
                    self.declare(&v.name, &v.span, false);
                }
                self.walk_block(body);
                self.scopes.pop();
                self.loop_depth -= 1;
            }
            K::LocalFunction { decl, .. } => self.declare(&decl.name, &decl.span, false),
            K::FunctionDecl { .. } => {}
        }
    }

    fn classify(&mut self, e: &hir::Expr, ctx: Ctx) {
        use hir::Expr as E;
        match e {
            E::Name { name, .. } => {
                if let Some(id) = self.resolve(name) {
                    match ctx {
                        Ctx::Internal(target, shape) if target == id => {
                            self.decls[id].shapes |= shape
                        }
                        _ => self.decls[id].external = true,
                    }
                }
            }
            // A value passed into a call escapes — never self-feeding.
            E::Call { base, args, .. } => {
                self.classify(base, Ctx::External);
                for a in args {
                    self.classify(a, Ctx::External);
                }
            }
            E::Index { base, key, .. } => {
                self.classify(base, ctx);
                if let Some(k) = key {
                    self.classify(k, ctx);
                }
            }
            E::Function(_) => {} // separate body; `captured` covers it
            E::BinOp { lhs, rhs, .. } => {
                self.classify(lhs, ctx);
                self.classify(rhs, ctx);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => self.classify(inner, ctx),
            E::Table(fields) => {
                for f in fields {
                    match f {
                        hir::TableField::Named { value, .. } => self.classify(value, ctx),
                        hir::TableField::Keyed { key, value } => {
                            self.classify(key, ctx);
                            self.classify(value, ctx);
                        }
                        hir::TableField::Item(v) => self.classify(v, ctx),
                    }
                }
            }
            E::IfExpr(arms) => {
                for (c, v) in arms {
                    if let Some(c) = c {
                        self.classify(c, ctx);
                    }
                    self.classify(v, ctx);
                }
            }
            E::Composite(exprs) => {
                for x in exprs {
                    self.classify(x, ctx);
                }
            }
            E::Literal { .. } | E::Leaf => {}
        }
    }
}

/// `if cond then <only effect-free assignments to one local> end` —
/// no elseifs, no else, no terminator. Returns the assigned name.
fn self_guard_target(
    then: &hir::Block,
    else_ifs: &[(hir::Expr, hir::Block)],
    or_else: &Option<hir::Block>,
) -> Option<String> {
    if !else_ifs.is_empty() || or_else.is_some() || then.terminator.is_some() || then.stmts.is_empty()
    {
        return None;
    }
    let mut target: Option<&str> = None;
    for stmt in &then.stmts {
        let (name, effect_free_values) = match &stmt.kind {
            hir::StmtKind::Assign { targets, values } => match targets.as_slice() {
                [hir::Target::Name { name, .. }] => (name, values.iter().all(effect_free)),
                _ => return None,
            },
            hir::StmtKind::CompoundAssign { target: hir::Target::Name { name, .. }, value } => {
                (name, effect_free(value))
            }
            _ => return None,
        };
        if !effect_free_values || target.is_some_and(|t| t != name) {
            return None;
        }
        target = Some(name);
    }
    target.map(str::to_string)
}

/// No calls anywhere in the expression — running it does nothing but
/// compute a value (metatable pedantry accepted; this feeds a
/// conservative-flagged rule).
fn effect_free(e: &hir::Expr) -> bool {
    use hir::Expr as E;
    match e {
        E::Call { .. } => false,
        E::Name { .. } | E::Literal { .. } | E::Leaf | E::Function(_) => true,
        E::Index { base, key, .. } => effect_free(base) && key.as_deref().is_none_or(effect_free),
        E::BinOp { lhs, rhs, .. } => effect_free(lhs) && effect_free(rhs),
        E::Not(inner) | E::UnOp { operand: inner, .. } => effect_free(inner),
        E::Table(fields) => fields.iter().all(|f| match f {
            hir::TableField::Named { value, .. } => effect_free(value),
            hir::TableField::Keyed { key, value } => effect_free(key) && effect_free(value),
            hir::TableField::Item(v) => effect_free(v),
        }),
        E::IfExpr(arms) => arms
            .iter()
            .all(|(c, v)| c.as_ref().is_none_or(|c| effect_free(c)) && effect_free(v)),
        E::Composite(exprs) => exprs.iter().all(effect_free),
    }
}

#[cfg(test)]
mod tests {
    // Ported from the survey detector's spec tests
    // (rime-bench/src/bin/faint_survey.rs); the corpus family tests/faint/
    // covers rendering and the R0104 suppression.

    fn r0017_names(src: &str) -> Vec<String> {
        let module = rime_syntax::parse(src).expect("test source must parse");
        let mut diags = Vec::new();
        for body in &module.bodies {
            super::check(&module, body, &mut diags);
        }
        diags
            .iter()
            .filter(|d| d.code.to_string() == "R0017")
            .map(|d| d.message.split('`').nth(1).unwrap().to_string())
            .collect()
    }

    #[test]
    fn guard_in_loop_is_faint() {
        // The guard-in-loop reduction.
        let hits = r0017_names(
            "local LABELS = { \"low\" }\n\
             local function f(levels)\n\
             \tfor index = 1, #levels do\n\
             \t\tlocal label = LABELS[levels[index]]\n\
             \t\tif not label then\n\
             \t\t\tlabel = \"?\"\n\
             \t\tend\n\
             \tend\n\
             end\n\
             return f\n",
        );
        assert_eq!(hits, vec!["label"]);
    }

    #[test]
    fn accumulator_is_faint() {
        let hits = r0017_names(
            "local function f(list)\n\
             \tlocal count = 0\n\
             \tfor _, e in pairs(list) do\n\
             \t\tcount += 1\n\
             \tend\n\
             end\n\
             return f\n",
        );
        assert_eq!(hits, vec!["count"]);
    }

    #[test]
    fn self_feed_chain_is_faint() {
        let hits = r0017_names(
            "local function f(a, b)\n\
             \tlocal s = \"\"\n\
             \ts = s .. a\n\
             \ts = s .. b\n\
             end\n\
             return f\n",
        );
        assert_eq!(hits, vec!["s"]);
    }

    #[test]
    fn straight_line_guard_is_faint() {
        let hits = r0017_names(
            "local function f(t)\n\
             \tlocal x = t.v\n\
             \tif not x then\n\
             \t\tx = 1\n\
             \tend\n\
             end\n\
             return f\n",
        );
        assert_eq!(hits, vec!["x"]);
    }

    #[test]
    fn read_after_guard_is_not_faint() {
        let hits = r0017_names(
            "local function f(t)\n\
             \tlocal x = t.v\n\
             \tif not x then\n\
             \t\tx = 1\n\
             \tend\n\
             \treturn x\n\
             end\n\
             return f\n",
        );
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn guard_with_call_is_not_faint() {
        // The guarded store runs a call the condition controls — `x`
        // affects behavior, so it is not faint.
        let hits = r0017_names(
            "local function f(t, mk)\n\
             \tlocal x = t.v\n\
             \tif not x then\n\
             \t\tx = mk()\n\
             \tend\n\
             end\n\
             return f\n",
        );
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn loop_condition_read_is_not_faint() {
        let hits = r0017_names(
            "local function f()\n\
             \tlocal done = false\n\
             \twhile not done do\n\
             \t\tdone = true\n\
             \tend\n\
             end\n\
             return f\n",
        );
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn read_escaping_into_call_is_not_faint() {
        let hits = r0017_names(
            "local function f(g)\n\
             \tlocal count = 0\n\
             \tcount = g(count)\n\
             end\n\
             return f\n",
        );
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn projection_write_is_a_use() {
        // `x.field = 1` reads x's value as a table reference.
        let hits = r0017_names(
            "local function f()\n\
             \tlocal x = {}\n\
             \tx = x\n\
             \tx.field = 1\n\
             end\n\
             return f\n",
        );
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn captured_local_is_skipped() {
        let hits = r0017_names(
            "local function f()\n\
             \tlocal count = 0\n\
             \tcount += 1\n\
             \tlocal function g()\n\
             \t\treturn count\n\
             \tend\n\
             \treturn g\n\
             end\n\
             return f\n",
        );
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn zero_reads_is_r0013_territory() {
        let hits = r0017_names(
            "local function f()\n\
             \tlocal x = 1\n\
             \tx = 2\n\
             end\n\
             return f\n",
        );
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn shadowing_does_not_leak_reads() {
        // Inner `x` is read externally; outer `x` only feeds itself.
        let hits = r0017_names(
            "local function f(g)\n\
             \tlocal x = 0\n\
             \tx = x + 1\n\
             \tdo\n\
             \t\tlocal x = 2\n\
             \t\tg(x)\n\
             \tend\n\
             end\n\
             return f\n",
        );
        assert_eq!(hits, vec!["x"]);
    }

    #[test]
    fn underscore_prefix_is_exempt() {
        let hits = r0017_names(
            "local function f()\n\
             \tlocal _count = 0\n\
             \t_count += 1\n\
             end\n\
             return f\n",
        );
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn root_body_accumulator_is_faint() {
        // Module scope is included since unused-vars v2 (2026-08-23) —
        // the survey exempted it; the rule does not.
        let hits = r0017_names("local total = 0\ntotal += 1\nreturn 0\n");
        assert_eq!(hits, vec!["total"]);
    }
}