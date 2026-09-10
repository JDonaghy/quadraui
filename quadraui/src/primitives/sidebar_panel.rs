//! `SidebarPanel` primitive: a vertical container with an optional
//! [`Toolbar`] header at the top + a content region beneath that the
//! host paints into.
//!
//! Solves the "panel-with-toolbar" composition gap surfaced in #259.
//! Hosts that wanted a sidebar with a clickable action header used to
//! carve a top rect by hand, paint a `Toolbar` into it, remember to
//! always reserve the slot (so content didn't shift when the toolbar
//! came/went), and hit-test the bar manually before falling through
//! to content. That coordination — paint + hit-test + layout-reserve
//! — is the easy-to-get-wrong part this primitive owns.
//!
//! ## Why "SidebarPanel" and not "Sidebar"
//!
//! The compose layer already exports a `SidebarEvent` enum tied to
//! the multi-section [`crate::SidebarSystem`]. Adding a `Sidebar`
//! primitive with its own `SidebarEvent` would collide at the lib
//! re-export boundary. `SidebarPanel` reads correctly at consumer
//! use sites (each panel IS a sidebar panel) and keeps the names
//! disjoint.
//!
//! ## Shape
//!
//! - `toolbar`:
//!   - `None`: no slot reserved. Content occupies the full rect.
//!   - `Some(bar)`: header slot reserved at `toolbar_height`. Reserved
//!     *even when `bar.buttons.is_empty()`*, so content below doesn't
//!     shift as toolbar items come and go (a bug a consumer app hit
//!     when toolbar appearance was state-dependent).
//! - `toolbar_height`: optional explicit height in native units. When
//!   `None`, backends pick an idiomatic default (`1.0` cells TUI,
//!   `line_height` GTK / macOS).
//!
//! ## Backend contract
//!
//! Each backend renders the toolbar into the header slot and **does
//! not paint the content region** — the content rect is returned in
//! `SidebarPanelLayout.content_bounds` for the host to paint into
//! (typically a tree, list, form, etc.). This is the same pattern
//! [`crate::Panel`] uses for its content region.
//!
//! Hit-test:
//! - Click inside the toolbar slot routes via the nested
//!   `ToolbarLayout::hit_test` to a [`SidebarPanelHit::ToolbarButton`]
//!   or, for misses inside the slot,
//!   [`SidebarPanelHit::ToolbarEmpty`].
//! - Click inside the content rect produces
//!   [`SidebarPanelHit::Content`] with content-local coordinates.
//! - Click outside both produces [`SidebarPanelHit::Empty`].

use crate::event::Rect;
use crate::primitives::toolbar::{Toolbar, ToolbarHit, ToolbarItemMeasure, ToolbarLayout};
use crate::types::WidgetId;
use serde::{Deserialize, Serialize};

// ── Data model ───────────────────────────────────────────────────────────────

/// Vertical container: optional header toolbar + content region.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidebarPanel {
    pub id: WidgetId,
    /// Optional header toolbar. When `None` the slot is **not**
    /// reserved (content gets the full rect). When `Some`, the slot
    /// is always reserved at `toolbar_height` even if `buttons` is
    /// empty — so content below doesn't shift as buttons appear /
    /// disappear.
    #[serde(default)]
    pub toolbar: Option<Toolbar>,
    /// Reserved height for the toolbar slot in native units. When
    /// `None`, backends pick an idiomatic default (1 cell TUI,
    /// `line_height` GTK / macOS).
    #[serde(default)]
    pub toolbar_height: Option<f32>,
}

// ── Layout + hit-testing ─────────────────────────────────────────────────────

/// Fully-resolved layout for a [`SidebarPanel`].
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarPanelLayout {
    /// Bounds of the entire panel as given to `layout()`.
    pub panel_bounds: Rect,
    /// Bounds of the toolbar slot. `None` when the panel has no
    /// toolbar (slot wasn't reserved). When `Some`, this rect is
    /// reserved even if the toolbar has no buttons.
    pub toolbar_bounds: Option<Rect>,
    /// Bounds of the content area. Always populated — equals the
    /// panel minus the toolbar slot.
    pub content_bounds: Rect,
    /// Resolved inner toolbar layout. `None` mirrors `toolbar_bounds`.
    pub toolbar_layout: Option<ToolbarLayout>,
}

/// Classification of a hit-test result on a [`SidebarPanel`].
#[derive(Debug, Clone, PartialEq)]
pub enum SidebarPanelHit {
    /// Click landed on a clickable toolbar button — carries its id.
    ToolbarButton(WidgetId),
    /// Click landed inside the toolbar slot but not on a clickable
    /// button (gap, separator, label, or disabled action).
    ToolbarEmpty,
    /// Click landed inside the content area. `(x, y)` are
    /// **content-local** — relative to `content_bounds.x` / `.y`.
    Content { x: f32, y: f32 },
    /// Click missed both the toolbar slot and the content area
    /// (outside the panel entirely).
    Empty,
}

