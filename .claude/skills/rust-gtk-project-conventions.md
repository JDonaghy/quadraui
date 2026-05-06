---
name: rust-gtk-project-conventions
description: Project structure, build conventions, and workflow rules for a Rust + GTK4/Adwaita application. Apply alongside rust-expert and gtk4-rs-master.
---

# Rust GTK Project Conventions

## 1. Project Structure

Follow this layout for a typical GTK4 Rust application:

```
project-root/
├── Cargo.toml
├── build.rs                  # GResource compilation
├── data/
│   ├── resources.gresource.xml
│   ├── icons/
│   ├── style.css
│   └── ui/                   # .ui template files
│       ├── window.ui
│       └── preferences.ui
├── src/
│   ├── main.rs               # Entry point — thin, just boots the app
│   ├── application.rs         # Application subclass, activate/startup
│   ├── config.rs              # Build-time constants (app ID, version)
│   ├── window/
│   │   ├── mod.rs             # Window subclass + CompositeTemplate
│   │   └── imp.rs             # ObjectImpl, WidgetImpl, etc.
│   ├── widgets/               # Custom reusable widgets
│   └── models/                # GObject model classes for ListStore
└── CLAUDE.md                  # This project's rules for Claude Code
```

- Keep `main.rs` under 30 lines. It should only create and run the `Application`.
- Each custom widget or GObject subclass gets its own directory with `mod.rs` + `imp.rs`.
- All `.ui` files live in `data/ui/`. Don't scatter them in `src/`.

## 2. Cargo.toml Conventions

- **Edition**: Always use `edition = "2021"` (or later if available).
- **Dependencies**: Pin GTK4/Adwaita crate versions to a specific minor version (e.g., `gtk = { version = "0.9", package = "gtk4" }`). GTK crate versions map to specific GTK C library versions — mixing them breaks builds.
- **Features**: Use feature flags for optional capabilities (e.g., `[features] libadwaita = ["dep:libadwaita"]`).
- **Release Profile**:
  ```toml
  [profile.release]
  lto = true
  strip = true
  codegen-units = 1
  ```

## 3. Build Process

- **Check before committing**: Always run `cargo clippy -- -W clippy::all` and `cargo fmt --check` before treating code as done.
- **Test**: Run `cargo test` to ensure nothing is broken. UI code is hard to unit test — focus tests on model/logic code.
- **GResource**: The `build.rs` should call `glib_build_tools::compile_resources()`. If the build fails with missing resources, check that `resources.gresource.xml` lists all files.

## 4. Naming Conventions

- **Application ID**: Use reverse-DNS (e.g., `com.github.username.appname`). Must match what's in `.desktop` and `resources.gresource.xml`.
- **Signal Names**: Use kebab-case (e.g., `"item-selected"`).
- **Property Names**: Use kebab-case (e.g., `"is-active"`). The `glib::Properties` macro converts `snake_case` Rust fields automatically.
- **CSS Classes**: Use kebab-case. Prefer Adwaita's built-in style classes (`.title-1`, `.card`, `.navigation-sidebar`) over custom CSS when possible.

## 5. Git Conventions

- **Commits**: Use conventional commits — `feat:`, `fix:`, `refactor:`, `chore:`, `docs:`.
- **Don't commit**: `target/`, `.flatpak-builder/`, `*.gresource` (compiled), `.env` files.
- **Do commit**: `.ui` files, `Cargo.lock` (it's an application, not a library), `CLAUDE.md`.

## 6. Common Pitfalls to Avoid

- **Don't use `gtk::main()`** — that's GTK3. Use `application.run()`.
- **Don't use `TreeView`** — use the new `ListView` + `ListStore` + `SignalListItemFactory` model.
- **Don't call widget methods from threads** — use `glib::spawn_future_local()` or `MainContext` channels.
- **Don't ignore deprecation warnings** — GTK4 moves fast and deprecated APIs get removed in the next major version.
- **Don't hard-code strings** — if the app will ever be translated, use `gettext` or `gettextrs` from the start.
