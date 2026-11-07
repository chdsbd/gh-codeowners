//! Converting between byte offsets and LSP positions, which count UTF-16
//! code units.

use std::ops::Range;

use gen_lsp_types::Position;
use line_index::{TextSize, WideEncoding, WideLineCol};

pub(crate) struct LineIndex(line_index::LineIndex);

impl LineIndex {
    pub(crate) fn new(text: &str) -> LineIndex {
        LineIndex(line_index::LineIndex::new(text))
    }

    /// The position of `offset`, which must be on a character boundary.
    pub(crate) fn position(&self, offset: usize) -> Position {
        let line_col = self.0.line_col(TextSize::new(offset as u32));
        let wide = self.0.to_wide(WideEncoding::Utf16, line_col).unwrap();
        Position::new(wide.line, wide.col)
    }

    pub(crate) fn range(&self, range: &Range<usize>) -> gen_lsp_types::Range {
        gen_lsp_types::Range::new(self.position(range.start), self.position(range.end))
    }

    /// The byte offset of `position`, or None if it isn't in the text.
    pub(crate) fn offset(&self, position: Position) -> Option<usize> {
        let wide = WideLineCol {
            line: position.line,
            col: position.character,
        };
        let line_col = self.0.to_utf8(WideEncoding::Utf16, wide)?;
        let offset = self.0.offset(line_col)?;
        // Past the end of its line, or inside a character, it maps back to
        // somewhere else, or nowhere.
        (self.0.try_line_col(offset)? == line_col).then(|| offset.into())
    }

    /// Where `line` ends, before its line ending.
    pub(crate) fn line_end(&self, text: &str, line: usize) -> usize {
        let range = self.0.line(line as u32).expect("a line in the text");
        let line = text[range].trim_end_matches(['\n', '\r']);
        usize::from(range.start()) + line.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_columns_in_utf16() {
        // 語 is 3 bytes and 1 UTF-16 unit, 🦀 is 4 bytes and 2 units.
        let text = "* @a\r\n語🦀 x\n";
        let x = text.find('x').unwrap();
        let index = LineIndex::new(text);
        assert_eq!(index.position(x), Position::new(1, 4));
        assert_eq!(index.offset(Position::new(1, 4)), Some(x));
        // Inside 🦀, and past the end of the text.
        assert_eq!(index.offset(Position::new(1, 2)), None);
        assert_eq!(index.offset(Position::new(9, 0)), None);
        assert_eq!(index.line_end(text, 0), 4);
    }
}
