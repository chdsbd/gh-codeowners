//! Lists the files in git's index by reading .git/index directly, falling
//! back to `git ls-files` for indexes it can't read.
//! https://git-scm.com/docs/index-format

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use memmap2::Mmap;

#[derive(Debug)]
pub enum Error {
    // The index uses a feature this reader doesn't handle.
    Unsupported(String),
    Io(io::Error),
    Git(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Unsupported(message) | Error::Git(message) => f.write_str(message),
            Error::Io(error) => error.fmt(f),
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Io(error)
    }
}

fn truncated() -> Error {
    Error::Io(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "Truncated git index",
    ))
}

/// The per-worktree git directory (holding the index) and the shared one
/// (holding the config).
pub fn git_dirs(root: &Path) -> Result<(PathBuf, PathBuf), Error> {
    let dot_git = root.join(".git");
    let metadata = fs::metadata(&dot_git)
        .map_err(|_| Error::Unsupported(format!("No .git in {}", root.display())))?;
    let mut git_dir = dot_git.clone();
    // Worktrees and submodules have a .git file pointing at their git dir.
    if metadata.is_file() {
        let text = fs::read_to_string(&dot_git)?;
        let target = text
            .lines()
            .find_map(|line| line.strip_prefix("gitdir: "))
            .ok_or_else(|| {
                Error::Unsupported(format!("Unrecognized .git file in {}", root.display()))
            })?;
        git_dir = root.join(target.trim());
    }
    let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(relative) => git_dir.join(relative.trim()),
        Err(_) => git_dir.clone(),
    };
    Ok((git_dir, common_dir))
}

// SHA-256 repositories store 32-byte object IDs in index entries.
fn hash_size(common_dir: &Path) -> usize {
    let Ok(config) = fs::read_to_string(common_dir.join("config")) else {
        return 20;
    };
    let mut section = String::new();
    for line in config.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[') {
            section = header
                .split(|c: char| c == ']' || c.is_whitespace())
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();
            continue;
        }
        if section == "extensions"
            && let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("objectformat")
            && value.trim().eq_ignore_ascii_case("sha256")
        {
            return 32;
        }
    }
    20
}

/// A git index, mapped into memory.
pub struct Index {
    // None before anything is added to a new repository.
    map: Option<Mmap>,
    hash_size: usize,
}

/// Paths from an index, in index order. Version 2 and 3 paths point into the
/// mapped index; version 4 and `git ls-files` paths are in `arena`.
#[derive(Default)]
pub struct Batch {
    arena: Vec<u8>,
    in_arena: bool,
    // (start, end) of each path.
    spans: Vec<(usize, usize)>,
}

