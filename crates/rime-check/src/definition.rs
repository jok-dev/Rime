//! Go-to-definition. Given a byte offset in a file the
//! session holds, find what a Ctrl+click there should jump to: the file a
//! `require(...)` names, the declaration of a required module's member, or
//! the binding site of a local / parameter / upvalue / `local function` /
//! global function in the same file.
//!
//! This reuses the resolution the checker already does — the require
//! resolver, the per-file `deps` map, and the extracted `ModuleInterface`
//! member spans — so a jump lands exactly where R0306/R0501 would point.
//! It reads only resident state (`&Session`), never re-runs a pass, so it
//! answers in microseconds against whatever the last edit left behind.

use crate::project::{local_env, Resolution};
use crate::rules::calls::{exported_name, ModuleInterface};
use crate::session::Session;
use rime_hir as hir;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Where a jump lands. `span` is a byte range in `path`'s source; a require
/// target that names a whole file uses `0..0` (the file's start).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub path: PathBuf,
    pub span: Range<usize>,
}

impl<'r> Session<'r> {
    /// The definition the token at byte `offset` in `path` refers to, or
    /// `None` when `path` is not held, did not parse, or the offset is not
    /// on a resolvable name.
    pub fn definition(&self, path: &Path, offset: usize) -> Option<Definition> {
        let state = self.file_state(path)?;
        let module = state.module.as_ref()?;
        let env = local_env(module);
        let finder = Finder {
            session: self,
            path,
            offset,
            src: &state.src,
            deps: &state.deps,
            exported: exported_name(module),
            env,
            module,
        };
        // Walk every body; the target lives in exactly one. Params are in
        // scope for their body, so seed each body's scope with them.
        finder.search()
    }
}

/// The winning binding for a plain name, from the scope stack.
struct Scope {
    /// Declarations visible in this scope, in program order: (name, decl
    /// span, is_module_root). Later entries shadow earlier ones.
    decls: Vec<(String, Range<usize>, bool)>,
}

struct Finder<'a, 'r> {
    session: &'a Session<'r>,
    path: &'a Path,
    offset: usize,
    src: &'a str,
    deps: &'a [(String, PathBuf, PathBuf)],
    exported: Option<String>,
    env: std::collections::HashMap<String, &'a hir::Expr>,
    module: &'a hir::Module,
}

/// Walking a body threads a mutable scope stack; a hit unwinds immediately.
struct Walk<'a, 'r, 'f> {
    f: &'f Finder<'a, 'r>,
    scopes: Vec<Scope>,
    /// Top-level (module-root body) global function decls: `function foo()`
    /// with no dotted path. A global name that no local shadows resolves
    /// here. Built once, order-independent (globals do not obey lexical
    /// order the way locals do).
    globals: &'f std::collections::HashMap<String, Range<usize>>,
}

impl<'a, 'r> Finder<'a, 'r> {
    fn search(&self) -> Option<Definition> {
        let mut globals = std::collections::HashMap::new();
        let root = self.module.body(self.module.root);
        for stmt in &root.block.stmts {
            if let hir::StmtKind::FunctionDecl { root: name, path, .. } = &stmt.kind {
                if path.is_empty() {
                    globals.entry(name.clone()).or_insert(stmt.span.clone());
                }
            }
        }
        let mut w = Walk { f: self, scopes: Vec::new(), globals: &globals };
        w.body(self.module.root, true)
    }

    /// A byte offset is inside `span`, treating it inclusively at the end so
    /// a click just past the last char of a one-char name still hits.
    fn hits(&self, span: &Range<usize>) -> bool {
        self.offset >= span.start && self.offset <= span.end
    }

