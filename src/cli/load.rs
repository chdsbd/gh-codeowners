use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use gh_codeowners::codeowners::{
    CodeownersRule, MAX_CODEOWNERS_BYTES, find_codeowners, find_repo_root, ignored_codeowners,
    repo_root_for_codeowners,
};
use gh_codeowners::counter::Rule;

use crate::Cli;

pub(crate) fn absolute(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) struct Loaded {
    // "-" for stdin.
    pub(crate) codeowners: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) text: String,
    // A CODEOWNERS file GitHub ignores, because the one it uses has no rules.
    pub(crate) ignored: Option<PathBuf>,
}

fn is_stdin(path: &Path) -> bool {
    path == Path::new("-")
}

pub(crate) fn load(cli: &Cli) -> Result<Loaded, String> {
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
    Ok(Loaded {
        codeowners,
        root,
        text,
        ignored,
    })
}

/// The CODEOWNERS file's path relative to the root, for messages.
pub(crate) fn display_path(loaded: &Loaded) -> String {
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

pub(crate) fn ignored_file_warning(loaded: &Loaded, subject: &str) -> Option<String> {
    let ignored = loaded.ignored.as_ref()?;
    let ignored = repo_display(&loaded.root, ignored);
    Some(format!(
        "{subject} has no rules, but GitHub uses it instead of {ignored}, so no file has an owner"
    ))
}

/// Warn that GitHub ignores another CODEOWNERS file's rules. On stderr, so it
/// doesn't mix with output meant for other programs.
pub(crate) fn warn_if_ignored(loaded: &Loaded) {
    if let Some(message) = ignored_file_warning(loaded, &display_path(loaded)) {
        eprintln!("{}: warning: {message}", crate::program_name());
    }
}

pub(crate) fn too_big(loaded: &Loaded) -> bool {
    loaded.text.len() as u64 > MAX_CODEOWNERS_BYTES
}

pub(crate) fn check_size(loaded: &Loaded) -> Result<(), String> {
    if too_big(loaded) {
        return Err(format!(
            "{} is {} bytes, over GitHub's limit of {MAX_CODEOWNERS_BYTES}, so GitHub ignores it",
            display_path(loaded),
            loaded.text.len()
        ));
    }
    Ok(())
}

pub(crate) fn rule_list(rules: &[CodeownersRule]) -> Vec<Rule> {
    rules
        .iter()
        .map(|r| Rule {
            pattern: r.pattern.clone(),
            lineno: r.lineno,
        })
        .collect()
}

/// Print a line to stdout. A reader that stops early, like `| head`, closes
/// the pipe; that isn't an error, so the exit code still reflects the run.
pub(crate) fn print_stdout(text: &str) -> Result<(), String> {
    match writeln!(io::stdout().lock(), "{text}") {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            Err(format!("Couldn't write output: {error}"))
        }
        _ => Ok(()),
    }
}
