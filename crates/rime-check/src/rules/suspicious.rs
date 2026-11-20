//! R0601–R0610: the suspicious family — selene-parity lints (decided
//! 2026-08-21) that are pure HIR walks. Every pattern here is valid Luau
//! that is far more often a slip than intent: error severity (the family
//! decision — suspicious = error, style = warn), and every diagnostic
//! carries `conservative: true` because Rime cannot prove which — except
//! where the outcome is the same whatever the program does: R0610, and
//! R0607 on `{} == {}` and on an ordering operator. R0602 is the one
//! warning: dividing by zero is defined in Luau (`inf`, `-inf`, `nan`) and
//! a program may mean it.
//!
//!   R0601 almost-swapped            `a = b` then `b = a` (names, fields, indexes)
//!   R0602 divide-by-zero            `x / 0`, `x % 0`, `x // 0`
//!   R0603 if-same-then-else         two identical `if` branches
//!   R0604 ifs-same-cond             an `elseif` repeating an earlier cond
//!   R0605 suspicious-reverse-loop   `for i = #t, 1 do` / `#t, -1, 1` / `8, 1`
//!   R0606 type-check-inside-call    `type(x == "number")`
//!   R0607 constant-table-comparison `t == {}`, `t < {}`
//!   R0608 unbalanced-assignments    name/value count mismatch
//!   R0609 suspicious-for-step       `for i = 1, 8, -1` / a step of 0
//!   R0610 compare-nan               `x == 0 / 0`
//!
//! R0611 bad-string-escape is in `strings.rs`: it reads string tokens, not
//! the tree.
//!
//! Equality here is STRICT structural equality: unmodelled expressions
//! (`Leaf`, `Composite`) and closures compare unequal, so the equality
//! lints stay silent rather than wrong — silence, never a wrong fact.

use rime_diag::{Code, Diagnostic, Label, Suggestion};
use rime_hir::{self as hir, visit, Span};
use std::collections::HashSet;

pub fn check(body: &hir::Body, src: &str, diags: &mut Vec<Diagnostic>) {
    visit::blocks(body, &mut |block| almost_swapped(block, src, diags));
    let mut type_checks = Vec::new();
    visit::exprs(body, &mut |e| {
        divide_by_zero(e, diags);
        type_checks.extend(type_check_inside_call(e));
        compare_nan(e, src, diags);
    });
    // R0607 walks statement by statement: `{} == {}` has no span of its
    // own and is placed from the statement around it.
    let mut placed = HashSet::new();
    visit::blocks(body, &mut |block| {
        for stmt in &block.stmts {
            visit::exprs_in_stmt(&stmt.kind, &mut |e| {
                constant_table_comparison(e, &stmt.span, src, &mut placed, diags)
            });
        }
        if let Some(hir::Terminator::Return { exprs, span }) = &block.terminator {
            for e in exprs {
                visit::expr(e, &mut |e| constant_table_comparison(e, span, src, &mut placed, diags));
            }
        }
    });
    if !type_checks.is_empty() {
        // The slot walk runs only for a body that has a hit.
        let slots = call_slots(body);
        for t in &type_checks {
            diags.push(type_check_diag(t, src, &slots));
        }
    }
    visit::stmts(body, &mut |stmt| {
        if let hir::StmtKind::If { cond, then, else_ifs, or_else } = &stmt.kind {
            if_same_then_else(then, else_ifs, or_else.as_ref(), diags);
            ifs_same_cond(cond, else_ifs, diags);
        }
        suspicious_for_range(stmt, src, diags);
        unbalanced_assignments(stmt, diags);
    });
}

// --- R0601 almost-swapped ------------------------------------------------

/// `a = b` immediately followed by `b = a`, where each side is a name, a
/// field or an index expression (`t.x`, `t[i]`). After the first statement
/// `a` already holds `b`'s value, so the second assigns `b` its own value
/// back: the pair reads as a swap and performs a copy.
///
/// Silent when that is not what happens: a call on either side may return
/// something else the second time, and when one side is part of the other
/// (`i = t[i]` then `t[i] = i`) the first write moves where the second
/// one lands.
fn almost_swapped(block: &hir::Block, src: &str, diags: &mut Vec<Diagnostic>) {
    for pair in block.stmts.windows(2) {
        let (Some((a, b)), Some((b2, a2))) = (single_assignment(&pair[0]), single_assignment(&pair[1]))
        else {
            continue;
        };
        if !places_equal(a, a2) || !places_equal(b, b2) || places_equal(a, b) {
            continue;
        }
        if place_has_call(a) || place_has_call(b) || place_mentions(a, b) || place_mentions(b, a) {
            continue;
        }
        let plain_names = matches!((a, b), (Place::Name(_), Place::Name(_)));
        // Each side as written, for the edit. The two statements must spell
        // the sides the same way, or there is no one spelling to keep.
        let written = match (assignment_sides(src, &pair[0].span), assignment_sides(src, &pair[1].span)) {
            (Some((a1, b1)), Some((b2, a2))) if a1 == a2 && b1 == b2 => Some((a1, b1)),
            _ => None,
        };
        let (a, b) = (describe_place(a), describe_place(b));
        let swap = {
            let (a, b) = written.unwrap_or((&a, &b));
            format!("{a}, {b} = {b}, {a}")
        };
        let fix = format!("swap in one statement: `{swap}`");
        // The edit replaces everything from the first statement through
        // the second. Only whitespace and `;` may sit between them: a
        // comment there would be deleted with the code around it.
        let adjacent = src
            .get(pair[0].span.end..pair[1].span.start)
            .is_some_and(|gap| gap.chars().all(|c| c.is_whitespace() || c == ';'));
        let d = Diagnostic::error(Code(601), format!("`{a}` and `{b}` do not swap"))
            .with_label(Label::primary(
                pair[1].span.clone(),
                format!("`{a}` already holds `{b}`'s value, so this is a no-op"),
            ))
            .with_label(Label::secondary(
                pair[0].span.clone(),
                format!("`{a}` takes `{b}`'s value here"),
            ))
            .with_note(format!(
                "after the first assignment both {}hold `{b}`'s value; \
                 `{a}`'s original value is lost",
                if plain_names { "names " } else { "" },
            ))
            .conservative(if plain_names {
                "valid Luau — but the second assignment does nothing, which \
                 is almost never what was meant"
            } else {
                "valid Luau — but unless an `__index` or `__newindex` \
                 metamethod steps in, the second assignment does nothing, \
                 which is almost never what was meant"
            });
        diags.push(if adjacent && written.is_some() {
            d.with_suggestion(Suggestion {
                message: fix,
                span: pair[0].span.start..pair[1].span.end,
                replacement: swap,
            })
        } else {
            d.with_help(fix)
        });
    }
}

