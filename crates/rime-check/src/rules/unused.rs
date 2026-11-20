//! R0013/R0014/R0015: unused local, unused parameter, unused local function
//! (decided 2026-08-21); R0016 unused-assignment (2026-08-23, Rust's
//! `unused_assignments`): when a binding is never read, EVERY store to it
//! beyond the initializer gets its own diagnostic at the write site —
//! closure writes included. Driver: deleting only the `local` line of a
//! write-only binding strands its writes as R0023 implicit globals; marking
//! each site shows the full footprint before the delete. Warning severity
//! by default — the code runs fine; the binding is clutter or a typo, and
//! Rime cannot tell which, so all four are conservative and individually
//! disableable in rime.toml.
//!
//! "Use" means any READ: an expression, an index base (`t.x = v` reads `t`),
//! an argument, a return value, a compound-assign target — a read inside
//! a nested closure, which the outer CFG cannot see, so the capture
//! machinery (`crate::capture`) supplies cross-body reads by name — or a
//! TYPE-position reference (`type Item = Types.Item`, `x: Config.Y`, `v ::
//! Enums.Kind`): lowering drops type structure, so `hir::Module::type_refs`
//! carries the referenced names and they count as reads here. A function
//! declared on a table (`function M.f() end`) reads `M` the way `M.f = v`
//! does. Plain reassignment is not a use.
//!
//! One exception to "an index base is a read" (2026-11-20, Selene's
//! write-only case): a local declared with a table constructor, never
//! reassigned, whose ONLY mentions are the base of a field write (`t.x =
//! v`, `t[k] = v`). Nobody else can hold that table, so what is stored in
//! it is never seen — R0013, worded "written to, but never read". Any
//! other mention keeps it silent: a read of a field, a nested write
//! (`t.x.y = v`), a compound assign, a closure, a call argument. `_`-prefixed names are exempt everywhere
//! (deliberate discards, same contract as shadowing's help text).
//!
//! Scope decisions:
//!   - one shared walk classifies by `LocalKind`: params → R0014, `local
//!     function` bindings → R0015, everything else (plain locals and loop
//!     variables, judged individually) → R0013;
//!   - self-recursion is NOT a use for R0015 (a function only its own body
//!     mentions is unreachable all the same) — the capture walk seeds the
//!     closure's own name so self-reference never escapes. Mutual recursion
//!     DOES count (either function keeps the other alive) — accepted v1;
//!   - the module top level is checked since v2 (2026-08-23; v1 exempted
//!     it). No require carve-out: an unused `local X = require(…)` or
//!     service cache is exactly the clutter/typo signal the rule exists
//!     for, and a large-codebase sweep found no false positives once type-position
//!     reads counted. Export locals are naturally silent (`return M` reads
//!     `M`); side-effect requires spell it `local _ = require(…)`;
//!   - a read in unreachable code still counts as a use — unreachable-code
//!     (R0103) already owns that finding, and silence beats double noise;
//!   - the closure-read and type-ref checks are name-based and
//!     over-approximate: a free name that actually refers to a global (or a
//!     different same-named local) silences a warning it could have raised,
//!     never the reverse.

use crate::capture;
use crate::cfg::{Cfg, Event, LocalKind};
use rime_diag::{Code, Diagnostic, Label};
use rime_hir as hir;
use std::collections::HashSet;

