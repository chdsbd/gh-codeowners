//! JSON output, shared by the CLI and the WebAssembly build.

use std::fmt::Write as _;

use crate::codeowners::{CodeownersRule, LineProblem};

#[derive(Clone, Copy, PartialEq, Eq)]
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

    pub fn name(self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Overridden => "overridden",
            Status::Unmatched => "unmatched",
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

pub fn json_string(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32).unwrap(),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub enum Json {
    Null,
    Number(usize),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(&'static str, Json)>),
}

impl Json {
    pub fn string(s: impl Into<String>) -> Json {
        Json::String(s.into())
    }

    pub fn strings(items: &[String]) -> Json {
        Json::Array(items.iter().map(|s| Json::string(s.as_str())).collect())
    }

    // Same layout as JSON.stringify(value, null, 2).
    fn write(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent + 1);
        match self {
            Json::Null => out.push_str("null"),
            Json::Number(n) => write!(out, "{n}").unwrap(),
            Json::String(s) => out.push_str(&json_string(s)),
            Json::Array(items) if items.is_empty() => out.push_str("[]"),
            Json::Array(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    out.push_str(&pad);
                    item.write(out, indent + 1);
                    out.push_str(if i + 1 == items.len() { "\n" } else { ",\n" });
                }
                write!(out, "{}]", "  ".repeat(indent)).unwrap();
            }
            Json::Object(fields) if fields.is_empty() => out.push_str("{}"),
            Json::Object(fields) => {
                out.push_str("{\n");
                for (i, (key, value)) in fields.iter().enumerate() {
                    write!(out, "{pad}{}: ", json_string(key)).unwrap();
                    value.write(out, indent + 1);
                    out.push_str(if i + 1 == fields.len() { "\n" } else { ",\n" });
                }
                write!(out, "{}}}", "  ".repeat(indent)).unwrap();
            }
        }
    }

    pub fn format(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }
}

/// A rule, with how many files it owns.
pub fn rule_json(rule: &CodeownersRule, files: usize, status: Status) -> Json {
    Json::Object(vec![
        ("line", Json::Number(rule.lineno + 1)),
        ("pattern", Json::string(rule.pattern.as_str())),
        ("owners", Json::strings(&rule.owners)),
        ("files", Json::Number(files)),
        ("status", Json::string(status.name())),
    ])
}

/// A file, with the rule that owns it, if any.
pub fn ownership_json(path: &str, rule: Option<&CodeownersRule>) -> Json {
    Json::Object(vec![
        ("path", Json::string(path)),
        (
            "line",
            rule.map_or(Json::Null, |r| Json::Number(r.lineno + 1)),
        ),
        (
            "pattern",
            rule.map_or(Json::Null, |r| Json::string(r.pattern.as_str())),
        ),
        (
            "owners",
            rule.map_or(Json::Array(Vec::new()), |r| Json::strings(&r.owners)),
        ),
    ])
}

/// A problem in a CODEOWNERS file.
pub fn problem_json(problem: &LineProblem) -> Json {
    Json::Object(vec![
        (
            "line",
            problem.lineno.map_or(Json::Null, |n| Json::Number(n + 1)),
        ),
        (
            "severity",
            Json::string(if problem.error { "error" } else { "warning" }),
        ),
        ("message", Json::string(problem.message.as_str())),
    ])
}
