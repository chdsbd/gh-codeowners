# gh-codeowners

A CLI for [GitHub CODEOWNERS](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners) files.

- Find a file's owner.
- Count the files owned by a rule.
- Find errors in your CODEOWNERS file.

## Install

As a [GitHub CLI](https://cli.github.com) extension (recommended):

```
gh extension install chdsbd/gh-codeowners
```

## Update

```
gh extension upgrade codeowners
```

## Install with Cargo

Cargo installs the `gh-codeowners` command:

```
cargo install --git https://github.com/chdsbd/gh-codeowners
```

## Usage

```
Work with GitHub CODEOWNERS files

Usage: gh codeowners [OPTIONS] <COMMAND>

Commands:
  check  Check CODEOWNERS for problems
  list   List each rule and how many files it owns
  view   Show the rule that owns each path
  lsp    Run a language server for CODEOWNERS files
  help   Print this message or the help of the given subcommand(s)

Options:
      --codeowners <path>  CODEOWNERS file, or - for stdin (default: the one GitHub would use)
      --root <dir>         Repository root (default: the repository containing the current directory)
      --json               Output JSON
  -h, --help               Print help
  -V, --version            Print version
```

### Find a file's owner

```
gh codeowners view app.ts components/button.tsx

src/app.ts: @acme/web, from .github/CODEOWNERS:5 /src/
src/components/button.tsx: @acme/design, from .github/CODEOWNERS:6 /src/components/
```

### List rules and how many files they own

```
gh codeowners list

.github/CODEOWNERS: 7 rules, 22 files, 3 files with no owner, 2 rules owning no files

line  files  pattern
   4      1  /README.md
   5      6  /src/
   6      7  /src/components/
   7      0  /app/[id]/        no files match
   8      4  /docs/
   9      1  !secrets
  10      0  /legacy/          no files match
```

### Check CODEOWNERS for errors

```
gh codeowners check

error: `/app/[id]/` isn't a valid pattern
 --> .github/CODEOWNERS:7:1
  |
7 | /app/[id]/ @acme/web
  | ^^^^^^^^^^ both `[` and `]` must be escaped with a `\`, like `/app/\[id\]/`

warning: `!` doesn't negate a pattern on GitHub
 --> .github/CODEOWNERS:9:1
  |
9 | !secrets @acme/security
  | ^^^^^^^^ this matches files named like the rest of the pattern, but not directories
```

### Check CODEOWNERS for errors or unused rules

```
gh codeowners check --unused

error: `/apps/web/` owns no files
 --> .github/CODEOWNERS:8:1
  |
8 | /apps/web/ @acme/web
  | ^^^^^^^^^^ no files match
```

### Editor support

Install [VS Code GitHub Code Owners](https://marketplace.visualstudio.com/items?itemName=chdsbd.github-code-owners) for editor support.

## Comparison

How well gh-codeowners, and others match GitHub's behavior. Count speed is the time to check every file in a large open-source repository (773 rules, 22,256 files), with each tool doing its closest equivalent; a dash means it wasn't timed.

| Library | Language | Agrees with GitHub | Count speed |
|---|---|---|---|
| gh-codeowners (this repository) | Rust | 4157/4157 | 39 ms |
| [sbdchd/codeowners](https://github.com/sbdchd/codeowners) | Python | 4039/4157 | — |
| [hmarr/codeowners](https://github.com/hmarr/codeowners) | Go | 4021/4157 | 495 ms |
| [codeowners-tool](https://github.com/jordonpeterson/codeowners-tool) | Go | 4021/4157 | 1.4 s |
| [codeowners-deadzone](https://www.npmjs.com/package/codeowners-deadzone) | Node | 4013/4157 | 97 s |
| [snyk/github-codeowners](https://github.com/snyk/github-codeowners), via the [chdsbd/github-codeowners](https://github.com/chdsbd/github-codeowners) fork | Node | 4005/4157 | 30 s |
| [codeowners-rs](https://crates.io/crates/codeowners-rs) | Rust | 4004/4157 | 59 ms |
| [jjmschofield/github-codeowners](https://github.com/jjmschofield/github-codeowners) | Node | 4004/4157 | 26 s |
| [softprops/codeowners](https://crates.io/crates/codeowners) | Rust | 3993/4157 | — |
| [codeowners-audit](https://github.com/watson/codeowners-audit) | Node | 3991/4157 | 2.6 s |
| [codeowners-validator](https://github.com/mszostok/codeowners-validator) | Go | 3990/4157 | 1.1 s |
| [beaugunderson/codeowners](https://github.com/beaugunderson/codeowners) | Node | 3976/4157 | 26 s |

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


See https://github.com/chdsbd/codeowners-comparison for the full comparison.

See [`SPECIFICATION.md`](SPECIFICATION.md) for matching behavior.