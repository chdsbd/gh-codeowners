# codeowners-stats

Count the files each [CODEOWNERS](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners) rule owns, and find rules that own nothing.

A file counts towards the last rule that matches it, like on GitHub, so a rule can match files and still own none of them.

```
$ codeowners-stats
.github/CODEOWNERS: 12 rules, 163 files, 0 files with no owner, 2 rules owning no files

line  files  pattern
   2      3  *
   5     40  /web/
  12      0  /api/auth/  no files match
  20      0  /docs/      every matching file is owned by a later rule
  21      8  *.md
```

Files are the ones in git's index: tracked files, which are what GitHub sees once committed. It reads `.git/index` directly on several threads and counts while reading, so a repository with 2 million files and 2,000 rules takes about 60 ms. Indexes it can't read, like split and sparse indexes, fall back to `git ls-files`.

## Install

```
cargo install --git https://github.com/chdsbd/codeowners-stats
```

## CLI

```
codeowners-stats [options] [root]

  --codeowners <path>  CODEOWNERS file (default: the one GitHub would use)
  --unused             Only list rules that own no files
  --check              Exit with status 1 if any rule owns no files
  --json               Print results as JSON
  -j, --threads <n>    Threads to count with (default: one per CPU)
```

To fail CI when CODEOWNERS has dead rules:

```
codeowners-stats --check --unused
```

## Library

```rust
use codeowners_stats::codeowners::parse_codeowners;
use codeowners_stats::count_tracked_files;
use codeowners_stats::counter::Rule;

let rules: Vec<Rule> = parse_codeowners(&text)
    .into_iter()
    .map(|r| Rule { pattern: r.pattern, lineno: r.lineno })
    .collect();
let counts = count_tracked_files(root, &rules, threads)?;
for rule in &counts.rules {
    println!("{} {} {}", rule.lineno, rule.owned, rule.matches_any);
}
```

`lineno` is zero-based. A rule with `owned == 0` and `matches_any` is overridden by later rules; without `matches_any`, it matches nothing.

## Matching

Patterns are matched like on GitHub: by [gitignore rules](https://git-scm.com/docs/gitignore#_pattern_format), with [GitHub's exceptions](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners#codeowners-syntax). Paths are case-sensitive, a pattern ending in `/*` doesn't match files in subdirectories, one ending in `/**/` matches files too, and `?` matches one byte, so `??` matches `é`. There are no character ranges: `[` and `]` must be escaped as `\[` and `\]`.

There's no negation either. A `!` pattern without a slash matches file names like the pattern without the `!` would, but not directories, so `!foo` owns `x/foo` but not `foo/bar`. In a pattern with a slash, `!` is an ordinary character, so `!a/` owns `!a/b`.

Lines GitHub rejects as invalid own nothing, such as ones with an unescaped bracket, a `\` before anything but a space, tab, `#`, `[`, `]` or `\`, or `***`. So do a few lines GitHub accepts but never matches: a different number of `\[`s and `\]`s, and a pattern starting with `\[` that has no slash.

Like on GitHub, a line is skipped if any owner isn't an `@user`, an `@org/team`, or an email address with a dot in its domain. Users and teams that don't exist don't make a line invalid. GitHub ignores a CODEOWNERS file over 3 MiB (its docs say 3 MB), so codeowners-stats reports an error for one.

These rules were checked against GitHub itself, by comparing which rule GitHub shows as owning each file in a test repository with thousands of random rules and paths.

## Testing

`cargo test` checks the matcher against every file ownership recorded from GitHub in [`tests/github.txt`](tests/github.txt).

To see how other CODEOWNERS libraries do on the same cases, run [`compare/compare.py`](compare/compare.py) with cargo, go and npm on your PATH. It compares [codeowners-rs](https://github.com/hmarr/codeowners-rs), [hmarr/codeowners](https://github.com/hmarr/codeowners), [sbdchd/codeowners](https://github.com/sbdchd/codeowners) and the [@snyk/github-codeowners](https://github.com/snyk/github-codeowners) fork used by the VS Code extension:

```
uv run compare/compare.py
```
