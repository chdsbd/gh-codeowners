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

Patterns are matched the same way as [@snyk/github-codeowners](https://github.com/snyk/github-codeowners): its gitignore-style conversion from node-ignore 5.2.4, quirks included.
