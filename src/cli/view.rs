use std::path::Path;
use std::process::ExitCode;

use gh_codeowners::codeowners::{CodeownersRule, check_size, parse_codeowners};
use gh_codeowners::counter::rule_list;
use gh_codeowners::owners_of;
use gh_codeowners::report::{ownership_json, to_json};
use path_clean::PathClean as _;
use path_slash::PathExt as _;

use super::load::{CodeownersFile, absolute, display_path, print_stdout, warn_if_ignored};

#[derive(clap::Args)]
pub(crate) struct Args {
    #[arg(required = true, value_name = "path")]
    paths: Vec<String>,
    /// Output JSON
    #[arg(long)]
    json: bool,
}

pub(crate) fn run(file: &CodeownersFile, args: &Args) -> Result<ExitCode, String> {
    warn_if_ignored(file);
    let files = ownership(file, &args.paths)?;
    print_stdout(&if args.json {
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
    let given = absolute(Path::new(path)).clean();
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
    let mut relative = relative.to_slash_lossy().into_owned();
    // Directories end in a slash, so rules for directories match them.
    if !relative.is_empty() && given.is_dir() {
        relative.push('/');
    }
    Ok(relative)
}

/// Each path to view, and the rule that owns it.
fn ownership(
    file: &CodeownersFile,
    paths: &[String],
) -> Result<Vec<(String, Option<CodeownersRule>)>, String> {
    check_size(&display_path(file), &file.text)?;
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
