//! R0012: duplicate keys in one table constructor.
//!
//! Purely syntactic — no scope, no flow — so this walks HIR directly rather
//! than the CFG. A key counts as duplicate when two fields of the SAME
//! constructor write the same slot:
//!
//!   - a string slot: the `name = v` sugar form, or a bracket key whose
//!     string literal is knowable (`["x"] = v`);
//!   - a number slot: a bracket key that is a number literal (`[1] = v`,
//!     `[-1] = v`), compared by VALUE (`[1]`, `[1.0]` and `[0x1]` are one
//!     slot), or a positional item, which fills the slot its position
//!     names (`{ "a", [1] = "b" }`).
//!
//! `["1"]` and `[1]` are different slots. Any other key expression is
//! unknown and stays silent, and so does a trailing call or `...` item,
//! which fills no slot or several.

use rime_diag::{Code, Diagnostic, Label};
use rime_hir as hir;
use rime_hir::Span;
use std::collections::HashMap;

pub fn check(body: &hir::Body, diags: &mut Vec<Diagnostic>) {
    for d in duplicate_keys(body) {
        diags.push(diag(&d));
    }
}

/// Conservative — valid Luau, the last value wins — but the earlier entry is
/// dead on arrival and Rime cannot tell a stale edit from intent.
fn diag(d: &DuplicateKey) -> Diagnostic {
    let name = &d.name;
    let (primary, secondary) = match (d.first_positional, d.positional, &d.first_span) {
        // Two written keys.
        (false, false, _) => (format!("`{name}` assigned again here"), "first assigned here".to_string()),
        // A key after the positional item that already fills its slot.
        (true, _, Some(_)) => (
            format!("`{name}` assigned again here"),
            format!("positional item {name} fills this slot first"),
        ),
        (true, _, None) => (
            format!("`{name}` assigned again here: positional item {name} already fills this slot"),
            String::new(),
        ),
        // A positional item after the key that names its slot.
        (false, true, Some(_)) => (
            format!("positional item {name} fills slot `{name}` again"),
            "first assigned here".to_string(),
        ),
        (false, true, None) => (
            format!("a later positional item fills slot `{name}` again"),
            String::new(),
        ),
    };
    let mut out = Diagnostic::error(
        Code(12),
        format!("duplicate key `{name}` in table constructor"),
    )
    .with_label(Label::primary(d.span.clone(), primary));
    if let Some(first) = &d.first_span {
        out = out.with_label(Label::secondary(first.clone(), secondary));
    }
    out.conservative(
        "the last value silently wins; the earlier entry is discarded before \
         the table exists",
    )
    .with_help(if d.positional || d.first_positional {
        format!(
            "remove the `[{name}]` key or the positional item, or assign the \
             slot after construction if the overwrite is deliberate"
        )
    } else {
        format!(
            "remove one of the `{name}` entries, or assign the slot after \
             construction if the overwrite is deliberate"
        )
    })
}

pub struct DuplicateKey {
    /// The slot, as a key would be written: `speed`, `1`, `-1`, `1.5`.
    pub name: String,
    /// Where the report points: the later occurrence — the LAST value wins,
    /// so this is the one that silently discards the earlier entry. When
    /// that is a positional item with no span of its own (a closure, an
    /// empty constructor), the key it collides with instead.
    pub span: Span,
    /// The first occurrence it collides with. None when `span` already
    /// points at it, or when it is a positional item with no span.
    pub first_span: Option<Span>,
    /// The later occurrence is a positional item.
    pub positional: bool,
    /// The first occurrence is a positional item.
    pub first_positional: bool,
}

/// All duplicate constructor keys in one body, in traversal order.
pub fn duplicate_keys(body: &hir::Body) -> Vec<DuplicateKey> {
    let mut out = Vec::new();
    walk_block(&body.block, &mut out);
    out
}

fn walk_block(block: &hir::Block, out: &mut Vec<DuplicateKey>) {
    for stmt in &block.stmts {
        walk_stmt(&stmt.kind, out);
    }
    if let Some(hir::Terminator::Return { exprs, .. }) = &block.terminator {
        for e in exprs {
            walk_expr(e, out);
        }
    }
}

fn walk_stmt(kind: &hir::StmtKind, out: &mut Vec<DuplicateKey>) {
    use hir::StmtKind as K;
    match kind {
        K::Local { init, .. } => {
            for e in init {
                walk_expr(e, out);
            }
        }
        K::Assign { targets, values } => {
            for t in targets {
                if let hir::Target::Projection(e) = t {
                    walk_expr(e, out);
                }
            }
            for e in values {
                walk_expr(e, out);
            }
        }
        K::CompoundAssign { target, value } => {
            if let hir::Target::Projection(e) = target {
                walk_expr(e, out);
            }
            walk_expr(value, out);
        }
        K::Call(e) => walk_expr(e, out),
        K::Do(b) => walk_block(b, out),
        K::If { cond, then, else_ifs, or_else } => {
            walk_expr(cond, out);
            walk_block(then, out);
            for (c, b) in else_ifs {
                walk_expr(c, out);
                walk_block(b, out);
            }
            if let Some(b) = or_else {
                walk_block(b, out);
            }
        }
        K::While { cond, body } => {
            walk_expr(cond, out);
            walk_block(body, out);
        }
        K::Repeat { body, until } => {
            walk_block(body, out);
            walk_expr(until, out);
        }
        K::NumericFor { start, end, step, body, .. } => {
            walk_expr(start, out);
            walk_expr(end, out);
            if let Some(s) = step {
                walk_expr(s, out);
            }
            walk_block(body, out);
        }
        K::GenericFor { exprs, body, .. } => {
            for e in exprs {
                walk_expr(e, out);
            }
            walk_block(body, out);
        }
        // Nested bodies are separate arena entries; check_body visits each.
        K::LocalFunction { .. } | K::FunctionDecl { .. } => {}
    }
}