pub fn check(cfg: &Cfg, module: &hir::Module, body: &hir::Body, diags: &mut Vec<Diagnostic>) {
    // A read that is only the `t` of a field write (`t.x = v`) is kept
    // apart: a table nothing else reads is write-only.
    let (write_bases, table_inits) = field_write_sites(body);
    let mut read = vec![false; cfg.locals.len()];
    let mut field_writes: Vec<Vec<rime_diag::Span>> = vec![Vec::new(); cfg.locals.len()];
    for block in &cfg.blocks {
        for ev in &block.events {
            if let Event::Read(id, span) = ev {
                if write_bases.contains(&span.start) {
                    field_writes[*id].push(span.clone());
                } else {
                    read[*id] = true;
                }
            }
        }
    }

    let (closure_read, closure_writes) = closure_uses(module, body);
    let mut field_function_roots = HashSet::new();
    collect_field_function_roots(module, &body.block, &mut field_function_roots);

    for (id, info) in cfg.locals.iter().enumerate() {
        if info.name.starts_with('_')
            || read[id]
            || closure_read.contains(&info.name)
            || module.type_refs.contains(&info.name)
            || field_function_roots.contains(&info.name)
        {
            continue;
        }
        // Stores beyond the initializer: explicit same-body assigns at a
        // span other than the declaration's (the initializer's Assign
        // carries decl_span — Rust likewise folds `let x = 5` into the
        // declaration warning), plus closure writes matched by name.
        let mut stores: Vec<rime_diag::Span> = Vec::new();
        for block in &cfg.blocks {
            for ev in &block.events {
                if let Event::Assign(aid, _, Some(span)) = ev {
                    if *aid == id && *span != info.decl_span {
                        stores.push(span.clone());
                    }
                }
            }
        }
        stores.extend(
            closure_writes
                .iter()
                .filter(|(n, _)| *n == info.name)
                .map(|(_, s)| s.clone()),
        );
        stores.sort_by_key(|s| s.start);
        let mut writes = std::mem::take(&mut field_writes[id]);
        if !writes.is_empty() {
            // Field writes are its only use. That is dead state only when
            // nobody else can hold the table: built by a constructor in
            // the declaration, and the local never pointed anywhere else.
            // Anything less — `local t = call()`, a later `t = other` —
            // stays silent, as it was before write-only was a finding.
            let fresh = matches!(info.kind, LocalKind::Plain)
                && table_inits.contains(&info.decl_span.start)
                && stores.is_empty();
            if fresh {
                writes.sort_by_key(|s| s.start);
                writes.dedup();
                diags.push(write_only_diag(&info.name, info.decl_span.clone(), writes));
            }
            continue;
        }
        diags.push(diag(
            &info.name,
            info.decl_span.clone(),
            info.kind,
            !stores.is_empty(),
        ));
        // R0016, Rust's `unused_assignments`: each dead store is its own
        // occurrence at its own site — deleting only the `local` line
        // would strand these writes as R0023 implicit globals.
        for span in stores {
            diags.push(store_diag(&info.name, span));
        }
    }
}

/// Two sets of source offsets, both keyed by where a name starts:
///   - every name that is the direct base of a plain assignment target —
///     the `t` of `t.x = v` and `t[k] = v`, not of `t.x.y = v` or
///     `t.x += v`;
///   - every local declared with a table constructor as its own value.
fn field_write_sites(body: &hir::Body) -> (HashSet<usize>, HashSet<usize>) {
    let (mut write_bases, mut table_inits) = (HashSet::new(), HashSet::new());
    hir::visit::stmts(body, &mut |stmt| match &stmt.kind {
        hir::StmtKind::Assign { targets, .. } => {
            for target in targets {
                if let hir::Target::Projection(hir::Expr::Index { base, .. }) = target {
                    if let hir::Expr::Name { span, .. } = &**base {
                        write_bases.insert(span.start);
                    }
                }
            }
        }
        hir::StmtKind::Local { decls, init } => {
            for (decl, value) in decls.iter().zip(init) {
                if matches!(value, hir::Expr::Table(_)) {
                    table_inits.insert(decl.span.start);
                }
            }
        }
        _ => {}
    });
    (write_bases, table_inits)
}

/// How many write sites one write-only report marks before it counts the
/// rest in a note.
const MARKED_WRITES: usize = 5;

/// R0013 for a table that only ever has fields stored in it. One report
/// for the binding, with the writes as secondary labels: they are why it
/// looks used, and what goes when it does.
fn write_only_diag(name: &str, decl_span: rime_diag::Span, writes: Vec<rime_diag::Span>) -> Diagnostic {
    let total = writes.len();
    let mut d = Diagnostic::warning(Code(13), format!("`{name}` is written to, but never read"))
        .with_label(Label::primary(decl_span, format!("`{name}` declared here")));
    for span in writes.into_iter().take(MARKED_WRITES) {
        d = d.with_label(Label::secondary(span, format!("`{name}` is only written to")));
    }
    if total > MARKED_WRITES {
        d = d.with_note(format!("{} more writes to `{name}` are not marked", total - MARKED_WRITES));
    }
    d.conservative(format!(
        "every use of `{name}` stores a field in it, and the table is never \
         read or passed on, so nothing can see what was stored; Rime cannot \
         tell leftover clutter from a table that was meant to be returned"
    ))
    .with_help(format!(
        "delete it and the writes to it, or rename to `_{name}` if the binding is deliberate"
    ))
}

fn store_diag(name: &str, span: rime_diag::Span) -> Diagnostic {
    Diagnostic::warning(Code(16), format!("value assigned to `{name}` is never read"))
        .with_label(Label::primary(span, "this write is dead"))
        .conservative(format!(
            "`{name}` is never read anywhere in this function or its \
             closures, so every store to it is dead; delete the binding and \
             its writes together"
        ))
}

