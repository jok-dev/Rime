//! Preorder walkers over one body's statements and expressions.
//!
//! Purely syntactic rules (duplicate keys, the selene-parity lint families)
//! all need the same traversal: every statement in a body, every expression
//! inside each statement, in source order. Before this module each rule
//! hand-rolled ~100 lines of it; now a rule is its detection logic plus one
//! closure.
//!
//! Scope discipline: nested closures (`Expr::Function`) are separate arena
//! bodies and are NOT entered — `check_body` visits each body independently,
//! and a walker that recursed into them would double-report. `LocalFunction`
//! / `FunctionDecl` statements are still yielded to the statement callback
//! (their bodies are not entered, same reason).

use crate::{Block, Body, Expr, Stmt, StmtKind, TableField, Target, Terminator};

/// Every statement in the body, preorder, including statements nested in
/// blocks (if arms, loop bodies, `do` blocks). Nested function bodies are
/// not entered.
pub fn stmts(body: &Body, f: &mut impl FnMut(&Stmt)) {
    stmts_in_block(&body.block, f);
}

/// Every expression in the body in source order, preorder (an expression is
/// yielded before its subexpressions). Includes `return` expressions.
/// Nested function bodies are not entered.
pub fn exprs(body: &Body, f: &mut impl FnMut(&Expr)) {
    exprs_in_block(&body.block, f);
}

/// Every block in the body: the root block, then nested ones preorder.
/// For rules about statement ADJACENCY (consecutive-statement patterns),
/// which the flattened `stmts` walk cannot express.
pub fn blocks(body: &Body, f: &mut impl FnMut(&Block)) {
    fn go(block: &Block, f: &mut impl FnMut(&Block)) {
        f(block);
        for stmt in &block.stmts {
            each_child_block(&stmt.kind, &mut |b| go(b, f));
        }
    }
    go(&body.block, f);
}

/// Every statement in the block and its nested blocks, preorder.
pub fn stmts_in_block(block: &Block, f: &mut impl FnMut(&Stmt)) {
    for stmt in &block.stmts {
        f(stmt);
        each_child_block(&stmt.kind, &mut |b| stmts_in_block(b, f));
    }
}

/// Every expression in the block and its nested blocks, in source order —
/// an `elseif` condition comes after the preceding arm's block, a `repeat`
/// condition after its body.
pub fn exprs_in_block(block: &Block, f: &mut impl FnMut(&Expr)) {
    use StmtKind as K;
    for stmt in &block.stmts {
        match &stmt.kind {
            K::If { cond, then, else_ifs, or_else } => {
                expr(cond, f);
                exprs_in_block(then, f);
                for (c, b) in else_ifs {
                    expr(c, f);
                    exprs_in_block(b, f);
                }
                if let Some(b) = or_else {
                    exprs_in_block(b, f);
                }
            }
            K::Repeat { body, until } => {
                exprs_in_block(body, f);
                expr(until, f);
            }
            other => {
                exprs_in_stmt(other, f);
                each_child_block(other, &mut |b| exprs_in_block(b, f));
            }
        }
    }
    if let Some(Terminator::Return { exprs, .. }) = &block.terminator {
        for e in exprs {
            expr(e, f);
        }
    }
}

fn each_child_block<'a>(kind: &'a StmtKind, f: &mut impl FnMut(&'a Block)) {
    use StmtKind as K;
    match kind {
        K::Do(b)
        | K::While { body: b, .. }
        | K::Repeat { body: b, .. }
        | K::NumericFor { body: b, .. }
        | K::GenericFor { body: b, .. } => f(b),
        K::If { then, else_ifs, or_else, .. } => {
            f(then);
            for (_, b) in else_ifs {
                f(b);
            }
            if let Some(b) = or_else {
                f(b);
            }
        }
        K::Local { .. }
        | K::Assign { .. }
        | K::CompoundAssign { .. }
        | K::Call(_)
        | K::LocalFunction { .. }
        | K::FunctionDecl { .. } => {}
    }
}

/// The expressions directly inside one statement (not those of nested
/// statements), each walked preorder. For an `if`/loop this is the
/// condition or bounds — bodies are separate blocks.
pub fn exprs_in_stmt(kind: &StmtKind, f: &mut impl FnMut(&Expr)) {
    use StmtKind as K;
    match kind {
        K::Local { init, .. } => {
            for e in init {
                expr(e, f);
            }
        }
        K::Assign { targets, values } => {
            for t in targets {
                if let Target::Projection(e) = t {
                    expr(e, f);
                }
            }
            for e in values {
                expr(e, f);
            }
        }
        K::CompoundAssign { target, value } => {
            if let Target::Projection(e) = target {
                expr(e, f);
            }
            expr(value, f);
        }
        K::Call(e) => expr(e, f),
        K::If { cond, else_ifs, .. } => {
            expr(cond, f);
            for (c, _) in else_ifs {
                expr(c, f);
            }
        }
        K::While { cond, .. } => expr(cond, f),
        K::Repeat { until, .. } => expr(until, f),
        K::NumericFor { start, end, step, .. } => {
            expr(start, f);
            expr(end, f);
            if let Some(s) = step {
                expr(s, f);
            }
        }
        K::GenericFor { exprs, .. } => {
            for e in exprs {
                expr(e, f);
            }
        }
        K::Do(_) | K::LocalFunction { .. } | K::FunctionDecl { .. } => {}
    }
}

/// One expression and all its subexpressions, preorder. Nested function
/// bodies are not entered.
pub fn expr(e: &Expr, f: &mut impl FnMut(&Expr)) {
    f(e);
    use Expr as E;
    match e {
        E::Call { base, args, .. } => {
            expr(base, f);
            for a in args {
                expr(a, f);
            }
        }
        E::Index { base, key, .. } => {
            expr(base, f);
            if let Some(k) = key {
                expr(k, f);
            }
        }
        E::BinOp { lhs, rhs, .. } => {
            expr(lhs, f);
            expr(rhs, f);
        }
        E::Not(inner) | E::UnOp { operand: inner, .. } => expr(inner, f),
        E::Table(fields) => {
            for field in fields {
                match field {
                    TableField::Keyed { key, value } => {
                        expr(key, f);
                        expr(value, f);
                    }
                    TableField::Named { value, .. } => expr(value, f),
                    TableField::Item(v) => expr(v, f),
                }
            }
        }
        E::IfExpr(arms) => {
            for (cond, val) in arms {
                if let Some(c) = cond {
                    expr(c, f);
                }
                expr(val, f);
            }
        }
        E::Composite(parts) => {
            for p in parts {
                expr(p, f);
            }
        }
        E::Name { .. } | E::Function(_) | E::Literal { .. } | E::Leaf => {}
    }
}
