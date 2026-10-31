# gh-codeowners

Count the files each [CODEOWNERS](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners) rule owns, and find rules that own nothing.

A file counts towards the last rule that matches it, like on GitHub, so a rule can match files and still own none of them.

```
$ gh-codeowners
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

As a [GitHub CLI](https://cli.github.com) extension, which downloads a prebuilt binary:

```
gh extension install chdsbd/gh-codeowners
gh codeowners
```

Or with Cargo, which installs the `gh-codeowners` command:

```
cargo install --git https://github.com/chdsbd/gh-codeowners
```

## CLI

```
gh-codeowners [options] [root]

  root                 Repository root (default: the git repository
                       containing the current directory)
  --codeowners <path>  CODEOWNERS file, or - for stdin (default: the one
                       GitHub would use)
  --owner-of <path>    Instead of counting, print the rule that owns a file,
                       which needn't be tracked. Relative paths are relative
                       to the current directory. Repeat for more files.
  --errors             Instead of counting, list lines GitHub rejects, and
                       lines that likely don't do what they look like
  --unused             Only list rules that own no files
  --check              Exit with status 1 if any rule owns no files, or with
                       --errors, if any line has an error
  --json               Print results as JSON
  -j, --threads <n>    Threads to count with (default: one per CPU)
```

It works from any directory in the repository. To find who owns a file, like GitHub shows when you hover over its code owners icon, or a directory:

```
$ cd src
$ gh-codeowners --owner-of app.ts --owner-of components
src/app.ts: @acme/web, from .github/CODEOWNERS:5 /src/
src/components/: @acme/design, from .github/CODEOWNERS:6 /src/components/
```

To check CODEOWNERS for mistakes, including lines GitHub accepts but that never match:

```
$ gh-codeowners --errors
.github/CODEOWNERS:7: error: both `[` and `]` must be escaped with a `\`
.github/CODEOWNERS:9: warning: `!` doesn't negate a pattern on GitHub - this matches files named like the rest of the pattern, but not directories
```

To fail CI when CODEOWNERS has dead rules:

```
gh-codeowners --check --unused
```

## Library

```rust
use gh_codeowners::codeowners::parse_codeowners;
use gh_codeowners::count_tracked_files;
use gh_codeowners::counter::Rule;

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

Like on GitHub, a line is skipped if any owner isn't an `@user`, an `@org/team`, or an email address with a dot in its domain. Users and teams that don't exist don't make a line invalid. GitHub ignores a CODEOWNERS file over 3 MiB (its docs say 3 MB), so `gh-codeowners` reports an error for one.

These rules were checked against GitHub itself, by comparing which rule GitHub shows as owning each file in a test repository with thousands of random rules and paths.

## WebAssembly

[`wasm/`](wasm) builds the library as WebAssembly, for the VS Code extension on platforms without the native CLI. WebAssembly can't read git's index, so the caller passes the list of files, like `git ls-files -z` prints it. See [`wasm/src/lib.rs`](wasm/src/lib.rs) for the interface.

```
cargo build --release --target wasm32-unknown-unknown --manifest-path wasm/Cargo.toml
```

## Releasing

Bump the version in `Cargo.toml`, then push a matching tag:

```
git tag v0.1.0
git push origin v0.1.0
```

[`release.yml`](.github/workflows/release.yml) builds a binary for each platform and attaches it to a GitHub release, named like `gh-codeowners-darwin-arm64`, which is how `gh extension install` finds it.

## Testing

`cargo test` checks the matcher against every file ownership recorded from GitHub in [`tests/github.txt`](tests/github.txt).

To see how other CODEOWNERS libraries do on the same cases, run [`compare/compare.py`](compare/compare.py) with cargo, go and npm on your PATH. It compares [codeowners-rs](https://github.com/hmarr/codeowners-rs), [hmarr/codeowners](https://github.com/hmarr/codeowners), [sbdchd/codeowners](https://github.com/sbdchd/codeowners) and the [@snyk/github-codeowners](https://github.com/snyk/github-codeowners) fork used by the VS Code extension:

```
uv run compare/compare.py
```