fn walk_expr(expr: &hir::Expr, out: &mut Vec<DuplicateKey>) {
    use hir::Expr as E;
    match expr {
        E::Call { base, args, .. } => {
            walk_expr(base, out);
            for a in args {
                walk_expr(a, out);
            }
        }
        E::Index { base, key, .. } => {
            walk_expr(base, out);
            if let Some(k) = key {
                walk_expr(k, out);
            }
        }
        E::BinOp { lhs, rhs, .. } => {
            walk_expr(lhs, out);
            walk_expr(rhs, out);
        }
        E::Not(inner) | E::UnOp { operand: inner, .. } => walk_expr(inner, out),
        E::Table(fields) => {
            check_constructor(fields, out);
            for f in fields {
                match f {
                    hir::TableField::Keyed { key, value } => {
                        walk_expr(key, out);
                        walk_expr(value, out);
                    }
                    hir::TableField::Named { value, .. } => walk_expr(value, out),
                    hir::TableField::Item(e) => walk_expr(e, out),
                }
            }
        }
        E::IfExpr(arms) => {
            for (cond, val) in arms {
                if let Some(c) = cond {
                    walk_expr(c, out);
                }
                walk_expr(val, out);
            }
        }
        E::Composite(parts) => {
            for p in parts {
                walk_expr(p, out);
            }
        }
        E::Name { .. } | E::Function(_) | E::Literal { .. } | E::Leaf => {}
    }
}

/// The slot a field writes, when knowable. Borrowed from the HIR: a
/// constructor with a thousand fields allocates nothing here.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Slot<'a> {
    Name(&'a str),
    /// A number key by value: the bits of the `f64`, with `-0` folded
    /// into `0` (one slot at runtime).
    Number(u64),
}

impl Slot<'_> {
    fn number(value: f64) -> Self {
        Slot::Number((value + 0.0).to_bits())
    }

    /// The slot as a key would be written.
    fn name(self) -> String {
        match self {
            Slot::Name(name) => name.to_string(),
            Slot::Number(bits) => {
                let value = f64::from_bits(bits);
                if value.fract() == 0.0 && value.abs() < 1e15 {
                    format!("{}", value as i64)
                } else {
                    format!("{value}")
                }
            }
        }
    }
}

/// One field's write: its slot, and where it can be pointed at.
struct Entry<'a> {
    slot: Slot<'a>,
    /// None for a positional item with no span in the HIR.
    span: Option<Span>,
    positional: bool,
}

/// The slot a written key names, with the span to point at. None for a key
/// that is not a name, a string literal or a number literal.
fn keyed_entry(field: &hir::TableField) -> Option<Entry<'_>> {
    let (slot, span) = match field {
        hir::TableField::Named { name, span, .. } => (Slot::Name(name), span.clone()),
        hir::TableField::Keyed {
            key: hir::Expr::Literal { prim: hir::Prim::String, span, text: Some(t) },
            ..
        } => (Slot::Name(t), span.clone()),
        hir::TableField::Keyed { key, .. } => {
            let (value, span) = super::suspicious::const_number(key)?;
            (Slot::number(value), span)
        }
        hir::TableField::Item(_) => return None,
    };
    Some(Entry { slot, span: Some(span), positional: false })
}

fn check_constructor(fields: &[hir::TableField], out: &mut Vec<DuplicateKey>) {
    let mut seen: HashMap<Slot, Entry> = HashMap::new();
    let mut position = 0usize;
    for (i, field) in fields.iter().enumerate() {
        let entry = match field {
            hir::TableField::Item(item) => {
                position += 1;
                // A call or `...` in the last field expands to every value
                // it yields: none, or several. (Anywhere else it is cut to
                // one.) Which slots it fills is not known.
                let expands = matches!(
                    item,
                    hir::Expr::Call { .. } | hir::Expr::Leaf | hir::Expr::Composite(_)
                );
                if expands && i + 1 == fields.len() {
                    continue;
                }
                Entry {
                    slot: Slot::number(position as f64),
                    span: super::suspicious::expr_span(item),
                    positional: true,
                }
            }
            keyed => match keyed_entry(keyed) {
                Some(entry) => entry,
                None => continue,
            },
        };
        let Some(first) = seen.get(&entry.slot) else {
            seen.insert(entry.slot, entry);
            continue;
        };
        // Two positional items never share a slot, so at least one side is
        // a written key, and a written key always has a span.
        let (span, first_span) = match (&entry.span, &first.span) {
            (Some(span), first_span) => (span.clone(), first_span.clone()),
            (None, Some(first_span)) => (first_span.clone(), None),
            (None, None) => continue,
        };
        out.push(DuplicateKey {
            name: entry.slot.name(),
            span,
            first_span,
            positional: entry.positional,
            first_positional: first.positional,
        });
    }
}

