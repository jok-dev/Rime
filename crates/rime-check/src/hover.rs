//! Hover documentation. A thin layer over
//! go-to-definition: resolve the token under the cursor to its declaration
//! (`Session::definition`), then render that declaration's source — its
//! leading `--` comment block and the line it sits on — as Markdown. No new
//! analysis and no type inference: the declaration line already carries the
//! written annotations, which is exactly what Rime's mandatory-annotation
//! model makes worth showing.

use crate::session::Session;
use std::ops::Range;
use std::path::Path;

/// A hover popup: `contents` is Markdown; `range` is the byte range of the
/// identifier under the cursor in the queried file (what the client
/// underlines).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub contents: String,
    pub range: Range<usize>,
}

impl Session<'_> {
    /// The hover for the token at byte `offset` in `path`, or `None` when
    /// the offset is not on an identifier that resolves to a declaration.
    pub fn hover(&self, path: &Path, offset: usize) -> Option<Hover> {
        let src = self.source(path)?;
        let range = word_range(src, offset);
        if range.is_empty() {
            return None;
        }
        let def = self.definition(path, offset)?;
        let target_src = self.source(&def.path)?;
        let contents = if def.span == (0..0) {
            // A `require(...)` jump names a whole file, not a span in it.
            let stem = def.path.file_stem().and_then(|s| s.to_str()).unwrap_or("module");
            format!("**{stem}** — module")
        } else {
            render_declaration(target_src, &def.span)
        };
        Some(Hover { contents, range })
    }
}

/// The identifier byte range around `offset` (`[A-Za-z0-9_]`). Empty when
/// the offset is not on an identifier char (whitespace, punctuation) — the
/// caller reads that as "nothing to hover".
fn word_range(src: &str, offset: usize) -> Range<usize> {
    let b = src.as_bytes();
    let is_word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let off = offset.min(src.len());
    let mut start = off;
    while start > 0 && is_word(b[start - 1]) {
        start -= 1;
    }
    let mut end = off;
    while end < src.len() && is_word(b[end]) {
        end += 1;
    }
    start..end
}

