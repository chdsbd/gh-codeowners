//! A language server for CODEOWNERS files, for editors like VS Code.
//!
//! It checks the text being edited, not the file on disk, and counts the
//! files each rule owns from git's index, which it lists once per repository
//! and lists again when the client reports the index changed.

mod text;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gen_lsp_types::{
    BaseUri, CodeAction, CodeActionKind, CodeActionOptions, CodeActionParams, CodeActionProvider,
    Contents, Diagnostic, DiagnosticSeverity, DiagnosticTag, DidChangeTextDocumentParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentFormattingParams,
    DocumentFormattingProvider, FileSystemWatcher, GlobPattern, Hover, HoverParams, HoverProvider,
    InitializeParams, InlayHint, InlayHintParams, InlayHintProvider, Label, LspNotificationMethod,
    LspRequestMethod, MarkupContent, MarkupKind, PublishDiagnosticsParams, Registration,
    RegistrationParams, RelativePattern, Save, SaveOptions, ServerCapabilities,
    TextDocumentContentChangeEvent, TextDocumentSync, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextEdit, Uri, WorkspaceEdit,
};
use gh_codeowners::codeowners::{
    CODEOWNERS_LOCATIONS, CodeownersRule, LineProblem, check_size, find_codeowners, find_problems,
    find_repo_root, ignored_codeowners, parse_codeowners, repo_root_for_codeowners, too_big,
};
use gh_codeowners::counter::{Rules, count, rule_list};
use gh_codeowners::format::{Align, format_with};
use gh_codeowners::report::Status;
use gh_codeowners::{TrackedFiles, index_path, owners_of};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use serde::{Deserialize, Serialize};

use super::check::unused_problems;
use super::load::{absolute, ignored_message, repo_display};
use super::plural;
use super::view::repo_path;
use text::LineIndex;

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

/// Settings, from `initializationOptions` and the `settings` of
/// `workspace/didChangeConfiguration`, like
/// `{ "format": { "align": "file", "spacing": 5 } }`.
#[derive(Deserialize, Default, Clone, Copy)]
#[serde(default)]
struct Settings {
    format: FormatSettings,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(default)]
struct FormatSettings {
    align: FormatAlign,
    spacing: usize,
}

impl Default for FormatSettings {
    fn default() -> Self {
        FormatSettings {
            align: FormatAlign::Group,
            spacing: 1,
        }
    }
}

#[derive(Deserialize, Default, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum FormatAlign {
    #[default]
    Group,
    File,
}

impl Settings {
    /// The settings in `value`, or the defaults if there are none.
    fn parse(value: Option<serde_json::Value>) -> Result<Settings, String> {
        let settings: Settings = match value {
            None | Some(serde_json::Value::Null) => return Ok(Settings::default()),
            Some(value) => {
                serde_json::from_value(value).map_err(|e| format!("Bad settings: {e}"))?
            }
        };
        if settings.format.spacing < 1 {
            return Err("Bad settings: format.spacing must be at least 1".to_string());
        }
        Ok(settings)
    }

    fn format_align(&self) -> Align {
        match self.format.align {
            FormatAlign::Group => Align::Group,
            FormatAlign::File => Align::File,
        }
    }
}

#[derive(Serialize)]
struct OwnerResult {
    /// The file's path in its repository.
    path: String,
    /// The CODEOWNERS file GitHub uses for the file's repository.
    codeowners: Uri,
    /// The rule that owns the file, if any.
    rule: Option<OwnerRule>,
}

#[derive(Serialize)]
struct OwnerRule {
    /// Where the pattern is in the CODEOWNERS file.
    range: gen_lsp_types::Range,
    pattern: String,
    owners: Vec<String>,
}