impl SidebarPanelLayout {
    /// Test point `(x, y)` against the layout. Toolbar slot wins over
    /// the content area when bounds overlap (which they shouldn't —
    /// `layout()` carves disjoint rects — but the precedence is
    /// documented in case future relaxations of that invariant break
    /// it).
    pub fn hit_test(&self, x: f32, y: f32) -> SidebarPanelHit {
        if let (Some(tb), Some(tlayout)) = (self.toolbar_bounds, &self.toolbar_layout) {
            if contains(tb, x, y) {
                return match tlayout.hit_test(x, y) {
                    ToolbarHit::Button(id) => SidebarPanelHit::ToolbarButton(id),
                    ToolbarHit::Empty => SidebarPanelHit::ToolbarEmpty,
                };
            }
        }
        if contains(self.content_bounds, x, y) {
            return SidebarPanelHit::Content {
                x: x - self.content_bounds.x,
                y: y - self.content_bounds.y,
            };
        }
        SidebarPanelHit::Empty
    }
}

fn contains(r: Rect, x: f32, y: f32) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

// ── Measurement ──────────────────────────────────────────────────────────────

/// Caller-supplied measurements for computing a
/// [`SidebarPanelLayout`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarPanelMeasure {
    /// Default header height when `SidebarPanel.toolbar_height` is
    /// `None`. Use `1.0` for TUI cells, `line_height` for native.
    pub default_toolbar_height: f32,
    /// Per-toolbar-item width in native units — passed through to the
    /// nested `Toolbar::layout`. Backends supply the same measurer
    /// they would use for a standalone toolbar in the same rect.
    pub item_width: f32,
}

impl SidebarPanelMeasure {
    pub fn new(default_toolbar_height: f32, item_width: f32) -> Self {
        Self {
            default_toolbar_height,
            item_width,
        }
    }
}

impl SidebarPanel {
    /// Compute the full layout for this panel.
    ///
    /// `measure_item` is the per-toolbar-item width measurer. It
    /// receives each `ToolbarButton` and returns its width in the
    /// caller's native unit — TUI passes a cell-count measurer, GTK
    /// passes a Pango pixel measurer. When the panel has no toolbar
    /// the closure is never invoked.
    pub fn layout<F>(
        &self,
        bounds: Rect,
        measure: SidebarPanelMeasure,
        measure_item: F,
    ) -> SidebarPanelLayout
    where
        F: Fn(&crate::primitives::toolbar::ToolbarButton) -> ToolbarItemMeasure,
    {
        let toolbar_height = self
            .toolbar_height
            .unwrap_or(measure.default_toolbar_height);

        match &self.toolbar {
            Some(bar) => {
                // Reserve the slot even when `bar.buttons.is_empty()`
                // so content doesn't shift as toolbar items come and
                // go (a bug a consumer app hit twice).
                let slot_height = toolbar_height.min(bounds.height).max(0.0);
                let toolbar_bounds = Rect::new(bounds.x, bounds.y, bounds.width, slot_height);
                let content_y = bounds.y + slot_height;
                let content_h = (bounds.height - slot_height).max(0.0);
                let content_bounds = Rect::new(bounds.x, content_y, bounds.width, content_h);

                let toolbar_layout = bar.layout(
                    toolbar_bounds.x,
                    toolbar_bounds.y,
                    toolbar_bounds.width,
                    toolbar_bounds.height,
                    measure_item,
                );

                SidebarPanelLayout {
                    panel_bounds: bounds,
                    toolbar_bounds: Some(toolbar_bounds),
                    content_bounds,
                    toolbar_layout: Some(toolbar_layout),
                }
            }
            None => SidebarPanelLayout {
                panel_bounds: bounds,
                toolbar_bounds: None,
                content_bounds: bounds,
                toolbar_layout: None,
            },
        }
    }
}