/// Render a declaration at `span` as Markdown: the leading line-comment
/// block (if any) as text, then the declaration line in a `luau` code
/// fence.
fn render_declaration(src: &str, span: &Range<usize>) -> String {
    let start = span.start.min(src.len());
    let line_start = src[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = src[start..].find('\n').map(|i| start + i).unwrap_or(src.len());
    let decl_line = src[line_start..line_end].trim();

    let doc = format_doc(&leading_comments(src, line_start));
    if doc.is_empty() {
        format!("```luau\n{decl_line}\n```")
    } else {
        // Signature first, then the prose/params — reads like an API entry.
        format!("```luau\n{decl_line}\n```\n\n{doc}")
    }
}

/// Turn a raw doc-comment body into Markdown. Moonwave / luau-lsp tags
/// (`@param`, `@return[s]`, `@desc`) become a description paragraph and
/// `**Parameters**` / `**Returns**` lists; a leading line that is just the
/// symbol's dotted name (the Moonwave header) is dropped as redundant with
/// the signature. Untagged prose is returned as a single wrapped paragraph
/// (source line breaks inside a paragraph are soft — Markdown collapses
/// them — which is what wrapped comment text wants).
fn format_doc(raw: &str) -> String {
    let mut desc: Vec<String> = Vec::new();
    let mut params: Vec<(String, Option<String>, String)> = Vec::new();
    let mut returns: Vec<(Option<String>, String)> = Vec::new();
    let mut other: Vec<(String, String)> = Vec::new();

    #[derive(Clone, Copy)]
    enum Cur {
        Desc,
        Param,
        Return,
        Other,
    }
    let mut cur = Cur::Desc;
    let mut seen_tag = false;

    for (i, line) in raw.lines().enumerate() {
        let t = line.trim();
        // Drop a leading Moonwave header line that is just the dotted name.
        if i == 0 && !t.is_empty() && !t.starts_with('@') && is_qualified_name(t) {
            continue;
        }
        if let Some(rest) = t.strip_prefix('@') {
            seen_tag = true;
            let (tag, body) = split_first_word(rest);
            let body = body.trim();
            match tag {
                "param" | "arg" => {
                    let (name, ty, d) = parse_param(body);
                    params.push((name, ty, d));
                    cur = Cur::Param;
                }
                "return" | "returns" => {
                    let (ty, d) = peel_type(body);
                    returns.push((ty, d));
                    cur = Cur::Return;
                }
                "desc" | "description" => {
                    if !body.is_empty() {
                        desc.push(body.to_string());
                    }
                    cur = Cur::Desc;
                }
                _ => {
                    other.push((tag.to_string(), body.to_string()));
                    cur = Cur::Other;
                }
            }
            continue;
        }
        // A continuation line: append to whatever section is open.
        if t.is_empty() {
            continue;
        }
        match cur {
            Cur::Desc => desc.push(t.to_string()),
            Cur::Param => append_last(params.last_mut().map(|p| &mut p.2), t),
            Cur::Return => append_last(returns.last_mut().map(|r| &mut r.1), t),
            Cur::Other => append_last(other.last_mut().map(|o| &mut o.1), t),
        }
        let _ = seen_tag;
    }

    let mut out: Vec<String> = Vec::new();
    if !desc.is_empty() {
        out.push(desc.join(" "));
    }
    if !params.is_empty() {
        let mut s = String::from("**Parameters**\n");
        for (name, ty, d) in &params {
            s.push_str(&format!("\n- {}", render_named(name, ty.as_deref(), d)));
        }
        out.push(s);
    }
    if !returns.is_empty() {
        let mut s = String::from("**Returns**\n");
        for (ty, d) in &returns {
            let label = ty.clone().unwrap_or_default();
            let item = if label.is_empty() {
                d.clone()
            } else if d.is_empty() {
                format!("`{label}`")
            } else {
                format!("`{label}` — {d}")
            };
            s.push_str(&format!("\n- {item}"));
        }
        out.push(s);
    }
    for (tag, body) in &other {
        out.push(format!("**{}** {}", capitalize(tag), body));
    }
    out.join("\n\n").trim().to_string()
}

/// `` `name` `type` — desc `` for a parameter, dropping the empty pieces.
fn render_named(name: &str, ty: Option<&str>, desc: &str) -> String {
    let mut s = format!("`{name}`");
    if let Some(t) = ty.filter(|t| !t.is_empty()) {
        s.push_str(&format!(" `{t}`"));
    }
    if !desc.is_empty() {
        s.push_str(&format!(" — {desc}"));
    }
    s
}

fn append_last(slot: Option<&mut String>, text: &str) {
    if let Some(s) = slot {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(text);
    }
}

/// `@param path string Where...` → (`path`, Some(`string`), `Where...`).
/// The type is peeled only when the second token actually looks like a type
/// (a primitive, `T?`, `{...}`, `(...)`), so `@param n The count` keeps
/// `The count` as the description with no bogus type.
fn parse_param(body: &str) -> (String, Option<String>, String) {
    let (name, after) = split_first_word(body);
    let (ty, desc) = peel_type(after.trim());
    (name.to_string(), ty, desc)
}

/// Peel a leading type token off `s`: a parenthesised group `(...)`, a
/// braced table `{...}`, or a single type-ish word. Returns (type, rest).
fn peel_type(s: &str) -> (Option<String>, String) {
    let s = s.trim();
    if let Some(close) = matching_group(s) {
        // A `?` right after the group makes it an optional type.
        let end = if s.as_bytes().get(close + 1) == Some(&b'?') { close + 2 } else { close + 1 };
        let (ty, rest) = s.split_at(end);
        return (Some(ty.to_string()), rest.trim().to_string());
    }
    let (word, rest) = split_first_word(s);
    if is_type_word(word) {
        (Some(word.to_string()), rest.trim().to_string())
    } else {
        (None, s.to_string())
    }
}

/// If `s` starts with `(` or `{`, the byte index of the matching close
/// (non-nesting scan — Luau type groups here do not nest brackets of the
/// same kind in practice).
fn matching_group(s: &str) -> Option<usize> {
    let (open, close) = match s.as_bytes().first()? {
        b'(' => (b'(', b')'),
        b'{' => (b'{', b'}'),
        _ => return None,
    };
    let mut depth = 0usize;
    for (i, b) in s.bytes().enumerate() {
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Whether `w` reads as a type rather than the start of prose: a known
/// primitive, or any token ending in `?` (an optional type).
fn is_type_word(w: &str) -> bool {
    if w.is_empty() {
        return false;
    }
    if w.ends_with('?') {
        return true;
    }
    matches!(
        w,
        "any" | "number" | "string" | "boolean" | "bool" | "nil" | "table" | "thread"
            | "function" | "userdata" | "unknown" | "never" | "vector" | "buffer"
    )
}

/// A dotted/`:`-qualified identifier path and nothing else (`Foo.Bar`,
/// `M:method`) — the Moonwave name-header shape.
fn is_qualified_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':'))
        && s.chars().any(|c| c.is_ascii_alphanumeric())
}