    /// Resolve a dotted member path — `root.a.b.c`, with `root` a top-level
    /// require binding or the module's own export — to the declaration of
    /// its last segment. A single segment (`root.a`) uses the pre-extracted
    /// interface (functions keep their narrowed name span); deeper paths
    /// walk the defining module's HIR, descending through table literals
    /// (`Limits = { Retries = ... }`). `base_is_root` gates the jump:
    /// a shadowing inner local of the root name resolves nothing.
    fn member_path(&self, root: &str, segs: &[String], base_is_root: bool) -> Option<Definition> {
        if !base_is_root || segs.is_empty() {
            return None;
        }
        // Where the root's members live: a required module (by terminal
        // path), or this file's own export.
        let (module_path, module): (PathBuf, &hir::Module) =
            if let Some((_, _, terminal)) = self.deps.iter().find(|(l, _, _)| l == root) {
                (terminal.clone(), self.session.file_state(terminal)?.module.as_ref()?)
            } else if self.exported.as_deref() == Some(root) {
                (self.path.to_path_buf(), self.module)
            } else {
                return None;
            };
        let iface = &self.session.file_state(&module_path)?.interface;
        // One level: the interface already has it (with function name spans
        // narrowed). Fall back to the HIR walk for a table-valued member the
        // interface only records the name of.
        if segs.len() == 1 {
            if let Some(span) = field_span(iface, &segs[0]) {
                return Some(Definition { path: module_path, span });
            }
        }
        let exported = exported_name(module)?;
        member_path_span(module, &exported, segs).map(|span| Definition { path: module_path, span })
    }

    /// A `require(<target>)` whose argument covers the offset: resolve the
    /// target to the file it names.
    ///
    /// Cheap `Resolver::resolve` first, then — exactly as `check_file` and
    /// `requires::check` do on their own `Dynamic` — the require-target evaluator.
    /// Without that second ask, every site the checker now resolves through
    /// an exported value (`require(Sdk.Modules.Flags)`) would report
    /// no diagnostic AND refuse to Ctrl+click: the editor would silently
    /// disagree with the checker about the same require.
    /// Anything but a single `Found` — `NilCapable`,
    /// `Diverged`, `Opaque`, a non-`Found` `Resolution` — is not a jump
    /// target; the click simply does not fire, same as before.
    fn require_jump(&self, target: &hir::Expr) -> Option<Definition> {
        let resolver = self.session.resolver_ref()?;
        let file = match resolver.resolve(self.path, target, &self.env) {
            Resolution::Found(file) => file,
            Resolution::Dynamic => {
                let eval = crate::eval::Evaluator::new(resolver, self.session);
                match eval.eval_require_target(self.path, self.module, target) {
                    crate::eval::Outcome::Resolved(Resolution::Found(file)) => file,
                    _ => return None,
                }
            }
            _ => return None,
        };
        Some(Definition { path: file, span: 0..0 })
    }
}

/// A member's declaration span within a module interface: functions and
/// methods carry a name-token span; static fields carry theirs.
fn field_span(iface: &ModuleInterface, field: &str) -> Option<Range<usize>> {
    if let Some(sig) = iface.functions.get(field) {
        return Some(sig.decl_span.clone());
    }
    iface.fields.get(field).cloned()
}

/// Walk a dotted path `[a, b, c]` from a module's exported table to the
/// declaration span of its LAST segment, descending through table literals.
/// `a` is a top-level member of the export (a `E.a = <table>` write or a
/// constructor field); each further segment is a `Named` field of the
/// previous segment's table-literal value. `None` if any hop is not a
/// statically-known named table entry (a computed key, a function, a value
/// from elsewhere) — the jump simply does not fire rather than guess.
fn member_path_span(module: &hir::Module, exported: &str, segs: &[String]) -> Option<Range<usize>> {
    let members = export_table_members(module, exported);
    let (mut span, mut value) = members.get(&segs[0]).cloned()?;
    for seg in &segs[1..] {
        let hir::Expr::Table(fields) = value? else { return None };
        let (s, v) = named_field(fields, seg)?;
        span = s;
        value = v;
    }
    Some(span)
}

