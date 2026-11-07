use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use gh_codeowners::codeowners::{
    find_codeowners, find_repo_root, ignored_codeowners, repo_root_for_codeowners,
};
use path_slash::PathExt as _;

use crate::Cli;

pub(crate) fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) struct CodeownersFile {
    // "-" for stdin.
    pub(crate) path: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) text: String,
    // A CODEOWNERS file GitHub ignores, because the one it uses has no rules.
    pub(crate) ignored: Option<PathBuf>,
}

fn is_stdin(path: &Path) -> bool {
    path == Path::new("-")
}

pub(crate) fn load(cli: &Cli) -> Result<CodeownersFile, String> {
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
    Ok(CodeownersFile {
        path: codeowners,
        root,
        text,
        ignored,
    })
}

/// The CODEOWNERS file's path relative to the root, for messages.
pub(crate) fn display_path(file: &CodeownersFile) -> String {
    if is_stdin(&file.path) {
        return "<stdin>".to_string();
    }
    repo_display(&file.root, &file.path)
}

/// `path` relative to `root`, for messages. Inside the repository, it's a
/// repository path, written with `/` like the paths of the files it owns.
pub(crate) fn repo_display(root: &Path, path: &Path) -> String {
    let path = absolute(path);
    match path.strip_prefix(absolute(root)) {
        Ok(relative) => relative.to_slash_lossy().into_owned(),
        Err(_) => path.display().to_string(),
    }
}

pub(crate) fn ignored_file_warning(file: &CodeownersFile, subject: &str) -> Option<String> {
    Some(ignored_message(subject, &file.root, file.ignored.as_ref()?))
}

/// The warning for a CODEOWNERS file, `subject`, with no rules that GitHub
/// uses instead of `ignored`, which has some.
pub(crate) fn ignored_message(subject: &str, root: &Path, ignored: &Path) -> String {
    format!(
        "{subject} has no rules, but GitHub uses it instead of {}, so no file has an owner",
        repo_display(root, ignored)
    )
}

/// Warn that GitHub ignores another CODEOWNERS file's rules. On stderr, so it
/// doesn't mix with output meant for other programs.
pub(crate) fn warn_if_ignored(file: &CodeownersFile) {
    if let Some(message) = ignored_file_warning(file, &display_path(file)) {
        eprintln!("{}: warning: {message}", crate::program_name());
    }
}

/// Print a line to stdout, without any styles if it isn't a terminal. A reader that stops early, like `| head`, closes
/// the pipe; that isn't an error, so the exit code still reflects the run.
pub(crate) fn print_stdout(text: &str) -> Result<(), String> {
    match writeln!(anstream::stdout().lock(), "{text}") {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            Err(format!("Couldn't write output: {error}"))
        }
        _ => Ok(()),
    }
}
