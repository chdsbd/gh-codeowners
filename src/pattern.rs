//! Turns a CODEOWNERS pattern into a regex, the way GitHub matches them:
//! gitignore rules, except that paths are case-sensitive, there are no
//! character classes, and a leading `!` is ignored rather than negating.
//! Unlike gitignore, a pattern ending in `/*` doesn't match nested files.
//! Where GitHub's behavior differs from its docs, this follows what GitHub
//! does, as checked against a test repository.

use std::sync::OnceLock;

/// GitHub doesn't support negation, and matches `!foo` like `foo`.
pub fn strip_negation(pattern: &str) -> &str {
    pattern.strip_prefix('!').unwrap_or(pattern)
}

/// Whether `pattern` can match a path. Blank lines, comments and patterns
/// GitHub rejects as invalid can't, nor can a few that GitHub accepts but
/// never matches.
pub fn can_match(pattern: &str) -> bool {
    let pattern = strip_negation(pattern);
    !(pattern.trim().is_empty()
        || pattern.starts_with('#')
        || pattern.trim_matches('/').is_empty())
        && is_valid(pattern)
        && brackets_pair_up(pattern)
        // On GitHub, a pattern starting with `\[` matches nothing unless it
        // has a slash.
        && !(pattern.starts_with("\\[") && !pattern.contains('/'))
}

/// Whether GitHub accepts `pattern`, which its CODEOWNERS errors API checks:
/// `[` and `]` must be escaped, `\` can only escape a space, tab, `#`, `[`,
/// `]` or `\`, and there can't be more than two `*`s in a row.
fn is_valid(pattern: &str) -> bool {
    let mut stars = 0;
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        stars = if c == '*' { stars + 1 } else { 0 };
        match c {
            '\\' if !matches!(chars.next(), Some(' ' | '\t' | '#' | '[' | ']' | '\\')) => {
                return false;
            }
            '[' | ']' => return false,
            _ if stars > 2 => return false,
            _ => {}
        }
    }
    true
}

/// Whether each `\[` is closed by a `\]` before the next `\[`. GitHub
/// matches nothing otherwise.
fn brackets_pair_up(pattern: &str) -> bool {
    let mut open = false;
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('[') if !open => open = true,
                Some(']') if open => open = false,
                Some('[' | ']') => return false,
                _ => {}
            }
        }
    }
    !open
}

/// Whether `pattern` is relative to the root: it has a leading or middle
/// slash. Otherwise it matches at any depth.
fn is_anchored(pattern: &str) -> bool {
    pattern.strip_suffix('/').unwrap_or(pattern).contains('/')
}

/// Append the regex for one path segment, without slashes.
fn push_segment(out: &mut String, segment: &str) {
    // A segment of only `*`s names something, so it can't be empty.
    if segment.bytes().all(|c| c == b'*') {
        out.push_str("[^/]+");
        return;
    }
    let mut chars = segment.chars();
    while let Some(c) = chars.next() {
        match c {
            '*' => out.push_str("[^/]*"),
            // One byte, even of a multibyte character, like on GitHub.
            '?' => out.push_str("[^/]"),
            '\\' => {
                if let Some(escaped) = chars.next() {
                    push_literal(out, escaped);
                }
            }
            _ => push_literal(out, c),
        }
    }
}

fn push_literal(out: &mut String, c: char) {
    if c.is_ascii() {
        out.push_str(&regex::escape(c.encode_utf8(&mut [0; 4])));
    } else {
        // Regexes match bytes, so match the character's UTF-8 encoding.
        for byte in c.encode_utf8(&mut [0; 4]).bytes() {
            out.push_str(&format!("\\x{byte:02x}"));
        }
    }
}

