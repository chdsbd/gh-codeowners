//! Turns a CODEOWNERS pattern into a regex, the same way as
//! @snyk/github-codeowners: node-ignore 5.2.4's conversion, plus its
//! fix for patterns ending in `/*`. This is a line-by-line port of
//! node-ignore's replacers, so its quirks carry over too.

use std::sync::{LazyLock, OnceLock};

use fancy_regex::Regex;

type Captures<'t> = fancy_regex::Captures<'t, str>;

struct Replacer {
    regex: Regex,
    // Replace every match, like a JavaScript regex with the `g` flag.
    global: bool,
    // Called with the match, the string being replaced and the pattern.
    replace: fn(&Captures, &str, &str) -> String,
}

fn group<'a>(caps: &Captures<'a>, i: usize) -> &'a str {
    caps.get(i).map_or("", |m| m.as_str())
}

fn replacer(regex: &str, global: bool, replace: fn(&Captures, &str, &str) -> String) -> Replacer {
    Replacer {
        regex: Regex::new(regex).unwrap(),
        global,
        replace,
    }
}

static HAS_LEADING_OR_MIDDLE_SLASH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\/(?!$)").unwrap());
static REGEX_RANGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"([0-z])-([0-z])").unwrap());

// Drop out-of-order ranges like `[z-a]`, which are fine in gitignore but
// invalid regex.
fn sanitize_range(range: &str) -> String {
    REGEX_RANGE
        .replace_all(range, |caps: &Captures| {
            if group(caps, 1) <= group(caps, 2) {
                group(caps, 0).to_string()
            } else {
                String::new()
            }
        })
        .into_owned()
}

static REPLACERS: LazyLock<Vec<Replacer>> = LazyLock::new(|| {
    vec![
        // Trailing spaces are ignored unless escaped.
        replacer(r"\\?\s+$", false, |caps, _, _| {
            if group(caps, 0).starts_with('\\') {
                " "
            } else {
                ""
            }
            .to_string()
        }),
        replacer(r"\\\s", true, |_, _, _| " ".to_string()),
        // Escape regex metacharacters.
        replacer(r"[\\$.|*+(){^]", true, |caps, _, _| {
            format!("\\{}", group(caps, 0))
        }),
        replacer(r"(?!\\)\?", true, |_, _, _| "[^/]".to_string()),
        // A leading slash matches the beginning of the path.
        replacer(r"^\/", false, |_, _, _| "^".to_string()),
        replacer(r"\/", true, |_, _, _| "\\/".to_string()),
        // A leading `**/` matches in all directories.
        replacer(r"^\^*\\\*\\\*\\\/", false, |_, _, _| {
            "^(?:.*\\/)?".to_string()
        }),
        // Patterns without a leading or middle slash match at any depth.
        replacer(r"^(?=[^^])", false, |_, _, pattern| {
            if HAS_LEADING_OR_MIDDLE_SLASH.is_match(pattern).unwrap() {
                "^"
            } else {
                "(?:^|\\/)"
            }
            .to_string()
        }),
        // `/**/` matches zero or more directories, and a trailing `/**`
        // everything inside.
        replacer(r"\\\/\\\*\\\*(?=\\\/|$)", true, |caps, input, _| {
            if caps.get(0).unwrap().end() < input.len() {
                "(?:\\/[^\\/]+)*"
            } else {
                "\\/.+"
            }
            .to_string()
        }),
        // Intermediate wildcards. Only the last of consecutive `*`s counts.
        replacer(r"(^|[^\\]+)(\\\*)+(?=.+)", true, |caps, _, _| {
            format!(
                "{}{}",
                group(caps, 1),
                group(caps, 2).replace("\\*", "[^\\/]*")
            )
        }),
        // Unescape, reverting the metacharacter escaping except for `\`.
        replacer(r"\\\\\\(?=[$.|*+(){^])", true, |_, _, _| "\\".to_string()),
        replacer(r"\\\\", true, |_, _, _| "\\".to_string()),
        // Ranges like `[a-z]`.
        replacer(r"(\\)?\[([^\]/]*?)(\\*)($|\])", true, |caps, _, _| {
            let (lead, range, end, close) = (
                group(caps, 1),
                group(caps, 2),
                group(caps, 3),
                group(caps, 4),
            );
            if lead == "\\" {
                let end = &end[..end.len() - end.len() % 2];
                format!("\\[{range}{end}{close}")
            } else if close == "]" && end.len() % 2 == 0 {
                format!("[{}{end}]", sanitize_range(range))
            } else {
                "[]".to_string()
            }
        }),
        // Patterns match the path or a directory containing it, except
        // those ending in `/`, which only match directories.
        replacer(r"(?:[^*])$", false, |caps, _, _| {
            let m = group(caps, 0);
            if m.ends_with('/') {
                format!("{m}$")
            } else {
                format!("{m}(?=$|\\/$)")
            }
        }),
        // Trailing wildcard.
        replacer(r"(\^|\\\/)?\\\*$", false, |caps, _, _| {
            let prefix = match caps.get(1) {
                Some(p) => format!("{}[^/]+", p.as_str()),
                None => "[^/]*".to_string(),
            };
            format!("{prefix}(?=$|\\/$)")
        }),
    ]
});

