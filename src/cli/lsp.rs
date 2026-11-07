//! A language server for CODEOWNERS files, for editors like VS Code.
//!
//! It checks the text being edited, not the file on disk, and counts the
//! files each rule owns from git's index, which it lists once per repository
//! and lists again when the client reports the index changed.

mod text;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gh_codeowners::codeowners::{
    CODEOWNERS_LOCATIONS, CodeownersRule, LineProblem, MAX_CODEOWNERS_BYTES, find_codeowners,
    find_problems, find_repo_root, ignored_codeowners, parse_codeowners, repo_root_for_codeowners,
};
use gh_codeowners::report::Status;
use gh_codeowners::{TrackedFiles, owners_of};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Exit, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    HoverRequest, InlayHintRefreshRequest, InlayHintRequest, RegisterCapability, Request as _,
};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DiagnosticTag, DidChangeTextDocumentParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, FileSystemWatcher, GlobPattern, Hover,
    HoverContents, HoverParams, HoverProviderCapability, InitializeParams, InlayHint,
    InlayHintLabel, InlayHintParams, MarkupContent, MarkupKind, OneOf, PositionEncodingKind,
    PublishDiagnosticsParams, Registration, RegistrationParams, RelativePattern, SaveOptions,
    ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions,
    TextDocumentSyncSaveOptions, Uri,
};
use serde::{Deserialize, Serialize};

use super::load::{absolute, repo_display, rule_list};
use super::view::repo_path;
use text::{Encoding, LineIndex, path_to_uri, uri_to_path};

#[derive(clap::Args)]
pub(crate) struct Args {
    /// Talk to the client over stdin and stdout, which is the default
    #[arg(long)]
    stdio: bool,
}

/// How long to wait for typing to pause before checking a document again.
const EDIT_DEBOUNCE: Duration = Duration::from_millis(150);
/// How long to wait for changes to git's index to settle, like in a rebase,
/// before listing files again.
const INDEX_DEBOUNCE: Duration = Duration::from_secs(1);

/// A custom request for the rule that owns a file, for the editor to show
/// a file's owners. Its CODEOWNERS file's unsaved changes count.
const OWNER_METHOD: &str = "gh-codeowners/owner";

#[derive(Deserialize)]
struct OwnerParams {
    uri: Uri,
}

#[derive(Serialize)]
struct OwnerResult {
    /// The file's path in its repository.
    path: String,
    /// The CODEOWNERS file GitHub uses for the file's repository.
    codeowners: String,
    /// The rule that owns the file, if any.
    rule: Option<OwnerRule>,
}

#[derive(Serialize)]
struct OwnerRule {
    /// Where the pattern is in the CODEOWNERS file.
    range: lsp_types::Range,
    pattern: String,
    owners: Vec<String>,
}

