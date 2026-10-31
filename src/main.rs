use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gh_codeowners::codeowners::{
    CodeownersRule, LineProblem, MAX_CODEOWNERS_BYTES, find_codeowners, find_problems,
    find_repo_root, parse_codeowners, repo_root_for_codeowners,
};
use gh_codeowners::counter::Rule;
use gh_codeowners::report::{Json, Status, ownership_json, problem_json, rule_json};
use gh_codeowners::{count_tracked_files, owners_of};

const USAGE: &str = "Usage: gh-codeowners [options] [root]

Count the files each CODEOWNERS rule owns, and find rules that own nothing.
A file counts towards the last rule that matches it, like on GitHub.

Arguments:
  root                 Repository root (default: the git repository
                       containing the current directory)

Options:
  --codeowners <path>  CODEOWNERS file, or - for stdin (default: the one
                       GitHub would use)
  --owner-of <path>    Instead of counting, print the rule that owns a file,
                       which needn't be tracked. Relative paths are relative
                       to the current directory. Repeat for more files.
  --errors             Instead of counting, list lines GitHub rejects, and
                       lines that likely don't do what they look like
  --unused             Only list rules that own no files
  --check              Exit with status 1 if any rule owns no files, or with
                       --errors, if any line has an error
  --json               Print results as JSON
  -j, --threads <n>    Threads to count with (default: one per CPU)
  -h, --help           Show this help
  -v, --version        Show the version";

struct RuleStats {
    rule: CodeownersRule,
    files: usize,
    status: Status,
}

struct Stats {
    codeowners: PathBuf,
    display_path: String,
    root: PathBuf,
    files: usize,
    unowned: usize,
    rules: Vec<RuleStats>,
}

struct Args {
    root: Option<PathBuf>,
    codeowners: Option<PathBuf>,
    owner_of: Vec<String>,
    errors: bool,
    unused: bool,
    check: bool,
    json: bool,
    threads: Option<usize>,
}

enum Parsed {
    Run(Args),
    Exit(ExitCode),
}

fn parse_args(argv: Vec<String>) -> Result<Parsed, String> {
    let mut args = Args {
        root: None,
        codeowners: None,
        owner_of: Vec::new(),
        errors: false,
        unused: false,
        check: false,
        json: false,
        threads: None,
    };
    let mut iter = argv.into_iter();
    while let Some(arg) = iter.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) if name.starts_with("--") => {
                (name.to_string(), Some(value.to_string()))
            }
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| iter.next())
                .ok_or_else(|| format!("Option '{name} <value>' argument missing"))
        };
        match name.as_str() {
            "--codeowners" => args.codeowners = Some(PathBuf::from(value("--codeowners")?)),
            "--owner-of" => args.owner_of.push(value("--owner-of")?),
            "--errors" => args.errors = true,
            "-j" | "--threads" => {
                let n = value("--threads")?;
                args.threads = Some(
                    n.parse()
                        .ok()
                        .filter(|&n| n > 0)
                        .ok_or_else(|| format!("Invalid thread count '{n}'"))?,
                );
            }
            "--unused" => args.unused = true,
            "--check" => args.check = true,
            "--json" => args.json = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(Parsed::Exit(ExitCode::SUCCESS));
            }
            "-v" | "--version" => {
                println!("{}", env!("CARGO_PKG_VERSION"));
                return Ok(Parsed::Exit(ExitCode::SUCCESS));
            }
            _ if arg.starts_with('-') && arg.len() > 1 => {
                return Err(format!("Unknown option '{arg}'"));
            }
            _ if args.root.is_some() => return Err("Expected at most one root, got 2".to_string()),
            _ => args.root = Some(PathBuf::from(arg)),
        }
    }
    if args.errors && !args.owner_of.is_empty() {
        return Err("Options '--errors' and '--owner-of' can't be used together".to_string());
    }
    if args.unused && (args.errors || !args.owner_of.is_empty()) {
        return Err("Option '--unused' only applies when counting".to_string());
    }
    if args.check && !args.owner_of.is_empty() {
        return Err("Option '--check' doesn't apply to '--owner-of'".to_string());
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
    Ok(Loaded {
        codeowners,
        root,
        text,
    })
}

/// The CODEOWNERS file's path relative to the root, for messages.
fn display_path(loaded: &Loaded) -> String {
    if is_stdin(&loaded.codeowners) {
        return "<stdin>".to_string();
    }
    let codeowners = absolute(&loaded.codeowners);
    codeowners
        .strip_prefix(absolute(&loaded.root))
        .unwrap_or(&codeowners)
        .display()
        .to_string()
}

