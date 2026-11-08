//! Drives `gh-codeowners lsp` like an editor would, over stdin and stdout.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

mod common;
use common::{git, write};

/// A repository's path, and the temporary directory it's in.
struct Repo(
    PathBuf,
    #[expect(dead_code, reason = "deletes the directory when dropped")] TempDir,
);

fn repo(name: &str) -> Repo {
    let dir = TempDir::with_prefix(format!("gh-codeowners-lsp-{name}-")).unwrap();
    write(dir.path(), ".github/CODEOWNERS", "* @acme/platform\n");
    write(dir.path(), "Makefile", "");
    write(dir.path(), "docs/guide.md", "");
    write(dir.path(), "src/main.ts", "");
    git(dir.path(), &["init", "-q"]);
    git(dir.path(), &["add", "."]);
    // The real path, as an editor would have it, since the temporary
    // directory is a symlink on macOS. Windows would add a `\\?\` prefix.
    let path = if cfg!(windows) {
        dir.path().to_path_buf()
    } else {
        std::fs::canonicalize(dir.path()).unwrap()
    };
    Repo(path, dir)
}

fn uri(path: &Path) -> String {
    let path = path.to_str().unwrap().replace('\\', "/");
    let path = path.replace(' ', "%20");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

struct Client {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: i64,
}

impl Client {
    fn start(dir: &Path, capabilities: Value) -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_gh-codeowners"))
            .args(["lsp", "--stdio"])
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let (sender, messages) = channel();
        std::thread::spawn(move || {
            loop {
                let mut length = 0;
                loop {
                    let mut header = String::new();
                    if stdout.read_line(&mut header).unwrap() == 0 {
                        return;
                    }
                    let header = header.trim_end();
                    if header.is_empty() {
                        break;
                    }
                    if let Some(n) = header.strip_prefix("Content-Length: ") {
                        length = n.parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                stdout.read_exact(&mut body).unwrap();
                if sender.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        let mut client = Client {
            child,
            stdin,
            messages,
            next_id: 0,
        };
        let result = client.request(
            "initialize",
            json!({ "processId": null, "rootUri": uri(dir), "capabilities": capabilities }),
        );
        assert_eq!(result["serverInfo"]["name"], "gh-codeowners");
        client.notify("initialized", json!({}));
        client
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn receive(&mut self) -> Value {
        self.messages
            .recv_timeout(Duration::from_secs(10))
            .expect("a message from the server")
    }

    /// Send a request and wait for its result, skipping other messages.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        loop {
            let message = self.receive();
            if message["id"] == id && message.get("method").is_none() {
                assert_eq!(message.get("error"), None, "{message}");
                return message["result"].clone();
            }
        }
    }

    /// Wait for a notification, skipping other messages.
    fn notification(&mut self, method: &str) -> Value {
        loop {
            let message = self.receive();
            if message["method"] == method && message.get("id").is_none() {
                return message["params"].clone();
            }
        }
    }

    fn open(&mut self, path: &Path, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri(path), "languageId": "codeowners", "version": 1, "text": text,
            } }),
        );
    }

    fn change(&mut self, path: &Path, version: i32, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": uri(path), "version": version },
                "contentChanges": [{ "text": text }],
            }),
        );
    }

    /// Shut the server down, returning its exit code.
    fn shutdown(mut self) -> i32 {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        self.child.wait().unwrap().code().unwrap()
    }
}

