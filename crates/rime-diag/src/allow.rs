//! Inline suppression: `-- rime: allow(<rule>, "reason")` and
//! `-- rime: allow-file(<rule>, "reason")`.
//!
//! Line form, two placements:
//!
//! ```lua
//! local x = require(t) -- rime: allow(dynamic-require, "tunes are legacy")
//! -- rime: allow(dynamic-require, "tunes are legacy")
//! local y = require(t)
//! ```
//!
//! Trailing form suppresses matching diagnostics on its own line; own-line
//! form suppresses the NEXT line (consecutive directives stack).
//!
//! A diagnostic is "on" a line when its primary label starts there. A rule
//! whose label points inside a construct that can run over several lines
//! may also name where that construct begins (`Diagnostic::allow_anchor`);
//! such a diagnostic is on that line as well. R0304 does, with the start
//! of the call, so a comment above a call written over several lines
//! silences the extra argument reported further down:
//!
//! ```lua
//! -- rime: allow(call-extra-args, "old callers still pass the id")
//! Shop.buy(
//!     player,
//!     itemId
//! )
//! ```
//!
//! The reach is those two lines and no more: not a call that starts on a
//! later line of the same statement, not the next statement, not another
//! rule.
//!
//! File form suppresses every matching diagnostic in the file, and must sit
//! ABOVE the first code line (only comments and blank lines before it, and
//! nothing but whitespace to its left) — suppressions stay discoverable at
//! the top. A buried or trailing `allow-file` suppresses nothing and gets
//! `R0021` saying where it belongs. Multiple directives stack, one rule
//! each.
//!
//! `<rule>` is a name from `RULE_NAMES` or a raw `R####` code. The reason
//! is mandatory: a directive without one suppresses nothing and gets
//! `R0021` telling the author why. Same for an unknown rule or verb — a
//! typo must never silently turn a suppression inert.
//!
//! Directives are found by text scan, not the parser, so they work even in
//! files the parser rejects. Known limitations: the scan cannot tell a
//! comment from a string literal containing `-- rime:` — a string crafted to
//! look like a directive is treated as one — and a line inside a `--[[`
//! block comment that does not itself start with `--` counts as code for
//! the allow-file placement check.

use crate::{Code, Diagnostic, Label, RULE_NAMES};

/// One well-formed allow directive.
struct Directive {
    code: Code,
    /// 0-based line the comment sits on.
    line: usize,
    /// Nothing but whitespace before the comment — applies to the next line.
    own_line: bool,
}

/// The result of applying inline allows to one file's diagnostics.
pub struct Applied {
    /// Surviving diagnostics, malformed-directive warnings appended.
    pub kept: Vec<Diagnostic>,
    /// How many diagnostics inline allows removed.
    pub suppressed: usize,
}

/// Apply every `-- rime: allow(...)` / `allow-file(...)` in `src` to `diags`.
pub fn apply(src: &str, diags: Vec<Diagnostic>) -> Applied {
    // Fast path: no marker anywhere, nothing to scan against.
    if !src.contains("rime:") {
        return Applied { kept: diags, suppressed: 0 };
    }
    let (directives, file_codes, malformed) = scan(src);
    if directives.is_empty() && file_codes.is_empty() && malformed.is_empty() {
        return Applied { kept: diags, suppressed: 0 };
    }

    // Line starts, for span -> line.
    let mut line_starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let line_of = |offset: usize| line_starts.partition_point(|&s| s <= offset) - 1;

    // Own-line directive lines, so stacked directives chain to the same
    // target line.
    let directive_lines: std::collections::HashSet<usize> = directives
        .iter()
        .filter(|d| d.own_line)
        .map(|d| d.line)
        .collect();
    let target_of = |d: &Directive| -> usize {
        if !d.own_line {
            return d.line;
        }
        let mut t = d.line + 1;
        while directive_lines.contains(&t) {
            t += 1;
        }
        t
    };

    let mut kept = Vec::new();
    let mut suppressed = 0usize;
    for diag in diags {
        let line = line_of(diag.primary_span().start);
        let anchor = diag.allow_anchor.map(line_of);
        let hit = file_codes.contains(&diag.code)
            || directives.iter().any(|d| {
                d.code == diag.code && {
                    let target = target_of(d);
                    target == line || Some(target) == anchor
                }
            });
        if hit {
            suppressed += 1;
        } else {
            kept.push(diag);
        }
    }
    kept.extend(malformed);
    Applied { kept, suppressed }
}

