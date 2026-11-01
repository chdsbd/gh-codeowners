//! Turns a CODEOWNERS pattern into a regex, the way GitHub matches them:
//! gitignore rules, except that paths are case-sensitive, there are no
//! character classes, and `!` doesn't negate. Unlike gitignore, a pattern
//! ending in `/*` doesn't match nested files, and one ending in `/**/` isn't
//! limited to directories.
//! Where GitHub's behavior differs from its docs, this follows what GitHub
//! does, as checked against a test repository.

use std::sync::OnceLock;

/// For a `!` pattern without a slash, the rest of the pattern. GitHub
/// doesn't negate these, but matches file names like the rest would, and
/// not directories. With a slash, `!` is an ordinary character.
fn negated_name(pattern: &str) -> Option<&str> {
    pattern.strip_prefix('!').filter(|rest| !rest.contains('/'))
}

/// Whether a pattern only matches directories: it ends in `/`, but not
/// `/**/`, which GitHub matches like `/**`.
pub fn is_dir_only(pattern: &str) -> bool {
    let trimmed = pattern.trim_end_matches('/');
    pattern.ends_with('/') && trimmed != "**" && !trimmed.ends_with("/**")
}

/// Something wrong with a pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Whether GitHub rejects the pattern, which its CODEOWNERS errors API
    /// reports. Otherwise GitHub accepts it, but it likely doesn't do what
    /// it looks like it does.
    pub invalid: bool,
    /// Whether the pattern matches nothing on GitHub.
    pub matches_nothing: bool,
    pub message: &'static str,
}

impl Problem {
    fn invalid(message: &'static str) -> Problem {
        Problem {
            invalid: true,
            matches_nothing: true,
            message,
        }
    }

    fn matches_nothing(message: &'static str) -> Problem {
        Problem {
            invalid: false,
            matches_nothing: true,
            message,
        }
    }
}

/// What's wrong with `pattern`, if anything. The messages for invalid
/// patterns follow GitHub's.
pub fn problem(pattern: &str) -> Option<Problem> {
    if let Some(message) = invalid_reason(pattern) {
        return Some(Problem::invalid(message));
    }
    let name = negated_name(pattern);
    let pattern = name.unwrap_or(pattern);
    if pattern.trim_matches('/').is_empty() {
        return Some(Problem::matches_nothing(
            "the pattern will never match any files - did you mean `*` instead?",
        ));
    }
    if !brackets_pair_up(pattern) {
        return Some(Problem::matches_nothing(
            "GitHub matches nothing when a pattern has a different number of `\\[`s and `\\]`s",
        ));
    }
    if pattern.starts_with("\\[") && !pattern.contains('/') {
        return Some(Problem::matches_nothing(
            "GitHub matches nothing for a pattern starting with `\\[` and without a slash - did you mean to start it with `**/`?",
        ));
    }
    if name.is_some() {
        return Some(Problem {
            invalid: false,
            matches_nothing: false,
            message: "`!` doesn't negate a pattern on GitHub - this matches files named like the rest of the pattern, but not directories",
        });
    }
    None
}

/// Whether `pattern` can match a path. Blank lines, comments and patterns
/// GitHub rejects as invalid can't, nor can a few that GitHub accepts but
/// never matches.
pub fn can_match(pattern: &str) -> bool {
    !(pattern.trim().is_empty() || pattern.starts_with('#'))
        && problem(pattern).is_none_or(|p| !p.matches_nothing)
}

/// Why GitHub rejects `pattern`, if it does: `[` and `]` must be escaped,
/// `\` can only escape a space, tab, `#`, `[`, `]` or `\`, and there can't be
/// more than two `*`s in a row.
fn invalid_reason(pattern: &str) -> Option<&'static str> {
    let mut stars = 0;
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        stars = if c == '*' { stars + 1 } else { 0 };
        match c {
            '\\' if !matches!(chars.next(), Some(' ' | '\t' | '#' | '[' | ']' | '\\')) => {
                return Some(
                    "all `\\` characters must be escaped with another `\\` unless escaping: tab, space, #, [, ]",
                );
            }
            '[' | ']' => return Some("both `[` and `]` must be escaped with a `\\`"),
            _ if stars > 2 => return Some("did you mean `**` instead?"),
            _ => {}
        }
    }
    None
}

/// Whether there are as many `\[`s as `\]`s, in any order. GitHub matches
/// nothing otherwise.
fn brackets_pair_up(pattern: &str) -> bool {
    let mut balance = 0i32;
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('[') => balance += 1,
                Some(']') => balance -= 1,
                _ => {}
            }
        }
    }
    balance == 0
}

/// Whether `pattern` is relative to the root: it has a leading or middle
/// slash. Otherwise it matches at any depth.
fn is_anchored(pattern: &str) -> bool {
    pattern.strip_suffix('/').unwrap_or(pattern).contains('/')
}

