// Checks the counter, which skips most rules for each path, against testing
// every rule's regex against every file and directory.
use gh_codeowners::counter::{Rule, Rules, count};
use gh_codeowners::pattern::compile;
use proptest::prelude::*;
use proptest::sample::select;

static NAMES: [&str; 12] = [
    "a", "b", "docs", "Docs", "src", "lib", "x.md", "y.MD", "z.ts", "[id]", "é", "a.b.c",
];
static SEGMENTS: [&str; 17] = [
    "a", "b", "docs", "Docs", "src", "*", "**", "*.md", "*.ts", "x*", "?", "[id]", "\\[id\\]", "é",
    "\\*", "a.b.c", "lib",
];

fn pattern() -> impl Strategy<Value = String> {
    (
        prop::collection::vec(select(&SEGMENTS[..]), 1..=3),
        prop::bool::weighted(1.0 / 3.0),
        prop::bool::weighted(1.0 / 4.0),
        prop::bool::weighted(1.0 / 8.0),
    )
        .prop_map(|(segments, leading_slash, trailing_slash, negated)| {
            let mut pattern = segments.join("/");
            if leading_slash {
                pattern.insert(0, '/');
            }
            if trailing_slash {
                pattern.push('/');
            }
            if negated {
                pattern.insert(0, '!');
            }
            pattern
        })
}

fn patterns() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(pattern(), 1..=8)
}

fn files() -> impl Strategy<Value = Vec<String>> {
    let file = prop::collection::vec(select(&NAMES[..]), 1..=4).prop_map(|names| names.join("/"));
    prop::collection::vec(file, 1..=40).prop_map(|mut files| {
        // Like git's index: sorted, unique, and no file is also a directory.
        files.sort();
        files.dedup();
        let all = files.clone();
        files.retain(|f| !all.iter().any(|g| g.starts_with(&format!("{f}/"))));
        files
    })
}

fn rule_list(patterns: &[String]) -> Vec<Rule> {
    patterns
        .iter()
        .enumerate()
        .map(|(lineno, pattern)| Rule {
            pattern: pattern.clone(),
            lineno,
        })
        .collect()
}

/// For each rule, the files it owns and whether it matches any path.
fn count_naively(patterns: &[String], files: &[String]) -> Vec<(usize, bool)> {
    let regexes: Vec<_> = patterns.iter().map(|p| compile(p)).collect();
    let mut result = vec![(0, false); patterns.len()];
    for file in files {
        // The file and each directory containing it.
        let mut paths: Vec<String> = file
            .match_indices('/')
            .map(|(i, _)| file[..=i].to_string())
            .collect();
        paths.push(file.clone());
        let mut owner = None;
        for (i, regex) in regexes.iter().enumerate() {
            let Some(regex) = regex else { continue };
            if paths.iter().any(|p| regex.is_match(p.as_bytes())) {
                result[i].1 = true;
                owner = Some(i);
            }
        }
        if let Some(i) = owner {
            result[i].0 += 1;
        }
    }
    result
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    #[test]
    fn agrees_with_testing_every_rule(patterns in patterns(), files in files()) {
        let rules = Rules::new(&rule_list(&patterns));
        let actual: Vec<(usize, bool)> = count(&rules, &files)
            .rules
            .iter()
            .map(|r| (r.owned, r.matches_any))
            .collect();
        prop_assert_eq!(actual, count_naively(&patterns, &files));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    #[test]
    fn owners_of_agrees_with_counting(patterns in patterns(), files in files()) {
        let rules = rule_list(&patterns);
        let compiled = Rules::new(&rules);
        let owners = gh_codeowners::owners_of(&rules, &files);
        for (file, owner) in files.iter().zip(owners) {
            let counted = count(&compiled, &[file])
                .rules
                .iter()
                .position(|rule| rule.owned == 1);
            prop_assert_eq!(owner, counted, "file: {}", file);
        }
    }
}
