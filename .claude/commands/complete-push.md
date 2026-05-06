Update all markdown files, archive anything that we no longer need to read into SESSION_HISTORY.md, commit and push.

Before pushing, ALWAYS run `cargo clippy --no-default-features -- -D warnings` and `cargo clippy --features win-gui --no-default-features -- -D warnings` to ensure clippy passes on all feature configurations. Fix any warnings before committing. This prevents CI failures on the Linux build.