/// One side of an assignment, as somewhere a value is kept: a name, or a
/// field or index expression.
#[derive(Clone, Copy)]
enum Place<'a> {
    Name(&'a str),
    Projection(&'a hir::Expr),
}

/// `target = value` with one target and one value, both places.
fn single_assignment(stmt: &hir::Stmt) -> Option<(Place<'_>, Place<'_>)> {
    let hir::StmtKind::Assign { targets, values } = &stmt.kind else {
        return None;
    };
    let ([target], [value]) = (targets.as_slice(), values.as_slice()) else {
        return None;
    };
    let target = match target {
        hir::Target::Name { name, .. } => Place::Name(name),
        hir::Target::Projection(e) => Place::Projection(e),
    };
    let value = match value {
        hir::Expr::Name { name, .. } => Place::Name(name),
        e @ hir::Expr::Index { .. } => Place::Projection(e),
        _ => return None,
    };
    Some((target, value))
}

fn places_equal(a: Place, b: Place) -> bool {
    match (a, b) {
        (Place::Name(a), Place::Name(b)) => a == b,
        (Place::Projection(a), Place::Projection(b)) => exprs_equal(a, b),
        _ => false,
    }
}

fn place_has_call(p: Place) -> bool {
    matches!(p, Place::Projection(e) if contains_call(e))
}

/// Whether `part` is read somewhere inside `whole`: writing `part` then
/// changes which slot `whole` names.
fn place_mentions(whole: Place, part: Place) -> bool {
    let Place::Projection(whole) = whole else {
        return false;
    };
    let mut found = false;
    visit::expr(whole, &mut |e| {
        found |= match (part, e) {
            (Place::Name(part), hir::Expr::Name { name, .. }) => part == name,
            (Place::Projection(part), e) => exprs_equal(part, e),
            _ => false,
        };
    });
    found
}

/// A place for a message: `a`, `t.x`, `t[1]`, `t["k"]`, `t[i]`. A key that
/// is anything longer is written `…`.
fn describe_place(p: Place) -> String {
    fn expr(e: &hir::Expr) -> String {
        match e {
            hir::Expr::Name { name, .. } => name.clone(),
            hir::Expr::Literal { prim: hir::Prim::String, text: Some(t), .. } => format!("\"{t}\""),
            hir::Expr::Literal { prim: hir::Prim::Number, text: Some(t), .. } => t.clone(),
            hir::Expr::Index { base, name: Some(name), .. } => format!("{}.{name}", expr(base)),
            hir::Expr::Index { base, key: Some(key), .. } => {
                let key = match &**key {
                    hir::Expr::Name { .. } | hir::Expr::Literal { .. } => expr(key),
                    _ => "…".to_string(),
                };
                format!("{}[{key}]", expr(base))
            }
            _ => "…".to_string(),
        }
    }
    match p {
        Place::Name(name) => name.to_string(),
        Place::Projection(e) => expr(e),
    }
}

/// The two sides of a one-target assignment as written: the text before
/// and after its `=`. The `=` is found by bracket depth, skipping quoted
/// strings. None when the statement holds anything that could mislead
/// that — a comment, a long string, an interpolated string.
fn assignment_sides<'s>(src: &'s str, stmt: &Span) -> Option<(&'s str, &'s str)> {
    let text = src.get(stmt.clone())?;
    if text.contains("--") || text.contains("[[") || text.contains("[=") || text.contains('`') {
        return None;
    }
    let bytes = text.as_bytes();
    let (mut depth, mut i) = (0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'"' | b'\'') => {
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            b'=' if depth == 0 => {
                let (target, value) = (text[..i].trim(), text[i + 1..].trim());
                return (!target.is_empty() && !value.is_empty()).then_some((target, value));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

// --- R0602 divide-by-zero ------------------------------------------------

fn divide_by_zero(e: &hir::Expr, diags: &mut Vec<Diagnostic>) {
    use hir::BinOp as Op;
    let hir::Expr::BinOp { op: op @ (Op::Div | Op::FloorDiv | Op::Mod), lhs, rhs } = e else {
        return;
    };
    // A literal zero over a zero is how NaN is written (`local NAN = 0 / 0`
    // — Luau has no `math.nan`). Nothing in it can be a slip about the
    // divisor, and Selene's `divide_by_zero` skips a literal `0` numerator.
    if let hir::Expr::Literal { prim: hir::Prim::Number, text: Some(t), .. } = &**lhs {
        if number_value(t) == Some(0.0) {
            return;
        }
    }
    // See through `-`: `x / -0` is still a zero divisor.
    let mut divisor: &hir::Expr = rhs;
    while let hir::Expr::UnOp { op: hir::UnOp::Neg, operand, .. } = divisor {
        divisor = operand;
    }
    let hir::Expr::Literal { prim: hir::Prim::Number, span, text: Some(t) } = divisor else {
        return;
    };
    if number_value(t) != Some(0.0) {
        return;
    }
    let (word, result) = match op {
        Op::Div => ("division", "`inf`, `-inf`, or `nan`"),
        Op::FloorDiv => ("floor division", "`inf`, `-inf`, or `nan`"),
        Op::Mod => ("modulo", "`nan`"),
        _ => unreachable!(),
    };
    diags.push(
        Diagnostic::warning(Code(602), format!("{word} by zero"))
            .with_label(Label::primary(span.clone(), "the divisor is a literal zero"))
            .with_note(format!(
                "Luau raises no error here — the result is {result}, which then \
                 spreads through every computation that touches it"
            ))
            .conservative(
                "a literal zero divisor is occasionally a deliberate way to \
                 produce `inf` or `nan`; write `math.huge` or `0 / 0` if so \
                 — neither is reported",
            ),
    );
}

// --- R0603 if-same-then-else ---------------------------------------------

/// Only conditional-arm-vs-ELSE pairs are compared. Two conditional arms
/// with the same body but different conditions are the deliberate
/// multi-condition dispatch idiom (large-codebase survey 2026-08-21: the
/// all-pairs hits spot-checked were all that shape — cmdr's word-boundary
/// scanner, promise's duck-typing chain); an arm identical to the
/// FALLBACK, by contrast, selects what would happen anyway.
fn if_same_then_else(
    then: &hir::Block,
    else_ifs: &[(hir::Expr, hir::Block)],
    or_else: Option<&hir::Block>,
    diags: &mut Vec<Diagnostic>,
) {
    let Some(or_else) = or_else else {
        return;
    };
    // Empty pairs are R0701's territory, and an empty block has no span to
    // point at.
    if or_else.stmts.is_empty() && or_else.terminator.is_none() {
        return;
    }
    let arms = std::iter::once(then).chain(else_ifs.iter().map(|(_, b)| b));
    for arm in arms {
        if !blocks_equal(arm, or_else) {
            continue;
        }
        let (Some(arm_span), Some(else_span)) = (block_span(arm), block_span(or_else)) else {
            continue;
        };
        diags.push(
            Diagnostic::error(Code(603), "this branch matches the `else` branch")
                .with_label(Label::primary(
                    arm_span,
                    "identical to the `else` fallback below",
                ))
                .with_label(Label::secondary(else_span, "the same statements run here"))
                .with_note(
                    "when this condition is true the branch does exactly what \
                     the fallback would do — either the branch was meant to \
                     differ, or its condition is unnecessary"
                        .to_string(),
                )
                .conservative(
                    "identical branches are valid — Rime cannot tell a \
                     copy-paste slip from a deliberate placeholder",
                ),
        );
        // No early return: every arm matching the fallback is its own
        // occurrence (CLAUDE.md: one diagnostic per occurrence).
    }
}

// --- R0604 ifs-same-cond -------------------------------------------------

fn ifs_same_cond(
    cond: &hir::Expr,
    else_ifs: &[(hir::Expr, hir::Block)],
    diags: &mut Vec<Diagnostic>,
) {
    let mut conds: Vec<&hir::Expr> = vec![cond];
    conds.extend(else_ifs.iter().map(|(c, _)| c));
    for j in 1..conds.len() {
        for i in 0..j {
            // A call can answer differently each time; only repeated
            // call-free conditions are certainly the same answer.
            if contains_call(conds[i]) || !exprs_equal(conds[i], conds[j]) {
                continue;
            }
            let (Some(first), Some(dup)) = (expr_span(conds[i]), expr_span(conds[j])) else {
                continue;
            };
            diags.push(
                Diagnostic::error(Code(604), "this `elseif` repeats an earlier condition")
                    .with_label(Label::primary(dup, "already checked above"))
                    .with_label(Label::secondary(first, "first checked here"))
                    .with_note(
                        "whenever this condition is true the earlier arm has \
                         already taken the branch, so this arm never runs"
                            .to_string(),
                    )
                    .conservative(
                        "valid Luau — the arm is unreachable unless an `__index` \
                         metamethod answers differently between the two checks",
                    ),
            );
            // One diagnostic per DUPLICATE ARM (the occurrence), citing its
            // first earlier match — not one per (i, j) pair, which would
            // report the same arm several times over.
            break;
        }
    }
}

// --- R0605 suspicious-reverse-loop / R0609 suspicious-for-step -----------
//
// One look at the numeric `for` header, two codes:
//
//   R0605  the range reads as a countdown that forgot its `-1`: a length
//          start (`#t`) with a constant end <= 1, or a constant start above
//          a constant end — with the step absent or positive. `#t` is never
//          negative, so an end below 0 makes the body provably dead
//          (`for i = #t, -1, 1`). Luau's own ForRange lint is silent
//          whenever ANY step is written, so that loop passes it.
//   R0609  the step contradicts the range: a constant 0 (never advances),
//          or a negative step with a constant start below a constant end.
//
// A non-constant step, or a non-constant end under a positive step, stays
// silent — beyond these shapes the user knows best.

/// A numeric literal, possibly under a unary minus: `1`, `-1`, `0x10`.
/// The span covers the sign.
pub(super) fn const_number(e: &hir::Expr) -> Option<(f64, Span)> {
    match e {
        hir::Expr::Literal { prim: hir::Prim::Number, span, text: Some(t) } => {
            number_value(t).map(|v| (v, span.clone()))
        }
        hir::Expr::UnOp { op: hir::UnOp::Neg, operand, span } => {
            const_number(operand).map(|(v, _)| (-v, span.clone()))
        }
        _ => None,
    }
}

fn suspicious_for_range(stmt: &hir::Stmt, src: &str, diags: &mut Vec<Diagnostic>) {
    let hir::StmtKind::NumericFor { start, end, step, .. } = &stmt.kind else {
        return;
    };
    // An absent step is +1; a non-constant one is the user's business.
    let (step_v, step_span) = match step {
        None => (1.0, None),
        Some(s) => match const_number(s) {
            Some((v, span)) => (v, Some(span)),
            None => return,
        },
    };
    let start_c = const_number(start);
    let end_c = const_number(end);

    if step_v == 0.0 {
        let Some(span) = step_span else {
            return;
        };
        diags.push(
            Diagnostic::error(Code(609), "`for` with a step of 0 never advances")
                .with_label(Label::primary(span, "step of 0"))
                .with_note(
                    "Luau skips the body when the start is below the end and \
                     repeats it forever otherwise — neither is a loop"
                        .to_string(),
                )
                .conservative("valid Luau — never useful"),
        );
        return;
    }
    if step_v < 0.0 {
        // Counting down is what a negative step is for; only a constant
        // range that counts UP contradicts it.
        let (Some((a, a_span)), Some((b, b_span)), Some(s_span)) = (start_c, end_c, step_span)
        else {
            return;
        };
        if a >= b {
            return;
        }
        let (a_src, b_src, s_src) =
            (snippet(src, &a_span), snippet(src, &b_span), snippet(src, &s_span));
        diags.push(
            Diagnostic::error(
                Code(609),
                format!("`for` from {a_src} up to {b_src} with a negative step never runs"),
            )
            .with_label(Label::primary(
                s_span,
                "negative step, but the start is below the end",
            ))
            .with_note(format!(
                "a negative step needs the start above the end — did you mean \
                 `{b_src}, {a_src}, {s_src}`, or a positive step?"
            ))
            .conservative("valid Luau — the body is provably dead"),
        );
        return;
    }

    // Positive step: the R0605 countdown shapes need a constant end.
    let Some((end_v, end_span)) = end_c else {
        return;
    };
    let start_is_len = matches!(start, hir::Expr::UnOp { op: hir::UnOp::Len, .. });
    let start_span = match (start_is_len, &start_c) {
        (true, _) => match expr_span(start) {
            Some(span) => span,
            None => return,
        },
        (false, Some((_, span))) => span.clone(),
        (false, None) => return,
    };
    // `#t, 5` may be a fill-up loop and a constant `1, 8` is just a loop.
    let never = match &start_c {
        _ if start_is_len => end_v < 0.0,
        Some((a, _)) => *a > end_v,
        None => false,
    };
    if start_is_len && end_v > 1.0 || !start_is_len && !never {
        return;
    }

    let start_src = snippet(src, &start_span);
    let end_src = snippet(src, &end_span);
    let start_desc = if start_is_len { format!("`{start_src}`") } else { start_src.clone() };
    let step_desc = match &step_span {
        None => "the default step is +1".to_string(),
        Some(span) => format!("the step is +{}", snippet(src, span)),
    };
    let (message, label, note, why) = if never && start_is_len {
        (
            format!("`for` from {start_desc} down to {end_src} never runs"),
            format!("`{start_src}` is never below 0, so this range is empty"),
            "a length is never negative, so the start sits above the end on every \
             entry and the body never runs — a reverse walk needs an explicit `-1` \
             step"
                .to_string(),
            "valid Luau — dead code unless `#` reaches a `__len` metamethod that \
             returns a negative number",
        )
    } else if never {
        (
            format!("`for` from {start_desc} down to {end_src} without a `-1` step"),
            format!("starts at {start_desc} and counts UP ({step_desc})"),
            "start above end with a positive step: the body never runs — a \
             countdown needs an explicit `-1` step"
                .to_string(),
            "valid Luau — a provably empty range is only ever deliberate as \
             scaffolding",
        )
    } else if end_v == 0.0 {
        (
            format!("`for` from {start_desc} down to {end_src} without a `-1` step"),
            format!("starts at {start_desc} and counts UP ({step_desc})"),
            "counting up from a length to 0, the body runs only for an empty table \
             — and then the index is 0, which no array holds"
                .to_string(),
            "valid Luau — deliberate only if the table is known to be empty",
        )
    } else {
        (
            format!("`for` from {start_desc} down to {end_src} without a `-1` step"),
            format!("starts at {start_desc} and counts UP ({step_desc})"),
            "counting up from a length to 1, the body never runs when the table \
             holds more than one element — a reverse walk needs an explicit `-1` \
             step"
                .to_string(),
            "valid Luau — deliberate only if the table is known to hold at most \
             one element",
        )
    };
    // Each edit keeps the source's own text for what it does not change,
    // and `plain` says whether the text it touches is what it assumes.
    let exact = |span: &Span| src.get(span.clone()).unwrap_or_default();
    let (suggestion, plain) = match (&step_span, never && start_is_len) {
        (None, false) => (
            Suggestion {
                message: format!("add the step: `{end_src}, -1`"),
                span: end_span.clone(),
                replacement: format!("{}, -1", exact(&end_span)),
            },
            true,
        ),
        (None, true) => (
            Suggestion {
                message: "count down to 1: `1, -1`".to_string(),
                span: end_span.clone(),
                replacement: "1, -1".to_string(),
            },
            true,
        ),
        // Replaces the end and the step together, so only `,` and
        // whitespace may sit between them: anything else there (a comment,
        // a parenthesis) would be deleted with them.
        (Some(span), true) => (
            Suggestion {
                message: "count down to 1: `1, -1`".to_string(),
                span: end_span.start..span.end,
                replacement: "1, -1".to_string(),
            },
            src.get(end_span.end..span.start).is_some_and(|gap| gap.trim() == ","),
        ),
        // A `-` in front of the step negates it only when the step is a
        // bare literal: in front of `- -1` it starts a comment.
        (Some(span), false) => (
            Suggestion {
                message: format!("make the step negative: `-{}`", snippet(src, span)),
                span: span.clone(),
                replacement: format!("-{}", exact(span)),
            },
            matches!(step, Some(hir::Expr::Literal { .. })),
        ),
    };
    let d = Diagnostic::error(Code(605), message)
        .with_label(Label::primary(start_span, label))
        .with_note(note)
        .conservative(why);
    // The end's span can sit inside parentheses the HIR dropped, where an
    // added step would land inside them too — `(1, -1)`. Parse the result.
    diags.push(if plain && crate::edits::keeps_syntax(src, &suggestion) {
        d.with_suggestion(suggestion)
    } else {
        d.with_help(suggestion.message)
    });
}

// --- R0606 type-check-inside-call ----------------------------------------

/// One `type(x == "name")` call, with the source ranges its rewrite is
/// assembled from.
struct TypeCheck {
    callee: String,
    callee_span: Span,
    /// The call suffix: `(` through `)`. `Expr::Call::span` does not cover
    /// the callee.
    call_span: Span,
    arg_span: Span,
    literal_span: Span,
    literal_is_rhs: bool,
    op: &'static str,
    tyname: String,
}

fn type_check_inside_call(e: &hir::Expr) -> Option<TypeCheck> {
    use hir::BinOp as Op;
    let hir::Expr::Call { base, args, method: None, span, arg_spans } = e else {
        return None;
    };
    let hir::Expr::Name { name: callee, span: callee_span } = &**base else {
        return None;
    };
    if callee != "type" && callee != "typeof" {
        return None;
    }
    let ([hir::Expr::BinOp { op: op @ (Op::Eq | Op::Ne), lhs, rhs }], [arg_span]) =
        (args.as_slice(), arg_spans.as_slice())
    else {
        return None;
    };
    let string_lit = |e: &hir::Expr| match e {
        hir::Expr::Literal { prim: hir::Prim::String, span, text: Some(t) } => {
            Some((span.clone(), t.clone()))
        }
        _ => None,
    };
    let (literal_is_rhs, (literal_span, tyname)) = match (string_lit(rhs), string_lit(lhs)) {
        (Some(lit), _) => (true, lit),
        (None, Some(lit)) => (false, lit),
        (None, None) => return None,
    };
    Some(TypeCheck {
        callee: callee.clone(),
        callee_span: callee_span.clone(),
        call_span: span.clone(),
        arg_span: arg_span.clone(),
        literal_span,
        literal_is_rhs,
        op: if *op == Op::Eq { "==" } else { "~=" },
        tyname,
    })
}

fn type_check_diag(t: &TypeCheck, src: &str, slots: &CallSlots) -> Diagnostic {
    let (callee, op, tyname) = (&t.callee, t.op, &t.tyname);
    let mut d = Diagnostic::error(Code(606), format!("the comparison is inside the `{callee}` call"))
        .with_label(Label::primary(
            t.call_span.clone(),
            format!("`{callee}` receives the comparison's boolean result, not the value"),
        ))
        .with_note(format!(
            "`{callee}(x {op} \"{tyname}\")` always answers \"boolean\" — the \
             intended check compares the call's RESULT to the string"
        ))
        .conservative(
            "valid Luau — but taking the type of a comparison result is almost \
             never what was meant",
        );
    if let Some(fixed) = type_check_rewrite(t, src, slots) {
        let message = if fixed.len() <= 80 && !fixed.contains('\n') {
            format!("move the comparison outside: `{fixed}`")
        } else {
            "move the comparison outside the call".to_string()
        };
        d = d.with_suggestion(Suggestion {
            message,
            // The whole call, callee included.
            span: t.callee_span.start..t.call_span.end,
            replacement: fixed,
        });
    }
    d
}

/// `type(x == "name")` rewritten as `type(x) == "name"`, built from the
/// source's own text for the value and the literal. None wherever the
/// rewrite is not a plain substitution for the call:
///
///   - the call's result is indexed or called, or the call is a statement;
///   - the text between the pieces is anything but whitespace (a comment
///     would be dropped, parentheses would be left unbalanced);
///   - the assembled text does not parse back as the comparison it is meant
///     to be (a `--` comment inside the value swallowing the `)`).
///
/// As an operand of an operator that binds tighter than a comparison the
/// rewrite is parenthesized, so the surrounding expression keeps its shape.
fn type_check_rewrite(t: &TypeCheck, src: &str, slots: &CallSlots) -> Option<String> {
    if slots.result_used.contains(&t.call_span.start) {
        return None;
    }
    let blank = |from: usize, to: usize| {
        src.get(from..to).is_some_and(|s| s.chars().all(char::is_whitespace))
    };
    let call = src.get(t.call_span.clone())?;
    if !call.starts_with('(') || !call.ends_with(')') {
        return None;
    }
    if !blank(t.callee_span.end, t.call_span.start)
        || !blank(t.call_span.start + 1, t.arg_span.start)
        || !blank(t.arg_span.end, t.call_span.end - 1)
    {
        return None;
    }
    let literal = src.get(t.literal_span.clone())?;
    let value = if t.literal_is_rhs {
        if t.literal_span.end != t.arg_span.end {
            return None;
        }
        src.get(t.arg_span.start..t.literal_span.start)?
            .trim_end()
            .strip_suffix(t.op)?
            .trim_end()
    } else {
        if t.literal_span.start != t.arg_span.start {
            return None;
        }
        src.get(t.literal_span.end..t.arg_span.end)?
            .trim_start()
            .strip_prefix(t.op)?
            .trim_start()
    };
    if value.is_empty() {
        return None;
    }
    let fixed = format!("{}({value}) {} {literal}", t.callee, t.op);
    if !is_type_comparison(&fixed, t) {
        return None;
    }
    Some(if slots.operand.contains(&t.call_span.start) { format!("({fixed})") } else { fixed })
}

/// Whether `text` parses as exactly `callee(<one argument>) op <string>`.
/// The rewrite is assembled from byte ranges; this confirms the pieces put
/// together the expression they were cut for.
fn is_type_comparison(text: &str, t: &TypeCheck) -> bool {
    use hir::BinOp as Op;
    let Ok(module) = rime_syntax::parse(&format!("return {text}\n")) else {
        return false;
    };
    let block = &module.body(module.root).block;
    let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator else {
        return false;
    };
    let ([], [hir::Expr::BinOp { op, lhs, rhs }]) = (block.stmts.as_slice(), exprs.as_slice())
    else {
        return false;
    };
    let op_matches = match op {
        Op::Eq => t.op == "==",
        Op::Ne => t.op == "~=",
        _ => false,
    };
    let lhs_is_call = match &**lhs {
        hir::Expr::Call { base, args, method: None, .. } => {
            args.len() == 1
                && matches!(&**base, hir::Expr::Name { name, .. } if *name == t.callee)
        }
        _ => false,
    };
    let rhs_is_literal = matches!(
        &**rhs,
        hir::Expr::Literal { prim: hir::Prim::String, text: Some(s), .. } if *s == t.tyname
    );
    op_matches && lhs_is_call && rhs_is_literal
}

/// Where the calls of a body sit, keyed by the start of the call's span.
/// A rewrite that turns a call into a looser-binding expression reads this
/// to know what the surrounding code allows.
#[derive(Default)]
struct CallSlots {
    /// Direct operand of an operator other than `and`/`or` (or part of an
    /// unmodelled composite): a comparison put here needs parentheses.
    operand: HashSet<usize>,
    /// The call's result is indexed or called, or the call is a statement:
    /// nothing but a call can stand here.
    result_used: HashSet<usize>,
}

fn call_slots(body: &hir::Body) -> CallSlots {
    use hir::Expr as E;
    fn call_start(e: &hir::Expr) -> Option<usize> {
        match e {
            hir::Expr::Call { span, .. } => Some(span.start),
            _ => None,
        }
    }
    let mut slots = CallSlots::default();
    visit::exprs(body, &mut |e| match e {
        E::BinOp { op: hir::BinOp::And | hir::BinOp::Or, .. } => {}
        E::BinOp { lhs, rhs, .. } => {
            slots.operand.extend(call_start(lhs));
            slots.operand.extend(call_start(rhs));
        }
        E::Not(inner) | E::UnOp { operand: inner, .. } => {
            slots.operand.extend(call_start(inner));
        }
        E::Composite(parts) => {
            slots.operand.extend(parts.iter().filter_map(call_start));
        }
        E::Call { base, .. } | E::Index { base, .. } => {
            slots.result_used.extend(call_start(base));
        }
        _ => {}
    });
    visit::stmts(body, &mut |stmt| {
        if let hir::StmtKind::Call(e) = &stmt.kind {
            slots.result_used.extend(call_start(e));
        }
    });
    slots
}

// --- R0607 constant-table-comparison -------------------------------------

/// `==`, `~=` or an ordering operator with a table constructor on either
/// side. `around` is the statement (or `return`) the expression sits in:
/// a comparison none of whose leaves has a span (`{} == {}`) is placed
/// from it, and `placed` keeps two of those in one statement apart.
fn constant_table_comparison(
    e: &hir::Expr,
    around: &Span,
    src: &str,
    placed: &mut HashSet<usize>,
    diags: &mut Vec<Diagnostic>,
) {
    use hir::BinOp as Op;
    let hir::Expr::BinOp { op, lhs, rhs } = e else {
        return;
    };
    let op_src = match op {
        Op::Eq => "==",
        Op::Ne => "~=",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
        _ => return,
    };
    let is_table = |side: &hir::Expr| matches!(side, hir::Expr::Table(_));
    let tables = usize::from(is_table(lhs)) + usize::from(is_table(rhs));
    if tables == 0 {
        return;
    }
    let span = expr_span(e)
        .or_else(|| empty_pair_span(src, around, op_src, placed))
        .unwrap_or_else(|| around.clone());
    if !matches!(op, Op::Eq | Op::Ne) {
        // Not conservative: no metatable, so no `__lt`/`__le`, and Luau
        // only calls one that BOTH operands share.
        diags.push(
            Diagnostic::error(Code(607), format!("`{op_src}` with a table constructor"))
                .with_label(Label::primary(
                    span,
                    "always raises an error — a fresh table has nothing to be ordered by",
                ))
                .with_note(
                    "Luau orders two numbers, two strings, or two values that \
                     share one `__lt`/`__le` metamethod; a just-constructed \
                     table has no metatable, so this is \"attempt to compare\" \
                     at run time"
                        .to_string(),
                )
                .with_help(
                    "to compare sizes compare lengths (`#a < #b`); to test \
                     for emptiness use `next(t) == nil`",
                ),
        );
        return;
    }
    let outcome = if *op == Op::Eq { "false" } else { "true" };
    if tables == 2 {
        // Not conservative: neither table has a metatable to carry `__eq`.
        diags.push(
            Diagnostic::error(Code(607), "comparison with a table constructor")
                .with_label(Label::primary(
                    span,
                    format!("always {outcome} — each constructor makes its own fresh table"),
                ))
                .with_note(
                    "`==` on tables compares identity, and two just-constructed \
                     tables are never the same object; to compare contents \
                     compare fields"
                        .to_string(),
                ),
        );
        return;
    }
    diags.push(
        Diagnostic::error(Code(607), "comparison with a table constructor")
            .with_label(Label::primary(
                span,
                format!("always {outcome} — the constructor makes a fresh table"),
            ))
            .with_note(
                "`==` on tables compares identity, and a just-constructed table \
                 is never the same object; to test for emptiness use \
                 `next(t) == nil`, to compare contents compare fields"
                    .to_string(),
            )
            .conservative(
                "an `__eq` metamethod on the other operand could make this \
                 meaningful; none is visible here",
            ),
    );
}

/// Where `{} <op> {}` is written inside `around`: the first one not handed
/// out yet. Two empty constructors are the one comparison with no span to
/// take from the HIR and a spelling simple enough to find in the text.
fn empty_pair_span(
    src: &str,
    around: &Span,
    op_src: &str,
    placed: &mut HashSet<usize>,
) -> Option<Span> {
    /// The offset just past an empty constructor that starts at `at`.
    fn empty_table(text: &str, at: usize) -> Option<usize> {
        let rest = text.get(at..)?.strip_prefix('{')?;
        let inner = rest.trim_start();
        inner.starts_with('}').then(|| at + 1 + (rest.len() - inner.len()) + 1)
    }
    fn skip_space(text: &str, at: usize) -> usize {
        at + (text[at..].len() - text[at..].trim_start().len())
    }
    let text = src.get(around.clone())?;
    for (start, _) in text.match_indices('{') {
        let Some(left_end) = empty_table(text, start) else {
            continue;
        };
        let op_at = skip_space(text, left_end);
        if !text[op_at..].starts_with(op_src) {
            continue;
        }
        let Some(end) = empty_table(text, skip_space(text, op_at + op_src.len())) else {
            continue;
        };
        if placed.insert(around.start + start) {
            return Some(around.start + start..around.start + end);
        }
    }
    None
}

// --- R0608 unbalanced-assignments ----------------------------------------

fn unbalanced_assignments(stmt: &hir::Stmt, diags: &mut Vec<Diagnostic>) {
    use hir::StmtKind as K;
    let (names, values) = match &stmt.kind {
        K::Local { decls, init } if !init.is_empty() => (decls.len(), init),
        K::Assign { targets, values } => (targets.len(), values),
        _ => return,
    };
    if names == values.len() {
        return;
    }
    // A trailing call or `...` (and anything unmodelled) can be
    // multi-valued, so a SHORT list ending in one may still fill every
    // name — silence, never a wrong fact. A list that is already too long
    // only gets longer: `x = 1, f()` is certain.
    let open_ended = matches!(
        values.last(),
        Some(hir::Expr::Call { .. } | hir::Expr::Leaf | hir::Expr::Composite(_))
    );
    if open_ended && values.len() < names {
        return;
    }
    // A short list ending in `nil` — `local a, b = nil` — leaves the
    // remaining names nil, which is what the `nil` already says. Selene's
    // `unbalanced_assignments` allows exactly this; so does Rime.
    if values.len() < names
        && matches!(values.last(), Some(hir::Expr::Literal { prim: hir::Prim::Nil, .. }))
    {
        return;
    }
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let d = if values.len() < names {
        let missing = names - values.len();
        // Name the names that come up empty, when they are plain names.
        let tail: Vec<String> = match &stmt.kind {
            K::Local { decls, .. } => decls
                .iter()
                .skip(values.len())
                .map(|d| format!("`{}`", d.name))
                .collect(),
            K::Assign { targets, .. } => targets
                .iter()
                .skip(values.len())
                .filter_map(|t| match t {
                    hir::Target::Name { name, .. } => Some(format!("`{name}`")),
                    hir::Target::Projection(_) => None,
                })
                .collect(),
            _ => unreachable!(),
        };
        let who = if tail.len() == missing {
            tail.join(", ")
        } else {
            format!("the last {missing} name{}", plural(missing))
        };
        let d = Diagnostic::error(
            Code(608),
            format!("{names} names but {} value{}", values.len(), plural(values.len())),
        )
        .with_label(Label::primary(
            stmt.span.clone(),
            format!("{who} become{} nil", if missing == 1 { "s" } else { "" }),
        ))
        .with_note("if the nils are intended, write them explicitly".to_string());
        // Selene's hint on `a, b, c = call(), 2`.
        if values[..values.len() - 1].iter().any(|v| matches!(v, hir::Expr::Call { .. })) {
            d.with_note(
                "a call that is not the last value gives exactly one value, \
                 however many it returns"
                    .to_string(),
            )
        } else {
            d
        }
    } else {
        let extra = values.len() - names;
        let discarded = format!(
            "the last {extra} value{} {} discarded",
            if extra == 1 { "" } else { "s" },
            if extra == 1 { "is" } else { "are" },
        );
        Diagnostic::error(
            Code(608),
            format!("{names} name{} but {} values", plural(names), values.len()),
        )
        .with_label(Label::primary(
            stmt.span.clone(),
            if matches!(values.last(), Some(hir::Expr::Call { .. })) {
                format!("{discarded}: the call still runs, and every result it returns is dropped")
            } else {
                discarded
            },
        ))
        .with_note(
            "every value still evaluates; only the results past the name count \
             are dropped"
                .to_string(),
        )
    };
    diags.push(d.conservative(
        "valid Luau — but a count mismatch with single-value expressions is \
         usually an edit slip",
    ));
}

// --- R0610 compare-nan -----------------------------------------------------

/// A number literal whose value is zero: `0`, `0.0`, `0x0`.
fn is_zero_literal(e: &hir::Expr) -> bool {
    matches!(
        e,
        hir::Expr::Literal { prim: hir::Prim::Number, text: Some(t), .. } if number_value(t) == Some(0.0)
    )
}

/// NaN as it is written: a literal zero over a literal zero. This is the
/// shape R0602 leaves alone as the NaN idiom (`/`, `//` and `%` alike, the
/// divisor seen through `-`), so the two rules agree on what a NaN is. A
/// leading `-` keeps it NaN.
fn is_nan_literal(e: &hir::Expr) -> bool {
    use hir::BinOp as Op;
    let mut e = e;
    while let hir::Expr::UnOp { op: hir::UnOp::Neg, operand, .. } = e {
        e = operand;
    }
    let hir::Expr::BinOp { op: Op::Div | Op::FloorDiv | Op::Mod, lhs, rhs } = e else {
        return false;
    };
    let mut divisor: &hir::Expr = rhs;
    while let hir::Expr::UnOp { op: hir::UnOp::Neg, operand, .. } = divisor {
        divisor = operand;
    }
    is_zero_literal(lhs) && is_zero_literal(divisor)
}

/// `x == 0 / 0` and `x ~= 0 / 0`, either way round. NaN equals nothing,
/// itself included, so the first is always false and the second always
/// true. Unlike the rest of the family this is not conservative: `__eq`
/// is consulted only when both operands are tables, and one of these is a
/// number, so nothing at runtime can change the answer.
///
/// The compared value must be a name, a field or an index, as in Selene's
/// `compare_nan`: those are the "is this variable NaN" checks the lint is
/// for. A literal or a call against NaN, and the ordering operators, stay
/// silent as they do there.
fn compare_nan(e: &hir::Expr, src: &str, diags: &mut Vec<Diagnostic>) {
    use hir::BinOp as Op;
    let hir::Expr::BinOp { op: op @ (Op::Eq | Op::Ne), lhs, rhs } = e else {
        return;
    };
    let is_variable = |e: &hir::Expr| matches!(e, hir::Expr::Name { .. } | hir::Expr::Index { .. });
    let (value, nan, nan_is_rhs) = if is_variable(lhs) && is_nan_literal(rhs) {
        (&**lhs, &**rhs, true)
    } else if is_variable(rhs) && is_nan_literal(lhs) {
        (&**rhs, &**lhs, false)
    } else {
        return;
    };
    let (Some(value_span), Some(nan_span)) = (variable_span(value, src), expr_span(nan)) else {
        return;
    };
    let span = value_span.start.min(nan_span.start)..value_span.end.max(nan_span.end);
    let (op_src, always, self_op, self_means) = match op {
        Op::Eq => ("==", "false", "~=", "true only for NaN"),
        _ => ("~=", "true", "==", "false only for NaN"),
    };
    let d = Diagnostic::error(Code(610), format!("comparison with NaN is always {always}"))
        .with_label(Label::primary(
            span.clone(),
            format!("`{op_src}` against NaN is always {always}, whatever the value is"),
        ))
        .with_note(
            "NaN is not equal to any value, itself included, so `==` against \
             it never matches and `~=` always does"
                .to_string(),
        );
    // The edit is offered for the one shape whose every character is
    // accounted for: `name == 0/0`. Anything else between those tokens
    // (parentheses the HIR dropped, a comment) would be lost or unbalanced.
    let plain_name = match value {
        hir::Expr::Name { name, .. } if nan_is_rhs => src
            .get(span.clone())
            .and_then(|text| text.strip_prefix(name.as_str()))
            .and_then(|rest| rest.trim_start().strip_prefix(op_src))
            .and_then(|rest| rest.trim_start().strip_prefix('0'))
            .and_then(|rest| rest.trim_start().strip_prefix('/'))
            .is_some_and(|rest| rest.trim_start() == "0")
            .then_some(name),
        _ => None,
    };
    diags.push(match plain_name {
        Some(name) => {
            let fixed = format!("{name} {self_op} {name}");
            d.with_suggestion(Suggestion {
                message: format!("compare the value to itself, which is {self_means}: `{fixed}`"),
                span,
                replacement: fixed,
            })
        }
        None => d.with_help(format!(
            "to test for NaN compare the value to itself: `v {self_op} v` is {self_means}"
        )),
    });
}

/// The source range of a name, field or index expression: `a`, `a.b`,
/// `a[k]`. The closing `]` of an index is not in the HIR; it is taken from
/// the source when it directly follows the key.
fn variable_span(e: &hir::Expr, src: &str) -> Option<Span> {
    let start = expr_span(e)?.start;
    let end = match e {
        hir::Expr::Name { span, .. } => span.end,
        hir::Expr::Index { name_span: Some(name), .. } => name.end,
        hir::Expr::Index { key: Some(key), .. } => {
            let key_end = expr_span(key)?.end;
            let rest = src.get(key_end..)?;
            let gap = rest.len() - rest.trim_start().len();
            if rest[gap..].starts_with(']') { key_end + gap + 1 } else { key_end }
        }
        _ => return None,
    };
    Some(start..end.max(start))
}

// --- structural equality --------------------------------------------------

/// Strict: unmodelled expressions and closures are never equal to anything,
/// including themselves.
fn exprs_equal(a: &hir::Expr, b: &hir::Expr) -> bool {
    use hir::Expr as E;
    match (a, b) {
        (E::Name { name: a, .. }, E::Name { name: b, .. }) => a == b,
        (
            E::Literal { prim: pa, text: ta, .. },
            E::Literal { prim: pb, text: tb, .. },
        ) => pa == pb && ta == tb,
        (
            E::Call { base: ba, args: aa, method: ma, .. },
            E::Call { base: bb, args: ab, method: mb, .. },
        ) => ma == mb && exprs_equal(ba, bb) && all_equal(aa, ab),
        (
            E::Index { base: ba, key: ka, name: na, .. },
            E::Index { base: bb, key: kb, name: nb, .. },
        ) => {
            na == nb
                && exprs_equal(ba, bb)
                && match (ka, kb) {
                    (None, None) => true,
                    (Some(ka), Some(kb)) => exprs_equal(ka, kb),
                    _ => false,
                }
        }
        (
            E::BinOp { op: oa, lhs: la, rhs: ra },
            E::BinOp { op: ob, lhs: lb, rhs: rb },
        ) => oa == ob && exprs_equal(la, lb) && exprs_equal(ra, rb),
        (E::Not(a), E::Not(b)) => exprs_equal(a, b),
        (
            E::UnOp { op: oa, operand: a, .. },
            E::UnOp { op: ob, operand: b, .. },
        ) => oa == ob && exprs_equal(a, b),
        (E::Table(fa), E::Table(fb)) => {
            fa.len() == fb.len()
                && fa.iter().zip(fb).all(|(a, b)| {
                    use hir::TableField as F;
                    match (a, b) {
                        (F::Named { name: na, value: va, .. }, F::Named { name: nb, value: vb, .. }) => {
                            na == nb && exprs_equal(va, vb)
                        }
                        (F::Keyed { key: ka, value: va }, F::Keyed { key: kb, value: vb }) => {
                            exprs_equal(ka, kb) && exprs_equal(va, vb)
                        }
                        (F::Item(a), F::Item(b)) => exprs_equal(a, b),
                        _ => false,
                    }
                })
        }
        (E::IfExpr(aa), E::IfExpr(ab)) => {
            aa.len() == ab.len()
                && aa.iter().zip(ab).all(|((ca, va), (cb, vb))| {
                    exprs_equal(va, vb)
                        && match (ca, cb) {
                            (None, None) => true,
                            (Some(ca), Some(cb)) => exprs_equal(ca, cb),
                            _ => false,
                        }
                })
        }
        // Closures (different bodies), Leaf, Composite: unknown — unequal.
        _ => false,
    }
}

fn all_equal(a: &[hir::Expr], b: &[hir::Expr]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| exprs_equal(a, b))
}

fn stmts_equal(a: &hir::Stmt, b: &hir::Stmt) -> bool {
    use hir::StmtKind as K;
    match (&a.kind, &b.kind) {
        (K::Local { decls: da, init: ia }, K::Local { decls: db, init: ib }) => {
            decls_equal(da, db) && all_equal(ia, ib)
        }
        (K::Assign { targets: ta, values: va }, K::Assign { targets: tb, values: vb }) => {
            ta.len() == tb.len()
                && ta.iter().zip(tb).all(|(a, b)| targets_equal(a, b))
                && all_equal(va, vb)
        }
        (
            K::CompoundAssign { target: ta, value: va },
            K::CompoundAssign { target: tb, value: vb },
        ) => targets_equal(ta, tb) && exprs_equal(va, vb),
        (K::Call(a), K::Call(b)) => exprs_equal(a, b),
        (K::Do(a), K::Do(b)) => blocks_equal(a, b),
        (
            K::If { cond: ca, then: ta, else_ifs: ea, or_else: oa },
            K::If { cond: cb, then: tb, else_ifs: eb, or_else: ob },
        ) => {
            exprs_equal(ca, cb)
                && blocks_equal(ta, tb)
                && ea.len() == eb.len()
                && ea.iter().zip(eb).all(|((c1, b1), (c2, b2))| {
                    exprs_equal(c1, c2) && blocks_equal(b1, b2)
                })
                && match (oa, ob) {
                    (None, None) => true,
                    (Some(a), Some(b)) => blocks_equal(a, b),
                    _ => false,
                }
        }
        (K::While { cond: ca, body: ba }, K::While { cond: cb, body: bb }) => {
            exprs_equal(ca, cb) && blocks_equal(ba, bb)
        }
        (K::Repeat { body: ba, until: ua }, K::Repeat { body: bb, until: ub }) => {
            blocks_equal(ba, bb) && exprs_equal(ua, ub)
        }
        (
            K::NumericFor { var: va, start: sa, end: ea, step: pa, body: ba },
            K::NumericFor { var: vb, start: sb, end: eb, step: pb, body: bb },
        ) => {
            va.name == vb.name
                && exprs_equal(sa, sb)
                && exprs_equal(ea, eb)
                && match (pa, pb) {
                    (None, None) => true,
                    (Some(a), Some(b)) => exprs_equal(a, b),
                    _ => false,
                }
                && blocks_equal(ba, bb)
        }
        (
            K::GenericFor { vars: va, exprs: ea, body: ba },
            K::GenericFor { vars: vb, exprs: eb, body: bb },
        ) => {
            va.len() == vb.len()
                && va.iter().zip(vb).all(|(a, b)| a.name == b.name)
                && all_equal(ea, eb)
                && blocks_equal(ba, bb)
        }
        // Function definitions: separate bodies — unknown, unequal.
        _ => false,
    }
}

fn targets_equal(a: &hir::Target, b: &hir::Target) -> bool {
    match (a, b) {
        (hir::Target::Name { name: a, .. }, hir::Target::Name { name: b, .. }) => a == b,
        (hir::Target::Projection(a), hir::Target::Projection(b)) => exprs_equal(a, b),
        _ => false,
    }
}

fn decls_equal(a: &[hir::LocalDecl], b: &[hir::LocalDecl]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.name == b.name
                && match (&a.ty, &b.ty) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.rendered == b.rendered,
                    _ => false,
                }
        })
}

