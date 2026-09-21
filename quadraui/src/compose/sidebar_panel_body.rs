//! `SidebarPanelBody` — composes the four layers every sidebar-panel
//! renderer hand-rolls: a background fill, optional header/search
//! chrome, the panel's own body widget, and an optional scrollbar
//! gutter.
//!
//! # The gap this closes (issue #1041)
//!
//! quadraui already has the *fragments*: [`crate::Backend::draw_solid_fill`]
//! (background), [`crate::Backend::draw_settings_chrome`] (header +
//! search rows), and per-content rasterisers (`draw_tree`, `draw_form`,
//! [`crate::SidebarSystem::render`], …) for the body. Nothing composed
//! them into one ordered paint sequence — every host that wanted
//! "fill, then optional header/search, then body, then optional
//! scrollbar" re-derived the row-slicing arithmetic itself. A 2026-09-20
//! fleet audit of `vimcode` (`vimcode/docs/TUI_AUDIT_R2.md` §2.9) found
//! six of its sidebar-panel renderers doing exactly that — 616
//! production lines whose *decision* was already shared but whose
//! *chrome composition* was written twice, once per backend, and had
//! already drifted (chrome present on one backend and absent on the
//! other for the same panel).
//!
//! # Shape
//!
//! [`SidebarPanelBody`] is a stateless per-frame value, the same shape
//! as [`crate::Panel`] or [`crate::Scrollbar`] — hosts build one fresh
//! each frame from their own state (search query, focus, theme colour)
//! and call [`SidebarPanelBody::render`]. It owns no interaction state
//! of its own: scrolling, search-query editing, and body-widget
//! selection stay with whatever controller already manages the body
//! (`TreeController`, `FormController`, [`crate::SidebarSystem`], a raw
//! `TreeView`, …).
//!
//! - **Background** — an optional flat fill under the whole panel rect,
//!   via [`crate::Backend::draw_solid_fill`]. `None` paints nothing (the
//!   host's own background shows through, matching GTK's current
//!   "just call the body widget, no chrome" panels).
//! - **Chrome** — [`SidebarPanelChrome::None`] (no rows reserved),
//!   [`SidebarPanelChrome::Header`] (one title row), or
//!   [`SidebarPanelChrome::HeaderAndSearch`] (title row + filter-input
//!   row) — the same two shapes every existing call site of
//!   [`crate::Backend::draw_settings_chrome`] actually needs (Settings,
//!   Extensions). Delegates straight to that method, so header/search
//!   visuals don't get a second implementation here.
//! - **Body** — any [`crate::BackendWidget`], painted into the
//!   remaining rect after chrome and the scrollbar gutter are carved
//!   off. Same trait `BottomPanelConfig` already uses for tab content,
//!   so a host can share one widget between a bottom-panel tab and a
//!   sidebar panel if it wants to.
//! - **Scrollbar** — [`SidebarPanelBody::scrollbar_gutter`] reserves a
//!   fixed-width column on the right of the body rect (mirroring
//!   [`crate::compose::tree_controller::TreeController`]'s own
//!   `scrollbar_width` field). [`SidebarPanelBody::render`] does *not*
//!   paint the scrollbar itself — its thumb position depends on scroll
//!   state this type doesn't own. Call [`SidebarPanelBody::layout`]
//!   (or read the [`SidebarPanelBodyLayout`] `render` returns),
//!   build a [`crate::Scrollbar`] with `track` set to
//!   `layout.scrollbar_rect`, and paint it via
//!   [`crate::Backend::draw_scrollbar`] — the same two-step
//!   layout-then-paint split [`crate::compose::tree_controller::TreeController::render`]
//!   already uses internally.
//!
//! `unit_h` (row height for chrome rows) comes from
//! [`crate::Backend::line_height`] — `1.0` cell on TUI, real Pango/Core
//! Text/DirectWrite line height elsewhere — the same metric
//! `draw_settings_chrome` itself resolves rows against, so this
//! composer's row-slicing can never drift from what
//! `draw_settings_chrome` actually paints.
//!
//! # Example
//!
//! ```
//! use quadraui::compose::sidebar_panel_body::{SidebarPanelBody, SidebarPanelChrome};
//! use quadraui::testing::RecordingBackend;
//! use quadraui::{Backend, BackendWidget, Color, Rect};
//!
//! struct TreeBody;
//! impl BackendWidget for TreeBody {
//!     fn render(&self, backend: &mut dyn Backend, rect: Rect) {
//!         backend.draw_solid_fill(rect, Color::rgb(30, 30, 30));
//!     }
//! }
//!
//! let panel = SidebarPanelBody {
//!     background: Some(Color::rgb(20, 20, 20)),
//!     chrome: SidebarPanelChrome::HeaderAndSearch {
//!         header: "EXTENSIONS".into(),
//!         query: String::new(),
//!         placeholder: "Search extensions".into(),
//!         active: false,
//!     },
//!     scrollbar_gutter: Some(1.0),
//! };
//!
//! let mut backend = RecordingBackend::new();
//! let layout = panel.render(&mut backend, Rect::new(0.0, 0.0, 40.0, 20.0), &TreeBody);
//! assert!(layout.scrollbar_rect.is_some());
//! ```

