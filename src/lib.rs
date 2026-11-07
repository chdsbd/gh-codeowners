//! Count the files each CODEOWNERS rule owns, find rules that own nothing,
//! find which rule owns a file, and find mistakes in CODEOWNERS, all matching
//! like GitHub.

pub mod codeowners;
pub mod counter;
pub mod pattern;
pub mod report;

// WebAssembly can't read git's index, so its callers list the files.
#[cfg(not(target_family = "wasm"))]
mod tracked;
#[cfg(not(target_family = "wasm"))]
pub use tracked::{count_tracked_files, tracked_files};

use counter::Rule;

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
