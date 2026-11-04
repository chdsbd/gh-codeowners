use std::num::NonZeroUsize;
use std::process::ExitCode;

use gh_codeowners::codeowners::{LineProblem, find_problems};
use gh_codeowners::report::{Status, problem_json, to_json};

use super::list::stats_for;
use super::load::{Loaded, display_path, ignored_file_warning, print_stdout, too_big};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Report rules that own no files as errors too
    #[arg(long)]
    unused: bool,
    /// Threads to count with, for --unused (default: one per CPU)
    #[arg(short = 'j', long, value_name = "n")]
    threads: Option<NonZeroUsize>,
}

pub(crate) fn run(loaded: &Loaded, args: &Args, json: bool) -> Result<ExitCode, String> {
    let problems = check_problems(loaded, args.unused, args.threads)?;
    print_stdout(&if json {
        to_json(&problems.iter().map(problem_json).collect::<Vec<_>>())
    } else if problems.is_empty() {
        format!("{}: no problems", display_path(loaded))
    } else {
        problem_lines(&display_path(loaded), &problems)
    })?;
    if problems.iter().any(|p| p.error) {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn problem_lines(display: &str, problems: &[LineProblem]) -> String {
    let severity = |p: &LineProblem| if p.error { "error" } else { "warning" };
    let lines: Vec<String> = problems
        .iter()
        .map(|p| match p.lineno {
            Some(n) => format!("{display}:{}: {}: {}", n + 1, severity(p), p.message),
            None => format!("{display}: {}: {}", severity(p), p.message),
        })
        .collect();
    lines.join("\n")
}

fn check_problems(
    loaded: &Loaded,
    unused: bool,
    threads: Option<NonZeroUsize>,
) -> Result<Vec<LineProblem>, String> {
    let mut problems = find_problems(&loaded.text);
    if let Some(message) = ignored_file_warning(loaded, "this file") {
        problems.push(LineProblem {
            lineno: None,
            error: false,
            message,
        });
    }
    if unused && 
    // oversized files are reported via find_problems.
    // We don't bother counting if the file is too large.
    !too_big(loaded) {
        let stats = stats_for(loaded, threads)?;
        for r in stats.rules.iter().filter(|r| r.status != Status::Active) {
            // Don't warn about a line being unowned when there's already a problem with it.
            if problems.iter().any(|p| p.lineno == Some(r.rule.lineno)) {
                continue;
            }
            problems.push(LineProblem {
                lineno: Some(r.rule.lineno),
                error: true,
                message: format!("`{}` owns no files - {}", r.rule.pattern, r.status.note()),
            });
        }
    }
    problems.sort_by_key(|p| p.lineno);
    Ok(problems)
}
