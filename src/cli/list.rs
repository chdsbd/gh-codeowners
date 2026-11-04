use std::fmt::Write as _;
use std::num::NonZeroUsize;
use std::process::ExitCode;

use comfy_table::presets::NOTHING;
use comfy_table::{CellAlignment, Table};
use gh_codeowners::codeowners::{CodeownersRule, LineProblem, find_problems, parse_codeowners};
use gh_codeowners::count_tracked_files;
use gh_codeowners::report::{Status, rule_json, to_json};

use super::check::problem_lines;
use super::load::{Loaded, check_size, display_path, print_stdout, rule_list, warn_if_ignored};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Only list rules that own no files
    #[arg(long)]
    unused: bool,
    /// Threads to count with (default: one per CPU)
    #[arg(short = 'j', long, value_name = "n")]
    threads: Option<NonZeroUsize>,
}

pub(crate) fn run(loaded: &Loaded, args: &Args, json: bool) -> Result<ExitCode, String> {
    warn_if_ignored(loaded);
    let stats = stats_for(loaded, args.threads)?;
    let rules: Vec<&RuleStats> = stats
        .rules
        .iter()
        .filter(|r| !args.unused || r.status != Status::Active)
        .collect();
    print_stdout(&if json {
        to_json(
            &rules
                .iter()
                .map(|r| rule_json(&r.rule, r.files, r.status))
                .collect::<Vec<_>>(),
        )
    } else {
        format_text_with_problems(&stats, &rules)
    })?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) struct RuleStats {
    pub(crate) rule: CodeownersRule,
    pub(crate) files: usize,
    pub(crate) status: Status,
}

pub(crate) struct Stats {
    pub(crate) display_path: String,
    pub(crate) files: usize,
    pub(crate) unowned: usize,
    pub(crate) rules: Vec<RuleStats>,
    pub(crate) problems: Vec<LineProblem>,
}

pub(crate) fn stats_for(loaded: &Loaded, threads: Option<NonZeroUsize>) -> Result<Stats, String> {
    check_size(loaded)?;
    let rules = parse_codeowners(&loaded.text);
    let threads = threads
        .or_else(|| std::thread::available_parallelism().ok())
        .map_or(1, NonZeroUsize::get);
    let counts = count_tracked_files(&loaded.root, &rule_list(&rules), threads)
        .map_err(|e| e.to_string())?;
    Ok(Stats {
        display_path: display_path(loaded),
        files: counts.files,
        unowned: counts.unowned,
        rules: rules
            .into_iter()
            .zip(counts.rules)
            .map(|(rule, c)| RuleStats {
                rule,
                files: c.owned,
                status: Status::new(c.owned, c.matches_any),
            })
            .collect(),
        problems: find_problems(&loaded.text),
    })
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn format_text(stats: &Stats, rules: &[&RuleStats]) -> String {
    let unused = stats
        .rules
        .iter()
        .filter(|r| r.status != Status::Active)
        .count();
    let mut out = format!(
        "{}: {}, {}, {} with no owner, {} owning no files",
        stats.display_path,
        plural(stats.rules.len(), "rule"),
        plural(stats.files, "file"),
        plural(stats.unowned, "file"),
        plural(unused, "rule"),
    );
    if rules.is_empty() {
        return out;
    }
    let mut table = Table::new();
    table
        .load_style(NOTHING)
        .set_header(["line", "files", "pattern", ""]);
    for r in rules {
        table.add_row([
            (r.rule.lineno + 1).to_string(),
            r.files.to_string(),
            r.rule.pattern.clone(),
            r.status.note().to_string(),
        ]);
    }
    for (i, column) in table.column_iter_mut().enumerate() {
        // Two spaces between columns, and none before the first.
        column.set_padding((0, if i < 3 { 2 } else { 0 }));
        if i < 2 {
            column.set_cell_alignment(CellAlignment::Right);
        }
    }
    out.push('\n');
    for line in table.lines() {
        write!(out, "\n{}", line.trim_end()).unwrap();
    }
    out
}

/// The counts, then any problems, which explain rules that match nothing
/// and lines GitHub skips, which aren't counted at all.
fn format_text_with_problems(stats: &Stats, rules: &[&RuleStats]) -> String {
    let mut out = format_text(stats, rules);
    if !stats.problems.is_empty() {
        write!(
            out,
            "\n\n{}",
            problem_lines(&stats.display_path, &stats.problems)
        )
        .unwrap();
    }
    out
}
