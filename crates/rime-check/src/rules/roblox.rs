//! R0801, R0804: Roblox constructor checks — selene-parity lints (decided
//! 2026-08-21) over constructor calls whose arguments are visibly wrong as
//! written. Warnings: the call runs without raising (channels clamp, the
//! missing arguments default to 0) and the result is a wrong render, which
//! a project raises to "error" in rime.toml if it wants the run to fail.
//! Conservative-flagged (valid Luau, and the globals could in principle be
//! shadowed). Pure HIR walks.
//!
//!   R0801 color3-bounds          `Color3.new(255, 0, 0)` — channels are 0–1
//!   R0804 suspicious-udim2-new   `UDim2.new(0.5, 0.5)` — it takes four
//!
//! R0804 counts arguments and never looks at their values. A different
//! `UDim2` check, on the VALUE in a scale slot (integer > 1 = pixels in a
//! scale slot), was built and dropped again: every hit on real code was
//! deliberate — off-screen tween positions (`UDim2.new(-2, 0, 0.25, 0)`),
//! BillboardGui sizes in studs (`UDim2.new(6, 0, h, 0)`), and test
//! fixtures. The accept case locks those
//! shapes silent. Do not rebuild it without new evidence.

use rime_diag::{Code, Diagnostic, Label, Suggestion};
use rime_hir::{self as hir, visit};

pub fn check(body: &hir::Body, src: &str, diags: &mut Vec<Diagnostic>) {
    visit::exprs(body, &mut |e| {
        color3_bounds(e, src, diags);
        udim2_new_arg_count(e, diags);
    });
}

/// A `Base.method(...)` call on a plain global name.
struct ConstructorCall<'a> {
    args: &'a [hir::Expr],
    /// The global name: where the call expression starts.
    root_span: &'a hir::Span,
    /// The call suffix, `(` through `)`. `Expr::Call::span` does not cover
    /// the callee.
    span: &'a hir::Span,
}

/// `Base.method(...)` on a plain global name — the constructor shape both
/// rules match.
fn constructor_call<'a>(
    e: &'a hir::Expr,
    global: &str,
    method: &str,
) -> Option<ConstructorCall<'a>> {
    let hir::Expr::Call { base, args, method: None, span, .. } = e else {
        return None;
    };
    let hir::Expr::Index { base: root, name: Some(n), key: None, .. } = &**base else {
        return None;
    };
    let hir::Expr::Name { name: g, span: root_span } = &**root else {
        return None;
    };
    (g == global && n == method).then_some(ConstructorCall { args: args.as_slice(), root_span, span })
}

/// The numeric value of a literal argument, seeing through `-`.
/// None for anything non-literal — those stay silent.
fn literal_number(e: &hir::Expr) -> Option<f64> {
    let mut neg = false;
    let mut e = e;
    while let hir::Expr::UnOp { op: hir::UnOp::Neg, operand, .. } = e {
        neg = !neg;
        e = operand;
    }
    let hir::Expr::Literal { prim: hir::Prim::Number, text: Some(t), .. } = e else {
        return None;
    };
    number_value(t).map(|v| if neg { -v } else { v })
}

// --- R0801 color3-bounds --------------------------------------------------

fn color3_bounds(e: &hir::Expr, src: &str, diags: &mut Vec<Diagnostic>) {
    let Some(call) = constructor_call(e, "Color3", "new") else {
        return;
    };
    let (args, span) = (call.args, call.span);
    let out_of_range = args
        .iter()
        .filter_map(literal_number)
        .any(|v| !(0.0..=1.0).contains(&v));
    if !out_of_range {
        return;
    }
    let mut d = Diagnostic::warning(Code(801), "`Color3.new` takes channels from 0 to 1")
        .with_label(Label::primary(
            span.clone(),
            "a channel outside 0–1 clamps: this is not the color it reads as",
        ))
        .with_note(
            "0–255 channel values belong to `Color3.fromRGB`; `Color3.new(255, 0, 0)` \
             happens to clamp to pure red, but mixed channels render as the \
             wrong color entirely — every channel above 1 flattens to 1"
                .to_string(),
        )
        .conservative(
            "channels are clamped, not rejected, at runtime — Rime cannot rule \
             out a deliberate out-of-range value feeding later math",
        );
    if let Some(rgb) = from_rgb_rewrite(&call, src) {
        let message = if rgb.len() <= 80 && !rgb.contains('\n') {
            format!("if these are 0–255 values: `{rgb}`")
        } else {
            "if these are 0–255 values: call `Color3.fromRGB` instead".to_string()
        };
        d = d.with_suggestion(Suggestion {
            message,
            // The whole call, from the `Color3` name.
            span: call.root_span.start..span.end,
            replacement: rgb,
        });
    }
    diags.push(d);
}