/// Find every `-- rime:` comment and parse it: well-formed line allows,
/// well-formed file allows (their codes), and `R0021` warnings for the
/// rest. Prose after `rime:` (no `ident(` shape) is not a directive and is
/// ignored.
fn scan(src: &str) -> (Vec<Directive>, Vec<Code>, Vec<Diagnostic>) {
    let mut directives = Vec::new();
    let mut file_codes = Vec::new();
    let mut malformed = Vec::new();
    let mut offset = 0usize;
    // Set once a line with code on it has been seen — the allow-file
    // placement boundary.
    let mut saw_code = false;
    for (lineno, raw) in src.split_inclusive('\n').enumerate() {
        let line = raw.trim_end_matches(['\n', '\r']);
        if let Some(marker) = find_marker(line) {
            let span = offset + marker.comment_start..offset + line.len();
            let own_line = line[..marker.comment_start].trim().is_empty();
            match parse_directive(&line[marker.body_start..]) {
                Ok(Some((code, Verb::Allow))) => {
                    directives.push(Directive { code, line: lineno, own_line })
                }
                Ok(Some((code, Verb::AllowFile))) => {
                    if own_line && !saw_code {
                        file_codes.push(code);
                    } else {
                        malformed.push(
                            Diagnostic::warning(
                                Code(21),
                                "`allow-file` must appear before any code",
                            )
                            .with_label(Label::primary(span, "in this directive"))
                            .with_note(
                                "move it to the top of the file, alone on its line — only \
                                 comments and blank lines may precede a file-level allow",
                            ),
                        );
                    }
                }
                Ok(None) => {} // prose mentioning `rime:` — not a directive
                Err((message, note)) => {
                    malformed.push(
                        Diagnostic::warning(Code(21), message)
                            .with_label(Label::primary(span, "in this directive"))
                            .with_note(note),
                    );
                }
            }
        }
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("--") {
            saw_code = true;
        }
        offset += raw.len();
    }
    (directives, file_codes, malformed)
}

/// Which allow form a directive used.
enum Verb {
    Allow,
    AllowFile,
}

struct Marker {
    /// Byte index of the `--` opening the comment.
    comment_start: usize,
    /// Byte index just past `rime:`.
    body_start: usize,
}

/// `rime:` preceded (spaces allowed) by `--`. First candidate wins.
fn find_marker(line: &str) -> Option<Marker> {
    for (i, _) in line.match_indices("rime:") {
        let before = line[..i].trim_end();
        if before.ends_with("--") {
            return Some(Marker {
                comment_start: before.len() - 2,
                body_start: i + "rime:".len(),
            });
        }
    }
    None
}

type Malformed = (String, String);

