# The compose layer

`src/compose/` holds quadraui's composite widgets — an app shell, a docked
bottom panel, dialogs, pickers, a menu bar, a document strip — and the
controllers that drive them. This page has one section per public module, in
the order `src/compose/mod.rs` declares them.

## Widgets and controllers

`src/primitives/` gives you single widgets: a `TreeView`, a `TabBar`, a
`Dialog`. A compose module builds something bigger out of them and owns the
interaction state machine once — selection, scroll, hover, drag, keyboard
navigation, focus moving between parts — so your app matches on a semantic
event ("row 3 of section 2 was activated", "the user chose Save") instead of
re-deriving hit-testing.

Most modules come in two parts:

- **A widget** — a descriptor implementing the widget contract
  ([`Primitive`] + [`Interactive`]): `ShellView`, `BottomPanel`,
  `MessageDialog`, `FormDialog`, `DualModePalette`, `FilePicker`,
  `FolderPicker`, `HelpOverlay`, `AppMenu`, `Workspace`. You build it from
  your state every frame, paint it with `draw_dyn` (or a `Surface::Widget`
  entry), and list it in `AppLogic::frame_tree`. The runtime routes input to
  it, keeps its gesture state (`Interactive::State`) between frames, and
  hands you its result as a `UiEvent` carrying the widget's id. Visible
  state — which tab is active, whether the dropdown is open — stays a
  descriptor field you own (controlled mode).
- **A controller** — `<Name>Controller`, holding the widget and its state
  for a host that feeds it raw `UiEvent`s itself instead of registering it
  in a `FrameTree`. Its `render` paints the widget and its `handle_event`
  returns [`Handled<Action>`]: `Handled::Event(action)` for a semantic
  result, `Handled::Consumed` when only internal state changed (redraw),
  `Handled::Ignored` when the event was not its business. A controller's
  older `handle` returning a `*Event` enum is deprecated in favour of
  `handle_event`.

A few modules are controllers or helpers with no widget of their own yet;
their sections say so.

### Shape 1: a widget the runtime routes

The app owns the visible state, rebuilds the descriptor each frame, and
applies the actions the runtime hands back:

```rust
use std::cell::Cell;

use quadraui::{
    AppLogic, Backend, FrameTree, Reaction, Rect, ScreenLayout, Surface, UiEvent, WidgetId,
    Workspace, WorkspaceDoc, WorkspaceEvent,
};

struct Docs {
    docs: Vec<WorkspaceDoc>,
    active: Option<usize>,
    /// Where the last frame painted the strip, for `frame_tree`.
    rect: Cell<Rect>,
}

impl Docs {
    const ID: &str = "docs";

    fn widget(&self) -> Workspace {
        Workspace::new(Self::ID)
            .with_docs(self.docs.clone())
            .with_active(self.active)
    }
}

impl AppLogic for Docs {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let rect = Rect::new(0.0, 0.0, vp.width, vp.height);
        self.rect.set(rect);
        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect,
            widget: Box::new(self.widget()),
        });
        let _ = frame.draw(backend);
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(WidgetId::new(Self::ID), self.rect.get(), Box::new(self.widget()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::Workspace(_, WorkspaceEvent::Activated { index, .. }) => {
                self.active = Some(index);
                Reaction::Redraw
            }
            UiEvent::Workspace(_, WorkspaceEvent::Closed { index, .. }) => {
                self.docs.remove(index);
                self.active = (!self.docs.is_empty()).then(|| index.min(self.docs.len() - 1));
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }
}
```

`examples/tui_wide_tab_bar_demo.rs` (`examples/common/wide_tab_bar_demo.rs`) is a
runnable app in this shape.

### Shape 2: a controller you drive

The controller holds the widget and its state; you paint it and offer it
every event until it resolves:

```rust
use quadraui::{
    AppLogic, Backend, Handled, MessageDialogButton, MessageDialogController,
    MessageDialogOptions, Reaction, UiEvent, WidgetId,
};

struct Confirm {
    dialog: Option<MessageDialogController>,
    answer: Option<WidgetId>,
}

impl Confirm {
    fn open(&mut self) {
        self.dialog = Some(MessageDialogController::new(MessageDialogOptions {
            title: "Discard changes?".into(),
            body: "Unsaved edits will be lost.".into(),
            buttons: vec![
                MessageDialogButton {
                    id: WidgetId::new("cancel"),
                    label: "Cancel".into(),
                    is_default: false,
                    is_cancel: true,
                    spec: None,
                },
                MessageDialogButton {
                    id: WidgetId::new("discard"),
                    label: "Discard".into(),
                    is_default: true,
                    is_cancel: false,
                    spec: None,
                },
            ],
            severity: None,
        }));
    }
}

impl AppLogic for Confirm {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        if let Some(dialog) = &self.dialog {
            dialog.render(backend);
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        if let Some(dialog) = self.dialog.as_mut() {
            match dialog.handle_event(&event, backend) {
                Handled::Event(choice) => {
                    self.answer = Some(choice);
                    self.dialog = None;
                    return Reaction::Redraw;
                }
                Handled::Consumed => return Reaction::Redraw,
                Handled::Ignored => {}
            }
        }
        if let UiEvent::KeyPressed { .. } = event {
            self.open();
            return Reaction::Redraw;
        }
        Reaction::Continue
    }
}
```

The per-module snippets below are marked `rust,ignore` because they are
fragments: they assume `backend: &mut dyn Backend`, `event: UiEvent` and the
app's own fields in scope. Each section names an example file that is the
compiled, runnable version.

---

## `ShellView` / `AppShell` — `src/compose/app_shell.rs`

**Owns:** the VS Code / JetBrains-style application frame — activity bar,
active panel switching, sidebar show/hide and resize drag, and the optional
title bar, bottom panel, bottom bands, command line and status bar slots.

