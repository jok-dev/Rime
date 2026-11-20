//! R0309: unused module export (decided 2026-08-22). Project mode only —
//! single-file checking cannot see consumers.
//!
//! An export member is UNUSED when the whole-project evidence is complete
//! and negative: the module's interface is closed (the extractor
//! saw every member and the table never escapes its file), at least one
//! file requires the module through a statically-resolved path, no consumer
//! reads the member, and the module never reads it off its own export.
//! Internal-only use counts as use (a "should be local" refinement is
//! future work, not this rule).
//!
//! Consumer-side evidence is collected per require binding by `collect_usage`
//! and is deliberately over-approximate in the silent direction:
//!
//!   - `Mod.x` / `Mod:m()` reads and calls mark `x`/`m` used; a WRITE
//!     `Mod.x = v` also marks `x` used (it references the member — R0308
//!     already owns the sealed-module complaint, and double noise helps
//!     nobody);
//!   - `Mod[k]`, a bare `Mod` in any non-index position (argument, alias,
//!     return, table value, direct call `Mod()`), or a reassignment of the
//!     binding POISONS the module: any member may be reached, every member
//!     counts as used;
//!   - shadowing is not modelled: a same-named local's member reads attribute
//!     to the module, which can only silence a real finding, never invent
//!     one — same one-way safety argument as the member-existence escape scan.
//!
//! The caller (project.rs pass 3) owns the remaining gates: linker stubs
//! (`return require(...)`) are skipped as defining sites and usage through
//! them is attributed to the terminal module; a module nobody requires is
//! silent (no consumers = no evidence — loaders and entry points never show
//! a require; a dead-module finding is queued future work); any file that
//! fails to parse disables the rule for the whole project (an unreadable
//! file could use anything).
//!
//! The rule runs in one of two modes (decided 2026-08-22, replacing the
//! original project-wide kill switch):
//!
//!   - STRICT — the require graph is fully static: per-binding attribution
//!     only, as above.
//!   - DEGRADED — any Dynamic resolution or stray require (a require call
//!     outside the top-level-binding and linker forms, `stray_require`)
//!     exists anywhere: those consumers' reads are unattributable, so the
//!     project-wide set of member NAMES read anywhere (`collect_read_names`)
//!     additionally counts as usage on EVERY module. Over-approximate in
//!     the silent direction.
//!     A string literal equal to a member's name (`collect_string_names`)
//!     is weaker evidence, and is treated as such: an unattributable
//!     consumer can look a member up by a name it took from a list of
//!     strings, but most strings that equal a member's name are something
//!     else (an event name, a status value). So the member is still
//!     reported, and the report is MARKED: a note names the places, and
//!     `Diagnostic::string_matches` lists them all for tools, which should
//!     leave a marked report to a person (user decision 2026-11-20). A
//!     string inside the member's own definition is left out
//!     (`own_body_strings`): one inside its function cannot be what starts
//!     that function, and a value that holds its own name is not a use.
//!     In STRICT mode a string proves nothing — reaching a member by name
//!     takes `Mod[k]` or an escaped module, and both already poison — so
//!     nothing is marked there.
//!     Why a mode and not a kill switch: a project whose loader requires
//!     every module dynamically and calls its lifecycle functions by name
//!     would get a warning on every export without a gate, or total
//!     silence with one; under degrade mode the loader's guarded reads
//!     silence the lifecycle names project-wide and the rule stays live
//!     for names nothing reads.
//!
//! OutsideView / Trusted resolutions trigger NEITHER mode change: they
//! prove the target is outside the project view, so reads through those
//! bindings cannot consume in-view exports.
//!
//! The degrade-mode FP hole, documented and accepted: a member reached
//! ONLY through a non-literal computed key (`m[k]()` dispatch) contributes
//! no name and would still be flagged — marked, if a string literal
//! somewhere in the project equals the name (a key built at run time
//! leaves none). Poisoning on computed keys is not an
//! option — array indexing is everywhere and would kill the rule; the
//! diagnostic stays `conservative` and says so.
//!
//! `_`-prefixed member names are exempt — same deliberate-keep contract as
//! R0013/14/15.

use rime_diag::{Code, Diagnostic, Label, Site, Span};
use rime_hir as hir;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::calls::ModuleInterface;

/// What one project's consumers did with a required module, aggregated
/// across every file that requires it. Entry existence alone means "required
/// at least once through a statically-resolved path".
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Usage {
    pub(crate) used: HashSet<String>,
    pub(crate) poisoned: bool,
}

/// Record `module`'s member usage of its require-bound externals into
/// `usage`. `bindings` maps each require-bound local to the TERMINAL file it
/// resolves to (linker chains already chased by the caller).
pub(crate) fn collect_usage(
    module: &hir::Module,
    bindings: &HashMap<String, PathBuf>,
    usage: &mut HashMap<PathBuf, Usage>,
) {
    if bindings.is_empty() {
        return;
    }
    for path in bindings.values() {
        usage.entry(path.clone()).or_default();
    }
    let mut scan = UsageScan { bindings, usage };
    for body in &module.bodies {
        hir::visit::blocks(body, &mut |block| scan.scan_block_shallow(block));
    }
}

struct UsageScan<'a> {
    bindings: &'a HashMap<String, PathBuf>,
    usage: &'a mut HashMap<PathBuf, Usage>,
}

impl<'a> UsageScan<'a> {
    fn mark_used(&mut self, binding: &str, member: &str) {
        if let Some(path) = self.bindings.get(binding) {
            self.usage.get_mut(path).unwrap().used.insert(member.to_string());
        }
    }

    fn poison(&mut self, binding: &str) {
        if let Some(path) = self.bindings.get(binding) {
            self.usage.get_mut(path).unwrap().poisoned = true;
        }
    }

