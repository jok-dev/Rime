//! Idiom counts over a codebase, feeding the exported-function-reassign
//! design decision. HIR-based, not grep — same lowering the
//! checker uses. Measures:
//!
//!   1. Export style — of files that return a value (module scripts), how
//!      many return a frozen table vs an unfrozen one vs something else.
//!   2. Own-module function-slot reassignment — a slot declared by
//!      `function M.f()` / `M.f = function` assigned AGAIN later in the file.
//!   3. Writes to require-bound module fields — `X.f = ...` where
//!      `local X = require(...)` (the cross-file half of the same rule).
//!   4. Local-function reassignment — `local function f` (or
//!      `local f = function`) later reassigned in the same body. Name-based,
//!      shadowing not modelled: an overcount, good enough for a survey.

use rime_hir as hir;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Default)]
struct Tally {
    files: usize,
    parse_fail: usize,
    // 1. export style
    returns_value: usize,
    export_frozen: usize,
    export_plain_table: usize,
    export_other: usize,
    // 2. own-module slot reassignment
    slot_reassign_events: usize,
    slot_reassign_files: HashSet<String>,
    slot_reassign_samples: Vec<String>,
    // 3. require-bound writes
    require_write_events: usize,
    require_fn_write_events: usize,
    require_write_files: HashSet<String>,
    require_write_samples: Vec<String>,
    // 4. local function reassignment
    local_fn_reassign_events: usize,
    local_fn_reassign_files: HashSet<String>,
    local_fn_reassign_samples: Vec<String>,
    // 5. shadowing (local re-declares a name visible from an outer scope of
    // the same body; `_`-named bindings exempt)
    shadow_same_scope: usize,
    shadow_nested_scope: usize,
    shadow_param: usize,
    shadow_files: HashSet<String>,
    shadow_samples: Vec<String>,
}

fn main() {
    let dir = std::env::args().nth(1).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../corpus")
            .to_string_lossy()
            .into_owned()
    });

    let mut t = Tally::default();

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
        t.files += 1;
        let Ok(module) = rime_syntax::parse(&src) else {
            t.parse_fail += 1;
            continue;
        };
        survey_file(&module, &src, &path, &mut t);
    }

    println!("files scanned          : {}", t.files);
    println!("parse failures         : {}", t.parse_fail);
    println!();
    println!("1. EXPORT STYLE ({} files return a value)", t.returns_value);
    println!("   frozen table        : {}", t.export_frozen);
    println!("   unfrozen table      : {}", t.export_plain_table);
    println!("   other (fn/call/...) : {}", t.export_other);
    println!();
    println!(
        "2. OWN-MODULE FUNCTION-SLOT REASSIGNMENT: {} events in {} files",
        t.slot_reassign_events,
        t.slot_reassign_files.len()
    );
    for s in t.slot_reassign_samples.iter().take(10) {
        println!("   {s}");
    }
    println!();
    println!(
        "3. WRITES TO REQUIRE-BOUND MODULE FIELDS: {} events in {} files ({} assign a function literal)",
        t.require_write_events,
        t.require_write_files.len(),
        t.require_fn_write_events
    );
    for s in t.require_write_samples.iter().take(10) {
        println!("   {s}");
    }
    println!();
    println!(
        "4. LOCAL-FUNCTION REASSIGNMENT: {} events in {} files",
        t.local_fn_reassign_events,
        t.local_fn_reassign_files.len()
    );
    for s in t.local_fn_reassign_samples.iter().take(10) {
        println!("   {s}");
    }
    println!();
    println!(
        "5. SHADOWING: {} same-scope + {} nested-scope + {} param, in {} files",
        t.shadow_same_scope,
        t.shadow_nested_scope,
        t.shadow_param,
        t.shadow_files.len()
    );
    for s in t.shadow_samples.iter().take(15) {
        println!("   {s}");
    }
}

