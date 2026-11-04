//! Counts the files each rule owns.

use std::collections::HashMap;

use crate::pattern::Matcher;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub pattern: String,
    pub lineno: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleCount {
    pub lineno: usize,
    pub owned: usize,
    // Whether the rule matches any file, even if a later rule owns them all.
    pub matches_any: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    pub files: usize,
    // Files no rule matches.
    pub unowned: usize,
    pub rules: Vec<RuleCount>,
}

fn literal_prefix(pattern: &str) -> Vec<u8> {
    let trimmed = pattern.strip_prefix('/').unwrap_or(pattern);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    // Without a leading or middle slash, a pattern matches at any depth.
    if !pattern.starts_with('/') && !trimmed.contains('/') {
        return Vec::new();
    }
    let literal: Vec<&str> = trimmed
        .split('/')
        .take_while(|segment| !segment.contains(['*', '?', '\\']))
        .collect();
    literal.join("/").into_bytes()
}

/// Compiled rules.
pub struct Rules {
    linenos: Vec<usize>,
    matchers: Vec<Matcher>,
    // Leading path segments without wildcards. Empty for rules that can match
    // anywhere.
    prefixes: Vec<Vec<u8>>,
}

impl Rules {
    pub fn new(rules: &[Rule]) -> Self {
        Rules {
            linenos: rules.iter().map(|r| r.lineno).collect(),
            matchers: rules.iter().map(|r| Matcher::new(&r.pattern)).collect(),
            prefixes: rules.iter().map(|r| literal_prefix(&r.pattern)).collect(),
        }
    }
}

/// Whether a rule with literal prefix `prefix` could match a file in `dir`:
/// one must contain the other.
fn could_match_in(prefix: &[u8], dir: &[u8]) -> bool {
    let (short, long) = if prefix.len() < dir.len() {
        (prefix, dir)
    } else {
        (dir, prefix)
    };
    short.is_empty()
        || (long.starts_with(short) && (long.len() == short.len() || long[short.len()] == b'/'))
}

/// The last of `candidates` that matches `path`, or `floor` if it's later,
/// recording which rules match.
fn last_match(
    rules: &Rules,
    matched: &mut [bool],
    candidates: &[usize],
    path: &[u8],
    floor: Option<usize>,
) -> Option<usize> {
    let mut last = floor;
    for &i in candidates.iter().rev() {
        if (Some(i) > last || !matched[i]) && rules.matchers[i].is_match(path) {
            matched[i] = true;
            last = last.max(Some(i));
        }
    }
    last
}

/// Count the files each rule owns. Files should be grouped by directory, as
/// git's index lists them; any order is correct, but out-of-order files redo
/// their directories' work.
pub fn count<P: AsRef<[u8]>>(rules: &Rules, files: &[P]) -> Counts {
    let all: Vec<usize> = (0..rules.matchers.len()).collect();
    let mut owned = vec![0; all.len()];
    let mut matched = vec![false; all.len()];
    let mut unowned = 0;
    // The last rule matching each directory.
    let mut dirs: HashMap<&[u8], Option<usize>> = HashMap::new();
    let mut current = None;
    let mut candidates = Vec::new();
    let mut dir_owner = None;
    for file in files {
        let file = file.as_ref();
        let dir = &file[..file.iter().rposition(|&c| c == b'/').unwrap_or(0)];
        if current != Some(dir) {
            current = Some(dir);
            candidates = all
                .iter()
                .copied()
                .filter(|&i| could_match_in(&rules.prefixes[i], dir))
                .collect();
            dir_owner = None;
            for (end, _) in file.iter().enumerate().filter(|&(_, &c)| c == b'/') {
                let path = &file[..=end];
                dir_owner = match dirs.get(path) {
                    Some(&owner) => owner,
                    None => {
                        let owner = last_match(rules, &mut matched, &all, path, dir_owner);
                        dirs.insert(path, owner);
                        owner
                    }
                };
            }
        }
        match last_match(rules, &mut matched, &candidates, file, dir_owner) {
            Some(i) => owned[i] += 1,
            None => unowned += 1,
        }
    }
    Counts {
        files: files.len(),
        unowned,
        rules: rules
            .linenos
            .iter()
            .enumerate()
            .map(|(i, &lineno)| RuleCount {
                lineno,
                owned: owned[i],
                matches_any: matched[i],
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(patterns: &[&str]) -> Rules {
        let list: Vec<Rule> = patterns
            .iter()
            .enumerate()
            .map(|(lineno, p)| Rule {
                pattern: p.to_string(),
                lineno,
            })
            .collect();
        Rules::new(&list)
    }

    fn owned(counts: &Counts) -> Vec<(usize, bool)> {
        counts
            .rules
            .iter()
            .map(|r| (r.owned, r.matches_any))
            .collect()
    }

    #[test]
    fn counts_files_towards_the_last_matching_rule() {
        let r = rules(&["*", "/web/", "*.css", "/web/old.css", "/missing/", "/web/"]);
        let files = [
            "README.md",
            "Makefile",
            "web/index.ts",
            "web/old.css",
            "web/new.css",
            "api/style.css",
        ];
        assert_eq!(
            owned(&count(&r, &files)),
            [
                (2, true),
                (0, true),
                (1, true),
                (0, true),
                (0, false),
                (3, true)
            ]
        );
    }

    #[test]
    fn follows_githubs_matching_rules() {
        let r = rules(&[
            "web/",
            "*.TS",
            "/docs/*",
            "/lib",
            "/missing/*.md",
            "/app/\\[id\\]/",
        ]);
        let files = [
            "web",
            "src/web/a.js",
            "x.ts",
            "y.TS",
            "Lib/y.js",
            "lib/x.js",
            "docs/a.md",
            "docs/sub/b.md",
            "app/[id]/page.tsx",
            "app/i/page.tsx",
        ];
        assert_eq!(
            owned(&count(&r, &files)),
            [
                (1, true),
                (1, true),
                (1, true),
                (1, true),
                (0, false),
                (1, true)
            ]
        );
    }
}