// ── NativeSurface paint (#862, Phase 2d slice 5/9 of the NativeSurface
// milestone) ─────────────────────────────────────────────────────────────
//
// Before this, `gtk::draw_sidebar_panel`, `macos::sidebar_panel::draw_sidebar_panel`
// and `win::sidebar_panel::draw_sidebar_panel` each independently computed
// this primitive's layout and then delegated painting to their own
// backend's `toolbar::draw_toolbar` free function (quadraui#785 child
// #811, `docs/SMELL_AUDIT_2026-07.md` §5). `paint` below is the one
// shared implementation, written against
// [`crate::native_surface::NativeSurface`] (#807, Phase 1) instead of any
// one backend's drawing API.
//
// `SidebarPanel` paints *nothing* of its own — its entire visible surface
// is the optional [`Toolbar`] header (the content region is always left
// to the host, unchanged by this migration). That means unifying this
// primitive's paint necessarily means porting the toolbar-button
// rendering itself to `NativeSurface`, even though `Toolbar` is not one
// of this issue's files and hasn't had its own Phase 2d slice yet (it's
// one of the remaining eight #811 slices, still open, and
// `gtk::toolbar::draw_toolbar` / `macos::toolbar::draw_toolbar` /
// `win::toolbar::draw_toolbar` are untouched by this PR — standalone
// `Backend::draw_toolbar` still calls them directly). `paint_toolbar_header`
// below is therefore a `NativeSurface`-generic duplicate of that
// rendering logic, scoped to what a sidebar panel's header needs. When
// `Toolbar` gets its own Phase 2d slice, that issue should either extract
// a helper both primitives share or delete this copy in favour of that
// one — flagged here so it isn't forgotten.
//
// Comparing the three deleted copies surfaced four divergences, reported
// per this issue's "compare what each backend actually did" instruction
// rather than silently picked:
//
// 1. **Rounded vs. plain highlight/focus-ring rects.** GTK painted the
//    hover/pressed/active highlight and the keyboard-focus ring as a
//    4px-radius rounded-rect pill (`gtk::rounded_rect_path`); macOS and
//    Win already painted plain rectangles (win's own module doc called
//    this out as a scope gap: "no rounded-rect / stroke-inset helper
//    exists yet in `win::text`"). `NativeSurface::surface_fill_rect` /
//    `surface_stroke_rect` have no rounded-rect verb, so the unified
//    `paint_toolbar_header` necessarily follows the 2-of-3 majority
//    (macOS/Win's plain rect) — GTK's toolbar-header buttons lose their
//    rounded corners. A future issue could add a `NativeSurface`
//    rounded-rect verb if the corner treatment is worth restoring.
// 2. **Missing clip on Win.** GTK (`cr.clip()`) and macOS
//    (`CGContextClipToRect`) both clipped painting to the toolbar
//    bounds before drawing; `win::toolbar::draw_toolbar` never clipped
//    at all. Nothing documents this as deliberate, so — mirroring how
//    #811/#859 fixed the analogous GTK alpha-drop omission at the
//    source rather than reproducing it — `paint_toolbar_header` clips via
//    `surface_push_clip`/`surface_pop_clip` on every backend, closing
//    the Win gap instead of carrying it forward.
// 3. **GTK alpha-drop.** The old `gtk::toolbar::draw_toolbar` filled the
//    bar background and button highlights via `gtk::set_source`
//    (`cr.set_source_rgb`), dropping `Color::a` — the exact class of bug
//    #811 fixed once, at the source, in `GtkBackend::surface_fill_rect`
//    (now `set_source_rgba`). Routing through `surface_fill_rect` here
//    inherits that fix automatically; no extra code needed, and no
//    visible effect today since `Toolbar.bg` / theme colours are always
//    opaque.
// 4. **Win never wired a live theme through.** `win::toolbar::draw_toolbar`
//    (and thus the old `win::sidebar_panel::draw_sidebar_panel`, which
//    called it) takes no `Theme` parameter at all — it always painted
//    with `Theme::default()`, matching the same "not wired through yet"
//    posture `WinBackend::draw_panel` documents for `Panel` chrome (#859).
//    Preserved as-is: `WinBackend::draw_sidebar_panel` still passes
//    `Theme::default()` into the shared `paint`, not `self.current_theme`
//    — fixing this is `win::toolbar`'s own gap, out of scope for #862.
//
// `#[allow(dead_code)]`: see `primitives::form`'s identical note (#808)
// — only *called* once a real pixel backend is compiled in, exercised by
// each backend's own `Backend::draw_sidebar_panel` call site plus this
// module's own `RecordingSurface` tests on every leg that enables one of
// the three cfg'd features.
#[cfg(any(
    feature = "gtk",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
#[allow(dead_code)]
pub(crate) mod native_surface_paint {
    use super::{SidebarPanel, SidebarPanelLayout, SidebarPanelMeasure};
    use crate::event::{Point, Rect};
    use crate::native_surface::NativeSurface;
    use crate::primitives::layout_metrics::TextMeasure;
    use crate::primitives::toolbar::{
        action_text, measure_button, Toolbar, ToolbarButton, ToolbarItemMeasure, ToolbarLayout,
    };
    use crate::theme::Theme;
    use crate::types::WidgetId;