pub(crate) fn run(_args: &Args) -> Result<ExitCode, String> {
    let (connection, io_threads) = Connection::stdio();
    let (id, params) = connection.initialize_start().map_err(|e| e.to_string())?;
    let params: InitializeParams =
        serde_json::from_value(params).map_err(|e| format!("Bad initialize request: {e}"))?;
    let mut server = Server::new(&connection, &params);
    let result = serde_json::json!({
        "capabilities": server.capabilities(),
        "serverInfo": { "name": "gh-codeowners", "version": env!("CARGO_PKG_VERSION") },
    });
    connection
        .initialize_finish(id, result)
        .map_err(|e| e.to_string())?;
    let clean = server.run().map_err(|e| e.to_string())?;
    drop(connection);
    io_threads.join().map_err(|e| e.to_string())?;
    // The protocol asks for 1 if the client exits without shutting down.
    Ok(if clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

struct Document {
    uri: Uri,
    // None for documents that aren't files, like unsaved ones.
    path: Option<PathBuf>,
    text: String,
    version: i32,
    // Computed when first needed after each change.
    analysis: Option<Rc<Analysis>>,
}

/// What the server knows about a version of a document.
struct Analysis {
    index: LineIndex,
    rules: Vec<CodeownersRule>,
    problems: Vec<LineProblem>,
    // How many files each rule owns, unless they couldn't be counted, like
    // outside a git repository.
    counts: Option<Vec<(usize, Status)>>,
}

struct Server<'a> {
    connection: &'a Connection,
    encoding: Encoding,
    // Whether the client can watch git's index for us, and with patterns
    // relative to a directory.
    watch_files: bool,
    relative_patterns: bool,
    refresh_inlay_hints: bool,
    // By URI.
    documents: HashMap<String, Document>,
    // Each repository's files, or None if they couldn't be listed.
    files: HashMap<PathBuf, Option<Rc<TrackedFiles>>>,
    // Repositories whose git index the client is watching.
    watched: HashSet<PathBuf>,
    // Documents whose diagnostics need publishing, and when.
    unpublished: HashSet<String>,
    publish_at: Option<Instant>,
    // When to list files again, after git's index changed.
    relist_at: Option<Instant>,
    next_request: i32,
}

impl<'a> Server<'a> {
    fn new(connection: &'a Connection, params: &InitializeParams) -> Server<'a> {
        let capabilities = &params.capabilities;
        let encoding = match capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_ref())
        {
            Some(encodings) if encodings.contains(&PositionEncodingKind::UTF8) => Encoding::Utf8,
            _ => Encoding::Utf16,
        };
        let workspace = capabilities.workspace.as_ref();
        let watched_files = workspace.and_then(|w| w.did_change_watched_files.as_ref());
        Server {
            connection,
            encoding,
            watch_files: watched_files.and_then(|w| w.dynamic_registration) == Some(true),
            relative_patterns: watched_files.and_then(|w| w.relative_pattern_support) == Some(true),
            refresh_inlay_hints: workspace
                .and_then(|w| w.inlay_hint.as_ref())
                .and_then(|i| i.refresh_support)
                == Some(true),
            documents: HashMap::new(),
            files: HashMap::new(),
            watched: HashSet::new(),
            unpublished: HashSet::new(),
            publish_at: None,
            relist_at: None,
            next_request: 0,
        }
    }

    fn capabilities(&self) -> ServerCapabilities {
        ServerCapabilities {
            position_encoding: Some(match self.encoding {
                Encoding::Utf8 => PositionEncodingKind::UTF8,
                Encoding::Utf16 => PositionEncodingKind::UTF16,
            }),
            text_document_sync: Some(TextDocumentSyncCapability::Options(
                TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(TextDocumentSyncKind::FULL),
                    save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                        include_text: Some(false),
                    })),
                    ..Default::default()
                },
            )),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            inlay_hint_provider: Some(OneOf::Left(true)),
            ..Default::default()
        }
    }

    /// Handle messages until the client exits. Returns whether it shut the
    /// server down first.
    fn run(&mut self) -> Result<bool, lsp_server::ProtocolError> {
        loop {
            let deadline = self.publish_at.into_iter().chain(self.relist_at).min();
            let message = match deadline {
                Some(deadline) => {
                    let timeout = deadline.saturating_duration_since(Instant::now());
                    match self.connection.receiver.recv_timeout(timeout) {
                        Ok(message) => Some(message),
                        Err(e) if e.is_timeout() => None,
                        Err(_) => return Ok(false),
                    }
                }
                None => match self.connection.receiver.recv() {
                    Ok(message) => Some(message),
                    Err(_) => return Ok(false),
                },
            };
            match message {
                Some(Message::Request(request)) => {
                    if self.connection.handle_shutdown(&request)? {
                        return Ok(true);
                    }
                    self.handle_request(request);
                }
                Some(Message::Notification(notification)) => {
                    if notification.method == Exit::METHOD {
                        return Ok(false);
                    }
                    self.handle_notification(notification);
                }
                // Replies to our requests, which need no handling.
                Some(Message::Response(_)) => {}
                None => {}
            }
            self.run_timers();
        }
    }

    fn run_timers(&mut self) {
        let now = Instant::now();
        if self.relist_at.is_some_and(|at| at <= now) {
            self.relist_at = None;
            self.files.clear();
            let uris: Vec<String> = self.documents.keys().cloned().collect();
            for uri in uris {
                self.changed(&uri);
            }
            self.publish_at = Some(now);
            if self.refresh_inlay_hints {
                self.send_request(InlayHintRefreshRequest::METHOD, ());
            }
        }
        if self.publish_at.is_some_and(|at| at <= now) {
            self.publish_at = None;
            for uri in std::mem::take(&mut self.unpublished) {
                self.publish(&uri);
            }
        }
    }

    fn send(&self, message: impl Into<Message>) {
        // A send only fails once the client is gone, which ends the loop.
        let _ = self.connection.sender.send(message.into());
    }

    fn send_request(&mut self, method: &str, params: impl Serialize) {
        self.next_request += 1;
        let id = RequestId::from(format!("gh-codeowners-{}", self.next_request));
        self.send(Request::new(id, method.to_string(), params));
    }

    fn handle_notification(&mut self, notification: Notification) {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let Ok(params) =
                    notification.extract::<DidOpenTextDocumentParams>(DidOpenTextDocument::METHOD)
                else {
                    return;
                };
                let document = params.text_document;
                let key = document.uri.as_str().to_string();
                let path = uri_to_path(document.uri.as_str());
                if let (Some(path), false) = (&path, self.watch_files) {
                    // Nothing says when git's index changes, so list again.
                    self.files.remove(&repo_root_for_codeowners(path));
                }
                self.documents.insert(
                    key.clone(),
                    Document {
                        uri: document.uri,
                        path,
                        text: document.text,
                        version: document.version,
                        analysis: None,
                    },
                );
                self.changed(&key);
                self.publish_at = Some(Instant::now());
            }
            DidChangeTextDocument::METHOD => {
                let Ok(params) = notification
                    .extract::<DidChangeTextDocumentParams>(DidChangeTextDocument::METHOD)
                else {
                    return;
                };
                let key = params.text_document.uri.as_str().to_string();
                let Some(document) = self.documents.get_mut(&key) else {
                    return;
                };
                // Changes are whole documents, as the server asks for.
                if let Some(change) = params.content_changes.into_iter().last() {
                    document.text = change.text;
                }
                document.version = params.text_document.version;
                self.changed(&key);
                self.publish_at = Some(Instant::now() + EDIT_DEBOUNCE);
            }
            DidSaveTextDocument::METHOD => {
                let Ok(params) =
                    notification.extract::<DidSaveTextDocumentParams>(DidSaveTextDocument::METHOD)
                else {
                    return;
                };
                if self.watch_files {
                    return;
                }
                // Nothing says when git's index changes, so list again.
                let path = uri_to_path(params.text_document.uri.as_str());
                if let Some(root) = path.as_deref().map(repo_root_for_codeowners) {
                    self.files.remove(&root);
                    let uris: Vec<String> = self
                        .documents
                        .iter()
                        .filter(|(_, d)| {
                            d.path.as_deref().map(repo_root_for_codeowners).as_ref() == Some(&root)
                        })
                        .map(|(uri, _)| uri.clone())
                        .collect();
                    for uri in uris {
                        self.changed(&uri);
                    }
                    self.publish_at = Some(Instant::now());
                }
            }
            DidCloseTextDocument::METHOD => {
                let Ok(params) = notification
                    .extract::<DidCloseTextDocumentParams>(DidCloseTextDocument::METHOD)
                else {
                    return;
                };
                let key = params.text_document.uri.as_str();
                self.documents.remove(key);
                self.unpublished.remove(key);
                self.send(Notification::new(
                    PublishDiagnostics::METHOD.to_string(),
                    PublishDiagnosticsParams::new(params.text_document.uri, Vec::new(), None),
                ));
            }
            DidChangeWatchedFiles::METHOD => {
                // Only git's index is watched.
                self.relist_at = Some(Instant::now() + INDEX_DEBOUNCE);
            }
            _ => {}
        }
    }

    /// Forget what's known about a document, and publish it again.
    fn changed(&mut self, key: &str) {
        if let Some(document) = self.documents.get_mut(key) {
            document.analysis = None;
            self.unpublished.insert(key.to_string());
        }
    }

    fn handle_request(&mut self, request: Request) {
        let id = request.id.clone();
        let result = match request.method.as_str() {
            HoverRequest::METHOD => request
                .extract::<HoverParams>(HoverRequest::METHOD)
                .map_err(|e| format!("{e:?}"))
                .map(|(_, params)| serde_json::to_value(self.hover(&params)).unwrap()),
            InlayHintRequest::METHOD => request
                .extract::<InlayHintParams>(InlayHintRequest::METHOD)
                .map_err(|e| format!("{e:?}"))
                .map(|(_, params)| serde_json::to_value(self.inlay_hints(&params)).unwrap()),
            OWNER_METHOD => request
                .extract::<OwnerParams>(OWNER_METHOD)
                .map_err(|e| format!("{e:?}"))
                .and_then(|(_, params)| self.owner(&params))
                .map(|result| serde_json::to_value(result).unwrap()),
            method => {
                self.send(Response::new_err(
                    id,
                    ErrorCode::MethodNotFound as i32,
                    format!("Unknown method {method}"),
                ));
                return;
            }
        };
        self.send(match result {
            Ok(value) => Response::new_ok(id, value),
            Err(message) => Response::new_err(id, ErrorCode::InvalidParams as i32, message),
        });
    }

    /// The files in the repository at `root`, listing them if needed.
    fn files(&mut self, root: &Path) -> Option<Rc<TrackedFiles>> {
        if let Some(files) = self.files.get(root) {
            return files.clone();
        }
        let files = match TrackedFiles::list(root) {
            Ok(files) => Some(Rc::new(files)),
            Err(message) => {
                // Shown in the client's log. Outside a repository is common
                // enough, like for an example CODEOWNERS file.
                eprintln!(
                    "gh-codeowners: couldn't list files in {}: {message}",
                    root.display()
                );
                None
            }
        };
        self.files.insert(root.to_path_buf(), files.clone());
        if files.is_some() {
            self.watch(root);
        }
        files
    }

    /// Ask the client to tell us when the git index at `root` changes.
    fn watch(&mut self, root: &Path) {
        if !self.watch_files || !self.watched.insert(root.to_path_buf()) {
            return;
        }
        // Worktrees and submodules keep their index outside `root`.
        let Some(git_dir) = Command::new("git")
            .args(["rev-parse", "--absolute-git-dir"])
            .current_dir(root)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|dir| PathBuf::from(dir.trim_end()))
        else {
            return;
        };
        let glob_pattern = if self.relative_patterns {
            GlobPattern::Relative(RelativePattern {
                base_uri: OneOf::Right(path_to_uri(&git_dir).parse().expect("a valid URI")),
                pattern: "index".to_string(),
            })
        } else {
            GlobPattern::String(git_dir.join("index").to_string_lossy().replace('\\', "/"))
        };
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![FileSystemWatcher {
                glob_pattern,
                kind: None,
            }],
        };
        let registration = Registration {
            id: format!("git-index:{}", git_dir.display()),
            method: DidChangeWatchedFiles::METHOD.to_string(),
            register_options: Some(serde_json::to_value(options).unwrap()),
        };
        self.send_request(
            RegisterCapability::METHOD,
            RegistrationParams {
                registrations: vec![registration],
            },
        );
    }

    /// What's known about a document's current text, if it's open.
    fn analysis(&mut self, key: &str) -> Option<(Rc<Analysis>, &Document)> {
        let document = self.documents.get(key)?;
        if document.analysis.is_none() {
            let root = document.path.as_deref().map(repo_root_for_codeowners);
            let too_big = document.text.len() as u64 > MAX_CODEOWNERS_BYTES;
            let files = match &root {
                Some(root) if !too_big => self.files(root),
                _ => None,
            };
            let document = self.documents.get_mut(key)?;
            let analysis = analyze(
                &document.text,
                document.path.as_deref().zip(root.as_deref()),
                files.as_deref(),
                self.encoding,
            );
            document.analysis = Some(Rc::new(analysis));
        }
        let document = self.documents.get(key)?;
        Some((document.analysis.clone()?, document))
    }

    fn publish(&mut self, key: &str) {
        let Some((analysis, document)) = self.analysis(key) else {
            return;
        };
        let diagnostics = diagnostics(&analysis);
        let params = PublishDiagnosticsParams::new(
            document.uri.clone(),
            diagnostics,
            Some(document.version),
        );
        self.send(Notification::new(
            PublishDiagnostics::METHOD.to_string(),
            params,
        ));
    }

    fn hover(&mut self, params: &HoverParams) -> Option<Hover> {
        let position = &params.text_document_position_params;
        let (analysis, document) = self.analysis(position.text_document.uri.as_str())?;
        let offset = analysis.index.offset(&document.text, position.position)?;
        let counts = analysis.counts.as_ref()?;
        let (rule, &(owned, status)) = analysis.rules.iter().zip(counts).find(|(rule, _)| {
            rule.pattern_span.contains(&offset) || rule.pattern_span.end == offset
        })?;
        let value = match status {
            Status::Active => format!("Owns {owned} {}", if owned == 1 { "file" } else { "files" }),
            _ => format!("Owns no files: {}", status.note()),
        };
        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: Some(analysis.index.range(&rule.pattern_span)),
        })
    }

    fn inlay_hints(&mut self, params: &InlayHintParams) -> Vec<InlayHint> {
        let Some((analysis, document)) = self.analysis(params.text_document.uri.as_str()) else {
            return Vec::new();
        };
        let Some(counts) = &analysis.counts else {
            return Vec::new();
        };
        let lines = params.range.start.line as usize..=params.range.end.line as usize;
        analysis
            .rules
            .iter()
            .zip(counts)
            .filter(|(rule, _)| lines.contains(&rule.lineno))
            .map(|(rule, &(owned, _))| InlayHint {
                position: analysis
                    .index
                    .position(analysis.index.line_end(&document.text, rule.lineno)),
                label: InlayHintLabel::String(format!(
                    "{owned} {}",
                    if owned == 1 { "file" } else { "files" }
                )),
                kind: None,
                text_edits: None,
                tooltip: None,
                padding_left: Some(true),
                padding_right: None,
                data: None,
            })
            .collect()
    }

    fn owner(&mut self, params: &OwnerParams) -> Result<Option<OwnerResult>, String> {
        let path = uri_to_path(params.uri.as_str())
            .ok_or_else(|| format!("{} isn't a file", params.uri.as_str()))?;
        let Some(root) = path.parent().and_then(find_repo_root) else {
            return Ok(None);
        };
        let Some(codeowners) = find_codeowners(&root) else {
            return Ok(None);
        };
        // An open CODEOWNERS file may have unsaved changes.
        let open = self
            .documents
            .values()
            .find(|d| d.path.as_deref().is_some_and(|p| same_file(p, &codeowners)));
        let text = match open {
            Some(document) => document.text.clone(),
            None => std::fs::read_to_string(&codeowners)
                .map_err(|e| format!("{}: {e}", codeowners.display()))?,
        };
        if text.len() as u64 > MAX_CODEOWNERS_BYTES {
            return Err(format!(
                "{} is over GitHub's limit of {MAX_CODEOWNERS_BYTES} bytes, so GitHub ignores it",
                codeowners.display()
            ));
        }
        let repo_path = repo_path(&root, &path.to_string_lossy())?;
        let rules = parse_codeowners(&text);
        let owner = owners_of(&rule_list(&rules), &[&repo_path])[0];
        let index = LineIndex::new(&text, self.encoding);
        Ok(Some(OwnerResult {
            path: repo_path,
            codeowners: path_to_uri(&absolute(&codeowners)),
            rule: owner.map(|i| OwnerRule {
                range: index.range(&rules[i].pattern_span),
                pattern: rules[i].pattern.clone(),
                owners: rules[i].owners.clone(),
            }),
        }))
    }
}

