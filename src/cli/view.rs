use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gh_codeowners::codeowners::{CodeownersRule, parse_codeowners};
use gh_codeowners::owners_of;
use gh_codeowners::report::{ownership_json, to_json};

use super::load::{
    CodeownersFile, absolute, check_size, display_path, print_stdout, rule_list, warn_if_ignored,
};

#[derive(clap::Args)]
pub(crate) struct Args {
    #[arg(required = true, value_name = "path")]
    paths: Vec<String>,
}

pub(crate) fn run(file: &CodeownersFile, args: &Args, json: bool) -> Result<ExitCode, String> {
    warn_if_ignored(file);
    let files = ownership(file, &args.paths)?;
    print_stdout(&if json {
        to_json(
            &files
                .iter()
                .map(|(path, rule)| ownership_json(path, rule.as_ref()))
                .collect::<Vec<_>>(),
        )
    } else {
        format_ownership(file, &files)
    })?;
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn repo_path(root: &Path, path: &str) -> Result<String, String> {
    let given = normalize(&absolute(Path::new(path)));
    let outside = || format!("{path} isn't in {}", root.display());
    // Either may be spelled through a symlink, like /var on macOS, so find the
    // root by where it resolves to. The rest stays as written, since git tracks
    // a symlink as a file, and it needn't exist.
    let real_root = std::fs::canonicalize(root).map_err(|_| outside())?;
    let entry = given
        .ancestors()
        .find(|ancestor| std::fs::canonicalize(ancestor).is_ok_and(|real| real == real_root))
        .ok_or_else(outside)?;
    let relative = given.strip_prefix(entry).expect("an ancestor");
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

/// Each path to view, and the rule that owns it.
fn ownership(
    file: &CodeownersFile,
    paths: &[String],
) -> Result<Vec<(String, Option<CodeownersRule>)>, String> {
    check_size(file)?;
    let rules = parse_codeowners(&file.text);
    let paths = paths
        .iter()
        .map(|path| repo_path(&file.root, path))
        .collect::<Result<Vec<_>, _>>()?;
    let owners = owners_of(&rule_list(&rules), &paths);
    Ok(paths
        .into_iter()
        .zip(owners)
        .map(|(path, rule)| (path, rule.map(|i| rules[i].clone())))
        .collect())
}

fn format_ownership(file: &CodeownersFile, files: &[(String, Option<CodeownersRule>)]) -> String {
    let display = display_path(file);
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