/// Free names inside closures nested in `body`, split by kind: READS keep a
/// binding alive; pure WRITES do not (2026-08-23; compound assigns count as
/// reads, so accumulators stay silent — faint-variable v2 territory) and
/// come back with their spans so R0016 can mark each dead store. For a
/// `local function`, the closure's own name is seeded into scope so
/// self-recursion never counts as an outside use.
fn closure_uses(
    module: &hir::Module,
    body: &hir::Body,
) -> (HashSet<String>, Vec<(String, rime_diag::Span)>) {
    let mut nested = Vec::new();
    capture::nested_closures(&body.block, &mut nested);
    let mut reads = HashSet::new();
    let mut writes = Vec::new();
    for (id, self_name) in nested {
        let closure = module.body(id);
        let mut seed: HashSet<String> = closure.params.iter().map(|p| p.name.clone()).collect();
        seed.extend(self_name);
        let mut scopes = vec![seed];
        capture::free_names(module, &closure.block, &mut scopes, &mut reads, &mut writes);
    }
    (reads, writes)
}

/// The `M` of every `function M.f() end` / `function M:f() end` in `block`
/// and in the closures under it. Such a declaration is `M.f = function()
/// end`: it reads `M` to write a field of the table it holds, and anything
/// else holding that table (a required module, say) sees the new field.
/// The CFG does not model the statement as a read, so the names are
/// collected here — by name, like the closure reads above, which can only
/// silence a warning, never raise one.
fn collect_field_function_roots(module: &hir::Module, block: &hir::Block, out: &mut HashSet<String>) {
    hir::visit::stmts_in_block(block, &mut |stmt| {
        if let hir::StmtKind::FunctionDecl { root, assigns_root: false, .. } = &stmt.kind {
            if !out.contains(root) {
                out.insert(root.clone());
            }
        }
    });
    let mut nested = Vec::new();
    capture::nested_closures(block, &mut nested);
    for (id, _) in nested {
        collect_field_function_roots(module, &module.body(id).block, out);
    }
}

fn diag(name: &str, decl_span: rime_diag::Span, kind: LocalKind, has_stores: bool) -> Diagnostic {
    // Rust-parity wording: a binding with live-looking stores says
    // "assigned, but never read" (rustc: "variable is assigned to, but
    // never used"), so the reader knows write sites exist — R0016 marks
    // each one. A binding with no stores keeps the plain message.
    match kind {
        LocalKind::Param => {
            let msg = if has_stores {
                format!("parameter `{name}` is assigned, but never read")
            } else {
                format!("parameter `{name}` is never read")
            };
            Diagnostic::warning(Code(14), msg)
                .with_label(Label::primary(
                    decl_span.clone(),
                    format!("`{name}` declared here"),
                ))
                .conservative(
                    "never read in the function body or its closures; Rime cannot tell \
                     an oversight from a positional parameter kept for its slot",
                )
                .with_help(format!("use it, or rename to `_{name}` if the position is required"))
        }
        LocalKind::Function => Diagnostic::warning(Code(15), format!("`{name}` is never used"))
            .with_label(Label::primary(
                decl_span.clone(),
                format!("`{name}` defined here"),
            ))
            .conservative(
                "nothing outside its own body references it, and self-recursion \
                     does not count as use",
            )
            .with_help("call it, export it, or delete it"),
        LocalKind::Plain | LocalKind::Loop => {
            let fix = match (kind, has_stores) {
                (LocalKind::Loop, _) => format!("rename to `_{name}` if the slot is required"),
                (_, true) => format!(
                    "delete it AND its writes (each marked by R0016), or \
                     rename to `_{name}` if the binding is deliberate"
                ),
                _ => format!("delete it, or rename to `_{name}` if the binding is deliberate"),
            };
            let msg = if has_stores {
                format!("`{name}` is assigned, but never read")
            } else {
                format!("`{name}` is never read")
            };
            Diagnostic::warning(Code(13), msg)
                .with_label(Label::primary(
                    decl_span.clone(),
                    format!("`{name}` declared here"),
                ))
                .conservative(
                    "never read anywhere in this function or its closures; Rime cannot \
                     tell leftover clutter from a mistyped name",
                )
                .with_help(fix)
        }
    }
}

