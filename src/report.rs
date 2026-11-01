//! JSON output, shared by the CLI and the WebAssembly build.

use serde::Serialize;

use crate::codeowners::{CodeownersRule, LineProblem};

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    // Determines the owners of at least one file.
    Active,
    // Matches files, but a later rule determines all their owners.
    Overridden,
    // Matches no files at all.
    Unmatched,
}

impl Status {
    /// The status of a rule that owns `owned` files, and matches any.
    pub fn new(owned: usize, matches_any: bool) -> Status {
        if owned > 0 {
            Status::Active
        } else if matches_any {
            Status::Overridden
        } else {
            Status::Unmatched
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            Status::Active => "",
            Status::Overridden => "every matching file is owned by a later rule",
            Status::Unmatched => "no files match",
        }
    }
}

/// `value` as JSON, laid out like `JSON.stringify(value, null, 2)`.
pub fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string_pretty(value).expect("JSON output")
}

/// A rule, with how many files it owns.
#[derive(Serialize)]
pub struct RuleJson<'a> {
    line: usize,
    pattern: &'a str,
    owners: &'a [String],
    files: usize,
    status: Status,
}

pub fn rule_json(rule: &CodeownersRule, files: usize, status: Status) -> RuleJson<'_> {
    RuleJson {
        line: rule.lineno + 1,
        pattern: &rule.pattern,
        owners: &rule.owners,
        files,
        status,
    }
}

/// A file, with the rule that owns it, if any.
#[derive(Serialize)]
pub struct OwnershipJson<'a> {
    path: &'a str,
    line: Option<usize>,
    pattern: Option<&'a str>,
    owners: &'a [String],
}

pub fn ownership_json<'a>(path: &'a str, rule: Option<&'a CodeownersRule>) -> OwnershipJson<'a> {
    OwnershipJson {
        path,
        line: rule.map(|r| r.lineno + 1),
        pattern: rule.map(|r| r.pattern.as_str()),
        owners: rule.map_or(&[], |r| &r.owners),
    }
}

/// A problem in a CODEOWNERS file.
#[derive(Serialize)]
pub struct ProblemJson<'a> {
    line: Option<usize>,
    severity: &'static str,
    message: &'a str,
}

pub fn problem_json(problem: &LineProblem) -> ProblemJson<'_> {
    ProblemJson {
        line: problem.lineno.map(|n| n + 1),
        severity: if problem.error { "error" } else { "warning" },
        message: &problem.message,
    }
}