fn blocks_equal(a: &hir::Block, b: &hir::Block) -> bool {
    use hir::Terminator as T;
    a.stmts.len() == b.stmts.len()
        && a.stmts.iter().zip(&b.stmts).all(|(a, b)| stmts_equal(a, b))
        && match (&a.terminator, &b.terminator) {
            (None, None) => true,
            (Some(T::Break), Some(T::Break)) => true,
            (Some(T::Continue), Some(T::Continue)) => true,
            (Some(T::Return { exprs: a, .. }), Some(T::Return { exprs: b, .. })) => {
                all_equal(a, b)
            }
            _ => false,
        }
}

// --- span and text helpers ------------------------------------------------

/// The source range an expression covers, from the spans its leaves carry.
/// None when no descendant has a span (a bare `...`, an empty constructor).
pub(super) fn expr_span(e: &hir::Expr) -> Option<Span> {
    let mut acc: Option<Span> = None;
    visit::expr(e, &mut |e| {
        let s = match e {
            hir::Expr::Name { span, .. }
            | hir::Expr::Call { span, .. }
            | hir::Expr::Literal { span, .. }
            | hir::Expr::UnOp { span, .. } => span,
            _ => return,
        };
        acc = Some(match &acc {
            None => s.clone(),
            Some(a) => a.start.min(s.start)..a.end.max(s.end),
        });
    });
    acc
}