/// Parse the text after `rime:`. `Ok(None)` means "not a directive at all"
/// (prose); `Err` is a directive-shaped comment that is wrong.
fn parse_directive(body: &str) -> Result<Option<(Code, Verb)>, Malformed> {
    let body = body.trim_start();
    let verb_len = body
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
        .count();
    let verb_name = &body[..verb_len];
    if verb_name.is_empty() || !body[verb_len..].starts_with('(') {
        return Ok(None);
    }
    let verb = match verb_name {
        "allow" => Verb::Allow,
        "allow-file" => Verb::AllowFile,
        _ => {
            return Err((
                format!("unknown rime directive `{verb_name}`"),
                "only `allow(<rule>, \"reason\")` and `allow-file(<rule>, \"reason\")` \
                 are supported"
                    .to_string(),
            ))
        }
    };
    let after_paren = &body[verb_len + 1..];
    let Some(close) = after_paren.rfind(')') else {
        return Err((
            format!("unclosed `{verb_name}(` directive"),
            format!("write `-- rime: {verb_name}(<rule>, \"reason\")`"),
        ));
    };
    let args = &after_paren[..close];
    let Some((rule, reason)) = args.split_once(',') else {
        return Err((
            format!("{verb_name}(`{}`) has no reason", args.trim()),
            format!(
                "every suppression must say why it is safe: \
                 `-- rime: {verb_name}(<rule>, \"reason\")`"
            ),
        ));
    };
    let code = resolve_rule(rule.trim())?;
    // The reason: a non-empty double-quoted string.
    let (Some(open), Some(end)) = (reason.find('"'), reason.rfind('"')) else {
        return Err((
            format!("{verb_name} reason must be a quoted string"),
            format!("write `-- rime: {verb_name}(<rule>, \"reason\")`"),
        ));
    };
    if end == open || reason[open + 1..end].trim().is_empty() {
        return Err((
            format!("{verb_name} reason is empty"),
            format!(
                "every suppression must say why it is safe: \
                 `-- rime: {verb_name}(<rule>, \"reason\")`"
            ),
        ));
    }
    Ok(Some((code, verb)))
}

