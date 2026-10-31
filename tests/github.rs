// Checks every file ownership recorded from GitHub in tests/github.txt.
use codeowners_stats::codeowners::parse_codeowners;
use codeowners_stats::counter::{Rule, Rules, count};

fn owns(line: &str, path: &str) -> bool {
    let rules: Vec<Rule> = parse_codeowners(line)
        .into_iter()
        .map(|r| Rule {
            pattern: r.pattern,
            lineno: r.lineno,
        })
        .collect();
    let counts = count(&Rules::new(&rules), &[path], 1);
    counts.rules.iter().any(|r| r.owned == 1)
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
