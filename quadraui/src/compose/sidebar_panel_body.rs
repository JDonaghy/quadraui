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
//!
//!   Two more shapes cover chrome `draw_settings_chrome` can't express
//!   (issue #1061):
//!   - [`SidebarPanelChrome::Search`] — a filter row with **no** header
//!     above it (vimcode's Extensions sidebar: the title comes from
//!     `AppShell`'s own sidebar-header row since vimcode#1343, so a
//!     second header here would be the double-header bug #1256).
//!     `draw_settings_chrome`'s row 0 is unconditionally header-styled,
//!     so this can't reuse it — it paints a single synthetic
//!     [`crate::StatusBar`] segment via
//!     [`crate::Backend::draw_status_bar_interactive`] instead, the
//!     same rasteriser every backend already implements.
//!   - [`SidebarPanelChrome::StatusBars`] — one row per
//!     [`crate::StatusBar`] (vimcode's Debug sidebar: a title bar +
//!     a Run/Stop action bar), each painted via
//!     [`crate::Backend::draw_status_bar_interactive`] with its hit
//!     regions translated into panel-space and surfaced on
//!     [`SidebarPanelBodyLayout::status_bar_hit_regions`], so the host
//!     routes clicks against the same geometry that was painted.
//! - **Body** — any [`crate::BackendWidget`] via [`SidebarPanelBody::render`],
//!   painted into the remaining rect after chrome and the scrollbar
//!   gutter are carved off. Same trait `BottomPanelConfig` already uses
//!   for tab content, so a host can share one widget between a
//!   bottom-panel tab and a sidebar panel if it wants to. `BackendWidget`
//!   carries a `Send + 'static` bound, so a body that only *borrows* the
//!   host's app state for one frame (a `TreeController`/`FormController`
//!   behind `Rc<RefCell<_>>`, anything tied to a `!Send` engine) can't
//!   implement it — [`SidebarPanelBody::render_with`] takes the body as
//!   an unbounded closure instead, for exactly that case (issue #1059).
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
use crate::interaction::InteractionState;
use crate::primitives::status_bar::{StatusBar, StatusBarHit, StatusBarSegment};
use crate::types::{Color, WidgetId};
use crate::Backend;

/// Optional header/search chrome painted above a [`SidebarPanelBody`]'s
/// body rect.
///
/// The two non-empty variants delegate straight to
/// [`crate::Backend::draw_settings_chrome`] — see that method's doc for
/// the exact row layout, `" / "`-prefixed prompt construction, and
/// placeholder logic. [`Self::Header`] reserves (and passes) a 1-row
/// `chrome_rect`; [`Self::HeaderAndSearch`] reserves 2 rows.
/// `draw_settings_chrome` is height-aware on every backend (issue #1041
/// review) — it only paints the search row when the rect it's given
/// covers a second row — so a 1-row `Header` rect reliably paints
/// header-only everywhere, not just on backends where that happened to
/// fall out of paint ordering.
///
/// `#[non_exhaustive]` as of issue #1061: this PR adds [`Self::Search`]
/// and [`Self::StatusBars`], which breaks any downstream exhaustive
/// `match` on this enum with no wildcard arm regardless of whether the
/// attribute is added (vimcode's `render::paint_sidebar_panel_chrome`
/// has exactly such a match — see this PR's `## Downstream impact`).
/// Marking it `#[non_exhaustive]` now, in the same breaking PR, is
/// `CLAUDE.md` rule 2's preferred shape so the *next* variant addition
/// is additive instead of breaking again.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
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
    /// A single search/filter row with **no** header row above it —
    /// e.g. vimcode's Extensions sidebar, whose title now comes from
    /// `AppShell`'s own sidebar-header row (issue #1061). Reserves one
    /// row, the same as [`Self::Header`].
    ///
    /// Painted via a single synthetic [`crate::StatusBar`] segment
    /// through [`crate::Backend::draw_status_bar_interactive`] rather
    /// than `draw_settings_chrome` — that method's row 0 is
    /// unconditionally header-styled, so reusing it here would repaint
    /// exactly the phantom header strip issue #1256 already fixed.
    /// [`crate::StatusBar`]/[`crate::StatusBarSegment`] always take
    /// caller-resolved colours (this composer has no theme of its own
    /// to consult — `Backend` exposes `set_theme`, not a getter), so
    /// `fg`/`bg` are resolved by the caller the same way
    /// [`SidebarPanelBody::background`] already is.
    Search {
        query: String,
        placeholder: String,
        /// Whether the search input has focus (drives the row's
        /// background tint, lightened from `bg`).
        active: bool,
        /// Text colour for the query/placeholder text.
        fg: Color,
        /// Base row background colour (tinted lighter while `active`).
        bg: Color,
    },
    /// One row per [`crate::StatusBar`] — e.g. vimcode's Debug sidebar
    /// chrome (a title bar + a Run/Stop action bar). Each bar is
    /// painted via [`crate::Backend::draw_status_bar_interactive`] in
    /// order, and its hit regions are translated into panel-space and
    /// appended to
    /// [`SidebarPanelBodyLayout::status_bar_hit_regions`] — the same
    /// resolved geometry the paint used, so the host can route clicks
    /// against it directly instead of re-deriving per-bar row
    /// arithmetic itself.
    StatusBars(Vec<StatusBar>),
}

