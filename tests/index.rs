use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use gh_codeowners::count_tracked_files;
use gh_codeowners::counter::Rule;
use gh_codeowners::index::{Error, Index};

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8(output.stdout).unwrap()
}

fn empty_repo(name: &str, init_args: &[&str]) -> Repo {
    let dir = std::env::temp_dir().join(format!("gh-codeowners-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &[&["init", "-q", "-b", "main"], init_args].concat());
    Repo(dir)
}

fn make_repo(name: &str, init_args: &[&str]) -> Repo {
    let repo = empty_repo(name, init_args);
    for file in ["a/1", "a/b/2", "c/3", "é.txt", "with space", "z"] {
        let path = repo.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{file}\n")).unwrap();
    }
    git(&repo.0, &["add", "."]);
    git(&repo.0, &["commit", "-qm", "init"]);
    repo
}

fn ls_files(dir: &Path) -> Vec<String> {
    let mut files: Vec<String> = git(dir, &["ls-files", "-z"])
        .split('\0')
        .map(String::from)
        .collect();
    files.pop();
    // ls-files lists conflicted files once per stage.
    files.dedup();
    files
}

fn read_index(dir: &Path) -> Result<Vec<String>, Error> {
    let index = Index::open(dir)?;
    let mut files = Vec::new();
    index.for_each_batch(3, |batch| {
        files.extend(
            batch
                .paths(Some(&index))
                .map(|p| String::from_utf8(p.to_vec()).unwrap()),
        );
    })?;
    Ok(files)
}

fn read_index_parallel(dir: &Path, threads: usize) -> (bool, Vec<String>) {
    let index = Index::open(dir).unwrap();
    let files = Mutex::new(Vec::new());
    let checked = index
        .for_each_batch_parallel(threads, 1000, |batch| {
            let paths = batch
                .paths(Some(&index))
                .map(|p| String::from_utf8(p.to_vec()).unwrap());
            files.lock().unwrap().extend(paths);
        })
        .unwrap();
    let mut files = files.into_inner().unwrap();
    // Threads finish batches in any order.
    files.sort();
    (checked, files)
}

fn count_everything(dir: &Path) -> usize {
    let rules = [Rule {
        pattern: "*".into(),
        lineno: 0,
    }];
    count_tracked_files(dir, &rules, 2).unwrap().files
}

#[test]
fn lists_the_same_files_as_git_for_every_index_version() {
    let repo = make_repo("versions", &[]);
    // Intent-to-add entries use version 3's extended flags.
    std::fs::write(repo.0.join("new"), "").unwrap();
    git(&repo.0, &["add", "-N", "new"]);
    git(&repo.0, &["update-index", "--skip-worktree", "c/3"]);
    for version in ["2", "3", "4"] {
        git(&repo.0, &["update-index", "--index-version", version]);
        assert_eq!(
            read_index(&repo.0).unwrap(),
            ls_files(&repo.0),
            "version {version}"
        );
    }
    assert!(read_index(&repo.0).unwrap().contains(&"new".to_string()));
}

#[test]
fn lists_conflicted_files_once() {
    let repo = make_repo("conflict", &[]);
    git(&repo.0, &["checkout", "-qb", "other"]);
    std::fs::write(repo.0.join("z"), "other\n").unwrap();
    git(&repo.0, &["commit", "-qam", "other"]);
    git(&repo.0, &["checkout", "-q", "main"]);
    std::fs::write(repo.0.join("z"), "main\n").unwrap();
    git(&repo.0, &["commit", "-qam", "main"]);
    git(&repo.0, &["merge", "-q", "other"]);
    for version in ["2", "4"] {
        git(&repo.0, &["update-index", "--index-version", version]);
        assert_eq!(
            read_index(&repo.0).unwrap(),
            ls_files(&repo.0),
            "version {version}"
        );
    }
}

#[test]
fn reads_sha256_repositories() {
    let repo = make_repo("sha256", &["--object-format=sha256"]);
    assert_eq!(read_index(&repo.0).unwrap(), ls_files(&repo.0));
}

#[test]
fn reads_linked_worktrees() {
    let repo = make_repo("worktree", &[]);
    let worktree = repo.0.join("worktree");
    git(
        &repo.0,
        &["worktree", "add", "-q", worktree.to_str().unwrap()],
    );
    std::fs::write(worktree.join("only-here"), "").unwrap();
    git(&worktree, &["add", "only-here"]);
    let files = read_index(&worktree).unwrap();
    assert_eq!(files, ls_files(&worktree));
    assert!(files.contains(&"only-here".to_string()));
}

#[test]
fn lists_nothing_before_the_first_add() {
    let repo = empty_repo("empty", &[]);
    assert!(read_index(&repo.0).unwrap().is_empty());
}

#[test]
fn falls_back_to_git_for_split_indexes() {
    let repo = make_repo("split", &[]);
    git(&repo.0, &["update-index", "--split-index"]);
    std::fs::write(repo.0.join("new"), "").unwrap();
    git(&repo.0, &["add", "new"]);
    assert!(matches!(read_index(&repo.0), Err(Error::Unsupported(_))));
    assert_eq!(count_everything(&repo.0), ls_files(&repo.0).len());
}

#[test]
fn falls_back_to_git_for_sparse_indexes() {
    let repo = make_repo("sparse", &[]);
    git(
        &repo.0,
        &["sparse-checkout", "set", "--cone", "--sparse-index", "a"],
    );
    // The index has a single entry for each directory outside the cone.
    assert!(matches!(read_index(&repo.0), Err(Error::Unsupported(_))));
    assert_eq!(count_everything(&repo.0), 6);
}

#[test]
fn parses_large_indexes_in_parallel() {
    let repo = empty_repo("parallel", &[]);
    // Enough entries to parse in parallel, with path lengths that vary so
    // threads start at unpredictable offsets.
    let blob = git(&repo.0, &["hash-object", "-w", "--stdin"]);
    let mut child = Command::new("git")
        .args(["update-index", "--index-info"])
        .current_dir(&repo.0)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for i in 0..60_000 {
        let name = "x".repeat(i % 37);
        writeln!(
            stdin,
            "100644 {} 0\tdir{}/sub{}/{name}file{i}",
            blob.trim(),
            i / 1000,
            i % 7
        )
        .unwrap();
    }
    drop(stdin);
    assert!(child.wait().unwrap().success());

    let mut expected = ls_files(&repo.0);
    expected.sort();
    for version in ["2", "3"] {
        git(&repo.0, &["update-index", "--index-version", version]);
        for threads in [2, 3, 8] {
            let (checked, files) = read_index_parallel(&repo.0, threads);
            assert!(checked, "version {version}, {threads} threads");
            assert_eq!(files, expected, "version {version}, {threads} threads");
        }
    }
    assert_eq!(count_everything(&repo.0), 60_000);
}
