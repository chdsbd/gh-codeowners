use std::path::Path;
use std::process::ExitCode;

use gh_codeowners::format::format;

use super::load::{CodeownersFile, display_path};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Don't change the file. Exit with 1 if it isn't formatted
    #[arg(long)]
    check: bool,
}

pub(crate) fn run(file: &CodeownersFile, args: &Args) -> Result<ExitCode, String> {
    let formatted = format(&file.text);
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
