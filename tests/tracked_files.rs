use std::path::Path;

use gh_codeowners::counter::Rule;
use gh_codeowners::{TrackedFiles, count_tracked_files, index_path};
use tempfile::TempDir;

mod common;
use common::{git, git_output, write};

fn empty_dir(name: &str) -> TempDir {
    TempDir::with_prefix(format!("gh-codeowners-{name}-")).unwrap()
}

fn make_repo(name: &str) -> TempDir {
    let repo = empty_dir(name);
    git(repo.path(), &["init", "-q", "-b", "main"]);
    for file in ["a/1", "a/b/2", "c/3", "é.txt", "with space", "z"] {
        write(repo.path(), file, &format!("{file}\n"));
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
    // Conflicts, on purpose.
    git_output(repo.path(), &["merge", "-q", "other"]);
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
    assert!(error.starts_with("Couldn't read git's index"), "{error}");
}

/// Run git in `dir`, which must succeed, for what it prints.
fn git_stdout(dir: &Path, args: &[&str]) -> Vec<u8> {
    let output = git_output(dir, args);
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn git_with_input(dir: &Path, args: &[&str], input: &str) {
    use std::io::Write;
    let mut child = std::process::Command::new("git")
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

/// Assert that we list the files `git ls-files` does in `dir`.
fn assert_lists_like_git(dir: &Path) -> Vec<Vec<u8>> {
    let mut expected: Vec<Vec<u8>> = git_stdout(dir, &["ls-files", "-z"])
        .split(|&b| b == 0)
        .map(<[u8]>::to_vec)
        .collect();
    expected.pop();
    expected.dedup();
    let files = TrackedFiles::read(dir).unwrap();
    let actual: Vec<Vec<u8>> = files.paths().into_iter().map(<[u8]>::to_vec).collect();
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
fn make_big_repo(name: &str, init: &[&str]) -> TempDir {
    let repo = empty_dir(name);
    let mut args = vec!["init", "-q", "-b", "main"];
    args.extend_from_slice(init);
    git(repo.path(), &args);
    for i in 0..40 {
        for file in [
            format!("a/{i}/x"),
            format!("a/{i}/y/z"),
            format!("b/{i}"),
            format!("c-{i}"),
        ] {
            let path = repo.path().join(&file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("{file}\n")).unwrap();
        }
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("b/0", repo.path().join("link")).unwrap();
    git(repo.path(), &["add", "."]);
    git(repo.path(), &["commit", "-qm", "init"]);
    let commit = String::from_utf8(git_stdout(repo.path(), &["rev-parse", "HEAD"])).unwrap();
    let cacheinfo = format!("160000,{},sub", commit.trim());
    git(
        repo.path(),
        &["update-index", "--add", "--cacheinfo", &cacheinfo],
    );
    std::fs::write(repo.path().join("intended"), "intended\n").unwrap();
    git(repo.path(), &["add", "-N", "intended"]);
    git(repo.path(), &["commit", "-qm", "more"]);
    repo
}

#[test]
fn lists_every_index_version() {
    for version in ["2", "3", "4"] {
        let repo = make_big_repo(&format!("version-{version}"), &[]);
        git(repo.path(), &["update-index", "--index-version", version]);
        assert_lists_like_git(repo.path());
    }
}

#[test]
fn lists_a_split_index() {
    let repo = make_big_repo("split", &[]);
    git(repo.path(), &["update-index", "--split-index"]);
    // Replace, delete, and add entries in the split index, leaving the
    // shared index as it was.
    std::fs::write(repo.path().join("b/1"), "changed\n").unwrap();
    std::fs::write(repo.path().join("b/new"), "new\n").unwrap();
    git(repo.path(), &["add", "b"]);
    git(repo.path(), &["rm", "-q", "--cached", "b/2", "a/3/x"]);
    assert!(has_extension(repo.path(), b"link"));
    let files = assert_lists_like_git(repo.path());
    assert!(files.contains(&b"b/new".to_vec()));
    assert!(!files.contains(&b"b/2".to_vec()));
}

#[test]
fn lists_a_sparse_index() {
    let repo = make_big_repo("sparse-index", &[]);
    git(
        repo.path(),
        &["sparse-checkout", "set", "--cone", "--sparse-index", "a/1"],
    );
    assert!(has_extension(repo.path(), b"sdir"));
    let files = assert_lists_like_git(repo.path());
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
        git(repo.path(), &["config", key, value]);
    }
    // Git only writes the offset table for an index big enough for threads.
    let blob = String::from_utf8(git_stdout(repo.path(), &["rev-parse", "HEAD:b/0"])).unwrap();
    let entries: String = (0..25_000)
        .map(|i| format!("100644 {} 0\tmany/{i}\n", blob.trim()))
        .collect();
    git_with_input(repo.path(), &["update-index", "--index-info"], &entries);
    std::fs::write(repo.path().join("untracked"), "untracked\n").unwrap();
    git(repo.path(), &["status", "-s"]);
    for signature in [b"TREE", b"UNTR", b"EOIE", b"IEOT"] {
        let name = String::from_utf8_lossy(signature);
        assert!(has_extension(repo.path(), signature), "{name}");
    }
    assert_lists_like_git(repo.path());
}

#[test]
fn lists_an_index_without_a_checksum() {
    let repo = make_big_repo("skip-hash", &[]);
    git(repo.path(), &["config", "index.skipHash", "true"]);
    git(repo.path(), &["update-index", "--force-write-index"]);
    assert_lists_like_git(repo.path());
}

#[test]
fn lists_a_sha256_repository() {
    let repo = make_big_repo("sha256", &["--object-format=sha256"]);
    git(
        repo.path(),
        &["sparse-checkout", "set", "--cone", "--sparse-index", "a/1"],
    );
    assert_lists_like_git(repo.path());
}

#[test]
fn lists_resolved_conflicts() {
    let repo = make_repo("resolve-undo");
    git(repo.path(), &["checkout", "-qb", "other"]);
    std::fs::write(repo.path().join("z"), "other\n").unwrap();
    git(repo.path(), &["commit", "-qam", "other"]);
    git(repo.path(), &["checkout", "-q", "main"]);
    std::fs::write(repo.path().join("z"), "main\n").unwrap();
    git(repo.path(), &["commit", "-qam", "main"]);
    // Conflicts, on purpose.
    git_output(repo.path(), &["merge", "-q", "other"]);
    assert_lists_like_git(repo.path());
    // Resolving the conflict leaves a resolve-undo extension.
    git(repo.path(), &["add", "z"]);
    assert!(has_extension(repo.path(), b"REUC"));
    assert_lists_like_git(repo.path());
}

#[test]
fn lists_from_a_subdirectory() {
    let repo = make_big_repo("subdirectory", &[]);
    let files = assert_lists_like_git(&repo.path().join("a"));
    assert!(files.contains(&b"1/x".to_vec()));
    git(
        repo.path(),
        &["sparse-checkout", "set", "--cone", "--sparse-index", "b"],
    );
    assert_lists_like_git(&repo.path().join("b"));
}

#[test]
fn lists_a_linked_worktree() {
    let repo = make_big_repo("worktree", &[]);
    let linked = empty_dir("worktree-linked");
    std::fs::remove_dir(linked.path()).unwrap();
    git(
        repo.path(),
        &["worktree", "add", "-q", linked.path().to_str().unwrap()],
    );
    git(linked.path(), &["rm", "-q", "--cached", "b/1"]);
    assert_lists_like_git(linked.path());
    assert_lists_like_git(repo.path());
}

#[test]
fn fails_in_a_bare_repository() {
    let repo = empty_dir("bare");
    git(repo.path(), &["init", "-q", "--bare"]);
    assert!(TrackedFiles::read(repo.path()).is_err());
}

#[test]
#[ignore = "gix-index 0.58 misreads the entry after a path of 4095 bytes or more"]
fn lists_a_path_too_long_for_its_length_field() {
    // The index stores paths of 4095 bytes or more with a special length.
    // They're too long for some file systems, so only the index has it.
    for version in ["2", "3", "4"] {
        let repo = make_repo(&format!("long-{version}"));
        let blob = String::from_utf8(git_stdout(repo.path(), &["rev-parse", "HEAD:z"])).unwrap();
        let long = format!("{}/{}", "d".repeat(200), "e".repeat(250)).repeat(10);
        for path in [long.clone(), format!("{long}x"), "zz".into()] {
            let cacheinfo = format!("100644,{},{path}", blob.trim());
            git(
                repo.path(),
                &["update-index", "--add", "--cacheinfo", &cacheinfo],
            );
        }
        git(repo.path(), &["update-index", "--index-version", version]);
        assert_lists_like_git(repo.path());
    }
}

#[cfg(unix)]
#[test]
fn lists_an_index_with_a_file_system_monitor() {
    use std::os::unix::fs::PermissionsExt;
    let repo = make_big_repo("fsmonitor", &[]);
    // A hook speaking version 2 of the protocol, which says everything
    // might have changed.
    let hook = repo.path().join(".git/fsmonitor");
    std::fs::write(&hook, "#!/bin/sh\nprintf 'token\\0/\\0'\n").unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(
        repo.path(),
        &["config", "core.fsmonitor", hook.to_str().unwrap()],
    );
    git(repo.path(), &["update-index", "--fsmonitor"]);
    git(repo.path(), &["status", "-s"]);
    assert!(has_extension(repo.path(), b"FSMN"));
    assert_lists_like_git(repo.path());
}

#[test]
fn finds_the_index_where_git_does() {
    let repo = make_repo("index-path");
    let linked = empty_dir("index-path-linked");
    std::fs::remove_dir(linked.path()).unwrap();
    git(
        repo.path(),
        &["worktree", "add", "-q", linked.path().to_str().unwrap()],
    );
    for dir in [repo.path(), linked.path(), &repo.path().join("a")] {
        let expected = git_stdout(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        );
        let expected = std::fs::canonicalize(String::from_utf8(expected).unwrap().trim()).unwrap();
        let actual = std::fs::canonicalize(index_path(dir).unwrap()).unwrap();
        assert_eq!(actual, expected);
    }
    assert_eq!(index_path(empty_dir("no-index-path").path()), None);
}
