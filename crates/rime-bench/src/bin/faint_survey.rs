//! Faint-variable survey, feeding the R0016 `self-contained-local` design
//! decision. HIR-based, same lowering the checker
//! uses. A local is FAINT when every read of it only feeds the local itself:
//!
//!   - a read in the RHS of an assignment back to the same local
//!     (`count += 1`, `s = s .. x` — the accumulator shape), or
//!   - a read in the condition of a self-guard `if`: no elseifs, no else,
//!     no terminator, and a body that only assigns the same local from
//!     effect-free expressions (`if not x then x = default end`).
//!
//! Loop conditions never count (they control how often effectful code
//! runs), reads inside call arguments never count (the value escapes), and
//! a candidate whose name appears in any other body is skipped as possibly
//! captured — a name-based overcount of capture, so the survey UNDERcounts
//! faint locals. Root-body locals are skipped to match the R0013 v1
//! module-scope exemption. Zero-read locals are R0013's territory and are
//! not counted here.
//!
//! Shape buckets tell the today-story:
//!   guard-in-loop  : draws today's R0104 with loop-carried framing (the
//!                    case in `guard_in_loop_is_faint` below)
//!   guard          : straight-line self-guard — today fully silent (the
//!                    guarded store is dead-to-end, which R0104 exempts)
//!   accum          : self-feeding accumulator/chain — today fully silent

use rime_hir as hir;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Shape {
    Accum,
    Guard,
    GuardInLoop,
}

#[derive(Debug, PartialEq)]
struct FaintHit {
    name: String,
    line: usize,
    shape: Shape,
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .to_string_lossy()
            .into_owned()
    });

    let mut files = 0usize;
    let mut parse_fail = 0usize;
    let mut hits: Vec<(String, FaintHit)> = Vec::new();

    for entry in walkdir::WalkDir::new(&dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x == "luau" || x == "lua")
        })
    {
        let path = entry.path().to_string_lossy().into_owned();
        let Ok(src) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        files += 1;
        let Ok(module) = rime_syntax::parse(&src) else {
            parse_fail += 1;
            continue;
        };
        for h in analyze(&module, &src) {
            hits.push((path.clone(), h));
        }
    }

    let count = |s: Shape| hits.iter().filter(|(_, h)| h.shape == s).count();
    println!("files scanned : {files}");
    println!("parse failures: {parse_fail}");
    println!();
    println!("FAINT LOCALS: {} total", hits.len());
    println!("  guard-in-loop (today: misleading R0104): {}", count(Shape::GuardInLoop));
    println!("  guard         (today: silent)          : {}", count(Shape::Guard));
    println!("  accum         (today: silent)          : {}", count(Shape::Accum));
    println!();
    for (path, h) in hits.iter().take(40) {
        println!("  {path}:{} `{}` [{:?}]", h.line, h.name, h.shape);
    }
    if hits.len() > 40 {
        println!("  ... and {} more", hits.len() - 40);
    }
}

fn analyze(module: &hir::Module, src: &str) -> Vec<FaintHit> {
    // Name-based capture approximation: every identifier occurrence per
    // body; a candidate whose name shows up in any OTHER body is skipped.
    let body_names: Vec<std::collections::HashSet<String>> =
        module.bodies.iter().map(collect_names).collect();

    let root = module.body(module.root);
    let mut hits = Vec::new();
    for (bi, body) in module.bodies.iter().enumerate() {
        if std::ptr::eq(body, root) {
            continue; // module-scope exemption, same as R0013 v1
        }
        let mut w = Walker { decls: Vec::new(), scopes: vec![Vec::new()], loop_depth: 0 };
        for p in &body.params {
            w.declare(&p.name, 0, false);
        }
        w.walk_block(&body.block);
        for d in &w.decls {
            if !d.candidate || d.external || d.shapes == 0 || d.name.starts_with('_') {
                continue;
            }
            let captured = body_names
                .iter()
                .enumerate()
                .any(|(i, names)| i != bi && names.contains(&d.name));
            if captured {
                continue;
            }
            let shape = if d.shapes & SHAPE_GUARD_IN_LOOP != 0 {
                Shape::GuardInLoop
            } else if d.shapes & SHAPE_GUARD != 0 {
                Shape::Guard
            } else {
                Shape::Accum
            };
            hits.push(FaintHit { name: d.name.clone(), line: line_of(src, d.span_start), shape });
        }
    }
    hits
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].matches('\n').count() + 1
}

const SHAPE_ACCUM: u8 = 1;
const SHAPE_GUARD: u8 = 2;
const SHAPE_GUARD_IN_LOOP: u8 = 4;