/// Whether `pattern` can match a path. Blank lines, comments, invalid
/// patterns and negations can't.
pub fn can_match(pattern: &str) -> bool {
    !(pattern.trim().is_empty()
        || pattern.starts_with('#')
        || invalid_trailing_backslash(pattern)
        // Negations can't own files.
        || pattern.starts_with('!'))
}

/// node-ignore's regex source for `pattern`, or None if it can't match.
pub fn javascript_regex(pattern: &str) -> Option<String> {
    if !can_match(pattern) {
        return None;
    }
    let pattern = if let Some(rest) = pattern.strip_prefix("\\!") {
        format!("!{rest}")
    } else if let Some(rest) = pattern.strip_prefix("\\#") {
        format!("#{rest}")
    } else {
        pattern.to_string()
    };
    let mut source = pattern.clone();
    for r in REPLACERS.iter() {
        let replace = |caps: &Captures| (r.replace)(caps, &source, &pattern);
        source = if r.global {
            r.regex.replace_all(&source, replace).into_owned()
        } else {
            r.regex.replace(&source, replace).into_owned()
        };
    }
    // GitHub doesn't match nested files for rules ending in `/*`.
    if pattern.ends_with("/*") {
        source = source.replacen("(?=$|\\/$)", "(?=$|[^\\/]$)", 1);
    }
    Some(source)
}

fn invalid_trailing_backslash(pattern: &str) -> bool {
    // `(?:[^\\]|^)\\$`: ends with a backslash that isn't itself escaped.
    let bytes = pattern.as_bytes();
    bytes.ends_with(b"\\") && (bytes.len() == 1 || bytes[bytes.len() - 2] != b'\\')
}

/// Translate node-ignore's JavaScript regex to the `regex` crate, which has
/// no lookaround. The only lookahead is at the very end, where it's
/// equivalent to a group for finding a match.
fn to_rust(source: &str) -> Option<String> {
    let mut out = String::with_capacity(source.len() + 8);
    out.push_str("(?i)");
    let mut chars = source.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                out.push(chars.next()?);
            }
            '[' if in_class => out.push_str("\\["),
            // Set operators in the regex crate, but literals in JavaScript.
            '&' | '~' if in_class => {
                out.push('\\');
                out.push(c);
            }
            '[' => {
                // JavaScript's `[]` matches nothing.
                if chars.peek() == Some(&']') {
                    chars.next();
                    out.push_str(r"[^\x00-\x{10FFFF}]");
                    continue;
                }
                in_class = true;
                out.push('[');
                // A leading `]` or `^]` is literal in the regex crate.
                if chars.peek() == Some(&'^') {
                    out.push(chars.next().unwrap());
                }
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            '(' if !in_class && chars.peek() == Some(&'?') => {
                let rest: String = chars.clone().take(2).collect();
                if rest == "?=" || rest == "?!" {
                    if rest == "?!" {
                        return None;
                    }
                    // `(?=` becomes `(?:`, and must close the regex.
                    chars.next();
                    chars.next();
                    out.push_str("(?:");
                } else {
                    out.push('(');
                }
            }
            _ => out.push(c),
        }
    }
    Some(out)
}