fn check_size(loaded: &Loaded) -> Result<(), String> {
    if loaded.text.len() as u64 > MAX_CODEOWNERS_BYTES {
        return Err(format!(
            "{} is {} bytes, over GitHub's limit of {MAX_CODEOWNERS_BYTES}, so GitHub ignores it",
            loaded.codeowners.display(),
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

fn stats_for(args: &Args, loaded: Loaded) -> Result<Stats, String> {
    check_size(&loaded)?;
    let display_path = display_path(&loaded);
    let Loaded {
        codeowners,
        root,
        text,
    } = loaded;
    let rules = parse_codeowners(&text);
    let threads = args
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
    let counts =
        count_tracked_files(&root, &rule_list(&rules), threads).map_err(|e| e.to_string())?;
    Ok(Stats {
        codeowners,
        display_path,
        root,
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

fn path_json(path: &Path) -> Json {
    Json::string(path.to_string_lossy())
}

fn format_json(stats: &Stats, rules: &[&RuleStats]) -> String {
    Json::Object(vec![
        ("codeowners", path_json(&stats.codeowners)),
        ("root", path_json(&stats.root)),
        ("files", Json::Number(stats.files)),
        ("unowned", Json::Number(stats.unowned)),
        (
            "rules",
            Json::Array(
                rules
                    .iter()
                    .map(|r| rule_json(&r.rule, r.files, r.status))
                    .collect(),
            ),
        ),
    ])
    .format()
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

/// Each `--owner-of` path, and the rule that owns it.
fn ownership(
    args: &Args,
    loaded: &Loaded,
) -> Result<Vec<(String, Option<CodeownersRule>)>, String> {
    check_size(loaded)?;
    let rules = parse_codeowners(&loaded.text);
    let paths = args
        .owner_of
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

fn format_ownership(
    loaded: &Loaded,
    files: &[(String, Option<CodeownersRule>)],
    json: bool,
) -> String {
    if json {
        return Json::Object(vec![
            ("codeowners", path_json(&loaded.codeowners)),
            ("root", path_json(&loaded.root)),
            (
                "files",
                Json::Array(
                    files
                        .iter()
                        .map(|(path, rule)| ownership_json(path, rule.as_ref()))
                        .collect(),
                ),
            ),
        ])
        .format();
    }
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

fn format_problems(loaded: &Loaded, problems: &[LineProblem], json: bool) -> String {
    let severity = |p: &LineProblem| if p.error { "error" } else { "warning" };
    if json {
        return Json::Object(vec![
            ("codeowners", path_json(&loaded.codeowners)),
            (
                "problems",
                Json::Array(problems.iter().map(problem_json).collect()),
            ),
        ])
        .format();
    }
    let display = display_path(loaded);
    if problems.is_empty() {
        return format!("{display}: no problems");
    }
    let lines: Vec<String> = problems
        .iter()
        .map(|p| match p.lineno {
            Some(n) => format!("{display}:{}: {}: {}", n + 1, severity(p), p.message),
            None => format!("{display}: {}: {}", severity(p), p.message),
        })
        .collect();
    lines.join("\n")
}

fn run(args: &Args) -> Result<ExitCode, String> {
    let loaded = load(args)?;
    if args.errors {
        let problems = find_problems(&loaded.text);
        println!("{}", format_problems(&loaded, &problems, args.json));
        let failed = args.check && problems.iter().any(|p| p.error);
        return Ok(if failed {
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        });
    }
    if !args.owner_of.is_empty() {
        let files = ownership(args, &loaded)?;
        println!("{}", format_ownership(&loaded, &files, args.json));
        return Ok(ExitCode::SUCCESS);
    }
    let stats = stats_for(args, loaded)?;
    let rules: Vec<&RuleStats> = stats
        .rules
        .iter()
        .filter(|r| !args.unused || r.status != Status::Active)
        .collect();
    println!(
        "{}",
        if args.json {
            format_json(&stats, &rules)
        } else {
            format_text(&stats, &rules)
        }
    );
    if args.check && stats.rules.iter().any(|r| r.status != Status::Active) {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1).collect()) {
        Ok(Parsed::Run(args)) => args,
        Ok(Parsed::Exit(code)) => return code,
        Err(message) => {
            eprintln!("{message}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    run(&args).unwrap_or_else(|message| {
        eprintln!("gh-codeowners: {message}");
        ExitCode::from(2)
    })
}