/// First-to-last statement range of a block (the `Return` terminator
/// included). None for a block with nothing spannable (empty, or a bare
/// `break`/`continue`).
fn block_span(b: &hir::Block) -> Option<Span> {
    let first = b.stmts.first().map(|s| s.span.clone());
    let ret = match &b.terminator {
        Some(hir::Terminator::Return { span, .. }) => Some(span.clone()),
        _ => None,
    };
    match (first, ret) {
        (Some(f), Some(r)) => Some(f.start..r.end.max(f.end)),
        (Some(f), None) => Some(f.start..b.stmts.last().unwrap().span.end.max(f.end)),
        (None, r) => r,
    }
}

fn contains_call(e: &hir::Expr) -> bool {
    let mut found = false;
    visit::expr(e, &mut |e| {
        if matches!(e, hir::Expr::Call { .. }) {
            found = true;
        }
    });
    found
}

/// One-line source excerpt for a span: whitespace runs collapsed, capped.
fn snippet(src: &str, span: &Span) -> String {
    let raw = src.get(span.clone()).unwrap_or("");
    let mut out = String::new();
    let mut last_ws = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            if !last_ws && !out.is_empty() {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(c);
            last_ws = false;
        }
    }
    if out.len() > 40 {
        out.truncate(37);
        out.push_str("...");
    }
    out
}

