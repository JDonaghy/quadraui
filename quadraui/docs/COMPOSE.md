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

### Shape 2: a widget you drive with raw events

You keep the widget and its state; you paint it with `draw_dyn` and offer
it every event until it resolves:

```rust
use quadraui::{
    AppLogic, Backend, DrawsPrimitives, Handled, MessageDialog, MessageDialogButton,
    MessageDialogOptions, MessageDialogState, Reaction, UiEvent, WidgetId,
};

struct Confirm {
    dialog: Option<(MessageDialog, MessageDialogState)>,
    answer: Option<WidgetId>,
}

impl Confirm {
    fn open(&mut self) {
        let dialog = MessageDialog::new(MessageDialogOptions {
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
        });
        self.dialog = Some((dialog, MessageDialogState::default()));
    }
}

impl AppLogic for Confirm {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        if let Some((dialog, _)) = &self.dialog {
            backend.draw_dyn(backend.viewport().rect(), dialog, &());
        }
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        if let Some((dialog, state)) = self.dialog.as_mut() {
            match dialog.handle_event(state, &event, backend) {
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

## `Chat` — `src/compose/chat.rs`

**Owns:** a chat overlay — scrollable transcript, multi-line input with
history navigation, status strip, collapsible turns.

**Widget:** `Chat` (`State` = `ChatState`, `Action` = `ChatEvent`, delivered
as `UiEvent::Chat(id, event)`). Every visible field is on the descriptor;
`Chat::apply` stores each write-back event (`InputChanged`,
`TranscriptScrolled`, `FocusedTurnChanged`, `TurnCollapsed`).

**Semantic events (`ChatEvent`):** `Submit { text }`,
`StreamChunk { text }`, `Cancelled`, `StopRequested`,
`KeyPressed { key, modifiers }`, `TurnClicked { turn_idx, row_in_turn }`.

**Smallest wiring:**

```rust,ignore
// render(): paint it; frame_tree(): register it.
frame.push(Surface::Widget { rect: area, widget: Box::new(chat.clone()) });
tree.push(chat.id, area, Box::new(chat.clone()));
// handle():
if let UiEvent::Chat(_, event) = event {
    chat.apply(&event);
    if let ChatEvent::Submit { text } = event {
        send_to_model(text);
        chat.clear_input();
    }
}
```

A host whose events do not pass through `AppLogic::frame_tree` (a
`ShellApp`) keeps a `ChatState` and drives the chat with
`compose::drive_ui_event`.

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

`DualModePalette::apply(&action, visible_rows)` stores the query, mode and
highlight changes. An unmodified `j`/`k` moves the highlight in list mode
only with `vim_keys` set (`with_vim_keys(true)`); by default both type into
the query.

**Actions (`DualModePaletteAction`):** `ItemConfirmed { idx }`,
`TextConfirmed { value }`, `QueryChanged { value }`,
`ModeToggled { new_mode }`, `SelectionChanged { idx }`, `Cancelled`.

**Smallest wiring:**

```rust,ignore
let pal = DualModePalette::new(id, "Branches", Some("New branch name:".to_string()), items);
// render()/frame_tree(): Surface::Widget + tree.push(pal.id, popup_rect, ...).
// handle():
if let UiEvent::DualModePalette(_, action) = event {
    pal.apply(&action, visible_rows);
    match action {
        DualModePaletteAction::ItemConfirmed { idx } => checkout(idx),
        DualModePaletteAction::TextConfirmed { value } => create_branch(value),
        _ => {}
    }
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

**Construct:** `FilePicker::new(mode, root, filters)` with
`FilePickerMode::Open` / `Save`; paint it with `draw_dyn`; a raw-event host
calls `handle_event(&event, visible_rows)` → `Handled<PathPickerAction>` and
stores each change with `apply(&action, visible_rows)`.

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

## `FolderPicker` — `src/compose/folder_picker.rs`

**Owns:** an interactive directory-browsing modal — filesystem walking,
fuzzy filtering, scroll, selection. Paints through the `Palette` widget.

**Widget:** `FolderPicker` (`State` = `FolderPickerState`, `Action` =
`PathPickerAction`, delivered as `UiEvent::PathPicker(id, action)`).

**Construct:** `FolderPicker::rooted_at(root, extra_file_names)`, optionally
`.with_show_hidden(true)`; paint it with `draw_dyn`; a raw-event host calls
`handle_event(&event, visible_rows)` → `Handled<PathPickerAction>` and stores
each change with `apply(&action, visible_rows)`.

**Smallest wiring:**

```rust,ignore
let mut picker = FolderPicker::rooted_at(start_dir, Vec::new());
backend.draw_dyn(popup_rect, &picker, &());
match picker.handle_event(&event, visible_rows) {
    Handled::Event(PathPickerAction::Confirmed { path }) => open_workspace(path),
    Handled::Event(change) => picker.apply(&change, visible_rows),
    _ => {}
}
```

**Example:** `examples/tui_folder_picker.rs` / `examples/gtk_folder_picker.rs`
(`examples/common/folder_picker_app.rs`). `TuiPlatformServices`'s folder
dialog uses it too.

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

**Raw-event host:** build it with `.with_buttons(..)`, `.with_body(..)`,
`.with_severity(..)` and set `form` every frame; paint
`dialog.painted(&state)` with `draw_dyn` over `backend.viewport().rect()`
(it centres itself in the viewport); `dialog.handle_event(&mut state,
&event, backend)` → `Handled<FormDialogAction>`. On the runtime's routing
engine, push the widget as a `Surface::Widget` and register it in
`frame_tree` instead.

**Actions (`FormDialogAction`):** `ButtonClicked(id)`,
`FormAction(FormEvent)`.

**Smallest wiring:**

```rust,ignore
let mut dialog = FormDialog::new(WidgetId::new("create_milestone"), "Create milestone")
    .with_buttons(vec![cancel_button, create_button]);
let mut state = FormDialogState::default();
dialog.form = Some(my_form_descriptor);
backend.draw_dyn(backend.viewport().rect(), &dialog.painted(&state), &());
match dialog.handle_event(&mut state, &event, backend) {
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

**Raw-event host:** `handle_event(&event)` → `Handled<HelpOverlayAction>`
(`Opened`, `Closed`; `Consumed` while open); write each action back with
`apply(action)`. `toggle_action()` flips it from a host binding. Paint it with
`draw_dyn` over the window, with `help` set to the active view's `ViewHelp`.

**Smallest wiring:**

```rust,ignore
let mut registry = HelpRegistry::new();
registry.register("editor", ViewHelp::new("Editor").with_notes(notes).with_actions(actions));
let mut overlay = HelpOverlay::new(WidgetId::new("help"));
// in handle():
if let Handled::Event(action) = overlay.handle_event(&event) {
    overlay.apply(action);
}
// every frame:
let shown = HelpOverlay { help: registry.get("editor").cloned(), ..overlay.clone() };
backend.draw_dyn(area, &shown, &());
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

## `AppMenu` — `src/compose/app_menu.rs`

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

**Raw-event host:** a host that reads raw `UiEvent`s keeps the `AppMenu` and
its `AppMenuState` and calls `render(backend, bar_rect)` and
`handle_ui_event(&mut state, &event, backend, bar_rect)`, which applies every
view change itself, keeps the open dropdown on the modal stack and answers a
`Handled<AppMenuEvent>`.

**Smallest wiring:**

```rust,ignore
match self.menu.handle_ui_event(&mut self.menu_state, &event, backend, bar_rect) {
    Handled::Event(AppMenuEvent::Activated(id)) if id.as_str() == "save" => save(),
    Handled::Ignored => {}
    _ => return Reaction::Redraw,
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

**Raw-event host:** optionally `.with_id(..)` / `.with_vertical_buttons(..)`;
paint it with `draw_dyn` over `backend.viewport().rect()`;
`dialog.handle_event(&mut state, &event, backend)` →
`Handled<MessageDialogChoice>`; `dialog.bounds(backend)` for a modal-stack
entry. Shape 2 above is a complete example.

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

## `Sidebar` — `src/compose/sidebar.rs`

**Owns:** a multi-section sidebar (VS Code's Explorer/Outline/Timeline
stack) — per-section scroll and selection, active-section cycling,
collapse, scrollbar drag, keyboard navigation, and two-layer click dispatch
(section header, then body). Each section is tree-bodied
(`SidebarSectionDef::new`) or form-bodied (`SidebarSectionDef::form`).

**Widget.** `Sidebar` implements the contract (`State` = `SidebarState`,
`Action` = `SidebarAction`) in controlled mode: collapse, visibility,
badges, the active section, each tree's rows, selection and scroll, and
the panel scroll are its fields, and every change comes back as a
`SidebarAction` the host applies (`Sidebar::apply`). List it in
`AppLogic::frame_tree`, or drive it with raw events through
`Sidebar::handle_event(&mut state, &event, backend, rect)`, which answers
every action the event produced. `Sidebar::from_defs(id, &defs)` builds
it; `set_rows` keeps a row's rename session across rebuilt rows;
`start_editing`, `reveal` and `toggle_expand_selected` cover the
programmatic gestures.

**Actions (`SidebarAction`):** `HeaderActivated { section }`,
`ActiveSectionChanged { section }`, `CollapseChanged { .. }`,
`RowSelected { section, path }`, `RowActivated { section, path }`,
`RowToggleExpand { section, path }`, `ContextMenuRequested { .. }`,
`ScrollChanged { section, offset }`, `PanelScrolled { offset }`,
`EditChanged { .. }`, `EditConfirmed { .. }`,
`EditCancelled { section, path }`, `Form { section, event }`.

**Smallest wiring:**

```rust,ignore
let mut sidebar = Sidebar::from_defs("files", &[SidebarSectionDef::new("files", "Files")]);
let mut state = SidebarState::default();
sidebar.set_rows(0, file_tree_rows);
backend.draw_dyn(sidebar_rect, &sidebar, &());
if let Handled::Event(actions) = sidebar.handle_event(&mut state, &event, backend, sidebar_rect) {
    for action in actions {
        if let SidebarAction::RowActivated { path, .. } = &action {
            open_file(path);
        }
        sidebar.apply(&action);
    }
}
```

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

## `TabGroup` — `src/compose/tab_group.rs`

**Owns:** an editor-group-style tabbed split layout — a `TabBar` per pane,
one `SplitTree` for the dividers and a `DropOverlay` popup for tab drags —
over N panes in an arbitrary nested horizontal/vertical split tree, each
pane with its own scrollable tab strip. Tab activation, closing, new-tab,
pane focus, divider drag and drag-a-tab-to-another-pane all route through
the widget's own `Interactive`.

**Widget:** `TabGroup::with_pane(id, pane, split_direction)` or
`TabGroup::from_layout(id, panes, layout, split_direction)`
(`State` = `TabGroupState`, `Action` = `TabGroupEvent`, delivered as
`UiEvent::TabGroup(id, event)`). The descriptor is the model: write each
intent back with `TabGroup::apply(event)`, which returns the changes it
made; `add_tab`, `switch_tab`, `close_tab`, `add_pane_after` and
`collapse_pane` change it directly. The layout
(`TabGroupPrimitiveLayout`) hands back each pane's `content_bounds`; the
host paints the bodies. A press in the body of the pane that already has
focus is the host's.

**Events (`TabGroupEvent`):** intents `TabActivated { pane_idx, tab_id }`,
`TabClosed { pane_idx, tab_id }`, `PaneFocused { pane_idx }`,
`DividerResized { divider_idx, ratio }`, `NewTabRequested { pane_idx }`,
`TabDropped { tab_id, instruction }`; changes `apply` reports besides
those: `PaneCollapsed { .. }`, `TabReordered { .. }`,
`TabMovedToPane { .. }`, `TabSplitToNewPane { .. }`.

**Smallest wiring:**

```rust,ignore
let mut group = TabGroup::with_pane(
    WidgetId::new("editors"),
    TabGroupPane::new("main", vec![TabGroupTab::new("t0", "main.rs")], "t0"),
    SplitDirection::Horizontal,
);
// render: paint the group, then each pane's body.
let painted = backend.draw_dyn(area, &group, &());
// frame_tree: route it.
tree.push(group.id, area, Box::new(group.clone()));
// handle:
if let UiEvent::TabGroup(_, intent) = event {
    for change in group.apply(intent) {
        if let TabGroupEvent::TabActivated { tab_id, .. } = change {
            activate(tab_id);
        }
    }
}
```

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

## `TreeNavCore` — `src/behaviour/nav.rs`

Not a compose module: it lives in `quadraui::behaviour` beside the other
shared interaction rules (`ensure_visible`, `FlatNav`, `Typeahead`), and is
listed here because `TreeView`'s `Interactive` and `TreeTableController` are
built on it.

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

**Example:** `quadraui-gallery/src/demos/tree_table.rs` — expand/collapse,
a header-click sort and the lazy `remote/` branch. Its `#[cfg(test)]`
module is the direct-use reference.

---

## `Workspace` — `src/compose/workspace.rs`

**Owns:** an **open-N-view-one** set of opaque document ids with exactly
one active, shown as a `TabBar` strip: open, close, activate, reorder,
preview tabs, click and keyboard routing. The host paints the active
document's body, which can borrow app state.

**Widget:** `Workspace::new(id)` with `.with_docs(..)`, `.with_active(..)`,
`.with_preview(..)` (`State` = `WorkspaceState`, `Action` =
`WorkspaceEvent`, delivered as `UiEvent::Workspace(id, event)`). Shape 1
above is a complete example. A press on the body area is the host's.

**Model:** `open(doc)`, `open_preview(doc)`, `close(id)`, `activate(id)`,
`reorder(from, to)`, `cycle(delta)`, `promote(id)` and `set_dirty(id, ..)`
return the `WorkspaceEvent`s they caused; `apply(intent)` writes back what
the strip reported. `Workspace::cycle_delta(&key, modifiers)` maps
`Ctrl+Tab`-style keys for a host that binds them app-wide.

**Actions (`WorkspaceEvent`):** `Opened { id, index }`,
`Activated { id, index }`, `Closed { id, index }`,
`Reordered { id, from, to }`, `Promoted { id }`.

**Smallest wiring:**

```rust,ignore
let mut ws = Workspace::new(WidgetId::new("panel:docs"));
ws.open(WorkspaceDoc::new("doc:a", "alpha.rs"));
// render:
let painted = backend.draw_dyn(panel_rect, &ws, &());
// frame_tree:
tree.push(ws.id, panel_rect, Box::new(ws.clone()));
// handle:
if let UiEvent::Workspace(_, intent) = event {
    for ev in ws.apply(intent) {
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
