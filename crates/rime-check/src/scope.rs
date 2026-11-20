//! Lexical name resolution over a whole module: which declaration a name
//! reaches at each place it is written or read, and what is assigned to
//! each declaration anywhere in the file.
//!
//! Rule-free. One walk, mirroring Luau's scoping (the same shape as the
//! walk in `rules::globals`, which records unresolved names instead):
//!   * `local a = e` binds after `e` is evaluated; `local function f` binds
//!     before its body
//!   * a nested function sees every binding around it, where it is created
//!   * `repeat ... until c` evaluates `c` inside the body's scope
//!   * `function f() end` assigns the binding `f` reaches, or a global
//!
//! A name no scope declares is a global. Globals written somewhere in the
//! file get a `Var` too (one per name); a global the file only reads has
//! none.
//!
//! Clients look a site up by the span the HIR already carries for it: an
//! `Expr::Name`, a `Target::Name`, or the whole `function f() end`
//! statement.

use rime_hir as hir;
use std::collections::HashMap;

pub(crate) type VarId = usize;

/// What one write stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stored {
    /// A function written in place: `local function f`, `function f()`,
    /// `f = function() end`.
    Function(hir::BodyId),
    /// Nothing: `local f`, `f = nil`, a name past the end of a value list
    /// that cannot expand.
    Nil,
    /// A value this walk does not look into: any other expression, a
    /// parameter, a loop variable, `x += 1`.
    Other,
}

#[derive(Debug, Clone)]
pub(crate) struct Write {
    pub stored: Stored,
    /// The declared name, the assignment target, or the whole
    /// `function f() end` statement.
    pub span: hir::Span,
}

#[derive(Debug)]
pub(crate) struct Var {
    /// Every write in the file, the declaration first for a local.
    pub writes: Vec<Write>,
}

/// What a name stands for at one site: one local declaration, or the
/// global of that name. Two sites with equal bindings name the same
/// variable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Binding {
    Local(VarId),
    Global(String),
}

#[derive(Debug, Default)]
pub(crate) struct Resolution {
    vars: Vec<Var>,
    /// Start offset of a site -> the local it reaches.
    sites: HashMap<usize, VarId>,
    globals: HashMap<String, VarId>,
}

impl Resolution {
    /// The local `name` reaches at the site that starts at `span`, or the
    /// file's own global of that name when no local is in scope there and
    /// the file writes the global somewhere.
    pub(crate) fn var(&self, name: &str, span: &hir::Span) -> Option<&Var> {
        self.sites
            .get(&span.start)
            .or_else(|| self.globals.get(name))
            .map(|id| &self.vars[*id])
    }

    /// Whether the name at `span` reaches a local.
    pub(crate) fn is_local(&self, span: &hir::Span) -> bool {
        self.sites.contains_key(&span.start)
    }

    /// The variable `name` stands for at the site that starts at `span`.
    /// Unlike `var`, a global the file never writes has a binding too.
    pub(crate) fn binding(&self, name: &str, span: &hir::Span) -> Binding {
        match self.sites.get(&span.start) {
            Some(id) => Binding::Local(*id),
            None => Binding::Global(name.to_string()),
        }
    }
}

pub(crate) fn resolve(module: &hir::Module) -> Resolution {
    let mut walk = Walk {
        module,
        out: Resolution::default(),
        scopes: Vec::new(),
    };
    walk.block(&module.body(module.root).block);
    walk.out
}

struct Walk<'a> {
    module: &'a hir::Module,
    out: Resolution,
    scopes: Vec<HashMap<String, VarId>>,
}

