//! R0509: circular-require — modules that require each other at load time.
//!
//! WHY: Roblox's `require` throws `Requested module was required
//! recursively` when a module that is still mid-load is required again.
//! With `A` requiring `B` and `B` requiring `A` at load time, whichever
//! loads second throws, and everything above it in the load chain fails
//! with it. A hard runtime error, so the firm form is an error.
//!
//! An EDGE is a `require(...)` that executes while the module loads: any
//! site in the ROOT body — top-level statements, and `do`/`if`/loop blocks
//! nested at top level. Requires inside function bodies are deferred: they
//! run when the function runs, which is the whole lazy-require idiom
//! (`local X = false and require(P)` at the top, `X = require(P)` in
//! init — common on a large codebase). Those are NOT edges, and neither
//! is the stub itself: `false and require(P)` never executes.
//!
//! Only `Resolution::Found` targets become edges. Missing / Dynamic /
//! OutsideView sites already carry their own codes (R0501/R0502/R0503),
//! and an edge Rime cannot see is silence, not a claim.
//!
//! Confidence: an edge under a top-level `if`/loop, or on the right of an
//! `and`/`or`, is CONDITIONAL. A cycle with an all-unconditional witness
//! path is firm; one that needs a conditional edge is conservative — a
//! `RunService:IsServer()` split can make the cycle unreachable, and Rime
//! cannot prove it either way.
//!
//! Per-occurrence (CLAUDE.md): every load-time require site whose target
//! sits in the same strongly-connected component as its own file gets its
//! own diagnostic, in its own file — a two-module cycle reports once in
//! each. Self-require (`A` requires `A`) is a one-node cycle and reports.
//!
//! Not claimed: cycles through functions CALLED at load time (a top-level
//! `init()` whose body requires). Silence, not a claim.

use rime_diag::{Code, Diagnostic, Label, Span};
use rime_hir as hir;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

/// One `require(...)` executed while the module loads, before resolution.
pub(crate) struct LoadSite<'m> {
    pub target: &'m hir::Expr,
    pub span: Span,
    /// Under a top-level `if`/loop or on the right of `and`/`or`.
    pub conditional: bool,
}

/// A resolved load-time require: the file it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoadEdge {
    pub target: PathBuf,
    pub span: Span,
    pub conditional: bool,
}

/// Every load-time require site in `module`'s root body, in source order.
pub(crate) fn load_sites(module: &hir::Module) -> Vec<LoadSite<'_>> {
    let mut out = Vec::new();
    walk_block(module, &module.body(module.root).block, false, &mut out);
    out
}

fn walk_block<'m>(
    module: &hir::Module,
    block: &'m hir::Block,
    cond: bool,
    out: &mut Vec<LoadSite<'m>>,
) {
    use hir::StmtKind as K;
    for stmt in &block.stmts {
        match &stmt.kind {
            K::Local { init, .. } => init.iter().for_each(|e| walk_expr(module, e, cond, out)),
            K::Assign { targets, values } => {
                for t in targets {
                    if let hir::Target::Projection(e) = t {
                        walk_expr(module, e, cond, out);
                    }
                }
                values.iter().for_each(|e| walk_expr(module, e, cond, out));
            }
            K::CompoundAssign { target, value } => {
                if let hir::Target::Projection(e) = target {
                    walk_expr(module, e, cond, out);
                }
                walk_expr(module, value, cond, out);
            }
            K::Call(e) => walk_expr(module, e, cond, out),
            K::Do(b) => walk_block(module, b, cond, out),
            K::If { cond: c, then, else_ifs, or_else } => {
                walk_expr(module, c, cond, out);
                // `if false then X = require(P) end` is the block form of
                // the lazy stub (a type-only import on a large codebase):
                // the block never runs.
                if !is_dead_guard(c) {
                    walk_block(module, then, true, out);
                }
                for (c, b) in else_ifs {
                    walk_expr(module, c, true, out);
                    walk_block(module, b, true, out);
                }
                if let Some(b) = or_else {
                    walk_block(module, b, true, out);
                }
            }
            K::While { cond: c, body } => {
                walk_expr(module, c, cond, out);
                walk_block(module, body, true, out);
            }
            K::Repeat { body, until } => {
                // A repeat body runs at least once — as unconditional as
                // the statement it sits in.
                walk_block(module, body, cond, out);
                walk_expr(module, until, cond, out);
            }
            K::NumericFor { start, end, step, body, .. } => {
                walk_expr(module, start, cond, out);
                walk_expr(module, end, cond, out);
                if let Some(s) = step {
                    walk_expr(module, s, cond, out);
                }
                walk_block(module, body, true, out);
            }
            K::GenericFor { exprs, body, .. } => {
                exprs.iter().for_each(|e| walk_expr(module, e, cond, out));
                walk_block(module, body, true, out);
            }
            // Function bodies are deferred — never an edge.
            K::LocalFunction { .. } | K::FunctionDecl { .. } => {}
        }
    }
    if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
        exprs.iter().for_each(|e| walk_expr(module, e, cond, out));
    }
}

