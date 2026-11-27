//! Backend-agnostic app demonstrating [`TabGroup`] on the widget contract.
//!
//! Two panes side-by-side (horizontal split), each with its own tab bar.
//! The group paints as a widget and is registered in the app's
//! `FrameTree`, so the runtime routes every press, drag and release on it
//! through `TabGroup`'s own `Interactive`; this app applies each
//! `UiEvent::TabGroup` intent with [`TabGroup::apply`] and paints the
//! active tab's body into each pane itself. Demonstrates:
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
//! - **Ctrl-W** — close the active tab in the focused pane (keyboard
//!   equivalent of clicking `×`).
//! - **`f`** — rebuild via `from_layout`: 3 panes in a mixed H/V tree
//!   (`left | (top-right / bottom-right)`).
//! - **`r`** — reset to the default 2-pane horizontal layout.
//! - `q` / Esc to quit (Esc first cancels a tab drag in progress).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use quadraui::compose::tab_group::{
    GroupLayout, TabGroup, TabGroupEvent, TabGroupPane, TabGroupPrimitiveLayout, TabGroupState,
    TabGroupTab,
};
use quadraui::{
    AppLogic, Backend, Color, DrawsPrimitives, FrameTree, Key, Modifiers, NamedKey, Point,
    Reaction, Rect, SplitDirection, StatusBar, StatusBarSegment, UiEvent, WidgetId,
    DROP_OVERLAY_ZONE_ID,
};

/// The group's widget id.
const GROUP_ID: &str = "tg-demo:group";

// ── Tab bodies ────────────────────────────────────────────────────────────────

/// What a tab's body shows: a labelled bar in its own colour.
#[derive(Clone)]
struct Body {
    text: String,
    bg: Color,
}

impl Body {
    fn paint(&self, backend: &mut dyn Backend, rect: Rect) {
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
        backend.draw_dyn(Rect::new(rect.x, rect.y, rect.width, lh), &bar, &());
    }
}

/// A closable tab plus the body it shows.
fn tab(id: &str, label: &str, body: &str, bg: Color) -> (TabGroupTab, Body) {
    (
        TabGroupTab::new(id, label),
        Body {
            text: body.to_string(),
            bg,
        },
    )
}

/// A pane of `tabs`, recording each tab's body in `bodies`.
fn pane(
    id: &str,
    tabs: Vec<(TabGroupTab, Body)>,
    active: &str,
    bodies: &mut HashMap<String, Body>,
) -> TabGroupPane {
    let tabs = tabs
        .into_iter()
        .map(|(tab, body)| {
            bodies.insert(tab.id.clone(), body);
            tab
        })
        .collect();
    TabGroupPane::new(id, tabs, active)
}

// ── Layout builders ───────────────────────────────────────────────────────────

/// Default layout: two panes side-by-side (horizontal), built
/// incrementally with `with_pane` / `add_pane_after`.
fn make_default_group(bodies: &mut HashMap<String, Body>) -> TabGroup {
    let first = pane(
        "pane:0",
        vec![
            tab(
                "p0:t0",
                " main.rs ",
                "main.rs content",
                Color::rgb(30, 40, 60),
            ),
            tab(
                "p0:t1",
                " lib.rs ",
                "lib.rs content",
                Color::rgb(40, 30, 60),
            ),
        ],
        "p0:t0",
        bodies,
    );
    let mut group = TabGroup::with_pane(GROUP_ID, first, SplitDirection::Horizontal);
    let second = pane(
        "pane:1",
        vec![tab(
            "p1:t0",
            " Cargo.toml ",
            "Cargo.toml content",
            Color::rgb(30, 60, 40),
        )],
        "p1:t0",
        bodies,
    );
    group.add_pane_after(0, second);
    group
}

