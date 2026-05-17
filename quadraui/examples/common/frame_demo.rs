//! Proof-of-concept for `ScreenLayout` + `FrameHitMap` (#202 Stage 3+4).
//!
//! Renders a TabBar + ListView + StatusBar entirely through
//! `ScreenLayout::draw()`, then dispatches clicks via
//! `FrameHitMap::hit_test()` to identify which surface was clicked.

use std::cell::RefCell;

use quadraui::{
    AppLogic, Backend, Color, FrameHitMap, FrameZone, Key, ListItem, ListView, NamedKey, Reaction,
    Rect, ScreenLayout, SegmentMeasure, StatusBar, StatusBarSegment, StyledText, Surface, TabBar,
    TabBarHit, TabItem, TabMeasure, UiEvent, WidgetId,
};

pub struct FrameDemo {
    active_tab: usize,
    items: Vec<String>,
    selected: usize,
    last_hit: String,
    cached_hit_map: RefCell<FrameHitMap>,
}

impl FrameDemo {
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
        }
    }

    fn status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " tab:{} sel:{} hit:{} ",
                    self.active_tab, self.selected, self.last_hit
                ),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " ScreenLayout demo | q=quit ".into(),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for FrameDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for FrameDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let lh = backend.line_height();
        let tab_h = (lh * 1.5).round();
        let status_h = (lh * 1.5).round();
        let list_h = (vp.height - tab_h - status_h).max(0.0);

        let tab_rect = Rect::new(0.0, 0.0, vp.width, tab_h);
        let list_rect = Rect::new(0.0, tab_h, vp.width, list_h);
        let status_rect = Rect::new(0.0, tab_h + list_h, vp.width, status_h);

        let tab_bar = self.tab_bar();
        let list = self.list_view();
        let status = self.status_bar();

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

        let hit_map = frame.draw(backend);
        *self.cached_hit_map.borrow_mut() = hit_map;
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
                let hit_map = self.cached_hit_map.borrow();
                let zone = hit_map.hit_test(position.x, position.y);
                match zone {
                    FrameZone::TabBar { .. } => {
                        let lh = backend.line_height();
                        let cw = backend.char_width();
                        let vp = backend.viewport();
                        let tab_bar = self.tab_bar();
                        let layout = tab_bar.layout(
                            vp.width,
                            (lh * 1.5).round(),
                            cw * 2.0,
                            |i| {
                                let w = tab_bar.tabs[i].label.chars().count() as f32 * cw;
                                TabMeasure::new(w, 0.0)
                            },
                            |_| SegmentMeasure::new(0.0),
                        );
                        if let TabBarHit::Tab(idx) = layout.hit_test(position.x, position.y) {
                            self.active_tab = idx;
                            self.last_hit = format!("Tab({idx})");
                        }
                    }
                    FrameZone::List { .. } => {
                        let lh = backend.line_height();
                        let tab_h = (lh * 1.5).round();
                        let row = ((position.y - tab_h) / lh).floor() as usize;
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
