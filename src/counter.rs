//! Counts the files each rule owns. The last rule matching each directory
//! is computed once and shared by its files, and rules are indexed by literal
//! prefix or extension so each path is only tested against rules that could
//! match it.
//!
//! Counting splits the files across threads. Each thread keeps its own
//! directory stack, so any split gives the same counts.

use rustc_hash::FxHashMap as HashMap;
use std::rc::Rc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use regex::bytes::Regex;

use crate::pattern;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub pattern: String,
    pub lineno: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleCount {
    pub lineno: usize,
    pub owned: usize,
    // Whether the rule matches any file, even if a later rule owns them all.
    pub matches_any: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Counts {
    pub files: usize,
    // Files no rule matches.
    pub unowned: usize,
    pub rules: Vec<RuleCount>,
}

struct CompiledRule {
    matcher: pattern::Matcher,
    // Patterns ending in "/" only match directories.
    dir_only: bool,
    // Leading path segments without wildcards. Empty for rules
    // that can match anywhere.
    prefix: Vec<u8>,
    // Extension for `*.ext` rules.
    suffix: Option<Vec<u8>>,
}

static SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\*(\.[^*?\\/]+)/?$").unwrap());

impl CompiledRule {
    fn can_match(&self) -> bool {
        !matches!(self.matcher, pattern::Matcher::Never)
    }
}

fn compile_rule(pattern: &str) -> CompiledRule {
    // Without a slash, a `!` pattern only matches files, so it's not a
    // suffix rule. The `!` keeps it out of the suffix index.
    CompiledRule {
        matcher: pattern::Matcher::new(pattern),
        dir_only: pattern::is_dir_only(pattern),
        prefix: literal_prefix(pattern),
        suffix: SUFFIX
            .captures(pattern.as_bytes())
            .map(|caps| caps[1].to_vec()),
    }
}

fn literal_prefix(pattern: &str) -> Vec<u8> {
    let trimmed = pattern.strip_prefix('/').unwrap_or(pattern);
    let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
    // Without a leading or middle slash, a pattern matches at any depth.
    if !pattern.starts_with('/') && !trimmed.contains('/') {
        return Vec::new();
    }
    let literal: Vec<&str> = trimmed
        .split('/')
        .take_while(|segment| !segment.contains(['*', '?', '\\']))
        .collect();
    literal.join("/").into_bytes()
}

/// Rules for `*.ext` patterns, keyed by extension.
#[derive(Default)]
struct SuffixIndex {
    rules: HashMap<Vec<u8>, Vec<u32>>,
    longest: usize,
    // Whether any extension ends with each byte, to skip most paths without
    // a lookup.
    last_bytes: Vec<bool>,
}

impl SuffixIndex {
    fn add(&mut self, suffix: &[u8], rule: u32) {
        self.rules.entry(suffix.to_vec()).or_default().push(rule);
        self.longest = self.longest.max(suffix.len());
        self.last_bytes.resize(256, false);
        if let Some(&last) = suffix.last() {
            self.last_bytes[usize::from(last)] = true;
        }
    }

    /// Call `f` with the rules for each extension `path`'s name ends with.
    /// Rather than testing every extension against every path, this looks up
    /// the suffixes starting at each of the last few dots.
    fn for_each<'r>(&'r self, path: &[u8], mut f: impl FnMut(&'r [u32])) {
        let Some(&last) = path.last() else {
            return;
        };
        if self.rules.is_empty() || !self.last_bytes[usize::from(last)] {
            return;
        }
        let tail = &path[path.len().saturating_sub(self.longest)..];
        let tail = &tail[tail.iter().rposition(|&c| c == b'/').map_or(0, |i| i + 1)..];
        for (i, &c) in tail.iter().enumerate() {
            if c == b'.'
                && let Some(rules) = self.rules.get(&tail[i..])
            {
                f(rules);
            }
        }
    }
}

type List = Rc<[u32]>;

/// Merge two ascending lists of rule indexes, keeping those above `floor`.
/// Returns `a` itself when nothing changes, so directories share lists.
fn merge_above(a: &List, b: Option<&[u32]>, floor: i64) -> List {
    let b = b.unwrap_or(&[]);
    if b.is_empty() && a.first().is_none_or(|&first| i64::from(first) > floor) {
        return a.clone();
    }
    let mut merged = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        let next = if j >= b.len() || (i < a.len() && a[i] < b[j]) {
            i += 1;
            a[i - 1]
        } else {
            j += 1;
            b[j - 1]
        };
        if i64::from(next) > floor {
            merged.push(next);
        }
    }
    merged.into()
}