#[cfg(test)]
mod tests {
    /// Lines of every R0012 in `src`.
    fn lines(src: &str) -> Vec<usize> {
        crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(12))
            .map(|d| src[..d.primary_span().start].matches('\n').count() + 1)
            .collect()
    }

    /// Selene's own `duplicate_keys` fixtures
    /// (`selene-lib/tests/lints/duplicate_keys`), with the lines their
    /// `.stderr` files report.
    const SELENE_DUPLICATE_KEYS: &str = "local foo = {\n\ta = true,\n\tb = false,\n\ta = false,\n}\n\n\
        local bar = {\n\ta = true,\n\tb = false,\n\ta = false,\n\tc = \"hello\",\n\ta = false,\n}\n\n\
        local baz = {\n\ta = true,\n\tb = false,\n\t[\"a\"] = true,\n}\n\n\
        -- Ignore complex expressions\n\
        local barbaz = {\n\t[call()] = true,\n\t[call()] = false,\n}\n";
    const SELENE_NUMBER_INDICES: &str = "local foo = {\n\t\"foo\",\n\t\"bar\",\n\t[1] = true,\n}\n\n\
        local bar = {\n\t\"foo\",\n\t\"bar\",\n\t[123] = 15,\n\t[false] = true,\n\t\"baz\",\n\t\"foo\",\n\t[3] = \"5\",\n}\n\n\
        local baz = {\n\t[0] = true,\n\t[1] = false,\n\t[0] = false,\n}\n";

    #[test]
    fn reports_what_selene_reports_on_its_fixtures() {
        assert_eq!(lines(SELENE_DUPLICATE_KEYS), vec![4, 10, 12, 18]);
        assert_eq!(lines(SELENE_NUMBER_INDICES), vec![4, 14, 20]);
    }

    #[test]
    fn a_positional_item_fills_the_slot_its_position_names() {
        let src = "local a = { \"x\", [1] = \"y\" }\n\
                   local b = { [1] = \"y\", \"x\" }\n\
                   local c = { \"x\", \"y\", [2] = \"z\", [3] = \"ok\" }\n\
                   local d = { \"x\", f(), [2] = \"z\" }\n\
                   local e = { \"x\", [2] = \"ok\", [0] = \"ok\" }\n\
                   return { a, b, c, d, e }\n";
        assert_eq!(lines(src), vec![1, 2, 3, 4]);
    }

    #[test]
    fn number_keys_are_compared_by_value() {
        // One slot, however the number is written. Selene compares the
        // literal text and misses these; `["1"]` is a different slot.
        let src = "local a = { [1] = 1, [1.0] = 2 }\n\
                   local b = { [0x10] = 1, [16] = 2 }\n\
                   local c = { [-1] = 1, [-1] = 2 }\n\
                   local d = { [1e2] = 1, [100] = 2 }\n\
                   local e = { [0] = 1, [-0] = 2 }\n\
                   local f = { [\"1\"] = 1, [1] = 2 }\n\
                   local g = { [1] = 1, [-1] = 2, [1.5] = 3 }\n\
                   return { a, b, c, d, e, f, g }\n";
        assert_eq!(lines(src), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn a_trailing_call_or_varargs_fills_an_unknown_number_of_slots() {
        // Zero values or several: nothing can be said about slot 1.
        let src = "local function f(...)\n\
                   \tlocal a = { [1] = \"y\", ... }\n\
                   \tlocal b = { [1] = \"y\", g() }\n\
                   \tlocal c = { [1] = \"y\", (g()) }\n\
                   \treturn a, b, c\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src), vec![]);
    }

    #[test]
    fn every_later_occurrence_is_its_own_report_naming_the_first() {
        let src = "local t = {\n\t\"first\",\n\t[1] = \"second\",\n\t[1] = \"third\",\n}\nreturn t\n";
        let diags: Vec<_> = crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(12))
            .collect();
        assert_eq!(diags.len(), 2);
        for d in &diags {
            assert!(d.message.contains("`1`"), "{}", d.message);
            let first = d.labels.iter().find(|l| !l.primary).expect("a label on the first entry");
            assert_eq!(&src[first.span.clone()], "\"first\"");
            assert!(first.message.contains("positional"), "{}", first.message);
        }
    }

    #[test]
    fn a_positional_item_with_nothing_to_point_at_still_reports_at_the_key() {
        // A closure or an empty constructor carries no span in the HIR.
        let src = "local a = { function() end, [1] = \"y\" }\n\
                   local b = { [1] = \"y\", {} }\n\
                   return { a, b }\n";
        assert_eq!(lines(src), vec![1, 2]);
    }
}
