# gh-codeowners

A CLI for [GitHub CODEOWNERS](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners).

Find a file's owner, count the files owned by a rule, and find errors in your CODEOWNERS file.

## Install

As a [GitHub CLI](https://cli.github.com) extension (recommended):

```bash
gh extension install chdsbd/gh-codeowners

# call as `gh codeowners` instead of `gh-codeowners`
gh codeowners list
gh codeowners list --unused
gh codeowners view src/app.ts
gh codeowners check
```

Or with Cargo, which installs the `gh-codeowners` command:

```bash
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
  --json <fields>      Output JSON with the specified fields, like gh. Without
                       fields, lists the ones the command has.
```

`gh codeowners <command> --help` shows each command's flags and JSON fields.

### Find a file's owner

```bash
gh codeowners view app.ts components

src/app.ts: @acme/web, from .github/CODEOWNERS:5 /src/
src/components/: @acme/design, from .github/CODEOWNERS:6 /src/components/
```

### Check CODEOWNERS for errors

```bash
gh codeowners check

.github/CODEOWNERS:7: error: both `[` and `]` must be escaped with a `\`
.github/CODEOWNERS:9: warning: `!` doesn't negate a pattern on GitHub - this matches files named like the rest of the pattern, but not directories
```

### Fail CI on errors or dead rules

```bash
gh codeowners check --unused
```

## Matching

See [`SPECIFICATION.md`](SPECIFICATION.md) for matching behavior.
