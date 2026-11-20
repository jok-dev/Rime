//! R0802 player-map-never-cleared / R0803 weak-player-map-never-cleared: a
//! module-lifetime table keyed by Player that gains entries and is never
//! emptied by the module that owns it. R0802 = strong keys: the table holds
//! the Player object itself (the keystone shape — one such table anywhere
//! keeps every weak-keyed cache entry for that Player alive). R0803 =
//! `__mode = "k"`: the table holds its VALUES while someone else holds the
//! Player. Split 2026-09-04 (user) so either can be turned off alone; same
//! walk, same teardown set, the key mode decides the code at report time.
//!
//! Why (2026-09-04): a server whose instance count only ever grows. One
//! Player object retained after the player leaves keeps every per-player
//! cache entry keyed `{[Player]: X}` alive with it — INCLUDING the ones
//! with `__mode = "k"` — and each entry pins whatever it holds (a whole
//! instance tree, in the shape that prompted the rule). Weak keys only
//! release an entry once the key is unreachable from everywhere; they are
//! not a teardown.
//!
//! Candidate: a root-body `local T = {}` / `setmetatable({}, mt)`, or a
//! root-body `Module.field = {}` on a root local (module lifetime either
//! way). `self.x`, per-object tables and function locals are not.
//!
//! Player-keyed write — `T[k] = v` anywhere in the module, where any of:
//!   - T's annotation reads `{[Player]: …}` (`Ty.rendered`, a text match:
//!     there is no type system yet)
//!   - `k` is a parameter or local annotated `Player` / `Player?`
//!   - `k` is a variable whose name is on the project's list of names that
//!     mean "a Player" (`Options::player_names`; rime.toml's `[names]
//!     player`, default `player` and `plr`), unless it is annotated
//!     `string`, `number` or `boolean` (`player: string` is a user name)
//!
//! The names are there because code that annotates neither the table nor
//! the key is common, and the annotations alone would miss it. Which names
//! a project uses is its own convention, so the list is a setting.
//!
//! Teardown — any of these anywhere in the module silences the table:
//!   - `T[x] = nil`, or `T[x] = p` where `p` is a parameter or a local
//!     whose type admits nil (the leave path passing `nil` through a
//!     helper: `setSession(player, reason, nil)`)
//!   - T reassigned, passed to any call other than a read-only builtin
//!     (`table.clear`, a janitor, a utility — something else owns it), or
//!     used as a value (returned, stored, aliased)
//!   - `__mode` containing `v`: weak values do release the entry
//!
//! A cache fill `T[player] = folder` from a call result inside the same
//! function is a strong write.
//!
//! One diagnostic per table at its declaration (a structural one-site
//! defect — the CLAUDE.md per-occurrence rule's exemption), warning,
//! conservative. Notes carry the Player-keyed write count and whether a
//! `PlayerRemoving` handler exists, reaches the table (same-module call
//! graph from the handler, nested closures included) or never touches it.
//!
//! Why a module-level walk: whether a write hits the ROOT table, and
//! whether any body anywhere tears it down, are cross-body questions —
//! same reason as `rules::globals`, whose scope-stack walk this mirrors.

use rime_diag::{Code, Diagnostic, Label};
use rime_hir::{self as hir, BodyId, Span};
use std::collections::{HashMap, HashSet, VecDeque};

/// What a project tells these rules (rime.toml's `[names]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Variable names read as "this holds a Player" when used as a table
    /// key. `*` in a name matches any run of characters (`*Player`); the
    /// match is case-sensitive. Empty: only `Player` annotations count.
    pub player_names: Vec<String>,
}

/// The names with no rime.toml.
pub const DEFAULT_PLAYER_NAMES: &[&str] = &["player", "plr"];

impl Default for Options {
    fn default() -> Self {
        Options { player_names: DEFAULT_PLAYER_NAMES.iter().map(|n| n.to_string()).collect() }
    }
}

pub fn check_module(module: &hir::Module, src: &str, options: &Options, diags: &mut Vec<Diagnostic>) {
    let root = module.body(module.root);
    let mut w = Walker::new(module, options);
    w.collect_candidates(&root.block);
    if w.cands.is_empty() {
        return;
    }
    w.body_stack.push(module.root);
    w.scopes.push(HashMap::new());
    w.walk_stmts(&root.block);
    w.scopes.pop();
    w.body_stack.pop();
    w.report(src, diags);
}

