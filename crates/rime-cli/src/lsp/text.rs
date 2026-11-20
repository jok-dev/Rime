//! Byte offsets → LSP positions (0-based line, UTF-16 column). Same
//! semantics as json.rs's `position`, minus one, but O(log n) per lookup
//! via a line-start table — the daemon converts every diagnostic of a file
//! on each publish.

use tower_lsp_server::ls_types::{Position, Range};

pub struct LineIndex {
    line_starts: Vec<usize>,
    src: String,
}

impl LineIndex {
    pub fn new(src: &str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        Self { line_starts, src: src.to_string() }
    }

    pub fn position(&self, offset: usize) -> Position {
        let mut b = offset.min(self.src.len());
        while b > 0 && !self.src.is_char_boundary(b) {
            b -= 1;
        }
        let line = match self.line_starts.binary_search(&b) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        let col: usize = self.src[self.line_starts[line]..b].chars().map(char::len_utf16).sum();
        Position::new(line as u32, col as u32)
    }

    pub fn range(&self, span: &std::ops::Range<usize>) -> Range {
        Range::new(self.position(span.start), self.position(span.end))
    }

    /// The inverse of `position`: an LSP position (0-based line, UTF-16
    /// column) to a byte offset. A line/column past the end clamps to the
    /// line's end, then to EOF — a Ctrl+click never lands out of bounds.
    pub fn offset(&self, pos: Position) -> usize {
        let line = pos.line as usize;
        let start = self.line_starts.get(line).copied().unwrap_or(self.src.len());
        let mut col = pos.character as usize;
        let mut b = start;
        while col > 0 {
            let Some(ch) = self.src[b..].chars().next() else { break };
            if ch == '\n' {
                break;
            }
            let u = ch.len_utf16();
            // A column that splits a surrogate pair lands on the char start.
            if u > col {
                break;
            }
            col -= u;
            b += ch.len_utf8();
        }
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf16_columns_and_lines() {
        // "🙂" = 4 bytes, 2 UTF-16 units. Line 2 starts at byte 5.
        let idx = LineIndex::new("-- a\n🙂x = 1\n");
        let p = idx.position(9); // the `x`
        assert_eq!((p.line, p.character), (1, 2));
        let p = idx.position(5);
        assert_eq!((p.line, p.character), (1, 0));
        let end = idx.position(usize::MAX); // clamps to EOF
        assert_eq!(end.line, 2);
    }

    #[test]
    fn offset_is_position_inverse() {
        let idx = LineIndex::new("-- a\n🙂x = 1\n");
        // `x` is line 1, UTF-16 column 2 (past the 2-unit emoji).
        assert_eq!(idx.offset(Position::new(1, 2)), 9);
        assert_eq!(idx.offset(Position::new(1, 0)), 5);
        // A column past the line end clamps to the newline, not into line 2.
        assert_eq!(idx.offset(Position::new(0, 99)), 4);
        // A line past EOF clamps to the source length.
        assert_eq!(idx.offset(Position::new(50, 0)), "-- a\n🙂x = 1\n".len());
    }
}
