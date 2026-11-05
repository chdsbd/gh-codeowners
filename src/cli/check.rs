use std::ops::Range;
use std::process::ExitCode;

use annotate_snippets::{AnnotationKind, Group, Level, Origin, Renderer, Snippet};
use gh_codeowners::codeowners::{LineProblem, find_problems};
use gh_codeowners::report::{Status, problem_json, to_json};

use super::list::stats_for;
use super::load::{CodeownersFile, display_path, ignored_file_warning, print_stdout, too_big};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Report rules that own no files as errors too
    #[arg(long)]
    unused: bool,
}

pub(crate) fn run(file: &CodeownersFile, args: &Args, json: bool) -> Result<ExitCode, String> {
    let problems = check_problems(file, args.unused)?;
    print_stdout(&if json {
        to_json(&problems.iter().map(problem_json).collect::<Vec<_>>())
    } else if problems.is_empty() {
        format!("{}: no problems", display_path(file))
    } else {
        problem_lines(&display_path(file), &file.text, &problems)
    })?;
    if problems.iter().any(|p| p.error) {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

/// Problems as rustc-style diagnostics, with their source lines. The styles
/// are for `print_stdout`, which drops them when stdout isn't a terminal.
pub(crate) fn problem_lines(display: &str, text: &str, problems: &[LineProblem]) -> String {
    let renderer = Renderer::styled();
    let reports: Vec<String> = problems
        .iter()
        .map(|p| {
            let level = if p.error {
                Level::ERROR
            } else {
                Level::WARNING
            };
            let report = match (p.lineno, &p.span) {
                (Some(lineno), Some(span)) => {
                    // Just the line, since rendering scans all of its source.
                    let (line, start) = line_containing(text, span);
                    // A pattern or owner can't contain " - ", since it would
                    // need an unescaped space.
                    let (title, label) = match p.message.split_once(" - ") {
                        Some((title, label)) => (title, Some(label)),
                        None => (p.message.as_str(), None),
                    };
                    let snippet = Snippet::source(line)
                        .path(display)
                        .line_start(lineno + 1)
                        .annotation(
                            AnnotationKind::Primary
                                .span(span.start - start..span.end - start)
                                .label(label),
                        );
                    [Group::with_title(level.primary_title(title)).element(snippet)]
                }
                _ => [Group::with_title(level.primary_title(&p.message))
                    .element(Origin::path(display))],
            };
            renderer.render(&report)
        })
        .collect();
    reports.join("\n\n")
}

/// The line of `text` containing `span`, without its line ending, and where
/// it starts.
fn line_containing<'a>(text: &'a str, span: &Range<usize>) -> (&'a str, usize) {
    let start = text[..span.start].rfind('\n').map_or(0, |i| i + 1);
    let end = text[span.end..]
        .find('\n')
        .map_or(text.len(), |i| span.end + i);
    let line = &text[start..end];
    (line.strip_suffix('\r').unwrap_or(line), start)
}

fn check_problems(file: &CodeownersFile, unused: bool) -> Result<Vec<LineProblem>, String> {
    let mut problems = find_problems(&file.text);
    if let Some(message) = ignored_file_warning(file, "this file") {
        problems.push(LineProblem {
            lineno: None,
            span: None,
            error: false,
            message,
        });
    }
    // oversized files are reported via find_problems.
    // We don't bother counting if the file is too large.
    if unused && !too_big(file) {
        let stats = stats_for(file)?;
        for r in stats.rules.iter().filter(|r| r.status != Status::Active) {
            // Don't warn about a line being unowned when there's already a problem with it.
            if problems.iter().any(|p| p.lineno == Some(r.rule.lineno)) {
                continue;
            }
            problems.push(LineProblem {
                lineno: Some(r.rule.lineno),
                span: Some(r.rule.pattern_span.clone()),
                error: true,
                message: format!("`{}` owns no files - {}", r.rule.pattern, r.status.note()),
            });
        }
    }
    problems.sort_by_key(|p| p.lineno);
    Ok(problems)
}
