//! `--format json` serialization (schema v1). Wrapper structs rather than
//! serde derives on
//! rime-diag types: the diagnostic crate stays serialization-free and the
//! wire schema can evolve independently of the internal representation.
//!
//! Columns are 1-based UTF-16 code units — VS Code `Position` semantics
//! plus the 1-base — so editor conversion is a plain `-1`. `span` keeps the
//! raw byte offsets for future `--fix`.

use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Output {
    pub version: u32,
    pub diagnostics: Vec<Diagnostic>,
    pub summary: Summary,
    /// What the run says once, in sentences, about checking it did not do:
    /// cross-file checking off, requires it did not follow (`notes.rs`).
    /// The same lines stderr carries after `rime: note: `. Added to v1 in
    /// place, and left out when there is nothing to say, so a run with no
    /// notes serializes exactly as before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub checked: usize,
    pub errors: usize,
    pub warnings: usize,
    pub ignored: usize,
    /// R0901s in this run: the ones in `diagnostics` plus any that belong
    /// to no file (those are on stderr only). Non-zero means the results
    /// are incomplete and the exit status is 101. Added to v1 in place: a
    /// new key, nothing existing moved.
    pub internal_errors: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub path: String,
    pub code: String,
    pub rule: Option<&'static str>,
    pub severity: &'static str,
    pub message: String,
    pub conservative: bool,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    /// Advice in words. Nothing here is an edit.
    pub help: Vec<String>,
    /// Edits: replacing `span` with `replacement` is the whole fix.
    pub suggestions: Vec<Suggestion>,
    /// The whole of the code the diagnostic is about, where the labels
    /// mark only part of it: for `unused-export`, the member's definition
    /// from its first character to its last. Deleting exactly `span`
    /// removes it and leaves the file parseable; `end.line - start.line +
    /// 1` is how many lines it takes. `null` when the diagnostic has no
    /// single such range. Added to v1 in place: a new key, nothing
    /// existing moved.
    pub extent: Option<Extent>,
    /// Every line where the name the diagnostic is about is also written
    /// as a string literal, when that makes the diagnostic less certain:
    /// for `unused-export` in a project with a dynamic require, code Rime
    /// cannot follow may reach the member by that string. The diagnostic
    /// is reported all the same. A tool that acts on diagnostics without
    /// a person reading them should skip one where this is not empty.
    /// Empty for everything else. Added to v1 in place: a new key,
    /// nothing existing moved.
    pub string_matches: Vec<StringMatch>,
}

/// A line of a file of the project, which may be another file than the
/// diagnostic's. `path` is written the way a diagnostic's `path` is.
#[derive(Serialize)]
pub struct StringMatch {
    pub path: String,
    pub line: usize,
}

/// A range of the file: `span` in bytes, `start`/`end` as positions. The
/// end is exclusive, as in a label.
#[derive(Serialize)]
pub struct Extent {
    pub span: ByteSpan,
    pub start: Position,
    pub end: Position,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Label {
    pub primary: bool,
    pub message: String,
    pub span: ByteSpan,
    pub start: Position,
    pub end: Position,
}

#[derive(Serialize)]
pub struct ByteSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Serialize)]
pub struct Position {
    pub line: usize,
    pub column: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub message: String,
    pub span: ByteSpan,
    pub replacement: String,
}

impl Diagnostic {
    /// `show` writes a path the way `path` was written, for the places a
    /// diagnostic names in other files.
    pub fn from_diag(
        path: &str,
        d: &rime_diag::Diagnostic,
        src: &str,
        show: &dyn Fn(&std::path::Path) -> String,
    ) -> Self {
        Self {
            path: path.to_string(),
            code: d.code.to_string(),
            rule: rime_diag::rule_name(d.code),
            severity: match d.severity {
                rime_diag::Severity::Error => "error",
                rime_diag::Severity::Warning => "warning",
            },
            message: d.message.clone(),
            conservative: d.conservative,
            labels: d
                .labels
                .iter()
                .map(|l| Label {
                    primary: l.primary,
                    message: l.message.clone(),
                    span: ByteSpan { start: l.span.start, end: l.span.end },
                    start: position(src, l.span.start),
                    end: position(src, l.span.end),
                })
                .collect(),
            notes: d.notes.clone(),
            help: d.help.clone(),
            suggestions: d
                .suggestions
                .iter()
                .map(|s| Suggestion {
                    message: s.message.clone(),
                    span: ByteSpan { start: s.span.start, end: s.span.end },
                    replacement: s.replacement.clone(),
                })
                .collect(),
            extent: d.extent.as_ref().map(|e| Extent {
                span: ByteSpan { start: e.start, end: e.end },
                start: position(src, e.start),
                end: position(src, e.end),
            }),
            string_matches: d
                .string_matches
                .iter()
                .map(|site| StringMatch { path: show(&site.path), line: site.line })
                .collect(),
        }
    }
}

