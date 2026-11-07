use std::path::Path;

use gh_codeowners::count_tracked_files;
use gh_codeowners::counter::Rule;

mod common;
use common::{Repo, git, git_output};

fn make_repo(name: &str) -> Repo {
    let repo = Repo::new(name);
    git(&repo.0, &["init", "-q", "-b", "main"]);
    for file in ["a/1", "a/b/2", "c/3", "é.txt", "with space", "z"] {
        repo.write(file, &format!("{file}\n"));
    }
    git(&repo.0, &["add", "."]);
    git(&repo.0, &["commit", "-qm", "init"]);
    repo
}

fn count_everything(dir: &Path) -> Result<usize, String> {
    let rules = [Rule {
        pattern: "*".into(),
        lineno: 0,
    }];
    count_tracked_files(dir, &rules).map(|counts| counts.files)
}

#[test]
fn counts_tracked_files_with_unusual_names() {
    assert_eq!(count_everything(&make_repo("names").0), Ok(6));
}

#[test]
fn counts_conflicted_files_once() {
    let repo = make_repo("conflict");
    git(&repo.0, &["checkout", "-qb", "other"]);
    std::fs::write(repo.0.join("z"), "other\n").unwrap();
    git(&repo.0, &["commit", "-qam", "other"]);
    git(&repo.0, &["checkout", "-q", "main"]);
    std::fs::write(repo.0.join("z"), "main\n").unwrap();
    git(&repo.0, &["commit", "-qam", "main"]);
    // Conflicts, on purpose.
    git_output(&repo.0, &["merge", "-q", "other"]);
    assert_eq!(count_everything(&repo.0), Ok(6));
}

#[test]
fn counts_files_outside_a_sparse_checkout() {
    let repo = make_repo("sparse");
    git(
        &repo.0,
        &["sparse-checkout", "set", "--cone", "--sparse-index", "a"],
    );
    assert_eq!(count_everything(&repo.0), Ok(6));
}

#[test]
fn counts_nothing_before_the_first_add() {
    let repo = Repo::new("empty");
    git(&repo.0, &["init", "-q"]);
    assert_eq!(count_everything(&repo.0), Ok(0));
}

#[test]
fn fails_outside_a_git_repository() {
    let dir = Repo::new("not-a-repo");
    let error = count_everything(&dir.0).unwrap_err();
    assert!(error.starts_with("git ls-files exited with"), "{error}");
}
