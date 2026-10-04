# The compose controllers

An independent framework audit of `develop @ ed402b4` (quadraui#1128) rated
the `quadraui::compose` controllers **the best part of the developer
experience and the least documented** — the root README named three of
fourteen-plus. This is the fix: one section per module in
`src/compose/`, in the order `src/compose/mod.rs` declares them.

## Why these exist

`src/primitives/` gives you stateless descriptors — paint `TreeView`,
paint `MenuBar`, and you get back nothing but pixels. Routing a raw
`UiEvent::MouseDown { position }` into "the user clicked row 3 of
section 2" is the same arithmetic in every app that uses a tree-shaped
sidebar. A **compose controller** owns that interaction state machine
once — selection, scroll, hover, drag, keyboard navigation — so your
`AppLogic::handle` matches on a semantic event (`SidebarEvent::RowSelected`,
`MenuEvent::Activated`) instead of reimplementing hit-testing.

The general shape, shared by nearly every controller below:

```ignore
struct MyApp {
    controller: SomeController,   // one field, app-owned
}

impl AppLogic for MyApp {
    fn render(&mut self, backend: &mut dyn Backend, area: Rect) {
        self.controller.set_rows(/* fresh data, every frame */);
        self.controller.render(backend, area);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match self.controller.handle(&event, backend) {
            SomeEvent::RowActivated { .. } => { /* react */ Reaction::Redraw }
            SomeEvent::Consumed => Reaction::Redraw,
            SomeEvent::Ignored => Reaction::None,
            _ => Reaction::None,
        }
    }
}
```

Push fresh data in `render` (controllers are cheap to re-feed per
frame, not built once), call `handle` from your event handler, match on
its event enum. Every controller below names the exact shape it
deviates from this with.

---

## `AppShell` — `src/compose/app_shell.rs`

**Owns:** the VS Code / JetBrains-style application frame — activity
bar click-to-toggle, active panel switching, sidebar show/hide, sidebar
resize drag, and (optionally) a `BottomPanel`. Register panels at
construction or via `AppShell::add_panel` / `remove_panel`; the shell
paints its own chrome (activity bar, sidebar header, resize divider)
and hands your app `AppShellLayout` with the bounds to paint panel
*content* and the main content area into.

**Events (`AppShellEvent`):** `PanelChanged { panel_id }`,
`SidebarHidden`, `SidebarResized { new_width }`,
`BottomPanelResized { new_height }`, `BottomPanelHidden`,
`BottomItemClicked { id }`, `Consumed`, `Ignored`.

**Smallest wiring:** most apps don't construct `AppShell` directly —
`ShellApp` + `quadraui::tui::run_with_shell` / the GTK equivalent build
and drive one for you from a `ShellConfig`. Implement `ShellApp`, fill
in each panel's `render`/`handle`, and the shell above owns the frame.
See `docs/GUIDE.md`'s Step 0 table for when to reach for `ShellApp`
over bare `AppLogic`.

**Example:** `examples/tui_full_chrome_demo.rs` / `examples/gtk_full_chrome_demo.rs`
(`examples/common/full_chrome_demo.rs`) — activity bar, sidebar,
bottom panel, title bar all wired together.

---

## `BottomPanelController` — `src/compose/bottom_panel.rs`

**Owns:** a tabbed, dockable panel along the bottom of an `AppShell`
(VS Code's terminal panel, modelled directly). Tab switching, closing,
maximise toggle, and the resize drag that changes its height. Attach a
`BottomPanelConfig` to `ShellConfig::bottom_panel` — `AppShell` drives
the controller for you; you supply `BackendWidget` content per tab.

**Events (`BottomPanelEvent`):** `TabActivated(String)`,
`TabClosed(String)`, `MaximiseToggled`, `Resized(f32)`.

**Smallest wiring:**

```ignore
let config = ShellConfig::new("App", panels).with_bottom_panel_config(
    BottomPanelConfig { tabs: vec![BottomPanelTab { id: "terminal".into(), label: "Terminal".into(), content: Box::new(MyTerminal) }], ..Default::default() },
);
```

**Example:** `examples/tui_bottom_panel.rs` / `examples/gtk_bottom_panel.rs`
(`examples/common/bottom_panel_demo.rs`).

---

## `ChatController` — `src/compose/chat_controller.rs`

**Owns:** a chat-overlay interaction state machine — scrollable
transcript, multi-line input with history navigation, status strip.
Push turns per frame with `set_transcript`, call `render` + `handle`.

**Events (`ChatControllerEvent`):** `Submit { text }`,
`StreamChunk { text }`, `Cancelled`, `StopRequested`,
`KeyPressed { key, modifiers }`, `TurnClicked { turn_idx, row_in_turn }`,
`Consumed`, `Ignored`.

**Smallest wiring:**

```ignore
let mut chat = ChatController::new();
chat.set_transcript(&turns);
chat.render(backend, area);
// in handle():
if let ChatControllerEvent::Submit { text } = chat.handle(&event, backend) {
    send_to_model(text);
}
```

**Example:** `examples/tui_chat.rs` / `examples/gtk_chat.rs`
(`examples/common/chat_demo.rs`).

---

## `ContextMenuController` — `src/compose/context_menu_controller.rs`

**Owns:** the "one call" right-click menu entry point (#1187) — builds
the menu, shows it natively where the backend supports a native
context menu, degrades to the in-canvas `ContextMenu` primitive
elsewhere, and routes the `ModalStack` either way. One code path
regardless of platform or `MenuStyle`.

**Events (`ContextMenuOutcome`):** `Event(UiEvent)` (an already-resolved
`UiEvent::ContextMenuItemActivated` / `ContextMenuDismissed` — route it
through your existing menu-event handler), `Consumed`, `Ignored`.

**Smallest wiring:**

```ignore
fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
    if let ContextMenuOutcome::Event(ev) = self.ctx_menu.handle(&event, backend) {
        return self.handle_menu_event(ev);
    }
    match event {
        UiEvent::MouseDown { button: MouseButton::Right, position, .. } => {
            self.ctx_menu.show(self.build_context_menu(), position, backend);
            Reaction::Redraw
        }
        _ => Reaction::None,
    }
}
```

**Example:** `examples/tui_context_menu_style.rs` / `examples/gtk_context_menu_style.rs`
/ `examples/macos_context_menu_style.rs` (`examples/common/context_menu_style_demo.rs`).

---

## `DualModePaletteController` — `src/compose/dual_mode_palette.rs`

**Owns:** a command palette that toggles between **list mode**
(type-to-filter, Enter selects a row) and **input mode** (free-text
confirm, e.g. "new branch name"). `Tab` (or `toggle_mode`) switches
modes without clearing the query.

**Events (`DualModePaletteEvent`):** `ItemConfirmed { idx }`,
`TextConfirmed { value }`, `QueryChanged { value }`,
`ModeToggled { new_mode }`, `Cancelled`, `Consumed`, `Ignored`.

**Smallest wiring:**

```ignore
let mut pal = DualModePaletteController::new("Branches", "New branch name:", branches);
pal.render(popup_rect, backend);
match pal.handle(&event, backend) {
    DualModePaletteEvent::ItemConfirmed { idx } => checkout(idx),
    DualModePaletteEvent::TextConfirmed { value } => create_branch(value),
    _ => {}
}
```

**Example:** `examples/tui_palette_dual_mode.rs` (`examples/common/help_layer_demo.rs`
also drives one via `help_actions_to_palette_items`).

---

## `FilePickerController` — `src/compose/file_picker.rs`

**Owns:** an in-app file open/save picker for backends without a
native dialog — fuzzy filtering, directory walking, scroll, selection,
and (in save mode) the typed query doubling as the destination
filename. Renders through the existing `Palette` primitive.

**Events (`FilePickerEvent`):** `Confirmed { path }`, `Cancelled`,
`Consumed`, `Ignored`.

**Smallest wiring:** identical shape to `FolderPickerController` below,
swap the type and mode (`FilePickerMode::Open` / `::Save`).

**Example:** `examples/tui_file_picker.rs` / `examples/gtk_file_picker.rs`
(`examples/common/file_picker_app.rs`). Also driven internally by
`crate::tui::services`'s `TuiPlatformServices::show_file_open_dialog` /
`show_file_save_dialog` degrade path.

---

## `FocusGroup` — `src/compose/focus_group.rs`

**Owns:** Tab/Shift+Tab cycling between N focusable regions **by
index**. Starts unfocused (`None`), supports a dynamic `count` (regions
added/removed at runtime with no stable id to key by) — the shape
`SidebarSystem` and `TabGroupController` need internally for their own
section/pane cycling.

**Events:** none — a plain state holder. Call `advance()` /
`retreat()` / `focused()` from your own handler.

**Smallest wiring:**

```ignore
let mut group = FocusGroup::new(3); // 3 regions
group.advance();       // Tab
assert_eq!(group.focused(), Some(0));
```

**Example:** no standalone demo — exercised internally by
`SidebarSystem` (`examples/msv_multi_tree.rs`) and `TabGroupController`
(`examples/tui_tabgroup.rs`). Prefer `FocusRing` below for app-level
Tab cycling between named widgets.

---

## `FocusRing` — `src/compose/focus_ring.rs`

**Owns:** Tab/Shift+Tab cycling through a list of `WidgetId`s — a
thin, id-keyed wrapper over `FocusGroup`'s index cycling (#509).
Register focusable widget ids once, call `advance` / `retreat` / `set`
from your event handler, read `current()` at render time to know what
to highlight.

**Events:** none — a plain state holder, same contract as `FocusGroup`.

**Smallest wiring:**

```ignore
let mut ring = FocusRing::new(vec!["search", "toggles", "replace", "buttons"]);
ring.advance();                              // Tab
ring.set(&WidgetId::new("replace"));         // click-to-focus
assert_eq!(ring.current(), Some(&WidgetId::new("replace")));
```

**Example:** `examples/tui_focus_ring.rs` / `examples/gtk_focus_ring.rs`
(`examples/common/form_groups.rs`'s `FocusDemo`).

---

## `FolderPickerController` — `src/compose/folder_picker.rs`

**Owns:** an interactive directory-browsing modal — filesystem
walking, fuzzy filtering, scroll, selection. Renders through the
`Palette` primitive, extracted verbatim from vimcode's TUI-local
`FolderPickerState`.

**Events (`FolderPickerEvent`):** `Confirmed { path }`, `Cancelled`,
`Consumed`, `Ignored`.

**Smallest wiring:**

```ignore
let mut picker = FolderPickerController::new(start_dir);
picker.render(popup_rect, backend);
if let FolderPickerEvent::Confirmed { path } = picker.handle(&event, backend) {
    open_workspace(path);
}
```

**Example:** `examples/tui_folder_picker.rs` / `examples/gtk_folder_picker.rs`
(`examples/common/folder_picker_app.rs`). Also driven internally by
`crate::tui::services`'s folder-dialog degrade path.

---

## `FormController` — `src/compose/form_controller.rs`

**Owns:** a single `Form`'s focus, editing, validation, and built-in
scrollbar — the `Form`-primitive mirror of `TreeController`. Push
field data per frame via `set_form`, call `render` + `handle`.

**Events (`FormControllerEvent`):** `FormAction(FormEvent)` (field-level
change/submit from the underlying `Form`), `ScrollChanged`, `Consumed`,
`Ignored`. Use `handle_cached` instead of `handle` when your event
handler runs without a `&mut dyn Backend` reference.

**Smallest wiring:**

```ignore
let mut form = FormController::new();
form.set_form(my_form_descriptor);
form.render(backend, area);
if let FormControllerEvent::FormAction(ev) = form.handle(&event, backend) {
    apply_field_change(ev);
}
```

**Example:** `examples/tui_form_scroll.rs` / `examples/gtk_form_scroll.rs`
(`examples/common/form_scroll.rs`).

---

## `HelpRegistry` / `HelpOverlayController` — `src/compose/help_layer.rs`

**Owns:** a context-sensitive `?`-triggered help overlay (#431), split
in two: `HelpRegistry` is where views register a `ViewHelp` (reference
`HelpNote`s + `HelpAction`s) per view/panel id; `HelpOverlayController`
renders the active view's registered help as a `Panel` + `TextDisplay`
cheatsheet. `help_actions_to_palette_items` / `filter_help_actions`
feed the same registered actions into `DualModePaletteController` so
they're searchable by label *and* description.

**Events (`HelpOverlayEvent`):** `Opened`, `Closed`, `Consumed`,
`Ignored`.

**Smallest wiring:**

```ignore
let mut registry = HelpRegistry::new();
registry.register("editor", ViewHelp { notes: vec![/* .. */], actions: vec![/* .. */] });
let mut overlay = HelpOverlayController::new();
// on `?`:
overlay.open(registry.get("editor"));
overlay.render(backend, area);
```

**Example:** `examples/tui_help_layer.rs` / `examples/gtk_help_layer.rs`
(`examples/common/help_layer_demo.rs`).

---

## Markdown → `StyledText` adapter — `src/compose/markdown.rs`

Not a stateful controller — a pure conversion function,
`render_markdown_to_styled`, from a CommonMark subset to the
`StyledText` lines every backend already knows how to paint. Headings,
bold/italic (with CommonMark flanking rules so `snake_case` and
`a * b * c` render upright), inline code, lists, fenced code blocks,
links, and blockquotes. See the module doc's table for the full
supported-syntax list.

**Events:** none — call it once per render with fresh markdown text.

**Smallest wiring:**

```ignore
let styled = render_markdown_to_styled(markdown_source, &theme);
backend.draw_rich_text_popup(area, &styled);
```

**Example:** `examples/tui_markdown.rs` / `examples/gtk_markdown.rs`.

---

## `MenuSystem` — `src/compose/menu_system.rs`

**Owns:** `MenuBar` + `ContextMenu` dropdown interaction — open/close,
Alt-key activation, arrow-key navigation, hover-to-switch between open
menus, modal stack coordination. Define your menu structure once,
match on `MenuEvent::Activated`.

**Events (`MenuEvent`):** `Activated(WidgetId)`, `StateChanged`,
`Consumed`, `Ignored`.

**Smallest wiring:**

```ignore
match self.menu_system.handle(&event, backend, bar_rect) {
    MenuEvent::Activated(id) if id.as_str() == "save" => save(),
    MenuEvent::StateChanged | MenuEvent::Consumed => return Reaction::Redraw,
    _ => {}
}
```

**Example:** `examples/tui_menu_bar.rs` / `examples/gtk_menu_bar.rs` /
`examples/macos_menu_bar.rs` / `examples/win_menu_bar.rs`
(`examples/common/menu_bar_app.rs`).

---

## `MessageDialogController` — `src/compose/message_dialog.rs`

**Owns:** the message/alert-box driver for the in-canvas `Dialog`
primitive — keyboard button focus and resolve-on-click — giving
backends without a native alert facility the same
show → block-until-resolved → return-a-choice contract
`PlatformServices::show_message_dialog` has everywhere else.

**Events (`MessageDialogEvent`):** `Resolved(MessageDialogChoice)`,
`Consumed`, `Ignored`.

**Smallest wiring:**

```ignore
let mut dialog = MessageDialogController::new(options);
dialog.render(backend, area);
if let MessageDialogEvent::Resolved(choice) = dialog.handle(&event, backend) {
    apply_choice(choice);
}
```

**Example:** `examples/tui_message_dialog.rs` / `examples/tui_message_dialog_app.rs`
/ `examples/gtk_message_dialog.rs` / `examples/gtk_message_dialog_app.rs`
(`examples/common/message_dialog_app.rs`). Also driven internally by
`crate::tui::services`'s message-dialog degrade path.

---

## `notify_or_toast` — `src/compose/notification.rs`

Not a stateful controller — a single branch point (#955). Sends a
native `Notification` where `BackendCaps::notifications` is `true`
(GTK today), degrades to returning a `Toast` for you to push onto your
own toast stack where it isn't (TUI, which has no OS notification
concept). One call instead of repeating the capability check at every
notify site.

**Events:** none — returns `Option<Toast>` directly.

**Smallest wiring:**

```ignore
if let Some(toast) = notify_or_toast(backend, Notification::new("Build", "Succeeded")) {
    self.toasts.push(toast); // backend had no native notifications; show it yourself
}
```

**Example:** no dedicated example yet — covered by `notification.rs`'s
own unit tests. `examples/gtk_platform_services.rs` exercises the
native-notification side of the same capability check.

---

## `SidebarPanelBody` — `src/compose/sidebar_panel_body.rs`

**Owns:** the four layers every sidebar-panel renderer hand-rolled
before issue #1041 — background fill, optional header/search chrome,
the panel's own body widget, optional scrollbar-gutter reservation —
composed from existing `Backend` methods (`draw_solid_fill`,
`draw_settings_chrome`, your content rasteriser) into one ordered paint
call. Stateless per-frame value, like `Panel` or `Scrollbar`.

**Events:** none — a paint composer, not an interaction state machine.
Click routing stays with whatever owns the body widget (e.g.
`TreeController`).

**Smallest wiring:**

```ignore
SidebarPanelBody::new(body_widget)
    .with_header("Explorer")
    .with_search(search_state)
    .render(backend, panel_rect);
```

**Example:** `examples/tui_sidebar_panel_body.rs` / `examples/gtk_sidebar_panel_body.rs`
(`examples/common/sidebar_panel_body_demo.rs`).

---

## `SidebarSystem` — `src/compose/sidebar_system.rs`

**Owns:** a multi-section MSV (Multi-Section View) sidebar — per-section
scroll/selection, active-section cycling (via `FocusGroup`), scrollbar
drag, keyboard navigation, and two-layer click dispatch (MSV header →
body with coordinate translation). Sections are Tree-bodied (delegates
to an internal `TreeController`) or Form-bodied (delegates to an
internal `FormController`), chosen per-section via `SectionKind`.

**Events (`SidebarEvent`):** `HeaderActivated { section }`,
`RowSelected { section, path }`, `RowActivated { section, path }`,
`RowToggleExpand { section, path }`, `ContextMenuRequested { .. }`,
`ScrollChanged { section }`, `EditConfirmed { .. }`,
`EditCancelled { section, path }`, `EditChanged { .. }`,
`FormEvent { section, event }`, `StateChanged`.

**Smallest wiring:**

```ignore
let mut sidebar = SidebarSystem::new(vec![
    SidebarSectionDef::tree("files", "Files"),
]);
sidebar.set_rows(0, file_tree_rows);
sidebar.render(backend, sidebar_rect);
if let SidebarEvent::RowActivated { path, .. } = sidebar.handle(&event, backend) {
    open_file(path);
}
```

**Example:** `examples/msv_multi_tree.rs` / `examples/gtk_multi_tree.rs`
(`examples/common/multi_tree.rs`); search + toggle sections:
`examples/tui_sidebar_search.rs` / `examples/gtk_sidebar_search.rs`
(`examples/common/sidebar_search.rs`).

---

## `StatusBarInteraction` — `src/compose/status_bar_interaction.rs`

**Owns:** hover/press state for `StatusBar` segments. Feed it every
mouse event plus the bar's rect; read `hovered_id()` / `pressed_id()`
at render time and pass them to `Backend::draw_status_bar`. Eliminates
per-backend status-bar hover routing.

**Events (`StatusBarAction`):** `Clicked(WidgetId)`, `Redraw`,
`Ignored`.

**Smallest wiring:**

```ignore
let action = self.status_interaction.handle(&event, bar_rect);
backend.draw_status_bar(bar_rect, &segments, self.status_interaction.hovered_id());
if let StatusBarAction::Clicked(id) = action { run_segment_command(id); }
```

**Example:** `examples/tui_full_chrome_demo.rs` / `examples/gtk_full_chrome_demo.rs`
(`examples/common/full_chrome_demo.rs`).

---

## `TabGroupController` — `src/compose/tab_group.rs`

**Owns:** an editor-group-style tabbed split-pane layout — wires
`TabBar` + `Split` + `DropZone` + `FocusGroup` into N panes arranged in
an arbitrary nested H/V split tree, each with its own scrollable tab
bar. Tab activation, closing, new-tab, pane focus, and divider drag all
go through one controller.

**Events (`TabGroupEvent`):** `TabActivated { pane_idx, tab_id }`,
`TabClosed { pane_idx, tab_id }`, `PaneCollapsed { .. }`,
`PaneAdded { pane_idx }`, `PaneFocused { pane_idx }`,
`DividerResized { divider_idx }`, `NewTabRequested { pane_idx }`,
`TabReordered { .. }`.

**Smallest wiring:**

```ignore
let tabs = vec![PaneTab { id: "t0".into(), label: "main.rs".into(), closable: true, content: Box::new(MyContent) }];
let mut group = TabGroupController::new(tabs);
let layout = group.render(backend, area);
if let TabGroupEvent::TabActivated { tab_id, .. } = group.handle(&event, backend) {
    activate(tab_id);
}
```

**Example:** `examples/tui_tabgroup.rs` / `examples/gtk_tabgroup.rs`
(`examples/common/tab_group_demo.rs`); active-tab bracket chrome:
`examples/tui_tab_chrome.rs` / `examples/gtk_tab_chrome.rs`
(`examples/common/tab_chrome_demo.rs`).

---

## `ToastStackController` — `src/compose/toast_stack.rs`

**Owns:** a pure keyboard-focus cursor over a `ToastOverlay` you build
yourself every frame (#1185). Toasts are non-modal — this controller
never blocks input while unfocused. Give it focus explicitly (e.g. a
"focus notifications" keybinding); then Tab/Shift+Tab/Left/Right cycle
a focused toast's buttons, Up/Down move between toasts, Enter
activates, Escape dismisses and returns focus to the app.

**Events (`ToastStackEvent`):** `Action(WidgetId)`, `Dismiss(WidgetId)`,
`FocusReturned`, `Consumed`, `Ignored`.

**Smallest wiring:**

```ignore
let mut controller = ToastStackController::new();
controller.give_focus(&toasts);               // "focus notifications" binding
if let ToastStackEvent::Action(button_id) = controller.handle(&event, &toasts) {
    run_toast_action(button_id);
}
```

**Example:** `examples/tui_toast_actions.rs` / `examples/gtk_toast_actions.rs`
(`examples/common/toast_actions_app.rs`).

---

## `ToolbarHoverTracker` — `src/compose/toolbar_hover_tracker.rs`

**Owns:** converting `MouseMoved` events into `hovered_id:
Option<WidgetId>` for the `Toolbar` primitive (stateless rasteriser,
stateful host). Feed it mouse-move events plus the current
`ToolbarLayout`; read the hovered id at render time.

**Events:** none — a plain state holder, read via `hovered_id()`.

**Smallest wiring:**

```ignore
self.hover.handle_mouse_move(position, &toolbar_layout);
backend.draw_toolbar(toolbar_rect, &buttons, self.hover.hovered_id());
```

**Example:** `examples/tui_sidebar_panel.rs` / `examples/gtk_sidebar_panel.rs`
(`examples/common/sidebar_panel_app.rs`).

---

## `TreeController` — `src/compose/tree_controller.rs`

**Owns:** a single keyboard-navigable `TreeView` with scrollbar —
selection movement with scroll-to-follow, click hit-testing, scrollbar
thumb drag, scroll-wheel handling. `SidebarSystem` constructs one of
these internally per Tree-bodied section; use it directly for a
standalone tree with no MSV chrome around it.

**Events (`TreeControllerEvent`):** `RowSelected { path }`,
`RowActivated { path }`, `RowToggleExpand { path }`, `ScrollChanged`,
`EditConfirmed { path, new_text }`, `EditCancelled { path }`,
`Consumed`, `Ignored`.

**Smallest wiring:**

```ignore
let mut tree = TreeController::new();
tree.set_rows(rows);
tree.render(backend, area);
if let TreeControllerEvent::RowActivated { path } = tree.handle(&event, backend) {
    open(path);
}
```

**Example:** no standalone demo — every example drives `TreeController`
through `SidebarSystem`'s Tree-bodied sections (`examples/msv_multi_tree.rs`,
`examples/common/sidebar_panel_body_demo.rs`). Its own `#[cfg(test)]`
module in `tree_controller.rs` is the direct-use reference.

---

## `WorkspaceController` — `src/compose/workspace.rs`

**Owns:** an **open-N-view-one** set of opaque document ids with
exactly one active, rendered through the `TabBar` primitive (#596).
`open` / `close` / `activate` / `reorder`, click + keyboard routing.
Unlike `TabGroupController`, it owns no content itself — the host
paints a body that borrows app state, keyed by whichever id is active.

**Events (`WorkspaceEvent`):** `Opened { .. }`, `Activated { .. }`,
`Closed { .. }`, `Reordered { .. }`, `Promoted { .. }`.

**Smallest wiring:**

```ignore
let mut ws = WorkspaceController::new("panel:docs");
ws.open(WorkspaceDoc::new("doc:a", "alpha.rs"));
let layout = ws.render(backend, panel_rect);
// host paints the active document's body into layout.body_bounds
if let WorkspaceEvent::Activated { id } = ws.handle(&event, backend) {
    switch_to(id);
}
```

**Example:** `examples/tui_workspace.rs` / `examples/gtk_workspace.rs`
(`examples/common/workspace_demo.rs`).
