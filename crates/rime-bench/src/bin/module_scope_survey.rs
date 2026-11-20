//! What files do at their top level, measured over a whole codebase. Fed the
//! design of R0312 `module-scope-computation`: which shapes
//! and callees are common enough to need an allow-list entry, and how files
//! that are not modules can be told apart.
//!
//! HIR-based, same lowering the checker uses. Writes one tab-separated line
//! per event to stdout:
//!
//!   file  line  file_kind  event  detail  position  in_loop  in_if  in_call  source_line
//!
//! `file_kind`: `script` (stem ends `.server` / `.client`), `module` (the
//! top level ends in `return <value>`), `no-return` (neither). The rule
//! built from this is wider on both: it also takes `.plugin` for a script,
//! and a `return <value>` inside a nested block for a module. So a file
//! this calls `no-return` can be a module to the rule.
//! `event`: `call` (detail = callee), `loop` (detail = `for-in`, `for-num`,
//! `while`, `repeat`), `if`, `assign`, `compound-assign`, `operator`.
//! `position`: the kind of top-level statement the event sits in.
//!
//! A `file` line per file (event `file`) carries the totals needed to count
//! files that do nothing at all.

use rime_hir as hir;
use std::path::PathBuf;

struct Ctx<'a> {
    file: &'a str,
    kind: &'static str,
    src: &'a str,
    line_starts: Vec<usize>,
    in_loop: bool,
    in_if: bool,
    events: usize,
}

impl Ctx<'_> {
    fn line_of(&self, offset: usize) -> usize {
        self.line_starts.partition_point(|&s| s <= offset)
    }

    fn emit(&mut self, offset: usize, event: &str, detail: &str, position: &str, in_call: bool) {
        self.events += 1;
        let line = self.line_of(offset);
        let text = self.src.lines().nth(line.saturating_sub(1)).unwrap_or("").trim();
        let text: String = text.chars().take(160).collect::<String>().replace('\t', " ");
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.file, line, self.kind, event, detail, position, self.in_loop, self.in_if, in_call, text
        );
    }
}

/// The callee as written: `a.b.c`, `a.b:m`, `f().x:m`, `<function>`.
fn callee_text(e: &hir::Expr) -> String {
    match e {
        hir::Expr::Name { name, .. } => name.clone(),
        hir::Expr::Index { base, name: Some(n), .. } => format!("{}.{}", callee_text(base), n),
        hir::Expr::Index { base, .. } => format!("{}[]", callee_text(base)),
        hir::Expr::Call { base, method, .. } => match method {
            Some(m) => format!("{}:{}()", callee_text(base), m),
            None => format!("{}()", callee_text(base)),
        },
        hir::Expr::Function(_) => "<function>".to_string(),
        _ => "<expr>".to_string(),
    }
}

fn expr_start(e: &hir::Expr) -> Option<usize> {
    match e {
        hir::Expr::Name { span, .. } | hir::Expr::Call { span, .. } | hir::Expr::Literal { span, .. } => {
            Some(span.start)
        }
        hir::Expr::UnOp { span, .. } => Some(span.start),
        hir::Expr::Index { base, .. } => expr_start(base),
        hir::Expr::BinOp { lhs, .. } => expr_start(lhs),
        _ => None,
    }
}

fn walk_expr(e: &hir::Expr, c: &mut Ctx<'_>, position: &str, stmt_at: usize, in_call: bool) {
    use hir::Expr as E;
    match e {
        E::Call { base, args, method, span, .. } => {
            let callee = match method {
                Some(m) => format!("{}:{}", callee_text(base), m),
                None => callee_text(base),
            };
            c.emit(span.start, "call", &callee, position, in_call);
            walk_expr(base, c, position, stmt_at, in_call);
            for a in args {
                walk_expr(a, c, position, stmt_at, true);
            }
        }
        E::Index { base, key, .. } => {
            walk_expr(base, c, position, stmt_at, in_call);
            if let Some(k) = key {
                walk_expr(k, c, position, stmt_at, in_call);
            }
        }
        E::BinOp { op, lhs, rhs } => {
            use hir::BinOp as B;
            if !matches!(op, B::And | B::Or | B::Eq | B::Ne) {
                c.emit(expr_start(e).unwrap_or(stmt_at), "operator", &format!("{op:?}"), position, in_call);
            }
            walk_expr(lhs, c, position, stmt_at, in_call);
            walk_expr(rhs, c, position, stmt_at, in_call);
        }
        E::Not(inner) | E::UnOp { operand: inner, .. } => walk_expr(inner, c, position, stmt_at, in_call),
        E::Table(fields) => {
            for f in fields {
                match f {
                    hir::TableField::Keyed { key, value } => {
                        walk_expr(key, c, position, stmt_at, in_call);
                        walk_expr(value, c, position, stmt_at, in_call);
                    }
                    hir::TableField::Named { value, .. } => walk_expr(value, c, position, stmt_at, in_call),
                    hir::TableField::Item(v) => walk_expr(v, c, position, stmt_at, in_call),
                }
            }
        }
        E::IfExpr(arms) => {
            for (cond, val) in arms {
                if let Some(cond) = cond {
                    walk_expr(cond, c, position, stmt_at, in_call);
                }
                walk_expr(val, c, position, stmt_at, in_call);
            }
        }
        E::Composite(parts) => {
            for p in parts {
                walk_expr(p, c, position, stmt_at, in_call);
            }
        }
        E::Name { .. } | E::Function(_) | E::Literal { .. } | E::Leaf => {}
    }
}