/// Mixed-direction layout built via `from_layout`:
///
/// ```text
///  ┌──────────┬──────────┐
///  │          │  top.rs  │
///  │  left.rs ├──────────┤
///  │          │bottom.rs │
///  └──────────┴──────────┘
/// ```
///
/// Root split is Horizontal (left | right); the right sub-tree is Vertical
/// (top-right / bottom-right).
fn make_from_layout_group(bodies: &mut HashMap<String, Body>) -> TabGroup {
    let layout = GroupLayout::Split {
        direction: SplitDirection::Horizontal,
        ratio: 0.5,
        first: Box::new(GroupLayout::Leaf(0)),
        second: Box::new(GroupLayout::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(GroupLayout::Leaf(1)),
            second: Box::new(GroupLayout::Leaf(2)),
        }),
    };
    let panes = vec![
        pane(
            "pane:left",
            vec![
                tab(
                    "left:t0",
                    " left.rs ",
                    "left pane (pane 0)",
                    Color::rgb(30, 40, 60),
                ),
                tab(
                    "left:t1",
                    " app.rs ",
                    "app.rs — left pane tab 2",
                    Color::rgb(20, 50, 70),
                ),
            ],
            "left:t0",
            bodies,
        ),
        pane(
            "pane:top-right",
            vec![tab(
                "tr:t0",
                " top.rs ",
                "top-right pane (pane 1)",
                Color::rgb(60, 30, 40),
            )],
            "tr:t0",
            bodies,
        ),
        pane(
            "pane:bottom-right",
            vec![tab(
                "br:t0",
                " bottom.rs ",
                "bottom-right pane (pane 2)",
                Color::rgb(30, 60, 30),
            )],
            "br:t0",
            bodies,
        ),
    ];
    TabGroup::from_layout(GROUP_ID, panes, layout, SplitDirection::Horizontal)
        .expect("from_layout: layout is structurally valid")
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct TabGroupDemo {
    group: TabGroup,
    /// Every tab's body, by tab id.
    bodies: HashMap<String, Body>,
    /// The pane keyboard commands (Ctrl-W) address.
    focused: Option<usize>,
    last_event: String,
    /// The rect the group painted into last frame, which `frame_tree`
    /// routes against.
    area: Cell<Rect>,
    /// The group's layout from the last frame, for a body press the group
    /// leaves to this app.
    painted: RefCell<Option<TabGroupPrimitiveLayout>>,
    /// Counter used to make new-tab IDs unique.
    next_tab_id: usize,
}

impl TabGroupDemo {
    pub fn new() -> Self {
        let mut bodies = HashMap::new();
        let group = make_default_group(&mut bodies);
        Self {
            group,
            bodies,
            focused: Some(0),
            last_event:
                "click tabs · drag divider · drag tab to edge/pane · Tab=focus · Ctrl-W=close tab · f=from_layout · r=reset · q=quit"
                    .into(),
            area: Cell::new(Rect::new(0.0, 0.0, 100.0, 20.0)),
            painted: RefCell::new(None),
            next_tab_id: 10,
        }
    }

    /// Swap in a freshly built group.
    fn rebuild(&mut self, build: fn(&mut HashMap<String, Body>) -> TabGroup, note: &str) {
        self.bodies.clear();
        self.group = build(&mut self.bodies);
        self.focused = Some(0);
        self.last_event = note.into();
    }

    /// Keep focus on a real pane after the pane count changed.
    fn clamp_focus(&mut self) {
        let n = self.group.pane_count();
        if let Some(f) = self.focused {
            if f >= n {
                self.focused = n.checked_sub(1);
            }
        }
    }

    /// React to one change the group made (or a request it reported).
    fn react(&mut self, ev: &TabGroupEvent) {
        match ev {
            TabGroupEvent::PaneFocused { pane_idx } => self.focused = Some(*pane_idx),
            TabGroupEvent::TabSplitToNewPane { new_pane_idx, .. } => {
                self.focused = Some(*new_pane_idx)
            }
            TabGroupEvent::NewTabRequested { pane_idx } => {
                self.next_tab_id += 1;
                let id = format!("dyn:{}", self.next_tab_id);
                let label = format!(" untitled-{} ", self.next_tab_id);
                self.bodies.insert(
                    id.clone(),
                    Body {
                        text: "(new tab)".into(),
                        bg: Color::rgb(50, 50, 50),
                    },
                );
                self.group
                    .add_tab(*pane_idx, TabGroupTab::new(id.as_str(), label));
                self.group.switch_tab(*pane_idx, &id);
            }
            _ => {}
        }
        self.clamp_focus();
    }

    fn report(&mut self, events: &[TabGroupEvent]) -> Reaction {
        let Some(primary) = events.first() else {
            return Reaction::Redraw;
        };
        self.last_event = format_event(primary);
        for ev in events {
            self.react(ev);
        }
        Reaction::Redraw
    }

    fn cycle_focus(&mut self, delta: isize) {
        let n = self.group.pane_count();
        if n == 0 {
            return;
        }
        self.focused = Some(match self.focused {
            Some(i) => quadraui::focus::cycle_index(i, delta, n),
            None if delta >= 0 => 0,
            None => n - 1,
        });
    }

