use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

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
// in its domain. GitHub skips lines with any other owner, but not lines with
// users or teams that don't exist, which only can't be assigned.
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

/// Parse CODEOWNERS text into rules. Like GitHub, lines with an invalid
/// owner are skipped.
pub fn parse_codeowners(text: &str) -> Vec<CodeownersRule> {
    let text = text.replace('\r', "");
    let mut rules = Vec::new();
    for (lineno, line) in text.split('\n').enumerate() {
        let mut words = split_words(line).into_iter();
        let Some(pattern) = words.next() else {
            continue;
        };
        let owners: Vec<String> = words.collect();
        if owners.iter().all(|owner| OWNER.is_match(owner)) {
            rules.push(CodeownersRule {
                pattern: pattern.to_string(),
                owners,
                lineno,
            });
        }
    }
    rules
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
    fn skips_lines_with_an_invalid_owner() {
        assert!(parse_codeowners("/api/ @acme/api not-an-owner\n").is_empty());
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
