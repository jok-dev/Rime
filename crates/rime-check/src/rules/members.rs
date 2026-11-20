//! Module member existence.
//!
//! `Mod.fn()` and `Mod.field` on a require-bound module are checked to EXIST
//! on the module's interface — when the interface is CLOSED: the extractor
//! saw the whole export shape. Closed means:
//!
//!   - every member is statically named: table-literal entries, top-level
//!     `M.x = v` / `function M.f()` / `function M:m()`, and statically-named
//!     writes in nested bodies (over-approximation: a write that may never
//!     run still names a member that COULD exist — membership checking
//!     wants the superset)
//!   - the table never escapes: no bare `M` as a call argument, alias,
//!     table value, or nested return — an escaped alias can add members the
//!     extractor cannot see. `setmetatable(M, ...)` opens the interface the
//!     same way (an `__index` makes membership unknowable).
//!   - the same holds for `self` inside the export's own functions, where
//!     `self` IS the export (`receiver_scan`): `self.x = v` names a member
//!     (`ExportShape::receiver_fields`), and `self[k] = v` or a bare `self`
//!     that is passed on, aliased, stored or returned opens the interface.
//!   - the export's own declared type does not say otherwise: an export
//!     typed with a string indexer (`local M: { [string]: T }`, the same
//!     behind a type alias of the file, or given by a cast on the
//!     initializer or on the `return`) is declared to hold keys the file
//!     never names. The type is the author saying "more members than you
//!     can see here", so the interface is open
//!     (`hir::Module::unknown_members`). `any` is not that statement: an
//!     export typed `any` stays closed.
//!
//! `table.freeze` is NOT required (decided 2026-08-22): required modules
//! are treated as SEALED by convention — a module's members are exactly
//! what its own file defines. A large codebase never uses freeze,
//! so a freeze gate would blank the rule there; the sealed-module
//! axiom is a design decision, and consumer-side population does not
//! reopen an interface — it is flagged instead (R0308 below). The member
//! diagnostics carry `conservative: true` for exactly this reason: a
//! runtime-populated member CAN exist when the rule says it does not; the
//! rejection is Rime holding the sealed-module line, not a proof the read
//! crashes. (Large-codebase survey 2026-08-22: the majority of hits were
//! real crash paths.)
//!
//! Anything else — dynamic keys, loop assignment, merge helpers, a non-table
//! initializer — leaves the interface OPEN and membership unjudged. Silence
//! over guessing.
//!
//! Shadowing note: the escape scan ignores scope — a nested local that
//! shadows the exported name makes its occurrences look like the export's.
//! That direction is safe: a false "escape" only opens the interface and
//! silences the rule.
//!
//! Three codes: R0306 (unknown member called — emitted from the call
//! checker's interface-miss path in calls.rs, and by `check_own_calls` here
//! for a call on the file's OWN export), R0307 (unknown field read —
//! the walker here), R0308 (consumer write to a required module — the
//! sealed-module rule itself; fires on one-level writes whatever the
//! closedness, static or dynamic key; deep writes `Mod.a.b = v` are v1
//! out-of-scope). This is shape MEMBERSHIP on an extracted interface, not
//! type inference; return-value misuse needs type inference and is not done here.

use rime_diag::{Code, Diagnostic, Label, Span};
use rime_hir as hir;
use std::collections::{HashMap, HashSet};

use super::calls::ModuleInterface;

/// What the export scan learned about a module's shape, plus the
/// self-read facts R0309 (unused export) consumes.
pub(crate) struct ExportShape {
    /// Statically-named members, keyed to their declaration span (first
    /// declaration wins — the span a diagnostic in the defining file
    /// anchors on).
    pub(crate) fields: HashMap<String, Span>,
    /// The extractor saw the whole export shape; see the module doc.
    pub(crate) closed: bool,
    /// The file declares the exported name itself, with a top-level
    /// `local`. False for a name the file only returns (a global, or a
    /// local of an inner block): the shape then describes a table this
    /// file did not build.
    pub(crate) declared: bool,
    /// Member names the module reads off its own export (`M.x`, `M:m()`,
    /// the inner read of a deep write `M.a.b = v`) — internal use counts
    /// as use for R0309. The same reads through `self` in the export's
    /// own functions count (`self.x`, `self:m()`).
    pub(crate) self_reads: HashSet<String>,
    /// The module indexes its own export with a computed key (`M[k]`, or
    /// `self[k]` in its own functions) —
    /// any member may be self-read, so R0309 stays silent on the module.
    /// Does NOT open the interface: membership itself is still exact.
    pub(crate) dynamic_self_read: bool,
    /// Members named only through the receiver of the export's own
    /// functions (`function M:init() self.helper = ... end`), see
    /// `receiver_scan`. They exist for membership checking (R0306, R0307).
    /// They are kept apart from `fields` because R0309 reports from
    /// `fields`, and the receiver may be some other table
    /// (`M.init(other)`): more names here cost a membership report, more
    /// names there would add an unused-export report.
    pub(crate) receiver_fields: HashSet<String>,
}

