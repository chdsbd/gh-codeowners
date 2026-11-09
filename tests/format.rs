use gh_codeowners::codeowners::{find_problems, parse_codeowners};
use gh_codeowners::format::{format, format_legacy};
use insta::{assert_snapshot, glob};
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

/// Each file in tests/format, formatted. Review changes with
/// `cargo insta review`.
#[test]
fn formats_fixtures() {
    glob!("format/*.codeowners", |path| {
        let text = std::fs::read_to_string(path).unwrap();
        let formatted = format(&text);
        assert_eq!(
            format(&formatted),
            formatted,
            "formatting twice changes nothing"
        );
        assert_snapshot!(formatted);
    });
}

/// The VS Code extension's old formatter, which `format_legacy` keeps for
/// those who prefer it, formatted its test file like this.
#[test]
fn legacy_matches_the_vscode_extensions_old_formatter() {
    let text = std::fs::read_to_string("tests/format/vscode-extension.codeowners").unwrap();
    assert_eq!(
        format_legacy(&text, 4),
        "\
*                                    @foo
  /some-path                         @bar
another-path/but-loooooooooonger     @baz

# a comment

    # an indented comment
/docs/**/*.md                        @foo @bar
# /docs/**/*.rst                     @foo @bar
"
    );
    let comments =
        std::fs::read_to_string("tests/format/vscode-extension-comments.codeowners").unwrap();
    assert_eq!(format_legacy(&comments, 4), comments);
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
    fn legacy_formatting_twice_changes_nothing(text in text(), offset in 0..6usize) {
        let once = format_legacy(&text, offset);
        prop_assert_eq!(format_legacy(&once, offset), once);
    }

    #[test]
    fn legacy_formatting_changes_only_whitespace_within_lines(text in text(), offset in 0..6usize) {
        let formatted = format_legacy(&text, offset);
        let before: Vec<String> = text.split('\n').map(without_whitespace).collect();
        let after: Vec<String> = formatted.split('\n').map(without_whitespace).collect();
        prop_assert_eq!(before, after);
    }

    #[test]
    fn formatting_keeps_rules_and_problems(text in text(), legacy in any::<bool>()) {
        let formatted = if legacy { format_legacy(&text, 4) } else { format(&text) };
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
