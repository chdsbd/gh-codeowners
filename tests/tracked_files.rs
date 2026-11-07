use std::path::Path;
use std::process::Command;

use gh_codeowners::count_tracked_files;
use gh_codeowners::counter::Rule;
use tempfile::TempDir;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        // Ignore the user's git config, like core.fsmonitor or index.version.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .unwrap();
    assert!(
        output.status.success() || args[0] == "merge",
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn empty_dir(name: &str) -> TempDir {
    TempDir::with_prefix(format!("gh-codeowners-{name}-")).unwrap()
}

fn make_repo(name: &str) -> TempDir {
    let repo = empty_dir(name);
    git(repo.path(), &["init", "-q", "-b", "main"]);
    for file in ["a/1", "a/b/2", "c/3", "é.txt", "with space", "z"] {
        let path = repo.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{file}\n")).unwrap();
    }
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-qm", "init"]);
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
    assert_eq!(count_everything(make_repo("names").path()), Ok(6));
}

#[test]
fn counts_conflicted_files_once() {
    let repo = make_repo("conflict");
    git(repo.path(), &["checkout", "-qb", "other"]);
    std::fs::write(repo.path().join("z"), "other\n").unwrap();
    git(repo.path(), &["commit", "-qam", "other"]);
    git(repo.path(), &["checkout", "-q", "main"]);
    std::fs::write(repo.path().join("z"), "main\n").unwrap();
    git(repo.path(), &["commit", "-qam", "main"]);
    git(repo.path(), &["merge", "-q", "other"]);
    assert_eq!(count_everything(repo.path()), Ok(6));
}

#[test]
fn counts_files_outside_a_sparse_checkout() {
    let repo = make_repo("sparse");
    git(
        repo.path(),
        &["sparse-checkout", "set", "--cone", "--sparse-index", "a"],
    );
    assert_eq!(count_everything(repo.path()), Ok(6));
}

#[test]
fn counts_nothing_before_the_first_add() {
    let repo = empty_dir("empty");
    git(repo.path(), &["init", "-q"]);
    assert_eq!(count_everything(repo.path()), Ok(0));
}

#[test]
fn fails_outside_a_git_repository() {
    let dir = empty_dir("not-a-repo");
    let error = count_everything(dir.path()).unwrap_err();
    assert!(error.starts_with("git ls-files exited with"), "{error}");
}
