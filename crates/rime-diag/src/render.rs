//! Rendering. Two backends over the same `Diagnostic`:
//!
//!   - `to_terminal`  — ariadne, colour, for humans
//!   - `to_snapshot`  — deterministic plain text, for `insta`
//!
//! The snapshot form is what `tests/corpus` records, so a wording regression
//! fails CI like any other regression (plan §8).

use crate::{Diagnostic, Severity};
use ariadne::{Color, Config, Label as ALabel, Report, ReportKind, Source};

pub fn to_terminal(diag: &Diagnostic, path: &str, src: &str) -> String {
    let kind = match diag.severity {
        Severity::Error => ReportKind::Error,
        Severity::Warning => ReportKind::Warning,
    };

    // Spans are BYTE offsets (full-moon); ariadne's Source indexes by CHAR.
    // Any multibyte character earlier in the file shifts every label right
    // unless converted.
    let to_chars = |span: &std::ops::Range<usize>| -> std::ops::Range<usize> {
        char_index(src, span.start)..char_index(src, span.end)
    };

    // Show the rime.toml key next to the raw code (`[R0502 dynamic-require]`)
    // so configuring a rule never means hunting for its name.
    let code_display = match crate::rule_name(diag.code) {
        Some(name) => format!("{} {name}", diag.code),
        None => diag.code.to_string(),
    };
    let span = to_chars(&diag.primary_span());
    let mut report = Report::build(kind, (path, span))
        .with_code(code_display)
        .with_message(&diag.message)
        .with_config(Config::default().with_compact(false));

    for label in &diag.labels {
        let colour = if label.primary { Color::Red } else { Color::Blue };
        report = report.with_label(
            ALabel::new((path, to_chars(&label.span)))
                .with_message(&label.message)
                .with_color(colour),
        );
    }

    for note in &diag.notes {
        report = report.with_note(note);
    }

    for help in &diag.help {
        report = report.with_help(help);
    }
    for s in &diag.suggestions {
        report = report.with_help(&s.message);
    }

    let mut out = Vec::new();
    report
        .finish()
        .write((path, Source::from(src)), &mut out)
        .expect("render");
    String::from_utf8(out).expect("utf8")
}

/// Deterministic, colourless, line/column resolved. Stable across terminal
/// widths and ariadne versions so snapshots do not churn.
pub fn to_snapshot(diag: &Diagnostic, src: &str) -> String {
    let mut out = String::new();
    let sev = match diag.severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
    };
    let tag = if diag.conservative { " (conservative)" } else { "" };
    out.push_str(&format!("{sev}[{}]{tag}: {}\n", diag.code, diag.message));

    for label in &diag.labels {
        let (line, col) = line_col(src, label.span.start);
        let marker = if label.primary { "-->" } else { "   " };
        out.push_str(&format!(
            "{marker} {line}:{col}: {}\n",
            label.message
        ));
    }
    // Tool-facing, so the text renderer leaves it out; the snapshot shows
    // it because the corpus is where it is checked. The end is exclusive.
    if let Some(extent) = &diag.extent {
        let (line, col) = line_col(src, extent.start);
        let (end_line, end_col) = line_col(src, extent.end);
        out.push_str(&format!("    extent: {line}:{col} to {end_line}:{end_col}\n"));
    }
    // Tool-facing as well. File names only: a snapshot must not hold the
    // path of the machine it was made on.
    for site in &diag.string_matches {
        let file = site.path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default();
        out.push_str(&format!("    string match: {file}:{}\n", site.line));
    }
    for note in &diag.notes {
        out.push_str(&format!("  = note: {note}\n"));
    }
    for help in &diag.help {
        out.push_str(&format!("  = help: {help}\n"));
    }
    for s in &diag.suggestions {
        out.push_str(&format!("  = help: {}\n", s.message));
        if !s.replacement.is_empty() {
            out.push_str(&format!("          {}\n", s.replacement));
        }
    }
    out
}

/// Char index of a byte offset, clamped down to the nearest char boundary.
fn char_index(src: &str, byte: usize) -> usize {
    let mut b = byte.min(src.len());
    while b > 0 && !src.is_char_boundary(b) {
        b -= 1;
    }
    src[..b].chars().count()
}

fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let mut offset = offset.min(src.len());
    // Slicing a mid-character offset panics, and a span can come from a
    // custom rule's own arithmetic — floor to a boundary like `char_index`.
    while !src.is_char_boundary(offset) {
        offset -= 1;
    }
    let before = &src[..offset];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(1, |l| l.chars().count() + 1);
    (line, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_basic() {
        let src = "local a = 1\nlocal b = 2\n";
        assert_eq!(line_col(src, 0), (1, 1));
        assert_eq!(line_col(src, 12), (2, 1));
        assert_eq!(line_col(src, 18), (2, 7));
    }

    #[test]
    fn help_renders_like_a_suggestion_message_and_before_the_edits() {
        let d = crate::Diagnostic::error(crate::Code(501), "x")
            .with_label(crate::Label::primary(0..1, "here"))
            .with_note("why")
            .with_help("fix the path")
            .with_suggestion(crate::Suggestion {
                message: "use the one that exists".into(),
                span: 0..1,
                replacement: "(a.b)".into(),
            });
        let out = to_snapshot(&d, "x\n");
        let tail = "  = note: why\n  = help: fix the path\n  = help: use the one that exists\n          (a.b)\n";
        assert!(out.ends_with(tail), "{out}");
    }

    #[test]
    fn snapshot_lists_string_matches_by_file_name() {
        let d = crate::Diagnostic::warning(crate::Code(309), "x")
            .with_label(crate::Label::primary(0..1, "here"))
            .with_extent(0..1)
            .with_string_matches(vec![
                crate::Site { path: "some/dir/a.luau".into(), line: 3 },
                crate::Site { path: "b.luau".into(), line: 12 },
            ])
            .with_note("why");
        let out = to_snapshot(&d, "x\n");
        let tail = "    extent: 1:1 to 1:2\n    string match: a.luau:3\n    string match: b.luau:12\n  = note: why\n";
        assert!(out.ends_with(tail), "{out}");
    }

    #[test]
    fn snapshot_renders_a_mid_character_span_without_panicking() {
        // `é` occupies bytes 3..5; offset 4 is inside it.
        let src = "-- é\n";
        let d = crate::Diagnostic::error(crate::Code(25), "x")
            .with_label(crate::Label::primary(4..4, "here"));
        let out = to_snapshot(&d, src);
        assert!(out.contains("--> 1:4: here"), "{out}");
    }
}
