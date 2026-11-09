# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Add `fmt --legacy` and `--alignment-offset` to format like the VS Code extension did before 5.1.0: the first owner of every rule in the file lines up, and nothing else changes.
- Add the language server setting `format`, from `initializationOptions` or `workspace/didChangeConfiguration`, to pick the formatting style: `{ "format": { "style": "legacy", "alignmentOffset": 4 } }`.

## 0.3.0 - 2026-11-09

### Added

- Add `fmt` to format CODEOWNERS like `terraform fmt` (#40).
- Add formatting to the language server (#40).

### Changed

- Move `--json` from global option to commands that support it:  `check`, `list` and `view` (#40).

## 0.2.0 - 2026-11-08

### Added

- Add quick fixes for invalid patterns like escaping `[` and `]` (#39).

### Changed

- Remove duplicate diagnostics for rules that own no files (#39).

## 0.1.0 - 2026-11-08

- Initial release.