**Widget:** `ShellView` (`State` = `ShellViewState`, `Action` =
`AppShellEvent`, delivered as `UiEvent::AppShell(id, event)`). Everything
visible is a descriptor field the app owns: the `PanelDefinition`s, the
active panel, sidebar width and visibility, bottom-panel height and
visibility. `ShellViewState` holds only gesture data (an in-progress divider
drag, activity-bar hover). The shell paints its own chrome through its
`ActivityBar` and `StatusBar` children and reports the bounds for panel
content, the main area, the bottom panel and the status bar in
`ShellViewLayout`. Each chrome region is a named sub-region, so `draw_dyn`
publishes it as a frame-inventory zone (`app-shell:status-bar`, …).

**Actions (`AppShellEvent`):** `PanelChanged { panel_id }`,
`SidebarHidden`, `SidebarResized { new_width }`,
`BottomPanelResized { new_height }`, `BottomPanelHidden`,
`BottomItemClicked { id }`, `ActivityCursorMoved { .. }`.

**Smallest wiring:** most apps don't build the shell themselves. Implement
`ShellApp`, describe the frame with a `ShellConfig`, and call
`<backend>::shell_runner::run_with_shell(app, config)`; the runner owns the
shell and calls your `render_content` with the resolved layout.
`examples/hello.rs` is the smallest such app. See `docs/GUIDE.md`'s Step 0
table for when to reach for `ShellApp` over bare `AppLogic`.