/// The exported table's directly-named members: constructor `local E = { a =
/// v, ... }` fields and top-level `E.a = v` writes. Each maps to (name-token
/// span, the value expression) so a nested path can descend into a value
/// that is itself a table literal. Functions are not included — they are
/// leaves the interface already resolves, and cannot be indexed further.
fn export_table_members<'m>(
    module: &'m hir::Module,
    exported: &str,
) -> std::collections::HashMap<String, (Range<usize>, Option<&'m hir::Expr>)> {
    let mut out = std::collections::HashMap::new();
    let root = module.body(module.root);
    for stmt in &root.block.stmts {
        match &stmt.kind {
            // `local E = { a = ..., ... }`
            hir::StmtKind::Local { decls, init } => {
                for (d, e) in decls.iter().zip(init) {
                    if d.name == exported {
                        if let hir::Expr::Table(fields) = e {
                            for (name, span, value) in named_fields(fields) {
                                out.entry(name).or_insert((span, Some(value)));
                            }
                        }
                    }
                }
            }
            // `E.a = <value>`
            hir::StmtKind::Assign { targets, values } => {
                for (t, v) in targets.iter().zip(values) {
                    if let Some((name, span)) = static_field_write(t, exported) {
                        out.entry(name).or_insert((span, Some(v)));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// `E.name = ...` as an assignment target: the written member's name and the
/// span of its name token. `None` for anything else (a plain local, a deeper
/// path, a bracket key).
fn static_field_write(target: &hir::Target, exported: &str) -> Option<(String, Range<usize>)> {
    let hir::Target::Projection(hir::Expr::Index { base, key: None, name: Some(n), name_span }) = target
    else {
        return None;
    };
    if matches!(&**base, hir::Expr::Name { name, .. } if name == exported) {
        return Some((n.clone(), name_span.clone()?));
    }
    None
}

/// A table literal's `Named` fields as (name, key span, value).
fn named_fields(fields: &[hir::TableField]) -> Vec<(String, Range<usize>, &hir::Expr)> {
    fields
        .iter()
        .filter_map(|f| match f {
            hir::TableField::Named { name, span, value } => Some((name.clone(), span.clone(), value)),
            _ => None,
        })
        .collect()
}

/// One `Named` field of a table literal by name: its key span and value.
fn named_field<'m>(
    fields: &'m [hir::TableField],
    name: &str,
) -> Option<(Range<usize>, Option<&'m hir::Expr>)> {
    fields.iter().find_map(|f| match f {
        hir::TableField::Named { name: n, span, value } if n == name => {
            Some((span.clone(), Some(value)))
        }
        _ => None,
    })
}

impl<'a, 'r, 'f> Walk<'a, 'r, 'f> {
    /// Resolve a plain name against the current scope stack (innermost
    /// first), then the module's global functions. Returns the decl span and
    /// whether it came from the module-root scope.
    fn resolve_name(&self, name: &str) -> Option<(Range<usize>, bool)> {
        for scope in self.scopes.iter().rev() {
            if let Some((_, span, root)) = scope.decls.iter().rev().find(|(n, _, _)| n == name) {
                return Some((span.clone(), *root));
            }
        }
        self.globals.get(name).map(|s| (s.clone(), true))
    }

    fn declare(&mut self, decl: &hir::LocalDecl, root: bool) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.decls.push((decl.name.clone(), decl.span.clone(), root));
        }
    }

    /// The cursor is on a declaration's own name token (`local x`, a
    /// parameter, a loop variable, a `local function` name): the jump is to
    /// itself, so hover renders the declaration in place.
    fn decl_hit(&self, decl: &hir::LocalDecl) -> Option<Definition> {
        self.f
            .hits(&decl.span)
            .then(|| Definition { path: self.f.path.to_path_buf(), span: decl.span.clone() })
    }

    /// The cursor is on a `function foo` / `function M.foo` / `function M:m`
    /// declaration's own name token: resolve to that token (the function's
    /// own name — the last dotted/`:` segment, or the root). The name span
    /// is recovered from source, since `FunctionDecl` keeps no per-segment
    /// span. `None` when the cursor is elsewhere on the header (or in the
    /// body, which the caller walks separately).
    fn fn_decl_hit(&self, stmt: &hir::Stmt, root: &str, path: &[String]) -> Option<Definition> {
        let hir::StmtKind::FunctionDecl { .. } = &stmt.kind else { return None };
        let start = stmt.span.start.min(self.f.src.len());
        let header_end =
            self.f.src[start..].find('(').map(|i| start + i).unwrap_or(stmt.span.end.min(self.f.src.len()));
        let header = self.f.src.get(start..header_end)?;
        let name = path.last().map(String::as_str).unwrap_or(root);
        // The name is the last identifier in the header (after the final
        // `.`/`:`), so its last occurrence is the declaration token.
        let pos = header.rfind(name)?;
        let span = (start + pos)..(start + pos + name.len());
        self.f
            .hits(&span)
            .then(|| Definition { path: self.f.path.to_path_buf(), span })
    }

    /// Walk one body. `root` marks the module top level, whose bindings are
    /// the ones a required-module member access may sit on.
    fn body(&mut self, id: hir::BodyId, root: bool) -> Option<Definition> {
        let body = self.f.module.body(id);
        self.scopes.push(Scope { decls: Vec::new() });
        for p in &body.params {
            if let Some(d) = self.decl_hit(p) {
                self.scopes.pop();
                return Some(d);
            }
            self.declare(p, root);
        }
        let hit = self.block(&body.block, root);
        self.scopes.pop();
        hit
    }

    fn block(&mut self, block: &hir::Block, root: bool) -> Option<Definition> {
        self.scopes.push(Scope { decls: Vec::new() });
        let hit = self.block_inner(block, root);
        // On a hit the resolution is already captured; leaving the scope on
        // the stack is harmless because the walk stops.
        if hit.is_none() {
            self.scopes.pop();
        }
        hit
    }

    fn block_inner(&mut self, block: &hir::Block, root: bool) -> Option<Definition> {
        use hir::StmtKind as K;
        for stmt in &block.stmts {
            match &stmt.kind {
                K::Local { decls, init } => {
                    // Initializers see the OUTER bindings (`local x = x`), so
                    // walk them before declaring the new names.
                    for e in init {
                        if let Some(d) = self.expr(e, root) {
                            return Some(d);
                        }
                    }
                    for d in decls {
                        if let Some(hit) = self.decl_hit(d) {
                            return Some(hit);
                        }
                        self.declare(d, root);
                    }
                }
                K::LocalFunction { decl, body } => {
                    // The name binds before its body, so recursion resolves.
                    if let Some(hit) = self.decl_hit(decl) {
                        return Some(hit);
                    }
                    self.declare(decl, root);
                    if let Some(d) = self.body(*body, false) {
                        return Some(d);
                    }
                }
                K::FunctionDecl { root: r, path, body, .. } => {
                    // `function foo()` / `function M.f()`: the name/path is a
                    // global or member decl, not a lexical binding here.
                    // Hovering the declaration's own name resolves to it; the
                    // body is still walked for names inside it.
                    if let Some(hit) = self.fn_decl_hit(stmt, r, path) {
                        return Some(hit);
                    }
                    if let Some(d) = self.body(*body, false) {
                        return Some(d);
                    }
                }
                K::Assign { targets, values } => {
                    for t in targets {
                        if let hir::Target::Projection(e) = t {
                            if let Some(d) = self.expr(e, root) {
                                return Some(d);
                            }
                        }
                    }
                    for v in values {
                        if let Some(d) = self.expr(v, root) {
                            return Some(d);
                        }
                    }
                }
                K::CompoundAssign { target, value } => {
                    if let hir::Target::Projection(e) = target {
                        if let Some(d) = self.expr(e, root) {
                            return Some(d);
                        }
                    }
                    if let Some(d) = self.expr(value, root) {
                        return Some(d);
                    }
                }
                K::Call(e) => {
                    if let Some(d) = self.expr(e, root) {
                        return Some(d);
                    }
                }
                K::Do(b) => {
                    if let Some(d) = self.block(b, false) {
                        return Some(d);
                    }
                }
                K::If { cond, then, else_ifs, or_else } => {
                    if let Some(d) = self.expr(cond, root) {
                        return Some(d);
                    }
                    if let Some(d) = self.block(then, false) {
                        return Some(d);
                    }
                    for (c, b) in else_ifs {
                        if let Some(d) = self.expr(c, root) {
                            return Some(d);
                        }
                        if let Some(d) = self.block(b, false) {
                            return Some(d);
                        }
                    }
                    if let Some(b) = or_else {
                        if let Some(d) = self.block(b, false) {
                            return Some(d);
                        }
                    }
                }
                K::While { cond, body } => {
                    if let Some(d) = self.expr(cond, root) {
                        return Some(d);
                    }
                    if let Some(d) = self.block(body, false) {
                        return Some(d);
                    }
                }
                K::Repeat { body, until } => {
                    // The `until` condition sees the body's locals: keep one
                    // scope across both.
                    self.scopes.push(Scope { decls: Vec::new() });
                    let hit = self.block_inner(body, false).or_else(|| self.expr(until, false));
                    if hit.is_none() {
                        self.scopes.pop();
                    }
                    if hit.is_some() {
                        return hit;
                    }
                }
                K::NumericFor { var, start, end, step, body } => {
                    if let Some(d) = self.expr(start, root) {
                        return Some(d);
                    }
                    if let Some(d) = self.expr(end, root) {
                        return Some(d);
                    }
                    if let Some(s) = step {
                        if let Some(d) = self.expr(s, root) {
                            return Some(d);
                        }
                    }
                    if let Some(hit) = self.decl_hit(var) {
                        return Some(hit);
                    }
                    self.scopes.push(Scope { decls: Vec::new() });
                    self.declare(var, false);
                    let hit = self.block_inner(body, false);
                    if hit.is_none() {
                        self.scopes.pop();
                    }
                    if hit.is_some() {
                        return hit;
                    }
                }
                K::GenericFor { vars, exprs, body } => {
                    for e in exprs {
                        if let Some(d) = self.expr(e, root) {
                            return Some(d);
                        }
                    }
                    for v in vars {
                        if let Some(hit) = self.decl_hit(v) {
                            return Some(hit);
                        }
                    }
                    self.scopes.push(Scope { decls: Vec::new() });
                    for v in vars {
                        self.declare(v, false);
                    }
                    let hit = self.block_inner(body, false);
                    if hit.is_none() {
                        self.scopes.pop();
                    }
                    if hit.is_some() {
                        return hit;
                    }
                }
            }
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            for e in exprs {
                if let Some(d) = self.expr(e, root) {
                    return Some(d);
                }
            }
        }
        None
    }

    /// Walk an expression, innermost node first, stopping at the offset.
    fn expr(&mut self, e: &hir::Expr, root: bool) -> Option<Definition> {
        match e {
            hir::Expr::Call { base, args, method, span, arg_spans } => {
                // A `require(<target>)` whose argument covers the offset wins
                // over any member access inside the argument chain.
                if self.f.module.is_require(base) {
                    if let (Some(target), Some(arg_span)) = (args.first(), arg_spans.first()) {
                        if self.f.hits(arg_span) {
                            return self.f.require_jump(target);
                        }
                    }
                }
                for a in args {
                    if let Some(d) = self.expr(a, root) {
                        return Some(d);
                    }
                }
                if let Some(d) = self.expr(base, root) {
                    return Some(d);
                }
                // `base:method(...)`: the method name has no span of its own.
                // A click inside the call but past the base resolves it as a
                // member of the base.
                if let Some(m) = method {
                    if self.f.hits(span) && self.offset() > base_end(base) {
                        if let Some(d) = self.member_on(base, m, root) {
                            return Some(d);
                        }
                    }
                }
                None
            }
            hir::Expr::Index { base, key, name, name_span } => {
                if let (Some(field), Some(sp)) = (name, name_span) {
                    if self.f.hits(sp) {
                        // A member read: try to resolve it against the base's
                        // module; if that fails, fall through to the base.
                        if let Some(d) = self.member_on(base, field, root) {
                            return Some(d);
                        }
                    }
                }
                if let Some(k) = key {
                    if let Some(d) = self.expr(k, root) {
                        return Some(d);
                    }
                }
                self.expr(base, root)
            }
            hir::Expr::Name { name, span } => {
                if self.f.hits(span) {
                    if let Some((decl, _)) = self.resolve_name(name) {
                        return Some(Definition { path: self.f.path.to_path_buf(), span: decl });
                    }
                }
                None
            }
            hir::Expr::Function(id) => self.body(*id, false),
            hir::Expr::BinOp { lhs, rhs, .. } => {
                self.expr(lhs, root).or_else(|| self.expr(rhs, root))
            }
            hir::Expr::Not(inner) => self.expr(inner, root),
            hir::Expr::UnOp { operand, .. } => self.expr(operand, root),
            hir::Expr::Table(fields) => {
                for field in fields {
                    let hit = match field {
                        hir::TableField::Named { value, .. } => self.expr(value, root),
                        hir::TableField::Keyed { key, value } => {
                            self.expr(key, root).or_else(|| self.expr(value, root))
                        }
                        hir::TableField::Item(e) => self.expr(e, root),
                    };
                    if hit.is_some() {
                        return hit;
                    }
                }
                None
            }
            hir::Expr::IfExpr(arms) => {
                for (cond, val) in arms {
                    if let Some(c) = cond {
                        if let Some(d) = self.expr(c, root) {
                            return Some(d);
                        }
                    }
                    if let Some(d) = self.expr(val, root) {
                        return Some(d);
                    }
                }
                None
            }
            hir::Expr::Composite(parts) => {
                for p in parts {
                    if let Some(d) = self.expr(p, root) {
                        return Some(d);
                    }
                }
                None
            }
            hir::Expr::Literal { .. } | hir::Expr::Leaf => None,
        }
    }

    fn offset(&self) -> usize {
        self.f.offset
    }

    /// A member access `base.field` / `base:field`, where `base` is a name
    /// or a chain of `.` accesses (`X`, `X.Limits`, ...). Collect the dotted
    /// path from its root name, then resolve the whole path through the
    /// module the root names (require binding or own export).
    fn member_on(&self, base: &hir::Expr, field: &str, _root: bool) -> Option<Definition> {
        let mut segs = vec![field.to_string()];
        let mut cur = base;
        let root = loop {
            match cur {
                hir::Expr::Index { base, key: None, name: Some(n), .. } => {
                    segs.push(n.clone());
                    cur = base;
                }
                hir::Expr::Name { name, .. } => break name,
                // A bracket index, a call, or anything else in the chain: not
                // a static dotted path, so no jump.
                _ => return None,
            }
        };
        segs.reverse();
        let base_is_root = self.resolve_name(root).map(|(_, r)| r).unwrap_or(false);
        self.f.member_path(root, &segs, base_is_root)
    }
}

/// Byte end of a base expression, for the method-name heuristic.
fn base_end(e: &hir::Expr) -> usize {
    match e {
        hir::Expr::Name { span, .. } => span.end,
        hir::Expr::Index { base, name_span, .. } => {
            name_span.as_ref().map(|s| s.end).unwrap_or_else(|| base_end(base))
        }
        hir::Expr::Call { span, .. } | hir::Expr::UnOp { span, .. } => span.end,
        _ => 0,
    }
}
