//! Backend-agnostic app demonstrating [`TabGroupController`].
//!
//! Two panes side-by-side (horizontal split), each with its own tab bar.
//! Demonstrates:
//!
//! - Click a tab to activate it.
//! - Click `×` on a closable tab to close it; closing the last tab in a pane
//!   collapses the pane.
//! - Click `+` (right segment) to add a new tab.
//! - Click inside the content area to focus that pane.
//! - Drag the divider to resize the two panes.
//! - **Drag a tab into another pane** to merge it there.
//! - **Drag a tab to a pane edge** to split it into a new pane.
//!   `Left`/`Right` edges create a horizontal split;
//!   `Top`/`Bottom` edges create a vertical split.
//! - Tab / Shift+Tab to cycle focus between panes.
//! - `q` / Esc to quit.

use std::cell::RefCell;

use quadraui::compose::tab_group::{PaneTab, TabGroupController, TabGroupEvent};
use quadraui::{
    AppLogic, Backend, Color, Key, Modifiers, NamedKey, Reaction, Rect, SplitDirection, StatusBar,
    StatusBarSegment, UiEvent, WidgetId,
};

// ── Content widgets ───────────────────────────────────────────────────────────

/// A content widget that fills its area with a labelled status bar.
pub struct LabelContent {
    pub text: String,
    pub bg: Color,
}

impl quadraui::BackendWidget for LabelContent {
    fn render(&self, backend: &mut dyn Backend, rect: Rect) {
        if rect.height < 1.0 {
            return;
        }
        let lh = backend.line_height();
        let bar = StatusBar {
            id: WidgetId::new("tg-demo:content"),
            left_segments: vec![StatusBarSegment {
                text: format!("  {} ", self.text),
                fg: Color::rgb(220, 220, 220),
                bg: self.bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        };
        backend.draw_status_bar(Rect::new(rect.x, rect.y, rect.width, lh), &bar, None, None);
    }
}

fn lbl(text: &str, bg: Color) -> Box<LabelContent> {
    Box::new(LabelContent {
        text: text.to_string(),
        bg,
    })
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct TabGroupDemo {
    /// Wrapped in RefCell so `render(&self, …)` can call `group.render(…)`.
    group: RefCell<TabGroupController>,
    last_event: RefCell<String>,
    last_bounds: RefCell<Rect>,
    /// True while a divider drag is in progress.
    divider_dragging: RefCell<bool>,
    /// True while a tab drag is in progress.
    tab_dragging: RefCell<bool>,
    /// Counter used to make new-tab IDs unique.
    next_tab_id: RefCell<usize>,
}

impl TabGroupDemo {
    pub fn new() -> Self {
        let mut group = TabGroupController::with_pane(
            "pane:0",
            vec![
                PaneTab {
                    id: "p0:t0".into(),
                    label: " main.rs ".into(),
                    closable: true,
                    content: lbl("main.rs content", Color::rgb(30, 40, 60)),
                },
                PaneTab {
                    id: "p0:t1".into(),
                    label: " lib.rs ".into(),
                    closable: true,
                    content: lbl("lib.rs content", Color::rgb(40, 30, 60)),
                },
            ],
            "p0:t0",
            SplitDirection::Horizontal,
        );

        group.add_pane_with_tab(
            "pane:1",
            PaneTab {
                id: "p1:t0".into(),
                label: " Cargo.toml ".into(),
                closable: true,
                content: lbl("Cargo.toml content", Color::rgb(30, 60, 40)),
            },
        );
        group.focus_pane(0);

        Self {
            group: RefCell::new(group),
            last_event: RefCell::new(
                "click tabs | drag divider | drag tab to edge/pane | Tab=focus | q=quit".into(),
            ),
            last_bounds: RefCell::new(Rect::new(0.0, 0.0, 100.0, 20.0)),
            divider_dragging: RefCell::new(false),
            tab_dragging: RefCell::new(false),
            next_tab_id: RefCell::new(10),
        }
    }
}

impl Default for TabGroupDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for TabGroupDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        // Reserve bottom row for status bar.
        let content_rect = Rect::new(0.0, 0.0, viewport.width, (viewport.height - lh).max(0.0));
        *self.last_bounds.borrow_mut() = content_rect;

        // Render the tab group (also draws the drop overlay when tab-dragging).
        self.group.borrow_mut().render(backend, content_rect);

        let group = self.group.borrow();

        // Status bar.
        let status = StatusBar {
            id: WidgetId::new("tg-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_event.borrow()),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(
                    " focus: {} | panes: {} ",
                    group
                        .focused_pane()
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "—".into()),
                    group.pane_count()
                ),
                fg: Color::rgb(180, 180, 180),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
        };
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        backend.draw_status_bar(status_rect, &status, None, None);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            // ── Quit ───────────────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => {
                // Cancel any active tab drag on Escape.
                self.group.borrow_mut().cancel_tab_drag();
                *self.tab_dragging.borrow_mut() = false;
                Reaction::Exit
            }