fn walk_expr<'m>(
    module: &hir::Module,
    expr: &'m hir::Expr,
    cond: bool,
    out: &mut Vec<LoadSite<'m>>,
) {
    use hir::Expr as E;
    match expr {
        E::Call { base, args, method: None, span, .. } if module.is_require(base) => {
            if let Some(target) = args.first() {
                out.push(LoadSite { target, span: span.clone(), conditional: cond });
            }
            args.iter().for_each(|a| walk_expr(module, a, cond, out));
        }
        E::Call { base, args, .. } => {
            walk_expr(module, base, cond, out);
            args.iter().for_each(|a| walk_expr(module, a, cond, out));
        }
        E::Index { base, key, .. } => {
            walk_expr(module, base, cond, out);
            if let Some(k) = key {
                walk_expr(module, k, cond, out);
            }
        }
        E::BinOp { op: hir::BinOp::And | hir::BinOp::Or, lhs, rhs } => {
            walk_expr(module, lhs, cond, out);
            // `false and require(P)` / `nil and require(P)` is the lazy
            // stub: the require never executes. Any other short-circuit
            // right side executes only on some values of the left.
            if !is_dead_guard(lhs) {
                walk_expr(module, rhs, true, out);
            }
        }
        E::BinOp { lhs, rhs, .. } => {
            walk_expr(module, lhs, cond, out);
            walk_expr(module, rhs, cond, out);
        }
        E::Not(inner) | E::UnOp { operand: inner, .. } => walk_expr(module, inner, cond, out),
        E::Table(fields) => {
            for f in fields {
                match f {
                    hir::TableField::Keyed { key, value } => {
                        walk_expr(module, key, cond, out);
                        walk_expr(module, value, cond, out);
                    }
                    hir::TableField::Named { value, .. } => walk_expr(module, value, cond, out),
                    hir::TableField::Item(e) => walk_expr(module, e, cond, out),
                }
            }
        }
        E::IfExpr(arms) => {
            for (c, val) in arms {
                if let Some(c) = c {
                    walk_expr(module, c, cond, out);
                }
                walk_expr(module, val, true, out);
            }
        }
        E::Composite(parts) => parts.iter().for_each(|p| walk_expr(module, p, cond, out)),
        E::Name { .. } | E::Function(_) | E::Literal { .. } | E::Leaf => {}
    }
}

/// The left side of a lazy-require stub: literal `false` or `nil`.
fn is_dead_guard(e: &hir::Expr) -> bool {
    match e {
        hir::Expr::Literal { prim: hir::Prim::Boolean, text: Some(t), .. } => t == "false",
        hir::Expr::Literal { prim: hir::Prim::Nil, .. } => true,
        _ => false,
    }
}

/// R0509 over the whole project. `order` is every file; `edges` maps a
/// file to its resolved load-time requires (files with none may be
/// absent). Returns each file's diagnostics, keyed by file; files with no
/// cycle site are absent.
pub(crate) fn check(
    order: &[PathBuf],
    edges: &HashMap<&Path, &[LoadEdge]>,
) -> HashMap<PathBuf, Vec<Diagnostic>> {
    let index: HashMap<&Path, usize> =
        order.iter().enumerate().map(|(i, p)| (p.as_path(), i)).collect();
    let n = order.len();
    // Adjacency as indices; targets outside the project are not nodes.
    let mut adj: Vec<Vec<(usize, bool)>> = vec![Vec::new(); n];
    for (i, path) in order.iter().enumerate() {
        for e in edges.get(path.as_path()).copied().unwrap_or(&[]) {
            if let Some(&j) = index.get(e.target.as_path()) {
                adj[i].push((j, e.conditional));
            }
        }
    }
    let scc = tarjan(&adj);

    let mut out: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();
    for (i, path) in order.iter().enumerate() {
        for e in edges.get(path.as_path()).copied().unwrap_or(&[]) {
            let Some(&j) = index.get(e.target.as_path()) else { continue };
            if scc[j] != scc[i] {
                continue;
            }
            // Firm needs an all-unconditional way back; otherwise take any
            // way back within the component and say so.
            let firm = !e.conditional && witness(&adj, &scc, j, i, false).is_some();
            let back = if firm {
                witness(&adj, &scc, j, i, false)
            } else {
                witness(&adj, &scc, j, i, true)
            };
            let Some(back) = back else { continue };
            // `back` runs target → … → this file, so the cycle is this
            // file, then `back` — which already ends on this file again.
            let mut cycle: Vec<&Path> = vec![path.as_path()];
            cycle.extend(back.iter().map(|&k| order[k].as_path()));
            out.entry(path.clone())
                .or_default()
                .push(cycle_diag(&cycle, &e.span, firm));
        }
    }
    out
}