/// Builtins whose table argument is only read — passing a candidate to
/// one is not an escape. Everything else (including `table.clear`) hands
/// the table to code this rule cannot see.
const READ_ONLY_BUILTINS: &[&str] = &[
    "pairs", "ipairs", "next", "rawget", "rawlen", "type", "typeof", "tostring", "print",
    "warn", "assert", "error", "select", "unpack",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Weak {
    No,
    Keys,
    Values,
    /// `setmetatable({}, SOMETHING)` — a non-literal metatable. Treated
    /// like strong keys: weak keys would not exempt it anyway.
    Unknown,
}

struct Candidate {
    display: String,
    decl_span: Span,
    /// The declaring statement: an assignment to `M.f` with this span is
    /// the declaration, any other is a reassignment (teardown).
    decl_stmt: Span,
    ty_player_keyed: bool,
    weak: Weak,
    /// Strong writes under a Player key, in source order.
    player_writes: Vec<Span>,
    torn_down: bool,
    /// Bodies that mention the table at all — for the handler note.
    touched_in: HashSet<BodyId>,
}

#[derive(Clone, Copy)]
struct NameInfo {
    nilable: bool,
    is_player: bool,
    /// Annotated `string`, `number` or `boolean`: whatever the variable is
    /// called, it is not a Player.
    never_player: bool,
}

const PLAIN: NameInfo = NameInfo { nilable: false, is_player: false, never_player: false };

enum HandlerRef {
    Body(BodyId),
    Named(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    /// Operand position: reading through the table does not let it escape.
    Read,
    /// Value position: stored, returned, passed — the table escapes.
    Value,
}

struct Walker<'m> {
    module: &'m hir::Module,
    options: &'m Options,
    cands: Vec<Candidate>,
    index: HashMap<(String, Option<String>), usize>,
    scopes: Vec<HashMap<String, NameInfo>>,
    body_stack: Vec<BodyId>,
    /// Module-scope callees named by each body (`f`, `M.f`).
    calls: HashMap<BodyId, Vec<String>>,
    /// Closures created inside each body.
    children: HashMap<BodyId, Vec<BodyId>>,
    /// Module-scope function names to their bodies.
    fn_bodies: HashMap<String, BodyId>,
    handlers: Vec<(HandlerRef, Span)>,
}

impl<'m> Walker<'m> {
    fn new(module: &'m hir::Module, options: &'m Options) -> Self {
        Walker {
            module,
            options,
            cands: Vec::new(),
            index: HashMap::new(),
            scopes: Vec::new(),
            body_stack: Vec::new(),
            calls: HashMap::new(),
            children: HashMap::new(),
            fn_bodies: HashMap::new(),
            handlers: Vec::new(),
        }
    }

    // --- candidates ---------------------------------------------------------

    fn collect_candidates(&mut self, block: &hir::Block) {
        let mut root_locals: HashSet<String> = HashSet::new();
        for stmt in &block.stmts {
            match &stmt.kind {
                hir::StmtKind::Local { decls, init } => {
                    for (i, d) in decls.iter().enumerate() {
                        root_locals.insert(d.name.clone());
                        let Some(weak) = init.get(i).and_then(table_init) else {
                            continue;
                        };
                        let keyed = d.ty.as_ref().is_some_and(|t| player_keyed_ty(&t.rendered));
                        self.add(d.name.clone(), None, d.span.clone(), stmt.span.clone(), keyed, weak);
                    }
                }
                hir::StmtKind::LocalFunction { decl, .. } => {
                    root_locals.insert(decl.name.clone());
                }
                hir::StmtKind::Assign { targets, values } => {
                    for (i, t) in targets.iter().enumerate() {
                        let hir::Target::Projection(hir::Expr::Index {
                            base,
                            name: Some(f),
                            key: None,
                            name_span,
                        }) = t
                        else {
                            continue;
                        };
                        let hir::Expr::Name { name: m, .. } = &**base else {
                            continue;
                        };
                        if !root_locals.contains(m) {
                            continue;
                        }
                        let Some(weak) = values.get(i).and_then(table_init) else {
                            continue;
                        };
                        let span = name_span.clone().unwrap_or_else(|| stmt.span.clone());
                        self.add(m.clone(), Some(f.clone()), span, stmt.span.clone(), false, weak);
                    }
                }
                _ => {}
            }
        }
    }

    fn add(
        &mut self,
        root: String,
        field: Option<String>,
        decl_span: Span,
        decl_stmt: Span,
        ty_player_keyed: bool,
        weak: Weak,
    ) {
        let key = (root.clone(), field.clone());
        if let Some(&i) = self.index.get(&key) {
            // Declared twice at the root: the second is a reassignment.
            self.cands[i].torn_down = true;
            return;
        }
        let display = match &field {
            Some(f) => format!("{root}.{f}"),
            None => root.clone(),
        };
        self.index.insert(key, self.cands.len());
        self.cands.push(Candidate {
            display,
            decl_span,
            decl_stmt,
            ty_player_keyed,
            weak,
            player_writes: Vec::new(),
            torn_down: false,
            touched_in: HashSet::new(),
        });
    }

    // --- scope ----------------------------------------------------------------

    fn declare(&mut self, name: &str, info: NameInfo) {
        self.scopes
            .last_mut()
            .expect("a scope is open")
            .insert(name.to_string(), info);
    }

    /// Innermost binding of `name`: its scope depth (0 = module root) and
    /// what its annotation says. None for globals.
    fn resolve(&self, name: &str) -> Option<(usize, NameInfo)> {
        self.scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(depth, s)| s.get(name).map(|info| (depth, *info)))
    }

    fn cand_of_name(&self, name: &str) -> Option<usize> {
        match self.resolve(name) {
            Some((0, _)) => self.index.get(&(name.to_string(), None)).copied(),
            _ => None,
        }
    }

    /// The candidate an expression names: a root local `T`, or `M.f` on a
    /// root local `M`. Shadowed names are not the table.
    fn cand_of(&self, e: &hir::Expr) -> Option<usize> {
        match e {
            hir::Expr::Name { name, .. } => self.cand_of_name(name),
            hir::Expr::Index { base, name: Some(f), key: None, .. } => {
                let hir::Expr::Name { name: m, .. } = &**base else {
                    return None;
                };
                if !matches!(self.resolve(m), Some((0, _))) {
                    return None;
                }
                self.index.get(&(m.clone(), Some(f.clone()))).copied()
            }
            _ => None,
        }
    }

    fn current_body(&self) -> BodyId {
        *self.body_stack.last().expect("a body is open")
    }

    fn touch(&mut self, i: usize) {
        let body = self.current_body();
        self.cands[i].touched_in.insert(body);
    }

    // --- the walk -------------------------------------------------------------

    fn walk_block(&mut self, block: &hir::Block) {
        self.scopes.push(HashMap::new());
        self.walk_stmts(block);
        self.scopes.pop();
    }

    fn walk_stmts(&mut self, block: &hir::Block) {
        for stmt in &block.stmts {
            self.walk_stmt(stmt);
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            for e in exprs {
                self.expr(e, Ctx::Value);
            }
        }
    }

    fn closure(&mut self, id: BodyId, is_method: bool) {
        let parent = self.current_body();
        self.children.entry(parent).or_default().push(id);
        let body = self.module.body(id);
        let mut scope: HashMap<String, NameInfo> = HashMap::new();
        for p in &body.params {
            // An unannotated parameter may be handed nil by any caller.
            let nilable = p.ty.as_ref().is_none_or(|t| t.admits_nil);
            let is_player = p.ty.as_ref().is_some_and(|t| is_player_ty(&t.rendered));
            let never_player = p.ty.as_ref().is_some_and(|t| is_scalar_ty(&t.rendered));
            scope.insert(p.name.clone(), NameInfo { nilable, is_player, never_player });
        }
        if is_method {
            scope.insert("self".to_string(), PLAIN);
        }
        self.scopes.push(scope);
        self.body_stack.push(id);
        self.walk_block(&body.block);
        self.body_stack.pop();
        self.scopes.pop();
    }

    fn walk_stmt(&mut self, stmt: &hir::Stmt) {
        use hir::StmtKind as K;
        match &stmt.kind {
            K::Local { decls, init } => {
                for e in init {
                    self.expr(e, Ctx::Value);
                }
                for (i, d) in decls.iter().enumerate() {
                    if let Some(hir::Expr::Function(id)) = init.get(i) {
                        self.fn_bodies.entry(d.name.clone()).or_insert(*id);
                    }
                    let nilable = d.ty.as_ref().is_some_and(|t| t.admits_nil);
                    let is_player = d.ty.as_ref().is_some_and(|t| is_player_ty(&t.rendered));
                    let never_player = d.ty.as_ref().is_some_and(|t| is_scalar_ty(&t.rendered));
                    self.declare(&d.name, NameInfo { nilable, is_player, never_player });
                }
            }
            K::Assign { targets, values } => {
                for e in values {
                    self.expr(e, Ctx::Value);
                }
                for (i, t) in targets.iter().enumerate() {
                    self.target(t, values.get(i), false, stmt);
                }
            }
            K::CompoundAssign { target, value } => {
                self.expr(value, Ctx::Read);
                // `T[k] += v` never stores nil: a strong write.
                self.target(target, None, true, stmt);
            }
            K::Call(e) => self.expr(e, Ctx::Read),
            K::Do(b) => self.walk_block(b),
            K::If { cond, then, else_ifs, or_else } => {
                self.expr(cond, Ctx::Read);
                self.walk_block(then);
                for (c, b) in else_ifs {
                    self.expr(c, Ctx::Read);
                    self.walk_block(b);
                }
                if let Some(b) = or_else {
                    self.walk_block(b);
                }
            }
            K::While { cond, body } => {
                self.expr(cond, Ctx::Read);
                self.walk_block(body);
            }
            K::Repeat { body, until } => {
                // `until` sees the body's locals (see rules::globals).
                self.scopes.push(HashMap::new());
                self.walk_stmts(body);
                self.expr(until, Ctx::Read);
                self.scopes.pop();
            }
            K::NumericFor { var, start, end, step, body } => {
                self.expr(start, Ctx::Read);
                self.expr(end, Ctx::Read);
                if let Some(s) = step {
                    self.expr(s, Ctx::Read);
                }
                self.scopes.push(HashMap::new());
                self.declare(&var.name, PLAIN);
                self.walk_block(body);
                self.scopes.pop();
            }
            K::GenericFor { vars, exprs, body } => {
                for e in exprs {
                    self.expr(e, Ctx::Read);
                }
                self.scopes.push(HashMap::new());
                for v in vars {
                    self.declare(&v.name, PLAIN);
                }
                self.walk_block(body);
                self.scopes.pop();
            }
            K::LocalFunction { decl, body } => {
                self.declare(&decl.name, PLAIN);
                self.fn_bodies.entry(decl.name.clone()).or_insert(*body);
                self.closure(*body, false);
            }
            K::FunctionDecl { root, path, is_method, assigns_root, body, .. } => {
                if *assigns_root {
                    // `function T() end` rebinds `T`: a reassignment if it
                    // was one of the tables.
                    if let Some(i) = self.cand_of_name(root) {
                        self.touch(i);
                        self.cands[i].torn_down = true;
                    }
                    self.fn_bodies.entry(root.clone()).or_insert(*body);
                } else if path.len() == 1 {
                    self.fn_bodies.entry(format!("{root}.{}", path[0])).or_insert(*body);
                }
                self.closure(*body, *is_method);
            }
        }
    }

    /// An assignment target. `value` is the expression stored (None when
    /// the value list ran short — that stores nil); `strong` marks a
    /// compound assignment, which never stores nil.
    fn target(&mut self, t: &hir::Target, value: Option<&hir::Expr>, strong: bool, stmt: &hir::Stmt) {
        match t {
            hir::Target::Name { name, .. } => {
                if let Some(i) = self.cand_of_name(name) {
                    self.touch(i);
                    self.cands[i].torn_down = true;
                }
            }
            hir::Target::Projection(e) => {
                if let Some(i) = self.cand_of(e) {
                    // `M.f = …`: the declaration itself, or a reassignment.
                    self.touch(i);
                    if self.cands[i].decl_stmt != stmt.span {
                        self.cands[i].torn_down = true;
                    }
                    return;
                }
                if let hir::Expr::Index { base, key: Some(k), .. } = e {
                    if let Some(i) = self.cand_of(base) {
                        self.touch(i);
                        let player_key = self.cands[i].ty_player_keyed || self.is_player_key(k);
                        if !strong && self.is_nilable_value(value) {
                            self.cands[i].torn_down = true;
                        } else if player_key {
                            self.cands[i].player_writes.push(stmt.span.clone());
                        }
                        self.expr(k, Ctx::Read);
                        return;
                    }
                }
                self.expr(e, Ctx::Read);
            }
        }
    }

    fn is_player_key(&self, k: &hir::Expr) -> bool {
        let hir::Expr::Name { name, .. } = k else {
            return false;
        };
        // The annotation outranks the name: `player: string` is a user
        // name, `who: Player` is a Player.
        match self.resolve(name) {
            Some((_, info)) if info.is_player => return true,
            Some((_, info)) if info.never_player => return false,
            _ => {}
        }
        self.options.player_names.iter().any(|pattern| name_matches(pattern, name))
    }

    /// Can this stored value be nil — i.e. is the write a possible removal?
    /// `nil`, a value list that ran short, a parameter (unannotated or
    /// `T?`), a `T?` local, `x or <nilable>` (`flag or nil` stores nothing
    /// for false), or an if-expression with a nilable arm.
    fn is_nilable_value(&self, v: Option<&hir::Expr>) -> bool {
        match v {
            None => true,
            Some(hir::Expr::Literal { prim: hir::Prim::Nil, .. }) => true,
            Some(hir::Expr::Name { name, .. }) => {
                matches!(self.resolve(name), Some((_, info)) if info.nilable)
            }
            Some(hir::Expr::BinOp { op: hir::BinOp::Or, rhs, .. }) => {
                self.is_nilable_value(Some(rhs))
            }
            Some(hir::Expr::IfExpr(arms)) => {
                arms.iter().any(|(_, val)| self.is_nilable_value(Some(val)))
            }
            _ => false,
        }
    }

    fn expr(&mut self, e: &hir::Expr, ctx: Ctx) {
        if let Some(i) = self.cand_of(e) {
            self.touch(i);
            if ctx == Ctx::Value {
                self.cands[i].torn_down = true;
            }
            return;
        }
        use hir::Expr as E;
        match e {
            E::Name { .. } | E::Literal { .. } | E::Leaf => {}
            E::Function(id) => self.closure(*id, false),
            E::Call { base, args, method, span, .. } => {
                self.record_call(base, method.as_deref());
                self.handler(base, method.as_deref(), args, span);
                self.expr(base, Ctx::Read);
                let read_only = method.is_none()
                    && matches!(&**base, E::Name { name, .. } if READ_ONLY_BUILTINS.contains(&name.as_str()));
                let arg_ctx = if read_only { Ctx::Read } else { Ctx::Value };
                for a in args {
                    self.expr(a, arg_ctx);
                }
            }
            E::Index { base, key, .. } => {
                self.expr(base, Ctx::Read);
                if let Some(k) = key {
                    self.expr(k, Ctx::Read);
                }
            }
            E::BinOp { lhs, rhs, .. } => {
                self.expr(lhs, Ctx::Read);
                self.expr(rhs, Ctx::Read);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => self.expr(inner, Ctx::Read),
            E::Table(fields) => {
                for f in fields {
                    match f {
                        hir::TableField::Keyed { key, value } => {
                            self.expr(key, Ctx::Read);
                            self.expr(value, Ctx::Value);
                        }
                        hir::TableField::Named { value, .. } => self.expr(value, Ctx::Value),
                        hir::TableField::Item(e) => self.expr(e, Ctx::Value),
                    }
                }
            }
            E::IfExpr(arms) => {
                for (cond, val) in arms {
                    if let Some(c) = cond {
                        self.expr(c, Ctx::Read);
                    }
                    self.expr(val, Ctx::Value);
                }
            }
            E::Composite(parts) => {
                for p in parts {
                    self.expr(p, Ctx::Value);
                }
            }
        }
    }

    /// Same-module call edge: `f(...)`, `M.f(...)`, `M:f(...)`.
    fn record_call(&mut self, base: &hir::Expr, method: Option<&str>) {
        let callee = match (base, method) {
            (hir::Expr::Name { name, .. }, None) => name.clone(),
            (hir::Expr::Name { name, .. }, Some(m)) => format!("{name}.{m}"),
            (hir::Expr::Index { base: b, name: Some(f), key: None, .. }, None) => {
                let hir::Expr::Name { name: m, .. } = &**b else {
                    return;
                };
                format!("{m}.{f}")
            }
            _ => return,
        };
        let body = self.current_body();
        self.calls.entry(body).or_default().push(callee);
    }

    /// `<anything>.PlayerRemoving:Connect(f)` / `:Once(f)`.
    fn handler(&mut self, base: &hir::Expr, method: Option<&str>, args: &[hir::Expr], span: &Span) {
        if !matches!(method, Some("Connect") | Some("Once")) {
            return;
        }
        let hir::Expr::Index { name: Some(n), key: None, .. } = base else {
            return;
        };
        if n != "PlayerRemoving" {
            return;
        }
        match args.first() {
            Some(hir::Expr::Function(id)) => self.handlers.push((HandlerRef::Body(*id), span.clone())),
            Some(hir::Expr::Name { name, .. }) => {
                self.handlers.push((HandlerRef::Named(name.clone()), span.clone()))
            }
            _ => {}
        }
    }

    // --- report -------------------------------------------------------------------

    fn reachable(&self, from: impl Iterator<Item = BodyId>) -> HashSet<BodyId> {
        let mut seen = HashSet::new();
        let mut queue: VecDeque<BodyId> = from.collect();
        while let Some(b) = queue.pop_front() {
            if !seen.insert(b) {
                continue;
            }
            if let Some(cs) = self.children.get(&b) {
                queue.extend(cs.iter().copied());
            }
            if let Some(callees) = self.calls.get(&b) {
                for c in callees {
                    if let Some(&id) = self.fn_bodies.get(c) {
                        queue.push_back(id);
                    }
                }
            }
        }
        seen
    }

    fn report(&self, src: &str, diags: &mut Vec<Diagnostic>) {
        let handlers: Vec<(BodyId, Span)> = self
            .handlers
            .iter()
            .filter_map(|(h, span)| {
                let id = match h {
                    HandlerRef::Body(id) => *id,
                    HandlerRef::Named(n) => *self.fn_bodies.get(n)?,
                };
                Some((id, span.clone()))
            })
            .collect();
        let reach = self.reachable(handlers.iter().map(|(id, _)| *id));

        for c in &self.cands {
            if c.weak == Weak::Values || c.torn_down || c.player_writes.is_empty() {
                continue;
            }
            let first = &c.player_writes[0];
            let count = match c.player_writes.len() {
                1 => format!("once, at line {}", line_of(src, first.start)),
                n => format!("{n} times (first at line {})", line_of(src, first.start)),
            };
            let handler_note = match handlers.first() {
                None => "no `PlayerRemoving` handler in this module".to_string(),
                Some((_, span)) => {
                    let line = line_of(src, span.start);
                    if c.touched_in.iter().any(|b| reach.contains(b)) {
                        format!(
                            "the `PlayerRemoving` handler at line {line} reaches this table \
                             but never clears it"
                        )
                    } else {
                        format!("the `PlayerRemoving` handler at line {line} never touches this table")
                    }
                }
            };
            // Strong keys hold the Player itself (R0802); weak keys hold
            // only the values, and only while someone else holds the
            // Player (R0803). Split so either can be turned off alone.
            let (code, message, label, why) = match c.weak {
                Weak::Keys => (
                    Code(803),
                    format!(
                        "`{}` is weak-keyed but holds its values for every Player ever \
                         added, and is never cleared in this module",
                        c.display
                    ),
                    "weak keys: each entry keeps its value alive while the Player is \
                     reachable anywhere",
                    "`__mode = \"k\"` is not a teardown: the table does not hold the \
                     Player, but an entry lives as long as its Player object is reachable \
                     from anywhere — and a Player retained elsewhere keeps every value \
                     here alive with it",
                ),
                Weak::Unknown => (
                    Code(802),
                    format!(
                        "`{}` holds every Player ever added and is never cleared in this \
                         module",
                        c.display
                    ),
                    "keys assumed strong: the metatable is not a literal, so its weak \
                     mode is unknown",
                    "a retained dead Player also keeps every weak-keyed cache entry for \
                     it alive — clearing this table on PlayerRemoving releases those too",
                ),
                Weak::No | Weak::Values => (
                    Code(802),
                    format!(
                        "`{}` holds every Player ever added and is never cleared in this \
                         module",
                        c.display
                    ),
                    "strong keys: each entry keeps its Player object alive",
                    "a retained dead Player also keeps every weak-keyed cache entry for \
                     it alive — clearing this table on PlayerRemoving releases those too",
                ),
            };
            diags.push(
                Diagnostic::warning(code, message)
                    .with_label(Label::primary(c.decl_span.clone(), label))
                    .with_label(Label::secondary(first.clone(), "first entry added here"))
                    .with_note(format!(
                        "written with a Player key {count}; nothing in this module removes \
                         an entry — no `nil` write, no `table.clear`, no reassignment, and \
                         the table never leaves the module"
                    ))
                    .with_note(handler_note)
                    .with_note(why)
                    .conservative(
                        "valid Luau — deliberate only if entries are meant to live for the \
                         server's lifetime, or a consumer module clears them",
                    ),
            );
        }
    }
}

// --- helpers ----------------------------------------------------------------------

/// `{}` or `setmetatable({}, mt)` — a fresh table, with its weak mode.
fn table_init(e: &hir::Expr) -> Option<Weak> {
    match e {
        hir::Expr::Table(_) => Some(Weak::No),
        hir::Expr::Call { base, args, method: None, .. } => {
            let hir::Expr::Name { name, .. } = &**base else {
                return None;
            };
            if name != "setmetatable" || !matches!(args.first(), Some(hir::Expr::Table(_))) {
                return None;
            }
            Some(match args.get(1) {
                Some(hir::Expr::Table(fields)) => weak_mode(fields),
                _ => Weak::Unknown,
            })
        }
        _ => None,
    }
}

fn weak_mode(fields: &[hir::TableField]) -> Weak {
    for f in fields {
        let hir::TableField::Named { name, value, .. } = f else {
            continue;
        };
        if name != "__mode" {
            continue;
        }
        return match value {
            hir::Expr::Literal { prim: hir::Prim::String, text: Some(t), .. } => {
                if t.contains('v') {
                    Weak::Values
                } else if t.contains('k') {
                    Weak::Keys
                } else {
                    Weak::No
                }
            }
            _ => Weak::Unknown,
        };
    }
    Weak::No
}

/// `{[Player]: …}` — the annotation's text, whitespace ignored.
fn player_keyed_ty(rendered: &str) -> bool {
    let compact: String = rendered.chars().filter(|c| !c.is_whitespace()).collect();
    compact.starts_with("{[Player]:")
}

/// `Player` / `Player?`.
fn is_player_ty(rendered: &str) -> bool {
    rendered.trim().trim_end_matches('?').trim() == "Player"
}

/// `string`, `number`, `boolean`, each also with `?`: a value of one of
/// these is never a Player. Any other annotation may be an alias of one.
fn is_scalar_ty(rendered: &str) -> bool {
    matches!(rendered.trim().trim_end_matches('?').trim(), "string" | "number" | "boolean")
}

/// Whether `name` is `pattern`, where `*` in the pattern stands for any run
/// of characters (none included). Case-sensitive.
fn name_matches(pattern: &str, name: &str) -> bool {
    let Some((head, rest)) = pattern.split_once('*') else {
        return pattern == name;
    };
    let Some(mut left) = name.strip_prefix(head) else {
        return false;
    };
    // Every middle piece is found leftmost; the last has to end the name.
    let mut pieces: Vec<&str> = rest.split('*').collect();
    let tail = pieces.pop().unwrap_or("");
    for piece in pieces {
        match left.find(piece) {
            Some(at) => left = &left[at + piece.len()..],
            None => return false,
        }
    }
    left.ends_with(tail)
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].bytes().filter(|&b| b == b'\n').count() + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables reported, in source order, under these player names.
    fn reported(src: &str, names: &[&str]) -> Vec<String> {
        let module = rime_syntax::parse(src).unwrap();
        let options = Options { player_names: names.iter().map(|n| n.to_string()).collect() };
        let mut diags = Vec::new();
        check_module(&module, src, &options, &mut diags);
        diags
            .iter()
            .map(|d| d.message.split('`').nth(1).unwrap().to_string())
            .collect()
    }

    const BY_NAME: &str = "\
local byPlayer = {}
local byPlr = {}
local byClient = {}
local byOther = {}
local function add(player, plr, client, otherPlayer)
\tbyPlayer[player] = true
\tbyPlr[plr] = true
\tbyClient[client] = true
\tbyOther[otherPlayer] = true
end
return add
";

    #[test]
    fn the_default_names_are_player_and_plr() {
        let names: Vec<&str> = DEFAULT_PLAYER_NAMES.to_vec();
        assert_eq!(names, ["player", "plr"]);
        assert_eq!(Options::default().player_names, names);
        assert_eq!(reported(BY_NAME, &names), ["byPlayer", "byPlr"]);
    }

    #[test]
    fn the_project_list_replaces_the_default() {
        assert_eq!(reported(BY_NAME, &["client"]), ["byClient"]);
        assert_eq!(reported(BY_NAME, &["client", "*Player"]), ["byClient", "byOther"]);
        assert_eq!(reported(BY_NAME, &["player", "plr", "client", "*Player"]).len(), 4);
    }

    #[test]
    fn with_no_names_only_annotations_count() {
        let src = "\
local byName = {}
local byAnnotation = {}
local byTable: { [Player]: boolean } = {}
local function add(player, who: Player, other)
\tbyName[player] = true
\tbyAnnotation[who] = true
\tbyTable[other] = true
end
return add
";
        assert_eq!(reported(src, &[]), ["byAnnotation", "byTable"]);
    }

    #[test]
    fn a_name_annotated_as_a_scalar_is_not_a_player() {
        // `player` here is a user name. The annotation says so, and it
        // outranks the variable's name.
        let src = "\
local seenName = {}
local seenId = {}
local seenFlag = {}
local seenLocal = {}
local unannotated = {}
local function add(player: string, plr: number?, client: boolean, other)
\tseenName[player] = true
\tseenId[plr] = true
\tseenFlag[client] = true
\tlocal player: string = other.Name
\tseenLocal[player] = true
end
local function addPlayer(player)
\tunannotated[player] = true
end
return { add, addPlayer }
";
        assert_eq!(reported(src, &["player", "plr", "client"]), ["unannotated"]);
    }

    #[test]
    fn any_other_annotation_leaves_the_name_test_alone() {
        // An alias may well be a Player; only a scalar cannot be one.
        let src = "\
local byAlias = {}
local byAny = {}
local function add(player: Types.Member, plr: any)
\tbyAlias[player] = true
\tbyAny[plr] = true
end
return add
";
        assert_eq!(reported(src, &["player", "plr"]), ["byAlias", "byAny"]);
    }

    #[test]
    fn names_match_whole_and_case_sensitive() {
        assert!(name_matches("player", "player"));
        assert!(!name_matches("player", "Player"));
        assert!(!name_matches("player", "players"));
        assert!(!name_matches("player", "otherPlayer"));
    }

    #[test]
    fn a_star_is_any_run_of_characters() {
        assert!(name_matches("*Player", "otherPlayer"));
        assert!(name_matches("*Player", "Player"), "the run may be empty");
        assert!(!name_matches("*Player", "otherPlayers"));
        assert!(name_matches("plr*", "plr2"));
        assert!(name_matches("*lay*", "player"));
        assert!(name_matches("p*a*r", "player"));
        assert!(!name_matches("p*a*r", "plr"));
        assert!(name_matches("*", "anything"));
        assert!(!name_matches("a*a", "a"), "the two ends may not overlap");
    }
}