fn number_value(text: &str) -> Option<f64> {
    let t = text.replace('_', "");
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).ok().map(|v| v as f64)
    } else if let Some(bin) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        u64::from_str_radix(bin, 2).ok().map(|v| v as f64)
    } else {
        t.parse::<f64>().ok()
    }
}

#[cfg(test)]
mod tests {
    use crate::edits::testing::fixed;

    /// Lines of every diagnostic `code` in `src`.
    fn lines(src: &str, code: u16) -> Vec<usize> {
        crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(code))
            .map(|d| src[..d.primary_span().start].matches('\n').count() + 1)
            .collect()
    }

    #[test]
    fn trailing_nil_fills_the_remaining_names() {
        // Selene's `unbalanced_assignments` allows a short value list that
        // ends in `nil`: the names past it are nil either way.
        let src = "local a, b = nil\n\
                   local c, d, e = 1, nil\n\
                   a, b = nil\n\
                   return { a, b, c, d, e }\n";
        assert_eq!(lines(src, 608), vec![]);
    }

    #[test]
    fn short_value_list_that_does_not_end_in_nil_is_still_reported() {
        let src = "local a, b = 1\n\
                   local c, d, e = nil, 1\n\
                   local f = nil, nil\n\
                   a, b = 2\n\
                   return { a, b, c, d, e, f }\n";
        assert_eq!(lines(src, 608), vec![1, 2, 3, 4]);
    }

    /// Selene's `unbalanced_assignments` fixture, with the
    /// lines its `.stderr` reports.
    const SELENE_UNBALANCED_ASSIGNMENTS: &str = "a, b, c = 1\n\
        a, b, c = 1, 2\n\
        a, b, c = 1, 2, 3\n\n\
        a, b, c = call()\n\
        a, b, c = call(), 2\n\
        a, b, c = call(), 2, 3\n\
        a, b, c = 1, call()\n\n\
        local a, b, c\n\
        a, b, c = nil\n\n\
        local a, b, c = 1, 2, 3, 4\n\
        x = 1, 2\n\n\
        local a, b, c = ...\n";

    #[test]
    fn unbalanced_assignments_reports_what_selene_reports_on_its_fixture() {
        assert_eq!(lines(SELENE_UNBALANCED_ASSIGNMENTS, 608), vec![1, 2, 6, 13, 14]);
    }

    #[test]
    fn too_many_values_is_reported_whatever_the_last_one_is() {
        // With more values than names the count is certain: a trailing
        // call or `...` only adds to what is already left over.
        let src = "local function f(g, ...)\n\
                   \tlocal a = 1, g()\n\
                   \tlocal b, c = 1, 2, g()\n\
                   \tlocal d = 1, ...\n\
                   \ta = 2, g()\n\
                   \treturn a, b, c, d\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 608), vec![2, 3, 4, 5]);
        let d = crate::check_source(src)
            .into_iter()
            .find(|d| d.code == rime_diag::Code(608))
            .unwrap();
        assert_eq!(d.message, "1 name but 2 values");
        assert!(d.labels[0].message.contains("the call still runs"), "{:?}", d.labels);
    }

    #[test]
    fn too_few_values_ending_in_a_call_or_varargs_stays_silent() {
        let src = "local function f(g, ...)\n\
                   \tlocal a, b, c = 1, g()\n\
                   \tlocal d, e = ...\n\
                   \tlocal h, i = g()\n\
                   \tlocal j, k = (g())\n\
                   \treturn a, b, c, d, e, h, i, j, k\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 608), vec![]);
    }

    #[test]
    fn a_call_that_is_not_last_is_named_as_giving_one_value() {
        let src = "local function f(g)\n\tlocal a, b, c = g(), 2\n\treturn a, b, c\nend\nreturn f\n";
        let d = crate::check_source(src)
            .into_iter()
            .find(|d| d.code == rime_diag::Code(608))
            .expect("R0608 on `local a, b, c = g(), 2`");
        assert!(d.notes.iter().any(|n| n.contains("exactly one value")), "{:?}", d.notes);
        // No call before the end: nothing to say about one.
        let src = "local a, b, c = 1, 2\nreturn { a, b, c }\n";
        let d = crate::check_source(src).into_iter().find(|d| d.code == rime_diag::Code(608)).unwrap();
        assert!(!d.notes.iter().any(|n| n.contains("exactly one value")), "{:?}", d.notes);
    }

    #[test]
    fn zero_over_zero_is_the_nan_idiom() {
        // Selene's `divide_by_zero` skips a literal `0` numerator; this is
        // the list it stays silent on, plus the spellings of the same
        // value it happens to treat differently.
        let src = "local NAN = 0 / 0\n\
                   local a = -(0 / 0)\n\
                   local b = 0 / 0.0\n\
                   local c = 0.0 / 0.0\n\
                   local d = 0.0 / 0\n\
                   local e = (0) / 0\n\
                   local f = 0 % 0\n\
                   local g = 0 // 0\n\
                   return { NAN, a, b, c, d, e, f, g }\n";
        assert_eq!(lines(src, 602), vec![]);
    }

    /// Selene's own `compare_nan` fixtures
    /// (`selene-lib/tests/lints/compare_nan`), with the lines its `.stderr`
    /// files report.
    const SELENE_COMPARE_NAN_IF: &str = "if x == 0/0 then\nend\n\n\
        if x ~= 0/0 then\nend\n\n\
        if x ~= x then\nend\n\n\
        if x >= 0/0 then\nend\n\n\
        if 1 == 0/0 then\nend\n\n\
        if 1 ~= 0/0 then\nend\n\n\
        if 1 + 0/0 then\nend\n\n\
        if y.foo() == 0/0 then\nend\n\n\
        if y.bar() ~= 0/0 then\nend\n\n\
        if x == 1 then\nend\n\n\
        if 1 == 0 then\nend\n";
    const SELENE_COMPARE_NAN_VARIABLES: &str = "local _ = 0/0\n\
        local _ = 1 ~= 0/0\n\
        local _ = 2 == 0/0\n\
        local _ = 3 + 0/0\n\
        local _ = 4 >= 0/0\n\
        local _ = x ~= 0/0\n\
        local _ = x == 0/0\n\
        local _ = x >= 0/0\n\
        local _ = y.foo() == 0/0\n\
        local _ = y.bar() ~= 0/0\n\n\
        local _ = x == 1\n\
        local _ = 1 == 0\n";

    #[test]
    fn compare_nan_reports_what_selene_reports_on_its_fixtures() {
        assert_eq!(lines(SELENE_COMPARE_NAN_IF, 610), vec![1, 4]);
        assert_eq!(lines(SELENE_COMPARE_NAN_VARIABLES, 610), vec![6, 7]);
    }

    #[test]
    fn compare_nan_and_divide_by_zero_agree_on_how_nan_is_written() {
        // Every spelling R0602 leaves alone as the NaN idiom is a NaN to
        // R0610 too — and none of these lines draws an R0602.
        let src = "local function f(x, t)\n\
                   \tlocal a = x == 0 / 0\n\
                   \tlocal b = x == 0.0 / 0\n\
                   \tlocal c = x == 0 / 0.0\n\
                   \tlocal d = x == 0 % 0\n\
                   \tlocal e = x == 0 // 0\n\
                   \tlocal g = x == -(0 / 0)\n\
                   \tlocal h = x == (0 / 0)\n\
                   \tlocal i = x == 0 / -0\n\
                   \tlocal j = t.value ~= 0 / 0\n\
                   \tlocal k = t[1] == 0 / 0\n\
                   \tlocal l = 0 / 0 == x\n\
                   \tlocal m = 0 / 0 ~= t.value\n\
                   \treturn { a, b, c, d, e, g, h, i, j, k, l, m }\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 610), vec![2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
        assert_eq!(lines(src, 602), vec![]);
    }

    #[test]
    fn compare_nan_stays_silent_on_everything_that_is_not_nan() {
        let src = "local NAN = 0 / 0\n\
                   local function f(x, t)\n\
                   \tlocal a = x ~= x\n\
                   \tlocal b = x == NAN\n\
                   \tlocal c = x == 1 / 0\n\
                   \tlocal d = x == 0 / 1\n\
                   \tlocal e = x == 0 * 0\n\
                   \tlocal g = x == 0\n\
                   \tlocal h = x == -0\n\
                   \tlocal i = x == t.zero / 0\n\
                   \tlocal j = x + 0 / 0\n\
                   \tlocal k = math.max(x, 0 / 0)\n\
                   \treturn { a, b, c, d, e, g, h, i, j, k }\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 610), vec![]);
    }

    #[test]
    fn compare_nan_says_which_way_the_comparison_always_goes() {
        let report = |src: &str| {
            crate::check_source(src)
                .into_iter()
                .find(|d| d.code == rime_diag::Code(610))
                .expect("R0610")
        };
        let eq = report("local function f(x)\n\treturn x == 0 / 0\nend\nreturn f\n");
        assert!(eq.message.contains("always false"), "{}", eq.message);
        assert!(!eq.conservative, "nothing can make `==` against NaN true");
        assert_eq!(eq.severity, rime_diag::Severity::Error);
        let ne = report("local function f(x)\n\treturn x ~= 0 / 0\nend\nreturn f\n");
        assert!(ne.message.contains("always true"), "{}", ne.message);
        // A field has no edit, but the advice still names the idiom.
        let field = report("local function f(t)\n\treturn t.value == 0 / 0\nend\nreturn f\n");
        assert!(field.suggestions.is_empty());
        assert!(field.help.iter().any(|h| h.contains("~=")), "{:?}", field.help);
    }

    #[test]
    fn compare_nan_edit_compares_the_name_to_itself() {
        let cases = [
            ("return v == 0/0", "return v ~= v"),
            ("return v ~= 0/0", "return v == v"),
            ("return v  ==  0 / 0", "return v ~= v"),
            ("return (v == 0/0)", "return (v ~= v)"),
            ("return a and v == 0/0", "return a and v ~= v"),
            ("if v == 0/0 then return 1 end", "if v ~= v then return 1 end"),
            ("return v == 0/0 -- nan", "return v ~= v -- nan"),
        ];
        for (before, after) in cases {
            assert_eq!(fixed(&body(before), 610), Some(body(after)), "{before}");
        }
    }

    #[test]
    fn compare_nan_edit_is_withheld_unless_the_text_is_exactly_name_op_zero_over_zero() {
        let cases = [
            // Parentheses the HIR dropped: the span ends inside them.
            "return v == (0/0)",
            "return v == -(0/0)",
            // A comment inside the comparison would be deleted.
            "return v == --[[ nan ]] 0/0",
            "return v == 0 --[[ over ]] / 0",
            // Other spellings of NaN: reported, but the edit only knows `0/0`.
            "return v == 0 % 0",
            "return v == 0.0 / 0",
            // NaN on the left, and values that are not a plain name.
            "return 0/0 == v",
            "return a.b == 0/0",
            "return a[index] == 0/0",
        ];
        for case in cases {
            let src = body(case);
            assert_eq!(fixed(&src, 610), None, "{case}");
            let d = crate::check_source(&src)
                .into_iter()
                .find(|d| d.code == rime_diag::Code(610))
                .unwrap();
            assert_eq!(d.help.len(), 1, "{case}: {:?}", d.help);
        }
    }

    #[test]
    fn nonzero_over_zero_is_still_reported() {
        let src = "local function f(x)\n\
                   \tlocal a = x / 0\n\
                   \tlocal b = 1 / 0\n\
                   \tlocal c = x % 0\n\
                   \tlocal d = x // 0\n\
                   \tlocal e = x / -0\n\
                   \tlocal g = -0 / 0\n\
                   \tlocal h = (x - x) / 0\n\
                   \treturn { a, b, c, d, e, g, h }\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 602), vec![2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn divide_by_zero_advice_is_not_itself_reported() {
        // The report tells the reader what to write instead; neither form
        // it names may draw the same report.
        let d = crate::check_source("local function f(x)\n\treturn x / 0\nend\nreturn f\n")
            .into_iter()
            .find(|d| d.code == rime_diag::Code(602))
            .expect("R0602 on `x / 0`");
        let advice = d.notes.join(" ");
        assert!(advice.contains("`math.huge`") && advice.contains("`0 / 0`"), "{advice}");
        let src = "local INF = math.huge\nlocal NAN = 0 / 0\nreturn { INF, NAN }\n";
        assert_eq!(lines(src, 602), vec![]);
    }

    fn body(line: &str) -> String {
        format!("local function f(v, a, self, index)\n\t{line}\nend\nreturn f\n")
    }

    /// A function over `a` and `b` with `lines` as the start of its body.
    fn swap_body(lines: &str) -> String {
        format!("local function f(a, b)\n\t{lines}\n\treturn a, b\nend\nreturn f\n")
    }

    #[test]
    fn swap_edit_joins_two_adjacent_statements() {
        for case in ["a = b\n\tb = a", "a = b; b = a", "a = b b = a", "a = b\n\n\tb = a -- after"] {
            let out = fixed(&swap_body(case), 601).unwrap_or_else(|| panic!("no edit for {case}"));
            assert!(out.contains("\ta, b = b, a"), "{case}:\n{out}");
            assert_eq!(out.contains("-- after"), case.contains("-- after"), "{case}:\n{out}");
        }
    }

    #[test]
    fn swap_edit_is_withheld_when_a_comment_sits_between_the_statements() {
        // The edit replaces everything from the first statement to the end
        // of the second, so a comment in between would go with it.
        let cases = [
            "a = b -- first half\n\tb = a",
            "a = b\n\t-- why this order\n\tb = a",
            "a = b --[[ why ]] b = a",
        ];
        for case in cases {
            let src = swap_body(case);
            assert_eq!(fixed(&src, 601), None, "{case}");
            // Still reported, and the advice is still there to read.
            let d = crate::check_source(&src)
                .into_iter()
                .find(|d| d.code == rime_diag::Code(601))
                .unwrap();
            assert!(d.help.iter().any(|h| h.contains("`a, b = b, a`")), "{case}: {:?}", d.help);
        }
    }

    /// Selene's `constant_table_comparison` fixture, with
    /// the lines its `.stderr` reports.
    const SELENE_CONSTANT_TABLE_COMPARISON: &str = "print(x == { \"a\", \"b\", \"c\" })\n\
        print({ \"a\", \"b\", \"c\" } == x)\n\n\
        print(x == {})\nprint({} == x)\nprint(x ~= {})\nprint({} ~= x)\n\n\
        print({ \"a\", \"b\", \"c\" } == { \"a\", \"b\", \"c\" })\n\
        print({ \"a\", \"b\", \"c\" } == {})\nprint({} == {})\n\n\
        print( -- my cool table\n\tt == {}\n)\n";

    #[test]
    fn constant_table_comparison_reports_what_selene_reports_on_its_fixture() {
        assert_eq!(
            lines(SELENE_CONSTANT_TABLE_COMPARISON, 607),
            vec![1, 2, 4, 5, 6, 7, 9, 10, 11, 14],
        );
    }

    /// Every R0607 in `src`: the text its label covers, and whether it is
    /// conservative.
    fn table_comparisons(src: &str) -> Vec<(String, bool)> {
        crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(607))
            .map(|d| (src[d.primary_span()].to_string(), d.conservative))
            .collect()
    }

    #[test]
    fn two_constructors_never_compare_equal_and_the_label_finds_them() {
        // No leaf of `{} == {}` carries a span, so the label is placed from
        // the statement around it. Nothing can make this comparison come
        // out the other way: not conservative.
        let src = "local a = {} == {}\n\
                   local b, c = {} ~= {}, {  } == { }\n\
                   print(a, b, c, {} == {})\n\
                   return {} ~= {}\n";
        let want = ["{} == {}", "{} ~= {}", "{  } == { }", "{} == {}", "{} ~= {}"];
        let found = table_comparisons(src);
        assert_eq!(found.iter().map(|(text, _)| text.as_str()).collect::<Vec<_>>(), want);
        assert!(found.iter().all(|(_, conservative)| !conservative));
        // Each of the two on line 2 gets its own place.
        let starts: Vec<usize> = crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(607))
            .map(|d| d.primary_span().start)
            .collect();
        let mut distinct = starts.clone();
        distinct.dedup();
        assert_eq!(starts, distinct);
    }

    #[test]
    fn a_constructor_with_nothing_to_point_at_is_reported_at_its_statement() {
        let src = "local function f(...)\n\
                   \tlocal a = ({}) == ({})\n\
                   \tlocal b = {} == ...\n\
                   \tlocal c = { function() end } == {}\n\
                   \treturn a, b, c\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 607), vec![2, 3, 4]);
    }

    #[test]
    fn ordering_against_a_constructor_always_raises() {
        // Luau orders two numbers, two strings, or two values that share
        // one `__lt`/`__le`. A fresh table has no metatable, so each of
        // these raises "attempt to compare" whatever `x` is.
        let src = "local function f(x, t)\n\
                   \tlocal a = x < {}\n\
                   \tlocal b = {} <= x\n\
                   \tlocal c = x > { 1 }\n\
                   \tlocal d = { 1 } >= t.n\n\
                   \tlocal e = {} < {}\n\
                   \treturn a, b, c, d, e\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 607), vec![2, 3, 4, 5, 6]);
        for d in crate::check_source(src).into_iter().filter(|d| d.code == rime_diag::Code(607)) {
            assert!(!d.conservative, "{}", d.message);
            assert!(d.labels.iter().any(|l| l.message.contains("raises")), "{:?}", d.labels);
        }
        assert_eq!(table_comparisons(src)[4].0, "{} < {}");
    }

    #[test]
    fn equality_against_one_constructor_is_unchanged() {
        let src = "local function f(t)\n\treturn t == {}, t ~= { 1 }\nend\nreturn f\n";
        let found = table_comparisons(src);
        assert_eq!(found.len(), 2);
        assert!(found.iter().all(|(_, conservative)| *conservative));
        // Ordering two values that are not constructors is not this rule's.
        assert_eq!(lines("local function f(a, b)\n\treturn a < b, a == b\nend\nreturn f\n", 607), vec![]);
    }

    /// Selene's `almost_swapped` fixture. Its `.stderr`
    /// reports the pairs starting at lines 3, 8 and 15; Rime points at the
    /// second statement of each, the one that does nothing.
    const SELENE_ALMOST_SWAPPED: &str = "x, y = y, x\n\n\
        a = b\nb = a\n\n\
        t[1], t[2] = t[2], t[1]\n\n\
        t[1] = t[2]\nt[2] = t[1]\n\n\
        foo().a = foo().b\nfoo().b = foo().a\n\n\
        -- We use a weird hack so this comment might break something, oh no!\n\
        a = b\nb = a\n";

    #[test]
    fn almost_swapped_reports_what_selene_reports_on_its_fixture() {
        assert_eq!(lines(SELENE_ALMOST_SWAPPED, 601), vec![4, 9, 16]);
        assert_eq!(lines("foo = \"bar\"\n", 601), vec![]);
    }

    /// A function over two tables with `lines` as the start of its body.
    fn field_swap_body(lines: &str) -> String {
        format!("local function f(t, u, i, j)\n\t{lines}\n\treturn t, u\nend\nreturn f\n")
    }

    #[test]
    fn almost_swapped_sees_fields_and_index_expressions() {
        let cases = [
            ("t.a = t.b\n\tt.b = t.a", "`t.a` and `t.b` do not swap"),
            ("t[1] = t[2]\n\tt[2] = t[1]", "`t[1]` and `t[2]` do not swap"),
            ("t[i] = t[j]\n\tt[j] = t[i]", "`t[i]` and `t[j]` do not swap"),
            ("t.x = u.x\n\tu.x = t.x", "`t.x` and `u.x` do not swap"),
            ("t.a.b = u\n\tu = t.a.b", "`t.a.b` and `u` do not swap"),
            ("t[\"a\"] = t[\"b\"]\n\tt[\"b\"] = t[\"a\"]", "`t[\"a\"]` and `t[\"b\"]` do not swap"),
            ("t[i + 1] = t[i]\n\tt[i] = t[i + 1]", "`t[…]` and `t[i]` do not swap"),
        ];
        for (case, message) in cases {
            let src = field_swap_body(case);
            let found: Vec<String> = crate::check_source(&src)
                .into_iter()
                .filter(|d| d.code == rime_diag::Code(601))
                .map(|d| d.message)
                .collect();
            assert_eq!(found, vec![message.to_string()], "{case}");
        }
    }

    #[test]
    fn almost_swapped_edit_keeps_each_side_as_written() {
        let cases = [
            ("t.a = t.b\n\tt.b = t.a", "\tt.a, t.b = t.b, t.a\n"),
            ("t[1] = t[2]\n\tt[2] = t[1]", "\tt[1], t[2] = t[2], t[1]\n"),
            ("t[ i ] = t[j]; t[j] = t[ i ]", "\tt[ i ], t[j] = t[j], t[ i ]\n"),
            ("t[i + 1] = t[i]\n\tt[i] = t[i + 1]", "\tt[i + 1], t[i] = t[i], t[i + 1]\n"),
            ("t[\"a=b\"] = u\n\tu = t[\"a=b\"]", "\tt[\"a=b\"], u = u, t[\"a=b\"]\n"),
        ];
        for (case, want) in cases {
            let out = fixed(&field_swap_body(case), 601).unwrap_or_else(|| panic!("no edit for {case}"));
            assert!(out.contains(want), "{case}:\n{out}");
        }
        // The two statements spell one side differently: reported, and the
        // advice is help, since neither spelling is the one to keep.
        let src = field_swap_body("t.a = t[1]\n\tt[1] = (t).a");
        assert_eq!(fixed(&src, 601), None);
        assert_eq!(lines(&src, 601), vec![3]);
    }

    #[test]
    fn almost_swapped_stays_silent_when_the_second_assignment_is_not_a_no_op() {
        let cases = [
            // A call may return something else each time (Selene's fixture).
            "t[f()] = t.a\n\tt.a = t[f()]",
            "u.get().a = t.a\n\tt.a = u.get().a",
            // Writing one side changes where the other points.
            "i = t[i]\n\tt[i] = i",
            "t = t.next\n\tt.next = t",
            "t.x = t.x.y\n\tt.x.y = t.x",
            // Not a pair.
            "t.a = t.a\n\tt.a = t.a",
            "t.a = t.b\n\tt.b = t.c",
            "t.a = t.b\n\tu.b = t.a",
            "t.a = t.b\n\tprint(t)\n\tt.b = t.a",
        ];
        for case in cases {
            assert_eq!(lines(&field_swap_body(case), 601), vec![], "{case}");
        }
    }

    /// A function over a table `t` holding one loop with this header.
    fn loop_body(header: &str) -> String {
        format!("local function f(t)\n\t{header}\n\t\tprint(i)\n\tend\nend\nreturn f\n")
    }

    #[test]
    fn countdown_edit_makes_the_loop_count_down() {
        let cases = [
            ("for i = 10, 1 do", "for i = 10, 1, -1 do"),
            ("for i = #t, 1 do", "for i = #t, 1, -1 do"),
            ("for i = #t, 0 do", "for i = #t, 0, -1 do"),
            ("for i = 10, -1 do", "for i = 10, -1, -1 do"),
            ("for i = 10, 1, 2 do", "for i = 10, 1, -2 do"),
            ("for i = 10, 1, 1e0 do", "for i = 10, 1, -1e0 do"),
            // Parentheses around a step stay around the negated step.
            ("for i = 10, 1, (1) do", "for i = 10, 1, (-1) do"),
            // A length never goes below zero: count down to 1.
            ("for i = #t, -1 do", "for i = #t, 1, -1 do"),
            ("for i = #t, -1, 2 do", "for i = #t, 1, -1 do"),
            // A comment after the range is not part of the edit.
            ("for i = 10, 1 --[[ down ]] do", "for i = 10, 1, -1 --[[ down ]] do"),
        ];
        for (before, after) in cases {
            let out = fixed(&loop_body(before), 605).unwrap_or_else(|| panic!("no edit for {before}"));
            assert_eq!(out, loop_body(after), "{before}");
        }
    }

    #[test]
    fn countdown_edit_is_withheld_where_it_would_not_be_a_countdown() {
        let cases = [
            // The end's span is the literal inside the parentheses, so the
            // step would land in there too: `(1, -1)`.
            "for i = 10, (1) do",
            "for i = #t, (-1) do",
            "for i = #t, (-1), 2 do",
            "for i = #t, -1, (2) do",
            // `-` in front of `- -1` is `-- -1`, a comment.
            "for i = 10, 1, - -1 do",
            // A comment between the end and the step would be deleted.
            "for i = #t, -1, --[[ step ]] 2 do",
        ];
        for case in cases {
            let src = loop_body(case);
            assert_eq!(fixed(&src, 605), None, "{case}");
            let d = crate::check_source(&src)
                .into_iter()
                .find(|d| d.code == rime_diag::Code(605))
                .unwrap();
            assert_eq!(d.help.len(), 1, "{case}: {:?}", d.help);
        }
    }

    #[test]
    fn type_check_edit_rewrites_the_whole_call() {
        let cases = [
            ("return type(v == \"number\")", "return type(v) == \"number\""),
            ("return typeof(v ~= \"Instance\")", "return typeof(v) ~= \"Instance\""),
            // The value keeps its own text: a field, a call, a long chain.
            ("return type(a.b == \"x\")", "return type(a.b) == \"x\""),
            ("return type(a.b.c[index] == \"x\")", "return type(a.b.c[index]) == \"x\""),
            (
                "return typeof(self._instances[index]:FindFirstChild(\"Handle\", true) == \"Instance\")",
                "return typeof(self._instances[index]:FindFirstChild(\"Handle\", true)) == \"Instance\"",
            ),
            ("return type((a or v) == \"x\")", "return type((a or v)) == \"x\""),
            // The literal keeps its own quoting, whichever side it is on.
            ("return type(v == 'number')", "return type(v) == 'number'"),
            ("return type(\"number\" == v)", "return type(v) == \"number\""),
            ("return type( v  ==  \"number\" )", "return type(v) == \"number\""),
            // Where a comparison binds looser than the call it replaces, it
            // is parenthesized.
            ("return not type(v == \"number\")", "return not (type(v) == \"number\")"),
            (
                "return type(v == \"number\") == \"boolean\"",
                "return (type(v) == \"number\") == \"boolean\"",
            ),
            ("return a .. type(v == \"number\")", "return a .. (type(v) == \"number\")"),
            // `and`/`or` bind looser still: no parentheses needed.
            ("return a and type(v == \"number\")", "return a and type(v) == \"number\""),
            ("if type(v == \"number\") then return 1 end", "if type(v) == \"number\" then return 1 end"),
            ("return a(type(v == \"number\"), 2)", "return a(type(v) == \"number\", 2)"),
        ];
        for (before, after) in cases {
            assert_eq!(fixed(&body(before), 606), Some(body(after)), "{before}");
        }
    }

    #[test]
    fn type_check_edit_is_withheld_where_it_would_not_be_a_plain_rewrite() {
        let cases = [
            // A statement cannot be a bare comparison.
            "type(v == \"number\")",
            // The call's result is indexed or called.
            "return type(v == \"number\"):upper()",
            "return type(v == \"number\").x",
            // A comment the rewrite would drop or run into.
            "return type(v -- the value\n\t== \"number\")",
            "return type --[[ why ]] (v == \"number\")",
            "return type(--[[ why ]] v == \"number\")",
            // Parentheses around the literal: the operand text is not `v ==`.
            "return type(v == (\"number\"))",
        ];
        for case in cases {
            assert_eq!(fixed(&body(case), 606), None, "{case}");
        }
    }
}
