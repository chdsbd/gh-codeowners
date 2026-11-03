use std::path::{Path, PathBuf};
use std::process::Command;

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .unwrap();
    assert!(status.success());
}

fn repo(name: &str) -> Repo {
    let dir = std::env::temp_dir().join(format!("gh-codeowners-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let write = |file: &str, text: &str| {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        ".github/CODEOWNERS",
        "*           @acme/platform\n/docs/      @acme/docs\n*.md        @acme/writers\n/missing/   @acme/nobody",
    );
    write("Makefile", "");
    write("docs/guide.md", "");
    write("src/main.ts", "");
    write("build/out.js", "");
    write(".gitignore", "build/\n");
    git(&dir, &["init", "-q"]);
    git(&dir, &["add", "."]);
    // Only files in git's index count.
    write("notes.md", "");
    Repo(dir)
}

fn run(repo: &Repo, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_gh-codeowners"))
        .args(args)
        .current_dir(&repo.0)
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
    assert_eq!(
        run(&repo("text"), &["list"]),
        (
            0,
            [
                ".github/CODEOWNERS: 4 rules, 5 files, 0 files with no owner, 2 rules owning no files",
                "",
                "line  files  pattern",
                "   1      4  *",
                "   2      0  /docs/     every matching file is owned by a later rule",
                "   3      1  *.md",
                "   4      0  /missing/  no files match",
                "",
            ]
            .join("\n"),
            String::new()
        )
    );
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
        .current_dir(&repo.0)
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
    assert_eq!(
        stdout,
        r#"[
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
"#
    );
}

#[test]
fn exits_with_2_when_theres_no_codeowners() {
    let empty =
        std::env::temp_dir().join(format!("gh-codeowners-cli-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let (status, _, stderr) = run(
        &repo("missing"),
        &["list", "--root", empty.to_str().unwrap()],
    );
    std::fs::remove_dir_all(&empty).unwrap();
    assert_eq!(status, 2);
    assert_eq!(
        stderr,
        format!(
            "gh-codeowners: No CODEOWNERS file found in {}\n",
            empty.display()
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
    std::fs::write(repo.0.join(".github/CODEOWNERS"), &text).unwrap();
    assert_eq!(run(&repo, &["list"]).0, 0);
    text.push('#');
    std::fs::write(repo.0.join(".github/CODEOWNERS"), &text).unwrap();
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
        .current_dir(&repo.0)
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
    let main = repo.0.join("src/main.ts");
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
    assert_eq!(
        stdout,
        [
            "docs/guide.md: @acme/writers, from .github/CODEOWNERS:3 *.md",
            "src/main.ts: @acme/platform, from .github/CODEOWNERS:1 *",
            "notes.md: @acme/writers, from .github/CODEOWNERS:3 *.md",
            "docs/new: @acme/docs, from .github/CODEOWNERS:2 /docs/",
            "",
        ]
        .join("\n")
    );
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
    assert_eq!(
        stdout,
        r#"[
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
"#
    );
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
    assert_eq!(
        stdout,
        [
            "<stdin>:2: error: `[x]` isn't a valid pattern - both `[` and `]` must be escaped with a `\\`, like `\\[x\\]`",
            "<stdin>:3: warning: `!` doesn't negate a pattern on GitHub - this matches files named like the rest of the pattern, but not directories",
            "",
        ]
        .join("\n")
    );
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
    assert_eq!(
        stdout,
        [
            "<stdin>: 2 rules, 5 files, 0 files with no owner, 1 rule owning no files",
            "",
            "line  files  pattern",
            "   1      5  *",
            "   2      0  [x]      no files match",
            "",
            "<stdin>:2: error: `[x]` isn't a valid pattern - both `[` and `]` must be escaped with a `\\`, like `\\[x\\]`",
            "<stdin>:3: error: `not-an-owner` isn't a valid owner - owners must be @user, @org/team or an email address",
            "",
        ]
        .join("\n")
    );
}

#[test]
fn check_json_lists_problems() {
    let (status, stdout, _) = run_with_stdin(
        &repo("check-json"),
        &["check", "--codeowners", "-", "--json"],
        "[x] @acme/x\n",
    );
    assert_eq!(
        (status, stdout.as_str()),
        (
            1,
            r#"[
  {
    "line": 1,
    "severity": "error",
    "message": "`[x]` isn't a valid pattern - both `[` and `]` must be escaped with a `\\`, like `\\[x\\]`"
  }
]
"#
        )
    );
}

#[test]
fn check_unused_reports_rules_that_own_no_files() {
    let (status, stdout, _) = run(&repo("check-unused"), &["check", "--unused"]);
    assert_eq!(status, 1);
    assert_eq!(
        stdout,
        [
            ".github/CODEOWNERS:2: error: `/docs/` owns no files - every matching file is owned by a later rule",
            ".github/CODEOWNERS:4: error: `/missing/` owns no files - no files match",
            "",
        ]
        .join("\n")
    );
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
fn points_old_flags_to_their_commands() {
    let repo = repo("old-flags");
    for (flag, command) in [
        ("--owner-of", "gh codeowners view <path>"),
        ("--errors", "gh codeowners check"),
        ("--check", "gh codeowners check"),
    ] {
        let (status, _, stderr) = run(&repo, &[flag]);
        assert_eq!(status, 2);
        assert!(
            stderr.starts_with(&format!("Unknown option '{flag}', use `{command}`\n")),
            "{stderr}"
        );
    }
}

#[test]
fn prints_help_without_a_command() {
    let (status, stdout, _) = run(&repo("help"), &[]);
    assert_eq!(status, 0);
    assert!(
        stdout.starts_with("Work with GitHub CODEOWNERS files.\n\nUSAGE\n"),
        "{stdout}"
    );
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
    let docs = repo.0.join("docs");
    let (status, stdout, _) = run_in(&docs, &["list"]);
    assert_eq!(status, 0);
    assert!(
        stdout.starts_with(".github/CODEOWNERS: 4 rules, 5 files"),
        "{stdout}"
    );
    // Relative paths are relative to the current directory.
    let (status, stdout, _) = run_in(&docs, &["view", "guide.md", "../src/main.ts", "./new/file"]);
    assert_eq!(status, 0);
    assert_eq!(
        stdout,
        [
            "docs/guide.md: @acme/writers, from .github/CODEOWNERS:3 *.md",
            "src/main.ts: @acme/platform, from .github/CODEOWNERS:1 *",
            "docs/new/file: @acme/docs, from .github/CODEOWNERS:2 /docs/",
            "",
        ]
        .join("\n")
    );
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
    assert_eq!(
        stdout,
        [
            "docs/: @acme/docs, from .github/CODEOWNERS:2 /docs/",
            "src/docs: @acme/platform, from .github/CODEOWNERS:1 *",
            "",
        ]
        .join("\n")
    );
}

#[test]
fn warns_when_github_ignores_rules_for_an_empty_codeowners() {
    let repo = repo("ignored");
    std::fs::write(repo.0.join(".github/CODEOWNERS"), "\n").unwrap();
    std::fs::write(repo.0.join("CODEOWNERS"), "* @acme/platform\n").unwrap();
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
        ".github/CODEOWNERS: warning: this file has no rules, but GitHub uses it instead of CODEOWNERS, so no file has an owner\n"
    );
    // Not for a file named with --codeowners, which GitHub may not use.
    let (_, _, stderr) = run(&repo, &["list", "--codeowners", ".github/CODEOWNERS"]);
    assert_eq!(stderr, "");
}
