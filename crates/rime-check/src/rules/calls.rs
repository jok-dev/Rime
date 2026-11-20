//! Call conformance.
//!
//! Every call site whose callee is statically known is checked against the
//! callee's declared signature: arity (R0301 missing / R0304 extra), the
//! `.`/`:` self-shift bug (R0303), and argument types where both sides
//! classify to a primitive (R0302).
//!
//! What "statically known" means this milestone:
//!   - `local function f(...)` at any scope, and `f(...)` calls that resolve
//!     to it under ordinary shadowing rules
//!   - `function Tbl.f(...)` / `function Tbl:m(...)` where `Tbl` is a local,
//!     called as `Tbl.f(...)` / `Tbl:m(...)`
//!   - a name that is EVER reassigned (or captured as an assignment target)
//!     drops out of the registry — conservative silence over guessing
//!
//! Silence rules (false negatives over false positives, every one deliberate):
//!   - a call whose last argument is a call or `...` has an unknowable arity
//!     (multi-value expansion) — arity checks are suppressed, type checks
//!     still run on the args before the tail
//!   - missing arguments only error when the omitted parameter's type
//!     excludes nil (an omitted `T?` param is an in-type nil)
//!   - unannotated params never produce R0301/R0302
//!   - variadic callees never produce R0304
//!
//! Cross-file (phase 2b) plugs in here: a require'd module's exported
//! signatures land in the same `Signature` shape, keyed by the local the
//! require was bound to.
//!
//! R0304 on a plain name, `f(...)`, is Selene's `mismatched_arg_count`
//! (selene parity, 2026-11-20) and does not use the registry above. The
//! callee is whatever variable the name reaches under Luau's scoping
//! (`crate::scope`), at any depth, and the variable is judged by every
//! function written to it anywhere in the file: `local function f`,
//! `function f()`, `local f = function`, `f = function`. Like Selene:
//!   - order does not matter: a call is checked against every definition,
//!     whether it runs before or after it
//!   - with several definitions, a call is reported only when it passes
//!     more than the largest of them takes
//!   - one variadic definition silences the variable
//!   - `f(a, g())` and `f(a, ...)` supply at least two arguments; past the
//!     last parameter that is enough to report (plain names only: `T.f`
//!     with a call or `...` as its last argument is not counted, as before)
//!   - a global the file defines is checked the same way
//!
//! Differences from Selene, all deliberate:
//!   - a variable that is also given anything but a function written in
//!     place (`f = require(x)`, `f = f or g`, a parameter, a loop
//!     variable, `f += 1`) is not checked. Selene ignores such writes and
//!     reports against the function it saw; Rime cannot say what the
//!     variable holds at the call. `f = nil` and `local f` do not count:
//!     a call that reaches nil throws, it does not take arguments.
//!   - `(f)(1, 2)` is checked; Selene skips a parenthesized callee.
//!   - Rime keeps its checks one step off a table (`T.f(...)`,
//!     `T:m(...)`) and across files. Selene checks plain names only.
//!   - a table slot given more than one function (`M.f = function() end`
//!     as a stand-in, the real function assigned later) is not checked by
//!     any call rule, in this file or from another. The slot is one field
//!     of one table variable (`crate::scope::Binding`): a table that only
//!     shares the name has slots of its own. Only a function written in
//!     place counts (`function M.f`, `function M:f`, `M.f = function`);
//!     `M.f = nil` and `M.f = wrap(M.f)` leave the one function the slot
//!     was given as the signature calls are checked against.