<!-- TODO(0.12 final pass): after DC2 (#2958) deletes `AppShell`, check what
`ShellApp`/`run_with_shell` hand `render_content` (the `AppShellLayout` name), and
drop `AppShell` from this heading. -->

**Example:** `examples/hello.rs`; the full frame in
`examples/tui_full_chrome_demo.rs` / `examples/gtk_full_chrome_demo.rs`
(`examples/common/full_chrome_demo.rs`).

---

## `BottomPanel` — `src/compose/bottom_panel.rs`

**Owns:** a tabbed, dockable panel along the bottom of the shell, modelled on
VS Code's terminal panel: tab switching, closing, the maximise toggle, and
the resize drag that changes its height.

**Widget:** `BottomPanel` (`State` = `BottomPanelState`, `Action` =
`BottomPanelEvent`, delivered as `UiEvent::BottomPanel(id, event)`). Its tab
strip paints and routes through a `TabBar` child; the content area below it
is the host's. `BottomPanel::new(tabs, active_tab_id)` with
`BottomPanelItem`s, plus `with_maximised`.

**Controller:** `BottomPanelController` — owns a `BottomPanelConfig`
(tabs with their `BackendWidget` content, active tab, maximised, height
fraction), renders strip and active content, and routes clicks:
`handle_click_event(x, y)` returns `Handled<BottomPanelEvent>`.

**Actions (`BottomPanelEvent`):** `TabActivated(id)`, `TabClosed(id)`,
`MaximiseToggled`, `Resized(height)` (native units).

**Smallest wiring:** attach a config to the shell and it drives the
controller for you:

```rust,ignore
let config = ShellConfig::new("App", panels).with_bottom_panel_config(BottomPanelConfig {
    tabs: vec![BottomPanelTab {
        id: "terminal".into(),
        label: "Terminal".into(),
        closable: false,
        badge: None,
        content: Box::new(MyTerminal),
    }],
    active_tab_id: "terminal".into(),
    maximised: false,
    height_fraction: 0.3,
});
```

**Example:** `examples/tui_bottom_panel.rs` / `examples/gtk_bottom_panel.rs`
(`examples/common/bottom_panel_demo.rs`).

---

## `ChatController` — `src/compose/chat_controller.rs`

**Owns:** a chat overlay — scrollable transcript, multi-line input with
history navigation, status strip, collapsible turns.

**Controller only.** `ChatController::new(id)`; push turns with
`set_transcript`, paint with `render(backend, rect)`, route with
`handle_event(&event, backend, rect)` → `Handled<ChatControllerAction>`.

**Actions (`ChatControllerAction`):** `Submit { text }`,
`StreamChunk { text }`, `Cancelled`, `StopRequested`,
`KeyPressed { key, modifiers }`, `TurnClicked { turn_idx, row_in_turn }`.

**Smallest wiring:**

```rust,ignore
let mut chat = ChatController::new("chat");
chat.set_transcript(turns.clone());
chat.render(backend, area);
// in handle():
if let Handled::Event(ChatControllerAction::Submit { text }) =
    chat.handle_event(&event, backend, area)
{
    send_to_model(text);
}
```

<!-- TODO(0.12 final pass): after D3 (#2949) ports Chat onto the contract, add
its widget (name, State, Action, UiEvent variant) and recheck this section. -->

**Example:** `examples/tui_chat.rs` / `examples/gtk_chat.rs`
(`examples/common/chat_demo.rs`).

---

## `ContextMenuController` — `src/compose/context_menu_controller.rs`

**Owns:** the one-call right-click menu. It shows the menu natively where
the backend has a native context menu (`BackendCaps::native_menu`), falls
back to the in-canvas `ContextMenu` widget elsewhere, and keeps the
`ModalStack` in step either way — one code path regardless of platform or
`MenuStyle`.

**Controller only.** `open(menu, anchor, backend)`, `close(backend)`,
`render(backend)`, and `handle(&event, backend)` → `ContextMenuOutcome`:
`Event(UiEvent)` (an already-resolved `UiEvent::ContextMenuItemActivated` /
`ContextMenuDismissed` — route it through your menu-event handler),
`Consumed`, `Ignored`.

**Smallest wiring:**

```rust,ignore
fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
    if let ContextMenuOutcome::Event(ev) = self.ctx_menu.handle(&event, backend) {
        return self.handle_menu_event(ev);
    }
    if let UiEvent::MouseDown { button: MouseButton::Right, position, .. } = event {
        self.ctx_menu.open(self.build_context_menu(), position, backend);
        return Reaction::Redraw;
    }
    Reaction::Continue
}
```

**Example:** `examples/tui_context_menu_style.rs` /
`examples/gtk_context_menu_style.rs` / `examples/macos_context_menu_style.rs`
(`examples/common/context_menu_style_demo.rs`).

---

## `DualModePalette` — `src/compose/dual_mode_palette.rs`

**Owns:** a command palette that toggles between **list mode**
(type-to-filter, Enter selects a row) and **input mode** (free-text confirm,
e.g. "new branch name"). `Tab` switches modes without clearing the query.

**Widget:** `DualModePalette` (`State` = `DualModePaletteState`, `Action` =
`DualModePaletteAction`, delivered as `UiEvent::DualModePalette(id, action)`).

**Controller:** `DualModePaletteController::new(title, input_label, items)`;
`render(rect, backend)`; `handle_event(&event, visible_rows)` →
`Handled<DualModePaletteAction>`.

**Actions (`DualModePaletteAction`):** `ItemConfirmed { idx }`,
`TextConfirmed { value }`, `QueryChanged { value }`,
`ModeToggled { new_mode }`, `SelectionChanged { idx }`, `Cancelled`.

**Smallest wiring:**

```rust,ignore
let mut pal = DualModePaletteController::new("Branches", Some("New branch name:".to_string()), items);
pal.render(popup_rect, backend);
match pal.handle_event(&event, visible_rows) {
    Handled::Event(DualModePaletteAction::ItemConfirmed { idx }) => checkout(idx),
    Handled::Event(DualModePaletteAction::TextConfirmed { value }) => create_branch(value),
    _ => {}
}
```

**Example:** `examples/tui_palette_dual_mode.rs`
(`examples/common/help_layer_demo.rs` also drives one through
`help_actions_to_palette_items`).

---

## `FilePicker` — `src/compose/file_picker.rs`

**Owns:** an in-app file open/save picker for backends without a native
dialog — fuzzy filtering, directory walking, scroll, selection, and (in save
mode) the typed query doubling as the destination filename. Paints through
the `Palette` widget.

**Widget:** `FilePicker` (`State` = `FilePickerState`, `Action` =
`PathPickerAction`, delivered as `UiEvent::PathPicker(id, action)`).

**Controller:** `FilePickerController::new(mode, root, filters)` with
`FilePickerMode::Open` / `Save`; `render(rect, backend)`;
`handle_event(&event, visible_rows)` → `Handled<PathPickerAction>`.

**Actions (`PathPickerAction`, shared with `FolderPicker`):**
`Confirmed { path }`, `Cancelled`, `QueryChanged { .. }`,
`SelectionChanged { .. }`, `Navigated { .. }`.

**Smallest wiring:** identical to `FolderPicker` below, with the type and
constructor swapped.

**Example:** `examples/tui_file_picker.rs` / `examples/gtk_file_picker.rs`
(`examples/common/file_picker_app.rs`). `TuiPlatformServices`'s
`show_file_open_dialog` / `show_file_save_dialog` use it as the terminal's
dialog.

---

## `FocusGroup` — `src/compose/focus_group.rs`

**Owns:** Tab/Shift+Tab cycling between N focusable regions **by index**,
with a dynamic count. `SidebarSystem` and `TabGroupController` use it for
their own section and pane cycling.

**Not for app code.** For keyboard focus between widgets, return them
from `AppLogic::frame_tree` and list their ids and rects in
`AppLogic::extra_tab_stops`. A click focuses any `frame_tree` widget with
an `Interactive::focus` stop; Tab and Shift+Tab cycle only through the
widgets `extra_tab_stops` names, and reach `AppLogic::handle` unclaimed
while it returns nothing. Focus is tracked in the backend's `FocusTree`.

**API:** `FocusGroup::new(count)`, `active()`, `set_active(idx)`,
`count()`, `set_count(count)`, `cycle(delta)`. No events.

<!-- TODO(0.12 final pass): W6 (#2955/#2971, with #2953) deletes `FocusGroup`.
Once `src/compose/focus_group.rs` is gone, delete this section
(`readme_truth`'s `docs_compose_md_covers_every_compose_module` only requires a
section for modules that still exist). -->

**Example:** none standalone — exercised inside `SidebarSystem`
(`examples/msv_multi_tree.rs`) and `TabGroupController`
(`examples/tui_tabgroup.rs`).

---

## `FolderPicker` — `src/compose/folder_picker.rs`

**Owns:** an interactive directory-browsing modal — filesystem walking,
fuzzy filtering, scroll, selection. Paints through the `Palette` widget.

**Widget:** `FolderPicker` (`State` = `FolderPickerState`, `Action` =
`PathPickerAction`, delivered as `UiEvent::PathPicker(id, action)`).

**Controller:** `FolderPickerController::rooted_at(root, extra_file_names)`,
optionally `.with_show_hidden(true)`; `render(rect, backend)`;
`handle_event(&event, visible_rows)` → `Handled<PathPickerAction>`.

**Smallest wiring:**

```rust,ignore
let mut picker = FolderPickerController::rooted_at(start_dir, Vec::new());
picker.render(popup_rect, backend);
if let Handled::Event(PathPickerAction::Confirmed { path }) =
    picker.handle_event(&event, visible_rows)
{
    open_workspace(path);
}
```

**Example:** `examples/tui_folder_picker.rs` / `examples/gtk_folder_picker.rs`
(`examples/common/folder_picker_app.rs`). `TuiPlatformServices`'s folder
dialog uses it too.

---

## `FormController` — `src/compose/form_controller.rs`

**Owns:** one `Form`'s focus, keyboard editing, validation and built-in
scrollbar, for a host that routes raw events itself.

**For new code, use the `Form` widget.** `Form` implements the contract
itself (`primitives/form/`): list it in `AppLogic::frame_tree` and the
runtime routes Tab traversal and field editing to it, emitting `FormEvent`s.

**Controller:** `FormController::new(id)`; `set_form(form)` each frame;
`render(backend, rect)`; `handle_event(&event, backend, rect)` →
`Handled<FormControllerAction>` (`FormAction(FormEvent)` or
`ScrollChanged`); `handle_cached_event(&event, rect)` when the handler has no
backend in reach. Tab / Shift+Tab move focus between editable fields and emit
`FormEvent::FocusChanged { id }`; typed text, Backspace/Delete and arrow
keys edit the focused text field and emit `FormEvent::TextInputChanged`;
Enter emits `FormEvent::TextInputCommitted`. The cursor and selection live
inside the controller; your model keeps only the `String` value.

<!-- TODO(0.12 final pass): the collapse (#2977/#2978) deletes `FormController`
when the binding `form` kind moves to `FormState`. Once
`src/compose/form_controller.rs` is gone, delete this section and point
`examples/common/form_scroll.rs` readers at the `Form` widget. -->

**Example:** `examples/tui_form_scroll.rs` / `examples/gtk_form_scroll.rs`
(`examples/common/form_scroll.rs`).

---

## `FormDialog` — `src/compose/form_dialog.rs`

**Owns:** a modal `Dialog` hosting a multi-field `Form`, plus the one thing
neither has alone: keyboard focus moving between the form's fields and the
dialog's buttons as one cyclic order. Tab / Shift+Tab traverse the fields,
then the buttons; Left / Right cycle the buttons; Enter activates the
focused button (or goes to the form when no button has focus); Escape
resolves to the cancel button, falling back to the default one.

**Widget:** `FormDialog::new(id, title)` (`State` = `FormDialogState`,
`Action` = `FormDialogAction`).

**Controller:** `FormDialogController::new(id, title)` with
`.with_buttons(..)`, `.with_body(..)`, `.with_severity(..)`;
`set_form(form)`; `render(backend)` (it centres itself in the viewport);
`handle_event(&event, backend)` → `Handled<FormDialogAction>`.

**Actions (`FormDialogAction`):** `ButtonClicked(id)`,
`FormAction(FormEvent)`.

**Smallest wiring:**

```rust,ignore
let mut dialog = FormDialogController::new("create_milestone", "Create milestone")
    .with_buttons(vec![cancel_button, create_button]);
dialog.set_form(my_form_descriptor);
dialog.render(backend);
match dialog.handle_event(&event, backend) {
    Handled::Event(FormDialogAction::ButtonClicked(id)) if id.as_str() == "create" => submit(),
    Handled::Event(FormDialogAction::FormAction(ev)) => apply_field_change(ev),
    _ => {}
}
```

**Example:** `examples/tui_form_dialog.rs`
(`examples/common/form_dialog_demo.rs`).

---

## `HelpRegistry` / `HelpOverlay` — `src/compose/help_layer.rs`

**Owns:** a context-sensitive `?` help overlay, in two parts.
`HelpRegistry` is where each view registers a `ViewHelp` (reference
`HelpNote`s and `HelpAction`s) under its id; the overlay paints the active
view's help as a `Dialog` cheatsheet. `help_actions_to_palette_items` /
`filter_help_actions` feed the same actions into a `DualModePalette`, so
they're searchable by label and description.

**Widget:** `HelpOverlay` (`id`, `open`, `help`; `Action` =
`HelpOverlayAction`, delivered as `UiEvent::HelpOverlay(id, action)`).

**Controller:** `HelpOverlayController::new()`; `open_action()` /
`close_action()` / `toggle_action()`;
`render(container, backend, registry.get(view_id))`;
`handle_event(&event)` → `Handled<HelpOverlayAction>` (`Opened`, `Closed`).

**Smallest wiring:**

```rust,ignore
let mut registry = HelpRegistry::new();
registry.register("editor", ViewHelp::new("Editor").with_notes(notes).with_actions(actions));
let mut overlay = HelpOverlayController::new();
// on `?`:
overlay.toggle_action();
// every frame:
overlay.render(area, backend, registry.get("editor"));
```

**Example:** `examples/tui_help_layer.rs` / `examples/gtk_help_layer.rs`
(`examples/common/help_layer_demo.rs`).

---

## `HoverTooltipTracker` — `src/compose/hover_tooltip_tracker.rs`

**Owns:** *when* a hover tooltip is on screen. A pointer crossing a widget
on its way elsewhere must not flash its tooltip, so the tracker waits
`Style::tooltip_delay_ms` of continuous hover before the tooltip becomes
visible, then hides it on any of the three ways a tooltip ends: the pointer
leaving, an explicit dismiss (a click, Escape), or an auto-dismiss timeout
while the pointer never moved. Generic over the item that identifies what
is hovered — a `WidgetId`, a row index, the tooltip text.

Every method takes the current time as a caller-supplied `u64` of
milliseconds and never reads a clock, so a test drives the whole state
machine with hand-picked timestamps and a binding feeds it a plain integer.

Painting is not its job: it answers *which* item's tooltip to show, and the
host paints the `Tooltip` widget, last of all so it sits on top.

**Helper, no events.** `on_hover(item, now_ms)`, `tick(delay_ms, now_ms)`,
`tick_auto_dismiss(timeout_ms, now_ms)` and `dismiss()` return `true` when
the caller should redraw; read the answer from `visible_item()`.
`next_wake_ms` tells a runner when to schedule the next tick.

**Smallest wiring:**

```rust,ignore
let mut tooltip: HoverTooltipTracker<WidgetId> = HoverTooltipTracker::new();

// On a mouse move: which painted widget is under the pointer?
let hovered = quadraui::primitive::painted_hit(x, y).map(|(id, _sub)| id);
tooltip.on_hover(hovered, now_ms);

// Every frame: notice the delay and the auto-dismiss timeout elapsing.
tooltip.tick(style.tooltip_delay_ms, now_ms);
tooltip.tick_auto_dismiss(5_000, now_ms);

// Render, last: paint the tooltip for whatever is visible.
if let Some(id) = tooltip.visible_item() {
    let tip = Tooltip::new(*id, tooltip_text_for(id));
    let layout = tip.layout(anchor_of(id), viewport, measure, margin);
    backend.draw_dyn(layout.bounds, &tip, &());
}
```

**Example:** no standalone demo — `examples/tui_tooltip.rs` /
`examples/gtk_tooltip.rs` (`examples/common/tooltip_demo.rs`) paint the
`Tooltip` widget this decides the visibility of, driven from keys. The
`#[cfg(test)]` module in `hover_tooltip_tracker.rs` is the direct-use
reference.

---

## Markdown → `StyledText` — `src/compose/markdown.rs`

Not stateful: a conversion function, `render_markdown_to_styled(source,
&theme)` (and `render_markdown_to_styled_wrapped`), from a CommonMark subset
to a `RenderedMarkdown` — styled lines, their plain text, per-line text
scales (headings) and code-block ranges. Headings, bold/italic (with
CommonMark flanking rules, so `snake_case` and `a * b * c` stay upright),
inline code, lists, fenced code blocks, links and blockquotes; the module
doc has the full table.

**Smallest wiring:** feed the result to a `RichTextPopup` and paint it:

```rust,ignore
let md = render_markdown_to_styled(source, &theme);
let popup = RichTextPopup {
    lines: md.lines,
    line_text: md.line_text,
    line_scales: md.line_scales,
    ..popup_template
};
let measure = popup.measure_for(backend, content_width);
let cw = backend.char_width();
let layout = popup.layout(anchor_x, anchor_y, viewport, measure, |_, start, end| {
    (end - start) as f32 * cw
});
backend.draw_dyn(layout.bounds, &popup, &());
```

**Example:** `examples/tui_markdown.rs` / `examples/gtk_markdown.rs`
(`examples/common/markdown_demo.rs`).

---

## `AppMenu` / `MenuSystem` — `src/compose/menu_system.rs`

**Owns:** a menu bar and its dropdowns — open/close, Alt-key activation,
arrow-key navigation, hover-to-switch between open menus, submenus. Define
the structure once as `MenuDef`s and match on activations. On a backend
whose effective menu style is native (the macOS system menu bar), the bar
is installed natively instead of painted.

**Widget:** `AppMenu::new(id, menus)` (`State` = `AppMenuState`, `Action` =
`AppMenuEvent`, delivered as `UiEvent::AppMenu(id, event)`). Which menu is
open is the `AppMenuView` the app holds; the dropdowns are popups
(`Interactive::popups`), so the runtime paints them above the frame and
routes them first. `AppMenuEvent::Changed(view)` asks the app to store a new
view (`AppMenu::apply` does it); `AppMenuEvent::Activated(id)` is a chosen
item. `Primitive::native_presentation` installs the bar natively where the
backend presents menus natively.

**Controller:** `MenuSystem::new(menus)`; `render(backend, bar_rect)`;
`handle(&event, backend, bar_rect)` → `MenuEvent` (`Activated(id)`,
`StateChanged`, `Consumed`, `Ignored`).

**Smallest wiring:**

```rust,ignore
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

## `MessageDialog` — `src/compose/message_dialog.rs`

**Owns:** the in-canvas message/alert box — keyboard button focus and
resolve-on-click — giving backends without a native alert the same
show → resolve → return-a-choice contract
`PlatformServices::show_message_dialog` has everywhere else.

**Widget:** `MessageDialog::new(options)` from the same
`MessageDialogOptions` `show_message_dialog` takes (`State` =
`MessageDialogState`; the resolved choice is the chosen button's `WidgetId`,
delivered as `UiEvent::MessageDialogResolved(choice)`). It centres itself in
the rect it is laid out in, so hand it the whole viewport.

**Controller:** `MessageDialogController::new(options)`, optionally
`.with_id(..)` / `.with_vertical_buttons(..)`; `render(backend)`;
`handle_event(&event, backend)` → `Handled<MessageDialogChoice>`. Shape 2
above is a complete example.

**Modal stack:** a host with its own engine pushes the dialog's bounds onto
`Backend::modal_stack_handle()` when it opens and pops them when it closes,
one push per open and one pop per resolve or dismiss, and offers events to
the dialog before any other overlay so hit-test precedence matches paint
order. `quadraui-binding-core`'s interaction engine does this for
`open_message_dialog` / `close_message_dialog`.

**Example:** `examples/tui_message_dialog.rs` / `examples/tui_message_dialog_app.rs`
/ `examples/gtk_message_dialog.rs` / `examples/gtk_message_dialog_app.rs`
(`examples/common/message_dialog_app.rs`). `TuiPlatformServices`'s
`show_message_dialog` uses it as the terminal's alert.

---

## `notify_or_toast` — `src/compose/notification.rs`

Not stateful: one branch point. `notify_or_toast(backend, notification)`
sends a native notification where `BackendCaps::notifications` is `true`,
and otherwise returns `Some(Toast)` for you to push onto your own toast
stack (TUI has no OS notifications). One call instead of repeating the
capability check at every notify site.

**Smallest wiring:**

```rust,ignore
if let Some(toast) = notify_or_toast(backend, Notification::new("Build", "Succeeded")) {
    self.toasts.push(toast); // no native notifications here; show it in-canvas
}
```

**Example:** none dedicated — `notification.rs`'s unit tests cover it, and
`examples/gtk_platform_services.rs` exercises the native side of the same
capability check.

---

## `SidebarPanelBody` — `src/compose/sidebar_panel_body.rs`

**Owns:** the four layers of a sidebar panel — background fill, optional
header/search chrome, the panel's own body widget, an optional
scrollbar-gutter reservation — painted in one ordered call.

**Widget, paint only.** `SidebarPanelBody` is a `Primitive` with no
behaviour (`non_interactive!`), built fresh each frame as a struct literal
(`background`, `chrome`, `scrollbar_gutter`; it derives `Default`). The
chrome is `SidebarPanelChrome::None`, `Header(title)`,
`HeaderAndSearch { .. }`, `Search { .. }` or `StatusBars(bars)`; the
header/search shapes paint through `paint_settings_chrome` /
`paint_settings_chrome_cells`. `render(backend, rect, &body)` paints the
background and chrome through `draw_dyn`, then hands the remaining rect to
your `BackendWidget` body and returns a `SidebarPanelBodyLayout` (body rect,
gutter rect, the status bars' hit regions). Click routing stays with
whatever owns the body widget.

**Smallest wiring:**

```rust,ignore
let panel = SidebarPanelBody {
    background: Some(theme.background),
    chrome: SidebarPanelChrome::Header("Explorer".into()),
    scrollbar_gutter: None,
};
let layout = panel.render(backend, panel_rect, &explorer_tree);
```

**Example:** `examples/tui_sidebar_panel_body.rs` /
`examples/gtk_sidebar_panel_body.rs`
(`examples/common/sidebar_panel_body_demo.rs`).

---

## `SidebarSystem` — `src/compose/sidebar_system.rs`

**Owns:** a multi-section sidebar (VS Code's Explorer/Outline/Timeline
stack) — per-section scroll and selection, active-section cycling,
collapse, scrollbar drag, keyboard navigation, and two-layer click dispatch
(section header, then body with coordinates translated). Each section is
tree-bodied (`SidebarSectionDef::new`) or form-bodied
(`SidebarSectionDef::form`).

**Controller only.** `SidebarSystem::new(defs)`; `set_rows(section, rows)`
/ `set_form(section, form)`; `render(backend, rect)`;
`handle(&event, backend, rect)` → `SidebarEvent`.

**Events (`SidebarEvent`):** `HeaderActivated { section }`,
`RowSelected { section, path }`, `RowActivated { section, path }`,
`RowToggleExpand { section, path }`, `ContextMenuRequested { .. }`,
`ScrollChanged { section }`, `EditConfirmed { .. }`,
`EditCancelled { section, path }`, `EditChanged { .. }`,
`FormEvent { section, event }`, `StateChanged`, `Consumed`, `Ignored`.

**Smallest wiring:**

```rust,ignore
let mut sidebar = SidebarSystem::new(vec![SidebarSectionDef::new("files", "Files")]);
sidebar.set_rows(0, file_tree_rows);
sidebar.render(backend, sidebar_rect);
if let SidebarEvent::RowActivated { path, .. } = sidebar.handle(&event, backend, sidebar_rect) {
    open_file(path);
}
```

<!-- TODO(0.12 final pass): after D7 (#2953, SidebarSystem half) ports this onto
the contract, add its widget, State and Action, and recheck the event list. -->

**Example:** `examples/msv_multi_tree.rs` / `examples/gtk_multi_tree.rs`
(`examples/common/multi_tree.rs`); search and toggle sections in
`examples/tui_sidebar_search.rs` / `examples/gtk_sidebar_search.rs`
(`examples/common/sidebar_search.rs`).

---

## `StatusBarInteraction` — `src/compose/status_bar_interaction.rs`

**Owns:** nothing of its own — it adapts raw `UiEvent`s to `StatusBar`'s
own hover/press state machine (`StatusBarState`, the same one `StatusBar`'s
`Interactive` impl drives), for a host that routes its own clicks rather
than listing the bar in its `FrameTree`.

**Helper.** After painting the bar, give it the layout the paint used
(`set_layout(backend.layout_primitive(rect, &bar).layout)`); then
`handle_event(&event, bar_rect)` → `Handled<WidgetId>`: `Event(id)` when a
segment was clicked (press and release on the same segment), `Consumed`
when hover or press changed, `Ignored` otherwise. Hover tints paint from
the runtime's live pointer state, so there is nothing to pass back into the
paint call.

**Smallest wiring:**

```rust,ignore
// render
let _ = backend.draw_dyn(bar_rect, &bar, &());
self.status.set_layout(backend.layout_primitive(bar_rect, &bar).layout);
// handle
if let Handled::Event(id) = self.status.handle_event(&event, bar_rect) {
    run_segment_command(id);
}
```

**Example:** `examples/tui_full_chrome_demo.rs` /
`examples/gtk_full_chrome_demo.rs` (`examples/common/full_chrome_demo.rs`,
its title bar).

---

## `TabGroupController` — `src/compose/tab_group.rs`

**Owns:** an editor-group-style tabbed split layout — `TabBar` + `Split` +
`DropZone` wired into N panes in an arbitrary nested horizontal/vertical
split tree, each pane with its own scrollable tab bar. Tab activation,
closing, new-tab, pane focus, divider drag and drag-a-tab-to-another-pane
all go through one controller.

**Controller only.** `TabGroupController::with_pane(pane_id, tabs,
active_tab_id, split_direction)`; `render(backend, bounds)` →
`TabGroupLayout`; `handle_click_event(x, y)` → `Handled<TabGroupEvent>`;
`handle_drag_start` / `handle_drag_move_event` / `handle_drag_end` for
divider drags; `handle_tab_drag_start` / `handle_tab_drag_move` /
`handle_tab_drop_event` for moving tabs.

**Events (`TabGroupEvent`):** `TabActivated { pane_idx, tab_id }`,
`TabClosed { pane_idx, tab_id }`, `PaneCollapsed { .. }`,
`PaneAdded { pane_idx }`, `PaneFocused { pane_idx }`,
`DividerResized { divider_idx }`, `NewTabRequested { pane_idx }`,
`TabReordered { .. }`, `TabMovedToPane { .. }`, `TabSplitToNewPane { .. }`.

**Smallest wiring:**

```rust,ignore
let tabs = vec![PaneTab {
    id: "t0".into(),
    label: "main.rs".into(),
    closable: true,
    content: Box::new(MyContent),
}];
let mut group = TabGroupController::with_pane("main", tabs, "t0", SplitDirection::Horizontal);
let layout = group.render(backend, area);
// on a click at (x, y):
if let Handled::Event(TabGroupEvent::TabActivated { tab_id, .. }) = group.handle_click_event(x, y) {
    activate(tab_id);
}
```

<!-- TODO(0.12 final pass): after D9 (#2955) ports TabGroup onto the contract,
add its widget, State and Action, and recheck the constructor and event list. -->

**Example:** `examples/tui_tabgroup.rs` / `examples/gtk_tabgroup.rs`
(`examples/common/tab_group_demo.rs`); active-tab bracket chrome in
`examples/tui_tab_chrome.rs` / `examples/gtk_tab_chrome.rs`
(`examples/common/tab_chrome_demo.rs`).

---

## `ToastStackController` — `src/compose/toast_stack.rs`

**Owns:** a keyboard-focus cursor over a `ToastOverlay` you build yourself
every frame. Toasts are non-modal, so the controller never blocks input
while unfocused. Give it focus explicitly (a "focus notifications" binding);
then Tab / Shift+Tab / Left / Right cycle a toast's buttons, Up / Down move
between toasts, Enter activates and Escape dismisses and returns focus to
the app.

**Controller only.** `ToastStackController::new()`; `give_focus(&stack)` /
`take_focus()`; `handle_event(&event, &stack)` →
`Handled<ToastStackAction>` (`Action(id)`, `Dismiss(id)`, `FocusReturned`).

**Smallest wiring:**

```rust,ignore
let mut controller = ToastStackController::new();
controller.give_focus(&toasts); // the "focus notifications" binding
if let Handled::Event(ToastStackAction::Action(button_id)) = controller.handle_event(&event, &toasts) {
    run_toast_action(button_id);
}
```

**Example:** `examples/tui_toast_actions.rs` / `examples/gtk_toast_actions.rs`
(`examples/common/toast_actions_app.rs`).

---

## `ToolbarHoverTracker` — `src/compose/toolbar_hover_tracker.rs`

**Owns:** "which item is under the pointer" for a host that routes its own
mouse moves: `ToolbarHoverTracker` (a `Toolbar` button's `WidgetId`),
`ListHoverTracker` and `TreeHoverTracker` (a row index). `update(&layout, x,
y)` returns `true` when the hovered item changed.

**Not needed for a `Toolbar` painted with `draw_dyn`.** The runtime tracks
the pointer against every widget painted that way and hands hover and press
to the widget's paint through `PaintEnv`, so the toolbar tints its hovered
button with no tracker.

<!-- TODO(0.12 final pass): DC1 (#2957) deletes `TreeHoverTracker` and keeps
`src/compose/toolbar_hover_tracker.rs` with `ToolbarHoverTracker` (the same
rule as README.md's compose-table marker). After W9, check whether
`ListHoverTracker` survived the residue sweep (its in-tree user is
`gtk/list.rs`), then drop each departed type from this section; keep the
section while the file exists. -->

**Example:** `examples/tui_sidebar_panel.rs` / `examples/gtk_sidebar_panel.rs`
(`examples/common/sidebar_panel_app.rs`).

---

## `TreeController` — `src/compose/tree_controller.rs`

**Owns:** one keyboard-navigable `TreeView` with scrollbar, for a host that
routes raw events itself — selection with scroll-to-follow, click
hit-testing, scrollbar thumb drag, wheel scrolling, inline rename.
`SidebarSystem` builds one per tree-bodied section.

**For a standalone tree, the `TreeView` widget routes itself.** `TreeView`
implements the contract (`State` = `TreeViewState`, `Action` = `TreeEvent`):
list it in `AppLogic::frame_tree` and the runtime handles selection,
navigation and typeahead.

**Controller:** `TreeController::new(id)`; `set_rows(rows)`;
`render(backend, rect)`; `handle(&event, backend, rect)` →
`TreeControllerEvent` (`RowSelected { path }`, `RowActivated { path }`,
`RowToggleExpand { path }`, `ScrollChanged`, `EditConfirmed { .. }`,
`EditCancelled { .. }`, `EditChanged { .. }`,
`ContextMenuRequested { .. }`, `Consumed`, `Ignored`).

**Smallest wiring:**

```rust,ignore
let mut tree = TreeController::new("files");
tree.set_rows(rows);
tree.render(backend, area);
if let TreeControllerEvent::RowActivated { path } = tree.handle(&event, backend, area) {
    open(path);
}
```

<!-- TODO(0.12 final pass): after the collapse (#2977/#2978) removes
`classify_tree_nav_key` and the `*_via` delegates, check whether
`TreeController` survives and update or delete this section. -->

**Example:** no standalone demo — the tree examples drive it through
`SidebarSystem` (`examples/msv_multi_tree.rs`,
`examples/common/sidebar_panel_body_demo.rs`). Its `#[cfg(test)]` module is
the direct-use reference.

---

## `TreeNavCore` — `src/behaviour/nav.rs`

Not a compose module: it lives in `quadraui::behaviour` beside the other
shared interaction rules (`ensure_visible`, `FlatNav`, `Typeahead`), and is
listed here because `TreeController` and `TreeTableController` are built on
it.

**Owns:** the row-type-agnostic half of tree navigation — selection by
`TreePath`, Up/Down/PageUp/PageDown/Home/End, the Left/Right
collapse-else-select-parent / expand-else-select-first-child rule,
scroll-to-follow, wheel scrolling and thumb drag. It knows nothing about
painting or hit-testing: a row type implements `NavRow` (`path`, `indent`,
`is_expanded`) and the core navigates any flattened row slice.

**State core, no events.** Movement returns `NavOutcome::{Empty, Unchanged,
Selected(path)}`; Left/Right return `HorizontalNav::{Collapse, SelectParent,
Expand, SelectFirstChild, None}`. The owner decides which of its own events
those become.

**Smallest wiring:**

```rust,ignore
// `rows: Vec<MyRow>` where `impl NavRow for MyRow`.
if let NavOutcome::Selected(path) = nav.move_selection_by(&rows, 1, viewport_rows) {
    emit(MyEvent::RowSelected { path });
}
let first_visible = nav.scroll_offset();
```

**Example:** none standalone; its `#[cfg(test)]` module in
`behaviour/nav.rs` is the direct-use reference.

---

## `TreeTableController` — `src/compose/tree_table_controller.rs`

**Owns:** one keyboard-navigable `TreeTable` driven from a *hierarchical*
model instead of an app-maintained flattened row list. It holds
`TreeTableNodeData` nodes keyed by a stable `NodeId`, the expanded set and
the sort column, and flattens them into a `TreeTable` on demand (`flatten`,
which is also what `render` paints). It adds three things an app on a raw
`TreeTable` would hand-roll: expand/collapse re-flattening,
**sibling-scoped** column sort (`header_clicked` sorts each loaded level's
children among themselves, so the hierarchy survives a sort), and **lazy
child loading** — a node built with `TreeTableNodeData::lazy_branch` emits
`ChildrenRequested` once on first expand and shows a dim `…` placeholder
until `set_children` answers. Navigation goes through `TreeNavCore`, with
the controller translating between `NodeId` and the positional `TreePath`.

**Controller only.** `TreeTableController::new(id)`; `set_nodes(nodes)`;
`render(backend, rect, columns)`; `handle(&event, backend, rect, columns)`
→ `TreeTableControllerEvent` (`RowSelected { id }`, `RowActivated { id }`,
`ToggleExpand { id, expand }`, `ChildrenRequested { id }`,
`SortChanged { col, ascending }`, `Consumed`, `Ignored`).

**Smallest wiring:**

```rust,ignore
let mut tree = TreeTableController::new("files");
tree.set_nodes(vec![
    TreeTableNodeData::branch("src", cells("src"), vec![
        TreeTableNodeData::leaf("src/main.rs", cells("main.rs")),
    ]),
    TreeTableNodeData::lazy_branch("remote", cells("remote")),
]);
tree.render(backend, rect, columns.clone());
match tree.handle(&event, backend, rect, columns) {
    TreeTableControllerEvent::ChildrenRequested { id } => {
        let children = load(&id);
        tree.set_children(&id, children);
    }
    TreeTableControllerEvent::RowActivated { id } => open(&id),
    _ => {}
}
```

<!-- TODO(0.12 final pass): after D2 (#2948) ports TreeTable and the collapse
(#2977) moves `tree_table_controller` off the binding extras, recheck this
section (lazy children and the sort override become descriptor content). -->

**Example:** `quadraui-gallery/src/demos/tree_table.rs` — expand/collapse,
a header-click sort and the lazy `remote/` branch. Its `#[cfg(test)]`
module is the direct-use reference.

---

## `Workspace` — `src/compose/workspace.rs`

**Owns:** an **open-N-view-one** set of opaque document ids with exactly
one active, shown as a `TabBar` strip: open, close, activate, reorder,
preview tabs, click and keyboard routing. Unlike `TabGroupController` it
owns no content: the host paints the active document's body, which can
borrow app state.

**Widget:** `Workspace::new(id)` with `.with_docs(..)`, `.with_active(..)`,
`.with_preview(..)` (`State` = `WorkspaceState`, `Action` =
`WorkspaceEvent`, delivered as `UiEvent::Workspace(id, event)`). Shape 1
above is a complete example. A press on the body area is the host's.

**Controller:** `WorkspaceController::new(bar_id)`; `open(doc)`,
`close(id)`, `activate(id)` and `reorder(from, to)` return the
`WorkspaceEvent`s they caused; `render(backend, bounds)` → `WorkspaceLayout` (`strip_bounds`,
`body_bounds`); `handle_click_event(x, y, button)` →
`Handled<Vec<WorkspaceEvent>>`; `handle_key_event(&key, modifiers)` →
`Handled<WorkspaceEvent>`.

**Actions (`WorkspaceEvent`):** `Opened { id, index }`,
`Activated { id, index }`, `Closed { id, index }`,
`Reordered { id, from, to }`, `Promoted { id }`.

**Smallest wiring (controller):**

```rust,ignore
let mut ws = WorkspaceController::new("panel:docs");
ws.open(WorkspaceDoc::new("doc:a", "alpha.rs"));
let layout = ws.render(backend, panel_rect);
paint_body(backend, layout.body_bounds);
// on a click at (x, y):
if let Handled::Event(events) = ws.handle_click_event(x, y, MouseButton::Left) {
    for ev in events {
        if let WorkspaceEvent::Activated { id, .. } = ev {
            switch_to(id);
        }
    }
}
```

**Example:** `examples/tui_workspace.rs` / `examples/gtk_workspace.rs`
(`examples/common/workspace_demo.rs`).

[`Primitive`]: https://docs.rs/quadraui/latest/quadraui/primitive/trait.Primitive.html
[`Interactive`]: https://docs.rs/quadraui/latest/quadraui/interactive/trait.Interactive.html
[`Handled<Action>`]: https://docs.rs/quadraui/latest/quadraui/enum.Handled.html
