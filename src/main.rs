use std::path::PathBuf;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};

use cli::load::{display_path, ignored_warning, load};
use cli::{check, list, view};

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
    /// Check CODEOWNERS for lines GitHub rejects
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
    /// Paths don't need to be tracked by git.
    View(view::Args),
}

fn run(cli: &Cli) -> Result<ExitCode, String> {
    let loaded = load(cli)?;
    // On stderr, so it doesn't mix with output meant for other programs.
    // `check` reports it as a problem instead.
    if !matches!(cli.command, Command::Check(_))
        && let Some(message) = ignored_warning(&loaded, &display_path(&loaded))
    {
        eprintln!("{}: warning: {message}", program_name());
    }
    match &cli.command {
        Command::Check(args) => check::run(&loaded, args, cli.json),
        Command::List(args) => list::run(&loaded, args, cli.json),
        Command::View(args) => view::run(&loaded, args, cli.json),
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