impl SidebarPanelChrome {
    /// Number of chrome rows this variant reserves, in `unit_h` units.
    fn rows(&self) -> f32 {
        match self {
            SidebarPanelChrome::None => 0.0,
            SidebarPanelChrome::Header(_) => 1.0,
            SidebarPanelChrome::HeaderAndSearch { .. } => 2.0,
            SidebarPanelChrome::Search { .. } => 1.0,
            SidebarPanelChrome::StatusBars(bars) => bars.len() as f32,
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
#[derive(Debug, Clone, PartialEq)]
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
    /// Hit regions for [`SidebarPanelChrome::StatusBars`], one entry per
    /// clickable segment across every bar, in panel-space (the same
    /// coordinate space as `chrome_rect`/`body_rect`, i.e. already
    /// offset by the rect passed to [`SidebarPanelBody::layout`] /
    /// [`SidebarPanelBody::render`]). Empty for every other
    /// [`SidebarPanelChrome`] variant.
    ///
    /// Only [`SidebarPanelBody::render`] / [`SidebarPanelBody::render_with`]
    /// populate this — [`SidebarPanelBody::layout`] has no `Backend` to
    /// measure segment widths against, so its no-paint twin always
    /// returns this empty even for `StatusBars` chrome.
    pub status_bar_hit_regions: Vec<(Rect, StatusBarHit)>,
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
            // No `Backend` here to measure real segment widths against —
            // see the field's doc. `render`/`render_with` fill this in.
            status_bar_hit_regions: Vec::new(),
        }
    }

    /// Paint background + chrome + `body` into `rect`, in that order,
    /// and return the resolved layout so the caller can paint its own
    /// scrollbar into `layout.scrollbar_rect` (see the module doc for
    /// why the scrollbar itself isn't painted here).
    ///
    /// `body` must be [`BackendWidget`], i.e. `Send + 'static` — that
    /// bound exists so *owned* content can live inside `ShellAdapter`
    /// (`BottomPanelConfig`, `TabGroupController`'s `PaneTab::content`)
    /// and cross into the runner thread. A body that only *borrows* the
    /// host's app state for one frame — `Rc<RefCell<_>>`-backed
    /// controllers like `TreeController`/`FormController`, or anything
    /// tied to a `!Send` engine — can never satisfy it. Use
    /// [`Self::render_with`] instead; it paints the identical
    /// background/chrome/body/gutter sequence but takes the body as a
    /// plain closure with no `Send`/`'static` bound (issue #1059).
    pub fn render(
        &self,
        backend: &mut dyn Backend,
        rect: Rect,
        body: &dyn BackendWidget,
    ) -> SidebarPanelBodyLayout {
        self.render_with(backend, rect, |b, r| body.render(b, r))
    }

