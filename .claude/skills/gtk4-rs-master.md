---
name: gtk4-rs-master
description: Patterns for GTK4 and Adwaita in Rust — UI state, signals, async, and the GTK4 list model.
---

# GTK4 Rust Specialist

## 1. Signal Handling & Closures

- **Use `glib::clone!`**: Always use the `clone!` macro when passing widgets or state into signal handlers (e.g., `button.connect_clicked(clone!(@weak label => move |_| ...))`).
- **Weak References**: Always prefer `@weak` references for widgets in closures to avoid reference cycles and memory leaks. Use `@strong` only when the closure must keep the object alive (rare).
- **`connect_closure!`**: For custom signals on subclassed GObjects, use `connect_closure!` when `connect_*` methods are not available. This is common when defining your own signals via `ObjectImpl`.
- **Signal Cleanup**: For long-lived connections that outlast a view, store `SignalHandlerId` and disconnect explicitly on dispose/teardown.

## 2. State Management

- **Interior Mutability**: Use `Rc<RefCell<T>>` for shared application state that needs to be modified from UI signals.
- **Properties**: For complex widgets, prefer `glib::Properties` and `glib::Object` subclassing over raw structs where appropriate.
- **Property Bindings**: When two widgets need to stay in sync, use `bind_property("source-prop", &target, "target-prop").sync_create().build()` instead of manual signal handlers. This is cleaner and handles lifecycle automatically.

## 3. UI Construction

- **GTK4 Defaults**: Use `gtk::Application` and `gtk::ApplicationWindow`. Do not use `gtk::main()` (GTK3 style).
- **Adwaita**: If the project uses `libadwaita`, prefer `adw::Application` and `adw::ApplicationWindow` for a modern GNOME look. Use `adw::NavigationView`, `adw::ToolbarView`, etc. for navigation patterns.
- **Composition**: Prefer `.ui` files (XML) with `gtk::Builder` or composite templates (`#[derive(CompositeTemplate)]`) for complex layouts. Use programmatic construction only for simple or dynamic UIs.

## 4. Layout & Widgets

- **No `add()`**: `gtk::Container` is gone in GTK4. Use `.set_child()` for single-child containers, `box_.append()` for `gtk::Box`, `grid.attach()` for `gtk::Grid`, etc.
- **String Handling**: Use `.to_string()` or `.as_str()` explicitly when passing Rust strings to GTK methods. GTK methods often expect `&str` or `Option<&str>`.
- **Sizing**: Prefer `set_hexpand(true)` / `set_vexpand(true)` and `set_halign()` / `set_valign()` over fixed sizes. Let the layout engine do its job.

## 5. List Model (GTK4 Pattern)

This is the biggest GTK3 → GTK4 change. Do NOT use `TreeView`/`ListStore` from GTK3.

- **Model**: Use `gio::ListStore` to hold your data objects (which must be `glib::Object` subclasses).
- **Selection**: Wrap in `gtk::SingleSelection` or `gtk::MultiSelection`.
- **View**: Use `gtk::ListView` (or `gtk::GridView`, `gtk::ColumnView`).
- **Factory**: Use `gtk::SignalListItemFactory` and connect `setup` + `bind` signals to create and populate row widgets.
- **Pattern**:
  ```rust
  let factory = gtk::SignalListItemFactory::new();
  factory.connect_setup(|_, list_item| { /* create widgets */ });
  factory.connect_bind(|_, list_item| { /* bind data to widgets */ });
  let selection = gtk::SingleSelection::new(Some(model));
  let list_view = gtk::ListView::new(Some(selection), Some(factory));
  ```

## 6. Async in GTK

GTK is single-threaded. You cannot touch widgets from a background thread.

- **`glib::spawn_future_local()`**: Use this to run async code on the GLib main loop. This is the correct way to do async work that needs to update the UI.
- **Do NOT use `tokio::spawn`** for anything that touches widgets. Tokio tasks run on a thread pool and will panic or cause UB.
- **Background Work**: If you need real async I/O (network, disk), spawn it on tokio, then send the result back via a `glib::MainContext` channel or `spawn_future_local`.
- **Pattern**:
  ```rust
  glib::spawn_future_local(clone!(@weak label => async move {
      let result = gio::spawn_blocking(|| expensive_computation()).await.unwrap();
      label.set_text(&result);
  }));
  ```

## 7. Resources & Actions

- **GResource**: Compile UI files, icons, and CSS into a `.gresource` bundle via `glib_build_tools::compile_resources()` in `build.rs`.
- **CSS**: Load stylesheets via `gtk::CssProvider` and `gtk::style_context_add_provider_for_display()`.
- **Actions**: Use `gio::SimpleAction` for menu items and keyboard shortcuts. Attach to the `ApplicationWindow` or `Application` as appropriate.