#[cfg(test)]
mod tests {
    /// Line and message of every R0013 in `src`.
    fn unused_locals(src: &str) -> Vec<(usize, String)> {
        crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(13))
            .map(|d| (src[..d.primary_span().start].matches('\n').count() + 1, d.message))
            .collect()
    }

    fn lines(src: &str) -> Vec<usize> {
        unused_locals(src).into_iter().map(|(line, _)| line).collect()
    }

    #[test]
    fn a_function_declared_on_a_local_table_uses_the_table() {
        // `function M.extra() end` is `M.extra = function() end`: it writes
        // a field of the table `M` holds, and whoever else holds that
        // table sees it. First case is Selene's `mutating_functions`
        // fixture, which it stays silent on.
        let cases = [
            "local noSideEffects = call()\n\nfunction noSideEffects.foo() end\n",
            "local M = require(script.Parent.Base)\n\nfunction M.extra()\n\treturn 1\nend\n",
            "local M = require(script.Parent.Base)\n\nfunction M:extra()\n\treturn self\nend\n",
            "local M = require(script.Parent.Base)\n\nfunction M.sub.extra() end\n",
            // Declared from inside a closure.
            "local M = require(script.Parent.Base)\n\
             local function setup()\n\tfunction M.extra() end\nend\n\
             setup()\n",
            // Declared in a nested block.
            "local M = require(script.Parent.Base)\nif M ~= nil then\n\tfunction M.extra() end\nend\n",
        ];
        for case in cases {
            assert_eq!(lines(case), vec![], "{case}");
        }
    }

    /// Selene's `write_only` fixture. Its `.stderr` reports
    /// `bad1` on line 1 and nothing else.
    const SELENE_WRITE_ONLY: &str = "local bad1 = {}\n\
        bad1.x = 1\n\
        bad1.y = 2\n\
        bad1[\"z\"] = 1\n\n\
        local badButNotYetImplemented = { x = {} }\n\
        badButNotYetImplemented.x.y = 1\n\n\
        local okay1 = ok()\n\
        okay1.x = 1\n\
        okay1.y = 2\n\n\
        local okay2 = { x = ok() }\n\
        okay2.x.y = 1\n\n\
        local okay3 = {}\n\
        okay3.x().y = 1\n";

    #[test]
    fn a_table_that_is_only_written_to_is_reported_as_selene_reports_it() {
        let found = unused_locals(SELENE_WRITE_ONLY);
        assert_eq!(found, vec![(1, "`bad1` is written to, but never read".to_string())]);
        // One report for the binding, with each write marked.
        let d = crate::check_source(SELENE_WRITE_ONLY)
            .into_iter()
            .find(|d| d.code == rime_diag::Code(13))
            .unwrap();
        let writes: Vec<&str> = d
            .labels
            .iter()
            .filter(|l| !l.primary)
            .map(|l| &SELENE_WRITE_ONLY[l.span.clone()])
            .collect();
        assert_eq!(writes, vec!["bad1", "bad1", "bad1"]);
        assert!(d.conservative);
    }

    #[test]
    fn write_only_needs_a_fresh_table_and_nothing_but_field_writes() {
        // (source, lines reported). Selene agrees on every case but
        // the closure one, which it reports and Rime leaves alone.
        let cases: [(&str, &[usize]); 14] = [
            ("local d = {}\nd.x = 1\n", &[1]),
            ("local e = { 1, 2 }\ne[1] = 5\n", &[1]),
            ("local n = { call() }\nn.x = 1\n", &[1]),
            ("local function f(v, k)\n\tlocal t = {}\n\tif v then\n\t\tt[k] = v\n\tend\nend\nreturn f\n", &[2]),
            // Not a fresh table: someone else may hold it.
            ("local k = call()\nk.x = 1\n", &[]),
            ("local o = setmetatable({}, {})\no.x = 1\n", &[]),
            ("local m\nm = {}\nm.x = 1\n", &[]),
            // The table is read somewhere.
            ("local g = {}\ng.x = g\n", &[]),
            ("local h = {}\nh[h] = 1\n", &[]),
            ("local l = {}\nl.x = l.y\n", &[]),
            ("local j = {}\nj.n = 1\nj.n += 1\n", &[]),
            ("local a = {}\na[#a + 1] = 1\n", &[]),
            ("local r = {}\nr.x = 1\nreturn r\n", &[]),
            // Mentioned in a closure: left alone.
            ("local i = {}\ni.x = 1\nprint(function()\n\ti.y = 2\nend)\n", &[]),
        ];
        for (src, want) in cases {
            assert_eq!(lines(src), want, "{src}");
        }
    }

    #[test]
    fn write_only_is_judged_per_binding() {
        // The second `t` shadows the first; only the first is write-only.
        let src = "local t = {}\nt.x = 1\ndo\n\tlocal t = {}\n\tt.y = 2\n\tprint(t)\nend\n";
        assert_eq!(lines(src), vec![1]);
        // `_`-prefixed names are exempt, as everywhere in this rule.
        assert_eq!(lines("local _sink = {}\n_sink.x = 1\n"), vec![]);
    }

    #[test]
    fn a_function_declaration_elsewhere_does_not_use_the_local() {
        // Another table's method.
        let src = "local M = require(script.Parent.Base)\nlocal N = {}\nfunction N.extra() end\nreturn N\n";
        assert_eq!(lines(src), vec![1]);
        // `function f() end` on a local `f` is a plain store, not a use.
        let src = "local f\nfunction f() end\n";
        assert_eq!(lines(src), vec![1]);
    }
}
