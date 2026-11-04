//! gh-codeowners as WebAssembly, for the VS Code extension on platforms
//! without the native CLI. WebAssembly can't read git's index or run git, so
//! the caller lists the files.
//!
//! Inputs are bytes the caller writes into memory from `alloc`, and frees
//! with `dealloc`. Lists of paths are separated by NUL bytes, like
//! `git ls-files -z` prints them. Each function returns 0 and leaves JSON in
//! the output, which `output_ptr` and `output_len` locate, or returns 1 and
//! leaves an error message there. The JSON matches the CLI's `--json`.

use std::cell::RefCell;

use gh_codeowners::codeowners::{
    CodeownersRule, MAX_CODEOWNERS_BYTES, find_problems, parse_codeowners,
};
use gh_codeowners::counter::{Rule, Rules, count};
use gh_codeowners::owners_of;
use gh_codeowners::report::{
    OwnershipJson, ProblemJson, RuleJson, Status, ownership_json, problem_json, rule_json, to_json,
};
use serde::Serialize;

thread_local! {
    static OUTPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut buffer = Vec::<u8>::with_capacity(len);
    let ptr = buffer.as_mut_ptr();
    std::mem::forget(buffer);
    ptr
}

/// # Safety
///
/// `ptr` and `len` must come from one call to `alloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    drop(unsafe { Vec::from_raw_parts(ptr, 0, len) });
}

#[unsafe(no_mangle)]
pub extern "C" fn output_ptr() -> *const u8 {
    OUTPUT.with_borrow(|output| output.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn output_len() -> usize {
    OUTPUT.with_borrow(|output| output.len())
}

/// Count the files each rule owns. `files` lists the repository's files.
///
/// # Safety
///
/// Each pointer must point to its length in bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn count_files(
    codeowners: *const u8,
    codeowners_len: usize,
    files: *const u8,
    files_len: usize,
) -> u32 {
    let codeowners = unsafe { bytes(codeowners, codeowners_len) };
    let files = unsafe { bytes(files, files_len) };
    respond(rules(codeowners).map(|rules| {
        let files: Vec<&[u8]> = paths(files).collect();
        let counts = count(&Rules::new(&rule_list(&rules)), &files);
        to_json(&CountsJson {
            files: counts.files,
            unowned: counts.unowned,
            rules: rules
                .iter()
                .zip(&counts.rules)
                .map(|(rule, c)| rule_json(rule, c.owned, Status::new(c.owned, c.matches_any)))
                .collect(),
        })
    }))
}

/// The rule that owns each of `paths`, which are relative to the repository
/// root, with `/` separators.
///
/// # Safety
///
/// Each pointer must point to its length in bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn owner_of(
    codeowners: *const u8,
    codeowners_len: usize,
    paths_ptr: *const u8,
    paths_len: usize,
) -> u32 {
    let codeowners = unsafe { bytes(codeowners, codeowners_len) };
    let paths_bytes = unsafe { bytes(paths_ptr, paths_len) };
    respond(rules(codeowners).and_then(|rules| {
        let paths = paths(paths_bytes)
            .map(|path| std::str::from_utf8(path).map_err(|_| "A path isn't UTF-8".to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let owners = owners_of(&rule_list(&rules), &paths);
        Ok(to_json(&OwnershipsJson {
            files: paths
                .iter()
                .zip(owners)
                .map(|(path, rule)| ownership_json(path, rule.map(|i| &rules[i])))
                .collect(),
        }))
    }))
}

/// Problems in a CODEOWNERS file, like the CLI's `--errors`.
///
/// # Safety
///
/// The pointer must point to its length in bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn problems(codeowners: *const u8, codeowners_len: usize) -> u32 {
    let codeowners = unsafe { bytes(codeowners, codeowners_len) };
    respond(text(codeowners).map(|text| {
        to_json(&ProblemsJson {
            problems: find_problems(text).iter().map(problem_json).collect(),
        })
    }))
}

#[derive(Serialize)]
struct CountsJson<'a> {
    files: usize,
    unowned: usize,
    rules: Vec<RuleJson<'a>>,
}

