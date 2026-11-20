//! rime diagnostics → LSP shapes. The server-side port of the extension's
//! mapping.ts (which stays for spawn mode): primary label = range,
//! secondary labels = related information, notes and help = message tail,
//! code = "R#### rule". Suggestions become quick-fix code actions; help
//! never does.

use super::text::LineIndex;
use tower_lsp_server::ls_types::*;

pub fn to_lsp(d: &rime_diag::Diagnostic, index: &LineIndex, uri: &Uri) -> Diagnostic {
    let primary = d.labels.iter().find(|l| l.primary).or_else(|| d.labels.first());
    let range = match primary {
        Some(l) => index.range(&l.span),
        None => Range::new(Position::new(0, 0), Position::new(0, 1)),
    };
    let mut message = d.message.clone();
    for n in &d.notes {
        message.push_str("\nnote: ");
        message.push_str(n);
    }
    // Advice has no edit to offer, so the message is the only place an
    // editor can show it.
    for h in &d.help {
        message.push_str("\nhelp: ");
        message.push_str(h);
    }
    let code = match rime_diag::rule_name(d.code) {
        Some(rule) => format!("{} {rule}", d.code),
        None => d.code.to_string(),
    };
    let related: Vec<DiagnosticRelatedInformation> = d
        .labels
        .iter()
        .filter(|l| Some(*l) != primary)
        .map(|l| DiagnosticRelatedInformation {
            location: Location { uri: uri.clone(), range: index.range(&l.span) },
            message: l.message.clone(),
        })
        .collect();
    Diagnostic {
        range,
        severity: Some(match d.severity {
            rime_diag::Severity::Error => DiagnosticSeverity::ERROR,
            rime_diag::Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(code)),
        source: Some("rime".to_string()),
        message,
        related_information: (!related.is_empty()).then_some(related),
        ..Default::default()
    }
}

fn overlaps(a: &Range, b: &Range) -> bool {
    !(a.end < b.start || b.end < a.start)
}

/// One quick fix per suggestion of every published diagnostic touching `range`.
pub fn quick_fixes(
    uri: &Uri,
    index: &LineIndex,
    published: &[(rime_diag::Diagnostic, Diagnostic)],
    range: Range,
) -> Vec<CodeActionOrCommand> {
    let mut out = Vec::new();
    for (raw, lsp) in published {
        if !overlaps(&lsp.range, &range) {
            continue;
        }
        for s in &raw.suggestions {
            let edit = TextEdit { range: index.range(&s.span), new_text: s.replacement.clone() };
            let mut changes = std::collections::HashMap::new();
            changes.insert(uri.clone(), vec![edit]);
            out.push(CodeActionOrCommand::CodeAction(CodeAction {
                title: s.message.clone(),
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![lsp.clone()]),
                edit: Some(WorkspaceEdit { changes: Some(changes), ..Default::default() }),
                ..Default::default()
            }));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rime_diag::{Code, Diagnostic, Label, Suggestion};
    use std::str::FromStr;

    fn uri() -> Uri { Uri::from_str("file:///c%3A/x/a.luau").unwrap() }

    #[test]
    fn maps_primary_secondary_notes_code() {
        let src = "local x = 1\nlocal y = 2\n";
        let idx = LineIndex::new(src);
        let d = Diagnostic::warning(Code(13), "unused local `x`")
            .with_label(Label::primary(6..7, "declared here"))
            .with_label(Label::secondary(18..19, "other"))
            .with_note("remove it");
        let out = to_lsp(&d, &idx, &uri());
        assert_eq!(out.range.start.line, 0);
        assert_eq!(out.range.start.character, 6);
        assert_eq!(out.message, "unused local `x`\nnote: remove it");
        assert_eq!(out.code, Some(NumberOrString::String("R0013 unused-local".into())));
        assert_eq!(out.source.as_deref(), Some("rime"));
        let related = out.related_information.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].location.range.start.line, 1);
    }

    #[test]
    fn no_label_anchors_at_top() {
        let idx = LineIndex::new("x\n");
        let d = Diagnostic::error(Code(900), "boom");
        let out = to_lsp(&d, &idx, &uri());
        assert_eq!((out.range.start.line, out.range.start.character, out.range.end.character), (0, 0, 1));
    }

    #[test]
    fn quick_fix_edit_from_suggestion() {
        let src = "return \"it\\'s\"\n";
        let idx = LineIndex::new(src);
        let d = Diagnostic::warning(Code(707), "needless escape")
            .with_label(Label::primary(10..12, "escape"))
            .with_suggestion(Suggestion { message: "remove the backslash".into(), span: 10..11, replacement: "".into() });
        let lsp = to_lsp(&d, &idx, &uri());
        let published = [(d, lsp)];
        let actions = quick_fixes(&uri(), &idx, &published, Range::new(Position::new(0, 11), Position::new(0, 11)));
        assert_eq!(actions.len(), 1);
        let CodeActionOrCommand::CodeAction(a) = &actions[0] else { panic!() };
        assert_eq!(a.title, "remove the backslash");
        let edits = &a.edit.as_ref().unwrap().changes.as_ref().unwrap()[&uri()];
        assert_eq!(edits[0].new_text, "");
        assert_eq!((edits[0].range.start.character, edits[0].range.end.character), (10, 11));
        // Same diagnostic, but the queried range is outside it: nothing.
        let none = quick_fixes(&uri(), &idx, &published, Range::new(Position::new(0, 30), Position::new(0, 30)));
        assert!(none.is_empty());
    }

    #[test]
    fn help_is_message_text_and_not_a_quick_fix() {
        let src = "local x = 1\n";
        let idx = LineIndex::new(src);
        let d = Diagnostic::warning(Code(13), "unused local `x`")
            .with_label(Label::primary(6..7, "declared here"))
            .with_note("never read")
            .with_help("use it, or delete it");
        let lsp = to_lsp(&d, &idx, &uri());
        assert_eq!(lsp.message, "unused local `x`\nnote: never read\nhelp: use it, or delete it");
        let published = [(d, lsp)];
        let actions = quick_fixes(&uri(), &idx, &published, Range::new(Position::new(0, 6), Position::new(0, 7)));
        assert!(actions.is_empty());
    }

    #[test]
    fn quick_fixes_ignores_diagnostics_without_suggestions() {
        let src = "local x = 1\n";
        let idx = LineIndex::new(src);
        let d = Diagnostic::warning(Code(13), "unused local `x`")
            .with_label(Label::primary(6..7, "declared here"));
        let lsp = to_lsp(&d, &idx, &uri());
        let published = [(d, lsp)];
        // The queried range overlaps the diagnostic's label, but it has no
        // suggestions, so there is nothing to turn into a quick fix.
        let actions = quick_fixes(&uri(), &idx, &published, Range::new(Position::new(0, 6), Position::new(0, 7)));
        assert!(actions.is_empty());
    }
}
