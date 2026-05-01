//! `cargo run --example gtk_multi_tree --features gtk`
//!
//! GTK twin of [`msv_multi_tree`]. Same Debug-sidebar consumer
//! pattern (4 `EqualShare` `TreeView` sections with per-section
//! `scroll_offset` + `selected_path` owned by the host), translated
//! to a GTK4 substrate. Companion to issue #12 and a visual smoke
//! surface for the GTK MSV + TreeView harnesses (#3 + #4).
//!
//! The host:
//! - Owns per-section state in [`DebugSidebar`] (no `Cell<T>`
//!   bridges into the primitive).
//! - Routes clicks via [`gtk_msv_layout`] + [`gtk_tree_layout`].
//! - Updates only the targeted section's `scroll_offset` on
//!   scrollbar drag, clamped to `[0, rows.len() - viewport_rows]`
//!   (the natural-max contract from #9).
//! - Routes track-page clicks (`TrackBefore` → page up,
//!   `TrackAfter` → page down by `body_bounds.height` rows).
//!
//! Controls (mirror the TUI version):
//! - mouse click on header        activate that section
//! - mouse click on body row      activate + select
//! - mouse click on scrollbar     drag thumb / page via track
//! - `Tab` / `Shift+Tab`          cycle active section
//! - `↑` / `↓`                    scroll active section
//! - `Enter`                      select first row of active section
//! - `q` / `Esc`                  quit
//!
//! # Why bypass the runner
//!
//! `Backend` doesn't have `draw_multi_section_view` (same gap as
//! TUI's `quadraui::tui::run` — MSV isn't on the trait). This
//! example owns its own `gtk4::Application` + `DrawingArea` so it
//! can call the free `quadraui::gtk::draw_multi_section_view` with
//! the live `cairo::Context`. Mirrors how `msv_multi_tree.rs`
//! manages its own crossterm + ratatui Terminal.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::cairo::Context;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box as GtkBox, DrawingArea, EventControllerKey, GestureDrag,
    Label, Orientation,
};
use pangocairo::functions as pcfn;

use quadraui::gtk::{draw_multi_section_view, gtk_msv_layout, gtk_tree_layout};
use quadraui::{
    Decoration, MsvAxis, MultiSectionView, MultiSectionViewHit, Rect as QRect, ScrollMode,
    ScrollbarHit, Section, SectionBody, SectionHeader, SectionId, SectionSize, SelectionMode,
    StyledText, Theme, TreePath, TreeRow, TreeView, TreeViewHit, WidgetId,
};

/// Fixed line height for the example. The runner-driven flow resolves
/// this from Pango font metrics each frame; for a self-contained
/// example we lock a value so the click/drag handlers and the draw
/// callback agree without threading state through.
const LINE_HEIGHT: f64 = 18.0;

/// Per-section host state. Mirrors `msv_multi_tree`'s `TreeSection`.
struct TreeSection {
    id: SectionId,
    title: String,
    rows: Vec<TreeRow>,
    scroll_offset: usize,
    selected_path: Option<TreePath>,
}

/// Active drag captured on press over a scrollbar thumb.
struct ScrollDrag {
    section: usize,
    origin_y: f64,
    origin_offset: usize,
    viewport_rows: usize,
}

pub struct DebugSidebar {
    sections: Vec<TreeSection>,
    active_section: Option<usize>,
    scroll_drag: Option<ScrollDrag>,
    last_action: Option<ClickAction>,
}

impl DebugSidebar {
    pub fn new() -> Self {
        Self {
            sections: vec![
                tree_section("variables", "VARIABLES", &fake_rows("v", 12)),
                tree_section("watch", "WATCH", &fake_rows("w", 8)),
                tree_section("call-stack", "CALL STACK", &fake_rows("frame", 5)),
                tree_section("breakpoints", "BREAKPOINTS", &fake_rows("bp", 0)),
            ],
            active_section: None,
            scroll_drag: None,
            last_action: None,
        }
    }