/// The regex source for `pattern`, or None if it can't match. It matches
/// a file path, or a directory path with a trailing slash.
pub fn regex_source(pattern: &str) -> Option<String> {
    if !can_match(pattern) {
        return None;
    }
    let pattern = strip_negation(pattern);
    let dir_only = pattern.ends_with('/');
    let trimmed = pattern.strip_suffix('/').unwrap_or(pattern);
    let trimmed = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let mut out = String::from(if is_anchored(pattern) {
        "(?s-u)^"
    } else {
        "(?s-u)^(?:.*/)?"
    });
    let segments: Vec<&str> = trimmed.split('/').collect();
    for (i, segment) in segments.iter().enumerate() {
        let last = i == segments.len() - 1;
        if *segment == "**" {
            // A trailing `/**` matches everything inside, and a leading or
            // middle `**/` zero or more directories.
            if last {
                out.push_str(if dir_only { ".+/$" } else { ".+" });
                return Some(out);
            }
            out.push_str("(?:.*/)?");
            continue;
        }
        push_segment(&mut out, segment);
        if !last {
            out.push('/');
        }
    }
    if dir_only {
        out.push_str("/$");
    } else if pattern.ends_with("/*") {
        // GitHub doesn't match nested files for rules ending in `/*`.
        out.push('$');
    } else {
        // The path, or a directory containing it.
        out.push_str("/?$");
    }
    Some(out)
}

/// Compile `pattern` for the `regex` crate.
pub fn compile(pattern: &str) -> Option<regex::bytes::Regex> {
    regex::bytes::Regex::new(&regex_source(pattern)?).ok()
}

/// Tests paths against a pattern. Most CODEOWNERS
/// patterns are plain paths, and comparing strings is much faster than
/// compiling a regex for each (tens of microseconds, or tens of milliseconds
/// for thousands of rules), so regexes are only built for wildcards, and
/// only once a path needs testing.
pub enum Matcher {
    Never,
    // `*`, which matches every path.
    Always,
    // A path relative to the root, from a pattern with a leading or middle
    // slash and no wildcards.
    Exact {
        path: Vec<u8>,
        dir_only: bool,
    },
    // A name at any depth, from a pattern with no slashes except a trailing
    // one and no wildcards.
    Name {
        name: Vec<u8>,
        dir_only: bool,
    },
    Regex {
        pattern: String,
        regex: OnceLock<Option<regex::bytes::Regex>>,
    },
}

impl Matcher {
    pub fn new(pattern: &str) -> Matcher {
        if !can_match(pattern) {
            return Matcher::Never;
        }
        let pattern = strip_negation(pattern);
        if pattern == "*" {
            return Matcher::Always;
        }
        if pattern.contains(['*', '?', '\\']) {
            return Matcher::Regex {
                pattern: pattern.to_string(),
                regex: OnceLock::new(),
            };
        }
        let dir_only = pattern.ends_with('/');
        let trimmed = pattern.strip_suffix('/').unwrap_or(pattern);
        if is_anchored(pattern) {
            let path = trimmed.strip_prefix('/').unwrap_or(trimmed);
            Matcher::Exact {
                path: path.as_bytes().to_vec(),
                dir_only,
            }
        } else {
            Matcher::Name {
                name: trimmed.as_bytes().to_vec(),
                dir_only,
            }
        }
    }

