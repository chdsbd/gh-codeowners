//! Reading the files in git's index.

use std::path::Path;

use crate::counter::{Counts, Rule, Rules, count};

/// The files in a repository's git index, as `git ls-files` lists them.
pub struct TrackedFiles(Vec<Vec<u8>>);

impl TrackedFiles {
    /// List the files in `root`'s git index.
    pub fn read(root: &Path) -> Result<TrackedFiles, String> {
        read(root).map(TrackedFiles)
    }

    /// Each file's path, grouped by directory, as `count` expects.
    pub fn paths(&self) -> Vec<&[u8]> {
        self.0.iter().map(Vec::as_slice).collect()
    }
}

/// Count the files in `root`'s git index that each rule owns.
pub fn count_tracked_files(root: &Path, rules: &[Rule]) -> Result<Counts, String> {
    let files = TrackedFiles::read(root)?;
    Ok(count(&Rules::new(rules), &files.paths()))
}

/// The files in the git index of the repository containing `root`, relative
/// to `root`, like `git ls-files` run there lists them: each path once, even
/// when conflicted, and files outside a sparse checkout too.
fn read(root: &Path) -> Result<Vec<Vec<u8>>, String> {
    let error =
        |error: gix::Error| format!("Couldn't read git's index in {}: {error}", root.display());
    let repo = gix::discover_with_environment_overrides(root).map_err(error)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| format!("{} is in a bare repository", root.display()))?;
    let prefix = prefix(workdir, root)?;
    let index = repo.index_or_empty().map_err(error)?;
    let mut paths = Vec::with_capacity(index.entries().len());
    for entry in index.entries() {
        let path = entry.path(&index);
        if !path.starts_with(&prefix) {
            continue;
        }
        if entry.mode.is_sparse() {
            // A sparse index stores a directory outside the sparse checkout
            // as one entry, its tree, which `git ls-files` expands.
            let mut path = path.to_vec();
            if !path.ends_with(b"/") {
                path.push(b'/');
            }
            expand_tree(&repo, entry.id, &mut path, prefix.len(), &mut paths).map_err(error)?;
        } else if paths
            .last()
            .is_none_or(|last: &Vec<u8>| last[..] != path[prefix.len()..])
        {
            // Conflicted files have an entry per stage, next to each other.
            paths.push(path[prefix.len()..].to_vec());
        }
    }
    Ok(paths)
}

/// `root` relative to the repository's working tree, with a trailing slash,
/// or empty at the top.
fn prefix(workdir: &Path, root: &Path) -> Result<Vec<u8>, String> {
    let real = |path: &Path| {
        std::fs::canonicalize(path)
            .map_err(|error| format!("Couldn't read {}: {error}", path.display()))
    };
    let (workdir, root) = (real(workdir)?, real(root)?);
    let relative = root
        .strip_prefix(&workdir)
        .map_err(|_| format!("{} isn't in {}", root.display(), workdir.display()))?;
    let mut prefix = Vec::new();
    for component in relative.components() {
        let name = component
            .as_os_str()
            .to_str()
            .ok_or_else(|| format!("{} isn't valid UTF-8", relative.display()))?;
        prefix.extend_from_slice(name.as_bytes());
        prefix.push(b'/');
    }
    Ok(prefix)
}

/// Push the path of everything but directories in tree `id`, which is at
/// `dir`, a path ending in `/`. Paths start after `strip` bytes.
fn expand_tree(
    repo: &gix::Repository,
    id: gix::ObjectId,
    dir: &mut Vec<u8>,
    strip: usize,
    paths: &mut Vec<Vec<u8>>,
) -> Result<(), gix::Error> {
    let tree = repo.find_tree(id)?;
    let len = dir.len();
    for entry in tree.decode()?.entries.iter() {
        dir.extend_from_slice(entry.filename);
        if entry.mode.is_tree() {
            dir.push(b'/');
            expand_tree(repo, entry.oid.to_owned(), dir, strip, paths)?;
        } else {
            paths.push(dir[strip..].to_vec());
        }
        dir.truncate(len);
    }
    Ok(())
}
