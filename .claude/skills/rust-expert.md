---
name: rust-expert
description: Advanced Rust patterns, focusing on ownership, safety, and performance. Apply to all .rs files and Cargo.toml changes.
---

# Rust Expert Skill

## 1. Ownership & Lifetimes

- **Borrow Checker First:** Always prefer borrowing (`&T` or `&mut T`) over cloning (`.clone()`) unless the data must be owned.
- **Lifetime Elision:** Do not manually specify lifetimes (e.g., `<'a>`) unless the compiler cannot infer them.
- **Smart Pointers:** Use `Rc<T>` for multiple readers, `Arc<T>` for thread-safe sharing, and `Box<T>` for heap allocation of large structs.
- **`Cow<str>`:** When an API sometimes needs an owned `String` and sometimes a `&str`, use `Cow<'_, str>` to avoid unnecessary allocations. This is especially relevant at FFI/GTK boundaries.

## 2. Error Handling

- **Production Code:** Avoid `unwrap()` and `expect()` in production code paths. Use `?` for propagation.
- **Tests & Build Scripts:** `unwrap()` and `expect()` are acceptable in `#[test]` functions, `build.rs`, and examples where a panic is the correct failure mode.
- **After Validation:** `unwrap()` is acceptable immediately after an explicit check (e.g., `if option.is_some() { option.unwrap() }`), but prefer `if let` or `match` instead — it's safer and more idiomatic.
- **Result Types:** Prefer the `anyhow` crate for application logic and `thiserror` for library-grade error enums.
- **Context:** Always use `.context("...")` or `.with_context(|| format!(...))` with anyhow to provide a stack-trace-like experience.

## 3. Style & Idioms

- **Pattern Matching:** Use `match` or `if let` instead of nested `if` statements for `Option` and `Result`.
- **Clippy:** Assume `cargo clippy` is active. Write code that passes default linting rules.
- **Functional Style:** Use iterator chains (`.map()`, `.filter()`, `.collect()`) where it improves readability over `for` loops. Don't force chains when a `for` loop with early returns is clearer.
- **Type Inference:** Let the compiler infer types. Don't annotate variables unless it aids readability or resolves ambiguity (e.g., `.collect::<Vec<_>>()`).
- **`impl` Over Generics in Args:** Prefer `fn foo(s: impl AsRef<str>)` over `fn foo<S: AsRef<str>>(s: S)` for single-use bounds to reduce visual noise.

## 4. Module Organization

- **Visibility:** Default to private. Use `pub(crate)` for internal sharing, `pub` only for the public API.
- **Thin Entry Points:** Keep `main.rs` and `lib.rs` thin — they should primarily re-export and wire things together.
- **Grouping:** One module per logical concern. If a file exceeds ~300 lines, consider splitting into a `module/mod.rs` + sub-files structure.
- **Re-exports:** Use `pub use` in `lib.rs` or `mod.rs` to flatten the public API so consumers don't need deep paths.

## 5. Modern Tooling

- **Async:** Use `tokio` as the default runtime. Use `#[tokio::main]`.
- **Serialization:** Use `serde` with `#[derive(Serialize, Deserialize)]`.
- **Feature Flags:** Gate optional dependencies behind Cargo features. Don't compile what you don't use.
- **Workspace:** For multi-crate projects, use a Cargo workspace to share dependencies and build settings.

## 6. Performance Defaults

- **Allocations:** Be allocation-aware. Prefer `&[T]` over `Vec<T>` in function signatures when ownership isn't needed. Use `String` only when you must own it.
- **`#[inline]`:** Don't add `#[inline]` unless profiling shows it matters. Let the compiler decide.
- **Release Profile:** Ensure `Cargo.toml` has `[profile.release] lto = true` for final builds when binary size/speed matters.