fn resolve_rule(rule: &str) -> Result<Code, Malformed> {
    if let Some(digits) = rule.strip_prefix('R') {
        if let Ok(n) = digits.parse::<u16>() {
            return Ok(Code(n));
        }
    }
    if let Some((_, n)) = RULE_NAMES.iter().find(|(name, _)| *name == rule) {
        return Ok(Code(*n));
    }
    if let Some(code) = crate::custom_code(rule) {
        return Ok(code);
    }
    Err((
        format!("allow names unknown rule `{rule}`"),
        "use a rule name rime.toml accepts (e.g. `dynamic-require`) or a raw \
         code like `R0502`"
            .to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Severity;

    fn diag_at(code: u16, start: usize) -> Diagnostic {
        Diagnostic::error(Code(code), "x").with_label(Label::primary(start..start + 1, "here"))
    }

    #[test]
    fn trailing_allow_suppresses_same_line() {
        let src = "local a = 1\nbad line -- rime: allow(freeze-write, \"legacy\")\n";
        let applied = apply(src, vec![diag_at(41, 12)]);
        assert_eq!(applied.suppressed, 1);
        assert!(applied.kept.is_empty());
    }

    #[test]
    fn own_line_allow_suppresses_next_line() {
        let src = "-- rime: allow(R0041, \"legacy\")\nbad line\n";
        let applied = apply(src, vec![diag_at(41, 32)]);
        assert_eq!(applied.suppressed, 1);
        assert!(applied.kept.is_empty());
    }

    #[test]
    fn stacked_own_line_allows_share_a_target() {
        let src = "-- rime: allow(freeze-write, \"a\")\n-- rime: allow(shadowing, \"b\")\nbad\n";
        let applied = apply(src, vec![diag_at(41, 66), diag_at(11, 66)]);
        assert_eq!(applied.suppressed, 2);
    }

    #[test]
    fn wrong_rule_does_not_suppress() {
        let src = "bad -- rime: allow(shadowing, \"nope\")\n";
        let applied = apply(src, vec![diag_at(41, 0)]);
        assert_eq!(applied.suppressed, 0);
        assert_eq!(applied.kept.len(), 1);
    }

    #[test]
    fn missing_reason_is_r0021_and_keeps_the_diagnostic() {
        let src = "bad -- rime: allow(freeze-write)\n";
        let applied = apply(src, vec![diag_at(41, 0)]);
        assert_eq!(applied.suppressed, 0);
        assert_eq!(applied.kept.len(), 2);
        let r21 = applied.kept.iter().find(|d| d.code == Code(21)).unwrap();
        assert_eq!(r21.severity, Severity::Warning);
        assert!(r21.message.contains("no reason"));
    }

    #[test]
    fn empty_reason_is_r0021() {
        let src = "bad -- rime: allow(freeze-write, \"  \")\n";
        let applied = apply(src, vec![diag_at(41, 0)]);
        assert_eq!(applied.suppressed, 0);
        assert!(applied.kept.iter().any(|d| d.code == Code(21)));
    }

    #[test]
    fn unknown_rule_is_r0021() {
        let src = "bad -- rime: allow(dynmic-require, \"typo\")\n";
        let applied = apply(src, vec![diag_at(502, 0)]);
        assert_eq!(applied.suppressed, 0);
        assert!(applied
            .kept
            .iter()
            .any(|d| d.code == Code(21) && d.message.contains("dynmic-require")));
    }

    #[test]
    fn unknown_verb_is_r0021() {
        let src = "-- rime: alow(freeze-write, \"typo\")\n";
        let applied = apply(src, Vec::new());
        assert_eq!(applied.kept.len(), 1);
        assert!(applied.kept[0].message.contains("alow"));
    }

    #[test]
    fn prose_is_ignored() {
        let src = "-- rime: the checker rejects this on purpose\n";
        let applied = apply(src, Vec::new());
        assert!(applied.kept.is_empty());
    }

    #[test]
    fn reason_may_contain_parens_and_commas() {
        let src = "bad -- rime: allow(freeze-write, \"legacy (old, weird) ids\")\n";
        let applied = apply(src, vec![diag_at(41, 0)]);
        assert_eq!(applied.suppressed, 1);
    }

    #[test]
    fn raw_code_form_works() {
        let src = "bad -- rime: allow(R0502, \"vetted\")\n";
        let applied = apply(src, vec![diag_at(502, 0)]);
        assert_eq!(applied.suppressed, 1);
    }

    #[test]
    fn directive_on_wrong_line_does_not_suppress() {
        let src = "-- rime: allow(freeze-write, \"x\")\nok line\nbad line\n";
        let applied = apply(src, vec![diag_at(41, 43)]);
        assert_eq!(applied.suppressed, 0);
    }

    // A report that names where its construct begins (a call, for a report
    // on one of its arguments) is also silenced from that line.
    //
    //   line 0: ok
    //   line 1: -- rime: allow(R0304, "x")
    //   line 2: call(          <- the call starts here, offset 30
    //   line 3:     a,
    //   line 4:     b          <- the primary label, offset 47
    //   line 5: )
    const SPLIT_CALL: &str = "ok\n-- rime: allow(R0304, \"x\")\ncall(\n    a,\n    b\n)\n";

    #[test]
    fn own_line_allow_above_the_anchor_line_suppresses() {
        assert_eq!(&SPLIT_CALL[30..34], "call");
        assert_eq!(&SPLIT_CALL[47..48], "b");
        let applied = apply(SPLIT_CALL, vec![diag_at(304, 47).with_allow_anchor(30)]);
        assert_eq!(applied.suppressed, 1);
        assert!(applied.kept.is_empty());
    }

    #[test]
    fn trailing_allow_on_the_anchor_line_suppresses() {
        let src = "ok\ncall( -- rime: allow(R0304, \"x\")\n    a,\n    b\n)\n";
        let b = src.find("    b").unwrap() + 4;
        let applied = apply(src, vec![diag_at(304, b).with_allow_anchor(3)]);
        assert_eq!(applied.suppressed, 1);
    }

    #[test]
    fn without_an_anchor_only_the_primary_line_counts() {
        let applied = apply(SPLIT_CALL, vec![diag_at(304, 47)]);
        assert_eq!(applied.suppressed, 0);
        assert_eq!(applied.kept.len(), 1);
    }

    #[test]
    fn an_anchor_reaches_its_own_line_and_no_other() {
        // The comment targets line 1; the anchor is on line 2.
        let src = "-- rime: allow(R0304, \"x\")\nok\ncall(\n    a,\n    b\n)\n";
        let call = src.find("call").unwrap();
        let b = src.find("    b").unwrap() + 4;
        let applied = apply(src, vec![diag_at(304, b).with_allow_anchor(call)]);
        assert_eq!(applied.suppressed, 0);
        // A line between the anchor and the primary label is not a target.
        let src = "call(\n    -- rime: allow(R0304, \"x\")\n    a,\n    b\n)\n";
        let b = src.find("    b").unwrap() + 4;
        let applied = apply(src, vec![diag_at(304, b).with_allow_anchor(0)]);
        assert_eq!(applied.suppressed, 0);
    }

    #[test]
    fn an_anchor_does_not_widen_another_rule() {
        let src = "-- rime: allow(shadowing, \"x\")\ncall(\n    a,\n    b\n)\n";
        let call = src.find("call").unwrap();
        let b = src.find("    b").unwrap() + 4;
        let applied = apply(src, vec![diag_at(304, b).with_allow_anchor(call)]);
        assert_eq!(applied.suppressed, 0);
    }

    #[test]
    fn the_primary_line_still_works_with_an_anchor() {
        let src = "call(\n    a,\n    -- rime: allow(R0304, \"x\")\n    b\n)\n";
        let b = src.find("    b").unwrap() + 4;
        let applied = apply(src, vec![diag_at(304, b).with_allow_anchor(0)]);
        assert_eq!(applied.suppressed, 1);
    }

    #[test]
    fn allow_file_suppresses_everywhere_in_the_file() {
        let src = "--!rime\n-- rime: allow-file(freeze-write, \"legacy\")\nbad\nok\nbad again\n";
        let applied = apply(src, vec![diag_at(41, 52), diag_at(41, 59)]);
        assert_eq!(applied.suppressed, 2);
        assert!(applied.kept.is_empty());
    }

    #[test]
    fn allow_file_only_suppresses_its_rule() {
        let src = "-- rime: allow-file(freeze-write, \"legacy\")\nbad\n";
        let applied = apply(src, vec![diag_at(11, 44)]);
        assert_eq!(applied.suppressed, 0);
        assert_eq!(applied.kept.len(), 1);
    }

    #[test]
    fn allow_file_below_code_is_r0021_and_suppresses_nothing() {
        let src = "local x = 1\n-- rime: allow-file(freeze-write, \"late\")\nbad\n";
        let applied = apply(src, vec![diag_at(41, 54)]);
        assert_eq!(applied.suppressed, 0);
        assert_eq!(applied.kept.len(), 2);
        assert!(applied
            .kept
            .iter()
            .any(|d| d.code == Code(21) && d.message.contains("before any code")));
    }

    #[test]
    fn allow_file_trailing_on_code_is_r0021() {
        let src = "local x = 1 -- rime: allow-file(freeze-write, \"trailing\")\nbad\n";
        let applied = apply(src, vec![diag_at(41, 58)]);
        assert_eq!(applied.suppressed, 0);
        assert!(applied.kept.iter().any(|d| d.code == Code(21)));
    }

    #[test]
    fn allow_file_needs_a_reason_too() {
        let src = "-- rime: allow-file(freeze-write)\nbad\n";
        let applied = apply(src, vec![diag_at(41, 34)]);
        assert_eq!(applied.suppressed, 0);
        assert!(applied.kept.iter().any(|d| d.code == Code(21)));
    }

    #[test]
    fn stacked_allow_file_directives_each_apply() {
        let src =
            "-- rime: allow-file(freeze-write, \"a\")\n-- rime: allow-file(shadowing, \"b\")\nbad\n";
        let applied = apply(src, vec![diag_at(41, 76), diag_at(11, 76)]);
        assert_eq!(applied.suppressed, 2);
    }

    #[test]
    fn allow_resolves_registered_custom_names() {
        let code = crate::register_custom("allow-tests-custom");
        let src = "local x = 1 -- rime: allow(allow-tests-custom, \"known\")\n";
        let diags = vec![diag_at(code.0, 0)];
        let out = apply(src, diags);
        assert_eq!(out.suppressed, 1);
        assert!(out.kept.is_empty(), "{:?}", out.kept);
    }
}
