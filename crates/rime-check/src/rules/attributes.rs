//! R0044 unknown-function-attribute: `@name` before `function` where Luau
//! has no attribute called `name`, or has it twice (selene parity
//! `unknown_function_attribute`, 2026-11-20).
//!
//! Error, NOT conservative: Luau refuses to compile the file ("Invalid
//! attribute '@foo'", "Cannot duplicate attribute '@native'"), on a
//! function declaration, a local function and a function expression
//! alike. Same footing as R0043 const-reassign beside it.
//!
//! The known names are Luau's own list (`kAttributeEntries` in
//! `Ast/src/Parser.cpp`, read at `c87565b8b0db`; each checked with lune
//! 0.10.5). Luau also has debug attributes behind flags (`@debugnoinline`);
//! those are refused in a normal build and are reported here.
//!
//! Differences from Selene, all deliberate:
//!   * Selene knows only `native` and reports `@checked` and `@deprecated`.
//!     Luau compiles both; Rime is silent.
//!   * Selene checks `function f()` declarations only. Rime also checks
//!     `local function`, `const function` and function expressions.
//!   * Selene is silent on a repeated attribute. Luau refuses it.
//!
//! Not seen at all: the list form `@[native]`. full_moon 3.0 does not
//! parse it, so such a file is an R0900 before any rule runs.

use rime_diag::{Code, Diagnostic, Label};
use rime_hir as hir;

/// The attributes Luau accepts, sorted.
const KNOWN: &[&str] = &["checked", "deprecated", "native"];

pub fn check(body: &hir::Body, diags: &mut Vec<Diagnostic>) {
    for (i, attr) in body.attributes.iter().enumerate() {
        if !KNOWN.contains(&attr.name.as_str()) {
            let mut d = Diagnostic::error(
                Code(44),
                format!("unknown function attribute `@{}`", attr.name),
            )
            .with_label(Label::primary(
                attr.span.clone(),
                "Luau has no attribute by this name",
            ))
            .with_note(
                "Luau refuses to compile the file. The attributes it knows are \
                 `@native`, `@checked` and `@deprecated`"
                    .to_string(),
            );
            let lower = attr.name.to_ascii_lowercase();
            if KNOWN.contains(&lower.as_str()) {
                d = d.with_help(format!("attribute names are lower case: `@{lower}`"));
            }
            diags.push(d);
        } else if let Some(first) = body.attributes[..i].iter().find(|a| a.name == attr.name) {
            diags.push(
                Diagnostic::error(Code(44), format!("`@{}` is given twice", attr.name))
                    .with_label(Label::primary(
                        attr.span.clone(),
                        "this function already has the attribute",
                    ))
                    .with_label(Label::secondary(first.span.clone(), "first given here"))
                    .with_note(
                        "Luau refuses to compile the file: an attribute can be given \
                         once per function"
                            .to_string(),
                    )
                    .with_help("remove the repeat"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// (line, message, conservative) of every report, in source order.
    fn reports(src: &str) -> Vec<(usize, String, bool)> {
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        for body in &module.bodies {
            check(body, &mut diags);
        }
        let mut out: Vec<_> = diags
            .iter()
            .map(|d| {
                assert_eq!(d.code.to_string(), "R0044");
                (
                    src[..d.primary_span().start].matches('\n').count() + 1,
                    d.message.clone(),
                    d.conservative,
                )
            })
            .collect();
        out.sort();
        out
    }

    fn lines(src: &str) -> Vec<usize> {
        reports(src).into_iter().map(|(line, _, _)| line).collect()
    }

    // What Selene reports on a function
    // declaration: every attribute but `native`. Rime agrees on the ones
    // Luau has no name for.
    #[test]
    fn an_attribute_luau_does_not_know_is_reported() {
        let src = "\
local M = {}
@foo function M.a() end
@foo function M:b() end
@native @foo function M.c() end
@foo @bar function M.d() end
return M
";
        let got = reports(src);
        assert_eq!(
            got.iter().map(|(l, m, _)| (*l, m.as_str())).collect::<Vec<_>>(),
            vec![
                (2, "unknown function attribute `@foo`"),
                (3, "unknown function attribute `@foo`"),
                (4, "unknown function attribute `@foo`"),
                (5, "unknown function attribute `@bar`"),
                (5, "unknown function attribute `@foo`"),
            ]
        );
        // Luau refuses to compile the file ("Invalid attribute '@foo'").
        assert!(got.iter().all(|(_, _, conservative)| !conservative));
    }

    // Deliberate difference: Selene knows only `native` and reports
    // `@checked` and `@deprecated`. Luau compiles both (checked with lune
    // 0.10.5; `kAttributeEntries` in Luau's Parser.cpp lists exactly these
    // three), so Rime is silent on them.
    #[test]
    fn the_three_attributes_luau_knows_are_silent() {
        let src = "\
@native function a() end
@checked function b() end
@deprecated function c() end
@native @checked local function d() end
return a, b, c, d
";
        assert_eq!(lines(src), Vec::<usize>::new());
    }

    // Deliberate difference: Selene checks `function f()` declarations
    // only. Luau refuses an unknown attribute on a local function and on a
    // function expression just the same.
    #[test]
    fn local_functions_and_function_expressions_are_checked_too() {
        let src = "\
@foo local function a() end
local b = @foo function() end
@foo const function c() end
return a, b, c
";
        assert_eq!(lines(src), vec![1, 2, 3]);
    }

    // Deliberate difference: Selene is silent on a repeated attribute.
    // Luau refuses it ("Cannot duplicate attribute '@native'"). The report
    // is on the repeat and points back at the first.
    #[test]
    fn a_repeated_attribute_is_reported_once_at_the_repeat() {
        let src = "@native @checked @native function a() end\nreturn a\n";
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        for body in &module.bodies {
            check(body, &mut diags);
        }
        assert_eq!(diags.len(), 1, "{diags:?}");
        let d = &diags[0];
        assert_eq!(d.message, "`@native` is given twice");
        assert!(!d.conservative);
        assert_eq!(d.primary_span().start, src.rfind("@native").unwrap());
        assert!(d.labels.iter().any(|l| l.span.start == 0), "{:?}", d.labels);
    }

    // Attribute names are case-sensitive in Luau: `@Native` is unknown.
    #[test]
    fn a_miscased_attribute_gets_the_right_spelling() {
        let src = "@Native function a() end\nreturn a\n";
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        for body in &module.bodies {
            check(body, &mut diags);
        }
        assert_eq!(diags.len(), 1);
        assert!(diags[0].help.iter().any(|h| h.contains("`@native`")), "{:?}", diags[0].help);
    }
}