use crate::scope;
use rime_diag::{Code, Diagnostic, Label, Span};
use rime_hir as hir;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallForm {
    /// `f(args)` or `Tbl.f(args)`
    Plain,
    /// `Tbl:m(args)`
    Method,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Signature {
    /// Display name for diagnostics (`applyDamage`, `Shop.buy`).
    name: String,
    /// Declared with `:` — implicit self not listed in `params`.
    is_method: bool,
    params: Vec<ParamInfo>,
    has_varargs: bool,
    /// Span of the declaration's name, for the secondary label. Only
    /// meaningful within the file the signature came from.
    pub(crate) decl_span: Span,
    /// Set when the signature lives in ANOTHER file (cross-file call
    /// checking): spans cannot label the caller's source, so diagnostics
    /// cite this as "declared in <file>:<line>" notes instead.
    decl_loc: Option<(String, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
struct ParamInfo {
    name: String,
    span: Span,
    ty: Option<hir::Ty>,
}

/// A required module's callable surface, keyed by field name — what
/// cross-file call checking consumes. Built once per file by
/// `module_interface`, shared by every caller.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModuleInterface {
    pub(crate) functions: HashMap<String, Signature>,
    /// Non-function member NAMES: table-literal entries and static
    /// `M.x = <value>` writes, keyed to their declaration span in the
    /// defining file (what R0309 anchors on). An over-approximation of the
    /// member set — membership checking needs "could exist", so any
    /// statically-named write counts, whichever body it sits in.
    pub(crate) fields: HashMap<String, Span>,
    /// Members the module names only through `self` in its own functions
    /// (`members::ExportShape::receiver_fields`). They count for
    /// membership (R0306, R0307) and are never reported by R0309.
    pub(crate) receiver_fields: HashSet<String>,
    /// Member names the module reads off its own export — internal use
    /// counts as use for R0309 (unused export).
    pub(crate) self_reads: HashSet<String>,
    /// The module indexes its own export with a computed key — R0309 stays
    /// silent on the whole module (membership checking is unaffected).
    pub(crate) dynamic_self_read: bool,
    /// The extractor saw the WHOLE export shape: frozen return,
    /// statically-known member set, and the exported table never escapes.
    /// Only a closed interface supports "member missing" as a theorem;
    /// open interfaces stay silent on unknown members.
    pub(crate) closed: bool,
    /// Module display name (file stem) for member-existence messages.
    pub(crate) display: String,
    /// File name for "module defined in <file>" notes.
    pub(crate) file: String,
}

/// Extract the exported table's function slots from a lowered module.
/// `display` names the module in diagnostics (usually the file stem);
/// `src` locates declarations for the "declared in file:line" notes.
pub(crate) fn module_interface(
    module: &hir::Module,
    display: &str,
    file: &str,
    src: &str,
) -> ModuleInterface {
    let mut diags = Vec::new(); // collection never emits, but Checker wants it
    let externals = HashMap::new();
    let names = scope::Resolution::default(); // only call sites read it
    let mut checker = Checker {
        module,
        names: &names,
        registry: HashMap::new(),
        poisoned: HashSet::new(),
        rewritten: HashSet::new(),
        externals: &externals,
        diags: &mut diags,
    };
    checker.collect(&module.body(module.root).block);

    // `return script` exports the module's own INSTANCE (the wally SDK
    // namespace idiom: members are child ModuleScripts, navigated with
    // `Sdk.Modules.X`). The extractor cannot know those members — the
    // interface must stay OPEN, never closed-and-empty. Same treatment for
    // anything `export_root` doesn't recognize as a named table.
    let exported = match crate::eval::export_root(module) {
        crate::eval::ExportRoot::Table(t) => t,
        crate::eval::ExportRoot::Instance | crate::eval::ExportRoot::None => {
            return ModuleInterface::default();
        }
    };

    // Fields of the exported table that are given more than one function.
    // Telling the exported table from another of the same name takes name
    // resolution, which is only run for a file where some `name.field` is
    // given two functions at all.
    let slots = slot_functions(module);
    let mut rewritten: HashSet<String> = HashSet::new();
    if any_slot_named_twice(&slots) {
        if let Some((name, site)) = exported_name_site(module) {
            let names = scope::resolve(module);
            let export = names.binding(name, site);
            rewritten = rewritten_slots(&slots, &names)
                .into_iter()
                .filter(|(table, _)| *table == export)
                .map(|(_, field)| field)
                .collect();
        }
    }

    let mut functions = HashMap::new();
    for (key, mut sig) in checker.registry {
        let Key::Field(tbl, field) = key else {
            continue;
        };
        // A slot given more than one function has no one signature. It
        // stays a member (`fields` below), so R0306 does not call it
        // missing.
        if tbl != exported || rewritten.contains(&field) {
            continue;
        }
        let line = src[..sig.decl_span.start.min(src.len())]
            .matches('\n')
            .count()
            + 1;
        sig.name = format!("{display}.{field}");
        sig.decl_loc = Some((file.to_string(), line));
        // The registry records the whole `function ... end` statement; the
        // declaration SITE is the member NAME — R0309's label should pin
        // it, not paint the body or the header. Narrow to the name token
        // within the header line (`.name` / `:name`); if the scan fails,
        // the header line is the fallback.
        let start = sig.decl_span.start.min(src.len());
        let end = sig.decl_span.end.min(src.len());
        let header_end = src[start..end].find('\n').map_or(end, |nl| start + nl);
        sig.decl_span.end = header_end;
        if let Some(pos) = member_token(&src[start..header_end], &field) {
            sig.decl_span.start = start + pos;
            sig.decl_span.end = start + pos + field.len();
        }
        functions.insert(field, sig);
    }
    let mut shape = super::members::export_shape(module, &exported);
    // The shape scan names every statically-written member, including the
    // function slots the registry already carries — keep `fields` to the
    // value-only remainder so member counts don't double.
    shape.fields.retain(|f, _| !functions.contains_key(f));
    ModuleInterface {
        functions,
        fields: shape.fields,
        receiver_fields: shape.receiver_fields,
        self_reads: shape.self_reads,
        dynamic_self_read: shape.dynamic_self_read,
        closed: shape.closed,
        display: display.to_string(),
        file: file.to_string(),
    }
}

/// Byte offset of `name` in `header` as a member token: preceded by `.` or
/// `:`, followed by a non-identifier byte (or the end). None when the
/// header does not contain it that way — the caller keeps its wider span.
fn member_token(header: &str, name: &str) -> Option<usize> {
    let is_ident = |b: u8| b == b'_' || b.is_ascii_alphanumeric();
    let bytes = header.as_bytes();
    let mut from = 0;
    while let Some(i) = header[from..].find(name).map(|i| from + i) {
        let before = i.checked_sub(1).map(|p| bytes[p]);
        let after = bytes.get(i + name.len());
        if matches!(before, Some(b'.') | Some(b':')) && !after.copied().is_some_and(is_ident) {
            return Some(i);
        }
        from = i + 1;
    }
    None
}

/// The name of the module's exported table: `return M` or
/// `return table.freeze(M)`. `None` for a module that returns something
/// unnamed (a literal, a call other than `table.freeze`, nothing at all)
/// — and `None` for `return script`, which exports an INSTANCE, not a
/// table named `script`.
///
/// Expressed in terms of `eval::export_root` on purpose: a hand-rolled
/// copy of the same match had drifted (it answered `Some("script")`, so
/// go-to-definition would look for `script`'s members in the module's own
/// HIR), and there must be exactly one interpretation of "what does this
/// module export". Shared by
/// `module_interface` and go-to-definition's own-export lookup.
pub(crate) fn exported_name(module: &hir::Module) -> Option<String> {
    match crate::eval::export_root(module) {
        crate::eval::ExportRoot::Table(t) => Some(t),
        crate::eval::ExportRoot::Instance | crate::eval::ExportRoot::None => None,
    }
}

/// `table.freeze(M)` → `M`
pub(crate) fn freeze_wrapped_name(e: &hir::Expr) -> Option<&str> {
    freeze_wrapped(e).map(|(name, _)| name)
}

/// The exported table's name where the module's `return` writes it:
/// `return M` or `return table.freeze(M)`. The same two shapes as
/// `eval::export_root`.
fn exported_name_site(module: &hir::Module) -> Option<(&str, &Span)> {
    let root = module.body(module.root);
    let Some(hir::Terminator::Return { exprs, .. }) = &root.block.terminator else {
        return None;
    };
    match exprs.first()? {
        hir::Expr::Name { name, span } => Some((name, span)),
        e => freeze_wrapped(e),
    }
}

/// `table.freeze(M)` → `M`, and where `M` is written.
fn freeze_wrapped(e: &hir::Expr) -> Option<(&str, &Span)> {
    let hir::Expr::Call { base, args, .. } = e else {
        return None;
    };
    let hir::Expr::Index {
        base: tbl,
        key: None,
        name: Some(f),
        ..
    } = &**base
    else {
        return None;
    };
    let hir::Expr::Name { name: t, .. } = &**tbl else {
        return None;
    };
    if t != "table" || f != "freeze" {
        return None;
    }
    match args.first() {
        Some(hir::Expr::Name { name, span }) => Some((name, span)),
        _ => None,
    }
}

/// Callee key: a bare local (`f`) or one dot/colon step off a local
/// (`Tbl` + `buy`).
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
enum Key {
    Local(String),
    Field(String, String),
}

/// The exported-function-reassign rule (R0305): the module's
/// exported table gets its function slots assigned exactly once. A later
/// assignment is rejected as conservative — late-binding is running code, but
/// module returns are require-cached and shared process-wide, so a swapped
/// slot changes behavior for every caller and silently invalidates call
/// checking. Locals are exempt on purpose; so are tables that are not the
/// export (callback data tables legitimately hot-swap). A sweep of a
/// large codebase: no cross-file function writes, a few in-file
/// late-bindings.
fn check_exported_reassign(module: &hir::Module, diags: &mut Vec<Diagnostic>) {
    let root = module.body(module.root);
    // Which local does this module export? `return M` or `return table.freeze(M)`.
    let Some(hir::Terminator::Return { exprs, .. }) = &root.block.terminator else {
        return;
    };
    let exported = match exprs.first() {
        Some(hir::Expr::Name { name, .. }) => name.clone(),
        Some(hir::Expr::Call { base, args, .. }) => {
            let hir::Expr::Index {
                base: tbl,
                key: None,
                name: Some(f),
                ..
            } = &**base
            else {
                return;
            };
            let hir::Expr::Name { name: t, .. } = &**tbl else {
                return;
            };
            if t != "table" || f != "freeze" {
                return;
            }
            match args.first() {
                Some(hir::Expr::Name { name, .. }) => name.clone(),
                _ => return,
            }
        }
        _ => return,
    };

    // Every slot event on the exported table, in source order across all
    // bodies: `function M.f()` declarations and `M.f = <value>` assignments.
    // (span, field, is_function_valued)
    let mut events: Vec<(usize, Span, String, bool)> = Vec::new();
    for body in &module.bodies {
        collect_slot_events(&body.block, &exported, &mut events);
    }
    events.sort_by_key(|(start, ..)| *start);

    let mut first_seen: HashMap<String, (Span, bool)> = HashMap::new();
    for (_, span, field, is_fn) in events {
        match first_seen.get(&field) {
            None => {
                first_seen.insert(field, (span, is_fn));
            }
            Some((decl_span, decl_was_fn)) => {
                // Only function slots are protected: a data field on the
                // export is the separate (opt-in, future) shared-state rule.
                if !decl_was_fn && !is_fn {
                    continue;
                }
                let decl_span = decl_span.clone();
                diags.push(
                    Diagnostic::error(
                        Code(305),
                        format!("cannot reassign exported function `{exported}.{field}`"),
                    )
                    .with_label(Label::primary(span, "reassigned here"))
                    .with_label(Label::secondary(decl_span, "first defined here"))
                    .with_note(format!(
                        "`{exported}` is this module's export — module returns are \
                         require-cached and shared process-wide, so swapping a function \
                         on it changes behavior for every caller"
                    ))
                    .conservative(
                        "this may be deliberate late-binding that works today, but Rime \
                         cannot prove call sites stay valid once the slot changes",
                    )
                    .with_help(format!(
                        "define the final `{exported}.{field}` once, or route the \
                         varying behavior through a local variable"
                    )),
                );
            }
        }
    }
}

fn collect_slot_events(
    block: &hir::Block,
    exported: &str,
    out: &mut Vec<(usize, Span, String, bool)>,
) {
    for stmt in &block.stmts {
        match &stmt.kind {
            hir::StmtKind::FunctionDecl { root, path, .. } => {
                if root == exported {
                    if let [field] = path.as_slice() {
                        out.push((stmt.span.start, stmt.span.clone(), field.clone(), true));
                    }
                }
            }
            hir::StmtKind::Assign { targets, values } => {
                for (i, t) in targets.iter().enumerate() {
                    let hir::Target::Projection(e) = t else {
                        continue;
                    };
                    let Some((tbl, field)) = one_level_field(e) else {
                        continue;
                    };
                    if tbl != exported {
                        continue;
                    }
                    let is_fn = matches!(values.get(i), Some(hir::Expr::Function(_)));
                    out.push((stmt.span.start, stmt.span.clone(), field, is_fn));
                }
            }
            hir::StmtKind::CompoundAssign { target, .. } => {
                if let hir::Target::Projection(e) = target {
                    if let Some((tbl, field)) = one_level_field(e) {
                        if tbl == exported {
                            out.push((stmt.span.start, stmt.span.clone(), field, false));
                        }
                    }
                }
            }
            _ => {}
        }
        for b in child_blocks(&stmt.kind) {
            collect_slot_events(b, exported, out);
        }
    }
}

/// `t.f` (dot, one level) → (t, f)
fn one_level_field(e: &hir::Expr) -> Option<(String, String)> {
    let hir::Expr::Index {
        base,
        key: None,
        name: Some(field),
        ..
    } = e
    else {
        return None;
    };
    let hir::Expr::Name { name, .. } = &**base else {
        return None;
    };
    Some((name.clone(), field.clone()))
}

pub(crate) fn check_module(
    module: &hir::Module,
    names: &scope::Resolution,
    diags: &mut Vec<Diagnostic>,
) {
    let externals = HashMap::new();
    check_module_with(module, names, &externals, diags);
}

/// Cross-file variant: `externals` maps a require-bound local name to the
/// required module's interface (built by the project layer).
pub(crate) fn check_module_with(
    module: &hir::Module,
    names: &scope::Resolution,
    externals: &HashMap<String, &ModuleInterface>,
    diags: &mut Vec<Diagnostic>,
) {
    check_exported_reassign(module, diags);
    super::members::check_own_calls(module, diags);
    let mut checker = Checker {
        module,
        names,
        registry: HashMap::new(),
        poisoned: HashSet::new(),
        rewritten: rewritten_slots(&slot_functions(module), names),
        externals,
        diags,
    };
    checker.collect(&module.body(module.root).block);
    // Second pass: with the registry complete, walk the module from its
    // root, entering each nested body where it is created. A body's scope
    // chain runs through every function around it, so the stack of names
    // declared on the way down is what decides whether a callee still
    // names the module-level declaration. (Walking the body arena flat lost
    // that chain: a parameter or an enclosing local named like a
    // module-level function was checked against the function's signature.)
    let mut shadow = Vec::new();
    checker.walk_block(&module.body(module.root).block, &mut shadow, None);
}

struct Checker<'a> {
    module: &'a hir::Module,
    /// Which variable each plain name reaches — R0304 on `f(...)`.
    names: &'a scope::Resolution,
    registry: HashMap<Key, Signature>,
    /// Names whose value is not provably the declared function (reassigned,
    /// or a same-named table re-bound). Checked before every diagnostic.
    poisoned: HashSet<String>,
    /// Table slots (the table variable, `field`) given more than one
    /// function, in any body. Which one a call reaches depends on when it
    /// runs, so no call through the slot is checked.
    rewritten: HashSet<(scope::Binding, String)>,
    /// require-bound local name -> the required module's interface.
    externals: &'a HashMap<String, &'a ModuleInterface>,
    diags: &'a mut Vec<Diagnostic>,
}

impl<'a> Checker<'a> {
    // --- pass 1: signature registry -----------------------------------------

    /// Collect module-scope declarations, and poison names that are
    /// reassigned anywhere (any scope — cheap and conservative).
    fn collect(&mut self, block: &hir::Block) {
        self.collect_block(block, true);
        // Reassignment scan must see every body, not just the module root:
        // a nested `f = other` invalidates the registry entry for `f`.
        for body in &self.module.bodies {
            self.poison_scan(&body.block);
        }
        // A name declared twice at module level (`local function f` twice,
        // or `local function f` then `local f = ...`) means two different
        // values, and which one a call reaches depends on where the call
        // sits. The registry is keyed by name alone, so it cannot say:
        // drop the name rather than check a call against the wrong one.
        let mut declared: HashMap<&str, u32> = HashMap::new();
        for stmt in &block.stmts {
            match &stmt.kind {
                hir::StmtKind::Local { decls, .. } => {
                    for d in decls {
                        *declared.entry(d.name.as_str()).or_default() += 1;
                    }
                }
                hir::StmtKind::LocalFunction { decl, .. } => {
                    *declared.entry(decl.name.as_str()).or_default() += 1;
                }
                _ => {}
            }
        }
        for (name, count) in declared {
            if count > 1 {
                self.poisoned.insert(name.to_string());
            }
        }
    }

    fn collect_block(&mut self, block: &hir::Block, top_level: bool) {
        for stmt in &block.stmts {
            match &stmt.kind {
                hir::StmtKind::LocalFunction { decl, body } => {
                    if top_level {
                        self.insert(
                            Key::Local(decl.name.clone()),
                            decl.name.clone(),
                            false,
                            *body,
                            decl.span.clone(),
                        );
                    }
                }
                hir::StmtKind::FunctionDecl {
                    root,
                    path,
                    is_method,
                    body,
                    ..
                } => {
                    if !top_level {
                        continue;
                    }
                    match path.as_slice() {
                        [] => self.insert(
                            Key::Local(root.clone()),
                            root.clone(),
                            false,
                            *body,
                            stmt.span.clone(),
                        ),
                        [field] => self.insert(
                            Key::Field(root.clone(), field.clone()),
                            format!("{root}.{field}"),
                            *is_method,
                            *body,
                            stmt.span.clone(),
                        ),
                        // `function a.b.c()` — deeper than the milestone models.
                        _ => {}
                    }
                }
                // `local M = {}; M.f = function() end` — the assignment IS the
                // declaration in this common module style; registering it both
                // widens call checking and defines the slot for the
                // exported-function-reassign rule.
                hir::StmtKind::Assign { targets, values } => {
                    for (i, t) in targets.iter().enumerate() {
                        let hir::Target::Projection(e) = t else {
                            continue;
                        };
                        let Some((tbl, field)) = one_level_field(e) else {
                            continue;
                        };
                        let Some(hir::Expr::Function(body)) = values.get(i) else {
                            continue;
                        };
                        let key = Key::Field(tbl.clone(), field.clone());
                        // First one wins; a second is the reassign rule's job.
                        if !self.registry.contains_key(&key) {
                            self.insert(
                                key,
                                format!("{tbl}.{field}"),
                                false,
                                *body,
                                stmt.span.clone(),
                            );
                        }
                    }
                }
                // `do` blocks at top level still declare into module scope
                // conceptually? No — they scope their locals. Only true
                // top-level statements feed the registry.
                _ => {}
            }
        }
    }

    fn insert(
        &mut self,
        key: Key,
        name: String,
        is_method: bool,
        body: hir::BodyId,
        decl_span: Span,
    ) {
        let b = self.module.body(body);
        let sig = Signature {
            name,
            is_method,
            params: b
                .params
                .iter()
                .map(|p| ParamInfo {
                    name: p.name.clone(),
                    span: p.span.clone(),
                    ty: p.ty.clone(),
                })
                .collect(),
            has_varargs: b.has_varargs,
            decl_span,
            decl_loc: None,
        };
        self.registry.insert(key, sig);
    }

    /// Any assignment to a bare name poisons it: the registry can no longer
    /// claim to know what the name holds.
    fn poison_scan(&mut self, block: &hir::Block) {
        for stmt in &block.stmts {
            match &stmt.kind {
                hir::StmtKind::Assign { targets, .. } => {
                    for t in targets {
                        if let hir::Target::Name { name, .. } = t {
                            self.poisoned.insert(name.clone());
                        }
                    }
                }
                hir::StmtKind::CompoundAssign { target, .. } => {
                    if let hir::Target::Name { name, .. } = target {
                        self.poisoned.insert(name.clone());
                    }
                }
                _ => {}
            }
            for b in child_blocks(&stmt.kind) {
                self.poison_scan(b);
            }
        }
    }

    // --- pass 2: call sites --------------------------------------------------

    /// `trailing` is an expression that evaluates inside the block's own
    /// scope after its statements: the condition of `repeat ... until`,
    /// which sees the locals the body declared.
    fn walk_block(
        &mut self,
        block: &hir::Block,
        shadow: &mut Vec<HashSet<String>>,
        trailing: Option<&hir::Expr>,
    ) {
        shadow.push(HashSet::new());
        for stmt in &block.stmts {
            self.walk_stmt(stmt, shadow);
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            for e in exprs {
                self.walk_expr(e, shadow);
            }
        }
        if let Some(e) = trailing {
            self.walk_expr(e, shadow);
        }
        shadow.pop();
    }

    /// Enter a nested function body where it is created, so everything
    /// declared around it stays on the stack. Its parameters (and the
    /// implicit `self` of a `:` declaration) open a scope of their own.
    fn walk_body(&mut self, id: hir::BodyId, is_method: bool, shadow: &mut Vec<HashSet<String>>) {
        let module = self.module;
        let body = module.body(id);
        let mut params: HashSet<String> = body.params.iter().map(|p| p.name.clone()).collect();
        if is_method {
            params.insert("self".to_string());
        }
        shadow.push(params);
        self.walk_block(&body.block, shadow, None);
        shadow.pop();
    }

    fn walk_stmt(&mut self, stmt: &hir::Stmt, shadow: &mut Vec<HashSet<String>>) {
        use hir::StmtKind as K;
        match &stmt.kind {
            K::Local { decls, init } => {
                for e in init {
                    self.walk_expr(e, shadow);
                }
                for d in decls {
                    shadow.last_mut().unwrap().insert(d.name.clone());
                }
            }
            K::Assign { targets, values } => {
                for e in values {
                    self.walk_expr(e, shadow);
                }
                for t in targets {
                    if let hir::Target::Projection(e) = t {
                        self.walk_expr(e, shadow);
                    }
                }
            }
            K::CompoundAssign { target, value } => {
                self.walk_expr(value, shadow);
                if let hir::Target::Projection(e) = target {
                    self.walk_expr(e, shadow);
                }
            }
            K::Call(e) => self.walk_expr(e, shadow),
            K::Do(b) => self.walk_block(b, shadow, None),
            K::If {
                cond,
                then,
                else_ifs,
                or_else,
            } => {
                self.walk_expr(cond, shadow);
                self.walk_block(then, shadow, None);
                for (c, b) in else_ifs {
                    self.walk_expr(c, shadow);
                    self.walk_block(b, shadow, None);
                }
                if let Some(b) = or_else {
                    self.walk_block(b, shadow, None);
                }
            }
            K::While { cond, body } => {
                self.walk_expr(cond, shadow);
                self.walk_block(body, shadow, None);
            }
            K::Repeat { body, until } => {
                // The condition sees the locals the body declared, so it is
                // walked before the body's scope closes.
                self.walk_block(body, shadow, Some(until));
            }
            K::NumericFor {
                var,
                start,
                end,
                step,
                body,
            } => {
                self.walk_expr(start, shadow);
                self.walk_expr(end, shadow);
                if let Some(s) = step {
                    self.walk_expr(s, shadow);
                }
                shadow.push(HashSet::from([var.name.clone()]));
                self.walk_block(body, shadow, None);
                shadow.pop();
            }
            K::GenericFor { vars, exprs, body } => {
                for e in exprs {
                    self.walk_expr(e, shadow);
                }
                shadow.push(vars.iter().map(|v| v.name.clone()).collect());
                self.walk_block(body, shadow, None);
                shadow.pop();
            }
            K::LocalFunction { decl, body } => {
                // The local name shadows from here on, its own body
                // included (a non-top-level redeclaration hides the registry
                // entry rather than updating it).
                let top = shadow.len() == 1;
                if !top {
                    shadow.last_mut().unwrap().insert(decl.name.clone());
                }
                self.walk_body(*body, false, shadow);
            }
            K::FunctionDecl { is_method, body, .. } => self.walk_body(*body, *is_method, shadow),
        }
    }

    fn walk_expr(&mut self, expr: &hir::Expr, shadow: &mut Vec<HashSet<String>>) {
        use hir::Expr as E;
        match expr {
            E::Call {
                base,
                args,
                method,
                span,
                arg_spans,
            } => {
                self.walk_expr(base, shadow);
                for a in args {
                    self.walk_expr(a, shadow);
                }
                self.check_call(base, args, method.as_deref(), span, arg_spans, shadow);
            }
            E::Index { base, key, .. } => {
                self.walk_expr(base, shadow);
                if let Some(k) = key {
                    self.walk_expr(k, shadow);
                }
            }
            E::BinOp { lhs, rhs, .. } => {
                self.walk_expr(lhs, shadow);
                self.walk_expr(rhs, shadow);
            }
            E::Not(inner) | E::UnOp { operand: inner, .. } => self.walk_expr(inner, shadow),
            E::Table(fields) => {
                for f in fields {
                    match f {
                        hir::TableField::Keyed { key, value } => {
                            self.walk_expr(key, shadow);
                            self.walk_expr(value, shadow);
                        }
                        hir::TableField::Named { value, .. } => self.walk_expr(value, shadow),
                        hir::TableField::Item(e) => self.walk_expr(e, shadow),
                    }
                }
            }
            E::IfExpr(arms) => {
                for (cond, val) in arms {
                    if let Some(c) = cond {
                        self.walk_expr(c, shadow);
                    }
                    self.walk_expr(val, shadow);
                }
            }
            E::Composite(parts) => {
                for p in parts {
                    self.walk_expr(p, shadow);
                }
            }
            E::Function(id) => self.walk_body(*id, false, shadow),
            E::Name { .. } | E::Literal { .. } | E::Leaf => {}
        }
    }

    fn shadowed(&self, name: &str, shadow: &[HashSet<String>]) -> bool {
        // The bottom scope is module scope itself — names declared there
        // ARE the registry's names. Every scope above it (a block, a loop,
        // a function's parameters, at any nesting depth) hides a registry
        // entry of the same name. A module-level name declared twice is
        // poisoned by `collect`.
        shadow.iter().skip(1).any(|s| s.contains(name)) || self.poisoned.contains(name)
    }

    fn check_call(
        &mut self,
        base: &hir::Expr,
        args: &[hir::Expr],
        method: Option<&str>,
        call_span: &Span,
        arg_spans: &[Span],
        shadow: &[HashSet<String>],
    ) {
        use hir::Expr as E;
        // R0304 on a plain name goes by the variable the name reaches, not
        // by the registry. One call, one report: nothing else is checked
        // on a call it reported.
        if let (E::Name { name, span }, None) = (base, method) {
            if self.plain_extra_args(name, span, args, call_span, arg_spans) {
                return;
            }
        }
        // Resolve the callee to a registry key + the form used at this site.
        // `callee_name_span` is the dot-field name token when the form has
        // one — the member-existence diagnostic anchors there.
        // `root_site` is where the call writes the name it starts from.
        let (key, form, callee_name_span, root_site) = match (base, method) {
            // f(...) — bare local call.
            (E::Name { name, span }, None) => {
                (Key::Local(name.clone()), CallForm::Plain, None, span)
            }
            // Tbl:m(...) — colon call one step off a local.
            (E::Name { name, span }, Some(m)) => (
                Key::Field(name.clone(), m.to_string()),
                CallForm::Method,
                None,
                span,
            ),
            // Tbl.f(...) — dot call one step off a local.
            (
                E::Index {
                    base: tbl,
                    key: None,
                    name: Some(field),
                    name_span,
                },
                None,
            ) => {
                let E::Name { name, span } = &**tbl else { return };
                (
                    Key::Field(name.clone(), field.clone()),
                    CallForm::Plain,
                    name_span.clone(),
                    span,
                )
            }
            _ => return, // anything deeper is out of milestone scope
        };
        let root = match &key {
            Key::Local(n) | Key::Field(n, _) => n.clone(),
        };
        if self.shadowed(&root, shadow) {
            return;
        }
        if let Key::Field(tbl, field) = &key {
            if !self.rewritten.is_empty()
                && self
                    .rewritten
                    .contains(&(self.names.binding(tbl, root_site), field.clone()))
            {
                return;
            }
        }
        // Same-file registry first; then the interface of a require-bound
        // module (`local Economy = require(...)`, called as `Economy.f(...)`).
        let sig = match self.registry.get(&key) {
            Some(sig) => sig,
            None => {
                let Key::Field(tbl, field) = &key else { return };
                let Some(interface) = self.externals.get(tbl) else {
                    return;
                };
                match interface.functions.get(field) {
                    Some(sig) => sig,
                    None => {
                        // R0306: on a CLOSED interface an unknown
                        // member is a theorem. A known value FIELD called as
                        // a function stays silent — its callability is not
                        // modeled. Open interfaces stay silent entirely.
                        if interface.closed
                            && !interface.fields.contains_key(field)
                            && !interface.receiver_fields.contains(field)
                        {
                            let span = callee_name_span.as_ref().unwrap_or(call_span);
                            self.diags.push(super::members::member_missing(
                                Code(306),
                                field,
                                span,
                                interface,
                            ));
                        }
                        return;
                    }
                }
            }
        };
        // Cross-file signatures cannot label the caller's source; they cite
        // their declaration as a note instead.
        let decl_note = sig
            .decl_loc
            .as_ref()
            .map(|(file, line)| format!("signature declared in {file}:{line}"));
        let local_decl = sig.decl_loc.is_none();

        // --- R0303: `.`/`:` confusion — diagnose the form itself ----------
        // Super-call idiom: `Base.method(self, ...)` dot-calls a `:` method
        // passing self explicitly — how Lua OOP calls an inherited method.
        // `Base._update(self, dt)` is correct, and the arity below accounts
        // for the explicit self.
        let super_style = sig.is_method
            && form == CallForm::Plain
            && matches!(args.first(), Some(E::Name { name, .. }) if name == "self");
        if sig.is_method && form == CallForm::Plain && !super_style {
            let name = &sig.name;
            let d = Diagnostic::error(
                Code(303),
                format!("`{}` is a method — call it with `:`", short_name(name)),
            )
            .with_label(Label::primary(
                call_span.clone(),
                "called with `.` here, so the first argument lands in `self`",
            ));
            let d = cite_decl(
                d,
                local_decl,
                &sig.decl_span,
                "declared with `:` here",
                &decl_note,
            )
            .with_note(
                "a `:` declaration takes an implicit self; dot-calling it shifts every \
                     argument left by one",
            )
            .with_help(format!("call it as `{}(...)`", name.replace('.', ":")));
            self.diags.push(d);
            return; // arity is hopelessly shifted; one root cause, one error
        }
        // A dot-declared function whose first parameter is an explicit self
        // (`function Tbl.f(_self: Tbl)`) is method-style on purpose — the
        // colon call correctly fills that parameter, and flagging it would
        // give actively wrong advice. Name-based heuristic, deliberately
        // narrow.
        let explicit_self = sig
            .params
            .first()
            .is_some_and(|p| p.name == "self" || p.name == "_self");
        if !sig.is_method && form == CallForm::Method && !explicit_self {
            let name = &sig.name;
            let d = Diagnostic::error(
                Code(303),
                format!("`{}` is not a method — call it with `.`", short_name(name)),
            )
            .with_label(Label::primary(
                call_span.clone(),
                "called with `:` here, which passes an extra implicit self",
            ));
            let d = cite_decl(
                d,
                local_decl,
                &sig.decl_span,
                "declared without `self` here",
                &decl_note,
            )
            .with_note("a `:` call inserts the table as a hidden first argument")
            .with_help(format!("call it as `{name}(...)` with a `.`"));
            self.diags.push(d);
            return;
        }

        // --- arity ----------------------------------------------------------
        // A colon call on an explicit-self declaration fills the first
        // parameter with the table; the written arguments match the rest.
        let eff_params: &[ParamInfo] = if form == CallForm::Method && explicit_self {
            &sig.params[1..]
        } else {
            &sig.params
        };
        let display_offset = sig.params.len() - eff_params.len();
        // A super-style call's explicit `self` fills the implicit method
        // param; the remaining args match the declared params.
        let (args, arg_spans) = if super_style {
            (&args[1..], &arg_spans[1..])
        } else {
            (args, arg_spans)
        };

        // A call tail that can expand to multiple values makes the given
        // count a lower bound only.
        let open_tail = matches!(args.last(), Some(E::Call { .. }) | Some(E::Leaf));

        if !open_tail {
            // Missing: first uncovered param whose type excludes nil.
            if args.len() < eff_params.len() {
                if let Some((i, p)) = eff_params
                    .iter()
                    .enumerate()
                    .skip(args.len())
                    .find(|(_, p)| p.ty.as_ref().is_some_and(|t| !t.admits_nil))
                {
                    let name = &sig.name;
                    let pname = &p.name;
                    let rendered =
                        p.ty.as_ref()
                            .map(|t| t.rendered.clone())
                            .unwrap_or_default();
                    let d = Diagnostic::error(
                        Code(301),
                        format!(
                            "call to `{}` is missing argument {} `{pname}`",
                            short_name(name),
                            i + 1 + display_offset
                        ),
                    )
                    .with_label(Label::primary(
                        call_span.clone(),
                        format!("`{pname}` gets nil here"),
                    ));
                    let d = cite_decl(
                        d,
                        local_decl,
                        &p.span,
                        &format!("`{pname}: {rendered}` declared here"),
                        &decl_note,
                    )
                    .with_note(format!(
                        "Luau fills missing arguments with nil, and `{rendered}` does not \
                         allow nil — the failure surfaces inside `{}`, blaming the wrong code",
                        short_name(name)
                    ))
                    .with_help(format!("pass a `{rendered}` for `{pname}`"));
                    self.diags.push(d);
                    return;
                }
            }
        }
        // Extra: more args than params, non-variadic callee. A plain name
        // was decided by `plain_extra_args` above. For a table slot an open
        // tail keeps its silence: `T.new(...)` handing its varargs on to a
        // function that takes none is how forwarding wrappers are written.
        let plain_name = matches!(key, Key::Local(_));
        if !plain_name && !open_tail && args.len() > eff_params.len() && !sig.has_varargs {
            let name = &sig.name;
            let d = extra_args(
                format!(
                    "call to `{}` has {} arguments, but `{}` takes {}",
                    short_name(name),
                    args.len(),
                    short_name(name),
                    eff_params.len()
                ),
                eff_params.len(),
                arg_spans,
                call_span,
            );
            let d = cite_decl(
                d,
                local_decl,
                &sig.decl_span,
                "parameters declared here",
                &decl_note,
            );
            self.diags.push(extra_args_notes(d, false));
            return;
        }

        // --- R0302: argument primitive mismatch ------------------------------
        for (i, (arg, p)) in args.iter().zip(eff_params).enumerate() {
            let Some(ty) = &p.ty else { continue };
            let Some(want) = ty.prim else { continue };
            let Some(got) = literal_prim(arg) else {
                continue;
            };
            if got == want {
                continue;
            }
            if got == hir::Prim::Nil && ty.admits_nil {
                continue;
            }
            let name = &sig.name;
            let pname = &p.name;
            let span = arg_spans
                .get(i)
                .cloned()
                .unwrap_or_else(|| call_span.clone());
            let d = Diagnostic::error(
                Code(302),
                format!(
                    "argument {} of `{}` expects `{}`, got `{}`",
                    i + 1,
                    short_name(name),
                    ty.rendered,
                    got.name()
                ),
            )
            .with_label(Label::primary(span, format!("this is a {}", got.name())));
            let d = cite_decl(
                d,
                local_decl,
                &p.span,
                &format!("`{pname}: {}` declared here", ty.rendered),
                &decl_note,
            )
            .with_help(match (got, want) {
                (hir::Prim::String, hir::Prim::Number) => {
                    format!("convert with `tonumber(...)` before passing to `{pname}`")
                }
                (hir::Prim::Number, hir::Prim::String) => {
                    format!("convert with `tostring(...)` before passing to `{pname}`")
                }
                _ => format!("pass a `{}`", ty.rendered),
            });
            self.diags.push(d);
            return; // one call site, one root cause
        }
    }
}

impl Checker<'_> {
    /// R0304 for `name(args)`: the variable `name` reaches here takes fewer
    /// arguments than the call supplies, whichever of its definitions the
    /// call finds at run time. Returns whether it reported.
    fn plain_extra_args(
        &mut self,
        name: &str,
        name_span: &Span,
        args: &[hir::Expr],
        call_span: &Span,
        arg_spans: &[Span],
    ) -> bool {
        let names = self.names;
        let Some(var) = names.var(name, name_span) else {
            return false;
        };
        // (parameters, where it is written)
        let mut definitions: Vec<(usize, &Span)> = Vec::new();
        for write in &var.writes {
            match write.stored {
                scope::Stored::Function(id) => {
                    let body = self.module.body(id);
                    if body.has_varargs {
                        return false;
                    }
                    definitions.push((body.params.len(), &write.span));
                }
                // A call that reaches nil throws; it discards nothing.
                scope::Stored::Nil => {}
                scope::Stored::Other => return false,
            }
        }
        let Some(takes) = definitions.iter().map(|(params, _)| *params).max() else {
            return false;
        };
        if args.len() <= takes {
            return false;
        }
        let mut d = if let [(_, span)] = definitions.as_slice() {
            extra_args(
                format!(
                    "call to `{name}` has {} arguments, but `{name}` takes {takes}",
                    args.len()
                ),
                takes,
                arg_spans,
                call_span,
            )
            .with_label(Label::secondary((*span).clone(), "parameters declared here"))
        } else {
            let mut d = extra_args(
                format!(
                    "call to `{name}` has {} arguments, but no definition of `{name}` \
                     takes more than {takes}",
                    args.len()
                ),
                takes,
                arg_spans,
                call_span,
            );
            for (params, span) in &definitions {
                d = d.with_label(Label::secondary(
                    (*span).clone(),
                    format!("this definition takes {params}"),
                ));
            }
            d
        };
        let open_tail = matches!(
            args.last(),
            Some(hir::Expr::Call { .. }) | Some(hir::Expr::Leaf)
        );
        d = extra_args_notes(d, open_tail);
        self.diags.push(d);
        true
    }
}