/// Each diagnostic's line, severity and message.
fn summary(diagnostics: &Value) -> Vec<(u64, u64, String)> {
    diagnostics["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["range"]["start"]["line"].as_u64().unwrap(),
                d["severity"].as_u64().unwrap(),
                d["message"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

#[test]
fn reports_problems_and_rules_that_own_nothing_in_unsaved_text() {
    let repo = repo("diagnostics");
    let codeowners = repo.0.join(".github/CODEOWNERS");
    let mut client = Client::start(&repo.0, json!({}));
    // Not what's on disk.
    client.open(
        &codeowners,
        "*        @acme/platform\n/app/[id]/  @acme/web\n/missing/  @acme/x\n/docs/ @acme/docs\n",
    );
    let diagnostics = client.notification("textDocument/publishDiagnostics");
    assert_eq!(diagnostics["uri"], uri(&codeowners));
    assert_eq!(diagnostics["version"], 1);
    assert_eq!(
        summary(&diagnostics),
        [
            (
                1,
                1,
                "`/app/[id]/` isn't a valid pattern; both `[` and `]` must be escaped with a `\\`, like `/app/\\[id\\]/`"
                    .to_string()
            ),
            (2, 4, "`/missing/` owns no files; no files match".to_string()),
        ]
    );
    let unmatched = &diagnostics["diagnostics"][1];
    assert_eq!(unmatched["tags"], json!([1]));
    assert_eq!(
        unmatched["range"],
        json!({ "start": { "line": 2, "character": 0 }, "end": { "line": 2, "character": 9 } })
    );

    // Edits are checked as they're made.
    client.change(
        &codeowners,
        2,
        "* @acme/platform\n/docs/ @acme/docs\n* @acme/all\n",
    );
    let diagnostics = client.notification("textDocument/publishDiagnostics");
    assert_eq!(diagnostics["version"], 2);
    assert_eq!(
        summary(&diagnostics),
        [
            (
                0,
                4,
                "`*` owns no files; every matching file is owned by a later rule".to_string()
            ),
            (
                1,
                4,
                "`/docs/` owns no files; every matching file is owned by a later rule".to_string()
            ),
        ]
    );
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn shows_how_many_files_each_rule_owns() {
    let repo = repo("counts");
    let codeowners = repo.0.join(".github/CODEOWNERS");
    let mut client = Client::start(&repo.0, json!({}));
    client.open(
        &codeowners,
        "* @acme/platform # all\r\n/docs/ @acme/docs\r\n",
    );
    let hints = client.request(
        "textDocument/inlayHint",
        json!({
            "textDocument": { "uri": uri(&codeowners) },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 3, "character": 0 } },
        }),
    );
    assert_eq!(
        hints,
        json!([
            { "position": { "line": 0, "character": 22 }, "label": "3 files", "paddingLeft": true },
            { "position": { "line": 1, "character": 17 }, "label": "1 file", "paddingLeft": true },
        ])
    );
    let hover = client.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri(&codeowners) }, "position": { "line": 1, "character": 3 } }),
    );
    assert_eq!(
        hover,
        json!({
            "contents": { "kind": "markdown", "value": "Owns 1 file" },
            "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 6 } },
        })
    );
    // Not on a pattern.
    let hover = client.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri(&codeowners) }, "position": { "line": 1, "character": 9 } }),
    );
    assert_eq!(hover, Value::Null);
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn fixes_patterns_github_skips() {
    let repo = repo("fixes");
    let codeowners = repo.0.join(".github/CODEOWNERS");
    let mut client = Client::start(&repo.0, json!({}));
    client.open(
        &codeowners,
        "* @acme/x\n/app/[id]/ @acme/web\n/src/*** @acme/y\n",
    );
    let diagnostics = client.notification("textDocument/publishDiagnostics");
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri(&codeowners) },
            "range": { "start": { "line": 1, "character": 3 }, "end": { "line": 1, "character": 3 } },
            "context": { "diagnostics": [] },
        }),
    );
    assert_eq!(
        actions,
        json!([{
            "title": "Replace with `/app/\\[id\\]/`",
            "kind": "quickfix",
            "diagnostics": [diagnostics["diagnostics"][0]],
            "isPreferred": true,
            "edit": { "changes": { uri(&codeowners): [{
                "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 1, "character": 10 } },
                "newText": "/app/\\[id\\]/",
            }] } },
        }])
    );
    // The whole file, asking only for quick fixes.
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri(&codeowners) },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 3, "character": 0 } },
            "context": { "diagnostics": [], "only": ["quickfix"] },
        }),
    );
    let titles: Vec<_> = actions
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["Replace with `/app/\\[id\\]/`", "Replace with `/src/**`"]
    );
    // Other kinds of actions.
    let actions = client.request(
        "textDocument/codeAction",
        json!({
            "textDocument": { "uri": uri(&codeowners) },
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 3, "character": 0 } },
            "context": { "diagnostics": [], "only": ["refactor"] },
        }),
    );
    assert_eq!(actions, json!([]));
    // The diagnostic says why a rule owns nothing, so hovering adds nothing.
    let hover = client.request(
        "textDocument/hover",
        json!({ "textDocument": { "uri": uri(&codeowners) }, "position": { "line": 1, "character": 3 } }),
    );
    assert_eq!(hover, Value::Null);
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn warns_about_codeowners_files_github_ignores() {
    let repo = repo("ignored");
    write(&repo.0, "CODEOWNERS", "* @acme/x\n");
    let ignored = repo.0.join("CODEOWNERS");
    let mut client = Client::start(&repo.0, json!({}));
    client.open(&ignored, "* @acme/x\n");
    let diagnostics = client.notification("textDocument/publishDiagnostics");
    assert_eq!(
        summary(&diagnostics),
        [(
            0,
            2,
            "GitHub uses .github/CODEOWNERS instead of this file, so none of its rules apply"
                .to_string()
        )]
    );
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn finds_the_rule_that_owns_a_file_with_unsaved_changes() {
    let repo = repo("owner");
    let codeowners = repo.0.join(".github/CODEOWNERS");
    let mut client = Client::start(&repo.0, json!({}));
    let owner = client.request(
        "gh-codeowners/owner",
        json!({ "uri": uri(&repo.0.join("docs/guide.md")) }),
    );
    assert_eq!(
        owner,
        json!({
            "path": "docs/guide.md",
            "codeowners": uri(&codeowners),
            "rule": {
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } },
                "pattern": "*",
                "owners": ["@acme/platform"],
            },
        })
    );
    client.open(
        &codeowners,
        "* @acme/platform\n/docs/ @acme/docs a@example.com\n",
    );
    let owner = client.request(
        "gh-codeowners/owner",
        json!({ "uri": uri(&repo.0.join("docs/guide.md")) }),
    );
    assert_eq!(owner["rule"]["pattern"], "/docs/");
    assert_eq!(
        owner["rule"]["owners"],
        json!(["@acme/docs", "a@example.com"])
    );
    assert_eq!(client.shutdown(), 0);
}