/// Analyze CODEOWNERS text, from the file at `location.0` in the repository
/// at `location.1`, if it's a file, counting `files` if they're known.
fn analyze(
    text: &str,
    location: Option<(&Path, &Path)>,
    files: Option<&TrackedFiles>,
    encoding: Encoding,
) -> Analysis {
    let rules = parse_codeowners(text);
    let mut problems = find_problems(text);
    if let Some((path, root)) = location {
        problems.extend(location_problem(text, path, root));
    }
    let counts = files.map(|files| {
        files
            .count(&rule_list(&rules))
            .rules
            .into_iter()
            .map(|c| (c.owned, Status::new(c.owned, c.matches_any)))
            .collect()
    });
    Analysis {
        index: LineIndex::new(text, encoding),
        rules,
        problems,
        counts,
    }
}

/// A problem with where a CODEOWNERS file is, if GitHub ignores it, or
/// ignores another because this one has no rules.
fn location_problem(text: &str, path: &Path, root: &Path) -> Option<LineProblem> {
    let in_location = CODEOWNERS_LOCATIONS
        .iter()
        .any(|location| same_file(&root.join(location), path));
    if !in_location {
        return None;
    }
    let used = find_codeowners(root)?;
    let message = if same_file(&used, path) {
        let ignored = ignored_codeowners(root, text)?;
        format!(
            "this file has no rules, but GitHub uses it instead of {}, so no file has an owner",
            repo_display(root, &ignored)
        )
    } else {
        format!(
            "GitHub uses {} instead of this file, so none of its rules apply",
            repo_display(root, &used)
        )
    };
    Some(LineProblem {
        lineno: None,
        span: None,
        error: false,
        message,
    })
}

