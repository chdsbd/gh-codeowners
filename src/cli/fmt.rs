use std::path::Path;
use std::process::ExitCode;

use gh_codeowners::format::{Style, format_with};

use super::load::{CodeownersFile, display_path};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Don't change the file. Exit with 1 if it isn't formatted
    #[arg(long)]
    check: bool,
    /// Line up the first owner of every rule in the file, and change nothing
    /// else, like the VS Code extension did before 5.1.0
    #[arg(long)]
    legacy: bool,
    /// Spaces between the longest pattern and the first owners, besides the
    /// one that's always there (default: 4)
    #[arg(long, value_name = "n", requires = "legacy")]
    alignment_offset: Option<usize>,
}

impl Args {
    fn style(&self) -> Style {
        if self.legacy {
            Style::Legacy {
                alignment_offset: self
                    .alignment_offset
                    .unwrap_or(Style::DEFAULT_ALIGNMENT_OFFSET),
            }
        } else {
            Style::Default
        }
    }
}

pub(crate) fn run(file: &CodeownersFile, args: &Args) -> Result<ExitCode, String> {
    let formatted = format_with(&file.text, args.style());
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