/// `Color3.new(255, 128, 0)` rewritten as `Color3.fromRGB(255, 128, 0)`:
/// the constructor name changes, the argument list keeps its own text, so
/// the edit never invents a channel value. Offered only when the 0–255
/// reading is the one the numbers support — three literal channels, each a
/// whole number from 0 to 255. A fraction among them (`0.5, 0.5, 2`) reads
/// as 0–1 channels with one slip, and there is no single edit for that.
/// None as well when the callee is not written as plain `Color3.new` (a
/// comment inside it would be dropped).
fn from_rgb_rewrite(call: &ConstructorCall<'_>, src: &str) -> Option<String> {
    let channels: Vec<f64> = call.args.iter().filter_map(literal_number).collect();
    if call.args.len() != 3 || channels.len() != 3 {
        return None;
    }
    if !channels.iter().all(|v| v.fract() == 0.0 && (0.0..=255.0).contains(v)) {
        return None;
    }
    let callee: String = src
        .get(call.root_span.start..call.span.start)?
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let arguments = src.get(call.span.clone())?;
    if callee != "Color3.new" || !arguments.starts_with('(') || !arguments.ends_with(')') {
        return None;
    }
    Some(format!("Color3.fromRGB{arguments}"))
}

// --- R0804 suspicious-udim2-new --------------------------------------------

