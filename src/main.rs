use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use comfy_table::presets::NOTHING;
use comfy_table::{CellAlignment, Table};

use gh_codeowners::codeowners::{
    CodeownersRule, LineProblem, MAX_CODEOWNERS_BYTES, find_codeowners, find_problems,
    find_repo_root, ignored_codeowners, parse_codeowners, repo_root_for_codeowners,
};
use gh_codeowners::counter::Rule;
use gh_codeowners::report::{Status, ownership_json, problem_json, rule_json, to_json};
use gh_codeowners::{count_tracked_files, owners_of};

/// Work with GitHub CODEOWNERS files
#[derive(Parser)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// CODEOWNERS file, or - for stdin (default: the one GitHub would use)
    #[arg(long, global = true, value_name = "path")]
    codeowners: Option<PathBuf>,
    /// Repository root (default: the repository containing the current directory)
    #[arg(long, global = true, value_name = "dir")]
    root: Option<PathBuf>,
    /// Output JSON
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Check CODEOWNERS for lines GitHub rejects
    ///
    /// Also warns about lines that likely don't do what they look like. Exits
    /// with 1 if CODEOWNERS has errors, and 2 if the command fails.
    Check {
        /// Report rules that own no files as errors too
        #[arg(long)]
        unused: bool,
        /// Threads to count with, for --unused (default: one per CPU)
        #[arg(short = 'j', long, value_name = "n")]
        threads: Option<NonZeroUsize>,
    },
    /// List each rule with how many files it owns
    ///
    /// A file counts towards the last rule that matches it, like on GitHub, so
    /// a rule can match files and still own none of them. Files are the ones in
    /// git's index. Any problems `check` reports follow the counts.
    List {
        /// Only list rules that own no files
        #[arg(long)]
        unused: bool,
        /// Threads to count with (default: one per CPU)
        #[arg(short = 'j', long, value_name = "n")]
        threads: Option<NonZeroUsize>,
    },
    /// Show the rule that owns each path
    ///
    /// Like GitHub shows when you hover over a file's code owners icon. Paths
    /// needn't be tracked, and relative paths are relative to the current
    /// directory.
    View {
        #[arg(required = true, value_name = "path")]
        paths: Vec<String>,
    },
}

struct RuleStats {
    rule: CodeownersRule,
    files: usize,
    status: Status,
}