/// Rule indexes (ascending) grouped by which paths they could match:
/// - by_prefix: paths at or under the rule's literal prefix ("" for anywhere)
/// - by_parent: files directly in the prefix's parent, since `/web/a.css` can
///   match a file inside `web` (file rules only)
/// - by_suffix: paths ending in the extension, for `*.ext` rules
#[derive(Default)]
struct RuleIndex {
    by_prefix: HashMap<Vec<u8>, Vec<u32>>,
    by_parent: HashMap<Vec<u8>, Vec<u32>>,
    by_suffix: SuffixIndex,
}

impl RuleIndex {
    fn new(rules: &[CompiledRule], for_files: bool) -> Self {
        let mut index = RuleIndex::default();
        for (i, rule) in rules.iter().enumerate() {
            let i = i as u32;
            if !rule.can_match() || (for_files && rule.dir_only) {
                continue;
            }
            if let Some(suffix) = &rule.suffix {
                index.by_suffix.add(suffix, i);
                continue;
            }
            index
                .by_prefix
                .entry(rule.prefix.clone())
                .or_default()
                .push(i);
            if for_files && !rule.prefix.is_empty() {
                let parent = match rule.prefix.iter().rposition(|&c| c == b'/') {
                    Some(slash) => rule.prefix[..slash].to_vec(),
                    None => Vec::new(),
                };
                index.by_parent.entry(parent).or_default().push(i);
            }
        }
        index
    }
}

/// Compiled rules, shared by every counting thread.
pub struct Rules {
    linenos: Vec<usize>,
    rules: Vec<CompiledRule>,
    dir_index: RuleIndex,
    file_index: RuleIndex,
}

impl Rules {
    pub fn new(rules: &[Rule]) -> Self {
        let compiled: Vec<CompiledRule> = rules.iter().map(|r| compile_rule(&r.pattern)).collect();
        Rules {
            linenos: rules.iter().map(|r| r.lineno).collect(),
            dir_index: RuleIndex::new(&compiled, false),
            file_index: RuleIndex::new(&compiled, true),
            rules: compiled,
        }
    }
}

struct DirFrame<'a> {
    dir: Vec<u8>,
    // Rules for files directly in this directory.
    by_parent: Option<&'a [u32]>,
    // Index of the last rule matching this directory or an ancestor, or -1.
    winner: i64,
    // Rules above `winner` keyed by this directory or an ancestor.
    dir_candidates: List,
    file_candidates: List,
    // Every rule keyed by this directory or an ancestor.
    any_dir_candidates: List,
    any_file_candidates: List,
}

/// Counts files on one thread.
struct Counter<'a> {
    rules: &'a Rules,
    owned: Vec<usize>,
    matched: Vec<bool>,
    unmatched: usize,
    files: usize,
    unowned: usize,
    stack: Vec<DirFrame<'a>>,
    test_path: Vec<u8>,
}

impl<'a> Counter<'a> {
    fn new(rules: &'a Rules) -> Self {
        let empty: List = Rc::new([]);
        let list = |index: &RuleIndex| -> List {
            index
                .by_prefix
                .get(&b""[..])
                .map_or(empty.clone(), |l| l.as_slice().into())
        };
        let dir_candidates = list(&rules.dir_index);
        let file_candidates = list(&rules.file_index);
        Counter {
            rules,
            owned: vec![0; rules.rules.len()],
            matched: vec![false; rules.rules.len()],
            // Rules that can never match (negations) are left out.
            unmatched: rules.rules.iter().filter(|r| r.can_match()).count(),
            files: 0,
            unowned: 0,
            stack: vec![DirFrame {
                dir: Vec::new(),
                by_parent: rules.file_index.by_parent.get(&b""[..]).map(Vec::as_slice),
                winner: -1,
                any_dir_candidates: dir_candidates.clone(),
                any_file_candidates: file_candidates.clone(),
                dir_candidates,
                file_candidates,
            }],
            test_path: Vec::new(),
        }
    }

    fn add(&mut self, file: &[u8]) {
        let dir_len = file.iter().rposition(|&c| c == b'/').unwrap_or(0);
        self.move_to(file, dir_len);
        let rules = self.rules;
        let frame = self.stack.last().unwrap();
        let by_parent = frame.by_parent;
        let mut winner = self.last_match(Some(&frame.file_candidates), file, frame.winner);
        winner = self.last_match(by_parent, file, winner);
        let tested_above = winner;
        winner = self.match_suffixes(&rules.file_index, file, winner);
        self.files += 1;
        if winner == -1 {
            self.unowned += 1;
        } else {
            self.owned[winner as usize] += 1;
        }
        if self.unmatched > 0 {
            let any = self.stack.last().unwrap().any_file_candidates.clone();
            self.check_matches(Some(&any), file, tested_above);
            self.check_matches(by_parent, file, tested_above);
        }
    }