    fn build_view(&self) -> MultiSectionView {
        let sections: Vec<Section> = self
            .sections
            .iter()
            .enumerate()
            .map(|(idx, s)| Section {
                id: s.id.clone(),
                header: SectionHeader {
                    title: StyledText::plain(s.title.clone()),
                    show_chevron: false,
                    ..Default::default()
                },
                body: SectionBody::Tree(TreeView {
                    id: WidgetId::new(format!("{}-tree", s.id)),
                    rows: s.rows.clone(),
                    selection_mode: SelectionMode::Single,
                    selected_path: s.selected_path.clone(),
                    scroll_offset: s.scroll_offset,
                    style: Default::default(),
                    has_focus: self.active_section == Some(idx),
                }),
                aux: None,
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            })
            .collect();
        MultiSectionView {
            id: WidgetId::new("debug-sidebar"),
            sections,
            active_section: self.active_section,
            axis: MsvAxis::Vertical,
            allow_resize: false,
            allow_collapse: false,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        }
    }

    /// Route a primary mouse-down at (x, y) inside the DrawingArea.
    pub fn click(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let view = self.build_view();
        let bounds = QRect::new(0.0, 0.0, w as f32, h as f32);
        let layout = gtk_msv_layout(&view, bounds, LINE_HEIGHT);
        let action = match layout.hit_test(x as f32, y as f32) {
            MultiSectionViewHit::Header { section, .. } => {
                self.active_section = Some(section);
                self.sections[section].selected_path = None;
                ClickAction::HeaderActivated(section)
            }
            MultiSectionViewHit::Body { section } => {
                self.active_section = Some(section);
                let body_b = layout.sections[section].body_bounds;
                let tree = match &view.sections[section].body {
                    SectionBody::Tree(t) => t.clone(),
                    _ => return,
                };
                let body_area = QRect::new(body_b.x, body_b.y, body_b.width, body_b.height);
                let inner = gtk_tree_layout(&tree, body_area, LINE_HEIGHT);
                match inner.hit_test(x as f32 - body_b.x, y as f32 - body_b.y) {
                    TreeViewHit::Row(idx) => {
                        let path = tree.rows[idx].path.clone();
                        self.sections[section].selected_path = Some(path.clone());
                        ClickAction::RowSelected { section, path }
                    }
                    TreeViewHit::Empty => ClickAction::BodyActivated(section),
                }
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::Thumb,
            } => {
                let viewport_rows = (layout.sections[section].body_bounds.height as f64
                    / (LINE_HEIGHT * 1.4))
                    .floor() as usize;
                self.scroll_drag = Some(ScrollDrag {
                    section,
                    origin_y: y,
                    origin_offset: self.sections[section].scroll_offset,
                    viewport_rows,
                });
                ClickAction::ScrollbarPressed(section)
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackBefore,
            } => {
                let viewport_rows = (layout.sections[section].body_bounds.height as f64
                    / (LINE_HEIGHT * 1.4))
                    .floor() as usize;
                self.page_scroll(section, -(viewport_rows as isize), viewport_rows);
                ClickAction::ScrollbarPagedUp(section)
            }
            MultiSectionViewHit::Scrollbar {
                section,
                kind: ScrollbarHit::TrackAfter,
            } => {
                let viewport_rows = (layout.sections[section].body_bounds.height as f64
                    / (LINE_HEIGHT * 1.4))
                    .floor() as usize;
                self.page_scroll(section, viewport_rows as isize, viewport_rows);
                ClickAction::ScrollbarPagedDown(section)
            }
            _ => ClickAction::None,
        };
        self.last_action = Some(action);
    }

    /// Apply mouse-move during an active scrollbar drag. 1 px of drag
    /// = 1 row of scroll, clamped to `[0, rows.len() - viewport_rows]`.
    pub fn drag_to(&mut self, y: f64) {
        let Some(drag) = &self.scroll_drag else {
            return;
        };
        let dy = y - drag.origin_y;
        // Convert pixel delta to row delta using the same row pitch
        // gtk_tree_layout uses (1.4 × line_height for normal rows).
        let drow = (dy / (LINE_HEIGHT * 1.4)).round() as i32;
        let row_count = self.sections[drag.section].rows.len();
        let max_offset = row_count.saturating_sub(drag.viewport_rows);
        let new = (drag.origin_offset as i32 + drow).max(0) as usize;
        self.sections[drag.section].scroll_offset = new.min(max_offset);
    }

    pub fn drag_end(&mut self) {
        self.scroll_drag = None;
    }

    fn page_scroll(&mut self, section: usize, delta: isize, viewport_rows: usize) {
        let row_count = self.sections[section].rows.len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.sections[section].scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.sections[section].scroll_offset = new;
    }