/// 1-based line, 1-based column in UTF-16 code units. Byte offset clamped
/// down to the nearest char boundary (same defence as the renderer).
fn position(src: &str, offset: usize) -> Position {
    let mut b = offset.min(src.len());
    while b > 0 && !src.is_char_boundary(b) {
        b -= 1;
    }
    let before = &src[..b];
    let line = before.matches('\n').count() + 1;
    let column: usize = before
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .chars()
        .map(char::len_utf16)
        .sum();
    Position { line, column: column + 1 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paths as they are, for diagnostics that name no other file.
    fn show(path: &std::path::Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn string_matches_is_a_list_of_path_and_line_or_empty() {
        let src = "local M = {}\nfunction M.dead() end\nreturn M\n";
        let d = rime_diag::Diagnostic::warning(rime_diag::Code(309), "export `dead` is never used")
            .with_label(rime_diag::Label::primary(24..28, "exported here"))
            .with_string_matches(vec![
                rime_diag::Site { path: "root/src/a.luau".into(), line: 4 },
                rime_diag::Site { path: "root/src/b.luau".into(), line: 9 },
            ]);
        // `show` is what turns a site's path into the form `path` has.
        let relative = |p: &std::path::Path| p.strip_prefix("root").unwrap().to_string_lossy().replace('\\', "/");
        let v = serde_json::to_value(Diagnostic::from_diag("src/M.luau", &d, src, &relative)).unwrap();
        assert_eq!(
            v["stringMatches"],
            serde_json::json!([{ "path": "src/a.luau", "line": 4 }, { "path": "src/b.luau", "line": 9 }])
        );

        let bare = rime_diag::Diagnostic::warning(rime_diag::Code(13), "unused local 'x'");
        let v = serde_json::to_value(Diagnostic::from_diag("a.luau", &bare, src, &show)).unwrap();
        assert_eq!(v["stringMatches"], serde_json::json!([]), "the key is always there");
    }

    #[test]
    fn position_counts_utf16_units() {
        // "🙂" is 4 bytes, 2 UTF-16 units. Line 2 starts at byte 5;
        // 🙂 spans bytes 5..9; `x` is byte 9.
        let src = "-- a\n🙂x = 1\n";
        let p = position(src, 9);
        assert_eq!((p.line, p.column), (2, 3)); // col: 2 units for 🙂 + 1-base
        let after_x = position(src, 10);
        assert_eq!((after_x.line, after_x.column), (2, 4));
        let start = position(src, 5);
        assert_eq!((start.line, start.column), (2, 1));
    }

    #[test]
    fn advice_is_help_and_only_edits_are_suggestions() {
        let src = "return \"it\\'s\"\n";
        let d = rime_diag::Diagnostic::warning(rime_diag::Code(707), "needless escape")
            .with_label(rime_diag::Label::primary(10..12, "escape"))
            .with_help("or use single quotes")
            .with_suggestion(rime_diag::Suggestion {
                message: "remove the backslash".into(),
                span: 10..11,
                replacement: "".into(),
            });
        let j = Diagnostic::from_diag("a.luau", &d, src, &show);
        let v = serde_json::to_value(&j).unwrap();
        assert_eq!(v["help"], serde_json::json!(["or use single quotes"]));
        assert_eq!(v["suggestions"].as_array().unwrap().len(), 1);
        assert_eq!(v["suggestions"][0]["replacement"], "");
    }

    #[test]
    fn extent_is_a_range_or_null() {
        let src = "local M = {}\nfunction M.dead()\n\treturn 1\nend\nreturn M\n";
        let start = src.find("function").unwrap();
        let end = src.find("end").unwrap() + 3;
        let d = rime_diag::Diagnostic::warning(rime_diag::Code(309), "export `dead` is never used")
            .with_label(rime_diag::Label::primary(start + 11..start + 15, "exported here"))
            .with_extent(start..end);
        let v = serde_json::to_value(Diagnostic::from_diag("a.luau", &d, src, &show)).unwrap();
        let e = &v["extent"];
        assert_eq!(e["span"], serde_json::json!({ "start": start, "end": end }));
        assert_eq!(e["start"], serde_json::json!({ "line": 2, "column": 1 }));
        assert_eq!(e["end"], serde_json::json!({ "line": 4, "column": 4 }));

        let bare = rime_diag::Diagnostic::warning(rime_diag::Code(13), "unused local 'x'");
        let v = serde_json::to_value(Diagnostic::from_diag("a.luau", &bare, src, &show)).unwrap();
        assert!(v.as_object().unwrap().contains_key("extent"), "the key is always there");
        assert!(v["extent"].is_null());
    }

    #[test]
    fn serializes_schema_v1() {
        let src = "local x = 1\n";
        let d = rime_diag::Diagnostic::warning(rime_diag::Code(13), "unused local 'x'")
            .with_label(rime_diag::Label::primary(6..7, "declared here"))
            .with_note("remove it or use it");
        let j = Diagnostic::from_diag("src\\a.luau", &d, src, &show);
        let out = Output {
            version: 1,
            diagnostics: vec![j],
            summary: Summary { checked: 1, errors: 0, warnings: 1, ignored: 0, internal_errors: 0 },
            notes: Vec::new(),
        };
        let text = serde_json::to_string_pretty(&out).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], 1);
        let d = &v["diagnostics"][0];
        assert_eq!(d["path"], "src\\a.luau");
        assert_eq!(d["code"], "R0013");
        assert_eq!(d["rule"], "unused-local");
        assert_eq!(d["severity"], "warning");
        assert_eq!(d["conservative"], false);
        let l = &d["labels"][0];
        assert_eq!(l["primary"], true);
        assert_eq!(l["span"]["start"], 6);
        assert_eq!(l["start"]["line"], 1);
        assert_eq!(l["start"]["column"], 7);
        assert_eq!(l["end"]["column"], 8);
        assert_eq!(d["notes"][0], "remove it or use it");
        assert_eq!(v["summary"]["warnings"], 1);
        assert_eq!(v["summary"]["internalErrors"], 0);
        assert!(v.get("notes").is_none(), "no notes, no key");
    }
}