struct DeclState {
    name: String,
    span_start: usize,
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
    fn declare(&mut self, name: &str, span_start: usize, candidate: bool) {
        let id = self.decls.len();
        self.decls.push(DeclState {
            name: name.to_string(),
            span_start,
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
                    self.declare(&d.name, d.span.start, true);
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
                let guard = self_guard_target(then, else_ifs, or_else)
                    .and_then(|n| self.resolve(&n));
                match guard {
                    Some(id) => {
                        let shape = if self.loop_depth > 0 { SHAPE_GUARD_IN_LOOP } else { SHAPE_GUARD };
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
                self.declare(&var.name, var.span.start, false);
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
                    self.declare(&v.name, v.span.start, false);
                }
                self.walk_block(body);
                self.scopes.pop();
                self.loop_depth -= 1;
            }
            K::LocalFunction { decl, .. } => self.declare(&decl.name, decl.span.start, false),
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
            E::Function(_) => {} // separate body; capture approximation covers it
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

/// Every identifier occurrence in one body — reads and write-targets —
/// for the name-based capture approximation.
fn collect_names(body: &hir::Body) -> std::collections::HashSet<String> {
    fn expr(e: &hir::Expr, out: &mut std::collections::HashSet<String>) {
        use hir::Expr as E;
        match e {
            E::Name { name, .. } => {
                out.insert(name.clone());
            }
            E::Call { base, args, .. } => {
                expr(base, out);
                args.iter().for_each(|a| expr(a, out));
            }
            E::Index { base, key, .. } => {
                expr(base, out);
                if let Some(k) = key {
                    expr(k, out);
                }
            }
            E::BinOp { lhs, rhs, .. } => {
                expr(lhs, out);
                expr(rhs, out);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => expr(inner, out),
            E::Table(fields) => fields.iter().for_each(|f| match f {
                hir::TableField::Named { value, .. } => expr(value, out),
                hir::TableField::Keyed { key, value } => {
                    expr(key, out);
                    expr(value, out);
                }
                hir::TableField::Item(v) => expr(v, out),
            }),
            E::IfExpr(arms) => arms.iter().for_each(|(c, v)| {
                if let Some(c) = c {
                    expr(c, out);
                }
                expr(v, out);
            }),
            E::Composite(exprs) => exprs.iter().for_each(|x| expr(x, out)),
            E::Function(_) | E::Literal { .. } | E::Leaf => {}
        }
    }
    fn target(t: &hir::Target, out: &mut std::collections::HashSet<String>) {
        match t {
            hir::Target::Name { name, .. } => {
                out.insert(name.clone());
            }
            hir::Target::Projection(e) => expr(e, out),
        }
    }
    fn block(b: &hir::Block, out: &mut std::collections::HashSet<String>) {
        use hir::StmtKind as K;
        for stmt in &b.stmts {
            match &stmt.kind {
                K::Local { init, .. } => init.iter().for_each(|e| expr(e, out)),
                K::Assign { targets, values } => {
                    targets.iter().for_each(|t| target(t, out));
                    values.iter().for_each(|v| expr(v, out));
                }
                K::CompoundAssign { target: t, value } => {
                    target(t, out);
                    expr(value, out);
                }
                K::Call(e) => expr(e, out),
                K::Do(inner) => block(inner, out),
                K::If { cond, then, else_ifs, or_else } => {
                    expr(cond, out);
                    block(then, out);
                    for (c, bl) in else_ifs {
                        expr(c, out);
                        block(bl, out);
                    }
                    if let Some(bl) = or_else {
                        block(bl, out);
                    }
                }
                K::While { cond, body } => {
                    expr(cond, out);
                    block(body, out);
                }
                K::Repeat { body, until } => {
                    block(body, out);
                    expr(until, out);
                }
                K::NumericFor { start, end, step, body, .. } => {
                    expr(start, out);
                    expr(end, out);
                    if let Some(s) = step {
                        expr(s, out);
                    }
                    block(body, out);
                }
                K::GenericFor { exprs, body, .. } => {
                    exprs.iter().for_each(|e| expr(e, out));
                    block(body, out);
                }
                K::LocalFunction { .. } | K::FunctionDecl { .. } => {}
            }
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &b.terminator {
            exprs.iter().for_each(|e| expr(e, out));
        }
    }
    let mut out = std::collections::HashSet::new();
    block(&body.block, &mut out);
    out
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
/// compute a value (metatable pedantry accepted; this feeds a survey and a
/// conservative-flagged rule).
fn effect_free(e: &hir::Expr) -> bool {
    use hir::Expr as E;
    match e {
        E::Call { .. } => false,
        E::Name { .. } | E::Literal { .. } | E::Leaf | E::Function(_) => true,
        E::Index { base, key, .. } => {
            effect_free(base) && key.as_deref().is_none_or(effect_free)
        }
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
    use super::*;

    fn run(src: &str) -> Vec<FaintHit> {
        let module = rime_syntax::parse(src).expect("test source must parse");
        analyze(&module, src)
    }

    #[test]
    fn guard_in_loop_is_faint() {
        // The guard-in-loop reduction.
        let hits = run("local LABELS = { \"low\" }\n\
             local function f(levels)\n\
             \tfor index = 1, #levels do\n\
             \t\tlocal label = LABELS[levels[index]]\n\
             \t\tif not label then\n\
             \t\t\tlabel = \"?\"\n\
             \t\tend\n\
             \tend\n\
             end\n\
             return f\n");
        assert_eq!(
            hits,
            vec![FaintHit { name: "label".into(), line: 4, shape: Shape::GuardInLoop }]
        );
    }

    #[test]
    fn accumulator_is_faint() {
        let hits = run("local function f(list)\n\
             \tlocal count = 0\n\
             \tfor _, e in pairs(list) do\n\
             \t\tcount += 1\n\
             \tend\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![FaintHit { name: "count".into(), line: 2, shape: Shape::Accum }]);
    }

    #[test]
    fn self_feed_chain_is_faint() {
        let hits = run("local function f(a, b)\n\
             \tlocal s = \"\"\n\
             \ts = s .. a\n\
             \ts = s .. b\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![FaintHit { name: "s".into(), line: 2, shape: Shape::Accum }]);
    }

    #[test]
    fn straight_line_guard_is_faint() {
        let hits = run("local function f(t)\n\
             \tlocal x = t.v\n\
             \tif not x then\n\
             \t\tx = 1\n\
             \tend\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![FaintHit { name: "x".into(), line: 2, shape: Shape::Guard }]);
    }

    #[test]
    fn read_after_guard_is_not_faint() {
        let hits = run("local function f(t)\n\
             \tlocal x = t.v\n\
             \tif not x then\n\
             \t\tx = 1\n\
             \tend\n\
             \treturn x\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![]);
    }

    #[test]
    fn guard_with_call_is_not_faint() {
        // The guarded store runs a call the condition controls — `x`
        // affects behavior, so it is not faint.
        let hits = run("local function f(t, mk)\n\
             \tlocal x = t.v\n\
             \tif not x then\n\
             \t\tx = mk()\n\
             \tend\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![]);
    }

    #[test]
    fn loop_condition_read_is_not_faint() {
        let hits = run("local function f()\n\
             \tlocal done = false\n\
             \twhile not done do\n\
             \t\tdone = true\n\
             \tend\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![]);
    }

    #[test]
    fn read_escaping_into_call_is_not_faint() {
        let hits = run("local function f(g)\n\
             \tlocal count = 0\n\
             \tcount = g(count)\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![]);
    }

    #[test]
    fn projection_write_is_a_use() {
        // `x.field = 1` reads x's value as a table reference.
        let hits = run("local function f()\n\
             \tlocal x = {}\n\
             \tx = x\n\
             \tx.field = 1\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![]);
    }

    #[test]
    fn captured_local_is_skipped() {
        let hits = run("local function f()\n\
             \tlocal count = 0\n\
             \tcount += 1\n\
             \tlocal function g()\n\
             \t\treturn count\n\
             \tend\n\
             \treturn g\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![]);
    }

    #[test]
    fn zero_reads_is_r0013_territory() {
        let hits = run("local function f()\n\
             \tlocal x = 1\n\
             \tx = 2\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![]);
    }

    #[test]
    fn shadowing_does_not_leak_reads() {
        // Inner `x` is read externally; outer `x` only feeds itself.
        let hits = run("local function f(g)\n\
             \tlocal x = 0\n\
             \tx = x + 1\n\
             \tdo\n\
             \t\tlocal x = 2\n\
             \t\tg(x)\n\
             \tend\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![FaintHit { name: "x".into(), line: 2, shape: Shape::Accum }]);
    }

    #[test]
    fn underscore_prefix_is_exempt() {
        let hits = run("local function f()\n\
             \tlocal _count = 0\n\
             \t_count += 1\n\
             end\n\
             return f\n");
        assert_eq!(hits, vec![]);
    }
}
