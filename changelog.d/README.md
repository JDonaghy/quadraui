# changelog.d/ — changelog fragments

`CHANGELOG.md`'s `[Unreleased]` section used to be edited directly by every
PR (#1500), but every PR's entry landed in the same few `###` spots, so any
two PRs in flight at once all but guaranteed a merge conflict on that one
file (#2419). This directory fixes that: **each PR adds its own new file
here instead of editing `CHANGELOG.md`.** Concurrent PRs never touch the
same lines, so they never conflict.

## Adding a fragment

Create one file: `changelog.d/<issue>.<kind>.md`.

- `<issue>` is this PR's tracking issue number (digits only).
- `<kind>` is one of `added`, `changed`, `deprecated`, `removed`, `fixed`
  — matching the `### Added` / `### Changed` / `### Deprecated` /
  `### Removed` / `### Fixed` heading the entry would have gone under in
  `CHANGELOG.md` directly. (There is no `security` kind; Keep a Changelog's
  `### Security` heading hasn't been needed in this file yet.)
- The file's content is **exactly the bullet text** that would have
  appeared under that heading in `[Unreleased]` — the same Markdown, the
  same wrapping, the same issue-number reference. For example,
  `changelog.d/2419.changed.md`:

  ```markdown
  - Changelog entries now go in `changelog.d/<issue>.<kind>.md` fragments
    instead of directly under `CHANGELOG.md`'s `[Unreleased]` section
    (#2419): `tools/changelog_check.py` requires a new fragment per PR
    instead of a direct `[Unreleased]` edit, and `scripts/release.py
    prepare` assembles every fragment into `[Unreleased]` before retitling
    it, then deletes the fragments it just assembled.
  ```

A multi-line entry continues with a 2-space indent, exactly like a
hand-written `CHANGELOG.md` bullet does.

## What NOT to do

- **Don't edit `CHANGELOG.md`'s `[Unreleased]` section directly.**
  `tools/changelog_check.py` rejects that diff shape outright and points
  back at this file — every PR's entry landing on the same lines is the
  exact conflict fragments exist to avoid.
- **Don't add more than one fragment per PR** unless the PR genuinely
  carries more than one independent changelog-worthy change.
- **Don't reuse another PR's `<issue>` number** or overwrite another
  fragment file.

## What happens at release time

`scripts/release.py prepare X.Y.Z` assembles every fragment in this
directory into `CHANGELOG.md`'s `[Unreleased]` section — grouped under the
matching `### Kind` heading (creating it if `[Unreleased]` doesn't already
have one, in Keep-a-Changelog order: Added, Changed, Deprecated, Removed,
Fixed) and sorted by issue number within each heading, appended below
whatever bullets that heading already had. It then retitles `[Unreleased]`
to `[X.Y.Z] - YYYY-MM-DD`, adds a fresh empty `[Unreleased]` above it, and
deletes every fragment it just assembled — all in the same version-bump
commit. Nothing in this directory survives a release; a fresh set
accumulates for the next one.

See `CLAUDE.md`'s *CHANGELOG enforcement* section and
`quadraui/docs/PRIMITIVE_RULES.md` rule 8 for when a PR needs an entry at
all (every PR touching a `pub` item does; most behavior fixes do too).