impl Batch {
    pub fn len(&self) -> usize {
        self.spans.len()
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// The batch's paths. `index` is the index the batch came from.
    pub fn paths<'a>(&'a self, index: Option<&'a Index>) -> impl Iterator<Item = &'a [u8]> {
        let bytes = if self.in_arena {
            &self.arena[..]
        } else {
            index.map_or(&[][..], Index::bytes)
        };
        self.spans
            .iter()
            .map(move |&(start, end)| &bytes[start..end])
    }
}

impl Index {
    pub fn open(root: &Path) -> Result<Index, Error> {
        let (git_dir, common_dir) = git_dirs(root)?;
        let hash_size = hash_size(&common_dir);
        let file = match fs::File::open(git_dir.join("index")) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Index {
                    map: None,
                    hash_size,
                });
            }
            Err(error) => return Err(error.into()),
        };
        // Safety: git replaces the index by renaming a new file over it, so
        // the mapped file isn't modified underneath us.
        let map = unsafe { Mmap::map(&file)? };
        Ok(Index {
            map: Some(map),
            hash_size,
        })
    }

    fn bytes(&self) -> &[u8] {
        self.map.as_deref().unwrap_or(&[])
    }

    /// Like `for_each_batch`, but parses version 2 and 3 indexes on `threads`
    /// threads at once, with `on_batch` called from any of them.
    ///
    /// Entries vary in length, so each thread past the first guesses where an
    /// entry starts in its part of the file. Guesses are checked once every
    /// thread finishes: each thread must stop exactly where the next one
    /// started, and the entries must add up to the header's count. Since the
    /// first thread starts at the first entry, that proves every guess right.
    /// Returns false if a check fails, in which case the batches were wrong
    /// and the caller should start over with `for_each_batch`.
    pub fn for_each_batch_parallel(
        &self,
        threads: usize,
        batch_size: usize,
        on_batch: impl Fn(Batch) + Sync,
    ) -> Result<bool, Error> {
        let data = self.bytes();
        let version = if data.len() >= 8 {
            u32::from_be_bytes(data[4..8].try_into().unwrap())
        } else {
            0
        };
        if threads < 2
            || data.len() < PARALLEL_MIN_BYTES
            || &data[..4] != b"DIRC"
            || !(2..=3).contains(&version)
        {
            self.for_each_batch(batch_size, on_batch)?;
            return Ok(true);
        }
        let count = u32::from_be_bytes(data[8..12].try_into().unwrap()) as usize;
        let parser = EntryParser {
            data,
            hash_size: self.hash_size,
            version,
        };
        // Where each thread starts. Entries start 4 bytes past a multiple of 8,
        // since the first starts at 12 and each is padded to a multiple of 8.
        let mut starts = vec![12];
        for k in 1..threads {
            let guess = (12 + (data.len() - 12) * k / threads) & !7 | 4;
            let found = (0..SEARCH_ENTRIES)
                .map(|i| guess + i * 8)
                .find(|&candidate| parser.looks_like_entries(candidate));
            if let Some(start) = found
                && start > *starts.last().unwrap()
            {
                starts.push(start);
            }
        }
        let segments: Vec<Result<Segment, Error>> = std::thread::scope(|scope| {
            let workers: Vec<_> = starts
                .iter()
                .enumerate()
                .map(|(k, &start)| {
                    let end = starts.get(k + 1).copied();
                    let (parser, on_batch) = (&parser, &on_batch);
                    scope.spawn(move || parser.segment(start, end, batch_size, on_batch))
                })
                .collect();
            workers.into_iter().map(|w| w.join().unwrap()).collect()
        });
        let segments = segments.into_iter().collect::<Result<Vec<_>, _>>()?;
        let entries: usize = segments.iter().map(|s| s.entries).sum();
        let aligned = segments.windows(2).all(|pair| {
            pair[0].end == pair[1].start
                // A conflicted file split across two threads would count twice.
                && pair[0].last.zip(pair[1].first).is_none_or(|((a, b), (c, d))| data[a..b] != data[c..d])
        });
        if !aligned || entries != count {
            return Ok(false);
        }
        check_extensions(data, segments.last().unwrap().end, self.hash_size)?;
        Ok(true)
    }

    /// Parse the index, calling `on_batch` with up to `batch_size` paths at
    /// a time. Conflicted files, which have an entry per stage, are listed
    /// once.
    ///
    /// Returns Error::Unsupported, possibly after some batches, for indexes
    /// this can't read in full, like split and sparse indexes.
    pub fn for_each_batch(
        &self,
        batch_size: usize,
        mut on_batch: impl FnMut(Batch),
    ) -> Result<(), Error> {
        let data = self.bytes();
        if data.is_empty() {
            return Ok(());
        }
        if data.len() < 12 || &data[..4] != b"DIRC" {
            return Err(Error::Unsupported("Not a git index".into()));
        }
        let version = u32::from_be_bytes(data[4..8].try_into().unwrap());
        let count = u32::from_be_bytes(data[8..12].try_into().unwrap()) as usize;
        if !(2..=4).contains(&version) {
            return Err(Error::Unsupported(format!(
                "Unsupported index version {version}"
            )));
        }
        let hash_size = self.hash_size;
        // Stat data, then object ID and flags.
        let fixed = 40 + hash_size + 2;
        let new_batch = || Batch {
            arena: Vec::new(),
            in_arena: version == 4,
            spans: Vec::with_capacity(batch_size),
        };
        let mut batch = new_batch();
        // Version 4 stores each path as a change to the previous one.
        let mut previous: Vec<u8> = Vec::new();
        let mut last: Option<(usize, usize)> = None;
        let mut pos = 12;
        for i in 0..count {
            let header = data.get(pos..pos + fixed).ok_or_else(truncated)?;
            let mode = u32::from_be_bytes(header[24..28].try_into().unwrap());
            if mode & 0o170000 == 0o040000 {
                return Err(Error::Unsupported("Sparse indexes aren't supported".into()));
            }
            let flags = u16::from_be_bytes(header[40 + hash_size..fixed].try_into().unwrap());
            // Extended flags (version 3 and up) add two bytes.
            let header_size = fixed + if flags & 0x4000 != 0 { 2 } else { 0 };
            pos += header_size;
            if version == 4 {
                let mut drop = 0usize;
                loop {
                    let byte = *data.get(pos).ok_or_else(truncated)?;
                    pos += 1;
                    drop = (drop << 7) | usize::from(byte & 0x7f);
                    if byte & 0x80 == 0 {
                        break;
                    }
                    drop += 1;
                }
                let nul = memchr::memchr(0, data.get(pos..).ok_or_else(truncated)?)
                    .ok_or_else(truncated)?;
                let suffix = &data[pos..pos + nul];
                pos += nul + 1;
                // The same path as the previous entry: another stage.
                if drop == 0 && suffix.is_empty() && i > 0 {
                    continue;
                }
                let keep = previous.len().checked_sub(drop).ok_or_else(truncated)?;
                previous.truncate(keep);
                previous.extend_from_slice(suffix);
                let start = batch.arena.len();
                batch.arena.extend_from_slice(&previous);
                batch.spans.push((start, batch.arena.len()));
            } else {
                let nul = memchr::memchr(0, data.get(pos..).ok_or_else(truncated)?)
                    .ok_or_else(truncated)?;
                let span = (pos, pos + nul);
                // Entries are padded with 1-8 NULs to a multiple of 8 bytes.
                pos += nul + 8 - (header_size + nul) % 8;
                if last.is_some_and(|(s, e)| data[s..e] == data[span.0..span.1]) {
                    continue;
                }
                last = Some(span);
                batch.spans.push(span);
            }
            if batch.spans.len() == batch_size {
                on_batch(std::mem::replace(&mut batch, new_batch()));
            }
        }
        check_extensions(data, pos, hash_size)?;
        if !batch.is_empty() {
            on_batch(batch);
        }
        Ok(())
    }
}

