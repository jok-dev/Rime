//! Applying a diagnostic's suggested edit to source text.
//!
//! A `Suggestion` is a span and the text that replaces it. Editors and
//! agents apply it verbatim, so this is the one definition of what an edit
//! does — the tests that hold every rule's edits to "the result still
//! parses and the diagnostic is gone" go through it.

use rime_diag::Suggestion;

/// `src` with the suggestion's span replaced by its replacement text. None
/// when the span does not lie inside `src` on character boundaries.
pub fn apply(src: &str, suggestion: &Suggestion) -> Option<String> {
    let (start, end) = (suggestion.span.start, suggestion.span.end);
    if start > end || end > src.len() || !src.is_char_boundary(start) || !src.is_char_boundary(end) {
        return None;
    }
    let mut out = String::with_capacity(src.len() + suggestion.replacement.len());
    out.push_str(&src[..start]);
    out.push_str(&suggestion.replacement);
    out.push_str(&src[end..]);
    Some(out)
}

/// Whether `src` still parses with the suggestion applied.
///
/// The HIR drops parentheses, so a span taken from an expression can sit
/// inside a pair the rule cannot see (`(1)` has the span of `1`). A rule
/// that inserts beside such a span asks this before offering the edit, and
/// gives the advice as help when the answer is no. It costs a parse of the
/// file, so it is for the rare diagnostic, not for every node visited.
pub fn keeps_syntax(src: &str, suggestion: &Suggestion) -> bool {
    apply(src, suggestion).is_some_and(|out| rime_syntax::parse(&out).is_ok())
}

#[cfg(test)]
pub(crate) mod testing {
    /// Applies the edit of the single diagnostic `code` in `src` and holds
    /// it to what every edit owes: the result parses, and the diagnostic it
    /// came from is gone. Returns the edited source, or None when the
    /// diagnostic offers no edit.
    pub(crate) fn fixed(src: &str, code: u16) -> Option<String> {
        let diags: Vec<_> = crate::check_source(src)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(code))
            .collect();
        assert_eq!(diags.len(), 1, "expected one R{code:04} in:\n{src}");
        let edit = diags[0].suggestions.first()?;
        let out = super::apply(src, edit).expect("edit span inside the source");
        assert!(rime_syntax::parse(&out).is_ok(), "the edit broke the syntax:\n{out}");
        let left = crate::check_source(&out)
            .into_iter()
            .filter(|d| d.code == rime_diag::Code(code))
            .count();
        assert_eq!(left, 0, "R{code:04} is still reported after its own edit:\n{out}");
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_exactly_the_span() {
        let s = Suggestion { message: String::new(), span: 4..7, replacement: "false".into() };
        assert_eq!(apply("x = nil\n", &s).as_deref(), Some("x = false\n"));
    }

    #[test]
    fn rejects_a_span_outside_the_source() {
        let s = Suggestion { message: String::new(), span: 4..70, replacement: "false".into() };
        assert_eq!(apply("x = nil\n", &s), None);
    }
}