use crate::backend::BackendWidget;
use crate::event::Rect;
use crate::types::Color;
use crate::Backend;

/// Optional header/search chrome painted above a [`SidebarPanelBody`]'s
/// body rect.
///
/// The two non-empty variants delegate straight to
/// [`crate::Backend::draw_settings_chrome`] — see that method's doc for
/// the exact row layout, `" / "`-prefixed prompt construction, and
/// placeholder logic. This enum only decides *whether* the header row
/// and the search row are reserved, not how they're painted.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SidebarPanelChrome {
    /// No chrome rows reserved — the body rect starts at the top of the
    /// panel rect.
    #[default]
    None,
    /// One title row, no search input beneath it (e.g. a Debug sidebar
    /// section header with no filter).
    Header(String),
    /// A title row followed by a search/filter input row — the exact
    /// shape `draw_settings_chrome` was designed for (Settings,
    /// Extensions).
    HeaderAndSearch {
        header: String,
        query: String,
        placeholder: String,
        /// Whether the search input has focus (drives caret + selected
        /// row-background tint — see `draw_settings_chrome`).
        active: bool,
    },
}

impl SidebarPanelChrome {
    /// Number of chrome rows this variant reserves, in `unit_h` units.
    fn rows(&self) -> f32 {
        match self {
            SidebarPanelChrome::None => 0.0,
            SidebarPanelChrome::Header(_) => 1.0,
            SidebarPanelChrome::HeaderAndSearch { .. } => 2.0,
        }
    }
}

/// A stateless, per-frame description of a sidebar panel's body:
/// background fill + optional header/search chrome + scrollbar gutter
/// reservation. Build one fresh per frame (same pattern as
/// [`crate::Panel`]/[`crate::Scrollbar`]) and call [`Self::render`] with
/// the body widget. See the module doc for the full rationale.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SidebarPanelBody {
    /// Flat fill painted under the entire panel rect before chrome/body.
    /// `None` paints no background (host's own background shows
    /// through).
    pub background: Option<Color>,
    /// Optional header/search chrome painted above the body.
    pub chrome: SidebarPanelChrome,
    /// Width, in native units, of a scrollbar gutter reserved on the
    /// right edge of the body rect. `None` (the default) reserves no
    /// gutter and gives the body widget the full width. A value `<= 0.0`
    /// or `>=` the panel's width is treated as `None` — reserving zero
    /// or negative body width would silently hide the body instead of
    /// signalling a caller bug.
    pub scrollbar_gutter: Option<f32>,
}

/// Resolved geometry for a [`SidebarPanelBody`] — the no-paint twin of
/// [`SidebarPanelBody::render`], following `docs/PRIMITIVE_RULES.md`
/// rule 7's paired `draw_<name>`/`<name>_layout` convention (this type
/// lives in `compose`, not on the `Backend` trait, because it composes
/// existing trait methods rather than adding a new rasteriser).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SidebarPanelBodyLayout {
    /// Bounds of the combined header/search chrome strip. `None` when
    /// [`SidebarPanelChrome::None`] or the panel rect has no height
    /// left for it.
    pub chrome_rect: Option<Rect>,
    /// Bounds the body widget is painted into — the panel rect minus
    /// the chrome strip and the scrollbar gutter.
    pub body_rect: Rect,
    /// Reserved scrollbar gutter, `body_rect`-height, on the right edge.
    /// `None` when [`SidebarPanelBody::scrollbar_gutter`] wasn't set (or
    /// was clamped away — see that field's doc).
    pub scrollbar_rect: Option<Rect>,
}

