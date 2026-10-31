// For each case in a fixture, prints "<gh-codeowners> <codeowners-rs>":
// 1 if the line owns the file, 0 if not, E if the line is rejected, P if the
// library panics.
use gh_codeowners::codeowners::parse_codeowners;
use gh_codeowners::counter::{Rule, Rules, count};
use gh_codeowners::pattern::can_match;

fn gh_codeowners(line: &str, path: &str) -> &'static str {
    let rules: Vec<Rule> = parse_codeowners(line)
        .into_iter()
        .map(|r| Rule {
            pattern: r.pattern,
            lineno: r.lineno,
        })
        .collect();
    let Some(rule) = rules.first() else {
        return "E";
    };
    if !can_match(&rule.pattern) {
        return "E";
    }
    if count(&Rules::new(&rules), &[path], 1).rules[0].owned == 1 {
        "1"
    } else {
        "0"
    }
}

fn codeowners_rs(line: &str, path: &str) -> &'static str {
    let parsed = codeowners_rs::parse(&format!("{line}\n"));
    if !parsed.errors.is_empty() || parsed.rules.is_empty() {
        return "E";
    }
    if parsed.into_ruleset().matching_rule(path).is_some() {
        "1"
    } else {
        "0"
    }
}

fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let fixture = std::fs::read_to_string(std::env::args().nth(1).unwrap()).unwrap();
    for case in fixture
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let fields: Vec<&str> = case.split(" | ").collect();
        let (line, path) = (fields[0], fields[1]);
        let ours = std::panic::catch_unwind(|| gh_codeowners(line, path)).unwrap_or("P");
        let theirs = std::panic::catch_unwind(|| codeowners_rs(line, path)).unwrap_or("P");
        println!("{ours} {theirs}");
    }
}
