After making any code changes, ALWAYS run these quality checks before committing:

1. Run `cargo fmt` to format the code
2. Run `cargo clippy -- -D warnings` to check for linting issues
3. Run `cargo test` to ensure all tests pass
4. Run `cargo build` to verify the code compiles

If any of these checks fail:
- Fix the issues immediately
- Re-run the failed check to verify the fix
- Only proceed to commit once ALL checks pass

This ensures that CI will pass and code quality is maintained.