/// Fields + closedness of a module's export. `exported` is the
/// local the module returns (bare or `table.freeze`-wrapped — freeze does
/// not matter to closedness, see the module doc). Called by
/// `calls::module_interface`.
pub(crate) fn export_shape(module: &hir::Module, exported: &str) -> ExportShape {
    let mut scan = ExportScan {
        module,
        exported,
        fields: HashMap::new(),
        open: false,
        saw_decl: false,
        self_reads: HashSet::new(),
        dynamic_self_read: false,
    };

    let root = module.body(module.root);
    // `return M :: T` where `T` has unknown members.
    if let Some(hir::Terminator::Return { span, .. }) = &root.block.terminator {
        if module.returns_unknown_members(span) {
            scan.open = true;
        }
    }
    scan.scan_block(&root.block, true);
    for (i, body) in module.bodies.iter().enumerate() {
        if i == module.root {
            continue;
        }
        if body.params.iter().any(|p| p.name == exported) {
            scan.open = true; // param shadow — safe silence
        }
        scan.scan_block(&body.block, false);
    }
    let receiver = receiver_scan(module, exported);
    let receiver_fields =
        receiver.fields.into_keys().filter(|f| !scan.fields.contains_key(f)).collect();
    scan.self_reads.extend(receiver.self_reads);
    ExportShape {
        fields: scan.fields,
        closed: !scan.open && !receiver.open,
        declared: scan.saw_decl,
        self_reads: scan.self_reads,
        dynamic_self_read: scan.dynamic_self_read || receiver.dynamic_self_read,
        receiver_fields,
    }
}

struct ExportScan<'a> {
    module: &'a hir::Module,
    exported: &'a str,
    fields: HashMap<String, Span>,
    open: bool,
    /// The first top-level `local <exported> = ...` is the declaration;
    /// any other binding of the name is a rebind/shadow and opens.
    saw_decl: bool,
    self_reads: HashSet<String>,
    dynamic_self_read: bool,
}

impl<'a> ExportScan<'a> {
    fn scan_block(&mut self, block: &hir::Block, top_level: bool) {
        use hir::StmtKind as K;
        for stmt in &block.stmts {
            match &stmt.kind {
                K::Local { decls, init } => {
                    let binds = decls.iter().any(|d| d.name == self.exported);
                    if binds && top_level && !self.saw_decl {
                        self.saw_decl = true;
                        // The declaration itself: a table-literal initializer
                        // seeds the field set; `local M` bare is fine; any
                        // other initializer is not a statically-known shape.
                        let idx = decls.iter().position(|d| d.name == self.exported).unwrap();
                        // Typed with a string indexer: the members
                        // named below are not all of them.
                        if self.module.declares_unknown_members(&decls[idx]) {
                            self.open = true;
                        }
                        match init.get(idx) {
                            Some(hir::Expr::Table(fs)) => self.seed_literal(fs),
                            None => {}
                            Some(_) => self.open = true,
                        }
                        for (i, e) in init.iter().enumerate() {
                            if i != idx {
                                self.scan_expr(e);
                            }
                        }
                        continue;
                    }
                    if binds {
                        self.open = true; // rebind or shadow — safe silence
                    }
                    for e in init {
                        self.scan_expr(e);
                    }
                }
                K::Assign { targets, values } => {
                    for t in targets {
                        self.scan_write_target(t, false);
                    }
                    for e in values {
                        self.scan_expr(e);
                    }
                }
                K::CompoundAssign { target, value } => {
                    // Compound assignment reads the slot before writing it.
                    self.scan_write_target(target, true);
                    self.scan_expr(value);
                }
                K::FunctionDecl { root, path, .. } => {
                    if root == self.exported {
                        if path.is_empty() {
                            self.open = true; // `function M() end` rebinds `M`
                        }
                        if let [member] = path.as_slice() {
                            self.fields
                                .entry(member.clone())
                                .or_insert_with(|| stmt.span.clone());
                        }
                        // `function M.a.b()` / `function M.a:m()` define a
                        // member of a member — M's own set is unchanged.
                        // Storing into the table `M.a` holds reads `a`, the
                        // same as the deep write `M.a.b = v` does.
                        if let [member, _, ..] = path.as_slice() {
                            self.self_reads.insert(member.clone());
                        }
                    }
                }
                K::LocalFunction { decl, .. } => {
                    if decl.name == self.exported {
                        self.open = true;
                    }
                }
                K::NumericFor { var, start, end, step, .. } => {
                    if var.name == self.exported {
                        self.open = true;
                    }
                    self.scan_expr(start);
                    self.scan_expr(end);
                    if let Some(s) = step {
                        self.scan_expr(s);
                    }
                }
                K::GenericFor { vars, exprs, .. } => {
                    if vars.iter().any(|v| v.name == self.exported) {
                        self.open = true;
                    }
                    for e in exprs {
                        self.scan_expr(e);
                    }
                }
                K::Call(e) => self.scan_expr(e),
                K::If { cond, else_ifs, .. } => {
                    self.scan_expr(cond);
                    for (cond, _) in else_ifs {
                        self.scan_expr(cond);
                    }
                }
                K::While { cond, .. } => self.scan_expr(cond),
                K::Repeat { until, .. } => self.scan_expr(until),
                K::Do(_) => {}
            }
            // Nested blocks: statements inside lose top-level status (a
            // conditional `M.f = v` still names a member statically, so
            // writes stay collectable — only the DECL is top-level-only).
            for b in child_blocks(&stmt.kind) {
                self.scan_block(b, false);
            }
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            for e in exprs {
                // The root return IS the export (`return M` /
                // `return table.freeze(M)`) — recognized upstream, not an
                // escape. Any other return of the bare table escapes it.
                if top_level && self.is_export_return(e) {
                    continue;
                }
                self.scan_expr(e);
            }
        }
    }

    fn is_export_return(&self, e: &hir::Expr) -> bool {
        match e {
            hir::Expr::Name { name, .. } => name == self.exported,
            _ => super::calls::freeze_wrapped_name(e) == Some(self.exported),
        }
    }

