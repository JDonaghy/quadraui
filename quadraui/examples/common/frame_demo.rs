//! Renders a TabBar + ListView + StatusBar entirely through
//! `ScreenLayout::draw()`, then dispatches clicks by recomputing each
//! surface's own rect and testing it directly, rather than caching a
//! `FrameHitMap`/`FrameZone` hit-map across the frame — the same
//! pattern `activity_style_demo.rs` and `ShellView::compute_layout`'s
//! own callers use.
//!
//! The ListView rows are intentionally wider than the terminal so the
//! example doubles as the interactive smoke test for ListView horizontal
//! scroll: `←`/`→` (or `h`/`l`) scroll the content and a horizontal
//! scrollbar appears along the bottom of the list area (TUI backend).

use quadraui::{
    AppLogic, Backend, Color, Key, ListItem, ListView, NamedKey, Reaction, Rect, ScreenLayout,
    SegmentMeasure, StatusBar, StatusBarSegment, StyledText, Surface, TabBar, TabBarHit, TabItem,
    TabMeasure, UiEvent, WidgetId,
};

pub struct FrameDemo {
    active_tab: usize,
    items: Vec<String>,
    selected: usize,
    h_scroll: usize,
    /// Toggles the ListView vertical scrollbar (the `v` key) so the demo
    /// visually exercises `show_v_scrollbar` — the vertical counterpart to the
    /// existing h_scroll demonstration.
    show_v_scroll: bool,
    /// Active h-scrollbar thumb drag: (track_x, track_w, thumb_len, grab_offset).
    h_sb_drag: Option<(f32, f32, f32, f32)>,
    last_hit: String,
}

impl FrameDemo {
    pub fn new() -> Self {
        // Five intentionally long rows so content overflows any reasonable
        // terminal width — this is what exercises ListView h_scroll.
        let mut items: Vec<String> = vec![
            "Pods         kube-system/coredns-5d78c9869d-xz4kp  ip-10-0-1-23.us-west-2.compute.internal  3/3 Running r:0".into(),
            "Deployments  kube-system/coredns                   2 desired / 2 updated / 2 available       Available".into(),
            "Services     default/kubernetes                    ClusterIP 10.96.0.1  ports 443/TCP        Active".into(),
            "ConfigMaps   kube-system/kube-root-ca.crt          1 key (ca.crt)  created 14d ago            Immutable".into(),
            "Secrets      default/default-token-abcde           kubernetes.io/service-account-token  3 keys  Opaque".into(),
        ];
        // Many more rows so the list also overflows VERTICALLY — gives the
        // `v` key a real vertical scrollbar to show, with the thumb tracking
        // scroll position as you move the selection with j/k.
        for i in 0..40 {
            items.push(format!(
                "Workload {i:02}  kube-system/workload-{i:03}            replica {}/3            Running",
                (i % 3) + 1,
            ));
        }
        Self {
            active_tab: 0,
            items,
            selected: 0,
            h_scroll: 0,
            show_v_scroll: true,
            h_sb_drag: None,
            last_hit: "—".into(),
        }
    }

    /// Widest row in chars: 2-char selection prefix + longest item text.
    /// No icons in this demo, so prefix + text is the full content width.
    fn max_content_width(&self) -> usize {
        let widest = self
            .items
            .iter()
            .map(|s| s.chars().count())
            .max()
            .unwrap_or(0);
        2 + widest
    }