#[derive(Serialize)]
struct OwnershipsJson<'a> {
    files: Vec<OwnershipJson<'a>>,
}

#[derive(Serialize)]
struct ProblemsJson<'a> {
    problems: Vec<ProblemJson<'a>>,
}

unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }
}

fn paths(list: &[u8]) -> impl Iterator<Item = &[u8]> {
    list.split(|&b| b == 0).filter(|path| !path.is_empty())
}

fn text(codeowners: &[u8]) -> Result<&str, String> {
    std::str::from_utf8(codeowners).map_err(|_| "CODEOWNERS isn't UTF-8".to_string())
}

/// The file's rules, or an error if GitHub would ignore the file.
fn rules(codeowners: &[u8]) -> Result<Vec<CodeownersRule>, String> {
    if codeowners.len() as u64 > MAX_CODEOWNERS_BYTES {
        return Err(format!(
            "CODEOWNERS is {} bytes, over GitHub's limit of {MAX_CODEOWNERS_BYTES}, so GitHub ignores it",
            codeowners.len()
        ));
    }
    Ok(parse_codeowners(text(codeowners)?))
}

fn rule_list(rules: &[CodeownersRule]) -> Vec<Rule> {
    rules
        .iter()
        .map(|r| Rule {
            pattern: r.pattern.clone(),
            lineno: r.lineno,
        })
        .collect()
}

fn respond(result: Result<String, String>) -> u32 {
    let (status, output) = match result {
        Ok(json) => (0, json),
        Err(message) => (1, message),
    };
    OUTPUT.set(output.into_bytes());
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> String {
        OUTPUT.with_borrow(|output| String::from_utf8(output.clone()).unwrap())
    }

    #[test]
    fn counts_files() {
        let codeowners = b"* @a\n/docs/ @b\n/missing/ @c\n";
        let files = b"README.md\0docs/a.md\0docs/b/c.md\0";
        let status = unsafe {
            count_files(
                codeowners.as_ptr(),
                codeowners.len(),
                files.as_ptr(),
                files.len(),
            )
        };
        assert_eq!(status, 0);
        let out = output();
        assert!(
            out.starts_with("{\n  \"files\": 3,\n  \"unowned\": 0,"),
            "{out}"
        );
        assert!(
            out.contains("\"files\": 1,\n      \"status\": \"active\""),
            "{out}"
        );
        assert!(
            out.contains("\"files\": 2,\n      \"status\": \"active\""),
            "{out}"
        );
        assert!(out.contains("\"status\": \"unmatched\""), "{out}");
    }

    #[test]
    fn finds_owners() {
        let codeowners = b"* @a\n/docs/ @b\n";
        let paths = b"docs/new.md\0src/x.ts";
        let status = unsafe {
            owner_of(
                codeowners.as_ptr(),
                codeowners.len(),
                paths.as_ptr(),
                paths.len(),
            )
        };
        assert_eq!(status, 0);
        let out = output();
        assert!(
            out.contains("\"path\": \"docs/new.md\",\n      \"line\": 2,"),
            "{out}"
        );
        assert!(
            out.contains("\"path\": \"src/x.ts\",\n      \"line\": 1,"),
            "{out}"
        );
    }

    #[test]
    fn finds_problems() {
        let codeowners = b"[x] @a\n";
        assert_eq!(
            unsafe { problems(codeowners.as_ptr(), codeowners.len()) },
            0
        );
        assert!(output().contains("\"severity\": \"error\""));
    }

    #[test]
    fn reports_errors() {
        let codeowners = [0xff];
        assert_eq!(
            unsafe { problems(codeowners.as_ptr(), codeowners.len()) },
            1
        );
        assert_eq!(output(), "CODEOWNERS isn't UTF-8");
    }
}