    /// One block's own statements and terminator — nested blocks arrive via
    /// `visit::blocks`, nested closures via the per-body loop above.
    fn scan_block_shallow(&mut self, block: &hir::Block) {
        use hir::StmtKind as K;
        for stmt in &block.stmts {
            match &stmt.kind {
                K::Local { init, .. } => {
                    for e in init {
                        self.scan_expr(e);
                    }
                }
                K::Assign { targets, values } => {
                    for t in targets {
                        self.scan_target(t);
                    }
                    for e in values {
                        self.scan_expr(e);
                    }
                }
                K::CompoundAssign { target, value } => {
                    self.scan_target(target);
                    self.scan_expr(value);
                }
                K::Call(e) => self.scan_expr(e),
                K::If { cond, else_ifs, .. } => {
                    self.scan_expr(cond);
                    for (c, _) in else_ifs {
                        self.scan_expr(c);
                    }
                }
                K::While { cond, .. } => self.scan_expr(cond),
                K::Repeat { until, .. } => self.scan_expr(until),
                K::NumericFor { start, end, step, .. } => {
                    self.scan_expr(start);
                    self.scan_expr(end);
                    if let Some(s) = step {
                        self.scan_expr(s);
                    }
                }
                K::GenericFor { exprs, .. } => {
                    for e in exprs {
                        self.scan_expr(e);
                    }
                }
                K::Do(_) | K::LocalFunction { .. } | K::FunctionDecl { .. } => {}
            }
        }
        if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
            for e in exprs {
                self.scan_expr(e);
            }
        }
    }

    fn scan_target(&mut self, t: &hir::Target) {
        match t {
            // `Mod = ...` — the binding no longer provably holds the module.
            hir::Target::Name { name, .. } => self.poison(name),
            hir::Target::Projection(e) => match e {
                hir::Expr::Index { base, key, name, .. }
                    if matches!(&**base, hir::Expr::Name { name: b, .. }
                        if self.bindings.contains_key(b)) =>
                {
                    let hir::Expr::Name { name: b, .. } = &**base else { unreachable!() };
                    let b = b.clone();
                    match (key, name) {
                        // `Mod.x = v` — references the member; R0308 owns
                        // the sealed-module complaint.
                        (None, Some(f)) => self.mark_used(&b, f),
                        // `Mod[k] = v` — any member may be written.
                        _ => self.poison(&b),
                    }
                    if let Some(k) = key {
                        self.scan_expr(k);
                    }
                }
                other => self.scan_expr(other),
            },
        }
    }

    fn scan_expr(&mut self, e: &hir::Expr) {
        use hir::Expr as E;
        match e {
            // Bare binding anywhere a member projection did not consume it:
            // escaped — any member may be reached through the alias.
            E::Name { name, .. } => {
                if self.bindings.contains_key(name) {
                    self.poison(name);
                }
            }
            E::Index { base, key, name, .. } => {
                if let E::Name { name: b, .. } = &**base {
                    if self.bindings.contains_key(b) {
                        let b = b.clone();
                        match (key, name) {
                            (None, Some(f)) => self.mark_used(&b, f),
                            _ => self.poison(&b),
                        }
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
                    // `Mod:m(...)` — a use of member `m`.
                    (E::Name { name: b, .. }, Some(m))
                        if self.bindings.contains_key(b.as_str()) =>
                    {
                        let b = b.clone();
                        self.mark_used(&b, m);
                    }
                    // `Mod(...)` — the module used as a callable; whole-table
                    // use. (A dot-callee `Mod.f(...)` lands in the Index arm.)
                    (E::Name { name: b, .. }, None)
                        if self.bindings.contains_key(b.as_str()) =>
                    {
                        let b = b.clone();
                        self.poison(&b);
                    }
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

/// Collect every member NAME `module` reads, regardless of what it reads it
/// off — the degrade-mode evidence pool. Included: `x.M` reads, `x:M()`
/// method calls, `x.M = v` writes, and literal-string keys `x["M"]`.
/// Excluded: table construction (`{ M = 1 }` defines, it does not read),
/// computed keys (`x[k]` — unattributable; the documented degrade-mode FP
/// hole, see the module doc), and the module's own defining writes:
/// `M.NAME = v` where `M` is the table the module returns. Such a write is
/// how the member comes to exist — counted as a read, every member defined
/// that way read itself, and none of them could be flagged in degrade mode.
/// `M.NAME += v` reads the slot and still counts. A write through any other
/// name (`self.x = v`, a consumer's `Mod.x = v`) still counts too:
/// narrowing that further is a separate decision.
pub(crate) fn collect_read_names(module: &hir::Module, out: &mut HashSet<String>) {
    let exported = super::calls::exported_name(module);
    for body in &module.bodies {
        // By expression identity, as `stray_require` does: the target of a
        // plain assignment is the same node the expression walk yields.
        let mut defining: HashSet<*const hir::Expr> = HashSet::new();
        hir::visit::stmts(body, &mut |stmt| match &stmt.kind {
            hir::StmtKind::Assign { targets, .. } => {
                let Some(exported) = &exported else { return };
                for t in targets {
                    let hir::Target::Projection(e) = t else { continue };
                    if let hir::Expr::Index { base, key: None, name: Some(_), .. } = e {
                        if matches!(&**base, hir::Expr::Name { name, .. } if name == exported) {
                            defining.insert(e as *const _);
                        }
                    }
                }
            }
            // `function a.b.c()` stores `c` into the table `a.b` holds: it
            // reads `b`, as the deep write `a.b.c = function() end` does.
            // The last segment is the name being defined, not a read.
            hir::StmtKind::FunctionDecl { path, .. } => {
                if let [reads @ .., _] = path.as_slice() {
                    out.extend(reads.iter().cloned());
                }
            }
            _ => {}
        });
        hir::visit::exprs(body, &mut |e| match e {
            hir::Expr::Index { key, name, .. } => match (key, name) {
                (None, Some(_)) if defining.contains(&(e as *const _)) => {}
                (None, Some(n)) => {
                    out.insert(n.clone());
                }
                (Some(k), _) => {
                    if let hir::Expr::Literal {
                        prim: hir::Prim::String,
                        text: Some(t),
                        ..
                    } = &**k
                    {
                        out.insert(t.clone());
                    }
                }
                _ => {}
            },
            hir::Expr::Call { method: Some(m), .. } => {
                out.insert(m.clone());
            }
            _ => {}
        });
    }
}

/// Collect every string literal in `module` that is shaped like a member
/// name — the second, weaker half of the degrade-mode evidence pool. A
/// consumer reached through a dynamic require can look a member up by name
/// (`m[name]`, with `name` taken from a list of strings), and that lookup
/// contributes nothing to `collect_read_names`: the string is the only
/// trace the name leaves. So a string EQUAL to a member's name marks the
/// member's report, wherever in the project it is written.
///
/// Only whole plain literals are seen. A name built at run time
/// (`"get" .. kind`, a backtick string with `{}` parts) leaves no literal
/// equal to it and stays in the documented degrade-mode hole.
///
/// `out` holds the byte offset of every such literal, in source order:
/// the report names the places, and `check_unused` leaves out the ones
/// written inside the flagged function itself (`own_body_strings`).
pub(crate) fn collect_string_names(module: &hir::Module, out: &mut HashMap<String, Vec<usize>>) {
    for body in &module.bodies {
        hir::visit::exprs(body, &mut |e| {
            if let hir::Expr::Literal { prim: hir::Prim::String, text: Some(t), span } = e {
                if is_member_name(t) {
                    match out.get_mut(t) {
                        Some(at) => at.push(span.start),
                        None => {
                            out.insert(t.clone(), vec![span.start]);
                        }
                    }
                }
            }
        });
    }
    for at in out.values_mut() {
        at.sort_unstable();
    }
}

/// The byte offset of every string literal equal to `name` that is written
/// inside `module`'s own definition of its export member `name`: the
/// function(s) it defines the member as, closures nested in them included,
/// or the value it gives the member.
///
/// Those strings are no evidence that the member is used. For one inside
/// a function to reach a lookup, the function has to be running already,
/// and a function nothing else names is never started: a function that
/// logs its own name (`log("save", ...)` inside `M.save`) does not use
/// itself. A value that holds its own name (`M.Opened = "Opened"`) is the
/// definition, not a use of it. The same string ANYWHERE ELSE still
/// counts — a live `A.save` that forwards by name (`backend[name]`) is a
/// real use of a `save` defined elsewhere, which is why these are left
/// out per flagged member and not dropped from the pool.
fn own_body_strings(module: &hir::Module, name: &str) -> HashSet<usize> {
    let mut own = HashSet::new();
    let Some(exported) = super::calls::exported_name(module) else { return own };
    let is_member = |t: &hir::Target| {
        matches!(t, hir::Target::Projection(hir::Expr::Index { base, key: None, name: Some(n), .. })
            if n == name
                && matches!(&**base, hir::Expr::Name { name: b, .. } if *b == exported))
    };
    // The function bodies still to read; a value is read on the spot.
    let mut defining: Vec<hir::BodyId> = Vec::new();
    let mut read = |e: &hir::Expr, defining: &mut Vec<hir::BodyId>| match e {
        hir::Expr::Literal { prim: hir::Prim::String, text: Some(t), span } if t == name => {
            own.insert(span.start);
        }
        hir::Expr::Function(inner) => defining.push(*inner),
        _ => {}
    };
    for body in &module.bodies {
        hir::visit::stmts(body, &mut |stmt| match &stmt.kind {
            hir::StmtKind::FunctionDecl { root, path, body, .. } => {
                if *root == exported && matches!(path.as_slice(), [m] if m == name) {
                    defining.push(*body);
                }
            }
            hir::StmtKind::Assign { targets, values } => {
                for (_, v) in targets.iter().zip(values).filter(|(t, _)| is_member(t)) {
                    hir::visit::expr(v, &mut |e| read(e, &mut defining));
                }
            }
            hir::StmtKind::Local { decls, init } => {
                for (d, v) in decls.iter().zip(init) {
                    let hir::Expr::Table(fields) = v else { continue };
                    if d.name != exported {
                        continue;
                    }
                    for f in fields {
                        match f {
                            hir::TableField::Named { name: n, value, .. } if n == name => {
                                hir::visit::expr(value, &mut |e| read(e, &mut defining));
                            }
                            // `["name"] = value`: the key is such a string too.
                            hir::TableField::Keyed { key, value } => {
                                if matches!(key, hir::Expr::Literal { prim: hir::Prim::String, text: Some(t), .. } if t == name) {
                                    read(key, &mut defining);
                                    hir::visit::expr(value, &mut |e| read(e, &mut defining));
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        });
    }
    let mut seen: HashSet<hir::BodyId> = HashSet::new();
    while let Some(id) = defining.pop() {
        if !seen.insert(id) {
            continue;
        }
        let body = module.body(id);
        hir::visit::exprs(body, &mut |e| read(e, &mut defining));
        hir::visit::stmts(body, &mut |stmt| {
            if let hir::StmtKind::LocalFunction { body, .. }
            | hir::StmtKind::FunctionDecl { body, .. } = &stmt.kind
            {
                defining.push(*body);
            }
        });
    }
    own
}

/// The degrade-mode evidence pool, gathered from every file in the project.
#[derive(Debug, Default)]
pub(crate) struct GlobalNames {
    /// Member names read anywhere (`collect_read_names`). A member whose
    /// name is here counts as used.
    pub(crate) reads: HashSet<String>,
    /// Every string literal equal to each name (`collect_string_names`),
    /// as (file, byte offset). The caller numbers the files; they come in
    /// that order, and by offset within a file. A member whose name is
    /// here is still reported, with these places attached.
    pub(crate) strings: HashMap<String, Vec<(usize, usize)>>,
}

/// What `check_unused` needs to know in degrade mode, for one defining file.
pub(crate) struct Degraded<'a> {
    pub(crate) names: &'a GlobalNames,
    /// The defining file's number in `GlobalNames::strings`.
    pub(crate) file: usize,
    /// The path and line of (file, byte offset).
    pub(crate) locate: &'a dyn Fn(usize, usize) -> Site,
}

/// Where a string equal to `module`'s export member `name` is written,
/// anywhere in the project except inside the member's own function. One
/// entry per line: two such strings on a line are one place to look.
fn string_matches(degraded: &Degraded<'_>, module: &hir::Module, name: &str) -> Vec<Site> {
    let Some(sites) = degraded.names.strings.get(name) else { return Vec::new() };
    let own = own_body_strings(module, name);
    let mut found: Vec<Site> = sites
        .iter()
        .filter(|(file, offset)| !(*file == degraded.file && own.contains(offset)))
        .map(|(file, offset)| (degraded.locate)(*file, *offset))
        .collect();
    // Sites arrive by file, then by offset, so equal lines are neighbours.
    found.dedup();
    found
}

/// A string no member can be named by is not worth keeping.
fn is_member_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// True when `module` contains a require call R0309 cannot attribute: any
/// require that is not a top-level `local X = require(...)` binding or the
/// root `return require(...)` linker form. Such a call (the loader idiom —
/// require inside a function or loop, member reads on the returned value)
/// is a consumer whose reads usage collection never sees, so its existence
/// anywhere in the project puts R0309 in degrade mode. The check is by
/// expression identity against the whitelisted positions. A call through a
/// rebound `require` is not a require at all here; the session degrades for
/// those itself (`RequireNote::Rebound`).
pub(crate) fn stray_require(module: &hir::Module) -> bool {
    let root = module.body(module.root);
    let mut allowed: HashSet<*const hir::Expr> = HashSet::new();
    for stmt in &root.block.stmts {
        if let hir::StmtKind::Local { decls, init } = &stmt.kind {
            for (_, e) in decls.iter().zip(init) {
                if is_require_call(module, e) {
                    allowed.insert(e as *const _);
                }
            }
        }
    }
    if let Some(hir::Terminator::Return { exprs, .. }) = &root.block.terminator {
        if let Some(e) = exprs.first() {
            if is_require_call(module, e) {
                allowed.insert(e as *const _);
            }
        }
    }
    let mut stray = false;
    for body in &module.bodies {
        hir::visit::exprs(body, &mut |e| {
            if is_require_call(module, e) && !allowed.contains(&(e as *const _)) {
                stray = true;
            }
        });
    }
    stray
}

fn is_require_call(module: &hir::Module, e: &hir::Expr) -> bool {
    matches!(e, hir::Expr::Call { base, .. } if module.is_require(base))
}

/// Emit R0309 for every member of `iface` with no use anywhere. The caller
/// has already established the module is required and is not a linker; the
/// closedness, poison, and self-read gates live here. `degraded` is `Some`
/// in degrade mode: a member whose name is read anywhere in the project
/// counts as used, and one whose name is written as a string is reported
/// with the places attached. `module` is the defining file's own tree.
pub(crate) fn check_unused(
    iface: &ModuleInterface,
    usage: &Usage,
    degraded: Option<&Degraded<'_>>,
    module: &hir::Module,
    diags: &mut Vec<Diagnostic>,
) {
    if !iface.closed || iface.dynamic_self_read || usage.poisoned {
        return;
    }
    let mut members: Vec<(&str, &Span)> = iface
        .functions
        .iter()
        .map(|(n, sig)| (n.as_str(), &sig.decl_span))
        .chain(iface.fields.iter().map(|(n, sp)| (n.as_str(), sp)))
        .collect();
    members.sort_by_key(|(_, sp)| sp.start);
    for (name, span) in members {
        if name.starts_with('_')
            || usage.used.contains(name)
            || iface.self_reads.contains(name)
            || degraded.is_some_and(|d| d.names.reads.contains(name))
        {
            continue;
        }
        let matches = degraded.map_or_else(Vec::new, |d| string_matches(d, module, name));
        let mut d = diag(name, span, iface, degraded.is_some(), matches);
        if let Some(extent) = definition_extent(module, name) {
            d = d.with_extent(extent);
        }
        diags.push(d);
    }
}

/// The whole definition of `module`'s export member `name`: the span whose
/// removal removes the member. `None` unless the module defines the member
/// exactly once, in a form that can be taken out on its own:
///
///   - `function M.name() ... end` / `function M:name() ... end` — the
///     statement;
///   - `M.name = value` — the statement, when `M.name` is its only target
///     (`M.a, M.b = 1, 2` cannot lose one half);
///
///     either one through the `;` that ends it, when there is one: Luau
///     does not allow a `;` on its own, so one left behind would not parse;
///   - `name = value` / `["name"] = value` in the table constructor the
///     export is declared with — the field and the separator after it.
///
/// A member written in two places (an `if`/`else` pair, a default that is
/// later replaced) has no extent: removing the reported one would leave
/// the other behind. Comments above the definition are not part of it.
fn definition_extent(module: &hir::Module, name: &str) -> Option<Span> {
    let exported = super::calls::exported_name(module)?;
    let is_member = |t: &hir::Target| {
        matches!(t, hir::Target::Projection(hir::Expr::Index { base, key: None, name: Some(n), .. })
            if n == name
                && matches!(&**base, hir::Expr::Name { name: b, .. } if *b == exported))
    };
    // Every place the member is defined; `None` for one that cannot be
    // removed on its own.
    let mut sites: Vec<Option<Span>> = Vec::new();
    for body in &module.bodies {
        let whole = |stmt: &hir::Stmt| {
            let semicolon = body.notes.semicolons.iter().find(|s| s.stmt == stmt.span);
            stmt.span.start..semicolon.map_or(stmt.span.end, |s| s.semicolon.end)
        };
        hir::visit::stmts(body, &mut |stmt| match &stmt.kind {
            hir::StmtKind::FunctionDecl { root, path, .. } => {
                if *root == exported && matches!(path.as_slice(), [m] if m == name) {
                    sites.push(Some(whole(stmt)));
                }
            }
            hir::StmtKind::Assign { targets, .. } => {
                for _ in targets.iter().filter(|t| is_member(t)) {
                    sites.push((targets.len() == 1).then(|| whole(stmt)));
                }
            }
            _ => {}
        });
    }
    // The table constructor the export is declared with: the first
    // top-level `local <exported> = { ... }`, as `members::export_shape`
    // reads it.
    let root = module.body(module.root);
    let declared = root.block.stmts.iter().find_map(|stmt| {
        let hir::StmtKind::Local { decls, init } = &stmt.kind else { return None };
        let idx = decls.iter().position(|d| d.name == exported)?;
        Some(init.get(idx))
    });
    if let Some(Some(hir::Expr::Table(fields))) = declared {
        for f in fields {
            let key = match f {
                hir::TableField::Named { name: n, span, .. } if n == name => span,
                hir::TableField::Keyed {
                    key: hir::Expr::Literal { prim: hir::Prim::String, text: Some(t), span },
                    ..
                } if t == name => span,
                _ => continue,
            };
            let noted = root.notes.table_fields.iter().find(|n| n.key == *key);
            sites.push(noted.map(|n| n.extent.clone()));
        }
    }
    match sites.as_slice() {
        [Some(extent)] => Some(extent.clone()),
        _ => None,
    }
}

/// How many string matches the note names; `--format json` has them all.
const STRING_MATCHES_SHOWN: usize = 3;

fn diag(
    name: &str,
    span: &Span,
    iface: &ModuleInterface,
    degraded: bool,
    string_matches: Vec<Site>,
) -> Diagnostic {
    let mut d = Diagnostic::warning(
        Code(309),
        format!("export `{}` of module `{}` is never used", name, iface.display),
    )
    .with_label(Label::primary(span.clone(), format!("`{name}` exported here")))
    .with_note(
        "no file that requires this module reads it, and the module itself \
         does not",
    );
    if degraded {
        d = d.with_note(format!(
            "the project has dynamic requires, so the bar is raised: `{name}` \
             is flagged only because nothing in the project reads a member of \
             that name off ANYTHING"
        ));
    }
    if !string_matches.is_empty() {
        let shown: Vec<String> = string_matches
            .iter()
            .take(STRING_MATCHES_SHOWN)
            .map(|site| {
                let file = site.path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default();
                format!("{file}:{}", site.line)
            })
            .collect();
        let more = match string_matches.len().saturating_sub(STRING_MATCHES_SHOWN) {
            0 => String::new(),
            n => format!(" and {n} more"),
        };
        d = d
            .with_note(format!(
                "the name also appears as a string, at {}{more}: code behind a \
                 dynamic require may reach `{name}` by that string, so check \
                 before deleting",
                shown.join(", ")
            ))
            .with_string_matches(string_matches);
    }
    d.conservative(
        "only statically-resolved requires and member reads are counted; a \
         consumer outside this check's view, or one reached dynamically, \
         would not be seen",
    )
    .with_help(format!("delete it, or rename to `_{name}` if it is kept deliberately"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage_of(src: &str, binding: &str) -> Usage {
        let module = rime_syntax::parse(src).unwrap();
        let dep = PathBuf::from("dep.luau");
        let bindings = HashMap::from([(binding.to_string(), dep.clone())]);
        let mut usage = HashMap::new();
        collect_usage(&module, &bindings, &mut usage);
        usage.remove(&dep).expect("binding recorded")
    }

    fn names_of(src: &str) -> HashSet<String> {
        let module = rime_syntax::parse(src).unwrap();
        let mut names = HashSet::new();
        collect_read_names(&module, &mut names);
        names
    }

    #[test]
    fn read_names_collects_dot_method_write_and_literal_key() {
        let n = names_of(
            "local t = f()\nprint(t.alpha)\nt:beta()\nt.gamma = 1\nprint(t[\"delta\"])\n",
        );
        assert_eq!(
            n,
            HashSet::from(["alpha", "beta", "gamma", "delta"].map(String::from))
        );
    }

    // `M.LIMIT = 5` in the module that returns `M` defines the member. If
    // it counted as a read, every member written this way would read
    // itself and R0309 could never flag one in degrade mode.
    #[test]
    fn read_names_skip_the_modules_own_defining_writes() {
        let n = names_of(
            "local M = {}\nM.LIMIT = 5\nM.handler = function() end\n\
             function M.init()\n\tM.late = true\nend\nreturn M\n",
        );
        assert!(n.is_empty(), "got {n:?}");
        // `table.freeze` itself is a read of `freeze`; `LIMIT` is not one.
        let frozen = names_of("local M = {}\nM.LIMIT = 5\nreturn table.freeze(M)\n");
        assert_eq!(frozen, HashSet::from(["freeze".to_string()]));
    }

    #[test]
    fn read_names_keep_the_modules_own_reads() {
        // `+=` reads the slot before writing it; `M.a.b = v` reads `a`.
        let n = names_of(
            "local M = {}\nM.count = 0\nM.count += 1\nM.box = {}\nM.box.lid = 1\n\
             print(M.seen)\nM:go()\nreturn M\n",
        );
        assert_eq!(
            n,
            HashSet::from(["count", "box", "lid", "seen", "go"].map(String::from))
        );
    }

    // `function M.ns.f()` stores into the table `M.ns` holds: it reads `ns`
    // exactly as `M.ns.f = function() end` does. With `ns` deleted the
    // declaration throws when the module loads.
    #[test]
    fn read_names_count_the_table_a_nested_function_is_declared_on() {
        let n = names_of("local M = {}\nM.ns = {}\nfunction M.ns.f() end\nreturn M\n");
        assert_eq!(n, HashSet::from(["ns".to_string()]));
        let n = names_of("local M = {}\nM.a = { b = {} }\nfunction M.a.b:m() end\nreturn M\n");
        assert_eq!(n, HashSet::from(["a", "b"].map(String::from)));
        let iface = super::super::calls::module_interface(
            &rime_syntax::parse("local M = {}\nM.ns = {}\nfunction M.ns.f() end\nreturn M\n").unwrap(),
            "M",
            "M.luau",
            "",
        );
        assert!(iface.self_reads.contains("ns"), "got {:?}", iface.self_reads);
    }

    #[test]
    fn read_names_keep_writes_to_any_other_table() {
        // Only the returned table's own members are defined by a write to
        // it. A write through any other name still counts, as before.
        let n = names_of("local M = {}\nlocal other = f()\nother.LIMIT = 5\nreturn M\n");
        assert_eq!(n, HashSet::from(["LIMIT".to_string()]));
        // No named export: nothing is a defining write.
        let n = names_of("local t = f()\nt.LIMIT = 5\nreturn { t }\n");
        assert_eq!(n, HashSet::from(["LIMIT".to_string()]));
    }

    fn strings_of(src: &str) -> HashSet<String> {
        string_sites_of(src).into_keys().collect()
    }

    fn string_sites_of(src: &str) -> HashMap<String, Vec<usize>> {
        let module = rime_syntax::parse(src).unwrap();
        let mut names = HashMap::new();
        collect_string_names(&module, &mut names);
        names
    }

    /// The 1-based lines of the strings `own_body_strings` leaves out.
    fn own_strings(src: &str, name: &str) -> Vec<usize> {
        let mut lines: Vec<usize> = own_body_strings(&rime_syntax::parse(src).unwrap(), name)
            .into_iter()
            .map(|offset| src[..offset].matches('\n').count() + 1)
            .collect();
        lines.sort_unstable();
        lines
    }

    #[test]
    fn string_names_record_where_each_occurrence_starts() {
        let src = "f(\"alpha\")\nlocal function g()\n\tf(\"alpha\", \"beta\")\nend\n";
        let n = string_sites_of(src);
        let at = |needle: &str, nth: usize| src.match_indices(needle).nth(nth).unwrap().0;
        assert_eq!(
            n,
            HashMap::from([
                ("alpha".to_string(), vec![at("\"alpha\"", 0), at("\"alpha\"", 1)]),
                ("beta".to_string(), vec![at("\"beta\"", 0)]),
            ])
        );
    }

    /// `check_unused` on `src` as a required module in degrade mode, with
    /// `other` as a second file of the project.
    fn degraded_reports(src: &str, other: &str) -> Vec<Diagnostic> {
        let module = rime_syntax::parse(src).unwrap();
        let other_module = rime_syntax::parse(other).unwrap();
        let iface = super::super::calls::module_interface(&module, "M", "M.luau", src);
        let mut names = GlobalNames::default();
        for (file, m) in [&module, &other_module].into_iter().enumerate() {
            collect_read_names(m, &mut names.reads);
            let mut strings = HashMap::new();
            collect_string_names(m, &mut strings);
            for (name, at) in strings {
                names.strings.entry(name).or_default().extend(at.into_iter().map(|o| (file, o)));
            }
        }
        let locate = |file: usize, offset: usize| Site {
            path: ["dir/M.luau", "dir/other.luau"][file].into(),
            line: [src, other][file][..offset].matches('\n').count() + 1,
        };
        let degraded = Degraded { names: &names, file: 0, locate: &locate };
        let mut diags = Vec::new();
        check_unused(&iface, &Usage::default(), Some(&degraded), &module, &mut diags);
        diags
    }

    fn sites(d: &Diagnostic) -> Vec<(String, usize)> {
        d.string_matches.iter().map(|s| (s.path.to_string_lossy().replace('\\', "/"), s.line)).collect()
    }

    // A string equal to the name does not silence the report. It marks it:
    // every place is in `string_matches`, the note names them.
    #[test]
    fn a_string_equal_to_the_name_marks_the_report_and_keeps_it() {
        let src = "local M = {}\nM.hooks = { \"refresh\" }\nfunction M.refresh() end\nfunction M.plain() end\nprint(M.hooks)\nreturn M\n";
        let diags = degraded_reports(src, "f(\"refresh\", \"refresh\")\n");
        let [refresh, plain] = diags.as_slice() else { panic!("got {diags:?}") };
        assert!(refresh.message.contains("`refresh`"), "{}", refresh.message);
        assert_eq!(sites(refresh), vec![("dir/M.luau".to_string(), 2), ("dir/other.luau".to_string(), 1)]);
        assert!(
            refresh.notes.iter().any(|n| n.contains("also appears as a string, at M.luau:2, other.luau:1:")),
            "{:?}",
            refresh.notes
        );
        // No string equals `plain`: no mark, and no note about strings.
        assert!(plain.string_matches.is_empty());
        assert!(!plain.notes.iter().any(|n| n.contains("string")), "{:?}", plain.notes);
    }

    #[test]
    fn the_note_names_three_places_and_counts_the_rest() {
        let src = "local M = {}\nfunction M.tick() end\nreturn M\n";
        let diags = degraded_reports(src, "f(\"tick\")\nf(\"tick\")\nf(\"tick\")\nf(\"tick\")\nf(\"tick\")\n");
        let [tick] = diags.as_slice() else { panic!("got {diags:?}") };
        assert_eq!(tick.string_matches.len(), 5);
        assert!(
            tick.notes.iter().any(|n| n.contains("at other.luau:1, other.luau:2, other.luau:3 and 2 more:")),
            "{:?}",
            tick.notes
        );
    }

    // A read of the name still silences, string or no string; and a string
    // inside the member's own function is not a match.
    #[test]
    fn reads_still_silence_and_own_strings_do_not_mark() {
        let src = "local M = {}\nfunction M.save(v)\n\tlog(\"save\", v)\nend\nfunction M.load()\n\treturn log(\"load\")\nend\n\
                   function M.relay(...)\n\tlog(\"load\", ...)\nend\nfunction M.read() end\nreturn M\n";
        let diags = degraded_reports(src, "f(\"read\")\nprint(x.read, x.relay)\n");
        let [save, load] = diags.as_slice() else { panic!("got {diags:?}") };
        assert!(save.message.contains("`save`") && save.string_matches.is_empty(), "{save:?}");
        assert!(load.message.contains("`load`"));
        assert_eq!(sites(load), vec![("dir/M.luau".to_string(), 9)]);
    }

    // A string inside the function it names is not a use of that function,
    // however the function is defined and however deep the string sits.
    #[test]
    fn own_body_strings_count_only_the_members_own_function() {
        let decl = "local M = {}\nfunction M.save()\n\tlog(\"save\")\n\
                    \ttask.spawn(function()\n\t\tlog(\"save\", \"load\")\n\tend)\nend\n\
                    function M.load()\n\tlog(\"save\")\nend\nM.names = { \"save\" }\nreturn M\n";
        assert_eq!(own_strings(decl, "save"), vec![3, 5]);
        assert_eq!(own_strings(decl, "load"), Vec::<usize>::new());
        assert_eq!(own_strings(decl, "names"), Vec::<usize>::new());
        let assigned = "local M = {}\nM.save = function()\n\tlog(\"save\")\nend\nreturn M\n";
        assert_eq!(own_strings(assigned, "save"), vec![3]);
        let method = "local M = {}\nfunction M:save()\n\tlog(\"save\")\nend\nreturn M\n";
        assert_eq!(own_strings(method, "save"), vec![3]);
        let literal = "local M = {\n\tsave = function()\n\t\tlog(\"save\")\n\tend,\n}\nreturn M\n";
        assert_eq!(own_strings(literal, "save"), vec![3]);
    }

    // A value that holds its own name is the definition, not a use of it.
    #[test]
    fn own_body_strings_count_the_members_own_value() {
        let src = "local M = {\n\tReady = \"Ready\",\n\t[\"Quoted\"] = 1,\n}\n\
                   M.Opened = \"Opened\"\nM.Closed = { name = \"Closed\", other = \"Opened\" }\n\
                   M.list = { \"Closed\" }\nM.a, M.b = \"b\", \"a\"\nreturn M\n";
        assert_eq!(own_strings(src, "Ready"), vec![2]);
        assert_eq!(own_strings(src, "Quoted"), vec![3]);
        assert_eq!(own_strings(src, "Opened"), vec![5]);
        assert_eq!(own_strings(src, "Closed"), vec![6]);
        assert_eq!(own_strings(src, "list"), Vec::<usize>::new());
        // Each target of a multiple assignment has its own value.
        assert_eq!(own_strings(src, "a"), Vec::<usize>::new());
    }

    #[test]
    fn own_body_strings_ignore_functions_that_are_not_the_member() {
        // Another table's `save`, a nested `M.ns.save`, a local `save`, and
        // a module with no named export.
        let n = own_strings(
            "local M = {}\nlocal other = {}\nfunction other.save()\n\tlog(\"save\")\nend\n\
             M.ns = {}\nfunction M.ns.save()\n\tlog(\"save\")\nend\n\
             local function save()\n\tlog(\"save\")\nend\nreturn M\n",
            "save",
        );
        assert_eq!(n, Vec::<usize>::new());
        assert_eq!(own_strings("return { save = function()\n\tlog(\"save\")\nend }\n", "save"), Vec::<usize>::new());
    }

    // A list of names, a call argument, the paren-less call form, and a
    // string inside a nested function all leave the name behind.
    #[test]
    fn string_names_collect_every_plain_literal_shaped_like_a_name() {
        let n = strings_of(
            "local list = { \"alpha\", 'beta' }\nf(\"gamma\")\nf \"delta\"\n\
             local function g()\n\treturn [[epsilon]]\nend\n",
        );
        assert_eq!(
            n,
            HashSet::from(["alpha", "beta", "gamma", "delta", "epsilon"].map(String::from))
        );
    }

    #[test]
    fn string_names_skip_what_no_member_is_named_by() {
        let n = strings_of(
            "f(\"two words\", \"a.b\", \"\", \"9lives\", \"dash-ed\")\n\
             local t = { key = 1 }\nprint(t.field)\n",
        );
        assert!(n.is_empty(), "got {n:?}");
        // A name built at run time leaves no literal equal to it.
        let n = strings_of("local kind = f()\nprint(\"get\" .. kind, `get{kind}`)\n");
        assert_eq!(n, HashSet::from(["get".to_string()]));
    }

    /// The text `definition_extent` covers, or `None`.
    fn extent_text<'a>(src: &'a str, name: &str) -> Option<&'a str> {
        let module = rime_syntax::parse(src).unwrap();
        definition_extent(&module, name).map(|e| &src[e])
    }

    #[test]
    fn extent_of_a_function_is_its_whole_statement() {
        let src = "local M = {}\n\n-- doc\nfunction M.run(a: number): number\n\tlocal b = a\n\treturn b\nend\n\n\
                   function M:step()\nend\n\nM.late = function()\n\treturn 1\nend\n\nreturn M\n";
        assert_eq!(
            extent_text(src, "run"),
            Some("function M.run(a: number): number\n\tlocal b = a\n\treturn b\nend")
        );
        assert_eq!(extent_text(src, "step"), Some("function M:step()\nend"));
        assert_eq!(extent_text(src, "late"), Some("M.late = function()\n\treturn 1\nend"));
    }

    #[test]
    fn extent_of_a_value_is_its_assignment_or_its_table_field() {
        let src = "local M = {\n\tfirst = 1,\n\tmiddle = { 1, 2 };\n\t[\"quoted\"] = f(3),\n\tlast = \"x\"\n}\n\
                   M.LIMIT = 5\nM.table = {\n\ta = 1,\n}\nreturn M\n";
        assert_eq!(extent_text(src, "first"), Some("first = 1,"));
        assert_eq!(extent_text(src, "middle"), Some("middle = { 1, 2 };"));
        assert_eq!(extent_text(src, "quoted"), Some("[\"quoted\"] = f(3),"));
        assert_eq!(extent_text(src, "last"), Some("last = \"x\""));
        assert_eq!(extent_text(src, "LIMIT"), Some("M.LIMIT = 5"));
        assert_eq!(extent_text(src, "table"), Some("M.table = {\n\ta = 1,\n}"));
        // A field of a table that is not the export.
        assert_eq!(extent_text(src, "a"), None);
    }

    #[test]
    fn extent_takes_the_semicolon_that_ends_the_statement() {
        // Luau does not allow a `;` on its own, so one left behind would
        // not parse.
        let src = "local M = {}\nM.used = 1;\nM.dead = 2;\nfunction M.run() end;\n\
                   M.late = function()\n\treturn 1\nend ; -- why\n\
                   M.far = 3 -- why\n;\n\
                   do\n\tM.first = 4; M.second = 5;\nend\n\
                   function M.inner()\n\tM.nested = 6;\nend\nM.bare = 7\nreturn M\n";
        assert_eq!(extent_text(src, "dead"), Some("M.dead = 2;"));
        assert_eq!(extent_text(src, "run"), Some("function M.run() end;"));
        assert_eq!(extent_text(src, "late"), Some("M.late = function()\n\treturn 1\nend ;"));
        assert_eq!(extent_text(src, "far"), Some("M.far = 3 -- why\n;"));
        assert_eq!(extent_text(src, "first"), Some("M.first = 4;"));
        assert_eq!(extent_text(src, "second"), Some("M.second = 5;"));
        assert_eq!(extent_text(src, "nested"), Some("M.nested = 6;"));
        assert_eq!(extent_text(src, "bare"), Some("M.bare = 7"));
    }

    #[test]
    fn deleting_an_extent_leaves_a_file_that_parses() {
        let src = "local M = {\n\tfirst = 1,\n\tmiddle = { 1, 2 };\n\t[\"quoted\"] = f(3),\n\tlast = \"x\"\n}\n\
                   M.plain = 1\nM.ended = 2;\nfunction M.run() end;\nfunction M:step()\nend\n\
                   M.late = function()\n\treturn 1\nend ;\n\
                   do\n\tM.head = 4; M.tail = 5;\nend\n\
                   if flag then\n\tM.only = 6;\nend\nreturn M\n";
        let module = rime_syntax::parse(src).unwrap();
        for name in [
            "first", "middle", "quoted", "last", "plain", "ended", "run", "step", "late", "head", "tail", "only",
        ] {
            let extent = definition_extent(&module, name).unwrap_or_else(|| panic!("no extent for {name}"));
            let rest = format!("{}{}", &src[..extent.start], &src[extent.end..]);
            assert!(rime_syntax::parse(&rest).is_ok(), "without `{name}`:\n{rest}");
        }
    }

    #[test]
    fn extent_is_none_when_one_span_does_not_remove_the_member() {
        // Written in two places: removing one leaves the other.
        let twice = "local M = {}\nif flag then\n\tM.mode = 1\nelse\n\tM.mode = 2\nend\nreturn M\n";
        assert_eq!(extent_text(twice, "mode"), None);
        let replaced = "local M = { mode = 1 }\nfunction M.mode() end\nreturn M\n";
        assert_eq!(extent_text(replaced, "mode"), None);
        // One statement defines two members.
        let pair = "local M = {}\nM.a, M.b = 1, 2\nreturn M\n";
        assert_eq!(extent_text(pair, "a"), None);
        // Not a member of the export, or no named export at all.
        assert_eq!(extent_text("local M = {}\nlocal other = {}\nother.x = 1\nreturn M\n", "x"), None);
        assert_eq!(extent_text("return { x = 1 }\n", "x"), None);
    }

    #[test]
    fn read_names_ignores_construction_and_computed_keys() {
        let n = names_of("local k = f()\nlocal t = { built = 1 }\nprint(t[k])\n");
        assert!(n.is_empty(), "got {n:?}");
    }

    #[test]
    fn reassigned_binding_poisons() {
        let u = usage_of("local Mod = require(x)\nMod = {}\nprint(Mod.a)\n", "Mod");
        assert!(u.poisoned);
    }

    #[test]
    fn member_write_counts_as_use_without_poison() {
        let u = usage_of("local Mod = require(x)\nMod.slot = 1\n", "Mod");
        assert!(!u.poisoned);
        assert!(u.used.contains("slot"));
    }

    #[test]
    fn bare_call_poisons() {
        let u = usage_of("local Mod = require(x)\nMod()\n", "Mod");
        assert!(u.poisoned);
    }

    #[test]
    fn reads_and_method_calls_mark_used() {
        let u = usage_of(
            "local Mod = require(x)\nprint(Mod.a)\nMod:go()\nlocal f = Mod.cb\n",
            "Mod",
        );
        assert!(!u.poisoned);
        assert_eq!(
            u.used,
            HashSet::from(["a".to_string(), "go".to_string(), "cb".to_string()])
        );
    }

    #[test]
    fn deep_write_reads_first_member() {
        // `Mod.a.b = v` mutates a member's table — that inner `Mod.a` is a
        // read of `a`, not a poison.
        let u = usage_of("local Mod = require(x)\nMod.a.b = 1\n", "Mod");
        assert!(!u.poisoned);
        assert!(u.used.contains("a"));
    }
}
