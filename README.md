# gh-codeowners

A CLI for [GitHub CODEOWNERS](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners).

- Find a file's owner.
- Count the files owned by a rule.
- Find errors in your CODEOWNERS file.

## TODOs

- [ ] `--online` flag for querying GitHub api to validate teams have write access to repository.

## Install

As a [GitHub CLI](https://cli.github.com) extension (recommended):

```
gh extension install chdsbd/gh-codeowners

gh codeowners list
gh codeowners list --unused
gh codeowners view src/app.ts
gh codeowners check
```

Or with Cargo, which installs the `gh-codeowners` command:

```
cargo install --git https://github.com/chdsbd/gh-codeowners
```

## Usage

```
gh codeowners <command> [flags]

  check   Check CODEOWNERS for lines GitHub rejects, and with --unused, rules
          that own no files. Exits with status 1 if there are errors.
  list    List each rule with how many files it owns
  view    Show the rule that owns each path

  --codeowners <path>  CODEOWNERS file, or - for stdin (default: the one
                       GitHub would use)
  --root <dir>         Repository root (default: the git repository
                       containing the current directory)
  --json               Output JSON
```

### Find a file's owner

```
gh codeowners view app.ts components/button.tsx

src/app.ts: @acme/web, from .github/CODEOWNERS:5 /src/
src/components/button.tsx: @acme/design, from .github/CODEOWNERS:6 /src/components/
```

### Check CODEOWNERS for errors

```
gh codeowners check

.github/CODEOWNERS:7: error: `/app/[id]/` isn't a valid pattern - both `[` and `]` must be escaped with a `\`, like `/app/\[id\]/`
.github/CODEOWNERS:9: warning: `!` doesn't negate a pattern on GitHub - this matches files named like the rest of the pattern, but not directories
```

### Fail CI on errors or dead rules

```
gh codeowners check --unused
```

## Matching

See [`SPECIFICATION.md`](SPECIFICATION.md) for matching behavior.

## Comparison

How often each library matches the same files as GitHub, across the 4,157 CODEOWNERS lines and paths recorded from GitHub in [codeowners-comparison](https://github.com/chdsbd/codeowners-comparison):

| Library | Language | Agrees with GitHub |
|---|---|---|
| gh-codeowners | Rust | 4157/4157 |
| [sbdchd/codeowners](https://github.com/sbdchd/codeowners) | Python | 4039/4157 |
| [hmarr/codeowners](https://github.com/hmarr/codeowners) | Go | 4021/4157 |
| [snyk/github-codeowners](https://github.com/snyk/github-codeowners), via the [chdsbd/github-codeowners](https://github.com/chdsbd/github-codeowners) fork | Node | 4005/4157 |
| [codeowners-rs](https://crates.io/crates/codeowners-rs) | Rust | 4004/4157 |

Where they differ:

| GitHub behavior | gh-codeowners | sbdchd | hmarr | snyk | codeowners-rs |
|---|---|---|---|---|---|
| `!secrets` owns files named `secrets`, not directories | ✓ | ✗ | ✗ | ✗ | ✗ |
| `!important/notes.md` owns the file with the `!` | ✓ | ✓ | ✗ | ✗ | ✓ |
| `?` matches one byte, not one character | ✓ | ✗ | ✗ | ✗ | ✗ |
| `**/` and `/**/` own every file | ✓ | ✗ | ✗ | ✗ | ✓ |
| `***` skips the line | ✓ | ✗ | ✓ | ✗ | ✗ |
| `{a,b}` matches literally | ✓ | ✓ | ✗ | ✓ | ✓ |
| Unescaped `[` or `]` skips the line | ✓ | ✓ | ✗ | ✓ | ✗ |
| `\[draft\]*` without a slash matches nothing | ✓ | ✗ | ✗ | ✗ | ✗ |
| Unpaired escaped brackets match nothing | ✓ | ✗ | ✗ | ✗ | ✗ |
| Escaping anything but space, tab, `#`, `[`, `]` or `\` skips the line | ✓ | ✗ | ✗ | ✗ | ✗ |
| `My\ Documents/` matches the space | ✓ | ✓ | ✓ | ✗ | ✓ |
| `win\\path` matches a backslash | ✓ | ✓ | ✓ | ✓ | ✗ |
| Unescaped `#` starts a comment mid-pattern | ✓ | ✗ | ✓ | ✓ | ✓ |
| `\#notes` matches `#notes` | ✓ | ✓ | ✗ | ✗ | ✓ |
| Patterns are case-sensitive | ✓ | ✓ | ✓ | ✗ | ✓ |
| Non-ASCII patterns like `日本語/` | ✓ | ✓ | ✗ | ✓ | ✓ |

