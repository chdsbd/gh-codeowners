use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gh_codeowners::codeowners::{
    CodeownersRule, LineProblem, MAX_CODEOWNERS_BYTES, find_codeowners, find_problems,
    find_repo_root, ignored_codeowners, parse_codeowners, repo_root_for_codeowners,
};
use gh_codeowners::counter::Rule;
use gh_codeowners::report::{Status, ownership_json, problem_json, rule_json, to_json};
use gh_codeowners::{count_tracked_files, owners_of};

// Help is laid out like gh's own, so `gh codeowners` reads like any other
// gh command.
const HELP: &str = "Work with GitHub CODEOWNERS files.

USAGE
  gh codeowners <command> [flags]

COMMANDS
  check:  Check CODEOWNERS for lines GitHub rejects
  list:   List each rule with how many files it owns
  view:   Show the rule that owns each path

FLAGS
  --help      Show help for command
  --version   Show gh-codeowners version

EXAMPLES
  $ gh codeowners list --unused
  $ gh codeowners view src/app.ts
  $ gh codeowners check

LEARN MORE
  Use `gh codeowners <command> --help` for more information about a command.";

const INHERITED_FLAGS: &str = "INHERITED FLAGS
      --codeowners <path>  CODEOWNERS file, or - for stdin (default: the one
                           GitHub would use)
      --help               Show help for command
      --root <dir>         Repository root (default: the git repository
                           containing the current directory)";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Check,
    List,
    View,
}

impl Command {
    fn from_name(name: &str) -> Option<Command> {
        match name {
            "check" => Some(Command::Check),
            "list" => Some(Command::List),
            "view" => Some(Command::View),
            _ => None,
        }
    }

    fn help(self) -> String {
        let (about, usage) = match self {
            Command::Check => (
                "Check CODEOWNERS for lines GitHub rejects, and lines that likely don't
do what they look like, like patterns that match nothing.

With --unused, rules that own no files are errors too.

Exit codes:
  0: No errors
  1: CODEOWNERS has errors
  2: The command failed

USAGE
  gh codeowners check [flags]

FLAGS
  -j, --threads <n>   Threads to count with, for --unused (default: one per CPU)
      --json          Output JSON
      --unused        Report rules that own no files as errors",
                "EXAMPLES
  $ gh codeowners check
  $ gh codeowners check --unused",
            ),
            Command::List => (
                "List each CODEOWNERS rule with how many files it owns.

A file counts towards the last rule that matches it, like on GitHub, so a
rule can match files and still own none of them. Files are the ones in
git's index. Any problems `gh codeowners check` reports follow the counts.

USAGE
  gh codeowners list [flags]

FLAGS
  -j, --threads <n>   Threads to count with (default: one per CPU)
      --json          Output JSON
      --unused        Only list rules that own no files",
                "EXAMPLES
  $ gh codeowners list
  $ gh codeowners list --unused --json",
            ),
            Command::View => (
                "Show the rule that owns each path, like GitHub shows when you hover over
a file's code owners icon. Paths needn't be tracked, and relative paths
are relative to the current directory.

USAGE
  gh codeowners view <path>... [flags]

FLAGS
      --json          Output JSON",
                "EXAMPLES
  $ gh codeowners view src/app.ts
  $ gh codeowners view src/components --json",
            ),
        };
        format!(
            "{about}\n\n{INHERITED_FLAGS}\n\n{usage}\n\nLEARN MORE\n  Use `gh codeowners <command> --help` for more information about a command."
        )
    }
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

struct Args {
    command: Command,
    root: Option<PathBuf>,
    codeowners: Option<PathBuf>,
    paths: Vec<String>,
    unused: bool,
    json: bool,
    threads: Option<usize>,
}

enum Parsed {
    Run(Args),
    Exit(ExitCode),
}

/// A usage error, and the help to show with it.
struct UsageError {
    message: String,
    help: String,
}

fn usage_error(message: impl Into<String>, help: String) -> UsageError {
    UsageError {
        message: message.into(),
        help,
    }
}

/// Where a flag from before subcommands went.
fn moved_flag(flag: &str) -> Option<&'static str> {
    match flag {
        "--owner-of" => Some("gh codeowners view <path>"),
        "--errors" | "--check" => Some("gh codeowners check"),
        _ => None,
    }
}

