use std::path::{Path, PathBuf};
use std::process::Command;

use gh_codeowners::counter::Rule;
use gh_codeowners::{count_tracked_files, tracked_files};

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) -> Vec<u8> {
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
    output.stdout
}

fn git_with_input(dir: &Path, args: &[&str], input: &str) {
    use std::io::Write;
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success(), "git {args:?}");
}

fn empty_dir(name: &str) -> Repo {
    let dir = std::env::temp_dir().join(format!("gh-codeowners-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Repo(dir)
}

fn make_repo(name: &str) -> Repo {
    let repo = empty_dir(name);
    git(&repo.0, &["init", "-q", "-b", "main"]);
    for file in ["a/1", "a/b/2", "c/3", "é.txt", "with space", "z"] {
        let path = repo.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{file}\n")).unwrap();
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
    git(&repo.0, &["merge", "-q", "other"]);
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
    let repo = empty_dir("empty");
    git(&repo.0, &["init", "-q"]);
    assert_eq!(count_everything(&repo.0), Ok(0));
}

#[test]
fn fails_outside_a_git_repository() {
    let dir = empty_dir("not-a-repo");
    let error = count_everything(&dir.0).unwrap_err();
    assert!(error.starts_with("Couldn't read git's index"), "{error}");
}

/// Assert that we list the files `git ls-files` does in `dir`.
fn assert_lists_like_git(dir: &Path) -> Vec<Vec<u8>> {
    let mut expected: Vec<Vec<u8>> = git(dir, &["ls-files", "-z"])
        .split(|&b| b == 0)
        .map(<[u8]>::to_vec)
        .collect();
    expected.pop();
    expected.dedup();
    let actual = tracked_files(dir).unwrap();
    let show = |paths: &[Vec<u8>]| -> Vec<String> {
        paths
            .iter()
            .map(|p| String::from_utf8_lossy(p).into_owned())
            .collect()
    };
    assert_eq!(show(&actual), show(&expected));
    actual
}

/// Whether `dir`'s index has the extension with `signature`.
fn has_extension(dir: &Path, signature: &[u8; 4]) -> bool {
    let index = std::fs::read(dir.join(".git/index")).unwrap();
    index.windows(4).any(|window| window == signature)
}

/// A repository with more files, nested deeper, in every kind of entry.
fn make_big_repo(name: &str, init: &[&str]) -> Repo {
    let repo = empty_dir(name);
    let mut args = vec!["init", "-q", "-b", "main"];
    args.extend_from_slice(init);
    git(&repo.0, &args);
    for i in 0..40 {
        for file in [
            format!("a/{i}/x"),
            format!("a/{i}/y/z"),
            format!("b/{i}"),
            format!("c-{i}"),
        ] {
            let path = repo.0.join(&file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("{file}\n")).unwrap();
        }
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("b/0", repo.0.join("link")).unwrap();
    git(&repo.0, &["add", "."]);
    git(&repo.0, &["commit", "-qm", "init"]);
    let commit = String::from_utf8(git(&repo.0, &["rev-parse", "HEAD"])).unwrap();
    let cacheinfo = format!("160000,{},sub", commit.trim());
    git(
        &repo.0,
        &["update-index", "--add", "--cacheinfo", &cacheinfo],
    );
    std::fs::write(repo.0.join("intended"), "intended\n").unwrap();
    git(&repo.0, &["add", "-N", "intended"]);
    git(&repo.0, &["commit", "-qm", "more"]);
    repo
}

#[test]
fn lists_every_index_version() {
    for version in ["2", "3", "4"] {
        let repo = make_big_repo(&format!("version-{version}"), &[]);
        git(&repo.0, &["update-index", "--index-version", version]);
        assert_lists_like_git(&repo.0);
    }
}

#[test]
fn lists_a_split_index() {
    let repo = make_big_repo("split", &[]);
    git(&repo.0, &["update-index", "--split-index"]);
    // Replace, delete, and add entries in the split index, leaving the
    // shared index as it was.
    std::fs::write(repo.0.join("b/1"), "changed\n").unwrap();
    std::fs::write(repo.0.join("b/new"), "new\n").unwrap();
    git(&repo.0, &["add", "b"]);
    git(&repo.0, &["rm", "-q", "--cached", "b/2", "a/3/x"]);
    assert!(has_extension(&repo.0, b"link"));
    let files = assert_lists_like_git(&repo.0);
    assert!(files.contains(&b"b/new".to_vec()));
    assert!(!files.contains(&b"b/2".to_vec()));
}

#[test]
fn lists_a_sparse_index() {
    let repo = make_big_repo("sparse-index", &[]);
    git(
        &repo.0,
        &["sparse-checkout", "set", "--cone", "--sparse-index", "a/1"],
    );
    assert!(has_extension(&repo.0, b"sdir"));
    let files = assert_lists_like_git(&repo.0);
    assert!(files.contains(&b"a/2/y/z".to_vec()));
}

#[test]
fn lists_an_index_with_optional_extensions() {
    let repo = make_big_repo("extensions", &[]);
    for (key, value) in [
        ("index.recordEndOfIndexEntries", "true"),
        ("index.recordOffsetTable", "true"),
        ("index.threads", "4"),
        ("core.untrackedCache", "true"),
    ] {
        git(&repo.0, &["config", key, value]);
    }
    // Git only writes the offset table for an index big enough for threads.
    let blob = String::from_utf8(git(&repo.0, &["rev-parse", "HEAD:b/0"])).unwrap();
    let entries: String = (0..25_000)
        .map(|i| format!("100644 {} 0\tmany/{i}\n", blob.trim()))
        .collect();
    git_with_input(&repo.0, &["update-index", "--index-info"], &entries);
    std::fs::write(repo.0.join("untracked"), "untracked\n").unwrap();
    git(&repo.0, &["status", "-s"]);
    for signature in [b"TREE", b"UNTR", b"EOIE", b"IEOT"] {
        let name = String::from_utf8_lossy(signature);
        assert!(has_extension(&repo.0, signature), "{name}");
    }
    assert_lists_like_git(&repo.0);
}

#[test]
fn lists_an_index_without_a_checksum() {
    let repo = make_big_repo("skip-hash", &[]);
    git(&repo.0, &["config", "index.skipHash", "true"]);
    git(&repo.0, &["update-index", "--force-write-index"]);
    assert_lists_like_git(&repo.0);
}

#[test]
fn lists_a_sha256_repository() {
    let repo = make_big_repo("sha256", &["--object-format=sha256"]);
    git(
        &repo.0,
        &["sparse-checkout", "set", "--cone", "--sparse-index", "a/1"],
    );
    assert_lists_like_git(&repo.0);
}

#[test]
fn lists_resolved_conflicts() {
    let repo = make_repo("resolve-undo");
    git(&repo.0, &["checkout", "-qb", "other"]);
    std::fs::write(repo.0.join("z"), "other\n").unwrap();
    git(&repo.0, &["commit", "-qam", "other"]);
    git(&repo.0, &["checkout", "-q", "main"]);
    std::fs::write(repo.0.join("z"), "main\n").unwrap();
    git(&repo.0, &["commit", "-qam", "main"]);
    git(&repo.0, &["merge", "-q", "other"]);
    assert_lists_like_git(&repo.0);
    // Resolving the conflict leaves a resolve-undo extension.
    git(&repo.0, &["add", "z"]);
    assert!(has_extension(&repo.0, b"REUC"));
    assert_lists_like_git(&repo.0);
}

#[test]
fn lists_from_a_subdirectory() {
    let repo = make_big_repo("subdirectory", &[]);
    let files = assert_lists_like_git(&repo.0.join("a"));
    assert!(files.contains(&b"1/x".to_vec()));
    git(
        &repo.0,
        &["sparse-checkout", "set", "--cone", "--sparse-index", "b"],
    );
    assert_lists_like_git(&repo.0.join("b"));
}

#[test]
fn lists_a_linked_worktree() {
    let repo = make_big_repo("worktree", &[]);
    let linked = empty_dir("worktree-linked");
    std::fs::remove_dir(&linked.0).unwrap();
    git(
        &repo.0,
        &["worktree", "add", "-q", linked.0.to_str().unwrap()],
    );
    git(&linked.0, &["rm", "-q", "--cached", "b/1"]);
    assert_lists_like_git(&linked.0);
    assert_lists_like_git(&repo.0);
}

#[test]
fn fails_in_a_bare_repository() {
    let repo = empty_dir("bare");
    git(&repo.0, &["init", "-q", "--bare"]);
    assert!(tracked_files(&repo.0).is_err());
}

#[test]
#[ignore = "gix-index 0.58 misreads the entry after a path of 4095 bytes or more"]
fn lists_a_path_too_long_for_its_length_field() {
    // The index stores paths of 4095 bytes or more with a special length.
    // They're too long for some file systems, so only the index has it.
    for version in ["2", "3", "4"] {
        let repo = make_repo(&format!("long-{version}"));
        let blob = String::from_utf8(git(&repo.0, &["rev-parse", "HEAD:z"])).unwrap();
        let long = format!("{}/{}", "d".repeat(200), "e".repeat(250)).repeat(10);
        for path in [long.clone(), format!("{long}x"), "zz".into()] {
            let cacheinfo = format!("100644,{},{path}", blob.trim());
            git(
                &repo.0,
                &["update-index", "--add", "--cacheinfo", &cacheinfo],
            );
        }
        git(&repo.0, &["update-index", "--index-version", version]);
        assert_lists_like_git(&repo.0);
    }
}

#[cfg(unix)]
#[test]
fn lists_an_index_with_a_file_system_monitor() {
    use std::os::unix::fs::PermissionsExt;
    let repo = make_big_repo("fsmonitor", &[]);
    // A hook speaking version 2 of the protocol, which says everything
    // might have changed.
    let hook = repo.0.join(".git/fsmonitor");
    std::fs::write(&hook, "#!/bin/sh\nprintf 'token\\0/\\0'\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(
        &repo.0,
        &["config", "core.fsmonitor", hook.to_str().unwrap()],
    );
    git(&repo.0, &["update-index", "--fsmonitor"]);
    git(&repo.0, &["status", "-s"]);
    assert!(has_extension(&repo.0, b"FSMN"));
    assert_lists_like_git(&repo.0);
}
