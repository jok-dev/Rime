//! R0611 bad-string-escape and R0707 needless-quote-escape: what a
//! backslash inside a string literal does. Together they are Selene's
//! `bad_string_escape`, split by what is at stake:
//!
//!   R0611  error, firm         an escape Luau refuses: the file does not
//!                              compile (`\300`, `\xZ`, `\u1234`,
//!                              `\u{110000}`)
//!   R0611  error, conservative a backslash before a character that is no
//!                              escape (`\d`, `\.`, `C:\Users`): Luau drops
//!                              the backslash and keeps the character
//!   R0707  warning             `\'` in a `"…"` string, `\"` in a `'…'`
//!                              string: the backslash changes nothing.
//!                              Quoted strings only, as in Selene.
//!
//! The escape rules are Luau's lexer's (`Lexer::fixupQuotedString`, read
//! at c87565b8b0db and checked case by case with lune 0.10.5), not
//! Selene's. Where the two disagree, Rime follows Luau:
//!
//! - `\256` to `\299`: Luau refuses, Selene is silent. Reported.
//! - `\x414`: `\x41` then a `4`. Luau compiles it, Selene reports it. Silent.
//! - `\u{100000041}`: Luau keeps the low 32 bits and compiles it, Selene
//!   calls it too high. Silent.
//! - `\u{…}` with 17 digits or more: Luau refuses, Selene is silent.
//!   Reported.
//! - Selene checks a string only where it is an expression of its own.
//!   The lexer has one rule for all of them, so a call's string argument
//!   (`f"…"`), a backtick string and a string in type position are
//!   checked too. In a backtick string `\{`, `\}`, `` \` `` and an escaped
//!   quote are not reported.
//!
//! The strings come from lowering (`hir::SyntaxNotes::string_tokens`): the
//! tree keeps what a string says, not how it was written.

use rime_diag::{Code, Diagnostic, Label, Suggestion};
use rime_hir as hir;

const REFUSED: &str = "Luau refuses to compile the file: \"String literal contains malformed escape sequence\"";

#[derive(Clone, Copy, PartialEq)]
enum Quote {
    Double,
    Single,
    Backtick,
}

pub fn check(body: &hir::Body, src: &str, diags: &mut Vec<Diagnostic>) {
    let mut tokens = body.notes.string_tokens.clone();
    tokens.sort_by_key(|s| (s.start, s.end));
    tokens.dedup();
    for token in tokens {
        let Some(text) = src.get(token.clone()) else { continue };
        let quote = match text.as_bytes().first() {
            Some(b'"') => Quote::Double,
            Some(b'\'') => Quote::Single,
            // A backtick string's later pieces start at the `}` that ends
            // the expression before them.
            Some(b'`' | b'}') => Quote::Backtick,
            // A long string (`[[…]]`) has no escapes.
            _ => continue,
        };
        if let Some(content) = text.len().checked_sub(1).and_then(|end| text.get(1..end)) {
            scan(quote, content, token.start + 1, diags);
        }
    }
}