    fn seed_literal(&mut self, fields: &[hir::TableField]) {
        for f in fields {
            match f {
                hir::TableField::Named { name, span, value } => {
                    self.fields.entry(name.clone()).or_insert_with(|| span.clone());
                    self.scan_expr(value);
                }
                hir::TableField::Keyed { key, value } => {
                    match key {
                        hir::Expr::Literal {
                            prim: hir::Prim::String,
                            text: Some(t),
                            span,
                        } => {
                            self.fields.entry(t.clone()).or_insert_with(|| span.clone());
                        }
                        _ => self.open = true, // computed key — not static
                    }
                    self.scan_expr(value);
                }
                // Positional items are numeric members; dot-name membership
                // is unaffected, and bracket reads are never checked.
                hir::TableField::Item(e) => self.scan_expr(e),
            }
        }
    }

    fn scan_write_target(&mut self, t: &hir::Target, reads_slot: bool) {
        match t {
            hir::Target::Name { name, .. } => {
                if name == self.exported {
                    self.open = true; // `M = ...` rebind
                }
            }
            hir::Target::Projection(e) => match e {
                hir::Expr::Index { base, key, name, name_span }
                    if matches!(&**base,
                        hir::Expr::Name { name: b, .. } if b == self.exported) =>
                {
                    match (key, name, name_span) {
                        // `M.x = v` — statically named, wherever it sits.
                        (None, Some(f), Some(sp)) => {
                            self.fields.entry(f.clone()).or_insert_with(|| sp.clone());
                            if reads_slot {
                                self.self_reads.insert(f.clone());
                            }
                        }
                        // `M[k] = v` — dynamic member. Open.
                        _ => self.open = true,
                    }
                    if let Some(k) = key {
                        self.scan_expr(k);
                    }
                }
                // Deeper chains (`M.a.b = v`) mutate a member's table, not
                // M's member set — the inner `M.a` is an ordinary read.
                other => self.scan_expr(other),
            },
        }
    }

    /// Escape scan: any occurrence of the bare exported name outside the
    /// whitelisted read positions opens the interface.
    fn scan_expr(&mut self, e: &hir::Expr) {
        use hir::Expr as E;
        match e {
            E::Name { name, .. } => {
                if name == self.exported {
                    self.open = true;
                }
            }
            E::Index { base, key, name, .. } => {
                // `M.f` / `M[k]` — reads off the table are benign. A dot
                // read is a self-read (R0309); a computed key may reach any
                // member, so it silences R0309 for the whole module.
                if matches!(&**base, E::Name { name, .. } if name == self.exported) {
                    match (key, name) {
                        (None, Some(f)) => {
                            self.self_reads.insert(f.clone());
                        }
                        _ => self.dynamic_self_read = true,
                    }
                } else {
                    self.scan_expr(base);
                }
                if let Some(k) = key {
                    self.scan_expr(k);
                }
            }
            E::Call { base, args, method, .. } => {
                match (&**base, method) {
                    // `M:m(...)` — the receiver read is benign, and a
                    // self-read of the method slot.
                    (E::Name { name, .. }, Some(m)) if name == self.exported => {
                        self.self_reads.insert(m.clone());
                    }
                    // `M(...)` bare call, or anything else: scan the base.
                    // (A dot-callee `M.f(...)` is benign via the Index arm.)
                    _ => self.scan_expr(base),
                }
                for a in args {
                    self.scan_expr(a);
                }
            }
            E::BinOp { lhs, rhs, .. } => {
                self.scan_expr(lhs);
                self.scan_expr(rhs);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => self.scan_expr(inner),
            E::Table(fields) => {
                for f in fields {
                    match f {
                        hir::TableField::Keyed { key, value } => {
                            self.scan_expr(key);
                            self.scan_expr(value);
                        }
                        hir::TableField::Named { value, .. } => self.scan_expr(value),
                        hir::TableField::Item(e) => self.scan_expr(e),
                    }
                }
            }
            E::IfExpr(arms) => {
                for (cond, val) in arms {
                    if let Some(c) = cond {
                        self.scan_expr(c);
                    }
                    self.scan_expr(val);
                }
            }
            E::Composite(parts) => {
                for p in parts {
                    self.scan_expr(p);
                }
            }
            E::Function(_) | E::Literal { .. } | E::Leaf => {}
        }
    }
}

fn child_blocks(kind: &hir::StmtKind) -> Vec<&hir::Block> {
    use hir::StmtKind as K;
    match kind {
        K::Do(b) | K::While { body: b, .. } | K::Repeat { body: b, .. } => vec![b],
        K::NumericFor { body: b, .. } | K::GenericFor { body: b, .. } => vec![b],
        K::If { then, else_ifs, or_else, .. } => {
            let mut out = vec![then];
            out.extend(else_ifs.iter().map(|(_, b)| b));
            out.extend(or_else.as_ref());
            out
        }
        _ => Vec::new(),
    }
}

// --- R0307: unknown field read -------------------------------------------

/// Walk every body; a dot-read `Mod.field` off a require-bound local whose
/// interface is closed must name an existing member. Call bases are excluded
/// (the call checker owns those — R0306); write targets are excluded (a
/// write to a frozen module is freeze-rule territory, reads only here).
pub(crate) fn check_field_reads(
    module: &hir::Module,
    externals: &HashMap<String, &ModuleInterface>,
    diags: &mut Vec<Diagnostic>,
) {
    if externals.is_empty() {
        return;
    }
    // A require-bound name reassigned anywhere no longer provably holds the
    // module — same conservatism as the call registry's poison set.
    let mut poisoned: HashSet<String> = HashSet::new();
    for body in &module.bodies {
        hir::visit::stmts(body, &mut |stmt| {
            let targets: &[hir::Target] = match &stmt.kind {
                hir::StmtKind::Assign { targets, .. } => targets,
                hir::StmtKind::CompoundAssign { target, .. } => std::slice::from_ref(target),
                _ => &[],
            };
            for t in targets {
                if let hir::Target::Name { name, .. } = t {
                    if externals.contains_key(name) {
                        poisoned.insert(name.clone());
                    }
                }
            }
        });
    }

    let mut checker = FieldReadChecker { externals, poisoned, diags };
    let mut shadow = Vec::new();
    checker.walk_block(&module.body(module.root).block, &mut shadow);
    for (i, body) in module.bodies.iter().enumerate() {
        if i == module.root {
            continue;
        }
        // Placeholder bottom keeps params ABOVE the exempt module scope —
        // a param named like the require binding must shadow it.
        let mut shadow: Vec<HashSet<String>> = vec![
            HashSet::new(),
            body.params.iter().map(|p| p.name.clone()).collect(),
        ];
        checker.walk_block(&body.block, &mut shadow);
    }
}

struct FieldReadChecker<'a> {
    externals: &'a HashMap<String, &'a ModuleInterface>,
    poisoned: HashSet<String>,
    diags: &'a mut Vec<Diagnostic>,
}