fn unknown_option(arg: &str, help: String) -> UsageError {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    match moved_flag(name) {
        Some(command) => usage_error(format!("Unknown option '{name}', use `{command}`"), help),
        None => usage_error(format!("Unknown option '{arg}'"), help),
    }
}

fn parse_args(argv: Vec<String>) -> Result<Parsed, UsageError> {
    let mut iter = argv.into_iter();
    let command = match iter.next().as_deref() {
        None | Some("-h" | "--help") => {
            print_stdout(HELP).map_err(|message| usage_error(message, String::new()))?;
            return Ok(Parsed::Exit(ExitCode::SUCCESS));
        }
        Some("--version") => {
            print_stdout(env!("CARGO_PKG_VERSION"))
                .map_err(|message| usage_error(message, String::new()))?;
            return Ok(Parsed::Exit(ExitCode::SUCCESS));
        }
        Some(arg) if arg.starts_with('-') => return Err(unknown_option(arg, HELP.to_string())),
        Some(name) => Command::from_name(name)
            .ok_or_else(|| usage_error(format!("Unknown command '{name}'"), HELP.to_string()))?,
    };
    let help = command.help();
    let mut args = Args {
        command,
        root: None,
        codeowners: None,
        paths: Vec::new(),
        unused: false,
        json: false,
        threads: None,
    };
    let counts = matches!(command, Command::Check | Command::List);
    while let Some(arg) = iter.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => {
                (name.to_string(), Some(value.to_string()))
            }
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| {
            inline.clone().or_else(|| iter.next()).ok_or_else(|| {
                usage_error(
                    format!("Option '{name} <value>' argument missing"),
                    help.clone(),
                )
            })
        };
        match name.as_str() {
            "--codeowners" => args.codeowners = Some(PathBuf::from(value("--codeowners")?)),
            "--root" => args.root = Some(PathBuf::from(value("--root")?)),
            "--json" => args.json = true,
            "-j" | "--threads" if counts => {
                let n = value("--threads")?;
                args.threads = Some(n.parse().ok().filter(|&n| n > 0).ok_or_else(|| {
                    usage_error(format!("Invalid thread count '{n}'"), help.clone())
                })?);
            }
            "--unused" if counts => args.unused = true,
            "-h" | "--help" => {
                print_stdout(&help).map_err(|message| usage_error(message, String::new()))?;
                return Ok(Parsed::Exit(ExitCode::SUCCESS));
            }
            _ if arg.starts_with('-') && arg.len() > 1 => return Err(unknown_option(&arg, help)),
            _ if command == Command::View => args.paths.push(arg),
            _ => return Err(usage_error(format!("Unexpected argument '{arg}'"), help)),
        }
    }
    if command == Command::View && args.paths.is_empty() {
        return Err(usage_error("Expected at least one path", help));
    }
    Ok(Parsed::Run(args))
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

