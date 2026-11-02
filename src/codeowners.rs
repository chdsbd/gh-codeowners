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

// An owner GitHub accepts: @user, @org/team, or an email address with a dot
// in its domain. Otherwise GitHub ignores the line.
//
// A line whose user or team doesn't exist still applies, though GitHub can't
// request a review from that owner. So matching files have no owner.
static OWNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:@[A-Za-z0-9_-]+(?:/[A-Za-z0-9_-]+)?|[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,})$",
    )
    .unwrap()
});

/// Split on whitespace up to a comment. Like on GitHub, an unescaped `#`
/// starts a comment anywhere, even inside a pattern, and an escaped space
/// is part of a pattern.
fn split_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            word.push(c);
            if let Some(next) = chars.next() {
                word.push(next);
            }
        } else if c == '#' {
            break;
        } else if c.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(c);
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Each line's pattern and owners, skipping blank lines and comments.
fn lines(text: &str) -> impl Iterator<Item = (usize, String, Vec<String>)> {
    text.split('\n').enumerate().filter_map(|(lineno, line)| {
        let mut words = split_words(line.strip_suffix('\r').unwrap_or(line)).into_iter();
        let pattern = words.next()?;
        Some((lineno, pattern, words.collect()))
    })
}

/// Parse CODEOWNERS text into rules. Like GitHub, lines with an invalid
/// owner are skipped.
pub fn parse_codeowners(text: &str) -> Vec<CodeownersRule> {
    lines(text)
        .filter(|(_, _, owners)| owners.iter().all(|owner| OWNER.is_match(owner)))
        .map(|(lineno, pattern, owners)| CodeownersRule {
            pattern,
            owners,
            lineno,
        })
        .collect()
}

/// Something wrong in a CODEOWNERS file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineProblem {
    /// Zero-based line number, or None for the whole file.
    pub lineno: Option<usize>,
    /// Whether GitHub skips the line, or ignores the file. Otherwise GitHub
    /// accepts it, but it likely doesn't do what it looks like it does.
    pub error: bool,
    pub message: String,
}

/// Everything wrong in CODEOWNERS text. Errors are what GitHub's CODEOWNERS
/// errors API reports, except owners that don't exist, which only GitHub can
/// check. Warnings are lines GitHub accepts that likely don't do what they
/// look like they do: patterns that match nothing, and `!` patterns, which
/// don't negate.
pub fn find_problems(text: &str) -> Vec<LineProblem> {
    let mut problems = Vec::new();
    if text.len() as u64 > MAX_CODEOWNERS_BYTES {
        problems.push(LineProblem {
            lineno: None,
            error: true,
            message: format!(
                "GitHub ignores CODEOWNERS files over {MAX_CODEOWNERS_BYTES} bytes, and this one is {} bytes",
                text.len()
            ),
        });
    }
    for (lineno, pattern, owners) in lines(text) {
        if let Some(problem) = pattern::problem(&pattern) {
            problems.push(LineProblem {
                lineno: Some(lineno),
                error: problem.invalid,
                message: problem.message.to_string(),
            });
        }
        for owner in owners.iter().filter(|owner| !OWNER.is_match(owner)) {
            problems.push(LineProblem {
                lineno: Some(lineno),
                error: true,
                message: format!(
                    "`{owner}` isn't a valid owner - owners must be @user, @org/team or an email address"
                ),
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
        let root =
            std::env::temp_dir().join(format!("gh-codeowners-ignored-{}", std::process::id()));
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
                ignored_codeowners(&root, used),
                Some(root.join("docs/CODEOWNERS")),
                "{used:?}"
            );
        }
        assert_eq!(ignored_codeowners(&root, "* @acme/x\n"), None);
        write("docs/CODEOWNERS", "# nothing\n");
        assert_eq!(ignored_codeowners(&root, ""), None);
        std::fs::remove_dir_all(&root).unwrap();
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