impl<'a> FieldReadChecker<'a> {
    fn shadowed(&self, name: &str, shadow: &[HashSet<String>]) -> bool {
        shadow.iter().skip(1).any(|s| s.contains(name)) || self.poisoned.contains(name)
    }

    fn walk_block(&mut self, block: &hir::Block, shadow: &mut Vec<HashSet<String>>) {
        shadow.push(HashSet::new());
        for stmt in &block.stmts {
            self.walk_stmt(stmt, shadow);
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            for e in exprs {
                self.walk_expr(e, shadow, false);
            }
        }
        shadow.pop();
    }

    fn walk_stmt(&mut self, stmt: &hir::Stmt, shadow: &mut Vec<HashSet<String>>) {
        use hir::StmtKind as K;
        match &stmt.kind {
            K::Local { decls, init } => {
                for e in init {
                    self.walk_expr(e, shadow, false);
                }
                for d in decls {
                    shadow.last_mut().unwrap().insert(d.name.clone());
                }
            }
            K::Assign { targets, values } => {
                for e in values {
                    self.walk_expr(e, shadow, false);
                }
                for t in targets {
                    self.walk_target(t, shadow);
                }
            }
            K::CompoundAssign { target, value } => {
                self.walk_expr(value, shadow, false);
                self.walk_target(target, shadow);
            }
            K::Call(e) => self.walk_expr(e, shadow, false),
            K::Do(b) => self.walk_block(b, shadow),
            K::If { cond, then, else_ifs, or_else } => {
                self.walk_expr(cond, shadow, false);
                self.walk_block(then, shadow);
                for (c, b) in else_ifs {
                    self.walk_expr(c, shadow, false);
                    self.walk_block(b, shadow);
                }
                if let Some(b) = or_else {
                    self.walk_block(b, shadow);
                }
            }
            K::While { cond, body } => {
                self.walk_expr(cond, shadow, false);
                self.walk_block(body, shadow);
            }
            K::Repeat { body, until } => {
                self.walk_block(body, shadow);
                self.walk_expr(until, shadow, false);
            }
            K::NumericFor { var, start, end, step, body } => {
                self.walk_expr(start, shadow, false);
                self.walk_expr(end, shadow, false);
                if let Some(s) = step {
                    self.walk_expr(s, shadow, false);
                }
                shadow.push(HashSet::from([var.name.clone()]));
                self.walk_block(body, shadow);
                shadow.pop();
            }
            K::GenericFor { vars, exprs, body } => {
                for e in exprs {
                    self.walk_expr(e, shadow, false);
                }
                shadow.push(vars.iter().map(|v| v.name.clone()).collect());
                self.walk_block(body, shadow);
                shadow.pop();
            }
            K::LocalFunction { decl, .. } => {
                shadow.last_mut().unwrap().insert(decl.name.clone());
            }
            K::FunctionDecl { .. } => {}
        }
    }

    fn walk_target(&mut self, t: &hir::Target, shadow: &mut Vec<HashSet<String>>) {
        match t {
            hir::Target::Name { .. } => {}
            // A one-level write to a require-bound module is R0308 — the
            // sealed-module rule. The written slot itself is not a read;
            // anything deeper is walked normally.
            hir::Target::Projection(hir::Expr::Index { base, key, name, name_span }) => {
                if let hir::Expr::Name { name: tbl, span: base_span } = &**base {
                    if !self.shadowed(tbl, shadow) {
                        if let Some(iface) = self.externals.get(tbl) {
                            self.diags.push(consumer_write(
                                name.as_deref().zip(name_span.as_ref()),
                                base_span,
                                iface,
                            ));
                        }
                    }
                } else {
                    self.walk_expr(base, shadow, false);
                }
                if let Some(k) = key {
                    self.walk_expr(k, shadow, false);
                }
            }
            hir::Target::Projection(e) => self.walk_expr(e, shadow, false),
        }
    }