/// Walks one string's content the way Luau's lexer does. `base` is where
/// `content` starts in the file.
fn scan(quote: Quote, content: &str, base: usize, diags: &mut Vec<Diagnostic>) {
    let bytes = content.as_bytes();
    let hex = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_hexdigit);
    let mut i = 0;
    while i < bytes.len() {
        // A `\` byte is never part of a longer UTF-8 character.
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let start = i;
        let Some(c) = content[i + 1..].chars().next() else { break };
        i += 1 + c.len_utf8();
        match c {
            // `\` at the end of a line continues the string on the next;
            // `\z` skips the whitespace after it.
            'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | '\\' | '\n' | '\r' | 'z' => {}
            // Luau refuses a NUL anywhere in a quoted string. Not an
            // escape's doing, and not this rule's to report.
            '\0' => {}
            // A backtick string spells a brace `\{` and a backtick `` \` ``.
            // `\}` is written to pair with `\{`, and neither quote delimits
            // the string, so an escaped one is habit, not a slip.
            '`' | '{' | '}' | '"' | '\'' if quote == Quote::Backtick => {}
            '"' | '\'' => {
                if (c == '"') != (quote == Quote::Double) {
                    diags.push(needless(quote, c, base + start));
                }
            }
            '0'..='9' => {
                let mut digits = 1;
                while digits < 3 && bytes.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                    digits += 1;
                }
                let written = &content[start + 1..i];
                let code: u32 = written.parse().unwrap_or(0);
                if code > 255 {
                    let mut d = refused(
                        format!("decimal escape `\\{written}` is above 255"),
                        base + start..base + i,
                        "a decimal escape is one byte: 0 to 255",
                    )
                    .with_help(format!("the code point {code} is written `\\u{{{code:X}}}`"));
                    // `\300` is a byte in C, where the digits are octal.
                    if let Ok(byte @ 0..=255) = u32::from_str_radix(written, 8) {
                        d = d.with_help(format!(
                            "Luau reads the digits as decimal; if they are octal, the byte is `\\x{byte:02X}`"
                        ));
                    }
                    diags.push(d);
                }
            }
            'x' => {
                let mut digits = 0;
                while digits < 2 && hex(i) {
                    i += 1;
                    digits += 1;
                }
                if digits < 2 {
                    let mut d = refused(
                        "`\\x` escape without two hexadecimal digits".to_string(),
                        base + start..base + i,
                        "`\\x` takes exactly two: `\\x0A`",
                    );
                    if digits == 1 {
                        d = d.with_help(format!("a single digit takes a zero before it: `\\x0{}`", &content[i - 1..i]));
                    }
                    diags.push(d);
                }
            }
            'u' => {
                if bytes.get(i) != Some(&b'{') {
                    let mut end = i;
                    while end - i < 6 && hex(end) {
                        end += 1;
                    }
                    let mut d = refused(
                        "`\\u` escape without braces".to_string(),
                        base + start..base + i,
                        "`\\u` takes a code point in braces",
                    );
                    if end > i {
                        d = d.with_help(format!("write `\\u{{{}}}`", &content[i..end]));
                    }
                    diags.push(d);
                    continue;
                }
                i += 1;
                if bytes.get(i) == Some(&b'}') {
                    i += 1;
                    diags.push(refused(
                        "`\\u{}` escape with no digits".to_string(),
                        base + start..base + i,
                        "the braces hold a code point: `\\u{1F600}`",
                    ));
                    continue;
                }
                // Luau reads up to 16 digits into 32 bits and lets the
                // high ones fall off: `\u{100000041}` is `A`.
                let (mut code, mut digits) = (0u32, 0);
                while digits < 16 && hex(i) {
                    let digit = (bytes[i] as char).to_digit(16).unwrap_or(0);
                    code = code.wrapping_mul(16).wrapping_add(digit);
                    i += 1;
                    digits += 1;
                }
                if bytes.get(i) != Some(&b'}') {
                    diags.push(refused(
                        "malformed `\\u{…}` escape".to_string(),
                        base + start..base + i,
                        "the braces hold 1 to 16 hexadecimal digits and nothing else",
                    ));
                    continue;
                }
                i += 1;
                if code > 0x10FFFF {
                    diags.push(refused(
                        format!("`{}` is above the last code point, U+10FFFF", &content[start..i]),
                        base + start..base + i,
                        "no such code point",
                    ));
                }
            }
            _ => diags.push(unknown(c, base + start..base + i)),
        }
    }
}

fn refused(message: String, span: hir::Span, label: &str) -> Diagnostic {
    Diagnostic::error(Code(611), message)
        .with_label(Label::primary(span, label))
        .with_note(REFUSED)
}