    /// Adapts a live `&dyn NativeSurface` to the shared [`TextMeasure`]
    /// trait so [`measure_button`] never has to name a backend-specific
    /// font type — generalises `gtk::toolbar::PangoMeasure` /
    /// `macos::toolbar::CtFontMeasure` / `win::toolbar::DWriteMeasure`
    /// into the one adapter every backend now shares.
    struct SurfaceMeasure<'a>(&'a dyn NativeSurface);

    impl TextMeasure for SurfaceMeasure<'_> {
        fn width_of(&self, text: &str) -> f32 {
            self.0.surface_measure_text(text).0
        }
    }

    /// Paint a [`SidebarPanel`] into `bounds` on `surface`, returning the
    /// resolved [`SidebarPanelLayout`] for the caller's click dispatch —
    /// same contract as [`crate::Backend::draw_sidebar_panel`]:
    /// `content_bounds` / `toolbar_bounds` are **ABSOLUTE** (shifted by
    /// `bounds.x`/`bounds.y`), matching every backend's pre-#862
    /// rasteriser and [`crate::Backend::sidebar_panel_layout`]'s
    /// documented contract. The content region is never painted — same
    /// contract as before this migration; the host paints
    /// `layout.content_bounds` itself.
    ///
    /// `hovered_toolbar_id` / `pressed_toolbar_id` tint the matching
    /// toolbar action button; the primitive itself carries no mouse
    /// state.
    ///
    /// `nerd_fonts_enabled` (issue #913 review fix) picks which half of
    /// an overridden button's [`crate::types::Icon`] (registered via
    /// [`Toolbar::with_icon_override`]) is measured and painted — `glyph`
    /// when `true`, `fallback` when `false` — same contract as every
    /// other rasteriser's `nerd_fonts_enabled`. Before this fix the
    /// parameter didn't exist here at all, so a `SidebarPanel`'s embedded
    /// `Toolbar.icon_overrides` was silently dead data on every backend
    /// that routes through this shared paint (GTK, macOS, Win) — see
    /// this module's own tests for the round-trip that now covers it. A
    /// `Toolbar` with no overrides measures/paints its own `icon` field
    /// regardless of the flag, so this is a no-op for every pre-#913
    /// panel.
    ///
    /// A non-positive `bounds.width`/`bounds.height` short-circuits to
    /// the no-paint layout without touching `surface` at all, matching
    /// every pre-#862 per-backend copy's `if w <= 0.0 || h <= 0.0 {
    /// return layout; }` guard.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint(
        panel: &SidebarPanel,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
        bounds: Rect,
        default_toolbar_height: f32,
        hovered_toolbar_id: Option<&WidgetId>,
        pressed_toolbar_id: Option<&WidgetId>,
        nerd_fonts_enabled: bool,
    ) -> SidebarPanelLayout {
        let layout = {
            // `item_width` (the measure's second field) is dead in
            // `SidebarPanel::layout` — every backend's own layout helper
            // already passes `0.0` for it (see e.g. `win_sidebar_panel_layout`)
            // since real per-button widths come from the `measure_item`
            // closure below, not this field.
            let measurer = SurfaceMeasure(&*surface);
            panel.layout(
                bounds,
                SidebarPanelMeasure::new(default_toolbar_height, 0.0),
                |btn| {
                    // `panel.toolbar` is always `Some` when this closure
                    // runs — `SidebarPanel::layout` only ever calls
                    // `measure_item` on buttons drawn from `self.toolbar`
                    // — so resolving the override through it is safe.
                    let resolved = match &panel.toolbar {
                        Some(bar) => bar.resolve_button_icon(btn, nerd_fonts_enabled),
                        None => std::borrow::Cow::Borrowed(btn),
                    };
                    ToolbarItemMeasure::new(measure_button(&measurer, &resolved))
                },
            )
        };

        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return layout;
        }

        if let (Some(bar), Some(tb_bounds)) = (&panel.toolbar, layout.toolbar_bounds) {
            if let Some(toolbar_layout) = &layout.toolbar_layout {
                paint_toolbar_header(
                    bar,
                    toolbar_layout,
                    surface,
                    theme,
                    tb_bounds,
                    hovered_toolbar_id,
                    pressed_toolbar_id,
                    nerd_fonts_enabled,
                );
            }
        }

        layout
    }

    /// Paint `bar`'s buttons into `tb_bounds` on `surface` — see this
    /// module's doc for why this exists (a `NativeSurface`-generic port
    /// of `gtk`/`macos`/`win`'s own `toolbar::draw_toolbar`, scoped to
    /// what a sidebar panel's header needs) and for the four named
    /// divergences found while porting it. See [`paint`] for
    /// `nerd_fonts_enabled`'s contract — paint and layout resolve every
    /// button's icon the same way, so a wide glyph's measured and
    /// painted widths never disagree.
    #[allow(clippy::too_many_arguments)]
    fn paint_toolbar_header(
        bar: &Toolbar,
        toolbar_layout: &ToolbarLayout,
        surface: &mut dyn NativeSurface,
        theme: &Theme,
        tb_bounds: Rect,
        hovered_id: Option<&WidgetId>,
        pressed_id: Option<&WidgetId>,
        nerd_fonts_enabled: bool,
    ) {
        // Divergence 2: clip on every backend, closing the pre-#862 Win
        // gap instead of reproducing it.
        surface.surface_push_clip(tb_bounds);

        let bar_bg = bar.bg.unwrap_or(theme.header_bg);
        surface.surface_fill_rect(tb_bounds, bar_bg);

        for vis in &toolbar_layout.visible_items {
            let item = vis.bounds;
            if item.width <= 0.0 || item.height <= 0.0 {
                continue;
            }
            let Some(btn) = bar.buttons.get(vis.item_idx) else {
                continue;
            };
            let resolved = bar.resolve_button_icon(btn, nerd_fonts_enabled);

            match resolved.as_ref() {
                ToolbarButton::Action {
                    id,
                    label,
                    icon,
                    key_hint,
                    enabled,
                    is_active,
                    ..
                } => {
                    let is_hovered = *enabled && hovered_id == Some(id);
                    let is_pressed = *enabled && pressed_id == Some(id);
                    let is_focused = *enabled && bar.focused_index == Some(vis.item_idx);

                    // Highlight background: pressed/active > hovered >
                    // none. Plain rect — see divergence 1 (GTK's
                    // pre-#862 rounded-rect pill isn't reproducible
                    // against `NativeSurface`).
                    let highlight = if is_pressed || *is_active {
                        Some(theme.selected_bg)
                    } else if is_hovered {
                        Some(theme.hover_bg)
                    } else {
                        None
                    };
                    if let Some(bg) = highlight {
                        let inset = Rect::new(
                            item.x + 2.0,
                            item.y + 2.0,
                            (item.width - 4.0).max(0.0),
                            (item.height - 4.0).max(0.0),
                        );
                        surface.surface_fill_rect(inset, bg);
                    }

                    // Focus ring: only when not already visually
                    // dominated by hover/pressed/active.
                    if is_focused && !is_hovered && !is_pressed && !*is_active {
                        let ring = Rect::new(
                            item.x + 1.5,
                            item.y + 1.5,
                            (item.width - 3.0).max(0.0),
                            (item.height - 3.0).max(0.0),
                        );
                        surface.surface_stroke_rect(ring, theme.accent_fg, 1.0);
                    }

                    let fg = if !*enabled {
                        theme.muted_fg
                    } else if is_hovered {
                        theme.hover_fg
                    } else {
                        theme.foreground
                    };

                    let text = action_text(label, icon.as_deref(), key_hint.as_deref());
                    let (tw, th) = surface.surface_measure_text(&text);
                    let tx = item.x + (item.width - tw) / 2.0;
                    let ty = item.y + (item.height - th) / 2.0;
                    surface.surface_draw_text_run(Rect::new(tx, ty, tw, th), &text, fg);
                }
                ToolbarButton::Separator => {
                    let mid_x = item.x + item.width / 2.0;
                    let pad_y = (item.height * 0.2).max(2.0);
                    surface.surface_draw_line(
                        Point::new(mid_x, item.y + pad_y),
                        Point::new(mid_x, item.y + item.height - pad_y),
                        theme.muted_fg,
                        1.0,
                    );
                }
                ToolbarButton::Label { text, fg } => {
                    let color = fg.unwrap_or(theme.muted_fg);
                    let (tw, th) = surface.surface_measure_text(text);
                    let ty = item.y + (item.height - th) / 2.0;
                    surface.surface_draw_text_run(Rect::new(item.x, ty, tw, th), text, color);
                }
            }
        }

        surface.surface_pop_clip();
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::backend::ImagePaintResult;
        use crate::event::Viewport;
        use crate::types::Color;
        use crate::Image;

        /// Records every surface verb this primitive's paint uses —
        /// mirrors `primitives::status_bar`'s `RecordingSurface` test
        /// double, so this test runs on any host without Cairo/Core
        /// Graphics/Direct2D.
        #[derive(Default)]
        struct RecordingSurface {
            fills: Vec<(Rect, Color)>,
            strokes: Vec<(Rect, Color, f32)>,
            text_runs: Vec<(Rect, String, Color)>,
            lines: Vec<(Point, Point, Color)>,
            clip_pushes: Vec<Rect>,
            clip_pops: usize,
        }

        impl NativeSurface for RecordingSurface {
            fn surface_begin_frame(&mut self, _viewport: Viewport) {}
            fn surface_end_frame(&mut self) {}
            fn surface_viewport(&self) -> Viewport {
                Viewport::new(200.0, 100.0, 1.0)
            }
            fn surface_line_height(&self) -> f32 {
                16.0
            }
            fn surface_char_width(&self) -> f32 {
                8.0
            }
            fn surface_measure_text(&self, text: &str) -> (f32, f32) {
                (text.chars().count() as f32 * 8.0, 16.0)
            }
            fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
                self.fills.push((rect, color));
            }
            fn surface_stroke_rect(&mut self, rect: Rect, color: Color, stroke_width: f32) {
                self.strokes.push((rect, color, stroke_width));
            }
            fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
                self.text_runs.push((rect, text.to_string(), color));
            }
            fn surface_draw_line(
                &mut self,
                from: Point,
                to: Point,
                color: Color,
                _stroke_width: f32,
            ) {
                self.lines.push((from, to, color));
            }
            fn surface_push_clip(&mut self, rect: Rect) {
                self.clip_pushes.push(rect);
            }
            fn surface_pop_clip(&mut self) {
                self.clip_pops += 1;
            }
            fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
                ImagePaintResult::Unsupported
            }
        }

        fn mk_action(id: &str, label: &str, enabled: bool) -> ToolbarButton {
            ToolbarButton::Action {
                id: WidgetId::new(id),
                label: label.into(),
                icon: None,
                key_hint: None,
                enabled,
                is_active: false,
                tooltip: String::new(),
            }
        }

        fn panel_with_toolbar() -> SidebarPanel {
            SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: Some(Toolbar {
                    id: WidgetId::new("sb:toolbar"),
                    buttons: vec![mk_action("a", "Refine", true), mk_action("b", "Drop", true)],
                    bg: None,
                    focused_index: None,
                    icon_overrides: Vec::new(),
                }),
                toolbar_height: None,
            }
        }

        #[test]
        fn no_toolbar_paints_nothing() {
            let panel = SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: None,
                toolbar_height: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                None,
                None,
                false,
            );
            assert!(surface.fills.is_empty());
            assert!(surface.text_runs.is_empty());
            assert_eq!(layout.content_bounds, Rect::new(0.0, 0.0, 100.0, 50.0));
        }

        #[test]
        fn zero_size_bounds_paints_nothing() {
            let panel = panel_with_toolbar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 0.0, 0.0),
                16.0,
                None,
                None,
                false,
            );
            assert!(surface.fills.is_empty());
            assert!(surface.clip_pushes.is_empty());
        }

        #[test]
        fn toolbar_header_fills_bar_background_and_clips() {
            let panel = panel_with_toolbar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                None,
                None,
                false,
            );
            let tb = layout.toolbar_bounds.expect("toolbar slot reserved");
            assert_eq!(surface.clip_pushes, vec![tb]);
            assert_eq!(surface.clip_pops, 1);
            assert_eq!(surface.fills[0], (tb, theme.header_bg));
        }

        #[test]
        fn hovered_button_gets_hover_bg_and_hover_fg() {
            let panel = panel_with_toolbar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                Some(&WidgetId::new("a")),
                None,
                false,
            );
            // fills[0] = bar background, fills[1] = hovered button's
            // highlight inset.
            assert_eq!(surface.fills[1].1, theme.hover_bg);
            let (_, _text, color) = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t == "Refine")
                .expect("Refine label drawn");
            assert_eq!(*color, theme.hover_fg);
        }

        #[test]
        fn pressed_button_gets_selected_bg() {
            let panel = panel_with_toolbar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                None,
                Some(&WidgetId::new("a")),
                false,
            );
            assert_eq!(surface.fills[1].1, theme.selected_bg);
        }

        #[test]
        fn disabled_action_paints_muted_fg_no_highlight() {
            let panel = SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: Some(Toolbar {
                    id: WidgetId::new("sb:toolbar"),
                    buttons: vec![mk_action("a", "Refine", false)],
                    bg: None,
                    focused_index: None,
                    icon_overrides: Vec::new(),
                }),
                toolbar_height: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                Some(&WidgetId::new("a")), // hover on a disabled button is a no-op
                None,
                false,
            );
            // Only the bar background fill — no highlight for a disabled
            // (thus never "hovered") action.
            assert_eq!(surface.fills.len(), 1);
            let (_, _, color) = &surface.text_runs[0];
            assert_eq!(*color, theme.muted_fg);
        }

        #[test]
        fn focused_button_gets_stroke_ring_when_not_hovered_or_pressed() {
            let panel = SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: Some(Toolbar {
                    id: WidgetId::new("sb:toolbar"),
                    buttons: vec![mk_action("a", "Refine", true)],
                    bg: None,
                    focused_index: Some(0),
                    icon_overrides: Vec::new(),
                }),
                toolbar_height: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                None,
                None,
                false,
            );
            assert_eq!(surface.strokes.len(), 1);
            assert_eq!(surface.strokes[0].1, theme.accent_fg);
        }

        #[test]
        fn separator_draws_a_vertical_line_not_a_fill() {
            let panel = SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: Some(Toolbar {
                    id: WidgetId::new("sb:toolbar"),
                    buttons: vec![mk_action("a", "Refine", true), ToolbarButton::Separator],
                    bg: None,
                    focused_index: None,
                    icon_overrides: Vec::new(),
                }),
                toolbar_height: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                None,
                None,
                false,
            );
            assert_eq!(surface.lines.len(), 1);
            assert_eq!(surface.lines[0].2, theme.muted_fg);
        }

        #[test]
        fn label_draws_left_aligned_unhighlighted_text() {
            let panel = SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: Some(Toolbar {
                    id: WidgetId::new("sb:toolbar"),
                    buttons: vec![ToolbarButton::Label {
                        text: "2 of 5".into(),
                        fg: None,
                    }],
                    bg: None,
                    focused_index: None,
                    icon_overrides: Vec::new(),
                }),
                toolbar_height: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                None,
                None,
                false,
            );
            let tb = layout.toolbar_bounds.unwrap();
            // Only the bar background fill — labels never get a
            // highlight fill.
            assert_eq!(surface.fills.len(), 1);
            let (rect, text, color) = &surface.text_runs[0];
            assert_eq!(text, "2 of 5");
            assert_eq!(rect.x, tb.x);
            assert_eq!(*color, theme.muted_fg);
        }

        #[test]
        fn content_bounds_still_start_below_toolbar_slot() {
            let panel = panel_with_toolbar();
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            let layout = paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                None,
                None,
                false,
            );
            let tb = layout.toolbar_bounds.unwrap();
            assert_eq!(layout.content_bounds.y, tb.y + tb.height);
        }

        /// Issue #913 review fix: closes the "no test exercises an
        /// embedded toolbar with a registered override" gap — every
        /// pre-fix `nerd_fonts_flag_selects_glyph_or_fallback`-style test
        /// targeted the standalone `Toolbar` path only, so a
        /// `SidebarPanel`'s embedded toolbar silently never plumbed
        /// `icon_overrides` on GTK/macOS/Win (all three route through
        /// this shared `paint`). `nerd_fonts_enabled: true` must paint
        /// the override's `glyph`, not the button's own `icon` field.
        #[test]
        fn nerd_fonts_enabled_paints_override_glyph_in_embedded_toolbar() {
            let panel = SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: Some(
                    Toolbar::new(
                        WidgetId::new("sb:toolbar"),
                        vec![mk_action("a", "Refine", true)],
                    )
                    .with_icon_override(
                        WidgetId::new("a"),
                        crate::types::Icon::new("\u{f021}", "R"),
                    ),
                ),
                toolbar_height: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                None,
                None,
                true,
            );
            let (_, text, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t.contains("Refine"))
                .expect("Refine label drawn");
            assert_eq!(
                text, "\u{f021} Refine",
                "nerd_fonts_enabled: true should paint the override's glyph, not its fallback"
            );
        }

        /// Flag-off twin of the above: the same override must resolve to
        /// its ASCII `fallback`, not the Nerd-Font glyph — this is the
        /// byte-identical-with-develop acceptance bar for a panel with no
        /// overrides, extended to confirm a *registered* override still
        /// respects the flag rather than always winning.
        #[test]
        fn nerd_fonts_disabled_paints_override_fallback_in_embedded_toolbar() {
            let panel = SidebarPanel {
                id: WidgetId::new("sb"),
                toolbar: Some(
                    Toolbar::new(
                        WidgetId::new("sb:toolbar"),
                        vec![mk_action("a", "Refine", true)],
                    )
                    .with_icon_override(
                        WidgetId::new("a"),
                        crate::types::Icon::new("\u{f021}", "R"),
                    ),
                ),
                toolbar_height: None,
            };
            let theme = Theme::default();
            let mut surface = RecordingSurface::default();
            paint(
                &panel,
                &mut surface,
                &theme,
                Rect::new(0.0, 0.0, 100.0, 50.0),
                16.0,
                None,
                None,
                false,
            );
            let (_, text, _) = surface
                .text_runs
                .iter()
                .find(|(_, t, _)| t.contains("Refine"))
                .expect("Refine label drawn");
            assert_eq!(
                text, "R Refine",
                "nerd_fonts_enabled: false should paint the override's fallback, not its glyph"
            );
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::toolbar::ToolbarButton;
    use crate::types::WidgetId;

    fn mk_action(id: &str, label: &str) -> ToolbarButton {
        ToolbarButton::Action {
            id: WidgetId::new(id),
            label: label.to_string(),
            icon: None,
            key_hint: None,
            enabled: true,
            is_active: false,
            tooltip: String::new(),
        }
    }

    fn panel_with_toolbar() -> SidebarPanel {
        SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![mk_action("a", "Refine"), mk_action("b", "Drop")],
                bg: None,
                focused_index: None,
                icon_overrides: Vec::new(),
            }),
            toolbar_height: None,
        }
    }

    fn measure() -> (
        SidebarPanelMeasure,
        impl Fn(&ToolbarButton) -> ToolbarItemMeasure,
    ) {
        let m = SidebarPanelMeasure::new(1.0, 8.0);
        (m, |_btn: &ToolbarButton| ToolbarItemMeasure::new(8.0))
    }

    #[test]
    fn no_toolbar_gives_full_rect_to_content() {
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: None,
            toolbar_height: None,
        };
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        assert!(layout.toolbar_bounds.is_none());
        assert!(layout.toolbar_layout.is_none());
        assert_eq!(layout.content_bounds, Rect::new(0.0, 0.0, 30.0, 10.0));
    }

    #[test]
    fn toolbar_reserves_top_slot() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        let tb = layout.toolbar_bounds.expect("toolbar slot reserved");
        assert_eq!(tb.x, 0.0);
        assert_eq!(tb.y, 0.0);
        assert_eq!(tb.width, 30.0);
        assert_eq!(tb.height, 1.0); // default_toolbar_height
                                    // Content starts below the slot.
        assert_eq!(layout.content_bounds.y, 1.0);
        assert_eq!(layout.content_bounds.height, 9.0);
    }

    #[test]
    fn empty_toolbar_still_reserves_slot() {
        // The whole point of Gap 2 is that content doesn't shift when
        // the toolbar's buttons vector is temporarily empty.
        let panel = SidebarPanel {
            id: WidgetId::new("sb"),
            toolbar: Some(Toolbar {
                id: WidgetId::new("sb:toolbar"),
                buttons: vec![], // empty!
                bg: None,
                focused_index: None,
                icon_overrides: Vec::new(),
            }),
            toolbar_height: None,
        };
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        assert!(layout.toolbar_bounds.is_some());
        // Slot still reserved — content offset by the toolbar height.
        assert_eq!(layout.content_bounds.y, 1.0);
        assert_eq!(layout.content_bounds.height, 9.0);
    }

    #[test]
    fn explicit_toolbar_height_overrides_default() {
        let mut panel = panel_with_toolbar();
        panel.toolbar_height = Some(3.0);
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        let tb = layout.toolbar_bounds.unwrap();
        assert_eq!(tb.height, 3.0);
        assert_eq!(layout.content_bounds.y, 3.0);
        assert_eq!(layout.content_bounds.height, 7.0);
    }

    #[test]
    fn hit_test_routes_toolbar_button_click() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        // First button is at x=0..8 in the toolbar slot.
        match layout.hit_test(2.0, 0.0) {
            SidebarPanelHit::ToolbarButton(id) => assert_eq!(id.as_str(), "a"),
            other => panic!("expected ToolbarButton hit, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_inside_toolbar_slot_off_button_is_toolbar_empty() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        // Past the second button's right edge (16) but still in the
        // toolbar slot (y < 1).
        let hit = layout.hit_test(20.0, 0.0);
        assert_eq!(hit, SidebarPanelHit::ToolbarEmpty);
    }

    #[test]
    fn hit_test_content_returns_content_local_coords() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(10.0, 5.0, 30.0, 10.0), m, mi);
        // Click at panel-absolute (12.0, 8.0): toolbar slot is rows
        // 5..6, so y=8 lands in content. Content origin is (10, 6).
        // Local: (12-10, 8-6) = (2.0, 2.0).
        match layout.hit_test(12.0, 8.0) {
            SidebarPanelHit::Content { x, y } => {
                assert_eq!(x, 2.0);
                assert_eq!(y, 2.0);
            }
            other => panic!("expected Content hit, got {other:?}"),
        }
    }

    #[test]
    fn hit_test_outside_panel_is_empty() {
        let panel = panel_with_toolbar();
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        assert_eq!(layout.hit_test(100.0, 100.0), SidebarPanelHit::Empty);
    }

    #[test]
    fn toolbar_height_clamps_to_panel_height() {
        // If a host accidentally asks for a toolbar taller than the
        // panel, the slot must not overflow into negative-content
        // territory.
        let mut panel = panel_with_toolbar();
        panel.toolbar_height = Some(50.0);
        let (m, mi) = measure();
        let layout = panel.layout(Rect::new(0.0, 0.0, 30.0, 10.0), m, mi);
        let tb = layout.toolbar_bounds.unwrap();
        assert_eq!(tb.height, 10.0); // clamped to panel height
        assert_eq!(layout.content_bounds.height, 0.0);
    }

    #[test]
    fn serde_roundtrip() {
        let panel = panel_with_toolbar();
        let json = serde_json::to_string(&panel).unwrap();
        let back: SidebarPanel = serde_json::from_str(&json).unwrap();
        assert_eq!(panel, back);
    }
}
