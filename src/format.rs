//! Formatting CODEOWNERS files, like `terraform fmt` formats HCL.

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
    // The line so far, as it's formatted.
    out: String,
}

fn width(text: &str) -> usize {
    text.chars().count()
}

/// Which rules' owners line up in one column.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Align {
    /// Each group of consecutive rules: see [`format_group`].
    #[default]
    Group,
    /// Every rule in the file, like the VS Code extension formatted before it
    /// used gh-codeowners: see [`format_file`].
    File,
}

/// The CODEOWNERS file formatted with `align`, and `spacing` spaces between
/// the longest pattern and the owners. Spacing is at least 1.
pub fn format_with(text: &str, align: Align, spacing: usize) -> String {
    match align {
        Align::Group => format_group(text, spacing),
        Align::File => format_file(text, spacing),
    }
}

/// The formatted CODEOWNERS file, with each group's owners one space after
/// its longest pattern.
pub fn format(text: &str) -> String {
    format_group(text, 1)
}

/// The CODEOWNERS file formatted like `terraform fmt`: indentation and extra
/// spaces removed, and the owners and comments of each group of consecutive
/// rules lined up. Owners start `spacing` spaces after the group's longest
/// pattern, and comments one space after the longest owners.
pub fn format_group(text: &str, spacing: usize) -> String {
    let mut lines: Vec<Line> = text.split('\n').map(parse_line).collect();
    // Owners first, since they move the comments after them.
    align(&mut lines, spacing.max(1), |line| line.owners.as_deref());
    align(&mut lines, 1, |line| line.comment);
    for line in &mut lines {
        // A comment on its own line doesn't line up with anything.
        if line.pattern.is_none() {
            line.out.push_str(line.comment.unwrap_or_default());
        }
        if line.crlf {
            line.out.push('\r');
        } else if line.out.ends_with('\r') {
            // An escaped `\r` at the end of a word, which would become a line
            // ending without the whitespace after it.
            line.out = line.raw.to_string();
        }
    }
    lines
        .iter()
        .map(|line| line.out.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Append `cell` to each rule that has one, lined up in each run of
/// consecutive rules, `spacing` spaces after the longest rule with one so far.
fn align(lines: &mut [Line], spacing: usize, cell: for<'l> fn(&'l Line) -> Option<&'l str>) {
    let rule = |line: &Line| line.pattern.is_some();
    for run in lines.chunk_by_mut(|a, b| rule(a) && rule(b)) {
        let column = run
            .iter()
            .filter(|line| rule(line) && cell(line).is_some())
            .map(|line| width(&line.out))
            .max();
        let Some(column) = column else { continue };
        for line in run {
            if let Some(cell) = cell(line) {
                // Padding counts characters, like `width`.
                line.out = format!("{:<column$}{:spacing$}{cell}", line.out, "");
            }
        }
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
        out: pattern.unwrap_or_default().to_string(),
    }
}

/// The CODEOWNERS file formatted like the VS Code extension did before it
/// used gh-codeowners: the first owner of every rule in the file lines up,
/// `spacing` spaces after the longest pattern, counting its indentation.
/// Only the space between a rule's pattern and its first owner changes.
/// Indentation, the space between owners, comments, and lines without owners
/// stay as they are.
pub fn format_file(text: &str, spacing: usize) -> String {
    // Each line, and where its pattern ends and its first owner starts.
    let lines: Vec<(&str, Option<(usize, usize)>)> = text
        .split('\n')
        .map(|raw| {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let (words, _) = split_line(line, 0);
            let gap = match words.as_slice() {
                [pattern, owner, ..] => Some((pattern.span.end, owner.span.start)),
                _ => None,
            };
            (raw, gap)
        })
        .collect();
    let Some(longest) = lines
        .iter()
        .filter_map(|(raw, gap)| gap.map(|(end, _)| width(&raw[..end])))
        .max()
    else {
        return text.to_string();
    };
    let column = longest + spacing.max(1);
    lines
        .iter()
        .map(|&(raw, gap)| match gap {
            Some((end, start)) => {
                let pattern = &raw[..end];
                let padding = column - width(pattern);
                format!("{pattern}{}{}", " ".repeat(padding), &raw[start..])
            }
            None => raw.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    // Snapshots show alignment as it looks. Review changes with
    // `cargo insta review`. Whitespace you can't see, like line endings,
    // is checked with assert_eq! instead.

    /// `text` formatted, without the newline that starts it, so the text
    /// can start on the line after its quote.
    fn fmt(text: &str) -> String {
        format(text.strip_prefix('\n').unwrap_or(text))
    }

    #[test]
    fn aligns_owners_and_comments_in_each_section() {
        assert_snapshot!(fmt(r"
  # Frontend
/web/      @acme/web   # React app
/web/legacy/ @acme/web     @old-team
  /docs/ docs@example.com

# Backend
*.go @acme/go
/api/ @acme/api # REST
"), @"
        # Frontend
        /web/        @acme/web # React app
        /web/legacy/ @acme/web @old-team
        /docs/       docs@example.com

        # Backend
        *.go  @acme/go
        /api/ @acme/api # REST
        ");
    }

    #[test]
    fn a_rule_without_owners_stays_in_its_run() {
        // Its pattern is the longest, but it doesn't move the others.
        assert_snapshot!(fmt(r"
/a/ @x # one
/a/vendor/longest/
/longer/ @y # two
"), @"
        /a/      @x # one
        /a/vendor/longest/
        /longer/ @y # two
        ");
    }

    #[test]
    fn lines_up_comments_after_rules_without_owners() {
        assert_snapshot!(fmt(r"
/a/ @x # one
/vendor/ # two
"), @"
        /a/ @x   # one
        /vendor/ # two
        ");
    }

    #[test]
    fn comments_and_blank_lines_end_a_run() {
        let text = "/a/ @x\n# B\n/longer/ @y\n\n/longest/path/ @z\n";
        assert_eq!(format(text), text);
    }

    #[test]
    fn keeps_escapes_and_comments_inside_words() {
        assert_snapshot!(fmt(r"
foo/b\ ar.py   @x
\#notes   @y#z
"), @r"
        foo/b\ ar.py @x
        \#notes      @y #z
        ");
    }

    #[test]
    fn counts_characters_not_bytes() {
        // é is 2 bytes.
        assert_snapshot!(fmt(r"
/é/ @x
/ab/ @y
"), @"
        /é/  @x
        /ab/ @y
        ");
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
    fn file_lines_up_first_owners_across_the_file() {
        assert_snapshot!(format_file(r"
*               @foo
  /some-path @bar   @baz   # kept as is
/docs/**/*.md                     @foo

# a comment
/vendor/
/vendor/ # no owners
/web/@scoped/ a@example.com
", 5), @r"
        *                 @foo
          /some-path      @bar   @baz   # kept as is
        /docs/**/*.md     @foo

        # a comment
        /vendor/
        /vendor/ # no owners
        /web/@scoped/     a@example.com
        ");
    }

    #[test]
    fn spacing_is_the_spaces_after_the_longest_pattern() {
        assert_eq!(format_file("/a/ @x\n/bb/ @y\n", 1), "/a/  @x\n/bb/ @y\n");
        assert_eq!(format_file("/a/ @x\n", 3), "/a/   @x\n");
        assert_eq!(
            format_group("/a/ @x # c\n/bb/ @y\n", 3),
            "/a/    @x # c\n/bb/   @y\n"
        );
        // At least 1.
        assert_eq!(format_file("/a/ @x\n", 0), "/a/ @x\n");
        assert_eq!(format_group("/a/  @x\n", 0), "/a/ @x\n");
    }

    #[test]
    fn file_keeps_line_endings_and_files_without_owners() {
        assert_eq!(
            format_file("* @x\r\n/a/ @y\r\n", 2),
            "*    @x\r\n/a/  @y\r\n"
        );
        assert_eq!(format_file("", 4), "");
        assert_eq!(format_file("# only\n  /a/\n", 4), "# only\n  /a/\n");
    }
}
