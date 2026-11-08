use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::pattern;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeownersRule {
    pub pattern: String,
    pub owners: Vec<String>,
    // Zero-based line number in the CODEOWNERS file.
    pub lineno: usize,
    // Byte range of the pattern in the CODEOWNERS file.
    pub pattern_span: Range<usize>,
}

/// Where GitHub looks for CODEOWNERS, in the order it checks them.
pub const CODEOWNERS_LOCATIONS: [&str; 3] = [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];

/// Path of the CODEOWNERS file GitHub would use for `root`.
pub fn find_codeowners(root: &Path) -> Option<PathBuf> {
    CODEOWNERS_LOCATIONS
        .iter()
        .map(|location| root.join(location))
        .find(|path| path.exists())
}

/// A CODEOWNERS file with rules that GitHub ignores for `root`, because the
/// one it uses, whose text is `used`, has none. GitHub uses the first file
/// it finds, even one that's empty or only comments.
pub fn ignored_codeowners(root: &Path, used: &str) -> Option<PathBuf> {
    if lines(used).next().is_some() {
        return None;
    }
    CODEOWNERS_LOCATIONS
        .iter()
        .map(|location| root.join(location))
        .filter(|path| path.exists())
        .skip(1)
        .find(|path| std::fs::read_to_string(path).is_ok_and(|text| lines(&text).next().is_some()))
}

/// The root of the git repository containing `dir`: the nearest directory
/// with a `.git` directory, or a `.git` file, as in worktrees and submodules.
pub fn find_repo_root(dir: &Path) -> Option<PathBuf> {
    let dir = std::path::absolute(dir).ok()?;
    dir.ancestors()
        .find(|ancestor| ancestor.join(".git").exists())
        .map(Path::to_path_buf)
}

/// The repository root for a CODEOWNERS file in one of GitHub's locations.
pub fn repo_root_for_codeowners(codeowners: &Path) -> PathBuf {
    let dir = codeowners.parent().unwrap_or(Path::new(""));
    match dir.file_name().and_then(|name| name.to_str()) {
        Some(".github" | "docs") => dir.parent().unwrap_or(Path::new("")).to_path_buf(),
        _ => dir.to_path_buf(),
    }
}

/// GitHub ignores CODEOWNERS files bigger than this. Its docs say "under
/// 3 MB", but it loads files of exactly 3 MiB.
pub const MAX_CODEOWNERS_BYTES: u64 = 3 * 1024 * 1024;

/// Whether GitHub ignores a CODEOWNERS file with `text` for being too big.
pub fn too_big(text: impl AsRef<[u8]>) -> bool {
    text.as_ref().len() as u64 > MAX_CODEOWNERS_BYTES
}

/// Fail if GitHub ignores the CODEOWNERS file shown as `display`, with
/// `text`, for being too big.
pub fn check_size(display: &str, text: impl AsRef<[u8]>) -> Result<(), String> {
    let len = text.as_ref().len();
    if len as u64 > MAX_CODEOWNERS_BYTES {
        return Err(format!(
            "{display} is {len} bytes, over GitHub's limit of {MAX_CODEOWNERS_BYTES}, so GitHub ignores it"
        ));
    }
    Ok(())
}

// An owner GitHub accepts: @user, @org/team, or an email address with a dot
// in its domain. Otherwise GitHub ignores the line.
//
// A line whose user or team doesn't exist still applies, though GitHub can't
// request a review from that owner. So matching files have no owner.
static OWNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        ^(?:
            # @user or @org/team
            @[A-Za-z0-9_-]+(?:/[A-Za-z0-9_-]+)?
            |
            # email local part
            [A-Za-z0-9.!\#$%&'*+/=?^_`{|}~-]+
            @
            # email domain
            [A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}
        )$",
    )
    .unwrap()
});

/// A word in a CODEOWNERS file, as written, with escapes.
struct Word<'a> {
    text: &'a str,
    // Byte range in the CODEOWNERS file.
    span: Range<usize>,
}

/// Split on whitespace up to a comment. Like on GitHub, an unescaped `#`
/// starts a comment anywhere, even inside a pattern, and an escaped space
/// is part of a pattern. `offset` is where `line` starts in the file.
fn split_words(line: &str, offset: usize) -> Vec<Word<'_>> {
    let word = |range: Range<usize>| Word {
        text: &line[range.clone()],
        span: range.start + offset..range.end + offset,
    };
    let mut words = Vec::new();
    let mut start = None;
    let mut end = line.len();
    let mut chars = line.char_indices();
    while let Some((i, c)) = chars.next() {
        if c == '#' {
            end = i;
            break;
        } else if c.is_whitespace() {
            if let Some(start) = start.take() {
                words.push(word(start..i));
            }
        } else {
            start.get_or_insert(i);
            if c == '\\' {
                chars.next();
            }
        }
    }
    if let Some(start) = start {
        words.push(word(start..end));
    }
    words
}