    /// Same paint sequence as [`Self::render`] — background, then
    /// chrome, then the body — but takes `body` as a borrowed closure
    /// instead of a `&dyn BackendWidget`, so it can close over
    /// non-`'static`, non-`Send` state (a `&TreeController`, an
    /// `Rc<RefCell<Engine>>` borrow, …) that [`BackendWidget`]'s
    /// `Send + 'static` supertrait would otherwise shut out (issue
    /// #1059). `body` is called exactly once, with `layout.body_rect`.
    ///
    /// Prefer [`Self::render`] when the body is already a
    /// [`BackendWidget`] (e.g. it's shared with a `BottomPanelConfig`
    /// tab) — `render` is implemented in terms of this method, so the
    /// two can never drift apart.
    ///
    /// ```
    /// use quadraui::compose::sidebar_panel_body::SidebarPanelBody;
    /// use quadraui::testing::RecordingBackend;
    /// use quadraui::{Color, Rect};
    /// use std::cell::RefCell;
    /// use std::rc::Rc;
    ///
    /// // Not `Send`, not `'static` — could never be a `&dyn BackendWidget`.
    /// let state = Rc::new(RefCell::new(vec!["one".to_string(), "two".to_string()]));
    ///
    /// let panel = SidebarPanelBody::default();
    /// let mut backend = RecordingBackend::new();
    /// panel.render_with(&mut backend, Rect::new(0.0, 0.0, 40.0, 20.0), |b, rect| {
    ///     // Borrow the shared, `!Send` state right here, mid-frame.
    ///     let rows = state.borrow();
    ///     if !rows.is_empty() {
    ///         b.draw_solid_fill(rect, Color::rgb(30, 30, 30));
    ///     }
    /// });
    /// assert_eq!(backend.calls, vec!["draw_solid_fill"]);
    /// ```
    pub fn render_with(
        &self,
        backend: &mut dyn Backend,
        rect: Rect,
        body: impl FnOnce(&mut dyn Backend, Rect),
    ) -> SidebarPanelBodyLayout {
        let unit_h = backend.line_height();
        let mut layout = self.layout(rect, unit_h);

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
                SidebarPanelChrome::Search {
                    query,
                    placeholder,
                    active,
                    fg,
                    bg,
                } => {
                    let show_placeholder = query.is_empty() && !placeholder.is_empty() && !*active;
                    let text = if show_placeholder {
                        format!(" / {placeholder}")
                    } else {
                        format!(" / {query}")
                    };
                    let row_bg = if *active { bg.lighten(0.08) } else { *bg };
                    let search_bar = StatusBar {
                        id: WidgetId::new("sidebar-panel-body:search"),
                        left_segments: vec![StatusBarSegment {
                            text,
                            fg: *fg,
                            bg: row_bg,
                            bold: false,
                            action_id: None,
                        }],
                        right_segments: Vec::new(),
                    };
                    let _ = backend.draw_status_bar_interactive(
                        chrome_rect,
                        &search_bar,
                        &InteractionState::new(),
                    );
                }
                SidebarPanelChrome::StatusBars(bars) => {
                    let interaction = InteractionState::new();
                    let mut row_y = chrome_rect.y;
                    for bar in bars {
                        let remaining = (chrome_rect.y + chrome_rect.height - row_y).max(0.0);
                        let row_h = unit_h.min(remaining);
                        if row_h <= 0.0 {
                            break;
                        }
                        let row_rect = Rect::new(chrome_rect.x, row_y, chrome_rect.width, row_h);
                        let bar_layout =
                            backend.draw_status_bar_interactive(row_rect, bar, &interaction);
                        for (r, hit) in bar_layout.hit_regions {
                            layout.status_bar_hit_regions.push((
                                Rect::new(row_rect.x + r.x, row_rect.y + r.y, r.width, r.height),
                                hit,
                            ));
                        }
                        row_y += row_h;
                    }
                }
            }
        }

        body(backend, layout.body_rect);

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

    // ── render_with (issue #1059: borrowed, non-Send, non-'static body) ─

    #[test]
    fn render_with_paints_borrowed_rc_refcell_state() {
        use std::cell::RefCell;
        use std::rc::Rc;

        // `Rc<RefCell<_>>` is neither `Send` nor `'static`-owned by the
        // closure below (it's captured by reference) — this state could
        // never back a `&dyn BackendWidget` for `SidebarPanelBody::render`.
        let rows = Rc::new(RefCell::new(vec!["alpha".to_string(), "beta".to_string()]));

        let panel = SidebarPanelBody {
            background: Some(Color::rgb(5, 5, 5)),
            chrome: SidebarPanelChrome::Header("TREE".into()),
            ..Default::default()
        };
        let mut backend = RecordingBackend::new();
        let layout = panel.render_with(&mut backend, rect(), |b, body_rect| {
            // Mutate through the shared, `!Send` handle mid-frame, then
            // paint based on what's there — exactly the pattern a
            // `TreeController` body needs and `BackendWidget` forbids.
            rows.borrow_mut().push("gamma".to_string());
            if !rows.borrow().is_empty() {
                b.draw_solid_fill(body_rect, Color::rgb(1, 2, 3));
            }
        });

        assert_eq!(rows.borrow().len(), 3);
        assert_eq!(
            backend.calls,
            vec!["draw_solid_fill", "draw_settings_chrome", "draw_solid_fill"]
        );
        assert_eq!(layout, panel.layout(rect(), 1.0));
    }

    #[test]
    fn render_with_body_receives_body_rect_not_full_rect() {
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::Header("DEBUG".into()),
            scrollbar_gutter: Some(1.0),
            ..Default::default()
        };
        let mut backend = RecordingBackend::new();
        let mut seen_rect = None;
        let layout = panel.render_with(&mut backend, rect(), |_b, body_rect| {
            seen_rect = Some(body_rect);
        });
        assert_eq!(seen_rect, Some(layout.body_rect));
        assert_ne!(seen_rect, Some(rect()));
    }

    // ── Search chrome (issue #1061: search-only row, no header) ──────

    fn search_chrome(query: &str, placeholder: &str, active: bool) -> SidebarPanelChrome {
        SidebarPanelChrome::Search {
            query: query.into(),
            placeholder: placeholder.into(),
            active,
            fg: Color::rgb(200, 200, 200),
            bg: Color::rgb(30, 30, 40),
        }
    }

    #[test]
    fn search_reserves_one_row() {
        let panel = SidebarPanelBody {
            chrome: search_chrome("", "Search extensions", false),
            ..Default::default()
        };
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.chrome_rect, Some(Rect::new(0.0, 0.0, 40.0, 1.0)));
        assert_eq!(layout.body_rect, Rect::new(0.0, 1.0, 40.0, 19.0));
        assert!(layout.status_bar_hit_regions.is_empty());
    }

    #[test]
    fn render_search_paints_background_then_one_status_bar_call_then_body() {
        let panel = SidebarPanelBody {
            background: Some(Color::rgb(10, 10, 10)),
            chrome: search_chrome("rust", "Search", true),
            ..Default::default()
        };
        let mut backend = RecordingBackend::new();
        panel.render(&mut backend, rect(), &RecordingWidget);

        // `draw_settings_chrome` must never fire for `Search` — its row 0
        // is unconditionally header-styled, which is exactly the
        // phantom-header bug (#1256) this variant exists to avoid.
        assert_eq!(
            backend.calls,
            vec!["draw_solid_fill", "draw_status_bar", "draw_solid_fill"]
        );
    }

    #[test]
    fn render_search_does_not_call_settings_chrome() {
        let panel = SidebarPanelBody {
            chrome: search_chrome("", "", false),
            ..Default::default()
        };
        let mut backend = RecordingBackend::new();
        panel.render(&mut backend, rect(), &RecordingWidget);
        assert!(!backend.calls.contains(&"draw_settings_chrome"));
    }

    // ── StatusBars chrome (issue #1061: Debug sidebar title + action bars) ─

    fn status_bar_with_segment(id: &str, seg_text: &str, action_id: Option<&str>) -> StatusBar {
        StatusBar {
            id: WidgetId::new(id),
            left_segments: vec![StatusBarSegment {
                text: seg_text.into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(20, 20, 20),
                bold: false,
                action_id: action_id.map(WidgetId::new),
            }],
            right_segments: Vec::new(),
        }
    }

    #[test]
    fn status_bars_reserves_one_row_per_bar() {
        let bars = vec![
            status_bar_with_segment("debug:title", "DEBUG", None),
            status_bar_with_segment("debug:actions", "RUN", Some("debug:run")),
        ];
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::StatusBars(bars),
            ..Default::default()
        };
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.chrome_rect, Some(Rect::new(0.0, 0.0, 40.0, 2.0)));
        assert_eq!(layout.body_rect, Rect::new(0.0, 2.0, 40.0, 18.0));
        // `layout()` has no `Backend` to measure real segment widths
        // against (see the field's doc) — only `render`/`render_with`
        // populate hit regions.
        assert!(layout.status_bar_hit_regions.is_empty());
    }

    #[test]
    fn status_bars_with_no_bars_reserves_nothing() {
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::StatusBars(Vec::new()),
            ..Default::default()
        };
        let layout = panel.layout(rect(), 1.0);
        assert_eq!(layout.chrome_rect, None);
        assert_eq!(layout.body_rect, rect());
    }

    #[test]
    fn render_status_bars_paints_one_call_per_bar_in_order() {
        let bars = vec![
            status_bar_with_segment("debug:title", "DEBUG", None),
            status_bar_with_segment("debug:actions", "RUN", Some("debug:run")),
        ];
        let panel = SidebarPanelBody {
            background: Some(Color::rgb(1, 1, 1)),
            chrome: SidebarPanelChrome::StatusBars(bars),
            ..Default::default()
        };
        let mut backend = RecordingBackend::new();
        panel.render(&mut backend, rect(), &RecordingWidget);
        assert_eq!(
            backend.calls,
            vec![
                "draw_solid_fill",
                "draw_status_bar",
                "draw_status_bar",
                "draw_solid_fill",
            ]
        );
    }

    /// The hit regions `render` surfaces on `SidebarPanelBodyLayout` must
    /// be exactly what `draw_status_bar_interactive` returned for each
    /// bar, translated from that bar's own bar-local coordinates into
    /// panel-space by adding the row's origin — nothing recomputed, no
    /// drift from what was actually painted.
    #[test]
    fn render_status_bars_surfaces_hit_regions_translated_into_panel_space() {
        let title = status_bar_with_segment("debug:title", "DEBUG", None);
        let actions = status_bar_with_segment("debug:actions", "RUN", Some("debug:run"));
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::StatusBars(vec![title, actions.clone()]),
            ..Default::default()
        };
        let mut backend = RecordingBackend::new(); // char_width = 1.0, line_height = 1.0
        let layout = panel.render(&mut backend, rect(), &RecordingWidget);

        // The title bar has no clickable segment, so it contributes no
        // hit regions. The actions bar is the second row (y offset =
        // 1 unit_h) — recompute the same char-cell layout
        // `RecordingBackend::draw_status_bar_interactive` computes, and
        // translate by that row's origin, to get the expected entry.
        let bar_layout = actions.layout(rect().width, 1.0, 2.0, |seg| {
            crate::StatusSegmentMeasure::new(seg.text.chars().count() as f32)
        });
        let (bar_local_rect, hit) = bar_layout.hit_regions[0].clone();
        let expected = (
            Rect::new(
                bar_local_rect.x,
                1.0 + bar_local_rect.y,
                bar_local_rect.width,
                bar_local_rect.height,
            ),
            hit,
        );
        assert_eq!(layout.status_bar_hit_regions, vec![expected]);
    }

    #[test]
    fn render_delegates_to_render_with_identically() {
        // `render`'s existing `&dyn BackendWidget` call sites must see no
        // behaviour change now that it's implemented via `render_with`.
        let panel = SidebarPanelBody {
            background: Some(Color::rgb(10, 10, 10)),
            chrome: SidebarPanelChrome::HeaderAndSearch {
                header: "EXTENSIONS".into(),
                query: String::new(),
                placeholder: "Search".into(),
                active: false,
            },
            scrollbar_gutter: Some(1.0),
        };

        let mut via_render = RecordingBackend::new();
        let layout_a = panel.render(&mut via_render, rect(), &RecordingWidget);

        let mut via_render_with = RecordingBackend::new();
        let layout_b = panel.render_with(&mut via_render_with, rect(), |b, r| {
            RecordingWidget.render(b, r)
        });

        assert_eq!(layout_a, layout_b);
        assert_eq!(via_render.calls, via_render_with.calls);
    }
}