impl SidebarPanelBody {
    /// Compute this panel's layout in `rect` without painting anything.
    /// `unit_h` is the chrome row height in native units — pass
    /// [`crate::Backend::line_height`]. [`Self::render`] calls this
    /// internally with the real backend's `line_height()`, so paint and
    /// no-paint geometry can't drift apart.
    pub fn layout(&self, rect: Rect, unit_h: f32) -> SidebarPanelBodyLayout {
        let chrome_rows = self.chrome.rows();
        let chrome_h = (chrome_rows * unit_h.max(0.0)).min(rect.height.max(0.0));
        let chrome_rect = if chrome_rows > 0.0 && chrome_h > 0.0 {
            Some(Rect::new(rect.x, rect.y, rect.width, chrome_h))
        } else {
            None
        };

        let body_y = rect.y + chrome_h;
        let body_h = (rect.height - chrome_h).max(0.0);

        let gutter = self
            .scrollbar_gutter
            .filter(|w| *w > 0.0 && *w < rect.width);
        let (body_w, scrollbar_rect) = match gutter {
            Some(w) => (
                rect.width - w,
                Some(Rect::new(rect.x + rect.width - w, body_y, w, body_h)),
            ),
            None => (rect.width, None),
        };

        SidebarPanelBodyLayout {
            chrome_rect,
            body_rect: Rect::new(rect.x, body_y, body_w, body_h),
            scrollbar_rect,
        }
    }

    /// Paint background + chrome + `body` into `rect`, in that order,
    /// and return the resolved layout so the caller can paint its own
    /// scrollbar into `layout.scrollbar_rect` (see the module doc for
    /// why the scrollbar itself isn't painted here).
    pub fn render(
        &self,
        backend: &mut dyn Backend,
        rect: Rect,
        body: &dyn BackendWidget,
    ) -> SidebarPanelBodyLayout {
        let layout = self.layout(rect, backend.line_height());

        if let Some(bg) = self.background {
            backend.draw_solid_fill(rect, bg);
        }

        if let Some(chrome_rect) = layout.chrome_rect {
            match &self.chrome {
                SidebarPanelChrome::None => {}
                SidebarPanelChrome::Header(text) => {
                    backend.draw_settings_chrome(chrome_rect, text, "", "", false);
                }
                SidebarPanelChrome::HeaderAndSearch {
                    header,
                    query,
                    placeholder,
                    active,
                } => {
                    backend.draw_settings_chrome(chrome_rect, header, query, placeholder, *active);
                }
            }
        }

        body.render(backend, layout.body_rect);

        layout
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::RecordingBackend;

    struct RecordingWidget;

    impl BackendWidget for RecordingWidget {
        fn render(&self, backend: &mut dyn Backend, rect: Rect) {
            backend.draw_solid_fill(rect, Color::rgb(1, 2, 3));
        }
    }

    fn rect() -> Rect {
        Rect::new(0.0, 0.0, 40.0, 20.0)
    }

    // ── Layout geometry ─────────────────────────────────────────────

    #[test]
    fn no_chrome_no_scrollbar_body_fills_rect() {
        let panel = SidebarPanelBody::default();
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.chrome_rect, None);
        assert_eq!(layout.scrollbar_rect, None);
        assert_eq!(layout.body_rect, rect());
    }