struct Stats {
    display_path: String,
    files: usize,
    unowned: usize,
    rules: Vec<RuleStats>,
    problems: Vec<LineProblem>,
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

struct Loaded {
    // "-" for stdin.
    codeowners: PathBuf,
    root: PathBuf,
    text: String,
    // A CODEOWNERS file GitHub ignores, because the one it uses has no rules.
    ignored: Option<PathBuf>,
}

fn is_stdin(path: &Path) -> bool {
    path == Path::new("-")
}

fn load(cli: &Cli) -> Result<Loaded, String> {
    // The repository containing the current directory, or else the directory.
    let current_root = || {
        let current_dir = std::env::current_dir().unwrap_or_default();
        find_repo_root(&current_dir).unwrap_or(current_dir)
    };
    let codeowners = match &cli.codeowners {
        Some(path) => path.clone(),
        None => {
            let root = cli.root.clone().unwrap_or_else(current_root);
            find_codeowners(&root)
                .map(|path| {
                    path.strip_prefix(".")
                        .map(Path::to_path_buf)
                        .unwrap_or(path)
                })
                .ok_or_else(|| format!("No CODEOWNERS file found in {}", root.display()))?
        }
    };
    let root = match &cli.root {
        Some(root) => root.clone(),
        None if is_stdin(&codeowners) => current_root(),
        None => repo_root_for_codeowners(&absolute(&codeowners)),
    };
    let text = if is_stdin(&codeowners) {
        std::io::read_to_string(std::io::stdin()).map_err(|e| format!("stdin: {e}"))?
    } else {
        std::fs::read_to_string(&codeowners)
            .map_err(|e| format!("{}: {e}", codeowners.display()))?
    };
    let ignored = match cli.codeowners {
        Some(_) => None,
        None => ignored_codeowners(&root, &text),
    };
    Ok(Loaded {
        codeowners,
        root,
        text,
        ignored,
    })
}

/// The CODEOWNERS file's path relative to the root, for messages.
fn display_path(loaded: &Loaded) -> String {
    if is_stdin(&loaded.codeowners) {
        return "<stdin>".to_string();
    }
    repo_display(&loaded.root, &loaded.codeowners)
}

/// `path` relative to `root`, for messages. Inside the repository, it's a
/// repository path, written with `/` like the paths of the files it owns.
fn repo_display(root: &Path, path: &Path) -> String {
    let path = absolute(path);
    match path.strip_prefix(absolute(root)) {
        Ok(relative) => relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    }
}

/// A warning that GitHub ignores another CODEOWNERS file's rules, about
/// `subject`, the file GitHub uses.
fn ignored_warning(loaded: &Loaded, subject: &str) -> Option<String> {
    let ignored = loaded.ignored.as_ref()?;
    let ignored = repo_display(&loaded.root, ignored);
    Some(format!(
        "{subject} has no rules, but GitHub uses it instead of {ignored}, so no file has an owner"
    ))
}

fn too_big(loaded: &Loaded) -> bool {
    loaded.text.len() as u64 > MAX_CODEOWNERS_BYTES
}

fn check_size(loaded: &Loaded) -> Result<(), String> {
    if too_big(loaded) {
        return Err(format!(
            "{} is {} bytes, over GitHub's limit of {MAX_CODEOWNERS_BYTES}, so GitHub ignores it",
            display_path(loaded),
            loaded.text.len()
        ));
    }
    Ok(())
}

fn rule_list(rules: &[CodeownersRule]) -> Vec<Rule> {
    rules
        .iter()
        .map(|r| Rule {
            pattern: r.pattern.clone(),
            lineno: r.lineno,
        })
        .collect()
}

fn stats_for(loaded: &Loaded, threads: Option<NonZeroUsize>) -> Result<Stats, String> {
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

/// `path` relative to `root`, with `/` separators. Relative paths are
/// relative to the current directory.
fn repo_path(root: &Path, path: &str) -> Result<String, String> {
    let given = normalize(&absolute(Path::new(path)));
    let relative = match given.strip_prefix(normalize(&absolute(root))) {
        Ok(relative) => relative.to_path_buf(),
        // One of the paths may go through a symlink, like /var on macOS.
        Err(_) => real_path(&given)
            .zip(std::fs::canonicalize(root).ok())
            .and_then(|(given, root)| given.strip_prefix(root).ok().map(Path::to_path_buf))
            .ok_or_else(|| format!("{path} isn't in {}", root.display()))?,
    };
    let mut relative = relative.to_string_lossy().into_owned();
    if cfg!(windows) {
        relative = relative.replace('\\', "/");
    }
    // Directories end in a slash, so rules for directories match them.
    if !relative.is_empty() && given.is_dir() {
        relative.push('/');
    }
    Ok(relative)
}

/// `path` without `.` and `..` components, without looking at the file
/// system.
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normal.pop();
            }
            component => normal.push(component),
        }
    }
    normal
}

/// `path` with symlinks resolved, even if the file doesn't exist yet.
fn real_path(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path)
        .ok()
        .or_else(|| Some(real_path(path.parent()?)?.join(path.file_name()?)))
}

/// Each path to view, and the rule that owns it.
fn ownership(
    loaded: &Loaded,
    paths: &[String],
) -> Result<Vec<(String, Option<CodeownersRule>)>, String> {
    check_size(loaded)?;
    let rules = parse_codeowners(&loaded.text);
    let paths = paths
        .iter()
        .map(|path| repo_path(&loaded.root, path))
        .collect::<Result<Vec<_>, _>>()?;
    let owners = owners_of(&rule_list(&rules), &paths);
    Ok(paths
        .into_iter()
        .zip(owners)
        .map(|(path, rule)| (path, rule.map(|i| rules[i].clone())))
        .collect())
}