fn walk_block(block: &hir::Block, c: &mut Ctx<'_>) {
    use hir::StmtKind as K;
    for stmt in &block.stmts {
        let at = stmt.span.start;
        match &stmt.kind {
            K::Local { init, .. } => {
                for e in init {
                    walk_expr(e, c, "local", at, false);
                }
            }
            K::Assign { targets, values } => {
                let shape = match targets.first() {
                    Some(hir::Target::Name { .. }) => "name".to_string(),
                    Some(hir::Target::Projection(e)) => match e {
                        hir::Expr::Index { name: Some(_), .. } => "field".to_string(),
                        _ => "index".to_string(),
                    },
                    None => "none".to_string(),
                };
                c.emit(at, "assign", &shape, "assign", false);
                for t in targets {
                    if let hir::Target::Projection(e) = t {
                        walk_expr(e, c, "assign", at, false);
                    }
                }
                for e in values {
                    walk_expr(e, c, "assign", at, false);
                }
            }
            K::CompoundAssign { target, value } => {
                c.emit(at, "compound-assign", "", "assign", false);
                if let hir::Target::Projection(e) = target {
                    walk_expr(e, c, "assign", at, false);
                }
                walk_expr(value, c, "assign", at, false);
            }
            K::Call(e) => walk_expr(e, c, "statement", at, false),
            K::Do(b) => walk_block(b, c),
            K::If { cond, then, else_ifs, or_else } => {
                c.emit(at, "if", "", "if", false);
                walk_expr(cond, c, "condition", at, false);
                let was = std::mem::replace(&mut c.in_if, true);
                walk_block(then, c);
                for (cond, b) in else_ifs {
                    walk_expr(cond, c, "condition", at, false);
                    walk_block(b, c);
                }
                if let Some(b) = or_else {
                    walk_block(b, c);
                }
                c.in_if = was;
            }
            K::While { cond, body } => {
                c.emit(at, "loop", "while", "loop", false);
                let was = std::mem::replace(&mut c.in_loop, true);
                walk_expr(cond, c, "condition", at, false);
                walk_block(body, c);
                c.in_loop = was;
            }
            K::Repeat { body, until } => {
                c.emit(at, "loop", "repeat", "loop", false);
                let was = std::mem::replace(&mut c.in_loop, true);
                walk_block(body, c);
                walk_expr(until, c, "condition", at, false);
                c.in_loop = was;
            }
            K::NumericFor { start, end, step, body, .. } => {
                c.emit(at, "loop", "for-num", "loop", false);
                walk_expr(start, c, "loop-header", at, false);
                walk_expr(end, c, "loop-header", at, false);
                if let Some(s) = step {
                    walk_expr(s, c, "loop-header", at, false);
                }
                let was = std::mem::replace(&mut c.in_loop, true);
                walk_block(body, c);
                c.in_loop = was;
            }
            K::GenericFor { exprs, body, .. } => {
                c.emit(at, "loop", "for-in", "loop", false);
                for e in exprs {
                    walk_expr(e, c, "loop-header", at, false);
                }
                let was = std::mem::replace(&mut c.in_loop, true);
                walk_block(body, c);
                c.in_loop = was;
            }
            K::LocalFunction { .. } | K::FunctionDecl { .. } => {}
        }
    }
    if let Some(hir::Terminator::Return { exprs, span }) = &block.terminator {
        for e in exprs {
            walk_expr(e, c, "return", span.start, false);
        }
    }
}

fn main() {
    // The parser recurses deeply; the main thread's stack is too small for
    // real files (see `WORK_STACK_BYTES` in rime-cli).
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(survey)
        .expect("spawn")
        .join()
        .expect("survey thread");
}

fn survey() {
    let dir = std::env::args().nth(1).expect("usage: rime-module-scope-survey <dir>");
    let root = PathBuf::from(&dir);
    for entry in walkdir::WalkDir::new(&root)
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "luau" || x == "lua"))
    {
        let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
        let file = rel.to_string_lossy().replace('\\', "/");
        let Ok(src) = std::fs::read_to_string(entry.path()) else { continue };
        let Ok(module) = rime_syntax::parse(&src) else {
            println!("{file}\t0\tparse-fail\tfile\t0\t\tfalse\tfalse\tfalse\t");
            continue;
        };
        let stem = entry.path().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let body = module.body(module.root);
        let returns_value = matches!(
            &body.block.terminator,
            Some(hir::Terminator::Return { exprs, .. }) if !exprs.is_empty()
        );
        let kind = if stem.ends_with(".server") || stem.ends_with(".client") {
            "script"
        } else if returns_value {
            "module"
        } else {
            "no-return"
        };
        let mut line_starts = vec![0usize];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        let mut c = Ctx { file: &file, kind, src: &src, line_starts, in_loop: false, in_if: false, events: 0 };
        walk_block(&body.block, &mut c);
        let events = c.events;
        println!("{file}\t0\t{kind}\tfile\t{events}\t\tfalse\tfalse\tfalse\t");
    }
}