    pub fn cycle_active(&mut self, delta: isize) {
        let n = self.sections.len() as isize;
        if n == 0 {
            return;
        }
        let next = match self.active_section {
            Some(i) => ((i as isize + delta).rem_euclid(n)) as usize,
            None => {
                if delta >= 0 {
                    0
                } else {
                    (n - 1) as usize
                }
            }
        };
        self.active_section = Some(next);
    }

    pub fn scroll_active(&mut self, w: f64, h: f64, delta: isize) {
        let Some(idx) = self.active_section else {
            return;
        };
        let view = self.build_view();
        let bounds = QRect::new(0.0, 0.0, w as f32, h as f32);
        let layout = gtk_msv_layout(&view, bounds, LINE_HEIGHT);
        let viewport_rows =
            (layout.sections[idx].body_bounds.height as f64 / (LINE_HEIGHT * 1.4)).floor() as usize;
        let row_count = self.sections[idx].rows.len();
        let max = row_count.saturating_sub(viewport_rows) as isize;
        let cur = self.sections[idx].scroll_offset as isize;
        let new = (cur + delta).max(0).min(max) as usize;
        self.sections[idx].scroll_offset = new;
    }

    pub fn select_first_of_active(&mut self) {
        let Some(idx) = self.active_section else {
            return;
        };
        if let Some(first) = self.sections[idx].rows.first() {
            self.sections[idx].selected_path = Some(first.path.clone());
        }
    }

    pub fn status_text(&self) -> String {
        let active = match self.active_section {
            Some(i) => self.sections[i].id.clone(),
            None => "<none>".to_string(),
        };
        let action = match &self.last_action {
            Some(ClickAction::HeaderActivated(i)) => format!("header→{}", self.sections[*i].id),
            Some(ClickAction::BodyActivated(i)) => format!("body→{}", self.sections[*i].id),
            Some(ClickAction::RowSelected { section, path }) => {
                format!("row→{} {:?}", self.sections[*section].id, path)
            }
            Some(ClickAction::ScrollbarPressed(i)) => {
                format!("scrollbar→{}", self.sections[*i].id)
            }
            Some(ClickAction::ScrollbarPagedUp(i)) => format!("page-up→{}", self.sections[*i].id),
            Some(ClickAction::ScrollbarPagedDown(i)) => {
                format!("page-down→{}", self.sections[*i].id)
            }
            Some(ClickAction::None) => "inert".to_string(),
            None => "—".to_string(),
        };
        format!("active: {active}  last: {action}  (mouse, Tab/↑↓/Enter, q quit)")
    }
}

