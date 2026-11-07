//! Count the files each CODEOWNERS rule owns, find rules that own nothing,
//! find which rule owns a file, and find mistakes in CODEOWNERS, all matching
//! like GitHub.

pub mod codeowners;
pub mod counter;
pub mod pattern;
pub mod report;

use std::path::Path;
use std::process::Command;

use counter::{Counts, Rule, Rules, count};

/// The files in a repository's git index, as `git ls-files` lists them.
pub struct TrackedFiles(Vec<u8>);

impl TrackedFiles {
    /// List the files in `root`'s git index, relative to `root`.
    pub fn list(root: &Path) -> Result<TrackedFiles, String> {
        let output = Command::new("git")
            .args(["ls-files", "-z", "--cached"])
            .current_dir(root)
            .output()
            .map_err(|error| format!("Couldn't run git: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git ls-files exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(TrackedFiles(output.stdout))
    }

    /// Each file's path, in git's order.
    pub fn paths(&self) -> Vec<&[u8]> {
        let mut paths: Vec<&[u8]> = self.0.split(|&b| b == 0).collect();
        // Every path ends in a NUL, which leaves an empty one at the end.
        paths.pop();
        // Conflicted files are listed once per stage.
        paths.dedup();
        paths
    }

    /// Count the files each rule owns.
    pub fn count(&self, rules: &[Rule]) -> Counts {
        count(&Rules::new(rules), &self.paths())
    }
}

/// Count the files in `root`'s git index that each rule owns.
pub fn count_tracked_files(root: &Path, rules: &[Rule]) -> Result<Counts, String> {
    Ok(TrackedFiles::list(root)?.count(rules))
}

/// For each path, the index in `rules` of the rule that owns it: the last
/// one matching the path or a directory containing it. Paths needn't be in
/// git's index, and a path ending in `/` is a directory.
pub fn owners_of<P: AsRef<[u8]>>(rules: &[Rule], paths: &[P]) -> Vec<Option<usize>> {
    let matchers: Vec<pattern::Matcher> = rules
        .iter()
        .map(|rule| pattern::Matcher::new(&rule.pattern))
        .collect();
    paths
        .iter()
        .map(|path| {
            let path = path.as_ref();
            // The path, and each directory containing it, with a trailing
            // slash, as matchers expect directories.
            let mut candidates: Vec<&[u8]> = path
                .iter()
                .enumerate()
                .filter(|&(i, &b)| b == b'/' && i + 1 < path.len())
                .map(|(i, _)| &path[..=i])
                .collect();
            candidates.push(path);
            matchers
                .iter()
                .rposition(|m| candidates.iter().any(|c| m.is_match(c)))
        })
        .collect()
}
