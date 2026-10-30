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

// @user, @org/team, or an email address. Same as @snyk/github-codeowners.
static OWNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(^@[a-zA-Z0-9_\-/]*$)|(?:[a-z0-9!#$%&'*+/=?^_`{|}~-]+(?:\.[a-z0-9!#$%&'*+/=?^_`{|}~-]+)*|"(?:[\x01-\x08\x0b\x0c\x0e-\x1f\x21\x23-\x5b\x5d-\x7f]|\\[\x01-\x09\x0b\x0c\x0e-\x7f])*")@(?:(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]*[a-z0-9])?|\[(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?|[a-z0-9-]*[a-z0-9]:(?:[\x01-\x08\x0b\x0c\x0e-\x1f\x21-\x5a\x53-\x7f]|\\[\x01-\x09\x0b\x0c\x0e-\x7f])+)\])"#,
    )
    .unwrap()
});

/// Parse CODEOWNERS text into rules. Like GitHub, lines with an invalid
/// owner are skipped.
pub fn parse_codeowners(text: &str) -> Vec<CodeownersRule> {
    let text = text.replace('\r', "");
    let mut rules = Vec::new();
    for (lineno, line) in text.split('\n').enumerate() {
        let without_comment = line.split('#').next().unwrap_or("").trim();
        let mut words = without_comment.split_whitespace();
        let Some(pattern) = words.next() else {
            continue;
        };
        let owners: Vec<String> = words.map(String::from).collect();
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
