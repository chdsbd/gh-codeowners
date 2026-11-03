use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gh_codeowners::codeowners::{CodeownersRule, parse_codeowners};
use gh_codeowners::owners_of;
use gh_codeowners::report::{ownership_json, to_json};

use super::load::{Loaded, absolute, check_size, display_path, print_stdout, rule_list};

#[derive(clap::Args)]
pub(crate) struct Args {
    #[arg(required = true, value_name = "path")]
    paths: Vec<String>,
}

pub(crate) fn run(loaded: &Loaded, args: &Args, json: bool) -> Result<ExitCode, String> {
    let files = ownership(loaded, &args.paths)?;
    print_stdout(&if json {
        to_json(
            &files
                .iter()
                .map(|(path, rule)| ownership_json(path, rule.as_ref()))
                .collect::<Vec<_>>(),
        )
    } else {
        format_ownership(loaded, &files)
    })?;
    Ok(ExitCode::SUCCESS)
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

/// Each path to view, and the rule that owns it.
fn ownership(
    loaded: &Loaded,
    paths: &[String],
) -> Result<Vec<(String, Option<CodeownersRule>)>, String> {
    check_size(loaded)?;
    let rules = parse_codeowners(&loaded.text);
    let paths = paths
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

fn format_ownership(loaded: &Loaded, files: &[(String, Option<CodeownersRule>)]) -> String {
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
