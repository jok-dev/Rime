//! R0701–R0706: the style family — selene-parity lints (decided
//! 2026-08-21: suspicious = error, style = warn). Warning severity: none of
//! these can be a runtime error, they are shapes with a clearly better
//! spelling. All conservative-flagged (valid Luau, intent unprovable). The
//! first four are pure HIR walks; R0706 reads a note lowering leaves on
//! the body, because the tree keeps no parentheses.
//!
//!   R0701 empty-if                 an `if`/`elseif`/`else` arm with no statements
//!   R0702 empty-loop               a loop whose body is empty
//!   R0703 mixed-table              positional items mixed with keyed fields
//!   R0704 manual-table-clone       `local d = {}; for k, v in pairs(t) do d[k] = v end`
//!   R0706 parenthesized-condition  `if (x) then`
//!
//! R0707 needless-quote-escape is in `strings.rs`, with the other
//! string-escape rule.
//!

//! R0705 high-cyclomatic-complexity is not implemented: selene ships it
//! disabled.

use rime_diag::{Code, Diagnostic, Label, Suggestion};
use rime_hir::{self as hir, visit};

pub fn check(body: &hir::Body, src: &str, diags: &mut Vec<Diagnostic>) {
    parenthesized_conditions(body, src, diags);
    visit::stmts(body, &mut |stmt| {
        empty_if(stmt, diags);
        empty_loop(stmt, diags);
    });
    visit::exprs(body, &mut |e| mixed_table(e, diags));
    visit::blocks(body, &mut |block| manual_table_clone(block, diags));
}

fn is_empty(b: &hir::Block) -> bool {
    b.stmts.is_empty() && b.terminator.is_none()
}

// --- R0701 empty-if -------------------------------------------------------

fn empty_if(stmt: &hir::Stmt, diags: &mut Vec<Diagnostic>) {
    let hir::StmtKind::If { then, else_ifs, or_else, .. } = &stmt.kind else {
        return;
    };
    let mut empty_arms: Vec<&str> = Vec::new();
    if is_empty(then) {
        empty_arms.push("`then`");
    }
    for (_, b) in else_ifs {
        if is_empty(b) {
            empty_arms.push("`elseif`");
        }
    }
    if let Some(b) = or_else {
        if is_empty(b) {
            empty_arms.push("`else`");
        }
    }
    let Some(first) = empty_arms.first() else {
        return;
    };
    let what = if empty_arms.len() == 1 {
        format!("the {first} arm of this `if` is empty")
    } else {
        format!("{} arms of this `if` are empty", empty_arms.len())
    };
    diags.push(
        Diagnostic::warning(Code(701), what)
            .with_label(Label::primary(
                stmt.span.clone(),
                "an empty arm runs nothing",
            ))
            .with_note(
                "either the arm is missing its logic, or the condition can \
                 fold into the arms that remain (an empty `then` inverts to \
                 `if not ...`)"
                    .to_string(),
            )
            .conservative(
                "Rime does not read comments — an arm holding only an \
                 explanatory comment still counts as empty; drop the arm or \
                 suppress with `-- rime: allow(empty-if, ...)`",
            ),
    );
}

// --- R0702 empty-loop -----------------------------------------------------

fn empty_loop(stmt: &hir::Stmt, diags: &mut Vec<Diagnostic>) {
    use hir::StmtKind as K;
    // A call in the loop header is the deliberate wait-loop idiom
    // (`while task.wait() do end`, `repeat until player:IsLoaded()`):
    // the header IS the body. Only call-free headers flag.
    let (body, kind) = match &stmt.kind {
        K::While { cond, body } if !contains_call(cond) => (body, "`while` loop"),
        K::Repeat { body, until } if !contains_call(until) => (body, "`repeat` loop"),
        K::NumericFor { start, end, step, body, .. }
            if !contains_call(start)
                && !contains_call(end)
                && !step.as_ref().is_some_and(contains_call) =>
        {
            (body, "numeric `for` loop")
        }
        K::GenericFor { exprs, body, .. }
            // The iterator call itself (`pairs(t)`) is not an effect; a
            // nested call inside its ARGUMENT could be, but the dominant
            // shape is plain `pairs`/`ipairs` over a name.
            if exprs.iter().all(iterator_side_effect_free) =>
        {
            (body, "generic `for` loop")
        }
        _ => return,
    };
    if !is_empty(body) {
        return;
    }
    diags.push(
        Diagnostic::warning(Code(702), format!("this {kind} has an empty body"))
            .with_label(Label::primary(
                stmt.span.clone(),
                "iterates and does nothing",
            ))
            .with_note(
                "a condition-only loop with no effect in its header is a \
                 busy-wait at best and dead code at worst"
                    .to_string(),
            )
            .conservative(
                "Rime does not read comments — a body holding only a comment \
                 still counts as empty",
            ),
    );
}