/// The R0304 diagnostic up to its primary label: the first argument past
/// the `takes` the callee declares. That argument can sit lines below
/// where the call starts, so the call's start is where an allow comment
/// may be put as well.
fn extra_args(message: String, takes: usize, arg_spans: &[Span], call_span: &Span) -> Diagnostic {
    let extra_span = arg_spans
        .get(takes)
        .cloned()
        .unwrap_or_else(|| call_span.clone());
    Diagnostic::error(Code(304), message)
        .with_label(Label::primary(
            extra_span,
            "this argument is silently discarded",
        ))
        .with_allow_anchor(call_span.start)
}

fn extra_args_notes(d: Diagnostic, open_tail: bool) -> Diagnostic {
    let d = d.with_note(
        "Luau discards extra arguments without a sound — usually a call site \
         that predates a signature change",
    );
    let d = if open_tail {
        d.with_note(
            "the last argument can expand to several values, and none of them is received",
        )
    } else {
        d
    };
    d.with_help("remove the extra argument(s), or add parameters")
}

/// One function written to a one-level table slot: `function T.f`,
/// `function T:m`, `T.f = function() end`.
struct SlotFunction<'a> {
    table: &'a str,
    /// The site that names the table, as `crate::scope` keys it: the whole
    /// `function T.f() end` statement, or the `T` of `T.f = ...`.
    site: &'a Span,
    field: &'a str,
}