/// Shortest path `from` → … → `to` inside `from`'s component, as the nodes
/// visited INCLUDING both ends. `any_edge` = false walks unconditional
/// edges only. `from == to` is the one-node cycle (a self-require): just
/// `[from]`, which the caller renders as the file twice.
fn witness(
    adj: &[Vec<(usize, bool)>],
    scc: &[usize],
    from: usize,
    to: usize,
    any_edge: bool,
) -> Option<Vec<usize>> {
    if from == to {
        return Some(vec![from]);
    }
    let mut prev: HashMap<usize, usize> = HashMap::new();
    let mut seen: HashSet<usize> = HashSet::new();
    let mut queue = VecDeque::new();
    seen.insert(from);
    queue.push_back(from);
    while let Some(u) = queue.pop_front() {
        for &(v, conditional) in &adj[u] {
            if (conditional && !any_edge) || scc[v] != scc[from] || !seen.insert(v) {
                continue;
            }
            prev.insert(v, u);
            if v == to {
                let mut path = vec![to];
                let mut cur = to;
                while cur != from {
                    cur = prev[&cur];
                    path.push(cur);
                }
                path.reverse();
                return Some(path);
            }
            queue.push_back(v);
        }
    }
    None
}

/// Tarjan's SCC: component id per node. Iterative on purpose — a DFS over
/// a large game's require graph goes deep
/// enough that the recursive form overflows the 1MB Windows main-thread
/// stack in a debug build.
fn tarjan(adj: &[Vec<(usize, bool)>]) -> Vec<usize> {
    const UNVISITED: usize = usize::MAX;
    let n = adj.len();
    let mut index = vec![UNVISITED; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut comp = vec![0; n];
    let mut next = 0;
    let mut ncomp = 0;
    // The explicit DFS stack: (node, next out-edge to look at).
    let mut work: Vec<(usize, usize)> = Vec::new();
    for root in 0..n {
        if index[root] != UNVISITED {
            continue;
        }
        index[root] = next;
        low[root] = next;
        next += 1;
        stack.push(root);
        on_stack[root] = true;
        work.push((root, 0));
        while let Some(&(v, k)) = work.last() {
            if k < adj[v].len() {
                work.last_mut().expect("just peeked").1 += 1;
                let w = adj[v][k].0;
                if index[w] == UNVISITED {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
                continue;
            }
            work.pop();
            if low[v] == index[v] {
                loop {
                    let w = stack.pop().expect("tarjan stack");
                    on_stack[w] = false;
                    comp[w] = ncomp;
                    if w == v {
                        break;
                    }
                }
                ncomp += 1;
            }
            if let Some(&(u, _)) = work.last() {
                low[u] = low[u].min(low[v]);
            }
        }
    }
    comp
}

fn cycle_diag(cycle: &[&Path], span: &Span, firm: bool) -> Diagnostic {
    let base = common_dir(cycle);
    let name = |p: &Path| -> String {
        p.strip_prefix(&base)
            .unwrap_or(p)
            .display()
            .to_string()
            .replace('\\', "/")
    };
    let target = name(cycle[1]);
    let rendered: Vec<String> = cycle.iter().map(|p| name(p)).collect();
    let mut d = Diagnostic::error(
        Code(509),
        format!("circular require: loading `{target}` leads back to this module"),
    )
    .with_label(Label::primary(span.clone(), format!("load-time require of `{target}`")))
    .with_note(format!("cycle: {}", rendered.join(" → ")));
    if !firm {
        d = d.conservative(
            "the cycle passes through a require under a top-level condition; if \
             those branches never run together, the cycle is unreachable — Rime \
             cannot tell",
        );
    }
    d.with_note(
        "Roblox throws `Requested module was required recursively` when the \
         second module in the cycle loads",
    )
    .with_help(
        "break the cycle: keep `local X = false and require(...)` at \
         the top and load for real inside the function that needs it, \
         or move the shared part into a third module",
    )
}

/// The deepest directory containing every path in `paths`.
fn common_dir(paths: &[&Path]) -> PathBuf {
    let mut base = paths[0].parent().unwrap_or(paths[0]).to_path_buf();
    while !paths.iter().all(|p| p.starts_with(&base)) {
        if !base.pop() {
            break;
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `conditional` flag of every load-time site, in source order.
    fn flags(src: &str) -> Vec<bool> {
        let module = rime_syntax::parse(src).expect("parses");
        load_sites(&module).iter().map(|s| s.conditional).collect()
    }

    #[test]
    fn plain_top_level_requires_are_unconditional_sites() {
        let src = "local A = require(script.Parent.a)\n\
                   do local B = require(script.Parent.b) end\n\
                   return require(script.Parent.c)\n";
        assert_eq!(flags(src), vec![false, false, false]);
    }

    #[test]
    fn function_bodies_are_never_sites() {
        let src = "local function f() return require(script.Parent.a) end\n\
                   task.spawn(function() require(script.Parent.b) end)\n\
                   local T = { g = function() return require(script.Parent.c) end }\n\
                   return f\n";
        assert_eq!(flags(src), Vec::<bool>::new());
    }

    #[test]
    fn dead_guards_are_not_sites() {
        let src = "local A = false and require(script.Parent.a)\n\
                   local B = nil and require(script.Parent.b)\n\
                   if false then A = require(script.Parent.a) end\n\
                   if nil then B = require(script.Parent.b) end\n\
                   return A\n";
        assert_eq!(flags(src), Vec::<bool>::new());
    }

    #[test]
    fn dead_if_still_walks_its_else() {
        let src = "local A\nif false then A = 1 else A = require(script.Parent.a) end\nreturn A\n";
        assert_eq!(flags(src), vec![true]);
    }

    #[test]
    fn branches_and_loops_are_conditional() {
        let src = "local A, B, C, D, E\n\
                   if flag then A = require(script.Parent.a) end\n\
                   while flag do B = require(script.Parent.b) end\n\
                   for i = 1, 2 do C = require(script.Parent.c) end\n\
                   for _, x in pairs(t) do D = require(script.Parent.d) end\n\
                   repeat E = require(script.Parent.e) until true\n\
                   return A\n";
        assert_eq!(flags(src), vec![true, true, true, true, false]);
    }

    #[test]
    fn short_circuit_right_sides_and_if_expr_arms_are_conditional() {
        let src = "local A = flag and require(script.Parent.a)\n\
                   local B = require(script.Parent.b) or nil\n\
                   local C = if flag then require(script.Parent.c) else require(script.Parent.d)\n\
                   return A\n";
        assert_eq!(flags(src), vec![true, false, true, true]);
    }

    #[test]
    fn pcall_require_is_not_a_site() {
        let src = "local ok, M = pcall(require, script.Parent.a)\nreturn M\n";
        assert_eq!(flags(src), Vec::<bool>::new());
    }

    #[test]
    fn witness_is_rendered_relative_to_the_common_dir() {
        let a = PathBuf::from("/p/src/Server/A.lua");
        let b = PathBuf::from("/p/src/Client/Deep/B.lua");
        let ab = vec![LoadEdge { target: b.clone(), span: 0..1, conditional: false }];
        let ba = vec![LoadEdge { target: a.clone(), span: 0..1, conditional: false }];
        let mut edges: HashMap<&Path, &[LoadEdge]> = HashMap::new();
        edges.insert(&a, &ab);
        edges.insert(&b, &ba);
        let out = check(&[a.clone(), b.clone()], &edges);
        let d = &out[&a][0];
        assert!(!d.conservative);
        assert_eq!(d.notes[0], "cycle: Server/A.lua → Client/Deep/B.lua → Server/A.lua");
        assert_eq!(out[&b][0].notes[0], "cycle: Client/Deep/B.lua → Server/A.lua → Client/Deep/B.lua");
    }

    #[test]
    fn conditional_only_way_back_is_conservative() {
        let a = PathBuf::from("/p/A.lua");
        let b = PathBuf::from("/p/B.lua");
        let ab = vec![LoadEdge { target: b.clone(), span: 0..1, conditional: false }];
        let ba = vec![LoadEdge { target: a.clone(), span: 0..1, conditional: true }];
        let mut edges: HashMap<&Path, &[LoadEdge]> = HashMap::new();
        edges.insert(&a, &ab);
        edges.insert(&b, &ba);
        let out = check(&[a.clone(), b.clone()], &edges);
        assert!(out[&a][0].conservative, "A's edge is firm but the only way back is conditional");
        assert!(out[&b][0].conservative);
    }

    #[test]
    fn diamond_has_no_cycle() {
        let m = PathBuf::from("/p/main.lua");
        let a = PathBuf::from("/p/a.lua");
        let b = PathBuf::from("/p/b.lua");
        let c = PathBuf::from("/p/c.lua");
        let e = |t: &PathBuf| vec![LoadEdge { target: t.clone(), span: 0..1, conditional: false }];
        let ma = [e(&a), e(&b)].concat();
        let ac = e(&c);
        let bc = e(&c);
        let mut edges: HashMap<&Path, &[LoadEdge]> = HashMap::new();
        edges.insert(&m, &ma);
        edges.insert(&a, &ac);
        edges.insert(&b, &bc);
        let out = check(&[m.clone(), a.clone(), b.clone(), c.clone()], &edges);
        assert!(out.is_empty());
    }
}
