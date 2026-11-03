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
    pub message: String,
}

impl Problem {
    fn invalid(message: String) -> Problem {
        Problem {
            invalid: true,
            matches_nothing: true,
            message,
        }
    }

    fn matches_nothing(message: &str) -> Problem {
        Problem {
            invalid: false,
            matches_nothing: true,
            message: message.to_string(),
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
    if strip_trailing_escape(pattern)
        .unwrap_or(pattern)
        .trim_matches('/')
        .is_empty()
    {
        return Some(Problem::matches_nothing(
            "the pattern will never match any files - did you mean `*` instead?",
        ));
    }
    if !brackets_pair_up(pattern) {
        return Some(Problem::matches_nothing(
            "GitHub matches nothing when a pattern has a different number of `\\[`s and `\\]`s",
        ));
    }
    if pattern
        .split('/')
        .any(|segment| segment == "." || segment == "..")
    {
        return Some(Problem::matches_nothing(
            "paths in git never have `.` or `..` segments, so GitHub matches nothing",
        ));
    }
    if pattern.starts_with("\\[") && !pattern.contains('/') {
        return Some(Problem::matches_nothing(&format!(
            "GitHub matches nothing for a pattern starting with `\\[` and without a slash - did you mean `**/{pattern}`?"
        )));
    }
    if name.is_some() {
        return Some(Problem {
            invalid: false,
            matches_nothing: false,
            message: "`!` doesn't negate a pattern on GitHub - this matches files named like the rest of the pattern, but not directories".to_string(),
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
/// more than two `*`s in a row. Where the fix is clear, the message suggests
/// the fixed pattern, like GitHub does for `*`s.
fn invalid_reason(pattern: &str) -> Option<String> {
    let mut stars = 0;
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        stars = if c == '*' { stars + 1 } else { 0 };
        let reason = match c {
            // GitHub allows a trailing `\`, which escapes nothing.
            '\\' if !matches!(chars.next(), None | Some(' ' | '\t' | '#' | '[' | ']' | '\\')) => {
                "all `\\` characters must be escaped with another `\\` unless escaping: tab, space, #, [, ]".to_string()
            }
            '[' | ']' => format!(
                "both `[` and `]` must be escaped with a `\\`, like `{}`",
                escape_brackets(pattern)
            ),
            _ if stars > 2 => format!("did you mean `{}` instead?", collapse_stars(pattern)),
            _ => continue,
        };
        return Some(format!("`{pattern}` isn't a valid pattern - {reason}"));
    }
    None
}

/// `pattern` with each unescaped `[` and `]` escaped.
fn escape_brackets(pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                out.extend(chars.next());
            }
            '[' | ']' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// `pattern` with each run of more than two `*`s cut to `**`.
fn collapse_stars(pattern: &str) -> String {
    let mut out = String::new();
    let mut stars = 0;
    for c in pattern.chars() {
        stars = if c == '*' { stars + 1 } else { 0 };
        if stars <= 2 {
            out.push(c);
        }
    }
    out
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
    let Some(rest) = strip_trailing_escape(pattern) else {
        return Some(format!("(?s-u){}", escaped_regex(pattern, files_only)));
    };
    // A trailing `\` escapes nothing, so GitHub drops it. With a slash, the
    // pattern also matches it as a literal `\`.
    let mut sources = Vec::new();
    if !rest.ends_with('/') {
        sources.push(escaped_regex(rest, files_only));
    }
    if is_anchored(pattern) {
        sources.push(escaped_regex(&format!("{rest}\\\\"), files_only));
    }
    if sources.is_empty() {
        return None;
    }
    Some(format!("(?s-u)(?:{})", sources.join(")|(?:")))
}

/// `pattern` without a trailing `\` that escapes nothing, if it has one.
fn strip_trailing_escape(pattern: &str) -> Option<&str> {
    let rest = pattern.strip_suffix('\\')?;
    let backslashes = rest.len() - rest.trim_end_matches('\\').len();
    (backslashes % 2 == 0).then_some(rest)
}

fn escaped_regex(pattern: &str, files_only: bool) -> String {
    let unescaped = pattern_regex(pattern, files_only, false);
    // GitHub matches escaped brackets either way, but not a mix of the two.
    if pattern.contains("\\[") {
        let raw = pattern_regex(pattern, files_only, true);
        format!("(?:{unescaped})|(?:{raw})")
    } else {
        unescaped
    }
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
    // A pattern whose wildcards are `*`s and `?`s, matched a segment at a
    // time, like `/drivers/*/*acpi*` or `*_test?.py`.
    Glob {
        // The leading directories without wildcards, each with a trailing
        // slash. None for a pattern without a slash, which matches a name at
        // any depth.
        dir: Option<Vec<u8>>,
        // The rest of the path, one glob per segment.
        segments: Vec<Vec<u8>>,
        dir_only: bool,
        // Patterns ending in `/*` don't match directories.
        files_only: bool,
    },
    Regex {
        pattern: String,
        regex: OnceLock<Option<regex::bytes::Regex>>,
    },
}

/// The `Glob` matcher for `pattern`, if it has that shape: wildcards, but
/// no escapes or `**` segments.
fn glob(pattern: &str) -> Option<Matcher> {
    if pattern.contains('\\') || !pattern.contains(['*', '?']) {
        return None;
    }
    let trimmed = pattern.strip_suffix('/').unwrap_or(pattern);
    let trimmed = trimmed.strip_prefix('/').unwrap_or(trimmed);
    let segments: Vec<&str> = trimmed.split('/').collect();
    // `**` alone is a recursive wildcard, not two `*`s.
    if segments.contains(&"**") {
        return None;
    }
    let literal = segments
        .iter()
        .take_while(|segment| !segment.contains(['*', '?']))
        .count();
    let dir: String = segments[..literal]
        .iter()
        .map(|s| format!("{s}/"))
        .collect();
    Some(Matcher::Glob {
        dir: is_anchored(pattern).then(|| dir.into_bytes()),
        segments: segments[literal..]
            .iter()
            .map(|segment| segment.as_bytes().to_vec())
            .collect(),
        dir_only: is_dir_only(pattern),
        files_only: pattern.ends_with("/*"),
    })
}

/// Whether `name`, one path segment, matches `glob`, where `*` matches any
/// run of bytes and `?` one byte. A glob of only `*`s can't match an empty
/// name.
fn segment_match(glob: &[u8], name: &[u8]) -> bool {
    if glob.iter().all(|&c| c == b'*') {
        return !name.is_empty();
    }
    let (mut g, mut n) = (0, 0);
    // After a mismatch, retry from the last `*`, letting it match one more
    // byte. Trying the shortest match for each `*` first finds a match if
    // there is one.
    let mut retry: Option<(usize, usize)> = None;
    while n < name.len() {
        match glob.get(g) {
            Some(b'*') => {
                retry = Some((g, n));
                g += 1;
            }
            Some(&c) if c == b'?' || c == name[n] => {
                g += 1;
                n += 1;
            }
            _ => match retry {
                Some((star, start)) => {
                    retry = Some((star, start + 1));
                    g = star + 1;
                    n = start + 1;
                }
                None => return false,
            },
        }
    }
    glob[g..].iter().all(|&c| c == b'*')
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
                segments,
                dir_only,
                files_only,
            } => {
                let rest = match dir {
                    Some(dir) => match trimmed.strip_prefix(dir.as_slice()) {
                        Some(rest) => rest,
                        None => return false,
                    },
                    None => {
                        let slash = trimmed.iter().rposition(|&c| c == b'/');
                        &trimmed[slash.map_or(0, |i| i + 1)..]
                    }
                };
                let mut names = rest.split(|&c| c == b'/');
                (is_dir || !dir_only)
                    && !(is_dir && *files_only)
                    && segments
                        .iter()
                        .all(|glob| names.next().is_some_and(|name| segment_match(glob, name)))
                    && names.next().is_none()
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
            ("a\\", true),
            ("a\\\\\\", true),
            ("x/\\", true),
        ] {
            assert_eq!(compile(pattern).is_some(), valid, "{pattern}");
        }
    }

    #[test]
    fn suggests_fixes_for_invalid_patterns() {
        let message = |pattern| problem(pattern).unwrap().message;
        assert_eq!(
            message("/tmp/***"),
            "`/tmp/***` isn't a valid pattern - did you mean `/tmp/**` instead?"
        );
        assert_eq!(
            message("a/****/b"),
            "`a/****/b` isn't a valid pattern - did you mean `a/**/b` instead?"
        );
        assert_eq!(
            message("/app/[id]/\\[x\\]"),
            "`/app/[id]/\\[x\\]` isn't a valid pattern - both `[` and `]` must be escaped with a `\\`, like `/app/\\[id\\]/\\[x\\]`"
        );
    }

    #[test]
    fn matches_a_trailing_backslash_like_github() {
        // From a test repository on GitHub.
        for (pattern, path, expected) in [
            ("a\\", "x/a", true),
            ("a\\", "a\\", false),
            ("a?\\", "a\\", true),
            ("/a\\", "a", true),
            ("/a\\", "a\\", true),
            ("**/a\\", "x/a\\", true),
            ("x/a\\", "x/a", true),
            ("x/a\\", "x/a\\", true),
            ("x/\\", "x/a", false),
        ] {
            assert_eq!(matches(pattern, path), expected, "{pattern} {path}");
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
            ".",
            "/.",
            "..",
            "./a",
            "./docs/",
            "../a",
            "a/.",
            "a/../docs",
            "docs/./x/",
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
        let paths: [&[u8]; 36] = [
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
            b"src/x/a",
            b"src/x/a/",
            b"src/y/a",
            b"src/x/y/a",
            b"x/a",
            b"a/x/",
            b"src/x/acpi_x.c",
            b"src/ab",
            b"src/abc",
            b"src/a/c",
            b"src/aXc",
            b"src/aaab",
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
            "/src/*/a",
            "/src/*/a/",
            "/src/a?",
            "/src/a?c",
            "/src/a?*c",
            "/src/a*?",
            "/src/?.c",
            "/src/*/*acpi*",
            "/*/*",
            "/*/x/*",
            "/a/*/",
            "src/*/a",
            "?*",
            "?.c",
            "??",
            "*a*ab",
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
            "/src/**", "/a/**/b", "**", "**/", "/\\#*", "/\\#?/*", "!a*", "!a?",
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