fn split_first_word(s: &str) -> (&str, &str) {
    match s.trim_start().split_once(char::is_whitespace) {
        Some((a, b)) => (a, b),
        None => (s.trim_start(), ""),
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The documentation immediately above the line starting at `line_start`:
/// a long-bracket block comment (`--[[ ]]`, `--[=[ ]=]`, ...) if one ends
/// there, else the contiguous `--` line-comment block. Empty when neither.
fn leading_comments(src: &str, line_start: usize) -> String {
    if let Some(block) = block_comment_above(src, line_start) {
        return block;
    }
    line_comments_above(src, line_start)
}

/// A long-bracket block comment (`--[[ ... ]]`, `--[=[ ... ]=]`, any level)
/// that ends on the line directly above `line_start`, as its inner text
/// (per-line trimmed, blank edges dropped). `None` if the text directly
/// above is not such a comment, or a blank line separates it from the
/// declaration.
fn block_comment_above(src: &str, line_start: usize) -> Option<String> {
    let above = &src[..line_start];
    let trimmed = above.trim_end_matches([' ', '\t', '\r', '\n']);
    // No blank line between the comment's close and the declaration.
    if above[trimmed.len()..].matches('\n').count() > 1 {
        return None;
    }
    let level = long_bracket_close_level(trimmed)?;
    let close_start = trimmed.len() - (level + 2);
    let opener = format!("--[{}[", "=".repeat(level));
    let open_pos = trimmed[..close_start].rfind(&opener)?;
    let inner = &trimmed[open_pos + opener.len()..close_start];
    let lines: Vec<&str> = inner.lines().map(str::trim).collect();
    // Drop blank lines at the top and bottom (a `--[[\n...\n]]` block opens
    // and closes on their own lines), keep interior blanks.
    let start = lines.iter().position(|l| !l.is_empty()).unwrap_or(lines.len());
    let end = lines.iter().rposition(|l| !l.is_empty()).map_or(0, |i| i + 1);
    Some(lines.get(start..end).unwrap_or(&[]).join("\n"))
}

/// If `s` ends with a long-bracket close `]` `=`×k `]`, its level `k`.
fn long_bracket_close_level(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let n = b.len();
    if n < 2 || b[n - 1] != b']' {
        return None;
    }
    let mut i = n - 2;
    let mut level = 0;
    while i > 0 && b[i] == b'=' {
        level += 1;
        i -= 1;
    }
    (b[i] == b']').then_some(level)
}

/// The contiguous block of `--` line comments immediately above the line
/// starting at `line_start`, as plain text (leading dashes and one space
/// stripped), in source order. A blank line or any code line ends the block.
fn line_comments_above(src: &str, line_start: usize) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut end = line_start;
    while end > 0 {
        // The char before `end` is the '\n' terminating the previous line.
        let prev_start = src[..end - 1].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let line = &src[prev_start..end - 1];
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("--") {
            // A `--[[` block-comment opener is not a line comment.
            if rest.starts_with('[') {
                break;
            }
            lines.push(rest.trim_start_matches('-').trim_start());
            end = prev_start;
        } else {
            break;
        }
    }
    lines.reverse();
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_range_spans_the_identifier() {
        let s = "local foo_bar = 1";
        assert_eq!(&s[word_range(s, 8)], "foo_bar"); // mid-word
        assert_eq!(&s[word_range(s, 6)], "foo_bar"); // at word start
        assert_eq!(&s[word_range(s, 13)], "foo_bar"); // just past the end
        assert_eq!(&s[word_range(s, 5)], "local"); // cursor-at-end attaches left
        // A position with a non-word char on both sides is empty.
        let gap = "a  b";
        assert!(word_range(gap, 2).is_empty());
    }

    #[test]
    fn leading_comments_stop_at_a_blank_line() {
        let src = "-- far\n\n--- near one\n--- near two\nlocal x = 1\n";
        let decl = src.find("local x").unwrap();
        assert_eq!(leading_comments(src, decl), "near one\nnear two");
    }

    #[test]
    fn block_comments_of_any_level() {
        assert_eq!(long_bracket_close_level("x ]]"), Some(0));
        assert_eq!(long_bracket_close_level("x ]=]"), Some(1));
        assert_eq!(long_bracket_close_level("x ]===]"), Some(3));
        assert_eq!(long_bracket_close_level("not a bracket"), None);

        let src = "--[[\n\tLine one.\n\tLine two.\n]]\nlocal x = 1\n";
        let decl = src.find("local x").unwrap();
        assert_eq!(leading_comments(src, decl), "Line one.\nLine two.");

        // A level-1 bracket whose body contains a bare `]]`.
        let src = "--[=[ has ]] inside ]=]\nlocal y = 1\n";
        let decl = src.find("local y").unwrap();
        assert_eq!(leading_comments(src, decl), "has ]] inside");

        // A blank line between the block and the declaration detaches it.
        let src = "--[[ far ]]\n\nlocal z = 1\n";
        let decl = src.find("local z").unwrap();
        assert_eq!(leading_comments(src, decl), "");
    }

    #[test]
    fn format_doc_structures_moonwave_tags() {
        let raw = "Mod.f\n@param path string Where the file is\n@returns (boolean, string?) Whether it loaded, and the error if not\n\n@desc Loads once.\nLater calls reuse it.";
        let md = format_doc(raw);
        assert!(md.starts_with("Loads once. Later calls reuse it."), "desc first: {md}");
        assert!(md.contains("**Parameters**\n\n- `path` `string` — Where the file is"), "{md}");
        assert!(md.contains("**Returns**\n\n- `(boolean, string?)` — Whether it loaded, and the error if not"), "{md}");
        assert!(!md.contains("Mod.f"), "name header dropped: {md}");
    }

    #[test]
    fn format_doc_peels_types_only_when_type_like() {
        // Second word is a primitive → peeled as a type.
        let (n, t, d) = parse_param("id number Which slot");
        assert_eq!((n.as_str(), t.as_deref(), d.as_str()), ("id", Some("number"), "Which slot"));
        // Second word is prose → no type.
        let (n, t, d) = parse_param("count The number of items");
        assert_eq!((n.as_str(), t, d.as_str()), ("count", None, "The number of items"));
        // Optional type.
        let (_, t, _) = parse_param("cb (() -> ())? optional callback");
        assert_eq!(t.as_deref(), Some("(() -> ())?"));
    }

    #[test]
    fn format_doc_leaves_untagged_prose_as_a_paragraph() {
        assert_eq!(format_doc("Just a sentence.\nWrapped across lines."), "Just a sentence. Wrapped across lines.");
        assert_eq!(format_doc(""), "");
    }
}
