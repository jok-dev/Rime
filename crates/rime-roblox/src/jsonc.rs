//! JSON the way Rojo reads project files and Luau reads `.luaurc`: `//` and
//! `/* */` comments and trailing commas are allowed.
//!
//! Strict JSON is tried first, so a file that is plain JSON costs one
//! parse and reports serde's own error position when it is broken.

/// Parse `text` as JSON, then as JSON with comments and trailing commas.
/// The error is the strict parse's: it points into the text as written.
pub(crate) fn parse(text: &str) -> Result<serde_json::Value, serde_json::Error> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match serde_json::from_str(text) {
        Ok(value) => Ok(value),
        Err(strict) => serde_json::from_str(&strip(text)).map_err(|_| strict),
    }
}

/// `text` with comments blanked out and trailing commas removed. Strings
/// are left exactly as written. Newlines survive, so line numbers do too.
fn strip(text: &str) -> String {
    // Pass 1: comments to spaces.
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        out.push(escaped);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                    }
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
                out.push(' ');
            }
            _ => out.push(c),
        }
    }

    // Pass 2: a comma whose next non-blank character closes a container.
    let bytes: Vec<char> = out.chars().collect();
    let mut kept = String::with_capacity(out.len());
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            kept.push(c);
            if c == '\\' {
                if let Some(escaped) = bytes.get(i + 1) {
                    kept.push(*escaped);
                    i += 1;
                }
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
            kept.push(c);
        } else if c == ',' {
            let next = bytes[i + 1..].iter().find(|c| !c.is_whitespace());
            if !matches!(next, Some('}') | Some(']')) {
                kept.push(c);
            }
        } else {
            kept.push(c);
        }
        i += 1;
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_json_parses() {
        assert_eq!(parse(r#"{"a": [1, 2]}"#).unwrap(), serde_json::json!({"a": [1, 2]}));
    }

    #[test]
    fn comments_and_trailing_commas_are_allowed() {
        let text = "{\n  // a line comment\n  \"a\": [1, 2, ], /* a block\n comment */\n  \"b\": \"x\",\n}\n";
        assert_eq!(parse(text).unwrap(), serde_json::json!({"a": [1, 2], "b": "x"}));
    }

    #[test]
    fn strings_are_left_alone() {
        let text = r#"{ "url": "http://x/y", "c": "a,}", "q": "say \"//hi\"", }"#;
        assert_eq!(
            parse(text).unwrap(),
            serde_json::json!({"url": "http://x/y", "c": "a,}", "q": "say \"//hi\""})
        );
    }

    #[test]
    fn a_byte_order_mark_is_skipped() {
        assert_eq!(parse("\u{feff}{\"a\": 1}").unwrap(), serde_json::json!({"a": 1}));
    }

    #[test]
    fn broken_json_is_still_an_error() {
        assert!(parse("{ \"a\": }").is_err());
        assert!(parse("{ \"a\": 1 ").is_err());
    }
}
