# Specification

## Matching rules

gh-codeowners matches patterns the way GitHub does. Patterns follow [gitignore rules](https://git-scm.com/docs/gitignore#_pattern_format), with [GitHub's exceptions](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/about-code-owners#codeowners-syntax) listed below. Each rule was checked against GitHub itself, by comparing which rule GitHub shows as owning each file in a test repository with thousands of random rules and paths.

The last rule that matches a file sets its owners, so later rules override earlier ones. A rule with no owners is valid, and leaves the files it matches with no owner.

### Patterns

| Rule | Example |
|---|---|
| Paths are case-sensitive. | `/Docs/` doesn't own `docs/a.md` |
| A pattern ending in `/*` doesn't match files in subdirectories. | `docs/*` owns `docs/a.md`, not `docs/sub/b.md` |
| A pattern ending in `/**/` matches files too, not only directories. | `a/**/` owns `a/x` |
| `?` matches exactly one byte of the UTF-8 path, not one character. | `??.txt` owns `é.txt`; `?.txt` doesn't |
| There are no character ranges. `[` and `]` must be escaped as `\[` and `\]`. | `x\[id\]` owns `x[id]` |
| Escaped brackets also match the backslashes literally, but not a mix of the two. | `x\[id\]` owns `x\[id\]`, not `x\[id]` |
| An unescaped `#` starts a comment anywhere, even inside a pattern. | `/src/C#/ @acme/x` is the pattern `/src/C` with no owners |
| `\#` escapes a `#`, though GitHub's docs say it doesn't. | `/src/C\#/` owns `src/C#/x` |
| A `\` at the end of a pattern escapes nothing and is dropped. With a slash, the pattern also matches it as a literal `\`. | `a\` owns `x/a`, not `a\`; `/a\` owns `a` and `a\` |

### Negation

There's no negation. What a `!` means depends on whether the pattern has a slash.

| Pattern | Meaning | Example |
|---|---|---|
| `!` with no slash | Matches file names the way the pattern without the `!` would, but never directories | `!foo` owns `x/foo`, but not `foo/bar` |
| `!` with a slash | `!` is an ordinary character | `!a/` owns `!a/b` |

### Lines that own nothing

**Lines GitHub rejects as invalid.** These are lines whose pattern has:

- an unescaped `[` or `]`
- a `\` that isn't followed by a space, tab, `#`, `[`, `]` or `\`, except one at the end of the pattern
- three or more `*`s in a row

**Lines GitHub accepts but never matches.** These are lines whose pattern, after dropping a `!` from one without a slash, has:

- a different number of `\[`s and `\]`s
- a `\[` at the start and no slash
- nothing but slashes, like `/`
- a `.` or `..` segment, like `./src/`, since paths in git never have one

### Owners

A line is skipped if any of its owners is something other than:

- an `@user`, of letters, digits, `-` and `_`
- an `@org/team`, of the same characters
- an email address whose domain ends in a dot and two or more letters, like `a@example.com`

Users and teams that don't exist don't make a line invalid. The line still applies, but GitHub can't request a review from them.

### The CODEOWNERS file

- **Size:** GitHub ignores a CODEOWNERS file over 3 MiB (3,145,728 bytes), though its docs say 3 MB. gh-codeowners reports an error for one.
- **Location:** GitHub uses the first CODEOWNERS file it finds, looking in this order:
  1. `.github/CODEOWNERS`
  2. `CODEOWNERS`
  3. `docs/CODEOWNERS`

  It uses that file even if it's empty or only comments. gh-codeowners warns when that leaves a later file's rules unused.

### Examples

[`examples/CODEOWNERS`](examples/CODEOWNERS) has a commented rule for each of these, with notes on which other CODEOWNERS libraries get it wrong.
