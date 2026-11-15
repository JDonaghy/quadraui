# Changelog

All notable changes to the `@quadraui/node` package (`quadraui-node`) are
documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This file starts now, not retroactively — `quadraui-node` has history
predating it (see `git log -- quadraui-node`); none of it is backfilled
here. `[Unreleased]` is where changelog discipline begins for this package.

Unlike the `quadraui` Rust crate's own `CHANGELOG.md`, entries here are
written directly under `[Unreleased]` — there is no `changelog.d/`
fragment step for this package (that mechanism, and
`tools/changelog_check.py`'s enforcement of it, only covers
`quadraui/src/**`).

## [Unreleased]

### Added

- `README.md` now documents install (npm vs. from source), build and run
  steps, the CommonJS-vs-ESM `.ts`/`require` constraint the examples and
  tests rely on, and this `CHANGELOG.md` itself.