pub(crate) fn run(_args: &Args) -> Result<ExitCode, String> {
    let (connection, io_threads) = Connection::stdio();
    let (id, params) = connection.initialize_start().map_err(|e| e.to_string())?;
    let params: InitializeParams =
        serde_json::from_value(params).map_err(|e| format!("Bad initialize request: {e}"))?;
    // Bad settings shouldn't stop the server, so it uses the defaults.
    let settings = Settings::parse(params.initialization_options.clone()).unwrap_or_else(|e| {
        eprintln!("{e}");
        Settings::default()
    });
    let mut server = Server::new(&connection, &params, &settings);
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
    // Whether the client can watch git's index for us, and with patterns
    // relative to a directory.
    watch_files: bool,
    relative_patterns: bool,
    refresh_inlay_hints: bool,
    settings: Settings,
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
    fn new(
        connection: &'a Connection,
        params: &InitializeParams,
        settings: &Settings,
    ) -> Server<'a> {
        let capabilities = &params.capabilities;
        let workspace = capabilities.workspace.as_ref();
        let watched_files = workspace.and_then(|w| w.did_change_watched_files.as_ref());
        Server {
            connection,
            watch_files: watched_files.and_then(|w| w.dynamic_registration) == Some(true),
            relative_patterns: watched_files.and_then(|w| w.relative_pattern_support) == Some(true),
            refresh_inlay_hints: workspace
                .and_then(|w| w.inlay_hint.as_ref())
                .and_then(|i| i.refresh_support)
                == Some(true),
            settings: *settings,
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
            text_document_sync: Some(TextDocumentSync::Options(TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::Full),
                save: Some(Save::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
                ..Default::default()
            })),
            hover_provider: Some(HoverProvider::Bool(true)),
            document_formatting_provider: Some(DocumentFormattingProvider::Bool(true)),
            inlay_hint_provider: Some(InlayHintProvider::Bool(true)),
            code_action_provider: Some(CodeActionProvider::CodeActionOptions(CodeActionOptions {
                code_action_kinds: Some(vec![CodeActionKind::QuickFix]),
                ..Default::default()
            })),
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
                    if notification.method == LspNotificationMethod::Exit.as_str() {
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
            self.changed(|_| true);
            self.publish_at = Some(now);
            if self.refresh_inlay_hints {
                self.send_request(LspRequestMethod::WorkspaceInlayHintRefresh, ());
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

    fn send_request(&mut self, method: LspRequestMethod, params: impl Serialize) {
        self.next_request += 1;
        let id = RequestId::from(format!("gh-codeowners-{}", self.next_request));
        self.send(Request::new(id, method.as_str().to_string(), params));
    }

    fn handle_notification(&mut self, notification: Notification) {
        let method = notification.method.clone();
        match LspNotificationMethod::from(method.as_str()) {
            LspNotificationMethod::TextDocumentDidOpen => {
                let Ok(params) = notification.extract::<DidOpenTextDocumentParams>(&method) else {
                    return;
                };
                let document = params.text_document;
                let key = document.uri.as_str().to_string();
                let path = document.uri.to_file_path().ok();
                self.documents.insert(
                    key.clone(),
                    Document {
                        uri: document.uri,
                        path: path.clone(),
                        text: document.text,
                        version: document.version,
                        analysis: None,
                    },
                );
                self.changed(|d| d.uri.as_str() == key);
                if !self.watch_files {
                    self.relist(path.as_deref());
                }
                self.publish_at = Some(Instant::now());
            }
            LspNotificationMethod::TextDocumentDidChange => {
                let Ok(params) = notification.extract::<DidChangeTextDocumentParams>(&method)
                else {
                    return;
                };
                let key = params
                    .text_document
                    .text_document_identifier
                    .uri
                    .as_str()
                    .to_string();
                let Some(document) = self.documents.get_mut(&key) else {
                    return;
                };
                // Changes are whole documents, as the server asks for.
                if let Some(change) = params.content_changes.into_iter().last() {
                    document.text = match change {
                        TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(
                            change,
                        ) => change.text,
                        TextDocumentContentChangeEvent::TextDocumentContentChangePartial(
                            change,
                        ) => change.text,
                    };
                }
                document.version = params.text_document.version;
                self.changed(|d| d.uri.as_str() == key);
                self.publish_at = Some(Instant::now() + EDIT_DEBOUNCE);
            }
            LspNotificationMethod::TextDocumentDidSave => {
                let Ok(params) = notification.extract::<DidSaveTextDocumentParams>(&method) else {
                    return;
                };
                if !self.watch_files {
                    self.relist(params.text_document.uri.to_file_path().ok().as_deref());
                    self.publish_at = Some(Instant::now());
                }
            }
            LspNotificationMethod::TextDocumentDidClose => {
                let Ok(params) = notification.extract::<DidCloseTextDocumentParams>(&method) else {
                    return;
                };
                let key = params.text_document.uri.as_str();
                self.documents.remove(key);
                self.unpublished.remove(key);
                self.send(Notification::new(
                    LspNotificationMethod::TextDocumentPublishDiagnostics.to_string(),
                    PublishDiagnosticsParams {
                        uri: params.text_document.uri,
                        version: None,
                        diagnostics: Vec::new(),
                    },
                ));
            }
            LspNotificationMethod::WorkspaceDidChangeWatchedFiles => {
                // Only git's index is watched.
                self.relist_at = Some(Instant::now() + INDEX_DEBOUNCE);
            }
            LspNotificationMethod::WorkspaceDidChangeConfiguration => {
                #[derive(Deserialize)]
                struct Params {
                    settings: Option<serde_json::Value>,
                }
                let settings = notification
                    .extract::<Params>(&method)
                    .map_err(|e| format!("{e:?}"))
                    .and_then(|params| Settings::parse(params.settings));
                // Bad settings keep the ones before.
                match settings {
                    Ok(settings) => self.settings = settings,
                    Err(e) => eprintln!("{e}"),
                }
            }
            _ => {}
        }
    }

    /// Forget what's known about a document, and publish it again.
    fn changed(&mut self, which: impl Fn(&Document) -> bool) {
        for (key, document) in &mut self.documents {
            if which(document) {
                document.analysis = None;
                self.unpublished.insert(key.clone());
            }
        }
    }

    /// List the files in the repository of the CODEOWNERS file at `path`
    /// again, since nothing says when its git index changes.
    fn relist(&mut self, path: Option<&Path>) {
        let Some(root) = path.map(repo_root_for_codeowners) else {
            return;
        };
        self.files.remove(&root);
        self.changed(|d| d.path.as_deref().map(repo_root_for_codeowners).as_ref() == Some(&root));
    }

    fn handle_request(&mut self, request: Request) {
        let id = request.id.clone();
        let method = request.method.clone();
        let result = match LspRequestMethod::from(method.as_str()) {
            LspRequestMethod::TextDocumentHover => request
                .extract::<HoverParams>(&method)
                .map_err(|e| format!("{e:?}"))
                .map(|(_, params)| serde_json::to_value(self.hover(&params)).unwrap()),
            LspRequestMethod::TextDocumentInlayHint => request
                .extract::<InlayHintParams>(&method)
                .map_err(|e| format!("{e:?}"))
                .map(|(_, params)| serde_json::to_value(self.inlay_hints(&params)).unwrap()),
            LspRequestMethod::TextDocumentCodeAction => request
                .extract::<CodeActionParams>(&method)
                .map_err(|e| format!("{e:?}"))
                .map(|(_, params)| serde_json::to_value(self.code_actions(&params)).unwrap()),
            LspRequestMethod::TextDocumentFormatting => request
                .extract::<DocumentFormattingParams>(&method)
                .map_err(|e| format!("{e:?}"))
                .map(|(_, params)| serde_json::to_value(self.format(&params)).unwrap()),
            LspRequestMethod::Custom(OWNER_METHOD) => request
                .extract::<OwnerParams>(&method)
                .map_err(|e| format!("{e:?}"))
                .and_then(|(_, params)| self.owner(&params))
                .map(|result| serde_json::to_value(result).unwrap()),
            _ => {
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
        let files = match TrackedFiles::read(root) {
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
        let Some(index) = index_path(root) else {
            return;
        };
        let (Some(dir), Some(name)) = (index.parent(), index.file_name()) else {
            return;
        };
        let Ok(dir_uri) = Uri::from_file_path(dir) else {
            return;
        };
        let glob_pattern = if self.relative_patterns {
            GlobPattern::RelativePattern(RelativePattern {
                base_uri: BaseUri::Uri(dir_uri),
                pattern: name.to_string_lossy().into_owned(),
            })
        } else {
            GlobPattern::Pattern(index.to_string_lossy().replace('\\', "/"))
        };
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![FileSystemWatcher {
                glob_pattern,
                kind: None,
            }],
        };
        let registration = Registration {
            id: format!("git-index:{}", index.display()),
            method: LspNotificationMethod::WorkspaceDidChangeWatchedFiles.to_string(),
            register_options: Some(serde_json::to_value(options).unwrap()),
        };
        self.send_request(
            LspRequestMethod::ClientRegisterCapability,
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
            // GitHub ignores the file, so counts would mislead.
            let files = match &root {
                Some(root) if !too_big(&document.text) => self.files(root),
                _ => None,
            };
            let document = self.documents.get_mut(key)?;
            let analysis = analyze(
                &document.text,
                document.path.as_deref().zip(root.as_deref()),
                files.as_deref(),
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
        let params = PublishDiagnosticsParams {
            uri: document.uri.clone(),
            version: Some(document.version),
            diagnostics,
        };
        self.send(Notification::new(
            LspNotificationMethod::TextDocumentPublishDiagnostics.to_string(),
            params,
        ));
    }

    fn hover(&mut self, params: &HoverParams) -> Option<Hover> {
        let position = &params.text_document_position_params;
        let (analysis, _) = self.analysis(position.text_document.uri.as_str())?;
        let offset = analysis.index.offset(position.position)?;
        let counts = analysis.counts.as_ref()?;
        let (rule, &(owned, status)) = analysis.rules.iter().zip(counts).find(|(rule, _)| {
            rule.pattern_span.contains(&offset) || rule.pattern_span.end == offset
        })?;
        // A rule that owns nothing has a diagnostic saying why.
        if status != Status::Active {
            return None;
        }
        Some(Hover {
            contents: Contents::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("Owns {}", plural(owned, "file")),
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
                label: Label::String(plural(owned, "file")),
                kind: None,
                text_edits: None,
                tooltip: None,
                padding_left: Some(true),
                padding_right: None,
                data: None,
            })
            .collect()
    }

    /// Quick fixes for the problems in `params.range` that have one.
    /// Edits that format a document, one for each line that changes, so the
    /// editor keeps the cursor and the rest of the document as it is.
    fn format(&self, params: &DocumentFormattingParams) -> Vec<TextEdit> {
        let Some(document) = self.documents.get(params.text_document.uri.as_str()) else {
            return Vec::new();
        };
        let text = &document.text;
        let index = LineIndex::new(text);
        let formatted = format_with(
            text,
            self.settings.format_align(),
            self.settings.format.spacing,
        );
        let mut start = 0;
        // Formatting keeps lines and their endings, so they pair up.
        text.split('\n')
            .zip(formatted.split('\n'))
            .filter_map(|(raw, new_raw)| {
                let line_start = start;
                start += raw.len() + 1;
                if raw == new_raw {
                    return None;
                }
                let line = raw.strip_suffix('\r').unwrap_or(raw);
                let new_line = new_raw.strip_suffix('\r').unwrap_or(new_raw);
                Some(TextEdit {
                    range: index.range(&(line_start..line_start + line.len())),
                    new_text: new_line.to_string(),
                })
            })
            .collect()
    }

    fn code_actions(&mut self, params: &CodeActionParams) -> Vec<CodeAction> {
        let uri = &params.text_document.uri;
        let Some((analysis, _)) = self.analysis(uri.as_str()) else {
            return Vec::new();
        };
        let wanted = params
            .context
            .only
            .as_ref()
            .is_none_or(|only| only.iter().any(is_quick_fix));
        if !wanted {
            return Vec::new();
        }
        analysis
            .problems
            .iter()
            .filter_map(|problem| {
                let fix = problem.fix.as_ref()?;
                let range = analysis.index.range(problem.span.as_ref()?);
                if range.end < params.range.start || params.range.end < range.start {
                    return None;
                }
                let edit = TextEdit {
                    range,
                    new_text: fix.clone(),
                };
                Some(CodeAction {
                    title: format!("Replace with `{fix}`"),
                    kind: Some(CodeActionKind::QuickFix),
                    diagnostics: Some(vec![diagnostic(&analysis, problem)]),
                    is_preferred: Some(true),
                    edit: Some(WorkspaceEdit {
                        changes: Some(HashMap::from([(uri.clone(), vec![edit])])),
                        ..Default::default()
                    }),
                    ..Default::default()
                })
            })
            .collect()
    }

    fn owner(&mut self, params: &OwnerParams) -> Result<Option<OwnerResult>, String> {
        let path = params
            .uri
            .to_file_path()
            .map_err(|()| format!("{} isn't a file", params.uri))?;
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
        check_size(&repo_display(&root, &codeowners), &text)?;
        let repo_path = repo_path(&root, &path.to_string_lossy())?;
        let rules = parse_codeowners(&text);
        let owner = owners_of(&rule_list(&rules), &[&repo_path])[0];
        let index = LineIndex::new(&text);
        Ok(Some(OwnerResult {
            path: repo_path,
            codeowners: Uri::from_file_path(absolute(&codeowners))
                .map_err(|()| format!("{} isn't absolute", codeowners.display()))?,
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
fn analyze(text: &str, location: Option<(&Path, &Path)>, files: Option<&TrackedFiles>) -> Analysis {
    let rules = parse_codeowners(text);
    let mut problems = find_problems(text);
    if let Some((path, root)) = location {
        problems.extend(location_problem(text, path, root));
    }
    let counts = files.map(|files| {
        count(&Rules::new(&rule_list(&rules)), &files.paths())
            .rules
            .into_iter()
            .map(|c| (c.owned, Status::new(&c)))
            .collect()
    });
    Analysis {
        index: LineIndex::new(text),
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
        ignored_message("this file", root, &ignored_codeowners(root, text)?)
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
        fix: None,
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

/// Whether a client asking for code actions of `kind` wants quick fixes.
fn is_quick_fix(kind: &CodeActionKind) -> bool {
    let kind = String::from(kind.clone());
    let quick_fix = String::from(CodeActionKind::QuickFix);
    // Kinds are hierarchical, so "" wants everything.
    kind.is_empty() || quick_fix == kind || quick_fix.starts_with(&format!("{kind}."))
}

fn diagnostic(analysis: &Analysis, problem: &LineProblem) -> Diagnostic {
    // `check` shows the label under the word with the problem.
    let message = match problem.title_and_label() {
        (title, Some(label)) => format!("{title}; {label}"),
        (title, None) => title.to_string(),
    };
    Diagnostic {
        range: problem
            .span
            .as_ref()
            .map(|span| analysis.index.range(span))
            .unwrap_or_default(),
        severity: Some(if problem.error {
            DiagnosticSeverity::Error
        } else {
            DiagnosticSeverity::Warning
        }),
        source: Some("gh-codeowners".to_string()),
        message: message.into(),
        ..Default::default()
    }
}

fn diagnostics(analysis: &Analysis) -> Vec<Diagnostic> {
    let diagnostic = |problem: &LineProblem| diagnostic(analysis, problem);
    let counts = analysis.counts.iter().flatten();
    let rules = analysis
        .rules
        .iter()
        .zip(counts)
        .map(|(rule, &(_, status))| (rule, status));
    let unused = unused_problems(rules, &analysis.problems)
        .into_iter()
        .map(|problem| Diagnostic {
            // Faded, rather than an error, while editing.
            severity: Some(DiagnosticSeverity::Hint),
            tags: Some(vec![DiagnosticTag::Unnecessary]),
            ..diagnostic(&problem)
        });
    analysis
        .problems
        .iter()
        .map(diagnostic)
        .chain(unused)
        .collect()
}