    fn focus_label(&self) -> String {
        self.focused
            .map(|i| i.to_string())
            .unwrap_or_else(|| "—".into())
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
        self.area.set(content_rect);

        let layout = backend
            .draw_dyn(content_rect, &self.group, &())
            .downcast_ref::<TabGroupPrimitiveLayout>()
            .cloned();
        if let Some(layout) = &layout {
            // Each pane's active tab paints its own body.
            for (pane, pane_layout) in self.group.panes().iter().zip(&layout.panes) {
                let content = pane_layout.content_bounds;
                if content.width > 0.0 && content.height > 0.0 {
                    if let Some(body) = self.bodies.get(pane.active_tab_id()) {
                        body.paint(backend, content);
                    }
                }
            }
            // The drop overlay paints no text: register its zone so the
            // frame inventory attributes it while a tab drag hovers a
            // drop zone.
            let overlay = quadraui::with_widget_states(|store| {
                store
                    .get(&self.group.id)
                    .and_then(|state| state.downcast_ref::<TabGroupState>())
                    .and_then(|state| self.group.drop_overlay(state, layout))
            });
            if let Some(overlay) = overlay {
                backend.register_zone(WidgetId::new(DROP_OVERLAY_ZONE_ID), overlay.bounds());
            }
        }
        *self.painted.borrow_mut() = layout;

        let status = StatusBar {
            id: WidgetId::new("tg-demo:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_event),
                fg: Color::rgb(200, 200, 200),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: format!(
                    " focus: {} | panes: {} ",
                    self.focus_label(),
                    self.group.pane_count()
                ),
                fg: Color::rgb(180, 180, 180),
                bg: Color::rgb(40, 40, 60),
                bold: false,
                action_id: None,
            }],
        };
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        backend.draw_dyn(status_rect, &status, &());
    }

    fn frame_tree(&self, _area: ()) -> FrameTree {
        let mut tree = FrameTree::new();
        tree.push(self.group.id, self.area.get(), Box::new(self.group.clone()));
        tree
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            // ── The group's own intents ────────────────────────────
            UiEvent::TabGroup(_, intent) => {
                let events = self.group.apply(intent);
                self.report(&events)
            }

            // ── Quit ───────────────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Char('q') | Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,

            // ── from_layout demo (f) / reset (r) ──────────────────
            UiEvent::KeyPressed {
                key: Key::Char('f'),
                ..
            } => {
                self.rebuild(
                    make_from_layout_group,
                    "from_layout: left | (top-right / bottom-right) — drag/click/Tab to verify",
                );
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('r'),
                ..
            } => {
                self.rebuild(
                    make_default_group,
                    "reset to default 2-pane layout (with_pane API)",
                );
                Reaction::Redraw
            }

            // ── Close active tab (Ctrl-W) ──────────────────────────
            UiEvent::KeyPressed {
                key: Key::Char('w'),
                modifiers: Modifiers { ctrl: true, .. },
                ..
            } => {
                let target = self.focused.and_then(|idx| {
                    self.group
                        .pane(idx)
                        .map(|p| (idx, p.active_tab_id().to_string()))
                });
                match target.and_then(|(pane_idx, tab_id)| self.group.close_tab(pane_idx, &tab_id))
                {
                    Some(ev) => self.report(&[ev]),
                    None => Reaction::Continue,
                }
            }

            // ── Focus cycling ──────────────────────────────────────
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Tab),
                modifiers,
                ..
            } => {
                let back = modifiers.shift;
                self.cycle_focus(if back { -1 } else { 1 });
                self.last_event = format!(
                    "{} → focus {}",
                    if back { "Shift+Tab" } else { "Tab" },
                    self.focus_label()
                );
                Reaction::Redraw
            }

            // ── A press the group left alone: a body of the pane that
            //    already holds the group's focus ────────────────────
            UiEvent::MouseDown { position, .. } => {
                let pane = self
                    .painted
                    .borrow()
                    .as_ref()
                    .and_then(|l| l.pane_at(Point::new(position.x, position.y)));
                match pane {
                    Some(k) if self.focused != Some(k) => {
                        self.focused = Some(k);
                        self.last_event = format_event(&TabGroupEvent::PaneFocused { pane_idx: k });
                        Reaction::Redraw
                    }
                    _ => Reaction::Continue,
                }
            }

            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
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
        TabGroupEvent::DividerResized { divider_idx, .. } => {
            format!("divider {divider_idx} resized")
        }
        TabGroupEvent::TabDropped { tab_id, .. } => format!("tab {tab_id} dropped"),
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
