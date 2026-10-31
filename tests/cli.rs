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
    let dir = std::env::temp_dir().join(format!(
        "codeowners-stats-cli-{name}-{}",
        std::process::id()
    ));
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
    let output = Command::new(env!("CARGO_BIN_EXE_codeowners-stats"))
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
        run(&repo("text"), &[]),
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
    assert_eq!(run(&repo("check"), &["--check"]).0, 1);
}

#[test]
fn json_unused_lists_only_rules_that_own_nothing() {
    let (status, stdout, _) = run(&repo("json"), &["--json", "--unused"]);
    assert_eq!(status, 0);
    let rules = &stdout[stdout.find("\"rules\"").unwrap()..];
    assert_eq!(
        rules,
        r#""rules": [
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
}
"#
    );
}

#[test]
fn exits_with_2_when_theres_no_codeowners() {
    let empty =
        std::env::temp_dir().join(format!("codeowners-stats-cli-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let (status, _, stderr) = run(&repo("missing"), &[empty.to_str().unwrap()]);
    std::fs::remove_dir_all(&empty).unwrap();
    assert_eq!(status, 2);
    assert_eq!(
        stderr,
        format!(
            "codeowners-stats: No CODEOWNERS file found in {}\n",
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
    assert_eq!(run(&repo, &[]).0, 0);
    text.push('#');
    std::fs::write(repo.0.join(".github/CODEOWNERS"), &text).unwrap();
    let (status, _, stderr) = run(&repo, &[]);
    assert_eq!(status, 2);
    assert!(
        stderr.ends_with(&format!(
            "/.github/CODEOWNERS is {} bytes, over GitHub's limit of {limit}, so GitHub ignores it\n",
            limit + 1
        )),
        "{stderr}"
    );
}
