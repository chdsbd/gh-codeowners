use gh_codeowners::codeowners::{find_problems, parse_codeowners};
use gh_codeowners::format::{format, format_file, format_group};
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

/// The VS Code extension's old formatter, which `format_file` keeps for
/// those who prefer it, formatted its test file like this with its default
/// offset of 4, which made 5 spaces.
#[test]
fn file_matches_the_vscode_extensions_old_formatter() {
    let text = std::fs::read_to_string("tests/format/vscode-extension.codeowners").unwrap();
    assert_eq!(
        format_file(&text, 5),
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
    assert_eq!(format_file(&comments, 5), comments);
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
    fn formatting_with_spacing_twice_changes_nothing(text in text(), spacing in 1..6usize) {
        let once = format_file(&text, spacing);
        prop_assert_eq!(format_file(&once, spacing), once);
        let once = format_group(&text, spacing);
        prop_assert_eq!(format_group(&once, spacing), once);
    }

    #[test]
    fn file_formatting_changes_only_whitespace_within_lines(text in text(), spacing in 1..6usize) {
        let formatted = format_file(&text, spacing);
        let before: Vec<String> = text.split('\n').map(without_whitespace).collect();
        let after: Vec<String> = formatted.split('\n').map(without_whitespace).collect();
        prop_assert_eq!(before, after);
    }

    #[test]
    fn formatting_keeps_rules_and_problems(
        text in text(),
        file in any::<bool>(),
        spacing in 1..6usize,
    ) {
        let formatted = if file {
            format_file(&text, spacing)
        } else {
            format_group(&text, spacing)
        };
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