/// Append the regex for one path segment, without slashes. With
/// `raw_brackets`, `\\[` and `\\]` match a backslash and a bracket.
fn push_segment(out: &mut String, segment: &str, raw_brackets: bool) {
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
                    if raw_brackets && matches!(escaped, '[' | ']') {
                        push_literal(out, '\\');
                    }
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
    let (pattern, files_only) = match negated_name(pattern) {
        Some(rest) => (rest, true),
        None => (pattern, false),
    };
    let unescaped = pattern_regex(pattern, files_only, false);
    // GitHub matches escaped brackets either way, but not a mix of the two.
    Some(if pattern.contains("\\[") {
        let raw = pattern_regex(pattern, files_only, true);
        format!("(?s-u)(?:{unescaped})|(?:{raw})")
    } else {
        format!("(?s-u){unescaped}")
    })
}

/// With `files_only`, the regex doesn't match directories.
fn pattern_regex(pattern: &str, files_only: bool, raw_brackets: bool) -> String {
    let dir_only = is_dir_only(pattern);
    let trimmed = pattern.strip_suffix('/').unwrap_or(pattern);
    let trimmed = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let mut out = String::from(if is_anchored(pattern) {
        "^"
    } else {
        "^(?:.*/)?"
    });
    let segments: Vec<&str> = trimmed.split('/').collect();
    for (i, segment) in segments.iter().enumerate() {
        let last = i == segments.len() - 1;
        if *segment == "**" {
            // A trailing `/**` matches everything inside, and a leading or
            // middle `**/` zero or more directories.
            if last {
                out.push_str(if dir_only { ".+/$" } else { ".+" });
                return out;
            }
            out.push_str("(?:.*/)?");
            continue;
        }
        push_segment(&mut out, segment, raw_brackets);
        if !last {
            out.push('/');
        }
    }
    if dir_only {
        out.push_str("/$");
    } else if files_only || pattern.ends_with("/*") {
        // GitHub doesn't match nested files for rules ending in `/*`.
        out.push('$');
    } else {
        // The path, or a directory containing it.
        out.push_str("/?$");
    }
    out
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
    // Names directly in a directory, from a pattern whose last segment's
    // only wildcards are `*`s, like `/src/*.test.*`, or at any depth for a
    // pattern without a slash, like `*_test*.py`.
    Glob {
        // The directory, with a trailing slash, or empty for the root. None
        // for any depth.
        dir: Option<Vec<u8>>,
        // The last segment split at its `*`s.
        parts: Vec<Vec<u8>>,
        dir_only: bool,
        // Patterns ending in `/*` don't match directories.
        files_only: bool,
    },
    Regex {
        pattern: String,
        regex: OnceLock<Option<regex::bytes::Regex>>,
    },
}

/// The `Glob` matcher for `pattern`, if it has that shape.
fn glob(pattern: &str) -> Option<Matcher> {
    if pattern.contains(['?', '\\']) {
        return None;
    }
    let dir_only = is_dir_only(pattern);
    let trimmed = pattern.strip_suffix('/').unwrap_or(pattern);
    let trimmed = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let (dir, name) = match trimmed.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", trimmed),
    };
    // `**` alone is a recursive wildcard, not two `*`s.
    if dir.contains('*') || !name.contains('*') || name == "**" {
        return None;
    }
    Some(Matcher::Glob {
        dir: if !is_anchored(pattern) {
            None
        } else if dir.is_empty() {
            Some(Vec::new())
        } else {
            Some(format!("{dir}/").into_bytes())
        },
        parts: name
            .split('*')
            .map(|part| part.as_bytes().to_vec())
            .collect(),
        dir_only,
        files_only: pattern.ends_with("/*"),
    })
}

