use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use cli::load::load;
use cli::{check, fmt, list, lsp, view};

mod cli;

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
    /// Check CODEOWNERS for problems
    ///
    /// Warns about lines that are misleading. Exits with 1 if CODEOWNERS has errors,
    /// and 2 if the command fails.
    Check(check::Args),
    /// List each rule and how many files it owns
    ///
    /// File are counted from git's index.
    /// Problems are reported after the counts.
    List(list::Args),
    /// Show the rule that owns each path
    ///
    /// Output matches GitHub's code owners icon tooltip.
    View(view::Args),
    /// Format CODEOWNERS, like `terraform fmt`
    ///
    /// Removes indentation and extra spaces, and lines up the owners and
    /// comments of consecutive rules. Changes only whitespace. Prints the
    /// file's path if it needed formatting, or with `--codeowners -`, prints
    /// the formatted text.
    Fmt(fmt::Args),
    /// Run a language server for CODEOWNERS files
    ///
    /// For editors like VS Code. It reports problems as you type, shows how
    /// many files each rule owns, and answers `gh-codeowners/owner` requests
    /// for the rule that owns a file.
    Lsp(lsp::Args),
}

fn run(cli: &Cli) -> Result<ExitCode, String> {
    if let Command::Lsp(args) = &cli.command {
        // The server finds each document's repository itself.
        if cli.codeowners.is_some() || cli.root.is_some() || cli.json {
            return Err("lsp doesn't take --codeowners, --root or --json".to_string());
        }
        return lsp::run(args);
    }
    if matches!(cli.command, Command::Fmt(_)) && cli.json {
        return Err("fmt doesn't take --json".to_string());
    }
    let file = load(cli)?;
    match &cli.command {
        Command::Check(args) => check::run(&file, args, cli.json),
        Command::List(args) => list::run(&file, args, cli.json),
        Command::View(args) => view::run(&file, args, cli.json),
        Command::Fmt(args) => fmt::run(&file, args),
        Command::Lsp(_) => unreachable!("handled above"),
    }
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
