//! Converting between byte offsets and LSP positions, and between file URIs
//! and paths.

use std::ops::Range;
use std::path::{Path, PathBuf};

use line_index::{LineCol, TextSize, WideEncoding, WideLineCol};
use lsp_types::Position;

/// How LSP positions count columns, as negotiated with the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    Utf8,
    Utf16,
}

/// Converts byte offsets in a text to LSP positions and back.
pub(crate) struct LineIndex {
    index: line_index::LineIndex,
    encoding: Encoding,
}

impl LineIndex {
    pub(crate) fn new(text: &str, encoding: Encoding) -> LineIndex {
        LineIndex {
            index: line_index::LineIndex::new(text),
            encoding,
        }
    }

    /// The position of `offset`, which must be on a character boundary.
    pub(crate) fn position(&self, offset: usize) -> Position {
        let line_col = self.index.line_col(TextSize::new(offset as u32));
        let (line, col) = match self.encoding {
            Encoding::Utf8 => (line_col.line, line_col.col),
            Encoding::Utf16 => {
                let wide = self
                    .index
                    .to_wide(WideEncoding::Utf16, line_col)
                    .expect("a column on its line");
                (wide.line, wide.col)
            }
        };
        Position::new(line, col)
    }

    pub(crate) fn range(&self, range: &Range<usize>) -> lsp_types::Range {
        lsp_types::Range::new(self.position(range.start), self.position(range.end))
    }

    /// The byte offset of `position`, clamped to the end of its line, and
    /// back to a character boundary. None past the last line.
    pub(crate) fn offset(&self, text: &str, position: Position) -> Option<usize> {
        let line_col = match self.encoding {
            Encoding::Utf8 => LineCol {
                line: position.line,
                col: position.character,
            },
            Encoding::Utf16 => self.index.to_utf8(
                WideEncoding::Utf16,
                WideLineCol {
                    line: position.line,
                    col: position.character,
                },
            )?,
        };
        self.index.line(line_col.line)?;
        let mut offset = u32::from(self.index.offset(line_col)?) as usize;
        offset = offset.min(self.line_end(text, line_col.line as usize));
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        Some(offset)
    }

    /// Where `line` ends, before its line ending.
    pub(crate) fn line_end(&self, text: &str, line: usize) -> usize {
        let range = self.index.line(line as u32).expect("a line in the text");
        let line = &text[range.start().into()..range.end().into()];
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);
        usize::from(range.start()) + line.len()
    }
}

/// The path of a `file:` URI.
pub(crate) fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let (authority, path) = rest.split_at(rest.find('/')?);
    let path = percent_decode(path)?;
    let local = authority.is_empty() || authority.eq_ignore_ascii_case("localhost");
    if cfg!(windows) {
        let path = String::from_utf8(path).ok()?;
        // `/C:/x`, which VS Code writes as `/c%3A/x`, is `C:\x`.
        let path = match path.as_bytes() {
            [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => &path[1..],
            _ => &path,
        };
        let path = path.replace('/', "\\");
        Some(PathBuf::from(if local {
            path
        } else {
            format!(r"\\{authority}{path}")
        }))
    } else if local {
        Some(bytes_to_path(path))
    } else {
        None
    }
}

#[cfg(unix)]
fn bytes_to_path(bytes: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

#[cfg(not(unix))]
fn bytes_to_path(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}

/// The `file:` URI of an absolute path.
pub(crate) fn path_to_uri(path: &Path) -> String {
    let mut path = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        path = path.replace('\\', "/");
        if let Some(unc) = path.strip_prefix("//") {
            // \\server\share\x is file://server/share/x.
            return format!("file://{}", percent_encode(unc));
        }
        path.insert(0, '/');
    }
    format!("file://{}", percent_encode(&path))
}

fn percent_encode(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for &b in path.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/:".contains(&b) {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    encoded
}

fn percent_decode(text: &str) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            decoded.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            decoded.push(b);
        }
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_columns_in_the_negotiated_encoding() {
        // 語 is 3 bytes and 1 UTF-16 unit, 🦀 is 4 bytes and 2 units.
        let text = "* @a\r\n語🦀 x\n";
        let x = text.find('x').unwrap();
        let utf16 = LineIndex::new(text, Encoding::Utf16);
        let utf8 = LineIndex::new(text, Encoding::Utf8);
        assert_eq!(utf16.position(x), Position::new(1, 4));
        assert_eq!(utf8.position(x), Position::new(1, 8));
        assert_eq!(utf16.offset(text, Position::new(1, 4)), Some(x));
        assert_eq!(utf8.offset(text, Position::new(1, 8)), Some(x));
    }

    #[test]
    fn clamps_offsets_to_the_end_of_the_line() {
        let text = "ab\r\ncd";
        let index = LineIndex::new(text, Encoding::Utf16);
        assert_eq!(index.offset(text, Position::new(0, 99)), Some(2));
        assert_eq!(index.offset(text, Position::new(1, 99)), Some(6));
        assert_eq!(index.offset(text, Position::new(2, 0)), None);
        assert_eq!(index.line_end(text, 0), 2);
        // Inside the surrogate pair for 🦀 is before it.
        let text = "a🦀b";
        let index = LineIndex::new(text, Encoding::Utf16);
        assert_eq!(index.offset(text, Position::new(0, 2)), Some(1));
        assert_eq!(index.offset(text, Position::new(0, 3)), Some(5));
    }

    #[cfg(unix)]
    #[test]
    fn converts_file_uris_to_paths_and_back() {
        let path = Path::new("/repo/my docs/日本語/C#");
        let uri = path_to_uri(path);
        assert_eq!(
            uri,
            "file:///repo/my%20docs/%E6%97%A5%E6%9C%AC%E8%AA%9E/C%23"
        );
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
        assert_eq!(
            uri_to_path("file://localhost/a").as_deref(),
            Some(Path::new("/a"))
        );
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
    }

    #[cfg(windows)]
    #[test]
    fn converts_windows_file_uris_to_paths_and_back() {
        assert_eq!(
            uri_to_path("file:///c%3A/repo/a%20b").as_deref(),
            Some(Path::new(r"c:\repo\a b"))
        );
        assert_eq!(
            path_to_uri(Path::new(r"C:\repo\a b")),
            "file:///C:/repo/a%20b"
        );
        assert_eq!(
            path_to_uri(Path::new(r"\\server\share\x")),
            "file://server/share/x"
        );
        assert_eq!(
            uri_to_path("file://server/share/x").as_deref(),
            Some(Path::new(r"\\server\share\x"))
        );
    }
}