/// Whether two paths are the same file, even if spelled differently, like
/// with a different case on Windows.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => absolute(a) == absolute(b),
    }
}

fn diagnostics(analysis: &Analysis) -> Vec<Diagnostic> {
    let index = &analysis.index;
    let mut diagnostics: Vec<Diagnostic> = analysis
        .problems
        .iter()
        .map(|problem| {
            // Like `check`, which shows the part after " - " as a label.
            let message = match problem.message.split_once(" - ") {
                Some((title, label)) => format!("{title}; {label}"),
                None => problem.message.clone(),
            };
            Diagnostic {
                range: problem
                    .span
                    .as_ref()
                    .map(|span| index.range(span))
                    .unwrap_or_default(),
                severity: Some(if problem.error {
                    DiagnosticSeverity::ERROR
                } else {
                    DiagnosticSeverity::WARNING
                }),
                source: Some("gh-codeowners".to_string()),
                message,
                ..Default::default()
            }
        })
        .collect();
    let counts = analysis.counts.iter().flatten();
    for (rule, &(_, status)) in analysis.rules.iter().zip(counts) {
        // Don't say a line owns nothing when there's already a problem with it.
        if status == Status::Active
            || analysis
                .problems
                .iter()
                .any(|p| p.lineno == Some(rule.lineno))
        {
            continue;
        }
        diagnostics.push(Diagnostic {
            range: index.range(&rule.pattern_span),
            severity: Some(DiagnosticSeverity::HINT),
            source: Some("gh-codeowners".to_string()),
            message: format!("`{}` owns no files; {}", rule.pattern, status.note()),
            tags: Some(vec![DiagnosticTag::UNNECESSARY]),
            ..Default::default()
        });
    }
    diagnostics
}