fn unknown(c: char, span: hir::Span) -> Diagnostic {
    let shown = if c.is_control() { format!("U+{:04X}", c as u32) } else { c.to_string() };
    let mut d = Diagnostic::error(Code(611), format!("`\\{shown}` is not an escape sequence"))
        .with_label(Label::primary(
            span,
            format!("Luau drops the backslash: this is a plain `{shown}`"),
        ))
        .conservative(
            "the file compiles, and Rime cannot tell whether the backslash \
             was meant to be part of the string",
        )
        .with_help(format!(
            "write `\\\\{shown}` to keep the backslash, or `{shown}` alone to say what Luau reads"
        ));
    // `\d`, `\.`: regular-expression habits. A Luau pattern escapes with `%`.
    if c.is_ascii_punctuation() || matches!(c, 'd' | 's' | 'w') {
        d = d.with_help(format!(
            "if this string is a pattern, its escape character is `%`: `%{c}`"
        ));
    }
    d
}

fn needless(quote: Quote, c: char, backslash: usize) -> Diagnostic {
    let kind = if quote == Quote::Double { "double-quoted" } else { "single-quoted" };
    Diagnostic::warning(Code(707), format!("`\\{c}` needs no backslash in a {kind} string"))
        .with_label(Label::primary(
            backslash..backslash + 2,
            format!("a plain `{c}` is the same string"),
        ))
        .conservative("valid Luau: the string is the same with or without the backslash")
        .with_suggestion(Suggestion {
            message: "remove the backslash".to_string(),
            span: backslash..backslash + 1,
            replacement: String::new(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn diags(src: &str) -> Vec<Diagnostic> {
        let module = rime_syntax::parse(src).unwrap();
        let mut diags = Vec::new();
        for body in &module.bodies {
            check(body, src, &mut diags);
        }
        diags.sort_by_key(|d| d.primary_span().start);
        diags
    }

    /// (line, code, reported text, firm) of every report, in source order.
    /// "Firm" is an error that is not conservative.
    fn reports(src: &str) -> Vec<(usize, String, &str, bool)> {
        diags(src)
            .iter()
            .map(|d| {
                let span = d.primary_span();
                (
                    src[..span.start].matches('\n').count() + 1,
                    d.code.to_string(),
                    &src[span],
                    d.severity == rime_diag::Severity::Error && !d.conservative,
                )
            })
            .collect()
    }

    fn of_line<'a>(all: &[(usize, String, &'a str, bool)], line: usize) -> Vec<(String, &'a str, bool)> {
        all.iter().filter(|r| r.0 == line).map(|r| (r.1.clone(), r.2, r.3)).collect()
    }

    fn firm(text: &str) -> (String, &str, bool) {
        ("R0611".to_string(), text, true)
    }
    fn unknown(text: &str) -> (String, &str, bool) {
        ("R0611".to_string(), text, false)
    }
    fn needless(text: &str) -> (String, &str, bool) {
        ("R0707".to_string(), text, false)
    }

    // Selene (roblox mode) was run on this
    // text, and every line was compiled with lune 0.10.5 to see what Luau
    // does. Each test below says where Rime differs from Selene, and why.
    const FIXTURE: &str = r#"local a = "\300 \256 \299 \255 \2555"
local b = "\x1 \xZZ \x4g \x414 \x41"
local c = "\u{110000} \u{12 \u{} \u1234 \u{ 41 } \u{D800} \u{10FFFF} \u{100000041} \u{00000000000000041}"
local d = "\q \{ \/ \- \% \. \d \s \w"
local e = "\' \""
local e2 = '\' \"'
local f = print"\q \300"
local g = print'\"'
local h = `\q \300 \{ \` {a} \d \" \' \}`
type T = "\q" | "\300"
local i = "\d\d\d"
local j = { ["\q"] = 1 }
local k = "ok \a \b \f \n \r \t \v \\ \065 \z
   x"
return a, b, c, d, e, e2, f, g, h, i, j, k
"#;

    // Luau refuses a decimal escape above 255. Selene reports `\300` and
    // misses `\256` and `\299` (it tests the digits one by one).
    #[test]
    fn decimal_escape_above_255_is_firm() {
        let all = reports(FIXTURE);
        assert_eq!(of_line(&all, 1), vec![firm(r"\300"), firm(r"\256"), firm(r"\299")]);
    }

    // `\x` takes exactly two hex digits. Selene also reports `\x414`,
    // which is `\x41` followed by a `4` and compiles; Rime does not.
    #[test]
    fn hex_escape_without_two_digits_is_firm() {
        let all = reports(FIXTURE);
        assert_eq!(of_line(&all, 2), vec![firm(r"\x1"), firm(r"\x"), firm(r"\x4")]);
    }

    // `\u{…}`: braces, 1 to 16 hex digits, at most U+10FFFF. Where Rime
    // and Selene differ:
    //   `\u{100000041}`  Selene: too high. Luau keeps the low 32 bits
    //                    (0x41) and compiles it. Rime is silent.
    //   17 digits        Selene: silent. Luau refuses. Rime reports.
    //   `\u{}`           both report; Selene calls it "too high".
    //   `\u{D800}`       both silent (Luau encodes a lone surrogate).
    #[test]
    fn unicode_escape_luau_refuses_is_firm() {
        let all = reports(FIXTURE);
        assert_eq!(
            of_line(&all, 3),
            vec![
                firm(r"\u{110000}"),
                firm(r"\u{12"),
                firm(r"\u{}"),
                firm(r"\u"),
                firm(r"\u{"),
                // 16 of the 17 digits are read; the next is not the `}`.
                firm(r"\u{0000000000000004"),
            ]
        );
        let messages: Vec<String> = diags(FIXTURE)
            .into_iter()
            .filter(|d| FIXTURE[..d.primary_span().start].matches('\n').count() == 2)
            .map(|d| d.message)
            .collect();
        assert_eq!(
            messages,
            vec![
                r"`\u{110000}` is above the last code point, U+10FFFF",
                r"malformed `\u{…}` escape",
                r"`\u{}` escape with no digits",
                r"`\u` escape without braces",
                r"malformed `\u{…}` escape",
                r"malformed `\u{…}` escape",
            ]
        );
    }

    // An escape Luau does not know: it drops the backslash. The file
    // compiles, so the report is conservative. Same lines as Selene.
    #[test]
    fn unknown_escape_is_conservative() {
        let all = reports(FIXTURE);
        assert_eq!(
            of_line(&all, 4),
            [r"\q", r"\{", r"\/", r"\-", r"\%", r"\.", r"\d", r"\s", r"\w"].map(unknown).to_vec()
        );
        // One report per escape, not per string.
        assert_eq!(of_line(&all, 11), vec![unknown(r"\d"), unknown(r"\d"), unknown(r"\d")]);
        assert_eq!(of_line(&all, 12), vec![unknown(r"\q")]);
    }

    #[test]
    fn needless_quote_escape_is_a_warning_with_the_edit() {
        let all = reports(FIXTURE);
        assert_eq!(of_line(&all, 5), vec![needless(r"\'")]);
        assert_eq!(of_line(&all, 6), vec![needless("\\\"")]);
        for d in diags(FIXTURE).into_iter().filter(|d| d.code.to_string() == "R0707") {
            assert_eq!(d.severity, rime_diag::Severity::Warning);
            assert!(d.conservative);
            assert_eq!(d.suggestions.len(), 1);
            let edit = &d.suggestions[0];
            assert_eq!(&FIXTURE[edit.span.clone()], "\\");
            assert_eq!(edit.replacement, "");
        }
    }

    // Selene checks a string only where it is an expression of its own.
    // Luau's lexer has one rule for every string, so Rime also checks a
    // call's string argument, a backtick string and a string in a type.
    #[test]
    fn every_kind_of_string_is_checked() {
        let all = reports(FIXTURE);
        assert_eq!(of_line(&all, 7), vec![unknown(r"\q"), firm(r"\300")]);
        assert_eq!(of_line(&all, 8), vec![needless("\\\"")]);
        // `\{` and `` \` `` are how a backtick string spells a brace and a
        // backtick, and `\}` is written to pair with `\{`: not reported
        // there. Nor is an escaped quote: neither kind delimits a backtick
        // string, and Selene has never asked anyone to unescape one.
        assert_eq!(of_line(&all, 9), vec![unknown(r"\q"), firm(r"\300"), unknown(r"\d")]);
        assert_eq!(of_line(&all, 10), vec![unknown(r"\q"), firm(r"\300")]);
    }

    #[test]
    fn valid_escapes_are_silent() {
        let all = reports(FIXTURE);
        assert_eq!(of_line(&all, 13), vec![]);
        assert_eq!(of_line(&all, 14), vec![]);
        let src = "local s = \"a\\\nb \\\r\nc \\\\q \\\\\" .. '\\'' .. \"\\\"\" .. [[\\q \\300]] .. [==[\\x]==]\nreturn s\n";
        assert_eq!(reports(src), vec![]);
    }

    // Luau refuses a NUL character anywhere in a quoted string, escaped or
    // not ("Malformed string", lune 0.10.5). That is not about escapes and
    // Rime has no rule for it; R0611 must not call `\<NUL>` an escape Luau
    // reads as a plain character.
    #[test]
    fn a_nul_after_a_backslash_is_not_called_an_unknown_escape() {
        assert_eq!(reports("return \"a\\\0b\"\n"), vec![]);
        assert_eq!(reports("return \"a\0b\"\n"), vec![]);
    }

    // Literal braces in a backtick string have to be escaped, or they
    // would open an interpolation.
    #[test]
    fn an_escaped_closing_brace_in_a_backtick_string_is_silent() {
        let src = "local id = 7\nreturn `\\{ id = {id}, tags = \\{\\} \\}`\n";
        assert_eq!(reports(src), vec![]);
        // Outside a backtick string `\}` pairs with nothing.
        assert_eq!(reports("return \"\\}\"\n").len(), 1);
    }

    #[test]
    fn nothing_else_in_the_fixture_is_reported() {
        assert_eq!(reports(FIXTURE).len(), 3 + 3 + 6 + 9 + 2 + 2 + 1 + 3 + 2 + 3 + 1);
    }

    #[test]
    fn unknown_escape_says_what_luau_reads_and_names_the_pattern_escape() {
        let src = "local a, b = \"C:\\Users\", \"%d+\\.%d+\"\nreturn a, b\n";
        let got = diags(src);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].message, r"`\U` is not an escape sequence");
        assert!(got[0].labels[0].message.contains("plain `U`"), "{:?}", got[0].labels);
        assert!(got[0].help.iter().any(|h| h.contains(r"`\\U`")), "{:?}", got[0].help);
        assert!(!got[0].help.iter().any(|h| h.contains('%')), "{:?}", got[0].help);
        assert_eq!(got[1].message, r"`\.` is not an escape sequence");
        assert!(got[1].help.iter().any(|h| h.contains("`%.`")), "{:?}", got[1].help);
        assert!(got.iter().all(|d| d.suggestions.is_empty()));
    }

    #[test]
    fn decimal_escape_says_how_to_write_the_code_point() {
        let got = diags("return \"\\300\"\n");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].message, r"decimal escape `\300` is above 255");
        assert!(got[0].help.iter().any(|h| h.contains(r"`\u{12C}`")), "{:?}", got[0].help);
        assert!(got[0].notes.iter().any(|n| n.contains("refuses to compile")), "{:?}", got[0].notes);
    }
}