impl Walk<'_> {
    fn declare(&mut self, decl: &hir::LocalDecl, stored: Stored) {
        self.declare_name(&decl.name, decl.span.clone(), stored);
    }

    fn declare_name(&mut self, name: &str, span: hir::Span, stored: Stored) {
        let id = self.out.vars.len();
        self.out.sites.insert(span.start, id);
        self.out.vars.push(Var {
            writes: vec![Write { stored, span }],
        });
        self.scopes.last_mut().unwrap().insert(name.to_string(), id);
    }

    fn lookup(&self, name: &str) -> Option<VarId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    fn read(&mut self, name: &str, span: &hir::Span) {
        if let Some(id) = self.lookup(name) {
            self.out.sites.insert(span.start, id);
        }
    }

    fn write(&mut self, name: &str, span: &hir::Span, stored: Stored) {
        let id = match self.lookup(name) {
            Some(id) => {
                self.out.sites.insert(span.start, id);
                id
            }
            None => match self.out.globals.get(name) {
                Some(id) => *id,
                None => {
                    let id = self.out.vars.len();
                    self.out.vars.push(Var { writes: Vec::new() });
                    self.out.globals.insert(name.to_string(), id);
                    id
                }
            },
        };
        self.out.vars[id].writes.push(Write {
            stored,
            span: span.clone(),
        });
    }

    fn block(&mut self, block: &hir::Block) {
        self.scopes.push(HashMap::new());
        self.stmts(block);
        self.scopes.pop();
    }

    fn stmts(&mut self, block: &hir::Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            for e in exprs {
                self.expr(e);
            }
        }
    }

    fn function(&mut self, id: hir::BodyId, is_method: bool) {
        let module = self.module;
        let body = module.body(id);
        self.scopes.push(HashMap::new());
        if is_method {
            // The implicit `self` has no token, so no site maps to it. It
            // is declared only to hide an outer `self`.
            let var = self.out.vars.len();
            self.out.vars.push(Var {
                writes: vec![Write { stored: Stored::Other, span: 0..0 }],
            });
            self.scopes.last_mut().unwrap().insert("self".to_string(), var);
        }
        for p in &body.params {
            self.declare(p, Stored::Other);
        }
        self.block(&body.block);
        self.scopes.pop();
    }

    fn stmt(&mut self, stmt: &hir::Stmt) {
        use hir::StmtKind as K;
        match &stmt.kind {
            K::Local { decls, init } => {
                for e in init {
                    self.expr(e);
                }
                for (i, d) in decls.iter().enumerate() {
                    self.declare(d, stored(init, i));
                }
            }
            K::Assign { targets, values } => {
                for e in values {
                    self.expr(e);
                }
                for (i, t) in targets.iter().enumerate() {
                    match t {
                        hir::Target::Name { name, span } => {
                            self.write(name, span, stored(values, i));
                        }
                        hir::Target::Projection(e) => self.expr(e),
                    }
                }
            }
            K::CompoundAssign { target, value } => {
                self.expr(value);
                match target {
                    hir::Target::Name { name, span } => self.write(name, span, Stored::Other),
                    hir::Target::Projection(e) => self.expr(e),
                }
            }
            K::Call(e) => self.expr(e),
            K::Do(b) => self.block(b),
            K::If {
                cond,
                then,
                else_ifs,
                or_else,
            } => {
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
                self.scopes.push(HashMap::new());
                self.stmts(body);
                self.expr(until);
                self.scopes.pop();
            }
            K::NumericFor {
                var,
                start,
                end,
                step,
                body,
            } => {
                self.expr(start);
                self.expr(end);
                if let Some(s) = step {
                    self.expr(s);
                }
                self.scopes.push(HashMap::new());
                self.declare(var, Stored::Other);
                self.block(body);
                self.scopes.pop();
            }
            K::GenericFor { vars, exprs, body } => {
                for e in exprs {
                    self.expr(e);
                }
                self.scopes.push(HashMap::new());
                for v in vars {
                    self.declare(v, Stored::Other);
                }
                self.block(body);
                self.scopes.pop();
            }
            K::LocalFunction { decl, body } => {
                self.declare(decl, Stored::Function(*body));
                self.function(*body, false);
            }
            K::FunctionDecl {
                root,
                is_method,
                assigns_root,
                body,
                ..
            } => {
                if *assigns_root {
                    self.write(root, &stmt.span, Stored::Function(*body));
                } else {
                    self.read(root, &stmt.span);
                }
                self.function(*body, *is_method);
            }
        }
    }

    fn expr(&mut self, e: &hir::Expr) {
        use hir::Expr as E;
        match e {
            E::Name { name, span } => self.read(name, span),
            E::Function(id) => self.function(*id, false),
            E::Call { base, args, .. } => {
                self.expr(base);
                for a in args {
                    self.expr(a);
                }
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
            E::Composite(parts) => {
                for p in parts {
                    self.expr(p);
                }
            }
            E::Literal { .. } | E::Leaf => {}
        }
    }
}