/// `pattern`'s regex in the `regex` crate's syntax, matching
/// case-insensitively like @snyk/github-codeowners.
pub fn rust_regex(pattern: &str) -> Option<String> {
    to_rust(&javascript_regex(pattern)?)
}

/// Compile `pattern` for the `regex` crate.
pub fn compile(pattern: &str) -> Option<regex::bytes::Regex> {
    regex::bytes::Regex::new(&rust_regex(pattern)?).ok()
}

/// Tests paths against a pattern, case-insensitively. Most CODEOWNERS
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

/// Lowercase `bytes` like JavaScript's toLowerCase, for comparing paths.
pub fn lowercase(bytes: &[u8]) -> Vec<u8> {
    if bytes.is_ascii() {
        bytes.to_ascii_lowercase()
    } else {
        String::from_utf8_lossy(bytes).to_lowercase().into_bytes()
    }
}

fn eq_ignore_case(path: &[u8], lower: &[u8]) -> bool {
    if path.is_ascii() {
        path.eq_ignore_ascii_case(lower)
    } else {
        lowercase(path) == lower
    }
}

impl Matcher {
    pub fn new(pattern: &str) -> Matcher {
        if !can_match(pattern) {
            return Matcher::Never;
        }
        if pattern == "*" {
            return Matcher::Always;
        }
        if pattern.contains(['*', '?', '[', '\\']) {
            return Matcher::Regex {
                pattern: pattern.to_string(),
                regex: OnceLock::new(),
            };
        }
        let dir_only = pattern.ends_with('/');
        let trimmed = pattern.strip_suffix('/').unwrap_or(pattern);
        if HAS_LEADING_OR_MIDDLE_SLASH.is_match(pattern).unwrap() {
            let path = trimmed.strip_prefix('/').unwrap_or(trimmed);
            Matcher::Exact {
                path: lowercase(path.as_bytes()),
                dir_only,
            }
        } else {
            Matcher::Name {
                name: lowercase(trimmed.as_bytes()),
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
                (is_dir || !dir_only) && eq_ignore_case(trimmed, path)
            }
            Matcher::Name { name, dir_only } => {
                (is_dir || !dir_only)
                    && trimmed.len() >= name.len()
                    && eq_ignore_case(&trimmed[trimmed.len() - name.len()..], name)
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

    #[test]
    fn matches_node_ignore_sources() {
        // From running node-ignore 5.2.4.
        for (pattern, expected) in [
            ("*", "(?:^|\\/)[^/]*(?=$|\\/$)"),
            ("/web/", "^web\\/$"),
            ("*.md", "(?:^|\\/)[^\\/]*\\.md(?=$|\\/$)"),
            ("docs/*", "^docs\\/[^/]+(?=$|[^\\/]$)"),
            ("a/**/b", "^a(?:\\/[^\\/]+)*\\/b(?=$|\\/$)"),
            ("**/foo", "^(?:.*\\/)?foo(?=$|\\/$)"),
            ("/lib/**", "^lib\\/.+(?=$|\\/$)"),
            ("a?c", "(?:^|\\/)a[^\\/]c(?=$|\\/$)"),
            ("[a-c]x", "(?:^|\\/)[a-c]x(?=$|\\/$)"),
        ] {
            assert_eq!(javascript_regex(pattern).unwrap(), expected, "{pattern}");
        }
    }

    #[test]
    fn negations_never_match() {
        assert!(compile("!a.md").is_none());
    }

    #[test]
    fn plain_paths_match_like_their_regexes() {
        let paths: [&[u8]; 12] = [
            b"docs",
            b"docs/",
            b"Docs/",
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
            "docs", "docs/", "/docs", "/docs/", "a/b", "/a/b/", "a/docs", "*",
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