            // ── Focus cycling ──────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                modifiers: Modifiers { shift: true, .. },
                ..
            } => {
                self.group.borrow_mut().cycle_focus(-1);
                *self.last_event.borrow_mut() = format!(
                    "Shift+Tab → focus {}",
                    self.group
                        .borrow()
                        .focused_pane()
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "—".into())
                );
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                ..
            } => {
                self.group.borrow_mut().cycle_focus(1);
                *self.last_event.borrow_mut() = format!(
                    "Tab → focus {}",
                    self.group
                        .borrow()
                        .focused_pane()
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "—".into())
                );
                Reaction::Redraw
            }

            // ── Mouse down: try tab drag first, then divider, then click ──
            UiEvent::MouseDown { position, .. } => {
                // 1. Try to start a tab drag.
                if self
                    .group
                    .borrow_mut()
                    .handle_tab_drag_start(position.x, position.y)
                {
                    *self.tab_dragging.borrow_mut() = true;
                    *self.last_event.borrow_mut() = "tab drag start".into();
                    return Reaction::Redraw;
                }

                // 2. Try to start a divider drag.
                if self
                    .group
                    .borrow_mut()
                    .handle_drag_start(position.x, position.y)
                {
                    *self.divider_dragging.borrow_mut() = true;
                    *self.last_event.borrow_mut() = "divider drag start".into();
                    return Reaction::Redraw;
                }

                // 3. Otherwise treat as a click.
                if let Some(ev) = self.group.borrow_mut().handle_click(position.x, position.y) {
                    *self.last_event.borrow_mut() = format_event(&ev);
                    self.handle_tab_group_event(ev);
                }
                Reaction::Redraw
            }

            // ── Mouse moved ────────────────────────────────────────
            UiEvent::MouseMoved { position, .. } => {
                if *self.tab_dragging.borrow() {
                    self.group
                        .borrow_mut()
                        .handle_tab_drag_move(position.x, position.y);
                    return Reaction::Redraw;
                }
                if *self.divider_dragging.borrow() {
                    let bounds = *self.last_bounds.borrow();
                    if let Some(ev) = self
                        .group
                        .borrow_mut()
                        .handle_drag_move(position.x, position.y, bounds)
                    {
                        *self.last_event.borrow_mut() = format_event(&ev);
                        return Reaction::Redraw;
                    }
                }
                Reaction::Continue
            }

            // ── Mouse up ───────────────────────────────────────────
            UiEvent::MouseUp { position, .. } => {
                if *self.tab_dragging.borrow() {
                    *self.tab_dragging.borrow_mut() = false;
                    let evs = self
                        .group
                        .borrow_mut()
                        .handle_tab_drop(position.x, position.y);
                    if evs.is_empty() {
                        // No mutation occurred (drop on source pane's own content,
                        // same-position drop, etc.). The most common cause is a
                        // simple click on a tab body — `MouseDown` started a drag
                        // because the cursor was on a tab, but the cursor never
                        // moved so `handle_tab_drop` resolved to a no-op slot.
                        // Cancel the drag and fall through to `handle_click` so
                        // tab activation still works for click-with-no-movement.
                        self.group.borrow_mut().cancel_tab_drag();
                        if let Some(ev) =
                            self.group.borrow_mut().handle_click(position.x, position.y)
                        {
                            *self.last_event.borrow_mut() = format_event(&ev);
                            self.handle_tab_group_event(ev);
                        } else {
                            *self.last_event.borrow_mut() = "tab drag cancelled".into();
                        }
                    } else {
                        // Show the primary event in the status line; dispatch each.
                        if let Some(primary) = evs.first() {
                            *self.last_event.borrow_mut() = format_event(primary);
                        }
                        for ev in evs {
                            self.handle_tab_group_event(ev);
                        }
                    }
                    return Reaction::Redraw;
                }
                if *self.divider_dragging.borrow() {
                    *self.divider_dragging.borrow_mut() = false;
                    self.group.borrow_mut().handle_drag_end();
                }
                Reaction::Continue
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}

impl TabGroupDemo {
    fn handle_tab_group_event(&self, ev: TabGroupEvent) {
        if let TabGroupEvent::NewTabRequested { pane_idx } = ev {
            let id_num = {
                let mut n = self.next_tab_id.borrow_mut();
                *n += 1;
                *n
            };
            let id = format!("dyn:{}", id_num);
            let label = format!(" untitled-{} ", id_num);
            self.group.borrow_mut().add_and_activate_tab(
                pane_idx,
                PaneTab {
                    id,
                    label,
                    closable: true,
                    content: lbl("(new tab)", Color::rgb(50, 50, 50)),
                },
            );
        }
    }
}

fn format_event(ev: &TabGroupEvent) -> String {
    match ev {
        TabGroupEvent::TabActivated { pane_idx, tab_id } => {
            format!("activated tab {tab_id} in pane {pane_idx}")
        }
        TabGroupEvent::TabClosed { pane_idx, tab_id } => {
            format!("closed tab {tab_id} in pane {pane_idx}")
        }
        TabGroupEvent::PaneCollapsed { pane_idx } => {
            format!("pane {pane_idx} collapsed (last tab closed)")
        }
        TabGroupEvent::PaneFocused { pane_idx } => format!("pane {pane_idx} focused"),
        TabGroupEvent::PaneAdded { pane_idx } => format!("pane {pane_idx} added"),
        TabGroupEvent::DividerResized { divider_idx } => {
            format!("divider {divider_idx} resized")
        }
        TabGroupEvent::NewTabRequested { pane_idx } => {
            format!("new tab requested in pane {pane_idx}")
        }
        TabGroupEvent::TabReordered {
            pane_idx,
            tab_id,
            from_idx,
            to_idx,
        } => format!("tab {tab_id} reordered in pane {pane_idx}: {from_idx} → {to_idx}"),
        TabGroupEvent::TabMovedToPane {
            from_pane_idx,
            to_pane_idx,
            tab_id,
            insert_idx,
        } => format!(
            "tab {tab_id} moved from pane {from_pane_idx} to pane {to_pane_idx} at {insert_idx}"
        ),
        TabGroupEvent::TabSplitToNewPane {
            from_pane_idx,
            tab_id,
            edge,
            new_pane_idx,
            ..
        } => format!(
            "tab {tab_id} split from pane {from_pane_idx} to new pane {new_pane_idx} ({edge:?})"
        ),
    }
}