// Only parse in parallel when there's enough to split.
const PARALLEL_MIN_BYTES: usize = 4 << 20;
// How many positions to try when looking for an entry to start at.
const SEARCH_ENTRIES: usize = 1 << 16;
// How many entries in a row must look valid to start at the first.
const CONFIRM_ENTRIES: usize = 8;

// One thread's part of a parallel parse.
struct Segment {
    start: usize,
    // Where parsing stopped: the next segment's start, or the end of the
    // entries for the last segment.
    end: usize,
    // Entries parsed, including each stage of conflicted files.
    entries: usize,
    first: Option<(usize, usize)>,
    last: Option<(usize, usize)>,
}

// Parses version 2 and 3 entries, which don't depend on earlier entries.
struct EntryParser<'a> {
    data: &'a [u8],
    hash_size: usize,
    version: u32,
}

impl EntryParser<'_> {
    /// Parse the entry at `pos` if it looks valid, returning its path and the
    /// next entry's position.
    fn entry(&self, pos: usize) -> Option<(u32, (usize, usize), usize)> {
        let fixed = 40 + self.hash_size + 2;
        let header = self.data.get(pos..pos + fixed)?;
        let mode = u32::from_be_bytes(header[24..28].try_into().unwrap());
        if mode > 0o177777 || !matches!(mode & 0o170000, 0o100000 | 0o120000 | 0o160000 | 0o040000)
        {
            return None;
        }
        let flags = u16::from_be_bytes(header[40 + self.hash_size..fixed].try_into().unwrap());
        let extended = flags & 0x4000 != 0;
        if extended && self.version < 3 {
            return None;
        }
        let header_size = fixed + if extended { 2 } else { 0 };
        if extended {
            // Only skip-worktree and intent-to-add are defined.
            let more = self.data.get(pos + fixed..pos + header_size)?;
            if u16::from_be_bytes(more.try_into().unwrap()) & !0x6000 != 0 {
                return None;
            }
        }
        let name = pos + header_size;
        let length = memchr::memchr(0, self.data.get(name..)?)?;
        let stored = usize::from(flags & 0xfff);
        if length == 0 || (stored < 0xfff && stored != length) {
            return None;
        }
        let next = name + length + 8 - (header_size + length) % 8;
        if self.data.get(name + length..next)?.iter().any(|&b| b != 0) {
            return None;
        }
        Some((mode, (name, name + length), next))
    }

    fn looks_like_entries(&self, mut pos: usize) -> bool {
        for _ in 0..CONFIRM_ENTRIES {
            match self.entry(pos) {
                Some((_, _, next)) => pos = next,
                None => return false,
            }
        }
        true
    }

    /// Parse entries from `start` until `end`, or until they stop looking
    /// valid for the last segment.
    fn segment(
        &self,
        start: usize,
        end: Option<usize>,
        batch_size: usize,
        on_batch: &impl Fn(Batch),
    ) -> Result<Segment, Error> {
        let mut segment = Segment {
            start,
            end: start,
            entries: 0,
            first: None,
            last: None,
        };
        let mut batch = Batch {
            arena: Vec::new(),
            in_arena: false,
            spans: Vec::with_capacity(batch_size),
        };
        let mut pos = start;
        while end.is_none_or(|end| pos < end) {
            let Some((mode, span, next)) = self.entry(pos) else {
                break;
            };
            if mode & 0o170000 == 0o040000 {
                return Err(Error::Unsupported("Sparse indexes aren't supported".into()));
            }
            pos = next;
            segment.entries += 1;
            if segment
                .last
                .is_some_and(|(s, e)| self.data[s..e] == self.data[span.0..span.1])
            {
                continue;
            }
            segment.first.get_or_insert(span);
            segment.last = Some(span);
            batch.spans.push(span);
            if batch.spans.len() == batch_size {
                on_batch(std::mem::replace(
                    &mut batch,
                    Batch {
                        arena: Vec::new(),
                        in_arena: false,
                        spans: Vec::with_capacity(batch_size),
                    },
                ));
            }
        }
        if !batch.is_empty() {
            on_batch(batch);
        }
        segment.end = pos;
        Ok(segment)
    }
}

