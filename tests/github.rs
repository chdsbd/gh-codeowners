// Checks every file ownership recorded from GitHub in tests/github.txt and
// tests/github-rules.txt.
use gh_codeowners::codeowners::{CodeownersRule, find_problems, parse_codeowners};
use gh_codeowners::counter::{Rule, Rules, count};

fn rule_list(rules: &[CodeownersRule]) -> Vec<Rule> {
    rules
        .iter()
        .map(|r| Rule {
            pattern: r.pattern.clone(),
            lineno: r.lineno,
        })
        .collect()
}

fn owns(line: &str, path: &str) -> bool {
    let rules = rule_list(&parse_codeowners(line));
    let counts = count(&Rules::new(&rules), &[path]);
    let owned = counts.rules.iter().any(|r| r.owned == 1);
    // Finding a file's owner should agree with counting.
    assert_eq!(
        gh_codeowners::owners_of(&rules, &[path])[0].is_some(),
        owned,
        "{line} | {path}"
    );
    owned
}

#[test]
fn matches_like_github() {
    let fixture = include_str!("github.txt");
    let mut failures = Vec::new();
    let mut cases = 0;
    for case in fixture
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let [line, path, expected] = case.split(" | ").collect::<Vec<_>>()[..] else {
            panic!("bad case: {case}");
        };
        cases += 1;
        if owns(line, path) != (expected == "1") {
            failures.push(format!("{line} | {path} | github: {expected}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {cases} differ from GitHub:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// A CODEOWNERS file recorded in tests/github-rules.txt.
struct Recording {
    text: String,
    // One-based lines GitHub's errors API rejects.
    rejected: Vec<usize>,
    // Each path, and the one-based line of the rule GitHub says owns it, or 0.
    owners: Vec<(String, usize)>,
}

fn recordings() -> Vec<Recording> {
    let fixture = include_str!("github-rules.txt");
    fixture
        .split("\n\n")
        .filter(|block| !block.starts_with('#'))
        .map(|block| {
            let mut recording = Recording {
                text: String::new(),
                rejected: Vec::new(),
                owners: Vec::new(),
            };
            for line in block.lines() {
                if let Some(rule) = line.strip_prefix("> ").or(line.strip_prefix(">")) {
                    let rule = rule
                        .strip_suffix("<CR>")
                        .map_or(rule.to_string(), |r| format!("{r}\r"));
                    recording.text.push_str(&rule);
                    recording.text.push('\n');
                } else if let Some(rejected) = line.strip_prefix("! ") {
                    let lineno = rejected.split(' ').next().unwrap();
                    recording
                        .rejected
                        .push(lineno.parse().expect("line number"));
                } else {
                    let (path, owner) = line.rsplit_once(" | ").expect("path | line");
                    recording
                        .owners
                        .push((path.to_string(), owner.parse().expect("line number")));
                }
            }
            recording
        })
        .collect()
}

#[test]
fn picks_the_rule_github_picks() {
    let mut failures = Vec::new();
    let recordings = recordings();
    for recording in &recordings {
        let rules = parse_codeowners(&recording.text);
        let list = rule_list(&rules);
        let paths: Vec<&str> = recording.owners.iter().map(|(p, _)| p.as_str()).collect();
        let owners = gh_codeowners::owners_of(&list, &paths);
        let counts = count(&Rules::new(&list), &paths);
        for ((path, expected), owner) in recording.owners.iter().zip(owners) {
            let actual = owner.map_or(0, |i| rules[i].lineno + 1);
            if actual != *expected {
                failures.push(format!(
                    "{:?} | {path}: github line {expected}, ours {actual}",
                    recording.text
                ));
            }
        }
        // Counting should agree with GitHub too.
        for (rule, c) in rules.iter().zip(&counts.rules) {
            let expected = recording
                .owners
                .iter()
                .filter(|(_, line)| *line == rule.lineno + 1)
                .count();
            if c.owned != expected {
                failures.push(format!(
                    "{:?}: line {} owns {expected} files on github, {} counted",
                    recording.text,
                    rule.lineno + 1,
                    c.owned
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} differences from GitHub in {} files:\n{}",
        failures.len(),
        recordings.len(),
        failures.join("\n")
    );
}

#[test]
fn rejects_the_lines_github_rejects() {
    let mut failures = Vec::new();
    for recording in recordings() {
        let mut errors: Vec<usize> = find_problems(&recording.text)
            .iter()
            .filter(|p| p.error)
            .filter_map(|p| p.lineno.map(|n| n + 1))
            .collect();
        errors.dedup();
        if errors != recording.rejected {
            failures.push(format!(
                "{:?}: github rejects {:?}, ours {errors:?}",
                recording.text, recording.rejected
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