/// Each line's number, pattern and owners, skipping blank lines and comments.
fn lines(text: &str) -> impl Iterator<Item = (usize, Word<'_>, Vec<Word<'_>>)> {
    let mut next_line = 0;
    text.split('\n')
        .enumerate()
        .filter_map(move |(lineno, line)| {
            let offset = next_line;
            next_line += line.len() + 1;
            let mut words =
                split_words(line.strip_suffix('\r').unwrap_or(line), offset).into_iter();
            let pattern = words.next()?;
            Some((lineno, pattern, words.collect()))
        })
}

/// Parse CODEOWNERS text into rules. Like GitHub, lines with an invalid
/// owner are skipped.
pub fn parse_codeowners(text: &str) -> Vec<CodeownersRule> {
    lines(text)
        .filter(|(_, _, owners)| owners.iter().all(|owner| OWNER.is_match(owner.text)))
        .map(|(lineno, pattern, owners)| CodeownersRule {
            pattern: pattern.text.to_string(),
            owners: owners.iter().map(|owner| owner.text.to_string()).collect(),
            lineno,
            pattern_span: pattern.span,
        })
        .collect()
}

/// Something wrong in a CODEOWNERS file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineProblem {
    /// Zero-based line number, or None for the whole file.
    pub lineno: Option<usize>,
    /// Byte range of the offending word in the file, if it's on a line.
    pub span: Option<Range<usize>>,
    /// Whether GitHub skips the line, or ignores the file. Otherwise GitHub
    /// accepts it, but it likely doesn't do what it looks like it does.
    pub error: bool,
    pub message: String,
    /// What to replace the span with to fix the problem, if it's clear.
    pub fix: Option<String>,
}

impl LineProblem {
    /// The message's title, and the part after " - ", which labels the word
    /// with the problem. A pattern or owner can't contain " - ", since it
    /// would need an unescaped space.
    pub fn title_and_label(&self) -> (&str, Option<&str>) {
        match self.message.split_once(" - ") {
            Some((title, label)) => (title, Some(label)),
            None => (&self.message, None),
        }
    }
}