    #[test]
    fn header_only_reserves_one_row() {
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::Header("DEBUG".into()),
            ..Default::default()
        };
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.chrome_rect, Some(Rect::new(0.0, 0.0, 40.0, 1.0)));
        assert_eq!(layout.body_rect, Rect::new(0.0, 1.0, 40.0, 19.0));
    }

    #[test]
    fn header_and_search_reserves_two_rows() {
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::HeaderAndSearch {
                header: "EXTENSIONS".into(),
                query: String::new(),
                placeholder: "Search".into(),
                active: false,
            },
            ..Default::default()
        };
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.chrome_rect, Some(Rect::new(0.0, 0.0, 40.0, 2.0)));
        assert_eq!(layout.body_rect, Rect::new(0.0, 2.0, 40.0, 18.0));
    }

    #[test]
    fn chrome_rows_use_pixel_unit_h() {
        // GTK-shaped: a real line height, not 1.0. Tall enough panel
        // rect that the two 18px chrome rows don't hit the clamp below.
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::HeaderAndSearch {
                header: "SETTINGS".into(),
                query: "q".into(),
                placeholder: String::new(),
                active: true,
            },
            ..Default::default()
        };
        let tall = Rect::new(0.0, 0.0, 240.0, 200.0);
        let layout = panel.layout(tall, 18.0);
        assert_eq!(layout.chrome_rect, Some(Rect::new(0.0, 0.0, 240.0, 36.0)));
        assert_eq!(layout.body_rect, Rect::new(0.0, 36.0, 240.0, 164.0));
    }

    #[test]
    fn scrollbar_gutter_reserves_right_column() {
        let panel = SidebarPanelBody {
            scrollbar_gutter: Some(1.0),
            ..Default::default()
        };
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.scrollbar_rect, Some(Rect::new(39.0, 0.0, 1.0, 20.0)));
        assert_eq!(layout.body_rect, Rect::new(0.0, 0.0, 39.0, 20.0));
    }

    #[test]
    fn scrollbar_gutter_and_chrome_compose() {
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::Header("EXT".into()),
            scrollbar_gutter: Some(2.0),
            ..Default::default()
        };
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.chrome_rect, Some(Rect::new(0.0, 0.0, 40.0, 1.0)));
        assert_eq!(layout.scrollbar_rect, Some(Rect::new(38.0, 1.0, 2.0, 19.0)));
        assert_eq!(layout.body_rect, Rect::new(0.0, 1.0, 38.0, 19.0));
    }

    #[test]
    fn zero_width_gutter_is_treated_as_none() {
        let panel = SidebarPanelBody {
            scrollbar_gutter: Some(0.0),
            ..Default::default()
        };
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.scrollbar_rect, None);
        assert_eq!(layout.body_rect, rect());
    }

    #[test]
    fn gutter_wider_than_rect_is_treated_as_none() {
        let panel = SidebarPanelBody {
            scrollbar_gutter: Some(1000.0),
            ..Default::default()
        };
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.scrollbar_rect, None);
        assert_eq!(layout.body_rect, rect());
    }

    #[test]
    fn chrome_taller_than_rect_is_clamped() {
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::HeaderAndSearch {
                header: "X".into(),
                query: String::new(),
                placeholder: String::new(),
                active: false,
            },
            ..Default::default()
        };
        let tiny = Rect::new(0.0, 0.0, 10.0, 1.0);
        let layout = panel.layout(tiny, 1.0);
        // Only 1 row of height is available — chrome takes it all, body
        // gets nothing, but nothing goes negative.
        assert_eq!(layout.chrome_rect, Some(Rect::new(0.0, 0.0, 10.0, 1.0)));
        assert_eq!(layout.body_rect, Rect::new(0.0, 1.0, 10.0, 0.0));
    }

    // ── Paint call order (RecordingBackend) ──────────────────────────

    #[test]
    fn render_paints_background_then_chrome_then_body_in_order() {
        let panel = SidebarPanelBody {
            background: Some(Color::rgb(10, 10, 10)),
            chrome: SidebarPanelChrome::HeaderAndSearch {
                header: "EXTENSIONS".into(),
                query: String::new(),
                placeholder: "Search".into(),
                active: false,
            },
            scrollbar_gutter: None,
        };
        let mut backend = RecordingBackend::new();
        panel.render(&mut backend, rect(), &RecordingWidget);

        // Background fill, then the settings-chrome row, then whatever
        // the body widget paints (here: another solid fill) — in that
        // order, with no scrollbar call since no gutter was requested.
        assert_eq!(
            backend.calls,
            vec!["draw_solid_fill", "draw_settings_chrome", "draw_solid_fill"]
        );
    }

    #[test]
    fn render_with_no_background_and_no_chrome_only_paints_body() {
        let panel = SidebarPanelBody::default();
        let mut backend = RecordingBackend::new();
        panel.render(&mut backend, rect(), &RecordingWidget);
        assert_eq!(backend.calls, vec!["draw_solid_fill"]);
    }

    #[test]
    fn render_header_only_chrome_does_not_call_settings_chrome_twice() {
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::Header("DEBUG".into()),
            ..Default::default()
        };
        let mut backend = RecordingBackend::new();
        panel.render(&mut backend, rect(), &RecordingWidget);
        assert_eq!(
            backend
                .calls
                .iter()
                .filter(|c| **c == "draw_settings_chrome")
                .count(),
            1
        );
    }

    #[test]
    fn render_returns_same_layout_as_layout_method() {
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::Header("DEBUG".into()),
            scrollbar_gutter: Some(1.0),
            ..Default::default()
        };
        let mut backend = RecordingBackend::new();
        let rendered = panel.render(&mut backend, rect(), &RecordingWidget);
        let computed = panel.layout(rect(), backend.line_height());
        assert_eq!(rendered, computed);
    }
}