    /// The three surfaces' rects, derived the same way `render` lays them
    /// out. Recomputed in `handle` so click/drag routing matches the
    /// paint — there is no cached hit map to read back (`FrameHitMap` is
    /// `#[deprecated]`; these surfaces carry no `Interactive`/`DynWidget`
    /// identity for a `FrameTree` node to hold either).
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
                    is_closable: true,
                })
                .collect(),
            scroll_offset: 0,
            right_segments: vec![],
            active_accent: Some(Color::rgb(80, 160, 240)),
            show_tab_close: false,
            compact: false,
            ..Default::default()
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
            // Drive scroll_offset from the selection so j/k scrolls the
            // list and the vertical-scrollbar thumb visibly tracks position.
            // The primitive clamps this in `layout()`.
            scroll_offset: self.selected,
            has_focus: true,
            bordered: false,
            h_scroll: self.h_scroll,
            max_content_width: Some(self.max_content_width()),
            show_v_scrollbar: self.show_v_scroll,
        }
    }

    fn status_bar(&self) -> StatusBar {
        let fg = Color::rgb(220, 220, 220);
        let bg = Color::rgb(40, 40, 60);
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " tab:{} sel:{} hscroll:{} vbar:{} hit:{} ",
                    self.active_tab,
                    self.selected,
                    self.h_scroll,
                    if self.show_v_scroll { "on" } else { "off" },
                    self.last_hit
                ),
                fg,
                bg,
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text:
                    " ←/→ or h/l = h-scroll | j/k = move (v-scrolls) | v = toggle v-bar | q=quit "
                        .into(),
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
        let (tab_rect, list_rect, status_rect) = self.rects(backend);

        let tab_bar = self.tab_bar();
        let list = self.list_view();
        let status = self.status_bar();

        let mut frame = ScreenLayout::new();
        frame.push(Surface::Widget {
            rect: tab_rect,
            widget: Box::new(tab_bar),
        });
        frame.push(Surface::List {
            rect: list_rect,
            list,
        });
        frame.push(Surface::Widget {
            rect: status_rect,
            widget: Box::new(status),
        });

        frame.draw(backend);
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
                Key::Char('l') | Key::Named(NamedKey::Right) => {
                    // Clamp so we can't scroll past the last content column.
                    let cw = backend.char_width().max(1.0);
                    let visible_cols = (backend.viewport().width / cw).floor() as usize;
                    let max_scroll = self.max_content_width().saturating_sub(visible_cols);
                    self.h_scroll = (self.h_scroll + 1).min(max_scroll);
                    return Reaction::Redraw;
                }
                Key::Char('h') | Key::Named(NamedKey::Left) => {
                    self.h_scroll = self.h_scroll.saturating_sub(1);
                    return Reaction::Redraw;
                }
                Key::Char('v') => {
                    // Toggle the ListView vertical scrollbar on/off.
                    self.show_v_scroll = !self.show_v_scroll;
                    return Reaction::Redraw;
                }
                Key::Named(NamedKey::Tab) => {
                    self.active_tab = (self.active_tab + 1) % 3;
                    return Reaction::Redraw;
                }
                _ => {}
            },
            UiEvent::MouseDown { position, .. } => {
                let (tab_rect, list_rect, status_rect) = self.rects(backend);

                // The h-scrollbar thumb sits on the bottom row of the list
                // rect, so hit-test it first. Geometry comes straight from
                // the backend (`list_hscrollbar`) — the same `Scrollbar`
                // the rasteriser painted — so the draggable thumb lines up
                // exactly with what's on screen.
                let list = self.list_view();
                if let Some(hsb) = backend.list_hscrollbar(list_rect, &list) {
                    let on_row =
                        position.y >= hsb.track.y && position.y < hsb.track.y + hsb.track.height;
                    if on_row {
                        let local = position.x - hsb.track.x;
                        let max_scroll = self
                            .max_content_width()
                            .saturating_sub(hsb.track.width as usize);
                        if local >= hsb.thumb_start && local < hsb.thumb_start + hsb.thumb_len {
                            // Grab the thumb; remember where within it we grabbed.
                            self.h_sb_drag = Some((
                                hsb.track.x,
                                hsb.track.width,
                                hsb.thumb_len,
                                local - hsb.thumb_start,
                            ));
                            self.last_hit = "HScrollThumb".into();
                        } else if local < hsb.thumb_start {
                            // Page-scroll toward the click.
                            self.h_scroll = self.h_scroll.saturating_sub(5);
                            self.last_hit = "HScrollTrack".into();
                        } else {
                            self.h_scroll = (self.h_scroll + 5).min(max_scroll);
                            self.last_hit = "HScrollTrack".into();
                        }
                        return Reaction::Redraw;
                    }
                }

                // The three surfaces stack vertically with no overlap, so
                // checking each rect in turn (instead of a cached z-ordered
                // hit map) resolves unambiguously.
                if tab_rect.contains(*position) {
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
                } else if list_rect.contains(*position) {
                    let lh = backend.line_height();
                    let row = ((position.y - list_rect.y) / lh).floor() as usize;
                    if row < self.items.len() {
                        self.selected = row;
                        self.last_hit = format!("List({row})");
                    }
                } else if status_rect.contains(*position) {
                    self.last_hit = "StatusBar".into();
                } else {
                    self.last_hit = "Empty".into();
                }
                return Reaction::Redraw;
            }
            UiEvent::MouseMoved { position, .. } => {
                if let Some((track_x, track_w, thumb_len, grab_off)) = self.h_sb_drag {
                    // Map the cursor's position along the track to an
                    // h_scroll column, accounting for where we grabbed the
                    // thumb. `track_w` is the visible content width, so
                    // max_scroll is content minus what fits.
                    let max_scroll = self.max_content_width().saturating_sub(track_w as usize);
                    let effective = (track_w - thumb_len).max(1.0);
                    let rel = ((position.x - track_x - grab_off) / effective).clamp(0.0, 1.0);
                    self.h_scroll = (rel * max_scroll as f32).round() as usize;
                    return Reaction::Redraw;
                }
            }
            UiEvent::MouseUp { .. } if self.h_sb_drag.take().is_some() => {
                return Reaction::Redraw;
            }
            _ => {}
        }
        Reaction::Continue
    }
}