/// `pairs(t)` / `ipairs(x.y)` — a single call whose own arguments are
/// call-free. The call produces the iterator; it does not DO anything.
fn iterator_side_effect_free(e: &hir::Expr) -> bool {
    match e {
        hir::Expr::Call { args, .. } => !args.iter().any(contains_call),
        _ => !contains_call(e),
    }
}

// --- R0703 mixed-table ----------------------------------------------------

fn mixed_table(e: &hir::Expr, diags: &mut Vec<Diagnostic>) {
    let hir::Expr::Table(fields) = e else {
        return;
    };
    let positional = fields
        .iter()
        .filter(|f| matches!(f, hir::TableField::Item(_)))
        .count();
    let keyed = fields.len() - positional;
    if positional == 0 || keyed == 0 {
        return;
    }
    let Some(span) = expr_span(e) else {
        return;
    };
    diags.push(
        Diagnostic::warning(Code(703), "array items mixed with keyed fields")
            .with_label(Label::primary(
                span,
                format!("{positional} positional item(s) alongside {keyed} keyed field(s)"),
            ))
            .with_note(
                "a table that is both an array and a map trips length (`#`), \
                 iteration order, and JSON encoding; split it into an array \
                 field and named fields"
                    .to_string(),
            )
            .conservative(
                "valid Luau, and occasionally deliberate (an options table \
                 with a positional payload) — Rime cannot tell",
            ),
    );
}

// --- R0704 manual-table-clone ---------------------------------------------

/// `local dst = {}` immediately followed by `for k, v in pairs(t) do
/// dst[k] = v end`. The fresh-`{}` requirement is load-bearing: with an
/// existing table this loop is a MERGE, and `table.clone` would be wrong.
fn manual_table_clone(block: &hir::Block, diags: &mut Vec<Diagnostic>) {
    use hir::{Expr as E, StmtKind as K};
    for pair in block.stmts.windows(2) {
        // local dst = {}
        let K::Local { decls, init } = &pair[0].kind else {
            continue;
        };
        let ([decl], [E::Table(fields)]) = (decls.as_slice(), init.as_slice()) else {
            continue;
        };
        if !fields.is_empty() {
            continue;
        }
        // for k, v in pairs(t) do dst[k] = v end
        let K::GenericFor { vars, exprs, body } = &pair[1].kind else {
            continue;
        };
        let ([kvar, vvar], [E::Call { base, args, method: None, .. }]) =
            (vars.as_slice(), exprs.as_slice())
        else {
            continue;
        };
        let E::Name { name: iter_fn, .. } = &**base else {
            continue;
        };
        if iter_fn != "pairs" && iter_fn != "ipairs" {
            continue;
        }
        let [src] = args.as_slice() else {
            continue;
        };
        // Body: exactly `dst[k] = v`.
        if body.terminator.is_some() || body.stmts.len() != 1 {
            continue;
        }
        let K::Assign { targets, values } = &body.stmts[0].kind else {
            continue;
        };
        let ([hir::Target::Projection(proj)], [E::Name { name: vread, .. }]) =
            (targets.as_slice(), values.as_slice())
        else {
            continue;
        };
        let E::Index { base: dst, key: Some(key), .. } = proj else {
            continue;
        };
        let (E::Name { name: dst_name, .. }, E::Name { name: kread, .. }) = (&**dst, &**key)
        else {
            continue;
        };
        if dst_name != &decl.name || kread != &kvar.name || vread != &vvar.name {
            continue;
        }
        let src_name = match src {
            E::Name { name, .. } => format!("`{name}`"),
            _ => "the source table".to_string(),
        };
        diags.push(
            Diagnostic::warning(Code(704), "manual shallow copy")
                .with_label(Label::primary(
                    pair[0].span.start..pair[1].span.end,
                    format!("copies {src_name} one entry at a time"),
                ))
                .with_note(format!(
                    "`table.clone` does this in one call, faster and \
                     without the loop: `{} {} = table.clone(...)`",
                    if decl.is_const { "const" } else { "local" },
                    decl.name
                ))
                .conservative(
                    "the spelled-out loop is valid — worth keeping only if \
                     entries need filtering or transforming, which this one \
                     does not do",
                ),
        );
    }
}

