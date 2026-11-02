# CONTRIBUTING

## Releasing

Bump the version in `Cargo.toml`, then push a matching tag:

```
git tag v0.1.0
git push origin v0.1.0
```

[`release.yml`](.github/workflows/release.yml) builds a binary for each platform and attaches it to a GitHub release, named like `gh-codeowners-darwin-arm64`, which is how `gh extension install` finds it.

## Testing

`cargo test` checks the matcher against every file ownership recorded from GitHub in [`tests/github.txt`](tests/github.txt).

[codeowners-comparison](https://github.com/chdsbd/codeowners-comparison) runs other CODEOWNERS libraries on the same cases.