    /// Move the stack to the directory `path[..dir_len]`.
    fn move_to(&mut self, path: &[u8], dir_len: usize) {
        let is_ancestor = |frame: &DirFrame<'a>| {
            let len = frame.dir.len();
            len == 0
                || (dir_len >= len
                    && (dir_len == len || path[len] == b'/')
                    && path.starts_with(&frame.dir))
        };
        while !is_ancestor(self.stack.last().unwrap()) {
            self.stack.pop();
        }
        loop {
            let len = self.stack.last().unwrap().dir.len();
            if len >= dir_len {
                return;
            }
            let from = if len == 0 { 0 } else { len + 1 };
            let end = path[from..dir_len]
                .iter()
                .position(|&c| c == b'/')
                .map_or(dir_len, |i| from + i);
            let frame = self.enter_dir(&path[..end]);
            self.stack.push(frame);
        }
    }

    fn enter_dir(&mut self, dir: &[u8]) -> DirFrame<'a> {
        let rules = self.rules;
        let mut test_path = std::mem::take(&mut self.test_path);
        test_path.clear();
        test_path.extend_from_slice(dir);
        test_path.push(b'/');
        let parent = self.stack.last().unwrap();
        let (parent_dir, parent_file, parent_any_dir, parent_any_file, parent_winner) = (
            parent.dir_candidates.clone(),
            parent.file_candidates.clone(),
            parent.any_dir_candidates.clone(),
            parent.any_file_candidates.clone(),
            parent.winner,
        );
        let own_dir_rules = rules.dir_index.by_prefix.get(dir).map(Vec::as_slice);
        let mut winner = self.last_match(Some(&parent_dir), &test_path, parent_winner);
        winner = self.last_match(own_dir_rules, &test_path, winner);
        let tested_above = winner;
        winner = self.match_suffixes(&rules.dir_index, dir, winner);
        let any_dir_candidates = merge_above(&parent_any_dir, own_dir_rules, -1);
        if self.unmatched > 0 {
            self.check_matches(Some(&any_dir_candidates), &test_path, tested_above);
        }
        self.test_path = test_path;
        let own_file_rules = rules.file_index.by_prefix.get(dir).map(Vec::as_slice);
        DirFrame {
            dir: dir.to_vec(),
            winner,
            dir_candidates: merge_above(&parent_dir, own_dir_rules, winner),
            file_candidates: merge_above(&parent_file, own_file_rules, winner),
            any_dir_candidates,
            any_file_candidates: merge_above(&parent_any_file, own_file_rules, -1),
            by_parent: rules.file_index.by_parent.get(dir).map(Vec::as_slice),
        }
    }

    fn is_match(&self, i: usize, path: &[u8]) -> bool {
        self.rules.rules[i].matcher.is_match(path)
    }

    // Highest index in `candidates` above `floor` whose rule matches `path`.
    fn last_match(&self, candidates: Option<&[u32]>, path: &[u8], floor: i64) -> i64 {
        let Some(candidates) = candidates else {
            return floor;
        };
        for &i in candidates.iter().rev() {
            if i64::from(i) <= floor {
                break;
            }
            if self.is_match(i as usize, path) {
                return i64::from(i);
            }
        }
        floor
    }

    // `name` is the path without a trailing slash, for matching extensions.
    // The last `*.ext` rule matching `name` above `floor`, recording that
    // each one matches. A `*.ext` rule's regex matches exactly the paths whose
    // name ends with the extension, so finding the rule by extension is
    // enough.
    fn match_suffixes(&mut self, index: &'a RuleIndex, name: &[u8], floor: i64) -> i64 {
        let mut winner = floor;
        let (matched, unmatched) = (&mut self.matched, &mut self.unmatched);
        index.by_suffix.for_each(name, |candidates| {
            winner = winner.max(candidates.last().map_or(-1, |&i| i64::from(i)));
            for &i in candidates {
                if !matched[i as usize] {
                    matched[i as usize] = true;
                    *unmatched -= 1;
                }
            }
        });
        winner
    }

    /// Record which `candidates` match `path`. Only those up to `tested_above`
    /// need testing: `last_match` already found that the candidates above it
    /// don't match. It may be -1, when none matched.
    fn check_matches(&mut self, candidates: Option<&[u32]>, path: &[u8], tested_above: i64) {
        let Some(candidates) = candidates else {
            return;
        };
        for &i in candidates {
            if i64::from(i) > tested_above {
                break;
            }
            let i = i as usize;
            if !self.matched[i] && self.is_match(i, path) {
                self.matched[i] = true;
                self.unmatched -= 1;
            }
        }
    }
}

