# CONTRIBUTING

## Releasing

Bump the version in `Cargo.toml` and add a section for it to `CHANGELOG.md`, which CI checks with `s/check-version`. When that reaches `main`, [`release.yml`](.github/workflows/release.yml) sees the version has no tag, tags the commit and drafts a GitHub release with the version's changelog section and a binary for each platform, named like `gh-codeowners-darwin-arm64`, which is how `gh extension install` finds it. Publish the draft once it looks right.

## Testing

`cargo test` checks the matcher against every file ownership recorded from GitHub in [`tests/github.txt`](tests/github.txt).

The CLI tests in [`tests/cli.rs`](tests/cli.rs) compare output to inline [insta](https://insta.rs) snapshots. After changing output on purpose, update them with [`cargo insta review`](https://insta.rs/docs/cli/), and review the diff like code.

The formatter's tests snapshot each file in [`tests/format`](tests/format) and the examples in [`src/format.rs`](src/format.rs). To test formatting of a new kind of file, add it to `tests/format` and run `cargo insta test --review`.

[codeowners-comparison](https://github.com/chdsbd/codeowners-comparison) runs other CODEOWNERS libraries on the same cases.
