use std::path::{Path, PathBuf};
use std::process::Command;

use insta::assert_snapshot;
use tempfile::TempDir;

mod common;
use common::{git, write};

type Repo = TempDir;

fn repo(name: &str) -> Repo {
    let repo = TempDir::with_prefix(format!("gh-codeowners-cli-{name}-")).unwrap();
    let dir = repo.path();
    let write = |file: &str, text: &str| write(dir, file, text);
    write(
        ".github/CODEOWNERS",
        "*           @acme/platform\n/docs/      @acme/docs\n*.md        @acme/writers\n/missing/   @acme/nobody",
    );
    write("Makefile", "");
    write("docs/guide.md", "");
    write("src/main.ts", "");
    write("build/out.js", "");
    write(".gitignore", "build/\n");
    git(dir, &["init", "-q"]);
    git(dir, &["add", "."]);
    // Only files in git's index count.
    write("notes.md", "");
    repo
}

fn run(repo: &Repo, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_gh-codeowners"))
        .args(args)
        .current_dir(repo.path())
        .output()
        .unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn prints_a_count_for_each_rule() {
    let (status, stdout, stderr) = run(&repo("text"), &["list"]);
    assert_eq!((status, stderr.as_str()), (0, ""));
    assert_snapshot!(stdout, @r"
    .github/CODEOWNERS: 4 rules, 5 files, 0 files with no owner, 2 rules owning no files

    line  files  pattern
       1      4  *
       2      0  /docs/     every matching file is owned by a later rule
       3      1  *.md
       4      0  /missing/  no files match
    ");
}

#[test]
fn check_fails_when_a_rule_owns_no_files() {
    assert_eq!(run(&repo("check"), &["check", "--unused"]).0, 1);
}

#[test]
fn a_closed_stdout_is_not_an_error() {
    let repo = repo("closed-stdout");
    let mut child = Command::new(env!("CARGO_BIN_EXE_gh-codeowners"))
        .args(["check", "--unused"])
        .current_dir(repo.path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // Close the read end before anything is written, like `| head -0`.
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    // check's exit code survives the closed pipe.
    assert_eq!(output.status.code(), Some(1));
}

#[test]
fn json_unused_lists_only_rules_that_own_nothing() {
    let (status, stdout, _) = run(&repo("json"), &["list", "--json", "--unused"]);
    assert_eq!(status, 0);
    assert_snapshot!(stdout, @r#"
    [
      {
        "line": 2,
        "pattern": "/docs/",
        "owners": [
          "@acme/docs"
        ],
        "files": 0,
        "status": "overridden"
      },
      {
        "line": 4,
        "pattern": "/missing/",
        "owners": [
          "@acme/nobody"
        ],
        "files": 0,
        "status": "unmatched"
      }
    ]
    "#);
}

#[test]
fn exits_with_2_when_theres_no_codeowners() {
    let empty = TempDir::new().unwrap();
    let (status, _, stderr) = run(
        &repo("missing"),
        &["list", "--root", empty.path().to_str().unwrap()],
    );
    assert_eq!(status, 2);
    assert_eq!(
        stderr,
        format!(
            "gh-codeowners: No CODEOWNERS file found in {}\n",
            empty.path().display()
        )
    );
}

#[test]
fn rejects_codeowners_files_github_ignores_for_size() {
    let repo = repo("size");
    // GitHub loads a CODEOWNERS file of exactly 3 MiB, but not one byte more.
    let limit = 3 * 1024 * 1024;
    let mut text = "* @acme/platform\n".to_string();
    text.push_str(&"#".repeat(limit - text.len() - 1));
    text.push('\n');
    std::fs::write(repo.path().join(".github/CODEOWNERS"), &text).unwrap();
    assert_eq!(run(&repo, &["list"]).0, 0);
    text.push('#');
    std::fs::write(repo.path().join(".github/CODEOWNERS"), &text).unwrap();
    let (status, _, stderr) = run(&repo, &["list"]);
    assert_eq!(status, 2);
    assert!(
        stderr.ends_with(&format!(
            ": .github/CODEOWNERS is {} bytes, over GitHub's limit of {limit}, so GitHub ignores it\n",
            limit + 1
        )),
        "{stderr}"
    );
}

fn run_with_stdin(repo: &Repo, args: &[&str], stdin: &str) -> (i32, String, String) {
    use std::io::Write as _;
    let mut child = Command::new(env!("CARGO_BIN_EXE_gh-codeowners"))
        .args(args)
        .current_dir(repo.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn owner_of_prints_the_rule_that_owns_each_file() {
    let repo = repo("owner-of");
    // An absolute path, and files that aren't tracked or don't exist.
    let main = repo.path().join("src/main.ts");
    let (status, stdout, _) = run(
        &repo,
        &[
            "view",
            "docs/guide.md",
            main.to_str().unwrap(),
            "notes.md",
            "./docs/new",
        ],
    );
    assert_eq!(status, 0);
    assert_snapshot!(stdout, @r"
    docs/guide.md: @acme/writers, from .github/CODEOWNERS:3 *.md
    src/main.ts: @acme/platform, from .github/CODEOWNERS:1 *
    notes.md: @acme/writers, from .github/CODEOWNERS:3 *.md
    docs/new: @acme/docs, from .github/CODEOWNERS:2 /docs/
    ");
}

#[test]
fn owner_of_json_has_null_for_files_no_rule_matches() {
    let repo = repo("owner-of-json");
    let (status, stdout, _) = run_with_stdin(
        &repo,
        &[
            "view",
            "--codeowners",
            "-",
            "--json",
            "docs/a.md",
            "src/main.ts",
        ],
        "/docs/ @acme/docs\n",
    );
    assert_eq!(status, 0);
    assert_snapshot!(stdout, @r#"
    [
      {
        "path": "docs/a.md",
        "line": 1,
        "pattern": "/docs/",
        "owners": [
          "@acme/docs"
        ]
      },
      {
        "path": "src/main.ts",
        "line": null,
        "pattern": null,
        "owners": []
      }
    ]
    "#);
}

#[test]
fn errors_lists_problems_github_would_have() {
    let repo = repo("errors");
    let (status, stdout, _) = run_with_stdin(
        &repo,
        &["check", "--codeowners", "-"],
        "*.js @acme/js\n[x] @acme/x\n!foo @acme/foo\n",
    );
    assert_eq!(status, 1);
    assert_snapshot!(stdout, @r"
    error: `[x]` isn't a valid pattern
     --> <stdin>:2:1
      |
    2 | [x] @acme/x
      | ^^^ both `[` and `]` must be escaped with a `\`, like `\[x\]`

    warning: `!` doesn't negate a pattern on GitHub
     --> <stdin>:3:1
      |
    3 | !foo @acme/foo
      | ^^^^ this matches files named like the rest of the pattern, but not directories
    ");
    let (status, stdout, _) = run(&repo, &["check"]);
    assert_eq!(
        (status, stdout.as_str()),
        (0, ".github/CODEOWNERS: no problems\n")
    );
}

#[test]
fn counting_lists_problems_after_the_counts() {
    let repo = repo("count-problems");
    let (status, stdout, _) = run_with_stdin(
        &repo,
        &["list", "--codeowners", "-"],
        "* @acme/all\n[x] @acme/x\n*.md not-an-owner\n",
    );
    assert_eq!(status, 0);
    assert_snapshot!(stdout, @r"
    <stdin>: 2 rules, 5 files, 0 files with no owner, 1 rule owning no files

    line  files  pattern
       1      5  *
       2      0  [x]      no files match

    error: `[x]` isn't a valid pattern
     --> <stdin>:2:1
      |
    2 | [x] @acme/x
      | ^^^ both `[` and `]` must be escaped with a `\`, like `\[x\]`

    error: `not-an-owner` isn't a valid owner
     --> <stdin>:3:6
      |
    3 | *.md not-an-owner
      |      ^^^^^^^^^^^^ owners must be @user, @org/team or an email address
    ");
}

#[test]
fn aligns_wide_characters_in_patterns() {
    let (status, stdout, _) = run_with_stdin(
        &repo("wide"),
        &["list", "--codeowners", "-"],
        "* @acme/all\n/文档目录/ @acme/docs\n/missing/ @acme/nobody\n",
    );
    assert_eq!(status, 0);
    assert_snapshot!(stdout, @r"
    <stdin>: 3 rules, 5 files, 0 files with no owner, 2 rules owning no files

    line  files  pattern
       1      5  *
       2      0  /文档目录/  no files match
       3      0  /missing/   no files match
    ");
}

#[test]
fn check_json_lists_problems() {
    let (status, stdout, _) = run_with_stdin(
        &repo("check-json"),
        &["check", "--codeowners", "-", "--json"],
        "[x] @acme/x\n",
    );
    assert_eq!(status, 1);
    assert_snapshot!(stdout, @r#"
    [
      {
        "line": 1,
        "severity": "error",
        "message": "`[x]` isn't a valid pattern - both `[` and `]` must be escaped with a `\\`, like `\\[x\\]`"
      }
    ]
    "#);
}

#[test]
fn check_unused_reports_rules_that_own_no_files() {
    let (status, stdout, _) = run(&repo("check-unused"), &["check", "--unused"]);
    assert_eq!(status, 1);
    assert_snapshot!(stdout, @r"
    error: `/docs/` owns no files
     --> .github/CODEOWNERS:2:1
      |
    2 | /docs/      @acme/docs
      | ^^^^^^ every matching file is owned by a later rule

    error: `/missing/` owns no files
     --> .github/CODEOWNERS:4:1
      |
    4 | /missing/   @acme/nobody
      | ^^^^^^^^^ no files match
    ");
}

#[test]
fn check_underlines_words_after_crlf_and_non_ascii_text() {
    let (status, stdout, _) = run_with_stdin(
        &repo("check-spans"),
        &["check", "--codeowners", "-"],
        "# 日本語\r\n文档\\ 目录 @acme/x 不是 # 注释\r\n\r\n/日本語/[x] @acme/y\r\n..",
    );
    assert_eq!(status, 1);
    assert_snapshot!(stdout, @r"
    error: `不是` isn't a valid owner
     --> <stdin>:2:16
      |
    2 | 文档\ 目录 @acme/x 不是 # 注释
      |                    ^^^^ owners must be @user, @org/team or an email address

    error: `/日本語/[x]` isn't a valid pattern
     --> <stdin>:4:1
      |
    4 | /日本語/[x] @acme/y
      | ^^^^^^^^^^^ both `[` and `]` must be escaped with a `\`, like `/日本語/\[x\]`

    warning: paths in git never have `.` or `..` segments, so GitHub matches nothing
     --> <stdin>:5:1
      |
    5 | ..
      | ^^
    ");
}

#[test]
fn check_reports_files_too_big_for_github_without_a_snippet() {
    let repo = repo("check-size");
    std::fs::write(
        repo.path().join(".github/CODEOWNERS"),
        "#".repeat(3 * 1024 * 1024 + 1),
    )
    .unwrap();
    let (status, stdout, _) = run(&repo, &["check"]);
    assert_eq!(status, 1);
    assert_snapshot!(stdout, @r"
    error: GitHub ignores CODEOWNERS files over 3145728 bytes, and this one is 3145729 bytes
    --> .github/CODEOWNERS
    ");
}

#[test]
fn colors_only_for_terminals() {
    let repo = repo("colors");
    let (_, plain, _) = run(&repo, &["check", "--unused"]);
    assert!(!plain.contains('\x1b'), "{plain}");
    // Stdout is a pipe here, so ask for what a terminal gets.
    let output = Command::new(env!("CARGO_BIN_EXE_gh-codeowners"))
        .args(["check", "--unused"])
        .env("CLICOLOR_FORCE", "1")
        .env_remove("NO_COLOR")
        .current_dir(repo.path())
        .output()
        .unwrap();
    let styled = String::from_utf8(output.stdout).unwrap();
    assert!(styled.starts_with("\x1b["), "{styled}");
}

#[test]
fn check_fails_when_a_line_has_an_error() {
    let repo = repo("check-errors");
    // The only rule owns every file, so the invalid owner on line 2 is what
    // fails the check.
    let (status, _, _) = run_with_stdin(
        &repo,
        &["check", "--codeowners", "-"],
        "* @acme/all\n*.md not-an-owner\n",
    );
    assert_eq!(status, 1);
    let (status, _, _) = run_with_stdin(&repo, &["check", "--codeowners", "-"], "* @acme/all\n");
    assert_eq!(status, 0);
}

#[test]
fn rejects_arguments_a_command_doesnt_take() {
    let repo = repo("conflicts");
    assert_eq!(run(&repo, &["view", "--unused", "a"]).0, 2);
    assert_eq!(run(&repo, &["list", "a"]).0, 2);
    assert_eq!(run(&repo, &["view"]).0, 2);
    assert_eq!(run(&repo, &["show"]).0, 2);
}

#[test]
fn prints_help_without_a_command() {
    let (status, _, stderr) = run(&repo("help"), &[]);
    assert_eq!(status, 2);
    assert!(
        stderr.starts_with("Work with GitHub CODEOWNERS files\n\nUsage: gh-codeowners "),
        "{stderr}"
    );
}

#[test]
fn calls_itself_gh_codeowners_as_a_gh_extension() {
    let repo = repo("extension");
    let output = Command::new(env!("CARGO_BIN_EXE_gh-codeowners"))
        .args(["view"])
        .env("GH_EXTENSION", "1")
        .current_dir(repo.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Usage: gh codeowners view"), "{stderr}");
}

fn run_in(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_gh-codeowners"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    (
        output.status.code().unwrap(),
        String::from_utf8(output.stdout).unwrap(),
        String::from_utf8(output.stderr).unwrap(),
    )
}

#[test]
fn works_from_a_subdirectory() {
    let repo = repo("subdirectory");
    let docs = repo.path().join("docs");
    let (status, stdout, _) = run_in(&docs, &["list"]);
    assert_eq!(status, 0);
    assert!(
        stdout.starts_with(".github/CODEOWNERS: 4 rules, 5 files"),
        "{stdout}"
    );
    // Relative paths are relative to the current directory.
    let (status, stdout, _) = run_in(&docs, &["view", "guide.md", "../src/main.ts", "./new/file"]);
    assert_eq!(status, 0);
    assert_snapshot!(stdout, @r"
    docs/guide.md: @acme/writers, from .github/CODEOWNERS:3 *.md
    src/main.ts: @acme/platform, from .github/CODEOWNERS:1 *
    docs/new/file: @acme/docs, from .github/CODEOWNERS:2 /docs/
    ");
    assert_eq!(
        run_in(&docs, &["check"]).1,
        ".github/CODEOWNERS: no problems\n"
    );
}

#[test]
fn owner_of_treats_directories_as_directories() {
    let repo = repo("owner-of-dir");
    // /docs/ only matches directories, so it owns docs/ but not a file named
    // docs.
    let (status, stdout, _) = run(&repo, &["view", "docs", "src/docs"]);
    assert_eq!(status, 0);
    assert_snapshot!(stdout, @r"
    docs/: @acme/docs, from .github/CODEOWNERS:2 /docs/
    src/docs: @acme/platform, from .github/CODEOWNERS:1 *
    ");
}

/// A symlink to `repo`, for spelling its path another way, in a directory
/// that's removed with it.
#[cfg(unix)]
fn symlink_to(repo: &Repo) -> (TempDir, PathBuf) {
    let dir = TempDir::new().unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(repo.path(), &link).unwrap();
    (dir, link)
}

#[cfg(unix)]
#[test]
fn view_accepts_a_root_or_path_spelled_through_a_symlink() {
    let repo = repo("view-root-symlink");
    let (_dir, link) = symlink_to(&repo);
    let through_link = link.join("src/main.ts");
    for args in [
        vec!["view", "--root", link.to_str().unwrap(), "src/main.ts"],
        vec!["view", through_link.to_str().unwrap()],
    ] {
        let (status, stdout, stderr) = run(&repo, &args);
        assert_eq!((status, stderr.as_str()), (0, ""));
        assert_eq!(
            stdout,
            "src/main.ts: @acme/platform, from .github/CODEOWNERS:1 *\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn view_keeps_symlinks_inside_the_repository_as_written() {
    let repo = repo("view-inner-symlink");
    std::os::unix::fs::symlink("src", repo.path().join("link")).unwrap();
    let (_dir, link) = symlink_to(&repo);
    // Git tracks a symlink as a file, so its path is the repository path,
    // however the root is spelled.
    for args in [
        vec!["view", "link/main.ts"],
        vec!["view", "--root", link.to_str().unwrap(), "link/main.ts"],
    ] {
        let (status, stdout, stderr) = run(&repo, &args);
        assert_eq!((status, stderr.as_str()), (0, ""));
        assert_eq!(
            stdout,
            "link/main.ts: @acme/platform, from .github/CODEOWNERS:1 *\n"
        );
    }
}

#[test]
fn warns_when_github_ignores_rules_for_an_empty_codeowners() {
    let repo = repo("ignored");
    std::fs::write(repo.path().join(".github/CODEOWNERS"), "\n").unwrap();
    std::fs::write(repo.path().join("CODEOWNERS"), "* @acme/platform\n").unwrap();
    let warning = ".github/CODEOWNERS has no rules, but GitHub uses it instead of CODEOWNERS, so no file has an owner";
    let (status, stdout, stderr) = run(&repo, &["list"]);
    assert_eq!(status, 0);
    assert!(
        stdout.starts_with(".github/CODEOWNERS: 0 rules"),
        "{stdout}"
    );
    assert_eq!(stderr, format!("gh-codeowners: warning: {warning}\n"));
    let (_, stdout, _) = run(&repo, &["check"]);
    assert_eq!(
        stdout,
        "warning: this file has no rules, but GitHub uses it instead of CODEOWNERS, so no file has an owner\n--> .github/CODEOWNERS\n"
    );
    // Not for a file named with --codeowners, which GitHub may not use.
    let (_, _, stderr) = run(&repo, &["list", "--codeowners", ".github/CODEOWNERS"]);
    assert_eq!(stderr, "");
}

#[test]
fn fmt_formats_codeowners_in_place() {
    let repo = repo("fmt");
    let codeowners = repo.path().join(".github/CODEOWNERS");
    std::fs::write(
        &codeowners,
        "  # Docs\n/docs/   @acme/docs   # guides\n*.md @acme/writers\n",
    )
    .unwrap();
    // --check reports the file without changing it.
    let (status, stdout, stderr) = run(&repo, &["fmt", "--check"]);
    assert_eq!(
        (status, stdout.as_str(), stderr.as_str()),
        (1, ".github/CODEOWNERS\n", "")
    );
    let (status, stdout, stderr) = run(&repo, &["fmt"]);
    assert_eq!(
        (status, stdout.as_str(), stderr.as_str()),
        (0, ".github/CODEOWNERS\n", "")
    );
    assert_snapshot!(std::fs::read_to_string(&codeowners).unwrap(), @r"
    # Docs
    /docs/ @acme/docs # guides
    *.md   @acme/writers
    ");
    // Nothing more to do.
    assert_eq!(run(&repo, &["fmt"]), (0, String::new(), String::new()));
    assert_eq!(
        run(&repo, &["fmt", "--check"]),
        (0, String::new(), String::new())
    );
}

#[test]
fn fmt_formats_stdin_to_stdout() {
    let repo = repo("fmt-stdin");
    let (status, stdout, _) =
        run_with_stdin(&repo, &["fmt", "--codeowners", "-"], "*   @a\n/docs/ @b");
    assert_eq!((status, stdout.as_str()), (0, "*      @a\n/docs/ @b"));
    let (status, stdout, _) =
        run_with_stdin(&repo, &["fmt", "--check", "--codeowners", "-"], "*   @a\n");
    assert_eq!((status, stdout.as_str()), (1, ""));
    // --json is only on the commands that print JSON.
    let (status, _, stderr) = run(&repo, &["fmt", "--json"]);
    assert_eq!(status, 2);
    assert!(stderr.contains("unexpected argument '--json'"), "{stderr}");
}