// Files per unit of work. Small enough to balance threads, big enough that
// re-entering a chunk's ancestor directories costs nothing.
pub const CHUNK: usize = 16 * 1024;

/// Count the files each rule owns, on `threads` threads. Files should be
/// grouped by directory, as git's index lists them; any order is correct,
/// but out-of-order files redo their directories' work.
pub fn count<P: AsRef<[u8]> + Sync>(rules: &Rules, files: &[P], threads: usize) -> Counts {
    let next = AtomicUsize::new(0);
    count_with(rules, threads, |add| {
        let start = next.fetch_add(CHUNK, Ordering::Relaxed);
        if start >= files.len() {
            return false;
        }
        for file in &files[start..(start + CHUNK).min(files.len())] {
            add(file.as_ref());
        }
        true
    })
}

/// Count files on `threads` threads. Each thread calls `work` until it
/// returns false; `work` passes the next files to count to `add`.
pub fn count_with<W>(rules: &Rules, threads: usize, work: W) -> Counts
where
    W: Fn(&mut dyn FnMut(&[u8])) -> bool + Sync,
{
    // Without spawning, which WebAssembly can't.
    if threads <= 1 {
        let mut counter = Counter::new(rules);
        while work(&mut |file| counter.add(file)) {}
        return merge(rules, vec![counter.into_tally()]);
    }
    let tallies: Vec<Tally> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.max(1))
            .map(|_| {
                scope.spawn(|| {
                    let mut counter = Counter::new(rules);
                    while work(&mut |file| counter.add(file)) {}
                    counter.into_tally()
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().unwrap()).collect()
    });
    merge(rules, tallies)
}

// One thread's counts, without its directory stack, which can't leave the
// thread.
struct Tally {
    owned: Vec<usize>,
    matched: Vec<bool>,
    files: usize,
    unowned: usize,
}

impl Counter<'_> {
    fn into_tally(self) -> Tally {
        Tally {
            owned: self.owned,
            matched: self.matched,
            files: self.files,
            unowned: self.unowned,
        }
    }
}

// Combine counts from each thread.
fn merge(rules: &Rules, tallies: impl IntoIterator<Item = Tally>) -> Counts {
    let mut counts = Counts {
        rules: rules
            .linenos
            .iter()
            .map(|&lineno| RuleCount {
                lineno,
                owned: 0,
                matches_any: false,
            })
            .collect(),
        ..Counts::default()
    };
    for counter in tallies {
        counts.files += counter.files;
        counts.unowned += counter.unowned;
        for (rule, (owned, matched)) in counts
            .rules
            .iter_mut()
            .zip(counter.owned.iter().zip(&counter.matched))
        {
            rule.owned += owned;
            rule.matches_any |= *owned > 0 || *matched;
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(patterns: &[&str]) -> Rules {
        let list: Vec<Rule> = patterns
            .iter()
            .enumerate()
            .map(|(lineno, p)| Rule {
                pattern: p.to_string(),
                lineno,
            })
            .collect();
        Rules::new(&list)
    }

    fn owned(counts: &Counts) -> Vec<(usize, bool)> {
        counts
            .rules
            .iter()
            .map(|r| (r.owned, r.matches_any))
            .collect()
    }

    #[test]
    fn counts_files_towards_the_last_matching_rule() {
        let r = rules(&["*", "/web/", "*.css", "/web/old.css", "/missing/", "/web/"]);
        let files = [
            "README.md",
            "Makefile",
            "web/index.ts",
            "web/old.css",
            "web/new.css",
            "api/style.css",
        ];
        for threads in [1, 4] {
            assert_eq!(
                owned(&count(&r, &files, threads)),
                [
                    (2, true),
                    (0, true),
                    (1, true),
                    (0, true),
                    (0, false),
                    (3, true)
                ]
            );
        }
    }

    #[test]
    fn follows_githubs_matching_rules() {
        let r = rules(&[
            "web/",
            "*.TS",
            "/docs/*",
            "/lib",
            "/missing/*.md",
            "/app/\\[id\\]/",
        ]);
        let files = [
            "web",
            "src/web/a.js",
            "x.ts",
            "y.TS",
            "Lib/y.js",
            "lib/x.js",
            "docs/a.md",
            "docs/sub/b.md",
            "app/[id]/page.tsx",
            "app/i/page.tsx",
        ];
        assert_eq!(
            owned(&count(&r, &files, 1)),
            [
                (1, true),
                (1, true),
                (1, true),
                (1, true),
                (0, false),
                (1, true)
            ]
        );
    }
}