#[test]
fn exits_with_1_without_a_shutdown() {
    let repo = repo("exit");
    let mut client = Client::start(&repo.0, json!({}));
    client.notify("exit", Value::Null);
    assert_eq!(client.child.wait().unwrap().code(), Some(1));
}

#[test]
fn rejects_options_for_other_commands() {
    let output = Command::new(env!("CARGO_BIN_EXE_gh-codeowners"))
        .args(["lsp", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "gh-codeowners: lsp doesn't take --codeowners, --root or --json\n"
    );
}

#[test]
fn counts_again_when_gits_index_changes() {
    let repo = repo("watch");
    let codeowners = repo.0.join(".github/CODEOWNERS");
    let mut client = Client::start(
        &repo.0,
        json!({ "workspace": {
            "didChangeWatchedFiles": { "dynamicRegistration": true, "relativePatternSupport": true },
            "inlayHint": { "refreshSupport": true },
        } }),
    );
    client.open(&codeowners, "* @acme/platform\n/lib/ @acme/lib\n");
    // The server asks to watch the index, wherever the git directory is, as
    // it first lists the repository's files.
    let (mut registration, mut diagnostics) = (None, None);
    while registration.is_none() || diagnostics.is_none() {
        let message = client.receive();
        match message["method"].as_str() {
            Some("client/registerCapability") => {
                registration = Some(message["params"]["registrations"][0].clone());
            }
            Some("textDocument/publishDiagnostics") => {
                diagnostics = Some(message["params"].clone())
            }
            _ => {}
        }
    }
    let (registration, diagnostics) = (registration.unwrap(), diagnostics.unwrap());
    assert_eq!(summary(&diagnostics).len(), 1);
    assert_eq!(registration["method"], "workspace/didChangeWatchedFiles");
    let pattern = &registration["registerOptions"]["watchers"][0]["globPattern"];
    assert!(pattern["baseUri"].as_str().unwrap().ends_with("/.git"));
    assert_eq!(pattern["pattern"], "index");

    write(&repo.0, "lib/a.rs", "");
    git(&repo.0, &["add", "."]);
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": uri(&repo.0.join(".git/index")), "type": 2 }] }),
    );
    // The client is asked to refresh inlay hints, and gets new diagnostics.
    let refresh = client.receive();
    assert_eq!(refresh["method"], "workspace/inlayHint/refresh");
    let diagnostics = client.notification("textDocument/publishDiagnostics");
    assert_eq!(summary(&diagnostics), []);
    assert_eq!(client.shutdown(), 0);
}
