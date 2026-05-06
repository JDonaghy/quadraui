Create a release from the develop branch. Follow these steps exactly:

## Pre-flight checks
1. Ensure you are on the `develop` branch
2. Run `cargo fmt && cargo clippy -- -D warnings && cargo build`
3. Run `cargo test --no-fail-fast` and report the total passing test count
4. Confirm there are no uncommitted changes (`git status`)

## Version bump
1. Read current version from `Cargo.toml`
2. Ask the user whether this is a **minor** (new features) or **patch** (bug fixes only) release
3. Bump the version in `Cargo.toml` accordingly
4. Commit with message: `chore: bump version to X.Y.Z for release`
5. Push to `origin develop`

## Flatpak sources
1. Check if `Cargo.lock` changed vs `main`: `git diff main -- Cargo.lock | head -5`
2. If changed, warn the user: "Cargo.lock changed since last release. The Flatpak CI build may fail if `flatpak/cargo-sources.json` is stale. You may need to regenerate it with `python3 flatpak-cargo-generator.py Cargo.lock -o flatpak/cargo-sources.json` from the flatpak-builder-tools repo."

## Create PR
1. Run `git log main..develop --oneline` to see all commits going into the release
2. Create a PR from `develop` to `main` using `gh pr create` with:
   - Title: `Release vX.Y.Z`
   - Body: Summary of changes (grouped by category), test plan with cargo check results
3. Print the PR URL

## After PR
Tell the user:
- Merging the PR to `main` triggers `release.yml` which creates a GitHub Release tagged `vX.Y.Z`
- Never push directly to `main` — always merge from `develop` via PR
- Monitor CI on the PR for any failures before merging
