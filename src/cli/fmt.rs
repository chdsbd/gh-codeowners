use std::path::Path;
use std::process::ExitCode;

use gh_codeowners::format::{Align, format_with};

use super::load::{CodeownersFile, display_path};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Don't change the file. Exit with 1 if it isn't formatted
    #[arg(long)]
    check: bool,
    /// Which rules' owners line up
    #[arg(long, value_enum, default_value_t = AlignArg::Group)]
    align: AlignArg,
    /// Spaces between the longest pattern and the owners
    #[arg(long, value_name = "n", default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..))]
    spacing: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum AlignArg {
    /// Each group of consecutive rules, like `terraform fmt`
    Group,
    /// Every rule in the file, like the VS Code extension before 5.1.0
    File,
}

pub(crate) fn run(file: &CodeownersFile, args: &Args) -> Result<ExitCode, String> {
    let align = match args.align {
        AlignArg::Group => Align::Group,
        AlignArg::File => Align::File,
    };
    let formatted = format_with(&file.text, align, args.spacing.into());
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