fn load(args: &Args) -> Result<Loaded, String> {
    // The repository containing the current directory, or else the directory.
    let current_root = || {
        let current_dir = std::env::current_dir().unwrap_or_default();
        find_repo_root(&current_dir).unwrap_or(current_dir)
    };
    let codeowners = match &args.codeowners {
        Some(path) => path.clone(),
        None => {
            let root = args.root.clone().unwrap_or_else(current_root);
            find_codeowners(&root)
                .map(|path| {
                    path.strip_prefix(".")
                        .map(Path::to_path_buf)
                        .unwrap_or(path)
                })
                .ok_or_else(|| format!("No CODEOWNERS file found in {}", root.display()))?
        }
    };
    let root = match &args.root {
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
    let ignored = match args.codeowners {
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

fn stats_for(args: &Args, loaded: &Loaded) -> Result<Stats, String> {
    check_size(loaded)?;
    let rules = parse_codeowners(&loaded.text);
    let threads = args
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
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
    let line_width = rules
        .iter()
        .map(|r| (r.rule.lineno + 1).to_string().len())
        .max()
        .unwrap_or(0)
        .max(4);
    let files_width = rules
        .iter()
        .map(|r| r.files.to_string().len())
        .max()
        .unwrap_or(0)
        .max(5);
    let pattern_width = rules
        .iter()
        .map(|r| r.rule.pattern.chars().count())
        .max()
        .unwrap_or(0)
        .max(7);
    write!(
        out,
        "\n\n{:>line_width$}  {:>files_width$}  pattern",
        "line", "files"
    )
    .unwrap();
    for r in rules {
        write!(
            out,
            "\n{:>line_width$}  {:>files_width$}  ",
            r.rule.lineno + 1,
            r.files
        )
        .unwrap();
        let note = r.status.note();
        if note.is_empty() {
            out.push_str(&r.rule.pattern);
        } else {
            write!(out, "{:<pattern_width$}  {note}", r.rule.pattern).unwrap();
        }
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
    args: &Args,
    loaded: &Loaded,
) -> Result<Vec<(String, Option<CodeownersRule>)>, String> {
    check_size(loaded)?;
    let rules = parse_codeowners(&loaded.text);
    let paths = args
        .paths
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
fn check_problems(args: &Args, loaded: &Loaded) -> Result<Vec<LineProblem>, String> {
    let mut problems = find_problems(&loaded.text);
    if let Some(message) = ignored_warning(loaded, "this file") {
        problems.push(LineProblem {
            lineno: None,
            error: false,
            message,
        });
    }
    // A file too big for GitHub already has an error, and nothing to count.
    if args.unused && !too_big(loaded) {
        let stats = stats_for(args, loaded)?;
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

fn run(args: &Args) -> Result<ExitCode, String> {
    let loaded = load(args)?;
    if args.command == Command::Check {
        let problems = check_problems(args, &loaded)?;
        print_stdout(&if args.json {
            to_json(&problems.iter().map(problem_json).collect::<Vec<_>>())
        } else if problems.is_empty() {
            format!("{}: no problems", display_path(&loaded))
        } else {
            problem_lines(&display_path(&loaded), &problems)
        })?;
        return Ok(if problems.iter().any(|p| p.error) {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        });
    }
    // On stderr, so it doesn't mix with output meant for other programs.
    if let Some(message) = ignored_warning(&loaded, &display_path(&loaded)) {
        eprintln!("gh-codeowners: warning: {message}");
    }
    if args.command == Command::View {
        let files = ownership(args, &loaded)?;
        print_stdout(&if args.json {
            to_json(
                &files
                    .iter()
                    .map(|(path, rule)| ownership_json(path, rule.as_ref()))
                    .collect::<Vec<_>>(),
            )
        } else {
            format_ownership(&loaded, &files)
        })?;
        return Ok(ExitCode::SUCCESS);
    }
    let stats = stats_for(args, &loaded)?;
    let rules: Vec<&RuleStats> = stats
        .rules
        .iter()
        .filter(|r| !args.unused || r.status != Status::Active)
        .collect();
    print_stdout(&if args.json {
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

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1).collect()) {
        Ok(Parsed::Run(args)) => args,
        Ok(Parsed::Exit(code)) => return code,
        Err(UsageError { message, help }) if help.is_empty() => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
        Err(UsageError { message, help }) => {
            eprintln!("{message}\n\n{help}");
            return ExitCode::from(2);
        }
    };
    run(&args).unwrap_or_else(|message| {
        eprintln!("gh-codeowners: {message}");
        ExitCode::from(2)
    })
}
