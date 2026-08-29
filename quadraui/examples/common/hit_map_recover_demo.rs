//! Demo for `ScreenLayout::hit_map()` (#425).
//!
//! `render()` paints the TabBar, ListView, and StatusBar with direct
//! `backend.draw_*()` calls — three independent call sites, standing in
//! for an app whose real paint pass interleaves `Surface` painting with
//! other non-`Surface` rendering (the vimcode GTK `render_content` shape
//! that motivated #425). Only *after* all three are on screen does the
//! demo build a `ScreenLayout`, push the same objects it just painted,
//! and call [`quadraui::ScreenLayout::hit_map`] — never `.draw()` — to
//! recover a `FrameHitMap` for click dispatch.
//!
//! `hit_map()` makes no `backend.draw_*()` calls, so pushing surfaces
//! into it can never reorder or repeat the real painting above. Compare
//! with `FrameDemo` (`frame_demo.rs`), which paints *and* hit-maps in a
//! single `ScreenLayout::draw()` batch — the pattern #425 exists to make
//! optional.

use std::cell::RefCell;

use quadraui::{
    AppLogic, Backend, Color, FrameHitMap, FrameZone, Key, ListItem, ListView, NamedKey, Reaction,
    Rect, ScreenLayout, StatusBar, StatusBarSegment, StyledText, Surface, TabBar, TabItem, UiEvent,
    WidgetId,
};

pub struct HitMapRecoverDemo {
    active_tab: usize,
    items: Vec<String>,
    selected: usize,
    last_hit: String,
    cached_hit_map: RefCell<FrameHitMap>,
}

impl HitMapRecoverDemo {
    pub fn new() -> Self {
        Self {
            active_tab: 0,
            items: vec![
                "Pods".into(),
                "Deployments".into(),
                "Services".into(),
                "ConfigMaps".into(),
                "Secrets".into(),
            ],
            selected: 0,
            last_hit: "—".into(),
            cached_hit_map: RefCell::new(FrameHitMap::default()),
        }
    }

    fn rects(&self, backend: &dyn Backend) -> (Rect, Rect, Rect) {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let tab_h = (lh * 1.5).round();
        let status_h = (lh * 1.5).round();
        let list_h = (vp.height - tab_h - status_h).max(0.0);
        (
            Rect::new(0.0, 0.0, vp.width, tab_h),
            Rect::new(0.0, tab_h, vp.width, list_h),
            Rect::new(0.0, tab_h + list_h, vp.width, status_h),
        )
    }

    fn tab_bar(&self) -> TabBar {
        TabBar {
            id: WidgetId::new("tabs"),
            tabs: ["Resources", "YAML", "Events"]
                .iter()
                .enumerate()
                .map(|(i, &label)| TabItem {
                    label: format!(" {label} "),
                    is_active: i == self.active_tab,
                    is_dirty: false,
                    is_preview: false,
                    is_closable: false,
                    icon: None,
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: Some(Color::rgb(80, 160, 240)),
            show_tab_close: false,
            compact: false,
        }
    }

    fn list_view(&self) -> ListView {
        ListView {
            id: WidgetId::new("list"),
            title: None,
            items: self
                .items
                .iter()
                .map(|name| ListItem {
                    text: StyledText::plain(name),
                    detail: None,
                    icon: None,
                    decoration: Default::default(),
                })
                .collect(),
            selected_idx: self.selected,
            scroll_offset: 0,
            has_focus: true,
            bordered: false,
            h_scroll: 0,
            max_content_width: None,
            show_v_scrollbar: false,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " tab:{} sel:{} last-hit:{} ",
                    self.active_tab, self.selected, self.last_hit
                ),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " via hit_map(), not draw() | q=quit ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for HitMapRecoverDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for HitMapRecoverDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let (tab_rect, list_rect, status_rect) = self.rects(backend);
        let tab_bar = self.tab_bar();
        let list = self.list_view();
        let status = self.status_bar();

        // ── Real paint pass: three independent call sites. ──────────
        // Nothing here goes through `ScreenLayout` — this is the frame
        // an app would already be producing before adopting #425.
        backend.draw_tab_bar(tab_rect, &tab_bar, None);
        backend.draw_list(list_rect, &list);
        backend.draw_status_bar(status_rect, &status, None, None);

        // ── Hit-map recovery: zero additional backend calls. ────────
        // Push the *same* objects just painted above into a fresh
        // `ScreenLayout`, purely to recover a `FrameHitMap`. `hit_map()`
        // never touches `backend`, so this can run after — or be
        // reordered relative to — any of the paint calls above with no
        // risk of re-rendering or reordering real output.
        let mut frame = ScreenLayout::new();
        frame.push(Surface::TabBar {
            rect: tab_rect,
            bar: &tab_bar,
            hovered_close: None,
        });
        frame.push(Surface::List {
            rect: list_rect,
            list: &list,
        });
        frame.push(Surface::StatusBar {
            rect: status_rect,
            bar: &status,
            hovered: None,
            pressed: None,
        });
        *self.cached_hit_map.borrow_mut() = frame.hit_map();
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        match &event {
            UiEvent::KeyPressed { key, .. } => match key {
                Key::Char('q') | Key::Named(NamedKey::Escape) => return Reaction::Exit,
                Key::Char('j') | Key::Named(NamedKey::Down) => {
                    if self.selected + 1 < self.items.len() {
                        self.selected += 1;
                    }
                    return Reaction::Redraw;
                }
                Key::Char('k') | Key::Named(NamedKey::Up) => {
                    self.selected = self.selected.saturating_sub(1);
                    return Reaction::Redraw;
                }
                Key::Named(NamedKey::Tab) => {
                    self.active_tab = (self.active_tab + 1) % 3;
                    return Reaction::Redraw;
                }
                _ => {}
            },
            UiEvent::MouseDown { position, .. } => {
                let (_, list_rect, _) = self.rects(backend);
                let hit_map = self.cached_hit_map.borrow();
                let zone = hit_map.hit_test(position.x, position.y);
                match zone {
                    FrameZone::TabBar { .. } => {
                        self.active_tab = (self.active_tab + 1) % 3;
                        self.last_hit = "TabBar".into();
                    }
                    FrameZone::List { .. } => {
                        let lh = backend.line_height();
                        let row = ((position.y - list_rect.y) / lh).floor() as usize;
                        if row < self.items.len() {
                            self.selected = row;
                            self.last_hit = format!("List({row})");
                        }
                    }
                    FrameZone::StatusBar { .. } => {
                        self.last_hit = "StatusBar".into();
                    }
                    _ => {
                        self.last_hit = format!("{zone:?}");
                    }
                }
                return Reaction::Redraw;
            }
            _ => {}
        }
        Reaction::Continue
    }
}
