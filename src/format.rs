//! Formatting CODEOWNERS files, like `terraform fmt` formats HCL.
//!
//! Formatting changes only whitespace, and never adds or removes lines:
//!
//! - Lines have no indentation or trailing whitespace, and one space
//!   between words.
//! - In each run of consecutive rules, the owners line up, one space after
//!   the longest pattern with owners. Only a blank line or a comment on its
//!   own line ends a run, so each section lines up on its own. A rule with
//!   no owners stays in its section, without moving the others.
//! - Likewise, comments after rules line up in each run.
//!
//! There are no options, so every formatted file looks the same.

use crate::codeowners::split_line;

/// A line split into the parts that line up with the lines around it.
struct Line<'a> {
    // As written, without its `\n`.
    raw: &'a str,
    pattern: Option<&'a str>,
    // The owners, with one space between them.
    owners: Option<String>,
    // From its `#`, without trailing whitespace.
    comment: Option<&'a str>,
    // Whether the line ends with `\r\n`, which formatting keeps.
    crlf: bool,
}

fn width(text: &str) -> usize {
    text.chars().count()
}

/// The formatted CODEOWNERS file.
pub fn format(text: &str) -> String {
    let lines: Vec<Line> = text.split('\n').map(parse_line).collect();
    let mut out: Vec<String> = lines
        .iter()
        .map(|line| line.pattern.unwrap_or_default().to_string())
        .collect();
    // Owners first, since they move the comments after them.
    align(&mut out, &lines, |line| line.owners.as_deref());
    align(&mut out, &lines, |line| line.comment);
    for (formatted, line) in out.iter_mut().zip(&lines) {
        // A comment on its own line doesn't line up with anything.
        if line.pattern.is_none() {
            formatted.push_str(line.comment.unwrap_or_default());
        }
        if line.crlf {
            formatted.push('\r');
        } else if formatted.ends_with('\r') {
            // An escaped `\r` at the end of a word, which would become a line
            // ending without the whitespace after it.
            *formatted = line.raw.to_string();
        }
    }
    out.join("\n")
}

/// Append `cell` to each rule that has one, lined up in each run of
/// consecutive rules, one space after the longest rule with one so far.
fn align<'a>(
    out: &mut [String],
    lines: &'a [Line<'a>],
    cell: impl Fn(&'a Line<'a>) -> Option<&'a str>,
) {
    let mut start = 0;
    while start < lines.len() {
        let len = lines[start..]
            .iter()
            .take_while(|line| line.pattern.is_some())
            .count();
        if len == 0 {
            start += 1;
            continue;
        }
        let run = start..start + len;
        let column = out[run.clone()]
            .iter()
            .zip(&lines[run.clone()])
            .filter(|(_, line)| cell(line).is_some())
            .map(|(formatted, _)| width(formatted))
            .max()
            .unwrap_or(0)
            + 1;
        for (formatted, line) in out[run].iter_mut().zip(&lines[start..]) {
            if let Some(cell) = cell(line) {
                let padding = column - width(formatted);
                formatted.extend(std::iter::repeat_n(' ', padding));
                formatted.push_str(cell);
            }
        }
        start += len;
    }
}

fn parse_line(raw: &str) -> Line<'_> {
    let (line, crlf) = match raw.strip_suffix('\r') {
        Some(line) => (line, true),
        None => (raw, false),
    };
    let (words, comment) = split_line(line, 0);
    let mut words = words.into_iter().map(|word| word.text);
    let pattern = words.next();
    let owners: Vec<&str> = words.collect();
    Line {
        raw,
        pattern,
        owners: (!owners.is_empty()).then(|| owners.join(" ")),
        comment: comment.map(str::trim_end),
        crlf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligns_owners_and_comments_in_each_section() {
        let text = "\
  # Frontend
/web/      @acme/web   # React app
/web/legacy/ @acme/web     @old-team
  /docs/ docs@example.com

# Backend
*.go @acme/go
/api/ @acme/api # REST
";
        assert_eq!(
            format(text),
            "\
# Frontend
/web/        @acme/web # React app
/web/legacy/ @acme/web @old-team
/docs/       docs@example.com

# Backend
*.go  @acme/go
/api/ @acme/api # REST
"
        );
    }

    #[test]
    fn a_rule_without_owners_stays_in_its_run() {
        // Its pattern is the longest, but it doesn't move the others.
        let text = "/a/ @x # one\n/a/vendor/longest/\n/longer/ @y # two\n";
        assert_eq!(
            format(text),
            "/a/      @x # one\n/a/vendor/longest/\n/longer/ @y # two\n"
        );
    }

    #[test]
    fn lines_up_comments_after_rules_without_owners() {
        let text = "/a/ @x # one\n/vendor/ # two\n";
        assert_eq!(format(text), "/a/ @x   # one\n/vendor/ # two\n");
    }

    #[test]
    fn comments_and_blank_lines_end_a_run() {
        let text = "/a/ @x\n# B\n/longer/ @y\n\n/longest/path/ @z\n";
        assert_eq!(format(text), text);
    }

    #[test]
    fn keeps_escapes_and_comments_inside_words() {
        let text = "foo/b\\ ar.py   @x\n\\#notes   @y#z\n";
        assert_eq!(format(text), "foo/b\\ ar.py @x\n\\#notes      @y #z\n");
    }

    #[test]
    fn keeps_line_endings() {
        assert_eq!(format("*  @x\r\n\r\n/a/ @y"), "* @x\r\n\r\n/a/ @y");
        assert_eq!(format(""), "");
        assert_eq!(format("   \n"), "\n");
    }

    #[test]
    fn keeps_a_line_whose_last_word_ends_with_an_escaped_cr() {
        assert_eq!(format("x\\\r  \n"), "x\\\r  \n");
    }

    #[test]
    fn counts_characters_not_bytes() {
        // é is 2 bytes.
        assert_eq!(format("/é/ @x\n/ab/ @y\n"), "/é/  @x\n/ab/ @y\n");
    }
}