    /// `in_callee` marks the immediate callee expression of a call — a
    /// dot-callee miss is R0306's diagnostic, not a second R0307.
    fn walk_expr(&mut self, e: &hir::Expr, shadow: &mut Vec<HashSet<String>>, in_callee: bool) {
        use hir::Expr as E;
        match e {
            E::Call { base, args, .. } => {
                self.walk_expr(base, shadow, true);
                for a in args {
                    self.walk_expr(a, shadow, false);
                }
            }
            E::Index { base, key, name, name_span } => {
                if let E::Name { name: tbl, .. } = &**base {
                    if !in_callee {
                        if let (Some(field), Some(span)) = (name, name_span) {
                            self.check_member(tbl, field, span, shadow);
                        }
                    }
                } else {
                    self.walk_expr(base, shadow, false);
                }
                if let Some(k) = key {
                    self.walk_expr(k, shadow, false);
                }
            }
            E::BinOp { lhs, rhs, .. } => {
                self.walk_expr(lhs, shadow, false);
                self.walk_expr(rhs, shadow, false);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => {
                self.walk_expr(inner, shadow, false)
            }
            E::Table(fields) => {
                for f in fields {
                    match f {
                        hir::TableField::Keyed { key, value } => {
                            self.walk_expr(key, shadow, false);
                            self.walk_expr(value, shadow, false);
                        }
                        hir::TableField::Named { value, .. } => {
                            self.walk_expr(value, shadow, false)
                        }
                        hir::TableField::Item(e) => self.walk_expr(e, shadow, false),
                    }
                }
            }
            E::IfExpr(arms) => {
                for (cond, val) in arms {
                    if let Some(c) = cond {
                        self.walk_expr(c, shadow, false);
                    }
                    self.walk_expr(val, shadow, false);
                }
            }
            E::Composite(parts) => {
                for p in parts {
                    self.walk_expr(p, shadow, false);
                }
            }
            E::Name { .. } | E::Function(_) | E::Literal { .. } | E::Leaf => {}
        }
    }

    fn check_member(
        &mut self,
        tbl: &str,
        field: &str,
        span: &Span,
        shadow: &[HashSet<String>],
    ) {
        if self.shadowed(tbl, shadow) {
            return;
        }
        let Some(iface) = self.externals.get(tbl) else { return };
        if !iface.closed
            || iface.functions.contains_key(field)
            || iface.fields.contains_key(field)
            || iface.receiver_fields.contains(field)
        {
            return;
        }
        self.diags.push(member_missing(Code(307), field, span, iface));
    }
}

/// R0308: a consumer wrote to a required module — sealed-module rule.
/// `named` carries the field name + its span for the static form
/// (`Mod.x = v`); None is the dynamic form (`Mod[k] = v`), labeled on the
/// module name instead.
fn consumer_write(
    named: Option<(&str, &Span)>,
    base_span: &Span,
    iface: &ModuleInterface,
) -> Diagnostic {
    let (message, span) = match named {
        Some((field, span)) => (
            format!("write to `{}` on required module `{}`", field, iface.display),
            span.clone(),
        ),
        None => (
            format!("dynamic write to required module `{}`", iface.display),
            base_span.clone(),
        ),
    };
    Diagnostic::warning(Code(308), message)
        .with_label(Label::primary(span, "written from outside the module"))
        .with_note(
            "a module's members belong to its own file — populating or patching it \
             from a consumer creates hidden coupling and defeats member checking",
        )
        .conservative(
            "Rime treats required modules as sealed; move the state or member \
             into the module that owns it",
        )
}

// --- R0306 inside the defining module ---------------------------------------

/// A call `M.f(...)` / `M:m(...)` on the module's OWN export must name a
/// member the file defines — the same theorem R0306 states across modules,
/// read from the inside. It is what catches the caller left behind when a
/// function is deleted and another function of the same file still calls
/// it: across modules the consumer's R0306 fires, but the defining file
/// had no check of its own.
///
/// Fires only when the file says exactly what the export's members are:
/// the shape is closed (see the module doc) and the export is declared by
/// the file's own top-level `local`. A closed shape also means the name
/// is never rebound or shadowed, so every `M` in the file is the export
/// and no scope tracking is needed. A member defined anywhere in the file
/// counts, whichever body or branch the definition sits in, and a value
/// member called as a function is not judged — the same lines R0306 holds
/// across modules. A member the export's own functions set through `self`
/// counts too, and a `self` that takes members the file does not name
/// opens the shape (`receiver_scan`). Reads (`local f = M.gone`) are not
/// checked.
pub(crate) fn check_own_calls(module: &hir::Module, diags: &mut Vec<Diagnostic>) {
    let Some(exported) = super::calls::exported_name(module) else { return };
    let shape = export_shape(module, &exported);
    if !shape.closed || !shape.declared {
        return;
    }
    let is_export = |e: &hir::Expr| matches!(e, hir::Expr::Name { name, .. } if *name == exported);
    let mut missing: Vec<(String, Span)> = Vec::new();
    for body in &module.bodies {
        hir::visit::exprs(body, &mut |e| {
            let hir::Expr::Call { base, method, span, .. } = e else { return };
            let (member, at) = match (&**base, method) {
                // `M:m(...)` — the tree keeps no span for the method name.
                (b, Some(m)) if is_export(b) => (m, span),
                // `M.f(...)`
                (hir::Expr::Index { base: tbl, key: None, name: Some(f), name_span }, None)
                    if is_export(tbl) =>
                {
                    (f, name_span.as_ref().unwrap_or(span))
                }
                _ => return,
            };
            if !shape.fields.contains_key(member) && !shape.receiver_fields.contains(member) {
                missing.push((member.clone(), at.clone()));
            }
        });
    }
    let count = shape.fields.len() + shape.receiver_fields.len();
    for (member, at) in missing {
        diags.push(
            Diagnostic::error(
                Code(306),
                format!("`{member}` is not a member of this module's export `{exported}`"),
            )
            .with_label(Label::primary(at, "no member of that name is defined in this file"))
            .with_note(format!(
                "`{exported}` is built in this file and never leaves it — its {count} \
                 member{} known exactly, and `{member}` is not one of them",
                if count == 1 { " is" } else { "s are" },
            ))
            .conservative(
                "Rime treats a module's members as exactly what its own file defines; \
                 if another file adds this member at runtime, define it here instead",
            ),
        );
    }
}

/// What the export's own functions do with their RECEIVER, read the way
/// the export scan reads `M`: `function M:init() self.helper = ... end`
/// defines `helper` on `M` as surely as `M.helper = ...` does, and no `M`
/// is written anywhere near. In the result, `fields` are the members
/// named through a receiver, and `open` says a receiver takes members
/// this file does not name — a computed key (`self[k] = v`), or the bare
/// receiver passed on, aliased, stored or returned — so the export's
/// members are not known after all. `self_reads` and `dynamic_self_read`
/// are the module using its own members through a receiver.
///
/// A receiver is:
///
///   - `self` in `function M:m()`;
///   - a first parameter named `self` of a function defined as a member
///     in another form (`function M.m(self)`, `M.m = function(self)`, a
///     function field of the table the export is declared with).
///
/// It keeps the role in the closures nested in that function. Everything
/// here errs towards more members and towards `open`: a receiver that is
/// in fact some other table (`M.m(other)`, a nested `function(self)`)
/// costs reports, never adds one. A first parameter of another name is
/// not a receiver: calling such a function with `:` is R0303's to report.
fn receiver_scan<'a>(module: &'a hir::Module, exported: &str) -> ExportScan<'a> {
    use hir::{Expr as E, StmtKind as K};
    // The bodies of the export's member functions that have a receiver.
    let mut receivers: Vec<hir::BodyId> = Vec::new();
    let takes_self = |id: hir::BodyId| module.body(id).params.first().is_some_and(|p| p.name == "self");
    let mut member_fn = |value: &E| {
        if let E::Function(id) = value {
            if takes_self(*id) {
                receivers.push(*id);
            }
        }
    };
    let is_export = |e: &E| matches!(e, E::Name { name, .. } if name == exported);
    let mut methods: Vec<hir::BodyId> = Vec::new();
    for body in &module.bodies {
        hir::visit::stmts(body, &mut |stmt| match &stmt.kind {
            K::FunctionDecl { root, path, is_method, body, .. } if root == exported => {
                if path.len() == 1 && (*is_method || takes_self(*body)) {
                    methods.push(*body);
                }
            }
            K::Assign { targets, values } => {
                for (target, value) in targets.iter().zip(values) {
                    if let hir::Target::Projection(E::Index { base, key: None, name: Some(_), .. }) = target {
                        if is_export(base) {
                            member_fn(value);
                        }
                    }
                }
            }
            // A closed shape has one binding of the name: the declaration.
            K::Local { decls, init } => {
                let declared = decls.iter().position(|d| d.name == exported);
                if let Some(E::Table(fields)) = declared.and_then(|i| init.get(i)) {
                    for f in fields {
                        match f {
                            hir::TableField::Named { value, .. } => member_fn(value),
                            hir::TableField::Keyed {
                                key: E::Literal { prim: hir::Prim::String, .. },
                                value,
                            } => member_fn(value),
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        });
    }
    receivers.extend(methods);

    // What the export scan asks of `M` is what is asked of the receiver
    // here: named writes are members, anything else that could add one
    // opens.
    let mut scan = ExportScan {
        module,
        exported: "self",
        fields: HashMap::new(),
        open: false,
        saw_decl: false,
        self_reads: HashSet::new(),
        dynamic_self_read: false,
    };
    for id in receivers {
        let mut bodies = vec![id];
        nested_bodies(module, id, &mut bodies);
        for id in bodies {
            scan.scan_block(&module.body(id).block, false);
        }
    }
    scan
}

/// Every function body written inside body `id`, at any depth.
fn nested_bodies(module: &hir::Module, id: hir::BodyId, out: &mut Vec<hir::BodyId>) {
    let body = module.body(id);
    let mut direct = Vec::new();
    hir::visit::stmts(body, &mut |stmt| {
        if let hir::StmtKind::LocalFunction { body, .. } | hir::StmtKind::FunctionDecl { body, .. } = &stmt.kind {
            direct.push(*body);
        }
    });
    hir::visit::exprs(body, &mut |e| {
        if let hir::Expr::Function(id) = e {
            direct.push(*id);
        }
    });
    for id in direct {
        out.push(id);
        nested_bodies(module, id, out);
    }
}

/// Shared diagnostic shape for R0306 (call) and R0307 (read).
pub(crate) fn member_missing(
    code: Code,
    field: &str,
    span: &Span,
    iface: &ModuleInterface,
) -> Diagnostic {
    let count = iface.functions.len() + iface.fields.len() + iface.receiver_fields.len();
    Diagnostic::error(
        code,
        format!("`{}` is not a member of module `{}`", field, iface.display),
    )
    .with_label(Label::primary(
        span.clone(),
        "no such member on the module's interface",
    ))
    .with_note(format!(
        "the export of `{}` is fully static — its {count} member{} known exactly",
        iface.file,
        if count == 1 { " is" } else { "s are" },
    ))
    .conservative(
        "Rime treats required modules as sealed; if this member is populated \
         at runtime, define it in the module's own file instead",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The member names `check_own_calls` reports, in source order.
    fn own_missing(src: &str) -> Vec<String> {
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        check_own_calls(&module, &mut diags);
        diags.sort_by_key(|d| d.labels[0].span.start);
        diags
            .iter()
            .map(|d| {
                assert_eq!(d.code, Code(306));
                assert!(d.conservative);
                // The label is the member name for a dot call, and the
                // whole call for a method call.
                let labelled = &src[d.labels[0].span.clone()];
                let member = d.message.split('`').nth(1).unwrap().to_string();
                assert!(labelled == member || labelled.contains(&format!(":{member}(")), "{labelled}");
                member
            })
            .collect()
    }

    #[test]
    fn own_call_to_an_undefined_member_is_reported_at_each_site() {
        let src = "local M = {}\nM.LIMIT = 1\nfunction M.a()\n\tM.gone()\n\tM:lost(1)\nend\n\
                   local function helper()\n\treturn M.gone(M.LIMIT)\nend\nM.gone()\nreturn M\n";
        assert_eq!(own_missing(src), ["gone", "lost", "gone", "gone"]);
        // `table.freeze` on the way out changes nothing.
        let frozen = "local M = {}\nfunction M.a()\n\tM.gone()\nend\nreturn table.freeze(M)\n";
        assert_eq!(own_missing(frozen), ["gone"]);
    }

    #[test]
    fn own_call_to_a_member_defined_anywhere_in_the_file_is_fine() {
        let src = "local M = { seeded = function() end, value = 1 }\n\
                   function M.a()\n\tM.b()\n\tM:c()\n\tM.late()\n\tM.seeded()\n\tM.value()\n\tM.slot()\nend\n\
                   function M.b() end\nfunction M:c() end\n\
                   function M.init(flag)\n\tif flag then\n\t\tM.late = function() end\n\tend\nend\n\
                   M.slot = nil\nreturn M\n";
        assert_eq!(own_missing(src), Vec::<String>::new());
    }

    #[test]
    fn own_calls_are_not_judged_when_the_file_does_not_fix_the_members() {
        for src in [
            // A computed key: any member may exist.
            "local M = {}\nM[key()] = function() end\nM.gone()\nreturn M\n",
            // The table leaves the file: another file may add members.
            "local M = {}\nregister(M)\nM.gone()\nreturn M\n",
            // The same in each kind of condition.
            "local M = {}\nif register(M) then\n\tM.gone()\nend\nreturn M\n",
            "local M = {}\nif a then\n\tb()\nelseif register(M) then\n\tM.gone()\nend\nreturn M\n",
            "local M = {}\nif a then\n\tb()\nelseif c then\n\tb()\nelseif register(M) then\n\tM.gone()\nend\nreturn M\n",
            "local M = {}\nwhile register(M) do\n\tM.gone()\nend\nreturn M\n",
            "local M = {}\nrepeat\n\tM.gone()\nuntil register(M)\nreturn M\n",
            // A metatable: `__index` can supply the member.
            "local M = {}\nsetmetatable(M, base)\nM.gone()\nreturn M\n",
            "local M = setmetatable({}, base)\nM.gone()\nreturn M\n",
            // Not a table this file builds.
            "local M = require(script.Parent.Other)\nM.gone()\nreturn M\n",
            // Rebound, or shadowed somewhere: an `M` may be another table.
            "local M = {}\nM = other\nM.gone()\nreturn M\n",
            "local M = {}\nfunction M() end\nM.gone()\nreturn M\n",
            "local M = {}\nlocal function f(M)\n\tM.gone()\nend\nreturn M\n",
            "local M = {}\ndo\n\tlocal M = other\n\tM.gone()\nend\nreturn M\n",
            // Never declared by this file's own top-level `local`.
            "M.gone()\nreturn M\n",
            "do\n\tlocal M = {}\nend\nM.gone()\nreturn M\n",
            // No named export.
            "local M = {}\nM.gone()\nreturn { M = M }\n",
            "local M = {}\nM.gone()\n",
        ] {
            assert_eq!(own_missing(src), Vec::<String>::new(), "{src}");
        }
    }

    #[test]
    fn an_elseif_condition_is_scanned_like_any_other() {
        // An escape there opens the shape; a read there is a self-read.
        let src = "local M = {}\nM.flag = true\nM.other = true\nfunction M.run(a)\n\tif a then\n\t\treturn 1\n\
                   \telseif M.flag then\n\t\treturn 2\n\tend\n\treturn 3\nend\nreturn M\n";
        let shape = export_shape(&rime_syntax::parse(src).unwrap(), "M");
        assert!(shape.closed);
        assert!(shape.self_reads.contains("flag"));
        assert!(!shape.self_reads.contains("other"));
        let escaped = src.replace("elseif M.flag then", "elseif register(M) then");
        assert!(!export_shape(&rime_syntax::parse(&escaped).unwrap(), "M").closed);
    }

    #[test]
    fn the_shape_carries_what_the_receiver_names_and_opens_on_the_rest() {
        let shape = |src: &str| export_shape(&rime_syntax::parse(src).unwrap(), "M");
        // Named through `self` only: a receiver field, not a field. One
        // the file also writes as `M.a` stays a field alone.
        let s = shape(
            "local M = {}\nM.a = 1\nfunction M:init()\n\tself.a = 2\n\tself.helper = function() end\nend\nreturn M\n",
        );
        assert!(s.closed);
        assert_eq!(s.receiver_fields, HashSet::from(["helper".to_string()]));
        assert!(s.fields.contains_key("a") && !s.fields.contains_key("helper"));
        // What opens the shape through `M` opens it through `self`.
        for body in ["self[k] = 1", "register(self)", "local alias = self", "return self"] {
            let src = format!("local M = {{}}\nfunction M:init(k)\n\t{body}\nend\nreturn M\n");
            assert!(!shape(&src).closed, "{src}");
        }
        // A `self` that is not the export's receiver changes nothing.
        let s = shape(
            "local M = {}\nM.ns = {}\nfunction M.ns:init()\n\tself.helper = 1\n\treturn self\nend\n\
             local function plain(self)\n\tself.other = 1\n\treturn self\nend\nM.run = plain\nreturn M\n",
        );
        assert!(s.closed);
        assert!(s.receiver_fields.is_empty());
    }

    #[test]
    fn a_read_through_the_receiver_is_a_self_read() {
        let shape = |src: &str| export_shape(&rime_syntax::parse(src).unwrap(), "M");
        let s = shape(
            "local M = {}\nM.limit = 3\nM.spare = 4\nfunction M:step() end\nfunction M:idle() end\n\
             function M:run()\n\ttask.defer(function()\n\t\tself:step(self.limit)\n\tend)\nend\nreturn M\n",
        );
        assert!(s.closed && !s.dynamic_self_read);
        assert_eq!(s.self_reads, HashSet::from(["limit".to_string(), "step".to_string()]));
        // A computed key on the receiver may read any member.
        let s = shape("local M = {}\nM.limit = 3\nfunction M:get(k)\n\treturn self[k]\nend\nreturn M\n");
        assert!(s.closed && s.dynamic_self_read);
    }

    #[test]
    fn a_member_set_through_the_receiver_is_a_member() {
        // `self` in a `:` method, in a closure inside one, and as an
        // explicit first parameter.
        for def in [
            "function M:init()\n\tself.helper = function() end\nend\n",
            "function M:init()\n\ttask.defer(function()\n\t\tself.helper = function() end\n\tend)\nend\n",
            "function M.init(self)\n\tself.helper = function() end\nend\n",
            "M.init = function(self)\n\tself.helper = function() end\nend\n",
            "function M:init()\n\tfunction self.helper() end\nend\n",
        ] {
            let src = format!(
                "local M = {{}}\n{def}function M.run()\n\tM:init()\n\tM.helper()\n\tM:helper()\n\tM.gone()\nend\nreturn M\n"
            );
            assert_eq!(own_missing(&src), ["gone"], "{src}");
        }
        // In the table the export is declared with.
        let src = "local M = { init = function(self)\n\tself.helper = function() end\nend }\n\
                   function M.run()\n\tM:init()\n\tM.helper()\n\tM.gone()\nend\nreturn M\n";
        assert_eq!(own_missing(src), ["gone"]);
        // A first parameter of another name is not a receiver (calling
        // such a function with `:` is R0303), nor is a second `self`.
        let src = "local M = {}\nfunction M.init(this)\n\tthis.helper = function() end\nend\n\
                   function M.fill(x, self)\n\tself.other = function() end\nend\n\
                   function M.run(x)\n\tM.init(x)\n\tM.helper()\n\tM.other()\nend\nreturn M\n";
        assert_eq!(own_missing(src), ["helper", "other"]);
        // `self` of `M.ns:m` is `M.ns`, and a plain local function has no
        // receiver.
        let src = "local M = {}\nM.ns = {}\nfunction M.ns:init()\n\tself.helper = function() end\nend\n\
                   local function plain(self)\n\tself.other = function() end\nend\n\
                   function M.run()\n\tM.helper()\n\tM.other()\n\treturn plain\nend\nreturn M\n";
        assert_eq!(own_missing(src), ["helper", "other"]);
    }

    #[test]
    fn a_receiver_that_takes_unknown_members_silences_the_file() {
        for body in [
            // A computed key.
            "self[name] = value",
            // Passed on, aliased, stored, returned: other code can add to it.
            "register(self)",
            "local alias = self\n\talias.helper = value",
            "holder.target = self",
            "return self",
            "setmetatable(self, base)",
            // The same inside a closure.
            "task.defer(function()\n\t\tregister(self)\n\tend)",
        ] {
            let src = format!(
                "local M = {{}}\nfunction M:init(name, value)\n\t{body}\nend\n\
                 function M.run()\n\tM:init()\n\tM.gone()\nend\nreturn M\n"
            );
            assert_eq!(own_missing(&src), Vec::<String>::new(), "{src}");
        }
        // Reading through the receiver, or calling a method on it, tells
        // nothing new about its members.
        let src = "local M = {}\nfunction M:init()\n\tlocal v = self.value\n\tself:step(v)\nend\n\
                   function M:step() end\nfunction M.run()\n\tM.gone()\nend\nreturn M\n";
        assert_eq!(own_missing(src), ["gone"]);
    }

    #[test]
    fn only_calls_one_step_off_the_export_are_judged() {
        // A read, a deeper chain, another table, and `self`.
        let src = "local M = {}\nlocal other = {}\nfunction M.a(self)\n\tlocal f = M.gone\n\
                   \tM.ns.gone()\n\tother.gone()\n\tself.gone()\n\tself:gone()\n\treturn f\nend\n\
                   M.ns = {}\nreturn M\n";
        assert_eq!(own_missing(src), Vec::<String>::new());
    }
}