// --- R0706 parenthesized-condition ----------------------------------------

/// `if (x) then`: one report per condition, however many pairs wrap it.
/// The conditions are found by lowering (`hir::ParenCondition`); the tree
/// keeps no parentheses.
fn parenthesized_conditions(body: &hir::Body, src: &str, diags: &mut Vec<Diagnostic>) {
    for cond in &body.notes.paren_conditions {
        let d = Diagnostic::warning(
            Code(706),
            format!("parentheses around the whole `{}` condition", cond.keyword),
        )
        .with_label(Label::primary(cond.span.clone(), "these parentheses change nothing"))
        .with_note(format!(
            "`{}` takes a bare expression: the keyword after it ends the condition",
            cond.keyword
        ))
        .conservative(
            "valid Luau, and it runs the same either way — this is only \
             about how the condition reads",
        );
        diags.push(match without_parentheses(cond, src) {
            Some(replacement) => d.with_suggestion(Suggestion {
                message: "remove the parentheses".to_string(),
                span: cond.span.clone(),
                replacement,
            }),
            None => d.with_help(
                "remove the parentheses by hand: a comment inside them has \
                 to move, so Rime offers no edit",
            ),
        });
    }
}

/// The text to put in place of `cond.span`, or `None` when the edit could
/// lose or misplace a comment. A `--` anywhere inside the parentheses
/// withholds it — also one inside a string, which this does not tell
/// apart from a comment.
fn without_parentheses(cond: &hir::ParenCondition, src: &str) -> Option<String> {
    if src.get(cond.span.clone())?.contains("--") {
        return None;
    }
    let held = src.get(cond.inner.clone())?.trim();
    if held.is_empty() {
        return None;
    }
    // `if(x)then` is valid; `ifxthen` is one name. Put back the space a
    // parenthesis stood in for.
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let before = word(src[..cond.span.start].chars().next_back());
    let after = word(src[cond.span.end..].chars().next());
    Some(format!(
        "{}{held}{}",
        if before { " " } else { "" },
        if after { " " } else { "" }
    ))
}

// --- shared helpers (kept local: style and suspicious stay independent) ---

fn contains_call(e: &hir::Expr) -> bool {
    let mut found = false;
    visit::expr(e, &mut |e| {
        if matches!(e, hir::Expr::Call { .. }) {
            found = true;
        }
    });
    found
}