/// `UDim2.new` with one, two or three arguments. It takes four numbers
/// (xScale, xOffset, yScale, yOffset); the ones left out are 0, so a
/// two-number call sets the X axis only. That is rarely what
/// `UDim2.new(0.5, 0.5)` was written to mean — `UDim2.fromScale` and
/// `UDim2.fromOffset` are the two-number forms.
///
/// Silent, as Selene's `roblox_suspicious_udim2_new` is, on no arguments,
/// on four or more, and on exactly two with no number literal among them
/// (`UDim2.new(UDim, UDim)` is a real overload). Silent also where the last
/// argument can be several values (a call, `...`): the count is not known
/// there. Selene counts written arguments and reports those.
fn udim2_new_arg_count(e: &hir::Expr, diags: &mut Vec<Diagnostic>) {
    const PARAMS: [&str; 4] = ["xScale", "xOffset", "yScale", "yOffset"];
    let Some(call) = constructor_call(e, "UDim2", "new") else {
        return;
    };
    let args = call.args;
    if args.is_empty() || args.len() >= PARAMS.len() {
        return;
    }
    if matches!(
        args.last(),
        Some(hir::Expr::Call { .. } | hir::Expr::Leaf | hir::Expr::Composite(_))
    ) {
        return;
    }
    let numbers: Vec<f64> = args.iter().filter_map(literal_number).collect();
    if args.len() == 2 && numbers.is_empty() {
        return;
    }
    let given = match args.len() {
        1 => "1 was given".to_string(),
        n => format!("{n} were given"),
    };
    let left_out = match &PARAMS[args.len()..] {
        [one] => format!("`{one}` is left out, so it is 0"),
        [first @ .., last] => format!(
            "{} and `{last}` are left out, so they are 0",
            first.iter().map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", ")
        ),
        [] => unreachable!(),
    };
    // Which two-number form the values read as, when both are literals.
    // Advice, not an edit: the call does not say which reading was meant.
    let help = if args.len() == 2 && numbers.len() == 2 {
        if numbers.iter().all(|v| (0.0..=1.0).contains(v)) {
            "for a scale on each axis use `UDim2.fromScale(x, y)`"
        } else {
            "for pixels on each axis use `UDim2.fromOffset(x, y)`"
        }
    } else {
        "pass all four arguments, writing the 0s out if they are meant"
    };
    diags.push(
        Diagnostic::warning(Code(804), format!("`UDim2.new` takes 4 numbers, but {given}"))
            .with_label(Label::primary(call.root_span.start..call.span.end, left_out))
            .with_note(
                "the arguments are xScale, xOffset, yScale, yOffset, in that \
                 order: the first two set the X axis only"
                    .to_string(),
            )
            .conservative(
                "the call runs, with 0 for every argument left out — Rime cannot \
                 tell a deliberate short form from a mix-up with \
                 `UDim2.fromScale` or `UDim2.fromOffset`",
            )
            .with_help(help),
    );
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

    fn body(line: &str) -> String {
        format!("local function f(x)\n\t{line}\nend\nreturn f\n")
    }

    /// Lines of every diagnostic `code` in `src`.
    fn lines(src: &str, code: u16) -> Vec<usize> {
        crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(code))
            .map(|d| src[..d.primary_span().start].matches('\n').count() + 1)
            .collect()
    }

    /// Selene's own `roblox_suspicious_udim2_new` fixture.
    /// Its `.stderr` file is empty there, so the expected lines are
    /// what running Selene on it reports.
    const SELENE_UDIM2_NEW: &str = "UDim2.new(0)\n\
        UDim2.new(0.5)\n\
        UDim2.new(1)\n\
        UDim2.new(2)\n\
        UDim2.new(a)\n\
        UDim2.new(1, 1)\n\
        UDim2.new(1, 2)\n\
        UDim2.new(a, b)\n\
        UDim2.new(1, a)\n\
        UDim2.new(1, 1, 1)\n\
        UDim2.new(a, b, c)\n\
        UDim2.new(1, 1, 1, 1)\n\
        UDim2.new(a, b, c, d)\n\
        UDim2.fromOffset(1, 1)\n\
        UDim2.fromScale(1, 1)\n\
        UDim2.new()\n";

    #[test]
    fn udim2_new_reports_what_selene_reports_on_its_fixture() {
        assert_eq!(lines(SELENE_UDIM2_NEW, 804), vec![1, 2, 3, 4, 5, 6, 7, 9, 10, 11]);
    }

    #[test]
    fn udim2_new_with_two_udims_is_the_other_constructor() {
        // `UDim2.new(UDim, UDim)` is a real overload: two arguments, neither
        // a number literal, is silent (as in Selene).
        let src = "local function f(x, y)\n\
                   \tlocal a = UDim2.new(UDim.new(0, 5), UDim.new(1, 0))\n\
                   \tlocal b = UDim2.new(x, y)\n\
                   \tlocal c = UDim2.new(x.X, y.Y)\n\
                   \treturn a, b, c\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 804), vec![]);
    }

    #[test]
    fn udim2_new_is_silent_when_the_last_argument_can_be_several_values() {
        // A trailing call or `...` can supply the rest, so the count is
        // not known. Selene counts written arguments and reports these.
        let src = "local function f(t, ...)\n\
                   \tlocal a = UDim2.new(unpack(t))\n\
                   \tlocal b = UDim2.new(...)\n\
                   \tlocal c = UDim2.new(1, 0, table.unpack(t))\n\
                   \tlocal d = UDim2.new(0.5, ...)\n\
                   \treturn a, b, c, d\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 804), vec![]);
    }

    #[test]
    fn udim2_new_counts_a_negative_literal_as_a_number() {
        let src = "local function f(v)\n\
                   \tlocal a = UDim2.new(-1, -1)\n\
                   \tlocal b = UDim2.new(-1, v)\n\
                   \tlocal c = UDim2.new(1, 0, v)\n\
                   \tlocal d = UDim2.new(v)\n\
                   \treturn a, b, c, d\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 804), vec![2, 3, 4, 5]);
    }

    #[test]
    fn udim2_new_only_matches_the_global_constructor() {
        let src = "local function f(Other, v)\n\
                   \tlocal a = Other.new(1, 1)\n\
                   \tlocal b = v.UDim2.new(1, 1)\n\
                   \tlocal c = UDim2:new(1, 1)\n\
                   \tlocal d = UDim.new(1)\n\
                   \tlocal e = UDim2.new(1, 0, 1, 0, 5)\n\
                   \treturn a, b, c, d, e\n\
                   end\n\
                   return f\n";
        assert_eq!(lines(src, 804), vec![]);
    }

    #[test]
    fn udim2_new_says_how_many_and_points_at_the_two_argument_forms() {
        let report = |call: &str| {
            let src = format!("local function f(v)\n\treturn {call}\nend\nreturn f\n");
            let d = crate::check_source(&src)
                .into_iter()
                .find(|d| d.code == rime_diag::Code(804))
                .unwrap_or_else(|| panic!("R0804 on {call}"));
            // Advice only: which of the three readings was meant is not
            // something the call says.
            assert!(d.suggestions.is_empty(), "{call}");
            assert!(d.conservative, "{call}");
            assert_eq!(&src[d.primary_span()], call, "the label covers the whole call");
            (d.message.clone(), d.help.join(" "))
        };
        let (message, help) = report("UDim2.new(0.5, 0.5)");
        assert!(message.contains("4 numbers") && message.contains("2 were given"), "{message}");
        assert!(help.contains("UDim2.fromScale"), "{help}");
        let (_, help) = report("UDim2.new(200, 50)");
        assert!(help.contains("UDim2.fromOffset"), "{help}");
        let (message, help) = report("UDim2.new(v)");
        assert!(message.contains("1 was given"), "{message}");
        assert!(!help.contains("fromScale") && !help.contains("fromOffset"), "{help}");
        let (message, _) = report("UDim2.new(1, 0, 1)");
        assert!(message.contains("3 were given"), "{message}");
    }

    #[test]
    fn color3_edit_swaps_the_constructor_and_keeps_the_arguments() {
        let cases = [
            ("return Color3.new(255, 128, 0)", "return Color3.fromRGB(255, 128, 0)"),
            ("return Color3.new( 255,128 , 0 )", "return Color3.fromRGB( 255,128 , 0 )"),
            ("return Color3.new(0xFF, 0x80, 0)", "return Color3.fromRGB(0xFF, 0x80, 0)"),
            ("return Color3.new(255, 255, 255):Lerp(x, 0.5)", "return Color3.fromRGB(255, 255, 255):Lerp(x, 0.5)"),
            ("return x(Color3.new(12, 34, 56), 1)", "return x(Color3.fromRGB(12, 34, 56), 1)"),
        ];
        for (before, after) in cases {
            assert_eq!(fixed(&body(before), 801), Some(body(after)), "{before}");
        }
    }

    #[test]
    fn color3_edit_is_withheld_unless_every_channel_reads_as_0_to_255() {
        let cases = [
            // A fraction is a 0–1 channel; the out-of-range one is a typo
            // or overbright on purpose, not a 0–255 colour.
            "return Color3.new(0.5, 0.5, 2)",
            "return Color3.new(1.5, 0, 0)",
            // Nothing `fromRGB` accepts.
            "return Color3.new(-1, 0, 0)",
            "return Color3.new(300, 0, 0)",
            // Not all literals, or not three channels.
            "return Color3.new(255, x, 0)",
            "return Color3.new(255, 128)",
            // A comment the rewrite would drop.
            "return Color3.new --[[ rgb ]] (255, 128, 0)",
        ];
        for case in cases {
            assert_eq!(fixed(&body(case), 801), None, "{case}");
        }
    }
}