impl Default for DebugSidebar {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClickAction {
    HeaderActivated(usize),
    BodyActivated(usize),
    RowSelected { section: usize, path: TreePath },
    ScrollbarPressed(usize),
    ScrollbarPagedUp(usize),
    ScrollbarPagedDown(usize),
    None,
}

fn tree_section(id: &str, title: &str, rows: &[TreeRow]) -> TreeSection {
    TreeSection {
        id: id.to_string(),
        title: title.to_string(),
        rows: rows.to_vec(),
        scroll_offset: 0,
        selected_path: None,
    }
}

fn fake_rows(prefix: &str, n: usize) -> Vec<TreeRow> {
    (0..n)
        .map(|i| TreeRow {
            path: vec![i as u16],
            indent: 0,
            icon: None,
            text: StyledText::plain(format!("{prefix}{i}")),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
        })
        .collect()
}

// ── GTK runner ─────────────────────────────────────────────────────────────

fn main() -> glib::ExitCode {
    let app = Application::builder()
        .application_id("dev.quadraui.gtk_multi_tree")
        .build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .title("MSV Multi-Tree (GTK)")
        .default_width(420)
        .default_height(640)
        .build();

    let vbox = GtkBox::new(Orientation::Vertical, 0);
    let da = DrawingArea::new();
    da.set_hexpand(true);
    da.set_vexpand(true);

    let status = Label::builder()
        .label("active: <none>  last: —")
        .halign(gtk4::Align::Start)
        .margin_start(8)
        .margin_end(8)
        .margin_top(2)
        .margin_bottom(2)
        .build();

    vbox.append(&da);
    vbox.append(&status);
    window.set_child(Some(&vbox));

    let sidebar = Rc::new(RefCell::new(DebugSidebar::new()));
    let theme = Rc::new(Theme::default());

    // Draw callback — calls the free draw_multi_section_view with
    // the live cairo Context, since Backend trait doesn't expose MSV.
    {
        let sidebar = sidebar.clone();
        let theme = theme.clone();
        da.set_draw_func(move |_da, cr, w, h| {
            paint(cr, w, h, &sidebar.borrow(), &theme);
        });
    }

    // Click + drag — GestureDrag fires drag_begin/update/end and
    // also click-released even on no-drag clicks. We use it for both
    // initial click routing and drag tracking.
    let drag = GestureDrag::builder().button(1).build();
    {
        let sidebar = sidebar.clone();
        let da = da.clone();
        let status = status.clone();
        drag.connect_drag_begin(move |_, x, y| {
            let w = da.width() as f64;
            let h = da.height() as f64;
            sidebar.borrow_mut().click(x, y, w, h);
            status.set_label(&sidebar.borrow().status_text());
            da.queue_draw();
        });
    }
    {
        let sidebar = sidebar.clone();
        let da = da.clone();
        drag.connect_drag_update(move |gesture, _ox, _oy| {
            // GestureDrag fires drag_update with offsets from the
            // begin point. Convert back to absolute y by adding
            // start_point.
            if let Some((sx, sy)) = gesture.start_point() {
                let (ox, oy) = gesture.offset().unwrap_or((0.0, 0.0));
                let abs_y = sy + oy;
                let _ = sx;
                let _ = ox;
                sidebar.borrow_mut().drag_to(abs_y);
                da.queue_draw();
            }
        });
    }
    {
        let sidebar = sidebar.clone();
        drag.connect_drag_end(move |_, _, _| {
            sidebar.borrow_mut().drag_end();
        });
    }
    da.add_controller(drag);

    // Keyboard — mirror TUI's bindings. Tab/BackTab cycle, Up/Down
    // scroll, Enter selects first row of active, q/Esc quits.
    let key_ctrl = EventControllerKey::new();
    {
        let sidebar = sidebar.clone();
        let da = da.clone();
        let status = status.clone();
        let window = window.clone();
        key_ctrl.connect_key_pressed(move |_, key, _, modifier| {
            use gtk4::gdk::Key;
            let shift = modifier.contains(gtk4::gdk::ModifierType::SHIFT_MASK);
            let w = da.width() as f64;
            let h = da.height() as f64;
            match key {
                Key::q | Key::Q | Key::Escape => {
                    window.close();
                    return glib::Propagation::Stop;
                }
                Key::Tab if !shift => sidebar.borrow_mut().cycle_active(1),
                Key::Tab if shift => sidebar.borrow_mut().cycle_active(-1),
                Key::ISO_Left_Tab => sidebar.borrow_mut().cycle_active(-1),
                Key::Up => sidebar.borrow_mut().scroll_active(w, h, -1),
                Key::Down => sidebar.borrow_mut().scroll_active(w, h, 1),
                Key::Return | Key::KP_Enter => sidebar.borrow_mut().select_first_of_active(),
                _ => return glib::Propagation::Proceed,
            }
            status.set_label(&sidebar.borrow().status_text());
            da.queue_draw();
            glib::Propagation::Stop
        });
    }
    window.add_controller(key_ctrl);

    window.present();
}

fn paint(cr: &Context, w: i32, h: i32, sidebar: &DebugSidebar, theme: &Theme) {
    let pango_ctx = pcfn::create_context(cr);
    let pango_layout = pango::Layout::new(&pango_ctx);
    let font = pango::FontDescription::from_string("Sans 11");
    pango_layout.set_font_description(Some(&font));
    pango_layout.set_width(-1);

    // Clear with theme background.
    let bg_r = theme.background.r as f64 / 255.0;
    let bg_g = theme.background.g as f64 / 255.0;
    let bg_b = theme.background.b as f64 / 255.0;
    cr.set_source_rgb(bg_r, bg_g, bg_b);
    cr.paint().ok();

    let view = sidebar.build_view();
    draw_multi_section_view(
        cr,
        &pango_layout,
        0.0,
        0.0,
        w as f64,
        h as f64,
        &view,
        theme,
        LINE_HEIGHT,
        /* nerd_fonts */ false,
    );
}