/// What name `i` of a declaration or assignment receives from `values`.
fn stored(values: &[hir::Expr], i: usize) -> Stored {
    match values.get(i) {
        Some(hir::Expr::Function(id)) => Stored::Function(*id),
        Some(hir::Expr::Literal {
            prim: hir::Prim::Nil,
            ..
        }) => Stored::Nil,
        Some(_) => Stored::Other,
        // Past the end of the list: nil, unless the last value can expand
        // into several (a call, or `...`).
        None => match values.last() {
            Some(hir::Expr::Call { .. }) | Some(hir::Expr::Leaf) => Stored::Other,
            _ => Stored::Nil,
        },
    }
}

/// Every statement of `block`, nested blocks included, in source order.
/// Function bodies are not entered: each is its own entry in
/// `Module::bodies`.
pub(crate) fn each_stmt<'a>(block: &'a hir::Block, f: &mut dyn FnMut(&'a hir::Stmt)) {
    use hir::StmtKind as K;
    for stmt in &block.stmts {
        f(stmt);
        match &stmt.kind {
            K::Do(b) | K::While { body: b, .. } | K::Repeat { body: b, .. } => each_stmt(b, f),
            K::NumericFor { body, .. } | K::GenericFor { body, .. } => each_stmt(body, f),
            K::If {
                then,
                else_ifs,
                or_else,
                ..
            } => {
                each_stmt(then, f);
                for (_, b) in else_ifs {
                    each_stmt(b, f);
                }
                if let Some(b) = or_else {
                    each_stmt(b, f);
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every expression of `block`, outermost first, nested blocks included.
    /// Function bodies are not entered (see `each_stmt`). Only these tests
    /// walk expressions this way.
    fn each_expr<'a>(block: &'a hir::Block, f: &mut dyn FnMut(&'a hir::Expr)) {
        use hir::StmtKind as K;
        for stmt in &block.stmts {
            match &stmt.kind {
                K::Local { init, .. } => init.iter().for_each(|e| sub_exprs(e, f)),
                K::Assign { targets, values } => {
                    values.iter().for_each(|e| sub_exprs(e, f));
                    for t in targets {
                        if let hir::Target::Projection(e) = t {
                            sub_exprs(e, f);
                        }
                    }
                }
                K::CompoundAssign { target, value } => {
                    sub_exprs(value, f);
                    if let hir::Target::Projection(e) = target {
                        sub_exprs(e, f);
                    }
                }
                K::Call(e) => sub_exprs(e, f),
                K::Do(b) => each_expr(b, f),
                K::If {
                    cond,
                    then,
                    else_ifs,
                    or_else,
                } => {
                    sub_exprs(cond, f);
                    each_expr(then, f);
                    for (c, b) in else_ifs {
                        sub_exprs(c, f);
                        each_expr(b, f);
                    }
                    if let Some(b) = or_else {
                        each_expr(b, f);
                    }
                }
                K::While { cond, body } => {
                    sub_exprs(cond, f);
                    each_expr(body, f);
                }
                K::Repeat { body, until } => {
                    each_expr(body, f);
                    sub_exprs(until, f);
                }
                K::NumericFor {
                    start,
                    end,
                    step,
                    body,
                    ..
                } => {
                    sub_exprs(start, f);
                    sub_exprs(end, f);
                    if let Some(s) = step {
                        sub_exprs(s, f);
                    }
                    each_expr(body, f);
                }
                K::GenericFor { exprs, body, .. } => {
                    exprs.iter().for_each(|e| sub_exprs(e, f));
                    each_expr(body, f);
                }
                K::LocalFunction { .. } | K::FunctionDecl { .. } => {}
            }
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            exprs.iter().for_each(|e| sub_exprs(e, f));
        }
    }

    fn sub_exprs<'a>(e: &'a hir::Expr, f: &mut dyn FnMut(&'a hir::Expr)) {
        use hir::Expr as E;
        f(e);
        match e {
            E::Call { base, args, .. } => {
                sub_exprs(base, f);
                args.iter().for_each(|a| sub_exprs(a, f));
            }
            E::Index { base, key, .. } => {
                sub_exprs(base, f);
                if let Some(k) = key {
                    sub_exprs(k, f);
                }
            }
            E::BinOp { lhs, rhs, .. } => {
                sub_exprs(lhs, f);
                sub_exprs(rhs, f);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => sub_exprs(inner, f),
            E::Table(fields) => {
                for field in fields {
                    match field {
                        hir::TableField::Keyed { key, value } => {
                            sub_exprs(key, f);
                            sub_exprs(value, f);
                        }
                        hir::TableField::Named { value, .. } => sub_exprs(value, f),
                        hir::TableField::Item(e) => sub_exprs(e, f),
                    }
                }
            }
            E::IfExpr(arms) => {
                for (cond, val) in arms {
                    if let Some(c) = cond {
                        sub_exprs(c, f);
                    }
                    sub_exprs(val, f);
                }
            }
            E::Composite(parts) => parts.iter().for_each(|p| sub_exprs(p, f)),
            E::Function(_) | E::Name { .. } | E::Literal { .. } | E::Leaf => {}
        }
    }

    /// For each call `name(...)` in the file, in source order: the line of
    /// the call, and the lines of the writes to the variable it reaches
    /// (`None` when it reaches no variable the file writes).
    fn calls(src: &str) -> Vec<(usize, Option<Vec<usize>>)> {
        let module = rime_syntax::parse(src).unwrap();
        let res = resolve(&module);
        let line = |at: usize| src[..at].matches('\n').count() + 1;
        let mut out = Vec::new();
        for body in &module.bodies {
            each_expr(&body.block, &mut |e| {
                if let hir::Expr::Call { base, .. } = e {
                    if let hir::Expr::Name { name, span } = &**base {
                        let writes = res
                            .var(name, span)
                            .map(|v| v.writes.iter().map(|w| line(w.span.start)).collect());
                        out.push((line(span.start), writes));
                    }
                }
            });
        }
        out.sort();
        out
    }

    #[test]
    fn a_call_reaches_the_innermost_declaration_in_scope() {
        let src = "\
local function f() end
f()
do
	local function f() end
	f()
end
f()
local f = nil
f()
";
        assert_eq!(
            calls(src),
            vec![
                (2, Some(vec![1])),
                (5, Some(vec![4])),
                (7, Some(vec![1])),
                (9, Some(vec![8])),
            ]
        );
    }

    #[test]
    fn writes_from_any_scope_land_on_the_declaration_they_reach() {
        let src = "\
local f
local function set()
	f = function() end
end
function f() end
f()
return set
";
        assert_eq!(calls(src), vec![(6, Some(vec![1, 3, 5]))]);
    }

    #[test]
    fn a_parameter_or_loop_variable_hides_an_outer_name() {
        let src = "\
local function f() end
local function g(f)
	f()
end
for f in pairs({}) do
	f()
end
repeat
	local f = g
until f()
";
        let got = calls(src);
        assert_eq!(got.len(), 4);
        assert_eq!(got[0], (3, Some(vec![2])));
        // `pairs` is a global this file never writes.
        assert_eq!(got[1], (5, None));
        assert_eq!(got[2], (6, Some(vec![5])));
        assert_eq!(got[3], (10, Some(vec![9])));
    }

    // A site's binding tells one table from another of the same name. The
    // site of `function T.f() end` is the whole statement.
    #[test]
    fn sites_that_name_the_same_variable_have_equal_bindings() {
        let src = "\
local T = {}
function T.a() end
local function inner()
	local T = {}
	function T.a() end
	T.b = 1
end
T.b = 2
G.c = 3
local function other(G)
	G.c = 4
end
";
        let module = rime_syntax::parse(src).unwrap();
        let res = resolve(&module);
        // (line, binding) of every `function T.f` statement and of every
        // name a field is assigned through.
        let mut got: Vec<(usize, Binding)> = Vec::new();
        let line = |at: usize| src[..at].matches('\n').count() + 1;
        for body in &module.bodies {
            each_stmt(&body.block, &mut |stmt| match &stmt.kind {
                hir::StmtKind::FunctionDecl { root, path, .. } if !path.is_empty() => {
                    got.push((line(stmt.span.start), res.binding(root, &stmt.span)));
                }
                hir::StmtKind::Assign { targets, .. } => {
                    for t in targets {
                        let hir::Target::Projection(hir::Expr::Index { base, .. }) = t else {
                            continue;
                        };
                        if let hir::Expr::Name { name, span } = &**base {
                            got.push((line(span.start), res.binding(name, span)));
                        }
                    }
                }
                _ => {}
            });
        }
        got.sort_by_key(|(line, _)| *line);
        let at = |l: usize| &got.iter().find(|(line, _)| *line == l).unwrap().1;
        assert_eq!(got.len(), 6);
        assert_eq!(at(2), at(8));
        assert_eq!(at(5), at(6));
        assert_ne!(at(2), at(5));
        assert_eq!(at(9), &Binding::Global("G".to_string()));
        assert_ne!(at(9), at(11));
        assert!(matches!(at(11), Binding::Local(_)));
    }

    #[test]
    fn a_local_is_not_in_scope_in_its_own_initializer() {
        let src = "\
local function f() end
local f = f()
f()
";
        assert_eq!(calls(src), vec![(2, Some(vec![1])), (3, Some(vec![2]))]);
    }

    #[test]
    fn a_global_the_file_writes_is_one_variable_however_it_is_written() {
        let src = "\
function g() end
g()
g = nil
local function inner()
	g()
end
return inner
";
        assert_eq!(calls(src), vec![(2, Some(vec![1, 3])), (5, Some(vec![1, 3]))]);
    }

    #[test]
    fn what_a_write_stores() {
        let src = "\
local a, b, c = function() end, 1
local d, e = call()
local f
f = nil
a, b = function() end
";
        let module = rime_syntax::parse(src).unwrap();
        let res = resolve(&module);
        // Every write to the variable declared where `at` first occurs.
        let of = |at: &str| -> Vec<Stored> {
            let var = &res.vars[res.sites[&src.find(at).unwrap()]];
            var.writes.iter().map(|w| w.stored).collect()
        };
        assert!(of("a, b, c").iter().all(|s| matches!(s, Stored::Function(_))));
        assert_eq!(of("a, b, c").len(), 2);
        assert_eq!(of("b, c =").as_slice(), [Stored::Other, Stored::Nil]);
        assert_eq!(of("c =").as_slice(), [Stored::Nil]);
        assert_eq!(of("d, e").as_slice(), [Stored::Other]);
        assert_eq!(of("e = call").as_slice(), [Stored::Other]);
        assert_eq!(of("f\nf = nil").as_slice(), [Stored::Nil, Stored::Nil]);
    }
}