/// Scope-stack walk of one body counting shadowing declarations.
/// Frame 0 holds the params.
fn shadow_scan(
    body: &rime_hir::Body,
    src: &str,
    path: &str,
    t: &mut Tally,
) {
    fn declare(
        name: &str,
        span_start: usize,
        scopes: &mut Vec<Vec<String>>,
        src: &str,
        path: &str,
        t: &mut Tally,
    ) {
        if name == "_" {
            scopes.last_mut().unwrap().push(name.to_string());
            return;
        }
        let depth = scopes.len();
        for (i, frame) in scopes.iter().enumerate() {
            if frame.iter().any(|n| n == name) {
                if i == 0 && depth > 1 {
                    t.shadow_param += 1;
                } else if i + 1 == depth {
                    t.shadow_same_scope += 1;
                } else {
                    t.shadow_nested_scope += 1;
                }
                t.shadow_files.insert(path.to_string());
                if t.shadow_samples.len() < 60 {
                    let kind = if i == 0 && depth > 1 {
                        "param"
                    } else if i + 1 == depth {
                        "same-scope"
                    } else {
                        "nested"
                    };
                    t.shadow_samples.push(format!(
                        "{path}:{} `{name}` [{kind}]",
                        line_of(src, span_start)
                    ));
                }
                break;
            }
        }
        scopes.last_mut().unwrap().push(name.to_string());
    }

    fn walk(
        block: &rime_hir::Block,
        scopes: &mut Vec<Vec<String>>,
        src: &str,
        path: &str,
        t: &mut Tally,
    ) {
        use rime_hir::StmtKind as K;
        scopes.push(Vec::new());
        for stmt in &block.stmts {
            match &stmt.kind {
                K::Local { decls, .. } => {
                    for d in decls {
                        declare(&d.name, d.span.start, scopes, src, path, t);
                    }
                }
                K::LocalFunction { decl, .. } => {
                    declare(&decl.name, decl.span.start, scopes, src, path, t);
                }
                K::NumericFor { var, body, .. } => {
                    scopes.push(Vec::new());
                    declare(&var.name, var.span.start, scopes, src, path, t);
                    walk(body, scopes, src, path, t);
                    scopes.pop();
                }
                K::GenericFor { vars, body, .. } => {
                    scopes.push(Vec::new());
                    for v in vars {
                        declare(&v.name, v.span.start, scopes, src, path, t);
                    }
                    walk(body, scopes, src, path, t);
                    scopes.pop();
                }
                K::Do(b) | K::While { body: b, .. } | K::Repeat { body: b, .. } => {
                    walk(b, scopes, src, path, t)
                }
                K::If { then, else_ifs, or_else, .. } => {
                    walk(then, scopes, src, path, t);
                    for (_, b) in else_ifs {
                        walk(b, scopes, src, path, t);
                    }
                    if let Some(b) = or_else {
                        walk(b, scopes, src, path, t);
                    }
                }
                _ => {}
            }
        }
        scopes.pop();
    }

    let mut scopes: Vec<Vec<String>> = vec![body.params.iter().map(|p| p.name.clone()).collect()];
    walk(&body.block, &mut scopes, src, path, t);
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset.min(src.len())].matches('\n').count() + 1
}