/// Every function written to a table slot anywhere in the module. Nothing
/// else is a write here: `T.f = nil`, `T.f = other` and `T.f += 1` store no
/// function this pass can read a signature from.
fn slot_functions(module: &hir::Module) -> Vec<SlotFunction<'_>> {
    let mut out = Vec::new();
    for body in &module.bodies {
        scope::each_stmt(&body.block, &mut |stmt| match &stmt.kind {
            hir::StmtKind::FunctionDecl { root, path, .. } => {
                if let [field] = path.as_slice() {
                    out.push(SlotFunction { table: root, site: &stmt.span, field });
                }
            }
            hir::StmtKind::Assign { targets, values } => {
                for (i, t) in targets.iter().enumerate() {
                    let (
                        hir::Target::Projection(hir::Expr::Index {
                            base,
                            key: None,
                            name: Some(field),
                            ..
                        }),
                        Some(hir::Expr::Function(_)),
                    ) = (t, values.get(i))
                    else {
                        continue;
                    };
                    if let hir::Expr::Name { name, span } = &**base {
                        out.push(SlotFunction { table: name, site: span, field });
                    }
                }
            }
            _ => {}
        });
    }
    out
}

/// Whether some `name.field` is given two functions, going by the table's
/// name alone. False means no slot is, whichever table each name reaches.
fn any_slot_named_twice(slots: &[SlotFunction<'_>]) -> bool {
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    slots.iter().any(|s| !seen.insert((s.table, s.field)))
}

/// The slots given more than one function, each as the table variable its
/// name reaches and the field.
fn rewritten_slots(
    slots: &[SlotFunction<'_>],
    names: &scope::Resolution,
) -> HashSet<(scope::Binding, String)> {
    if !any_slot_named_twice(slots) {
        return HashSet::new();
    }
    let mut functions: HashMap<(scope::Binding, &str), u32> = HashMap::new();
    for s in slots {
        *functions
            .entry((names.binding(s.table, s.site), s.field))
            .or_default() += 1;
    }
    functions
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|((table, field), _)| (table, field.to_string()))
        .collect()
}

/// The primitive of an argument expression, when knowable at a glance:
/// literals only this milestone (annotated locals need per-call scope types,
/// which arrive with the narrowing machinery).
fn literal_prim(e: &hir::Expr) -> Option<hir::Prim> {
    match e {
        hir::Expr::Literal { prim, .. } => Some(*prim),
        _ => None,
    }
}

fn short_name(name: &str) -> &str {
    name
}

/// Point at the declaration: a secondary label when it is in this file, a
/// "declared in <file>:<line>" note when it is in another one (its spans
/// cannot label the caller's source).
fn cite_decl(
    d: Diagnostic,
    local_decl: bool,
    span: &Span,
    label: &str,
    decl_note: &Option<String>,
) -> Diagnostic {
    if local_decl {
        d.with_label(Label::secondary(span.clone(), label.to_string()))
    } else if let Some(n) = decl_note {
        d.with_note(n.clone())
    } else {
        d
    }
}

fn child_blocks(kind: &hir::StmtKind) -> Vec<&hir::Block> {
    use hir::StmtKind as K;
    match kind {
        K::Do(b) | K::While { body: b, .. } | K::Repeat { body: b, .. } => vec![b],
        K::NumericFor { body, .. } | K::GenericFor { body, .. } => vec![body],
        K::If {
            then,
            else_ifs,
            or_else,
            ..
        } => {
            let mut v = vec![then];
            v.extend(else_ifs.iter().map(|(_, b)| b));
            v.extend(or_else.iter());
            v
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // R0309 anchors its label on `decl_span`; a span painting the whole
    // `function ... end` body (or even the whole header line) buries the
    // one token that matters. The declaration SITE is the member NAME.
    #[test]
    fn interface_function_spans_cover_header_only() {
        let src = "local M = {}\nfunction M.f(x: number)\n\tprint(x)\nend\n\
                   M.g = function()\n\tprint(1)\nend\nreturn table.freeze(M)\n";
        let module = rime_syntax::parse(src).unwrap();
        let iface = module_interface(&module, "M", "M.luau", src);
        for name in ["f", "g"] {
            let sp = &iface.functions[name].decl_span;
            assert_eq!(
                &src[sp.start..sp.end],
                name,
                "`{name}` span is not the name token"
            );
        }
    }

    /// `(line, code)` of every call-conformance diagnostic (R0301–R0304).
    fn call_diags(src: &str) -> Vec<(usize, u16)> {
        crate::check_source(src)
            .into_iter()
            .filter(|d| (301..=304).contains(&d.code.0))
            .map(|d| (src[..d.primary_span().start].matches('\n').count() + 1, d.code.0))
            .collect()
    }

    #[test]
    fn parameter_hides_module_level_function() {
        let src = "local function f(a)\n\treturn a\nend\n\
                   local function g(f)\n\treturn f(1, 2)\nend\n\
                   return { f, g }\n";
        assert_eq!(call_diags(src), vec![]);
    }

    #[test]
    fn enclosing_binding_hides_module_level_function_inside_a_closure() {
        // The closure body is its own arena entry; its scope chain still
        // runs through the function that created it.
        let param = "local function f(a)\n\treturn a\nend\n\
                     local function g(f)\n\treturn function()\n\t\treturn f(1, 2)\n\tend\nend\n\
                     return { f, g }\n";
        assert_eq!(call_diags(param), vec![]);
        let local = "local function f(a)\n\treturn a\nend\n\
                     local function g()\n\tlocal f = math.max\n\
                     \treturn function()\n\t\treturn f(1, 2)\n\tend\nend\n\
                     return { f, g }\n";
        assert_eq!(call_diags(local), vec![]);
        let loop_var = "local function f(a)\n\treturn a\nend\n\
                        local function g(list)\n\tfor _, f in list do\n\
                        \t\ttask.spawn(function()\n\t\t\tf(1, 2)\n\t\tend)\n\tend\nend\n\
                        return { f, g }\n";
        assert_eq!(call_diags(loop_var), vec![]);
    }

    #[test]
    fn parameter_hides_module_level_table() {
        // All four call rules resolve through the same name: `.`/`:`
        // confusion and missing arguments were wrong the same way.
        let src = "local T = {}\n\
                   function T.m(a)\n\treturn a\nend\n\
                   function T:n(a: number)\n\treturn a\nend\n\
                   local function g(T)\n\tT.m(1, 2)\n\tT:m(1)\n\tT.n(1)\n\tT:n()\nend\n\
                   return { T, g }\n";
        assert_eq!(call_diags(src), vec![]);
    }

    #[test]
    fn until_condition_sees_the_loop_body_locals() {
        let src = "local function f(a)\n\treturn a\nend\n\
                   local function g()\n\trepeat\n\t\tlocal f = math.max\n\tuntil f(1, 2)\nend\n\
                   return { f, g }\n";
        assert_eq!(call_diags(src), vec![]);
    }

    #[test]
    fn module_level_redeclaration_is_not_checked_against_either_signature() {
        // Which declaration a call reaches depends on its position, which
        // the name-keyed registry does not track — silence over guessing.
        let two_functions = "local function f(a, b)\n\treturn a + b\nend\n\
                             print(f(1, 2))\n\
                             local function f(a)\n\treturn a\nend\n\
                             print(f(1))\n";
        assert_eq!(call_diags(two_functions), vec![]);
        let rebound = "local function g(a)\n\treturn a\nend\n\
                       print(g(1))\n\
                       local g = math.max\n\
                       print(g(1, 2))\n";
        assert_eq!(call_diags(rebound), vec![]);
    }

    #[test]
    fn calls_through_the_module_level_binding_are_still_checked() {
        // Same call shapes, nothing hiding the callee: from a function with
        // an unrelated parameter, from a closure, from a method body.
        let src = "local function f(a)\n\treturn a\nend\n\
                   local T = {}\n\
                   function T.m(a)\n\treturn a\nend\n\
                   local function g(x)\n\treturn f(x, 2)\nend\n\
                   local function h()\n\treturn function()\n\t\treturn f(1, 2)\n\tend\nend\n\
                   function T:k(x)\n\treturn f(x, 2), T.m(x, 2), T:m()\nend\n\
                   return { g, h, T }\n";
        assert_eq!(call_diags(src), vec![(9, 304), (13, 304), (17, 304), (17, 304), (17, 303)]);
    }

    // --- R0304 as Selene's `mismatched_arg_count` ---------------------------

    /// Line of every R0304, in source order.
    fn extra(src: &str) -> Vec<usize> {
        call_diags(src)
            .into_iter()
            .filter(|(_, code)| *code == 304)
            .map(|(line, _)| line)
            .collect()
    }

    fn extra_diags(src: &str) -> Vec<Diagnostic> {
        crate::check_source(src)
            .into_iter()
            .filter(|d| d.code.0 == 304)
            .collect()
    }

    // Selene's own fixtures (selene-lib/tests/lints/mismatched_arg_count),
    // each with the lines its .stderr reports. Rime reports the same lines.
    #[test]
    fn selene_fixtures_report_the_same_lines() {
        let fixtures: [(&str, &str, &[usize]); 9] = [
            (
                "alternative_function_definition",
                "function foo(a, b)\nend\n\nfoo(1)\nfoo(1, 2)\nfoo(1, 2, 3)\n\n\
                 local bar = function(a, b)\nend\nbar(1)\nbar(1, 2)\n\n\
                 baz = function(a, b, c)\nend\nbaz(1)\nbaz(1, 2)\n",
                &[6],
            ),
            (
                "call_side_effects",
                "local function foo(a, b, c)\nend\n\nfoo(a, call())\nfoo(a, call(), c)\n\
                 foo(a, b, call())\nfoo(a, b, c, call())\n\n\
                 local function bar(...)\n\tfoo(1, 2, 3, ...)\nend\n",
                &[7, 10],
            ),
            (
                "definition_location",
                "local dispatch\n\nlocal function setDispatch()\n    dispatch = function() end\nend\n\n\
                 dispatch(\"foo\", \"bar\")\n",
                &[7],
            ),
            (
                "mismatched_arg_count",
                "local function foo(a, b, c)\nend\n\nfoo(\"a\", \"b\")\nfoo(\"a\", \"b\", \"c\")\n\
                 foo(\"a\", \"b\", \"c\", \"d\")\n",
                &[6],
            ),
            (
                "multiple_definition_locations",
                "local dispatch\n\nif math.random() > 0.5 then\n    dispatch = function() end\nelse\n    \
                 dispatch = function(a) end\nend\n\ndispatch()\ndispatch(1)\ndispatch(1, 2)\n\n\
                 local hasVararg\n\nif math.random() > 0.5 then\n    hasVararg = function(a, b, ...) end\n\
                 else\n    hasVararg = function(a, b, c, d) end\nend\n\nhasVararg(1)\n\
                 hasVararg(1, 2, 3, 4, 5)\n",
                &[11],
            ),
            (
                "reassigned_variables",
                "local function foo(a, b, c)\nend\n\nfoo(1, 2, 3)\nfoo(1, 2, 3, 4)\n\n\
                 foo = function(p, q, r, s)\nend\n\nfoo(1, 2, 3, 4)\n\ndo\n\
                 \t-- still lint a local function\n\tlocal function foo(l, m)\n\tend\n\n\
                 \tfoo(1, 2)\n\tfoo(1, 2, 3)\nend\n\nfoo(1, 2, 3, 4)\n\n\
                 foo = function(d)\nend\n\nfoo(1, 2, 3)\n",
                &[18],
            ),
            (
                "reassigned_variables_2",
                "local function foo(a, b, c)\n\tprint(\"hello from a\")\nend\n\n\
                 function updateFoo(a, b, c, d)\n\tfoo = function()\n\t\tprint(\"UPDATED FOO\")\n\tend\nend\n\n\
                 foo(1, 2, 3, 4)\nupdateFoo()\nfoo(1, 2, 3, 4)\n",
                &[11, 13],
            ),
            (
                "shadowing_variables",
                "function foo(a, b, c)\nend\n\nfoo(1, 2, 3)\n\nfunction foo(a, b)\nend\n\nfoo(1, 2)\n\n\
                 local function bar(a)\nend\n\nbar(1)\nbar(3, 4)\n\n\
                 local function bar(a, b)\nend\n\nbar(1, 2)\n",
                &[15],
            ),
            (
                "variable_function_def",
                "local function foo(...)\nend\n\nfoo()\nfoo(1)\nfoo(1, 2, 3, 4, 5)\n\n\
                 local function bar(a, b, ...)\nend\n\nbar()\nbar(1, 2)\nbar(1, 2, 3)\nbar(1, 2, 3, 4, 5)\n",
                &[],
            ),
        ];
        for (name, src, want) in fixtures {
            assert_eq!(extra(src), want, "{name}");
        }
    }

    // One file run through Selene (std = "lua53+roblox") and
    // through Rime.
    //
    // Selene reports 3 6 9 12 14 17 18 19 27 30 32 34 43 44 46 49 51 52 56.
    // Rime differs on seven lines, every one deliberate:
    //   * 3, 6, 9, 12: the variable is also given something that is not a
    //     function written in place (a `require`, `bar or print`, a number,
    //     `+=`). Selene ignores those writes and checks the call against
    //     the function it did see. Rime does not know what the variable
    //     holds at the call, and is silent.
    //   * 16: Selene does not look at a parenthesized callee. `(f)(1, 2)`
    //     calls the same `f`.
    //   * 22, 54: Selene checks plain names only. Rime also checks a call
    //     one step off a table (`t.f(...)`, `t:m(...)`).
    #[test]
    fn selene_and_rime_on_one_file() {
        let src = "\
local function foo(a) end
foo = require(script.Other)
foo(1, 2)
local bar = function(a) end
bar = bar or print
bar(1, 2)
local function baz(a) end
baz, qux = 1, 2
baz(1, 2)
local function quux(a) end
quux += 1
quux(1, 2)
function g(a) end
g(1, 2)
local function f() end;
(f)(1, 2)
f \"x\"
f {}
f(1, 2)(3)
local t = {}
function t.f(a) end
t.f(1, 2)
local fwd
fwd(1)
local late = nil
late = function(a) end
late(1, 2)
local function outer()
	local function inner(a) end
	inner(1, 2)
end
local function r(a) r(1, 2) end
local helper
local function use() helper(1, 2) end
helper = function(a) end
local function v(a, ...) end
v = function(a) end
v(1, 2)
local function p(a) end
local function q(p) p(1, 2) end
for _, p in pairs({}) do p(1, 2) end
local function m(a) end
m(1, (m()))
m(1, ...)
m(m())
m(1, m())
local w = function(a) end
w = nil
w(1, 2)
local x1, x2 = function(a) end, function() end
x1(1, 2)
x2(1)
function t:meth(a) end
t:meth(1, 2)
local function sf(self, a) end
sf(1, 2, 3)
";
        assert_eq!(
            extra(src),
            vec![14, 16, 17, 18, 19, 22, 27, 30, 32, 34, 43, 44, 46, 49, 51, 52, 54, 56]
        );
    }

    #[test]
    fn one_definition_keeps_the_existing_wording() {
        let src = "local function greet(name)\n\tprint(name)\nend\ngreet(\"a\", \"b\")\n";
        let got = extra_diags(src);
        assert_eq!(got.len(), 1, "{got:?}");
        let d = &got[0];
        assert_eq!(d.message, "call to `greet` has 2 arguments, but `greet` takes 1");
        assert!(!d.conservative);
        assert_eq!(&src[d.primary_span()], "\"b\"");
        let secondary: Vec<&str> = d
            .labels
            .iter()
            .filter(|l| !l.primary)
            .map(|l| &src[l.span.clone()])
            .collect();
        assert_eq!(secondary, vec!["greet"]);
    }

    #[test]
    fn several_definitions_are_all_pointed_at() {
        let src = "\
local dispatch
if math.random() > 0.5 then
	dispatch = function() end
else
	dispatch = function(a) end
end
dispatch(1, 2)
";
        let got = extra_diags(src);
        assert_eq!(got.len(), 1, "{got:?}");
        let d = &got[0];
        assert_eq!(
            d.message,
            "call to `dispatch` has 2 arguments, but no definition of `dispatch` takes more than 1"
        );
        assert_eq!(&src[d.primary_span()], "2");
        let mut secondary: Vec<(usize, &str)> = d
            .labels
            .iter()
            .filter(|l| !l.primary)
            .map(|l| (src[..l.span.start].matches('\n').count() + 1, l.message.as_str()))
            .collect();
        secondary.sort();
        assert_eq!(
            secondary,
            vec![(3, "this definition takes 0"), (5, "this definition takes 1")]
        );
    }

    // `f(a, g())`: the count is a lower bound, since `g()` can return any
    // number of values. Past the last parameter it does not matter how
    // many: none is received. As in Selene, and like it for plain names
    // only: one step off a table an open tail is left alone, as before
    // (`BaseOcclusion.new(...)` in Roblox's own PlayerModule).
    #[test]
    fn a_call_or_vararg_past_the_last_parameter_is_reported() {
        let src = "\
local function f(a, b) end
local T = {}
function T.g(a) end
local function h(...)
	f(1, ...)
	f(1, 2, ...)
	f(1, 2, h())
	T.g(h())
	T.g(1, h())
	T.g(1, ...)
end
return T
";
        assert_eq!(extra(src), vec![6, 7]);
        let d = &extra_diags(src)[0];
        assert!(
            d.notes.iter().any(|n| n.contains("can expand to several values")),
            "{:?}",
            d.notes
        );
    }

    // An allow comment above a call written over several lines. The report
    // points at the first extra argument, lines below; the comment sits
    // where the call starts.
    #[test]
    fn an_allow_above_a_call_written_over_several_lines_silences_it() {
        let plain = "\
local function one(a) return a end
local long1, long2 = 1, 2
-- rime: allow(call-extra-args, \"kept for the old callers\")
one(
	long1,
	long2
)
";
        assert_eq!(extra(plain), Vec::<usize>::new());
        let field = "\
local T = {}
function T.one(a) return a end
function T:two(a) return a end
-- rime: allow(R0304, \"kept for the old callers\")
local got = T.one(
	1,
	2
)
T:two( -- rime: allow(call-extra-args, \"trailing, on the line the call starts\")
	1,
	2
)
return got
";
        assert_eq!(extra(field), Vec::<usize>::new());
        // Unchanged: the comment on the line above the argument itself.
        let inside = "\
local function one(a) return a end
one(
	1,
	-- rime: allow(call-extra-args, \"x\")
	2
)
";
        assert_eq!(extra(inside), Vec::<usize>::new());
    }

    // How far the comment reaches: the call that starts on its target
    // line, and nothing past that.
    #[test]
    fn an_allow_reaches_the_call_that_starts_on_its_line_and_no_further() {
        // A call that starts on a later line of the same statement.
        let nested = "\
local function one(a) return a end
local function wrap(a) return a end
-- rime: allow(call-extra-args, \"x\")
local got = wrap(
	one(
		1,
		2
	)
)
return got
";
        assert_eq!(extra(nested), vec![7]);
        // The next statement, and a comment one line too high.
        let next = "\
local function one(a) return a end
-- rime: allow(call-extra-args, \"x\")
one(1)
one(
	1,
	2
)
-- rime: allow(call-extra-args, \"x\")

one(
	1,
	2
)
";
        assert_eq!(extra(next), vec![6, 12]);
        // Another rule's comment, and a comment without a reason.
        let other = "\
local function one(a) return a end
-- rime: allow(shadowing, \"x\")
one(
	1,
	2
)
-- rime: allow(call-extra-args)
one(
	1,
	2
)
";
        assert_eq!(extra(other), vec![5, 10]);
        // The other call rules keep the line of their own label.
        let missing = "\
local function two(a: number, b: number) return a + b end
-- rime: allow(call-extra-args, \"x\")
two(
	\"a\",
	2
)
";
        assert_eq!(call_diags(missing), vec![(4, 302)]);
    }

    // One call, one report: R0302 would also fire on the string.
    #[test]
    fn an_extra_argument_is_the_only_report_on_its_call() {
        let src = "local function f(a: number) end\nf(\"x\", 2)\n";
        assert_eq!(call_diags(src), vec![(2, 304)]);
    }

    #[test]
    fn missing_arguments_on_plain_calls_are_untouched() {
        let src = "local function f(a: number, b: number) end\nf(1)\n";
        assert_eq!(call_diags(src), vec![(2, 301)]);
    }

    // `M.f = function() end` as a stand-in, with the real function assigned
    // later. Which one a call reaches depends on when it runs, so no call
    // rule checks the slot.
    #[test]
    fn a_table_slot_given_two_functions_is_not_checked() {
        let src = "\
local M = {}
M.report = function() end
function M.verbose()
	M.report = function(line: string, level: number)
		print(level, line)
	end
end
M.report(\"ready\", 2)
return M
";
        assert_eq!(call_diags(src), vec![]);
        let once = "local M = {}\nM.report = function() end\nM.report(\"ready\", 2)\nreturn M\n";
        assert_eq!(call_diags(once), vec![(3, 304)]);
    }

    // A method declared twice is two functions in one slot as well.
    #[test]
    fn a_method_declared_twice_is_not_checked() {
        let src = "\
local M = {}
function M:connect(host: string) return host end
function M:connect(host: string, port: number) return host, port end
M:connect()
M.connect(1)
return M
";
        assert_eq!(call_diags(src), vec![]);
    }

    // Only a function counts. `M.f = nil` takes the function away: a call
    // that reaches nil throws, it does not pick another signature.
    #[test]
    fn clearing_a_slot_does_not_switch_its_checks_off() {
        let src = "\
local M = {}
function M.send(player: string) return player end
function M:notify(player: string) return player end
function M.teardown()
	M.send = nil
	M.notify = nil
end
M.send(\"a\", 2)
M.send()
M.send(1)
M.notify(\"a\")
return M
";
        assert_eq!(call_diags(src), vec![(8, 304), (9, 301), (10, 302), (11, 303)]);
    }

    // A write that stores something other than a function written in
    // place does not count either: the slot still has one known function.
    #[test]
    fn a_slot_with_one_function_and_other_writes_is_still_checked() {
        let src = "\
local M = {}
function M.load(id) return id end
M.load = M.load
M.count = 0
M.count += 1
M.count = 2
M.load(1, 2)
return M
";
        assert_eq!(call_diags(src), vec![(7, 304)]);
    }

    // The slot belongs to one table. Another table that shares the name (a
    // local in another function, a parameter) has slots of its own.
    #[test]
    fn another_table_of_the_same_name_does_not_switch_the_checks_off() {
        let local = "\
local M = {}
local function scratch()
	local M = {}
	function M.load() end
	M.save = function() end
	return M
end
function M.load(id) return id end
M.save = function(id) return id end
M.load(1, 2)
M.save(1, 2)
return M, scratch
";
        assert_eq!(call_diags(local), vec![(10, 304), (11, 304)]);
        let param = "\
local M = {}
local function fill(M)
	M.load = function() end
end
function M.load(id) return id end
M.load(1, 2)
return M, fill
";
        assert_eq!(call_diags(param), vec![(6, 304)]);
        // The other way round: the inner table has two functions in the
        // slot, and calls through it are not checked. The outer one is.
        let inner = "\
local M = {}
function M.load(id) return id end
local function scratch()
	local M = {}
	M.load = function() end
	M.load = function(a, b) return a, b end
	M.load(1, 2)
	return M
end
M.load(1, 2)
return M, scratch
";
        assert_eq!(call_diags(inner), vec![(10, 304)]);
    }

    // A global table is one table wherever the file names it.
    #[test]
    fn a_global_table_slot_given_two_functions_is_not_checked() {
        let src = "\
Registry.find = function() end
local function init()
	Registry.find = function(name) return name end
end
Registry.find(\"a\")
return init
";
        assert_eq!(call_diags(src), vec![]);
        let once = "\
Registry.find = function() end
local function init(Registry)
	Registry.find = function(name) return name end
end
Registry.find(\"a\")
return init
";
        assert_eq!(call_diags(once), vec![(5, 304)]);
    }

    // The same slot seen from another file: it is a member, but not a
    // function with a known signature.
    #[test]
    fn a_table_slot_given_two_functions_is_exported_without_a_signature() {
        let src = "\
local M = {}
M.report = function() end
function M.verbose()
	M.report = function(line) print(line) end
end
function M.other(a) return a end
return M
";
        let module = rime_syntax::parse(src).unwrap();
        let iface = module_interface(&module, "M", "M.luau", src);
        assert!(!iface.functions.contains_key("report"));
        assert!(iface.fields.contains_key("report"));
        assert!(iface.functions.contains_key("other"));
        assert!(iface.functions.contains_key("verbose"));
    }

    // A cleared slot, and a same-named table elsewhere in the file, leave
    // the exported signature in place for other files to check against.
    #[test]
    fn a_cleared_slot_and_a_namesake_table_keep_the_exported_signature() {
        let src = "\
local M = {}
function M.send(player) return player end
function M.teardown()
	M.send = nil
end
function M.load(id) return id end
local function scratch()
	local M = {}
	function M.load() end
	return M
end
M.scratch = scratch
return M
";
        let module = rime_syntax::parse(src).unwrap();
        let iface = module_interface(&module, "M", "M.luau", src);
        assert!(iface.functions.contains_key("send"));
        assert!(iface.functions.contains_key("load"));
        // The export named through `table.freeze` is the same table.
        let frozen = src.replace("return M\n", "return table.freeze(M)\n");
        let module = rime_syntax::parse(&frozen).unwrap();
        let iface = module_interface(&module, "M", "M.luau", &frozen);
        assert!(iface.functions.contains_key("send"));
        assert!(iface.functions.contains_key("load"));
    }
}