/// Whether `name` matches the `*`-separated `parts`, with each `*` matching
/// any run of bytes. A segment of only `*`s can't match an empty name.
fn glob_match(parts: &[Vec<u8>], name: &[u8]) -> bool {
    let (first, last) = (&parts[0], &parts[parts.len() - 1]);
    if name.is_empty()
        || name.len() < first.len() + last.len()
        || !name.starts_with(first)
        || !name.ends_with(last)
    {
        return false;
    }
    // Leftmost matches of the middle parts leave the most room for the rest.
    let mut rest = &name[first.len()..name.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match memchr::memmem::find(rest, part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

impl Matcher {
    pub fn new(pattern: &str) -> Matcher {
        if !can_match(pattern) {
            return Matcher::Never;
        }
        if negated_name(pattern).is_some() {
            return Matcher::Regex {
                pattern: pattern.to_string(),
                regex: OnceLock::new(),
            };
        }
        if pattern == "*" {
            return Matcher::Always;
        }
        if let Some(matcher) = glob(pattern) {
            return matcher;
        }
        if pattern.contains(['*', '?', '\\']) {
            return Matcher::Regex {
                pattern: pattern.to_string(),
                regex: OnceLock::new(),
            };
        }
        let dir_only = is_dir_only(pattern);
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
            Matcher::Glob {
                dir,
                parts,
                dir_only,
                files_only,
            } => {
                let name = match dir {
                    Some(dir) => match trimmed.strip_prefix(dir.as_slice()) {
                        Some(name) if !name.contains(&b'/') => name,
                        _ => return false,
                    },
                    None => {
                        let slash = trimmed.iter().rposition(|&c| c == b'/');
                        &trimmed[slash.map_or(0, |i| i + 1)..]
                    }
                };
                (is_dir || !dir_only) && !(is_dir && *files_only) && glob_match(parts, name)
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
        // From a test repository on GitHub.
        assert!(matches("/app/\\[id\\]/", "app/\\[id\\]/"));
        assert!(!matches("/app/\\[id\\]/", "app/\\[id]/"));
        assert!(!matches("/app/\\[id\\]/", "app/[id\\]/"));
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
    fn matches_negations_like_github() {
        // From a test repository on GitHub.
        for (pattern, path, expected) in [
            ("!foo", "foo", true),
            ("!foo", "x/foo", true),
            ("!foo", "!foo", false),
            ("!foo", "foo/", false),
            ("!*.md", "x/a.md", true),
            ("!*.md", "a.md/", false),
            ("!*", "a/b", true),
            ("!f?o", "foo", true),
            // With a slash, `!` is literal.
            ("!a/", "y/!a/", true),
            ("!a/", "a/", false),
            ("!a/b", "!a/b", true),
            ("!a/b", "y/!a/b", false),
            ("!/foo", "!/foo", true),
            ("!/foo", "foo", false),
            ("!*/x", "!/x", true),
            ("!**/foo", "!x/foo", true),
            ("!**/foo", "x/foo", false),
        ] {
            assert_eq!(matches(pattern, path), expected, "{pattern} {path}");
        }
    }

    #[test]
    fn matches_trailing_double_star_slash_like_double_star() {
        // From a test repository on GitHub.
        for (pattern, path, expected) in [
            ("**/", "x", true),
            ("/**/", "x", true),
            ("a/**/", "a/x", true),
            ("a/**/", "b/c", false),
            ("**/a/", "a/", true),
            ("**/a/", "a", false),
        ] {
            assert_eq!(matches(pattern, path), expected, "{pattern} {path}");
        }
    }

    #[test]
    fn matches_nothing_where_github_matches_nothing() {
        // From a test repository on GitHub.
        for pattern in [
            "\\[ab\\]x",
            "\\[ab\\]*.js",
            "a\\[b",
            "a\\]b",
            "/a\\]b",
            "/x\\]\\[y\\]",
            "a\\[b\\]c\\]d",
        ] {
            assert!(compile(pattern).is_none(), "{pattern}");
        }
        for pattern in [
            "\\[ab\\]x/",
            "/\\[ab\\]x",
            "**/\\[ab\\]x",
            "x\\[ab\\]",
            // Only the counts matter, not the order.
            "x\\]\\[y",
            "a\\]b/c\\[d",
        ] {
            assert!(compile(pattern).is_some(), "{pattern}");
        }
    }

    #[test]
    fn globs_match_like_their_regexes() {
        let paths: [&[u8]; 24] = [
            b"src",
            b"src/",
            b"src/a",
            b"src/a/",
            b"src/a.c",
            b"src/a.c/",
            b"src/x/a.c",
            b"src/x/a.c/",
            b"src/acpi",
            b"src/acpi_thermal.c",
            b"src/thermal_acpi",
            b"src/acpiacpi",
            b"src/aa",
            b"src/a",
            b"src/.c",
            b"Src/a.c",
            b"x/src/a.c",
            b"a.c",
            b"a.c/",
            b"ab/",
            b"a/b",
            b"src/\xc3\xa9.c",
            b"src/a*b",
            b"",
        ];
        for pattern in [
            "/src/*",
            "src/*",
            "/src/*/",
            "/src/*.c",
            "/src/*.c/",
            "/src/a*",
            "/src/*acpi*",
            "/src/*acpi*/",
            "/src/a*a*",
            "/src/a*c*a",
            "/src/**.c",
            "/src/a**",
            "/*",
            "/*/",
            "/*.c",
            "/a*",
            "/src/é*",
            "src/x/*",
            "*.c",
            "*.c/",
            "a*",
            "*acpi*",
            "*a*c*",
            "**.c",
            "*/",
        ] {
            let matcher = Matcher::new(pattern);
            assert!(matches!(matcher, Matcher::Glob { .. }), "{pattern}");
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
        // Other wildcards still use regexes.
        for pattern in [
            "/src/**", "/src/*/a", "/src/a?", "**", "**/", "?*", "/\\#*", "!a*",
        ] {
            assert!(
                matches!(Matcher::new(pattern), Matcher::Regex { .. }),
                "{pattern}"
            );
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
