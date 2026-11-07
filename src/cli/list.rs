use std::fmt::Write as _;
use std::process::ExitCode;

use comfy_table::presets::NOTHING;
use comfy_table::{CellAlignment, Table};
use gh_codeowners::codeowners::{CodeownersRule, LineProblem, find_problems, parse_codeowners};
use gh_codeowners::count_tracked_files;
use gh_codeowners::counter::rule_list;
use gh_codeowners::report::{Status, rule_json, to_json};

use super::check::problem_lines;
use super::load::{CodeownersFile, check_size, display_path, print_stdout, warn_if_ignored};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Only list rules that own no files
    #[arg(long)]
    unused: bool,
}

pub(crate) fn run(file: &CodeownersFile, args: &Args, json: bool) -> Result<ExitCode, String> {
    warn_if_ignored(file);
    let stats = stats_for(file)?;
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
        format_text_with_problems(&stats, &rules, &file.text)
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

pub(crate) fn stats_for(file: &CodeownersFile) -> Result<Stats, String> {
    check_size(file)?;
    let rules = parse_codeowners(&file.text);
    let counts = count_tracked_files(&file.root, &rule_list(&rules))?;
    Ok(Stats {
        display_path: display_path(file),
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
        problems: find_problems(&file.text),
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
fn format_text_with_problems(stats: &Stats, rules: &[&RuleStats], text: &str) -> String {
    let mut out = format_text(stats, rules);
    if !stats.problems.is_empty() {
        write!(
            out,
            "\n\n{}",
            problem_lines(&stats.display_path, text, &stats.problems)
        )
        .unwrap();
    }
    out
}
