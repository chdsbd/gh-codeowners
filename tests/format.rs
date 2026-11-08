use gh_codeowners::codeowners::{find_problems, parse_codeowners};
use gh_codeowners::format::format;
use proptest::prelude::*;
use proptest::sample::select;

/// CODEOWNERS-ish text, from pieces that are likely to go wrong.
fn text() -> impl Strategy<Value = String> {
    let piece = select(vec![
        "*",
        "/docs/",
        "a/b.md",
        "@acme/x",
        "@y",
        "x@example.com",
        "bad",
        "#",
        "# note",
        "#c",
        "\\ ",
        "\\#",
        "\\",
        "é",
        "語",
        " ",
        "  ",
        "\t",
        "\r",
        "\u{a0}",
    ]);
    let line = prop::collection::vec(piece, 0..8).prop_map(|pieces| pieces.concat());
    prop::collection::vec(line, 0..8).prop_map(|lines| lines.join("\n"))
}

fn without_whitespace(line: &str) -> String {
    line.chars().filter(|c| !c.is_whitespace()).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    #[test]
    fn formatting_twice_changes_nothing(text in text()) {
        let once = format(&text);
        prop_assert_eq!(format(&once), once);
    }

    #[test]
    fn formatting_changes_only_whitespace_within_lines(text in text()) {
        let formatted = format(&text);
        let before: Vec<String> = text.split('\n').map(without_whitespace).collect();
        let after: Vec<String> = formatted.split('\n').map(without_whitespace).collect();
        prop_assert_eq!(before, after);
    }

    #[test]
    fn formatting_keeps_rules_and_problems(text in text()) {
        let formatted = format(&text);
        let rules = |text: &str| -> Vec<_> {
            parse_codeowners(text)
                .into_iter()
                .map(|rule| (rule.pattern, rule.owners, rule.lineno))
                .collect()
        };
        prop_assert_eq!(rules(&formatted), rules(&text));
        let problems = |text: &str| -> Vec<_> {
            find_problems(text)
                .into_iter()
                .map(|problem| (problem.lineno, problem.message, problem.fix))
                .collect()
        };
        prop_assert_eq!(problems(&formatted), problems(&text));
    }
}