fn expr_span(e: &hir::Expr) -> Option<hir::Span> {
    let mut acc: Option<hir::Span> = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn r0706(src: &str) -> Vec<Diagnostic> {
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        for body in &module.bodies {
            parenthesized_conditions(body, src, &mut diags);
        }
        diags.sort_by_key(|d| d.primary_span().start);
        diags
    }

    fn lines(src: &str) -> Vec<usize> {
        r0706(src)
            .iter()
            .map(|d| src[..d.primary_span().start].matches('\n').count() + 1)
            .collect()
    }

    /// `src` with every R0706 edit applied.
    fn fixed(src: &str) -> String {
        let mut edits: Vec<_> = r0706(src).into_iter().flat_map(|d| d.suggestions).collect();
        edits.sort_by_key(|s| std::cmp::Reverse(s.span.start));
        let mut out = src.to_string();
        for s in edits {
            out.replace_range(s.span, &s.replacement);
        }
        out
    }

    // Selene, `parenthese_conditions`, run on
    // this text: it reports lines 2, 4, 5, 6, 9, 10, 11, 14 and 16, and
    // nothing else. Rime reports the same lines.
    const SELENE_FIXTURE: &str = "\
local x, y, z, f = 1, 2, 3, print
if (x) then end
if (x) and (y) then end
if ((x)) then end
if (x == y) then end
if (f()) then end
if not (x) then end
if (x)() then end
if x then elseif (y) then end
while (x) do break end
repeat until (x)
local a = if (x) then 1 else 2
local b = if x then 1 elseif (y) then 2 else 3
if (x :: any) then end
if (x) :: any then end
if (
	x
) then end
print(a, b, z)
";

    #[test]
    fn parenthesized_condition_selene_fixture() {
        assert_eq!(lines(SELENE_FIXTURE), vec![2, 4, 5, 6, 9, 10, 11, 14, 16]);
    }

    #[test]
    fn parenthesized_condition_is_a_conservative_warning_naming_the_keyword() {
        let src = "local x = 1\nif x then elseif (x) then end\nrepeat until (x)\n";
        let got = r0706(src);
        assert_eq!(
            got.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
            vec![
                "parentheses around the whole `elseif` condition",
                "parentheses around the whole `until` condition",
            ]
        );
        for d in &got {
            assert_eq!(d.code.to_string(), "R0706");
            assert_eq!(d.severity, rime_diag::Severity::Warning);
            assert!(d.conservative);
            assert_eq!(&src[d.primary_span()], "(x)");
        }
    }

    #[test]
    fn parenthesized_condition_one_report_for_nested_pairs() {
        let src = "local x = 1\nif (((x))) then end\n";
        let got = r0706(src);
        assert_eq!(got.len(), 1);
        assert_eq!(&src[got[0].primary_span()], "(((x)))");
    }

    #[test]
    fn parenthesized_condition_edit_removes_the_parentheses() {
        assert_eq!(fixed(SELENE_FIXTURE), "\
local x, y, z, f = 1, 2, 3, print
if x then end
if (x) and (y) then end
if x then end
if x == y then end
if f() then end
if not (x) then end
if (x)() then end
if x then elseif y then end
while x do break end
repeat until x
local a = if (x) then 1 else 2
local b = if x then 1 elseif (y) then 2 else 3
if x :: any then end
if (x) :: any then end
if x then end
print(a, b, z)
");
    }

    // `if(x)then` is valid Luau; `ifxthen` is one name. The edit puts back
    // the space the parenthesis stood in for, on each side that needs one.
    #[test]
    fn parenthesized_condition_edit_keeps_keywords_apart() {
        assert_eq!(
            fixed("local x = 1\nif(x)then end\nwhile(x)do break end\nrepeat until(x)\n"),
            "local x = 1\nif x then end\nwhile x do break end\nrepeat until x\n"
        );
        assert_eq!(fixed("local x = 1\nif( x )then end\n"), "local x = 1\nif x then end\n");
    }

    // A comment inside the parentheses would be deleted, or would swallow
    // the `then` after it. The report stays; the edit is not offered.
    #[test]
    fn parenthesized_condition_no_edit_across_a_comment() {
        for src in [
            "local x = 1\nif (x -- why\n) then end\n",
            "local x = 1\nif ( --[[ why ]] (x)) then end\n",
            "local x = 1\nif ((x) --[[ why ]]) then end\n",
        ] {
            let got = r0706(src);
            assert_eq!(got.len(), 1, "{src}");
            assert!(got[0].suggestions.is_empty(), "{src}: {:?}", got[0].suggestions);
            assert!(!got[0].help.is_empty(), "{src}");
        }
        // A `--` inside a string is no comment, but telling the two apart
        // is the lexer's job; the edit is withheld there too.
        let got = r0706("local x = 1\nif (x == \"--\") then end\n");
        assert_eq!(got.len(), 1);
        assert!(got[0].suggestions.is_empty());
    }

    // A delimiter inside a string must not shift where the edit cuts.
    #[test]
    fn parenthesized_condition_edit_with_delimiters_in_strings() {
        assert_eq!(
            fixed("local x, t = \"\", {}\nif (x == \"(\" or t[\")\"]) then end\n"),
            "local x, t = \"\", {}\nif x == \"(\" or t[\")\"] then end\n"
        );
    }
}