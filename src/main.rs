use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use codeowners_stats::codeowners::{
    CodeownersRule, MAX_CODEOWNERS_BYTES, find_codeowners, parse_codeowners,
    repo_root_for_codeowners,
};
use codeowners_stats::count_tracked_files;
use codeowners_stats::counter::Rule;

const USAGE: &str = "Usage: codeowners-stats [options] [root]

Count the files each CODEOWNERS rule owns, and find rules that own nothing.
A file counts towards the last rule that matches it, like on GitHub.

Arguments:
  root                 Repository root (default: current directory)

Options:
  --codeowners <path>  CODEOWNERS file (default: the one GitHub would use)
  --unused             Only list rules that own no files
  --check              Exit with status 1 if any rule owns no files
  --json               Print results as JSON
  -j, --threads <n>    Threads to count with (default: one per CPU)
  -h, --help           Show this help
  -v, --version        Show the version";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    // Determines the owners of at least one file.
    Active,
    // Matches files, but a later rule determines all their owners.
    Overridden,
    // Matches no files at all.
    Unmatched,
}

impl Status {
    fn name(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Overridden => "overridden",
            Status::Unmatched => "unmatched",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Status::Active => "",
            Status::Overridden => "every matching file is owned by a later rule",
            Status::Unmatched => "no files match",
        }
    }
}

struct RuleStats {
    rule: CodeownersRule,
    files: usize,
    status: Status,
}

struct Stats {
    codeowners: PathBuf,
    root: PathBuf,
    files: usize,
    unowned: usize,
    rules: Vec<RuleStats>,
}

struct Args {
    root: Option<PathBuf>,
    codeowners: Option<PathBuf>,
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
    Ok(Parsed::Run(args))
}

fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

fn codeowners_stats(args: &Args) -> Result<Stats, String> {
    let codeowners = match &args.codeowners {
        Some(path) => path.clone(),
        None => {
            let root = args
                .root
                .clone()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            find_codeowners(&root)
                .map(|path| {
                    path.strip_prefix(".")
                        .map(Path::to_path_buf)
                        .unwrap_or(path)
                })
                .ok_or_else(|| format!("No CODEOWNERS file found in {}", root.display()))?
        }
    };
    let root = args
        .root
        .clone()
        .unwrap_or_else(|| repo_root_for_codeowners(&absolute(&codeowners)));
    let text = std::fs::read_to_string(&codeowners)
        .map_err(|e| format!("{}: {e}", codeowners.display()))?;
    if text.len() as u64 > MAX_CODEOWNERS_BYTES {
        return Err(format!(
            "{} is {} bytes, over GitHub's limit of {MAX_CODEOWNERS_BYTES}, so GitHub ignores it",
            codeowners.display(),
            text.len()
        ));
    }
    let rules = parse_codeowners(&text);
    let threads = args
        .threads
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
    let rule_list: Vec<Rule> = rules
        .iter()
        .map(|r| Rule {
            pattern: r.pattern.clone(),
            lineno: r.lineno,
        })
        .collect();
    let counts = count_tracked_files(&root, &rule_list, threads).map_err(|e| e.to_string())?;
    Ok(Stats {
        codeowners,
        root,
        files: counts.files,
        unowned: counts.unowned,
        rules: rules
            .into_iter()
            .zip(counts.rules)
            .map(|(rule, c)| RuleStats {
                rule,
                files: c.owned,
                status: if c.owned > 0 {
                    Status::Active
                } else if c.matches_any {
                    Status::Overridden
                } else {
                    Status::Unmatched
                },
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
    let codeowners = absolute(&stats.codeowners);
    let relative = codeowners
        .strip_prefix(absolute(&stats.root))
        .unwrap_or(&codeowners);
    let mut out = format!(
        "{}: {}, {}, {} with no owner, {} owning no files",
        relative.display(),
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

fn json_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// Same layout as JSON.stringify(value, null, 2).
fn format_json(stats: &Stats, rules: &[&RuleStats]) -> String {
    let mut out = String::from("{\n");
    writeln!(
        out,
        "  \"codeowners\": {},",
        json_string(&stats.codeowners.to_string_lossy())
    )
    .unwrap();
    writeln!(
        out,
        "  \"root\": {},",
        json_string(&stats.root.to_string_lossy())
    )
    .unwrap();
    writeln!(out, "  \"files\": {},", stats.files).unwrap();
    writeln!(out, "  \"unowned\": {},", stats.unowned).unwrap();
    if rules.is_empty() {
        out.push_str("  \"rules\": []\n}");
        return out;
    }
    out.push_str("  \"rules\": [\n");
    for (i, r) in rules.iter().enumerate() {
        out.push_str("    {\n");
        writeln!(out, "      \"line\": {},", r.rule.lineno + 1).unwrap();
        writeln!(out, "      \"pattern\": {},", json_string(&r.rule.pattern)).unwrap();
        if r.rule.owners.is_empty() {
            out.push_str("      \"owners\": [],\n");
        } else {
            out.push_str("      \"owners\": [\n");
            let owners: Vec<String> = r
                .rule
                .owners
                .iter()
                .map(|o| format!("        {}", json_string(o)))
                .collect();
            out.push_str(&owners.join(",\n"));
            out.push_str("\n      ],\n");
        }
        writeln!(out, "      \"files\": {},", r.files).unwrap();
        writeln!(out, "      \"status\": \"{}\"", r.status.name()).unwrap();
        out.push_str(if i + 1 == rules.len() {
            "    }\n"
        } else {
            "    },\n"
        });
    }
    out.push_str("  ]\n}");
    out
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
    let stats = match codeowners_stats(&args) {
        Ok(stats) => stats,
        Err(message) => {
            eprintln!("codeowners-stats: {message}");
            return ExitCode::from(2);
        }
    };
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
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