// Required extensions start with a lowercase letter. Those for split and
// sparse indexes mean the entries aren't the full list of files.
fn check_extensions(data: &[u8], mut pos: usize, hash_size: usize) -> Result<(), Error> {
    while pos + 8 <= data.len().saturating_sub(hash_size) {
        match &data[pos..pos + 4] {
            b"link" => return Err(Error::Unsupported("Split indexes aren't supported".into())),
            b"sdir" => return Err(Error::Unsupported("Sparse indexes aren't supported".into())),
            _ => {}
        }
        pos += 8 + u32::from_be_bytes(data[pos + 4..pos + 8].try_into().unwrap()) as usize;
    }
    Ok(())
}

/// List the index with `git ls-files`, which handles every index git can
/// read, but is slower and loads the whole index into memory.
pub fn ls_files(root: &Path) -> Result<Batch, Error> {
    let output = Command::new("git")
        .args(["ls-files", "-z", "--cached"])
        .current_dir(root)
        .output()
        .map_err(|error| Error::Git(format!("Couldn't run git: {error}")))?;
    if !output.status.success() {
        return Err(Error::Git(format!(
            "git ls-files exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(Batch::from_nul_separated(output.stdout))
}

impl Batch {
    /// Paths separated by NULs, as `git ls-files -z` prints them.
    pub fn from_nul_separated(arena: Vec<u8>) -> Batch {
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let mut start = 0;
        for end in memchr::memchr_iter(0, &arena) {
            // Conflicted files are listed once per stage.
            if !spans
                .last()
                .is_some_and(|&(s, e)| arena[s..e] == arena[start..end])
            {
                spans.push((start, end));
            }
            start = end + 1;
        }
        Batch {
            arena,
            in_arena: true,
            spans,
        }
    }
}
