use std::path::Path;
use std::process::ExitCode;

use gh_codeowners::format::{Style, format_with};

use super::load::{CodeownersFile, display_path};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Don't change the file. Exit with 1 if it isn't formatted
    #[arg(long)]
    check: bool,
    /// How to format
    #[arg(long, value_enum, default_value_t = StyleArg::Default)]
    style: StyleArg,
    /// With `--style legacy`, spaces between the longest pattern and the
    /// first owners, besides the one that's always there (default: 4)
    #[arg(long, value_name = "n")]
    alignment_offset: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum StyleArg {
    /// Line up owners and comments in each group of rules, like `terraform fmt`
    Default,
    /// Format like the VS Code extension did before 5.1.0
    Legacy,
}

impl Args {
    fn style(&self) -> Result<Style, String> {
        match (self.style, self.alignment_offset) {
            (StyleArg::Default, None) => Ok(Style::Default),
            (StyleArg::Default, Some(_)) => {
                Err("--alignment-offset needs --style legacy".to_string())
            }
            (StyleArg::Legacy, offset) => Ok(Style::Legacy {
                alignment_offset: offset.unwrap_or(Style::DEFAULT_ALIGNMENT_OFFSET),
            }),
        }
    }
}

pub(crate) fn run(file: &CodeownersFile, args: &Args) -> Result<ExitCode, String> {
    let formatted = format_with(&file.text, args.style()?);
    if file.path == Path::new("-") {
        // From stdin, without a newline since the text has its own.
        if !args.check {
            print!("{formatted}");
        }
    } else if formatted != file.text {
        if !args.check {
            std::fs::write(&file.path, &formatted)
                .map_err(|e| format!("{}: {e}", file.path.display()))?;
        }
        // Like `terraform fmt`, list the files that needed formatting.
        println!("{}", display_path(file));
    }
    Ok(if args.check && formatted != file.text {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}