    /// Whether the pattern matches `path`: a file path, or a directory path
    /// with a trailing slash.
    pub fn is_match(&self, path: &[u8]) -> bool {
        let (trimmed, is_dir) = match path.strip_suffix(b"/") {
            Some(trimmed) => (trimmed, true),
            None => (path, false),
        };
        match self {
            Matcher::Never => false,
            Matcher::Always => !path.is_empty(),
            Matcher::Exact { path, dir_only } => {
                (is_dir || !dir_only) && trimmed == path.as_slice()
            }
            Matcher::Name { name, dir_only } => {
                (is_dir || !dir_only)
                    && trimmed.len() >= name.len()
                    && trimmed.ends_with(name)
                    && (trimmed.len() == name.len()
                        || trimmed[trimmed.len() - name.len() - 1] == b'/')
            }
            Matcher::Regex { pattern, regex } => regex
                .get_or_init(|| compile(pattern))
                .as_ref()
                .is_some_and(|regex| regex.is_match(path)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, path: &str) -> bool {
        compile(pattern).unwrap().is_match(path.as_bytes())
    }

    #[test]
    fn follows_githubs_examples() {
        // From GitHub's CODEOWNERS documentation.
        for (pattern, path, expected) in [
            ("*.js", "src/app.js", true),
            ("/build/logs/", "build/logs/", true),
            ("/build/logs/", "src/build/logs/", false),
            ("docs/*", "docs/getting-started.md", true),
            ("docs/*", "docs/build-app/troubleshooting.md", false),
            ("docs/*", "docs/build-app/", false),
            ("docs/*", "docs/", false),
            ("apps/", "apps/", true),
            ("apps/", "src/apps/", true),
            ("apps/", "apps", false),
            ("**/logs", "deeply/nested/logs/", true),
            ("**/logs", "logs/", true),
            ("/apps/github", "apps/github/", true),
            ("/apps/github", "apps/github", true),
        ] {
            assert_eq!(matches(pattern, path), expected, "{pattern} {path}");
        }
    }

    #[test]
    fn matches_paths_case_sensitively() {
        assert!(!matches("*.MD", "README.md"));
        assert!(!matches("/Docs/", "docs/"));
        assert!(!matches("docs", "Docs"));
    }

    #[test]
    fn matches_escaped_brackets_literally() {
        assert!(matches("x\\[a-c\\]", "d/x[a-c]"));
        assert!(!matches("x\\[a-c\\]", "xb"));
        assert!(matches("/app/\\[id\\]/", "app/[id]/"));
    }

    #[test]
    fn rejects_patterns_github_rejects() {
        // From GitHub's CODEOWNERS errors API.
        for (pattern, valid) in [
            ("[a-c]x", false),
            ("/app/[id]/", false),
            ("x]", false),
            ("/\\[x\\]", true),
            ("\\*star", false),
            ("\\?a", false),
            ("a\\b", false),
            ("a\\\\b", true),
            ("\\#foo", true),
            ("foo\\ bar", true),
            ("tab\\\tx", true),
            ("***", false),
            ("a/***/b", false),
            ("a**b", true),
            ("**a", true),
            ("{a,b}", true),
        ] {
            assert_eq!(compile(pattern).is_some(), valid, "{pattern}");
        }
    }

    #[test]
    fn matches_wildcards_like_gitignore() {
        for (pattern, path, expected) in [
            ("a?c", "x/abc", true),
            ("a?c", "a/c", false),
            ("a/**/b", "a/b", true),
            ("a/**/b", "a/x/y/b", true),
            ("/lib/**", "lib/x/y", true),
            ("/lib/**", "lib", false),
            ("*.md", "docs/a.md", true),
            ("/*", "README.md", true),
            ("/*", "docs/a.md", false),
            ("\\#a", "#a", true),
            ("a\\ b", "a b", true),
            ("a\\\\b", "a\\b", true),
            ("é.txt", "a/é.txt", true),
            ("?.txt", "é.txt", false),
            ("??.txt", "é.txt", true),
        ] {
            assert_eq!(matches(pattern, path), expected, "{pattern} {path}");
        }
    }

    #[test]
    fn ignores_negation() {
        assert!(matches("!foo", "x/foo"));
        assert!(!matches("!foo", "!foo"));
    }

    #[test]
    fn matches_nothing_where_github_matches_nothing() {
        // From a test repository on GitHub.
        for pattern in ["\\[ab\\]x", "\\[ab\\]*.js", "a\\[b"] {
            assert!(compile(pattern).is_none(), "{pattern}");
        }
        for pattern in ["\\[ab\\]x/", "/\\[ab\\]x", "**/\\[ab\\]x", "x\\[ab\\]"] {
            assert!(compile(pattern).is_some(), "{pattern}");
        }
    }

    #[test]
    fn plain_paths_match_like_their_regexes() {
        let paths: [&[u8]; 13] = [
            b"docs",
            b"docs/",
            b"Docs/",
            b"Docs",
            b"a/docs",
            b"a/docs/",
            b"docs/a",
            b"docs/a/",
            b"xdocs",
            b"a/xdocs/",
            b"a/b",
            b"a/b/",
            b"A/B/c",
        ];
        for pattern in [
            "docs", "docs/", "/docs", "/docs/", "a/b", "/a/b/", "a/docs", "*", "{a,b}",
        ] {
            let matcher = Matcher::new(pattern);
            assert!(!matches!(matcher, Matcher::Regex { .. }), "{pattern}");
            let regex = compile(pattern).unwrap();
            for path in paths {
                assert_eq!(
                    matcher.is_match(path),
                    regex.is_match(path),
                    "{pattern} {}",
                    String::from_utf8_lossy(path)
                );
            }
        }
    }
}