fn format_ownership(loaded: &Loaded, files: &[(String, Option<CodeownersRule>)]) -> String {
    let display = display_path(loaded);
    let lines: Vec<String> = files
        .iter()
        .map(|(path, rule)| match rule {
            None => format!("{path}: no rule in {display} matches"),
            Some(rule) if rule.owners.is_empty() => format!(
                "{path}: no owners, from {display}:{} {}",
                rule.lineno + 1,
                rule.pattern
            ),
            Some(rule) => format!(
                "{path}: {}, from {display}:{} {}",
                rule.owners.join(" "),
                rule.lineno + 1,
                rule.pattern
            ),
        })
        .collect();
    lines.join("\n")
}

/// Each problem on its own line, prefixed with the file and line number.
fn problem_lines(display: &str, problems: &[LineProblem]) -> String {
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

/// Everything `check` reports: the lines GitHub rejects or that likely don't
/// do what they look like, and with --unused, rules that own no files.
fn check_problems(
    loaded: &Loaded,
    unused: bool,
    threads: Option<NonZeroUsize>,
) -> Result<Vec<LineProblem>, String> {
    let mut problems = find_problems(&loaded.text);
    if let Some(message) = ignored_warning(loaded, "this file") {
        problems.push(LineProblem {
            lineno: None,
            error: false,
            message,
        });
    }
    // A file too big for GitHub already has an error, and nothing to count.
    if unused && !too_big(loaded) {
        let stats = stats_for(loaded, threads)?;
        for r in stats.rules.iter().filter(|r| r.status != Status::Active) {
            // A line already reported likely explains why it owns nothing.
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

/// Print a line to stdout. A reader that stops early, like `| head`, closes
/// the pipe; that isn't an error, so the exit code still reflects the run.
fn print_stdout(text: &str) -> Result<(), String> {
    match writeln!(io::stdout().lock(), "{text}") {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            Err(format!("Couldn't write output: {error}"))
        }
        _ => Ok(()),
    }
}

fn run(cli: &Cli) -> Result<ExitCode, String> {
    let loaded = load(cli)?;
    // On stderr, so it doesn't mix with output meant for other programs.
    // `check` reports it as a problem instead.
    if !matches!(cli.command, Command::Check { .. })
        && let Some(message) = ignored_warning(&loaded, &display_path(&loaded))
    {
        eprintln!("{}: warning: {message}", program_name());
    }
    match &cli.command {
        Command::Check { unused, threads } => {
            let problems = check_problems(&loaded, *unused, *threads)?;
            print_stdout(&if cli.json {
                to_json(&problems.iter().map(problem_json).collect::<Vec<_>>())
            } else if problems.is_empty() {
                format!("{}: no problems", display_path(&loaded))
            } else {
                problem_lines(&display_path(&loaded), &problems)
            })?;
            if problems.iter().any(|p| p.error) {
                return Ok(ExitCode::from(1));
            }
        }
        Command::View { paths } => {
            let files = ownership(&loaded, paths)?;
            print_stdout(&if cli.json {
                to_json(
                    &files
                        .iter()
                        .map(|(path, rule)| ownership_json(path, rule.as_ref()))
                        .collect::<Vec<_>>(),
                )
            } else {
                format_ownership(&loaded, &files)
            })?;
        }
        Command::List { unused, threads } => {
            let stats = stats_for(&loaded, *threads)?;
            let rules: Vec<&RuleStats> = stats
                .rules
                .iter()
                .filter(|r| !unused || r.status != Status::Active)
                .collect();
            print_stdout(&if cli.json {
                to_json(
                    &rules
                        .iter()
                        .map(|r| rule_json(&r.rule, r.files, r.status))
                        .collect::<Vec<_>>(),
                )
            } else {
                format_text_with_problems(&stats, &rules)
            })?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// How the user ran this: `gh codeowners` when gh runs it as an extension,
/// which gh signals with `GH_EXTENSION`.
fn program_name() -> &'static str {
    if std::env::var_os("GH_EXTENSION").is_some() {
        "gh codeowners"
    } else {
        "gh-codeowners"
    }
}

fn main() -> ExitCode {
    let matches = Cli::command().bin_name(program_name()).get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    run(&cli).unwrap_or_else(|message| {
        eprintln!("{}: {message}", program_name());
        ExitCode::from(2)
    })
}