pub fn find_problems(text: &str) -> Vec<LineProblem> {
    let mut problems = Vec::new();
    if too_big(text) {
        problems.push(LineProblem {
            lineno: None,
            span: None,
            error: true,
            message: format!(
                "GitHub ignores CODEOWNERS files over {MAX_CODEOWNERS_BYTES} bytes, and this one is {} bytes",
                text.len()
            ),
            fix: None,
        });
    }
    for (lineno, pattern, owners) in lines(text) {
        if let Some(problem) = pattern::problem(pattern.text) {
            problems.push(LineProblem {
                lineno: Some(lineno),
                span: Some(pattern.span),
                error: problem.invalid,
                message: problem.message,
                fix: problem.fix,
            });
        }
        for owner in owners.iter().filter(|owner| !OWNER.is_match(owner.text)) {
            problems.push(LineProblem {
                lineno: Some(lineno),
                span: Some(owner.span.clone()),
                error: true,
                message: format!(
                    "`{}` isn't a valid owner - owners must be @user, @org/team or an email address",
                    owner.text
                ),
                fix: None,
            });
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules_with_their_line_numbers() {
        let text = "# Default owners\r\n*  @acme/platform\r\n\r\n/docs/ @acme/docs docs@example.com # comment\r\n/vendor/";
        let rules = parse_codeowners(text);
        let summary: Vec<_> = rules
            .iter()
            .map(|r| (r.pattern.as_str(), r.owners.len(), r.lineno))
            .collect();
        assert_eq!(summary, [("*", 1, 1), ("/docs/", 2, 3), ("/vendor/", 0, 4)]);
    }

    #[test]
    fn keeps_escaped_spaces_in_patterns() {
        let rules = parse_codeowners("foo/b\\ ar.py @acme/x\n");
        assert_eq!(rules[0].pattern, "foo/b\\ ar.py");
        assert_eq!(rules[0].owners, ["@acme/x"]);
    }

    #[test]
    fn starts_comments_at_unescaped_hashes() {
        let rules = parse_codeowners("a#b @acme/x\n\\#c @acme/y\n");
        let summary: Vec<_> = rules
            .iter()
            .map(|r| (r.pattern.as_str(), r.owners.len()))
            .collect();
        assert_eq!(summary, [("a", 0), ("\\#c", 1)]);
    }

    #[test]
    fn accepts_owners_like_github() {
        // From a test repository on GitHub, which used a line with each of
        // these owners, and skipped the rest.
        for owner in [
            "@octocat",
            "@no-such-user",
            "@-x",
            "@a--b",
            "@user-",
            "@CAPS",
            "@user_x",
            "@org/team-name_1",
            "@org/Team",
            "nobody@example.com",
            "UPPER@Example.COM",
            "a.b+c@example.com",
            ".a@example.com",
            "a..b@example.com",
            "a@sub.example.co",
            "a@-x.com",
        ] {
            assert_eq!(parse_codeowners(&format!("a {owner}")).len(), 1, "{owner}");
        }
        for owner in [
            "not-an-owner",
            "@",
            "@org/",
            "@/team",
            "@org//team",
            "@org/team/sub",
            "@a-b/c.d",
            "@user.name",
            "@user,",
            "@@user",
            "\"q\"@example.com",
            "a@b",
            "a@example",
            "a@example.c",
            "a@example.com.",
            "a@exa_mple.com",
            "a@[1.2.3.4]",
            "@user@example.com",
        ] {
            assert!(
                parse_codeowners(&format!("a {owner}")).is_empty(),
                "{owner}"
            );
        }
    }

    #[test]
    fn finds_problems_on_each_line() {
        let text = "# comment\n*.js @a\n[ab]x @a\n!foo @a\n/ @a\n*.md not-an-owner\n\\[a\n";
        let summary: Vec<_> = find_problems(text)
            .into_iter()
            .map(|p| (p.lineno, p.error))
            .collect();
        assert_eq!(
            summary,
            [
                (Some(2), true),
                (Some(3), false),
                (Some(4), false),
                (Some(5), true),
                (Some(6), false),
            ]
        );
    }

    /// The text each problem's span covers.
    fn spanned(text: &str) -> Vec<&str> {
        find_problems(text)
            .into_iter()
            .map(|p| &text[p.span.unwrap()])
            .collect()
    }

    #[test]
    fn spans_cover_the_word_with_the_problem() {
        // Owners, after a pattern with an escaped space, and a comment.
        assert_eq!(
            spanned("a\\ b  bad-owner\t@ok bad2 # bad3\n"),
            ["bad-owner", "bad2"]
        );
        // A pattern ending at a comment.
        assert_eq!(spanned("  [x]#y @a\n"), ["[x]"]);
        // A pattern with an escaped space that has a problem.
        assert_eq!(spanned("[a\\ b] @a\n"), ["[a\\ b]"]);
        // After and inside non-ASCII text, including non-ASCII whitespace.
        assert_eq!(spanned("# 日本語\n/日本語/ 文档\n"), ["文档"]);
        assert_eq!(spanned("文档\u{3000}[語]\n"), ["[語]"]);
        assert_eq!(spanned("[é\\é] @a\n"), ["[é\\é]"]);
        // Each line ending, with and without a final newline.
        assert_eq!(spanned("* @a\r\n\r\n[x] @a\r\n"), ["[x]"]);
        assert_eq!(spanned("* @a\r\n* bad\r"), ["bad"]);
        assert_eq!(spanned("* bad"), ["bad"]);
    }

    #[test]
    fn rules_know_where_their_pattern_is() {
        let text = "# 日本語\r\n  /docs/ @a\r\na\\ b\n";
        let spans: Vec<_> = parse_codeowners(text)
            .into_iter()
            .map(|r| &text[r.pattern_span])
            .collect();
        assert_eq!(spans, ["/docs/", "a\\ b"]);
    }

    #[test]
    fn finds_codeowners_files_too_big_for_github() {
        let text = "#".repeat(MAX_CODEOWNERS_BYTES as usize + 1);
        assert_eq!(find_problems(&text)[0].lineno, None);
        assert!(find_problems(&text[1..]).is_empty());
    }

    #[test]
    fn skips_lines_with_an_invalid_owner() {
        assert!(parse_codeowners("/api/ @acme/api not-an-owner\n").is_empty());
    }

    #[test]
    fn finds_rules_github_ignores_for_an_empty_codeowners() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path();
        let write = |file: &str, text: &str| {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        // GitHub uses an empty, blank or comment-only .github/CODEOWNERS,
        // and gives no file an owner, as checked in a test repository.
        write("CODEOWNERS", "");
        write("docs/CODEOWNERS", "* @acme/docs\n");
        for used in ["", "\n", "# no rules\n"] {
            assert_eq!(
                ignored_codeowners(root, used),
                Some(root.join("docs/CODEOWNERS")),
                "{used:?}"
            );
        }
        assert_eq!(ignored_codeowners(root, "* @acme/x\n"), None);
        write("docs/CODEOWNERS", "# nothing\n");
        assert_eq!(ignored_codeowners(root, ""), None);
    }

    #[test]
    fn infers_the_repository_root() {
        assert_eq!(
            repo_root_for_codeowners(Path::new("/repo/.github/CODEOWNERS")),
            Path::new("/repo")
        );
        assert_eq!(
            repo_root_for_codeowners(Path::new("/repo/docs/CODEOWNERS")),
            Path::new("/repo")
        );
        assert_eq!(
            repo_root_for_codeowners(Path::new("/repo/CODEOWNERS")),
            Path::new("/repo")
        );
    }
}