fn survey_file(module: &hir::Module, src: &str, path: &str, t: &mut Tally) {
    let root = module.body(module.root);

    // ---- collect, over the whole file -----------------------------------
    // slot declarations: (table, field) -> assignment count
    let mut slots: HashMap<(String, String), usize> = HashMap::new();
    let mut slot_hits: Vec<(String, String, usize)> = Vec::new(); // reassign sites
    let mut requires: HashSet<String> = HashSet::new();
    let mut require_hits: Vec<(usize, bool)> = Vec::new();
    let mut frozen_locals: HashSet<String> = HashSet::new();

    // Top-level declarations first: requires + `local M = table.freeze(..)`.
    for stmt in &root.block.stmts {
        if let hir::StmtKind::Local { decls, init } = &stmt.kind {
            for (d, e) in decls.iter().zip(init) {
                if is_require_call(e) {
                    requires.insert(d.name.clone());
                }
                if is_freeze_call(e) {
                    frozen_locals.insert(d.name.clone());
                }
            }
        }
        // statement-position `table.freeze(M)`
        if let hir::StmtKind::Call(e) = &stmt.kind {
            if let Some(arg) = freeze_arg(e) {
                frozen_locals.insert(arg.to_string());
            }
        }
    }

    for body in &module.bodies {
        shadow_scan(body, src, path, t);
    }

    // Walk every statement in every body.
    let mut local_fn_state: Vec<(HashSet<String>, Vec<usize>)> = Vec::new();
    for body in &module.bodies {
        let mut fn_locals: HashSet<String> = HashSet::new();
        let mut reassigns: Vec<usize> = Vec::new();
        walk_block(
            &body.block,
            &mut |stmt| {
                match &stmt.kind {
                    hir::StmtKind::FunctionDecl { root: r, path: p, .. } => {
                        if p.len() == 1 {
                            let key = (r.clone(), p[0].clone());
                            let n = slots.entry(key.clone()).or_insert(0);
                            *n += 1;
                            if *n > 1 {
                                slot_hits.push((key.0, key.1, stmt.span.start));
                            }
                        }
                    }
                    hir::StmtKind::LocalFunction { decl, .. } => {
                        fn_locals.insert(decl.name.clone());
                    }
                    hir::StmtKind::Local { decls, init } => {
                        for (d, e) in decls.iter().zip(init) {
                            if matches!(e, hir::Expr::Function(_)) {
                                fn_locals.insert(d.name.clone());
                            }
                        }
                    }
                    hir::StmtKind::Assign { targets, values } => {
                        for (i, target) in targets.iter().enumerate() {
                            match target {
                                hir::Target::Name { name, .. } => {
                                    if fn_locals.contains(name) {
                                        reassigns.push(stmt.span.start);
                                    }
                                }
                                hir::Target::Projection(e) => {
                                    if let Some((tbl, field)) = one_level_field(e) {
                                        if requires.contains(&tbl) {
                                            let is_fn = matches!(
                                                values.get(i),
                                                Some(hir::Expr::Function(_))
                                            );
                                            require_hits.push((stmt.span.start, is_fn));
                                        }
                                        let key = (tbl, field);
                                        let is_fn_value = matches!(
                                            values.get(i),
                                            Some(hir::Expr::Function(_))
                                        );
                                        let known = slots.contains_key(&key);
                                        if known || is_fn_value {
                                            let n = slots.entry(key.clone()).or_insert(0);
                                            *n += 1;
                                            if *n > 1 {
                                                slot_hits.push((key.0, key.1, stmt.span.start));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            },
        );
        local_fn_state.push((fn_locals, reassigns));
    }

    // ---- export style ----------------------------------------------------
    if let Some(hir::Terminator::Return { exprs, .. }) = &root.block.terminator {
        if !exprs.is_empty() {
            t.returns_value += 1;
            let e = &exprs[0];
            if is_freeze_call(e) {
                t.export_frozen += 1;
            } else if let hir::Expr::Name { name, .. } = e {
                if frozen_locals.contains(name) {
                    t.export_frozen += 1;
                } else if slots.keys().any(|(tbl, _)| tbl == name) {
                    // has function slots -> it's a module table, unfrozen
                    t.export_plain_table += 1;
                } else {
                    // returned local with no known function slots: count as
                    // plain table if it was declared from a composite, else
                    // other. We cannot see constructors distinctly; be
                    // conservative and call it other.
                    t.export_other += 1;
                }
            } else {
                t.export_other += 1;
            }
        }
    }

    // ---- tallies ---------------------------------------------------------
    for (tbl, field, off) in slot_hits {
        t.slot_reassign_events += 1;
        t.slot_reassign_files.insert(path.to_string());
        if t.slot_reassign_samples.len() < 40 {
            t.slot_reassign_samples
                .push(format!("{path}:{} {tbl}.{field}", line_of(src, off)));
        }
    }
    for (off, is_fn) in require_hits {
        if is_fn {
            t.require_fn_write_events += 1;
        }
        t.require_write_events += 1;
        t.require_write_files.insert(path.to_string());
        if t.require_write_samples.len() < 40 {
            let tag = if is_fn { " [FUNCTION]" } else { "" };
            t.require_write_samples
                .push(format!("{path}:{}{tag}", line_of(src, off)));
        }
    }
    for (_, reassigns) in local_fn_state {
        for off in reassigns {
            t.local_fn_reassign_events += 1;
            t.local_fn_reassign_files.insert(path.to_string());
            if t.local_fn_reassign_samples.len() < 40 {
                t.local_fn_reassign_samples
                    .push(format!("{path}:{}", line_of(src, off)));
            }
        }
    }
}

fn walk_block(block: &hir::Block, f: &mut impl FnMut(&hir::Stmt)) {
    for stmt in &block.stmts {
        f(stmt);
        use hir::StmtKind as K;
        match &stmt.kind {
            K::Do(b) | K::While { body: b, .. } | K::Repeat { body: b, .. } => walk_block(b, f),
            K::NumericFor { body, .. } | K::GenericFor { body, .. } => walk_block(body, f),
            K::If { then, else_ifs, or_else, .. } => {
                walk_block(then, f);
                for (_, b) in else_ifs {
                    walk_block(b, f);
                }
                if let Some(b) = or_else {
                    walk_block(b, f);
                }
            }
            _ => {}
        }
    }
}

/// `t.f` (dot, one level) → (t, f)
fn one_level_field(e: &hir::Expr) -> Option<(String, String)> {
    let hir::Expr::Index { base, key: None, name: Some(field), .. } = e else {
        return None;
    };
    let hir::Expr::Name { name, .. } = &**base else {
        return None;
    };
    Some((name.clone(), field.clone()))
}

fn is_require_call(e: &hir::Expr) -> bool {
    let hir::Expr::Call { base, .. } = e else {
        return false;
    };
    matches!(&**base, hir::Expr::Name { name, .. } if name == "require")
}

fn is_freeze_call(e: &hir::Expr) -> bool {
    freeze_arg(e).is_some()
}

/// If `e` is `table.freeze(x)` with `x` a plain name, return `x`.
fn freeze_arg(e: &hir::Expr) -> Option<&str> {
    let hir::Expr::Call { base, args, .. } = e else {
        return None;
    };
    let hir::Expr::Index { base: tbl, key: None, name: Some(field), .. } = &**base else {
        return None;
    };
    let hir::Expr::Name { name, .. } = &**tbl else {
        return None;
    };
    if name != "table" || field != "freeze" {
        return None;
    }
    match args.first() {
        Some(hir::Expr::Name { name, .. }) => Some(name),
        _ => Some(""), // freeze of a literal/expression — still a freeze
    }
}
