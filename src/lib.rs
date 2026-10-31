//! Count the files each CODEOWNERS rule owns, find rules that own nothing,
//! find which rule owns a file, and find mistakes in CODEOWNERS, all matching
//! like GitHub.

pub mod codeowners;
pub mod counter;
pub mod index;
pub mod pattern;

use std::path::Path;
use std::sync::{Mutex, mpsc};

use counter::{CHUNK, Counts, Rule, Rules, count, count_with};
use index::{Error, Index, ls_files};

/// Count the files in `root`'s git index that each rule owns, on `threads`
/// threads.
///
/// Reading the index and counting overlap: threads parse the index into
/// batches, and others count batches as they arrive. Indexes the reader
/// can't handle, like split and sparse indexes, are listed with
/// `git ls-files` instead.
pub fn count_tracked_files(root: &Path, rules: &[Rule], threads: usize) -> Result<Counts, Error> {
    let threads = threads.max(1);
    let index = match Index::open(root) {
        Ok(index) => index,
        Err(Error::Unsupported(_)) => return count_ls_files(root, &Rules::new(rules), threads),
        Err(error) => return Err(error),
    };
    let compiled = Rules::new(rules);
    let counted = match count_index(&index, &compiled, threads, true) {
        // A guess in the parallel parse was wrong, so parse sequentially.
        Ok(None) => count_index(&index, &compiled, threads, false),
        result => result,
    };
    match counted {
        // Sequential parses are never rejected.
        Ok(counts) => Ok(counts.expect("sequential parse")),
        // Parsing may have stopped partway through, so start over.
        Err(Error::Unsupported(_)) => count_ls_files(root, &compiled, threads),
        Err(error) => Err(error),
    }
}

// Count while parsing, returning None if a parallel parse failed its checks.
fn count_index(
    index: &Index,
    rules: &Rules,
    threads: usize,
    parallel: bool,
) -> Result<Option<Counts>, Error> {
    let (sender, receiver) = mpsc::channel();
    let receiver = Mutex::new(receiver);
    let (counts, parsed) = std::thread::scope(|scope| {
        let parsing = scope.spawn(move || {
            // Dropping `sender` at the end tells counting threads to stop.
            let send = |batch| sender.send(batch).unwrap();
            if parallel {
                index.for_each_batch_parallel(threads, CHUNK, send)
            } else {
                index.for_each_batch(CHUNK, send).map(|()| true)
            }
        });
        let counts = count_with(rules, threads, |add| {
            let Ok(batch) = receiver.lock().unwrap().recv() else {
                return false;
            };
            batch.paths(Some(index)).for_each(add);
            true
        });
        (counts, parsing.join().unwrap())
    });
    Ok(parsed?.then_some(counts))
}

/// For each path, the index in `rules` of the rule that owns it: the last
/// one matching the path or a directory containing it. Paths needn't be in
/// git's index.
pub fn owners_of<P: AsRef<[u8]>>(rules: &[Rule], paths: &[P]) -> Vec<Option<usize>> {
    let compiled = Rules::new(rules);
    paths
        .iter()
        .map(|path| {
            count(&compiled, &[path.as_ref()], 1)
                .rules
                .iter()
                .position(|rule| rule.owned == 1)
        })
        .collect()
}

fn count_ls_files(root: &Path, rules: &Rules, threads: usize) -> Result<Counts, Error> {
    let batch = ls_files(root)?;
    let paths: Vec<&[u8]> = batch.paths(None).collect();
    Ok(count(rules, &paths, threads))
}
