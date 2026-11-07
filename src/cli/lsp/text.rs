//! Converting between byte offsets and LSP positions, which count UTF-16
//! code units, and between file URIs and paths.

use std::ops::Range;
use std::path::{Path, PathBuf};

use line_index::{TextSize, WideEncoding, WideLineCol};
use lsp_types::{Position, Uri};

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

    pub(crate) fn range(&self, range: &Range<usize>) -> lsp_types::Range {
        lsp_types::Range::new(self.position(range.start), self.position(range.end))
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

/// The path of a `file:` URI.
pub(crate) fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    if !uri.scheme()?.as_str().eq_ignore_ascii_case("file") {
        return None;
    }
    let host = uri
        .authority()
        .map_or("", |authority| authority.host().as_str());
    let local = host.is_empty() || host.eq_ignore_ascii_case("localhost");
    let path = uri.path().as_estr().decode().into_string().ok()?;
    if cfg!(windows) {
        // `/C:/x`, which VS Code writes as `/c%3A/x`, is `C:\x`.
        let path = match path.as_bytes() {
            [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => &path[1..],
            _ => &path,
        };
        let path = path.replace('/', "\\");
        Some(PathBuf::from(if local {
            path
        } else {
            format!(r"\\{host}{path}")
        }))
    } else if local {
        Some(PathBuf::from(path.into_owned()))
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(text: &str) -> Uri {
        text.parse().unwrap()
    }

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

    #[cfg(unix)]
    #[test]
    fn converts_file_uris_to_paths_and_back() {
        let path = Path::new("/repo/my docs/日本語/C#");
        let encoded = path_to_uri(path);
        assert_eq!(
            encoded,
            "file:///repo/my%20docs/%E6%97%A5%E6%9C%AC%E8%AA%9E/C%23"
        );
        assert_eq!(uri_to_path(&uri(&encoded)).as_deref(), Some(path));
        assert_eq!(
            uri_to_path(&uri("file://localhost/a")).as_deref(),
            Some(Path::new("/a"))
        );
        assert_eq!(uri_to_path(&uri("untitled:Untitled-1")), None);
    }

    #[cfg(windows)]
    #[test]
    fn converts_windows_file_uris_to_paths_and_back() {
        assert_eq!(
            uri_to_path(&uri("file:///c%3A/repo/a%20b")).as_deref(),
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
            uri_to_path(&uri("file://server/share/x")).as_deref(),
            Some(Path::new(r"\\server\share\x"))
        );
    }
}
