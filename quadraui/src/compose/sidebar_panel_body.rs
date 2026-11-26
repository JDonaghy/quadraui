//! `SidebarPanelBody` — composes the four layers every sidebar-panel
//! renderer hand-rolls: a background fill, optional header/search
//! chrome, the panel's own body widget, and an optional scrollbar
//! gutter.
//!
//! # Shape
//!
//! [`SidebarPanelBody`] is a stateless per-frame value, the same shape
//! as [`crate::SidebarPanel`] or [`crate::Scrollbar`] — hosts build one fresh
//! each frame from their own state (search query, focus, theme colour)
//! and call [`SidebarPanelBody::render`]. It owns no interaction state
//! of its own: scrolling, search-query editing, and body-widget
//! selection stay with whatever controller already manages the body
//! (`TreeController`, `FormController`, [`crate::SidebarSystem`], a raw
//! `TreeView`, …).
//!
//! The panel is a widget-contract [`crate::primitive::Primitive`]
//! (with no behaviour of its own, so `non_interactive!`):
//! [`SidebarPanelBody::render`] paints the background and chrome through
//! [`crate::DrawsPrimitives::draw_dyn`], then hands the body rect to the
//! caller's body widget.
//!
//! - **Background** — an optional flat fill under the whole panel rect,
//!   via `PaintSurface::surface_fill_rect` / `CellSurface::fill`. `None`
//!   paints nothing (the
//!   host's own background shows through, matching GTK's current
//!   "just call the body widget, no chrome" panels).
//! - **Chrome** — [`SidebarPanelChrome::None`] (no rows reserved),
//!   [`SidebarPanelChrome::Header`] (one title row), or
//!   [`SidebarPanelChrome::HeaderAndSearch`] (title row + filter-input
//!   row) — the Settings/Extensions shapes, painted by
//!   [`paint_settings_chrome`] / [`paint_settings_chrome_cells`], the
//!   same painters [`crate::Backend::draw_settings_chrome`] forwards to.
//!
//!   Two more shapes cover chrome the settings painters can't express
//!   - [`SidebarPanelChrome::Search`] — a filter row with **no** header
//!     above it. The settings painters' row 0 is unconditionally
//!     header-styled, so this paints a single synthetic
//!     [`crate::StatusBar`] segment through that bar's own `Primitive`
//!     instead.
//!   - [`SidebarPanelChrome::StatusBars`] — one row per
//!     [`crate::StatusBar`] (e.g. a title bar + an action bar), each
//!     painted through its own `Primitive` with
//!     its hit regions translated into panel-space and surfaced on
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
//!   an unbounded closure instead, for exactly that case.
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
//! `unit_h` (row height for chrome rows) is the
//! [`crate::primitive::LayoutCtx`]'s chrome row height — `1.0` cell on
//! TUI, the resolved chrome row pitch elsewhere — and the settings
//! painters paint at that same height, so the row-slicing can never
//! drift from what is painted.
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

use crate::a11y::{A11yChild, A11yInfo, A11yPrimitive, A11yRole};
use crate::backend::BackendWidget;
use crate::backend::DrawsPrimitives;
use crate::event::Rect;
use crate::paint_surface::PaintSurface;
use crate::primitive::{LayoutCtx, PaintEnv, Primitive};
use crate::primitives::status_bar::{
    StatusBar, StatusBarHit, StatusBarPrimitiveLayout, StatusBarSegment,
};
use crate::types::{Color, WidgetId};
use crate::Backend;

/// Optional header/search chrome painted above a [`SidebarPanelBody`]'s
/// body rect.
///
/// [`Self::Header`] and [`Self::HeaderAndSearch`] paint through
/// [`paint_settings_chrome`] / [`paint_settings_chrome_cells`] — see
/// those painters' docs for the exact row layout, `" / "`-prefixed prompt
/// construction, and placeholder logic. [`Self::Header`] reserves (and
/// passes) a 1-row `chrome_rect`; [`Self::HeaderAndSearch`] reserves 2
/// rows. The painters only paint the search row when the rect covers a
/// second row, so a 1-row `Header` rect paints header-only everywhere.
///
/// `#[non_exhaustive]` guards against downstream exhaustive matches
/// breaking when new variants are added.
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
    /// A title row followed by a search/filter input row — the
    /// Settings/Extensions shape.
    HeaderAndSearch {
        header: String,
        query: String,
        placeholder: String,
        /// Whether the search input has focus (drives caret + selected
        /// row-background tint — see [`paint_settings_chrome`]).
        active: bool,
    },
    /// A single search/filter row with **no** header row above it.
    /// Reserves one row, the same as [`Self::Header`].
    ///
    /// Painted via a single synthetic [`crate::StatusBar`] segment
    /// through that bar's own `Primitive` rather than the settings
    /// painters — their row 0 is unconditionally header-styled, so
    /// reusing them here would repaint a phantom header strip. [`crate::StatusBar`]/[`crate::StatusBarSegment`]
    /// always take caller-resolved colours (this composer has no theme of
    /// its own to consult — `Backend` exposes `set_theme`, not a getter),
    /// so `fg`/`bg` are resolved by the caller the same way
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
    /// painted through its own `Primitive` in order, and its hit regions are translated into panel-space and
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
/// [`crate::SidebarPanel`]/[`crate::Scrollbar`]) and call [`Self::render`] with
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
    /// `unit_h` is the chrome row height in native units. The panel's
    /// own `Primitive::layout` calls this with its `LayoutCtx`'s chrome
    /// row height, so paint and no-paint geometry can't drift apart.
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
    /// plain closure with no `Send`/`'static` bound.
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
    /// `Send + 'static` supertrait would otherwise shut out. `body` is
    /// called exactly once, with `layout.body_rect`.
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
        let painted = backend.draw_dyn(rect, self, &());
        let layout = painted
            .downcast_ref::<SidebarPanelBodyPrimitiveLayout>()
            .map(|p| p.panel.clone())
            .unwrap_or_else(|| self.layout(rect, backend.chrome_line_height()));

        body(backend, layout.body_rect);

        layout
    }

    /// The synthetic one-segment bar [`SidebarPanelChrome::Search`]
    /// paints its filter row with, or `None` for every other chrome.
    fn search_bar(&self) -> Option<StatusBar> {
        let SidebarPanelChrome::Search {
            query,
            placeholder,
            active,
            fg,
            bg,
        } = &self.chrome
        else {
            return None;
        };
        let show_placeholder = query.is_empty() && !placeholder.is_empty() && !*active;
        let text = if show_placeholder {
            format!(" / {placeholder}")
        } else {
            format!(" / {query}")
        };
        let row_bg = if *active { bg.lighten(0.08) } else { *bg };
        Some(StatusBar {
            id: WidgetId::new("sidebar-panel-body:search"),
            left_segments: vec![StatusBarSegment {
                text,
                fg: *fg,
                bg: row_bg,
                bold: false,
                action_id: None,
            }],
            right_segments: Vec::new(),
        })
    }

    /// The header text the settings-chrome painters draw, plus the search
    /// row's query, placeholder and focus, for the two chrome shapes they
    /// paint; `None` for every other chrome.
    fn settings_chrome(&self) -> Option<(&str, &str, &str, bool)> {
        match &self.chrome {
            SidebarPanelChrome::Header(text) => Some((text, "", "", false)),
            SidebarPanelChrome::HeaderAndSearch {
                header,
                query,
                placeholder,
                active,
            } => Some((header, query, placeholder, *active)),
            _ => None,
        }
    }
}

// ── Widget contract ──────────────────────────────────────────────────

/// [`Primitive::Layout`] for [`SidebarPanelBody`]: the panel geometry
/// plus every chrome status bar with its own resolved layout, so paint
/// draws each bar through its own [`Primitive`].
///
/// `#[non_exhaustive]`: produced by [`Primitive::layout`], never built by
/// a consumer via struct literal.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct SidebarPanelBodyPrimitiveLayout {
    /// The rect the panel was laid out into.
    pub area: Rect,
    /// The panel geometry, including
    /// [`SidebarPanelBodyLayout::status_bar_hit_regions`] for
    /// [`SidebarPanelChrome::StatusBars`].
    pub panel: SidebarPanelBodyLayout,
    /// The chrome row height the panel was sliced with.
    pub line_height: f32,
    /// Every chrome status bar with its resolved layout, in paint order:
    /// the synthetic filter row for [`SidebarPanelChrome::Search`], one
    /// entry per bar for [`SidebarPanelChrome::StatusBars`], none
    /// otherwise.
    pub bars: Vec<(StatusBar, StatusBarPrimitiveLayout)>,
}

impl A11yPrimitive for SidebarPanelBody {
    fn a11y_info(&self) -> A11yInfo {
        let label = match &self.chrome {
            SidebarPanelChrome::Header(text) => text.as_str(),
            SidebarPanelChrome::HeaderAndSearch { header, .. } => header.as_str(),
            _ => "Sidebar panel",
        };
        A11yInfo::new().with_role(A11yRole::Group).with_label(label)
    }

    fn a11y_children(&self) -> Vec<A11yChild> {
        match &self.chrome {
            SidebarPanelChrome::HeaderAndSearch {
                query, placeholder, ..
            }
            | SidebarPanelChrome::Search {
                query, placeholder, ..
            } => {
                let label = if query.is_empty() {
                    placeholder.as_str()
                } else {
                    query.as_str()
                };
                vec![A11yChild::new(
                    "search",
                    A11yInfo::new()
                        .with_role(A11yRole::TextInput)
                        .with_label(label),
                )]
            }
            SidebarPanelChrome::StatusBars(bars) => bars
                .iter()
                .enumerate()
                .map(|(i, bar)| {
                    A11yChild::new(format!("bar.{i}"), bar.a11y_info())
                        .with_children(bar.a11y_children())
                })
                .collect(),
            _ => Vec::new(),
        }
    }
}

impl Primitive for SidebarPanelBody {
    type Layout = SidebarPanelBodyPrimitiveLayout;
    type Event = crate::interactive::Never;

    /// Slice the chrome rows off the top at the ctx's chrome row height,
    /// then resolve each chrome status bar's own layout for its row.
    fn layout(&self, ctx: &LayoutCtx, rect: Rect) -> Self::Layout {
        let unit_h = ctx.line_height;
        let mut panel = SidebarPanelBody::layout(self, rect, unit_h);
        let mut bars = Vec::new();
        if let Some(chrome_rect) = panel.chrome_rect {
            if let Some(bar) = self.search_bar() {
                let bar_layout = Primitive::layout(&bar, ctx, chrome_rect);
                bars.push((bar, bar_layout));
            }
            if let SidebarPanelChrome::StatusBars(list) = &self.chrome {
                let mut row_y = chrome_rect.y;
                for bar in list {
                    let remaining = (chrome_rect.y + chrome_rect.height - row_y).max(0.0);
                    let row_h = unit_h.min(remaining);
                    if row_h <= 0.0 {
                        break;
                    }
                    let row_rect = Rect::new(chrome_rect.x, row_y, chrome_rect.width, row_h);
                    let bar_layout = Primitive::layout(bar, ctx, row_rect);
                    // Bar-local hit regions, translated into panel space.
                    for (r, hit) in &bar_layout.layout.hit_regions {
                        panel.status_bar_hit_regions.push((
                            Rect::new(row_rect.x + r.x, row_rect.y + r.y, r.width, r.height),
                            hit.clone(),
                        ));
                    }
                    bars.push((bar.clone(), bar_layout));
                    row_y += row_h;
                }
            }
        }
        SidebarPanelBodyPrimitiveLayout {
            area: rect,
            panel,
            line_height: unit_h,
            bars,
        }
    }

    /// Background, then the settings chrome through
    /// [`paint_settings_chrome`], then each chrome status bar through its
    /// own `Primitive`. The body is the caller's to paint into
    /// [`SidebarPanelBodyLayout::body_rect`].
    fn paint(&self, layout: &Self::Layout, surface: &mut dyn PaintSurface, env: &PaintEnv) {
        if let Some(bg) = self.background {
            surface.surface_fill_rect(layout.area, bg);
        }
        if let (Some(chrome_rect), Some((header, query, placeholder, active))) =
            (layout.panel.chrome_rect, self.settings_chrome())
        {
            paint_settings_chrome(
                surface,
                env.theme(),
                chrome_rect,
                layout.line_height,
                header,
                query,
                placeholder,
                active,
            );
        }
        for (bar, bar_layout) in &layout.bars {
            Primitive::paint(bar, bar_layout, surface, env);
        }
    }

    /// The cell-grid twin of [`Self::paint`]: the background through
    /// [`crate::CellSurface::fill`], the settings chrome through
    /// [`paint_settings_chrome_cells`], each status bar through its own
    /// `Primitive`.
    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &Self::Layout,
        cells: &mut dyn crate::CellSurface,
        env: &PaintEnv,
    ) {
        if let Some(bg) = self.background {
            cells.fill(
                layout.area,
                crate::cell_surface::CellStyle {
                    fg: Some(bg),
                    bg: Some(bg),
                    ..crate::cell_surface::CellStyle::default()
                },
            );
        }
        if let (Some(chrome_rect), Some((header, query, placeholder, active))) =
            (layout.panel.chrome_rect, self.settings_chrome())
        {
            paint_settings_chrome_cells(
                cells,
                env.theme(),
                chrome_rect,
                header,
                query,
                placeholder,
                active,
            );
        }
        for (bar, bar_layout) in &layout.bars {
            Primitive::paint_cells(bar, bar_layout, cells, env);
        }
    }
}

crate::non_interactive!(SidebarPanelBody);

// ── Settings chrome painters ─────────────────────────────────────────

/// Caret width for the settings-chrome search row on a pixel surface.
const SETTINGS_CURSOR_W: f32 = 1.5;
/// Prefix painted before the settings-chrome search query on a pixel
/// surface.
const SETTINGS_SEARCH_PREFIX: &str = " /  ";

/// Paint settings-panel chrome on a pixel surface: a header row and,
/// when `rect.height` leaves room for it, a search input row beneath it,
/// designed to sit immediately above a [`crate::Form`] body. Every pixel
/// backend's [`crate::Backend::draw_settings_chrome`] forwards here with
/// its own chrome-font surface, line height and theme.
///
/// The header row always paints at one `line_height` tall. The search
/// row paints only when `rect.height >= line_height * 1.5`, so a caller
/// reserving a single row (e.g. [`SidebarPanelChrome::Header`]) gets a
/// header-only strip instead of an unrequested second row painted past
/// its own rect. When the search row does paint, its full-width
/// background is `theme.selected_bg` when `active`, otherwise
/// `theme.tab_bar_bg`. Layout left to right: [`SETTINGS_SEARCH_PREFIX`]
/// in `theme.muted_fg`, then either `query` (in `theme.foreground`) or
/// `placeholder` (in `theme.muted_fg`, shown only when the query is
/// empty *and* the row is inactive). A [`SETTINGS_CURSOR_W`]-wide
/// `theme.accent_fg` caret follows the query when `active`. Text is
/// centred vertically in its row and snapped to a whole pixel.
///
/// Chrome only: the form body and any scrollbar layered below are
/// painted separately by the caller.
#[allow(clippy::too_many_arguments)]
pub fn paint_settings_chrome(
    surface: &mut dyn crate::PaintSurface,
    theme: &crate::Theme,
    rect: Rect,
    line_height: f32,
    header_text: &str,
    query: &str,
    placeholder: &str,
    active: bool,
) {
    if rect.width <= 0.0 || line_height <= 0.0 {
        return;
    }
    let paints_search_row = rect.height >= line_height * 1.5;
    let rows_h = if paints_search_row {
        line_height * 2.0
    } else {
        line_height
    };
    surface.surface_push_clip(Rect::new(rect.x, rect.y, rect.width, rows_h));

    let text_y =
        |row_y: f32, text_h: f32| (row_y + ((line_height - text_h) / 2.0).max(0.0)).round();

    // Row 0: header bar.
    surface.surface_fill_rect(
        Rect::new(rect.x, rect.y, rect.width, line_height),
        theme.header_bg,
    );
    let (header_w, header_h) = surface.surface_measure_text(header_text);
    surface.surface_draw_text_run(
        Rect::new(rect.x + 2.0, text_y(rect.y, header_h), header_w, header_h),
        header_text,
        theme.header_fg,
    );

    // Row 1: search input, only when the rect covers it.
    if paints_search_row {
        let search_y = rect.y + line_height;
        let row_bg = if active {
            theme.selected_bg
        } else {
            theme.tab_bar_bg
        };
        surface.surface_fill_rect(Rect::new(rect.x, search_y, rect.width, line_height), row_bg);

        let (prefix_w, prefix_h) = surface.surface_measure_text(SETTINGS_SEARCH_PREFIX);
        surface.surface_draw_text_run(
            Rect::new(rect.x + 2.0, text_y(search_y, prefix_h), prefix_w, prefix_h),
            SETTINGS_SEARCH_PREFIX,
            theme.muted_fg,
        );

        let q_x = rect.x + 2.0 + prefix_w;
        let show_placeholder = query.is_empty() && !placeholder.is_empty() && !active;
        let (text, color) = if show_placeholder {
            (placeholder, theme.muted_fg)
        } else if query.is_empty() {
            (query, theme.muted_fg)
        } else {
            (query, theme.foreground)
        };
        let (text_w, text_h) = surface.surface_measure_text(text);
        surface.surface_draw_text_run(
            Rect::new(q_x, text_y(search_y, text_h), text_w, text_h),
            text,
            color,
        );

        if active {
            let (query_w, _) = surface.surface_measure_text(query);
            let cur_x = q_x + if query.is_empty() { 0.0 } else { query_w };
            surface.surface_fill_rect(
                Rect::new(
                    cur_x,
                    search_y + 2.0,
                    SETTINGS_CURSOR_W,
                    (line_height - 4.0).max(0.0),
                ),
                theme.accent_fg,
            );
        }
    }

    surface.surface_pop_clip();
}

/// Paint settings-panel chrome on a cell grid: the cell counterpart of
/// [`paint_settings_chrome`], which [`crate::Backend::draw_settings_chrome`]
/// forwards to on TUI.
///
/// Row 0 is the header (`header_bg` / `header_fg`). Row 1, painted only
/// when `rect` is at least two rows tall, is the search input:
/// full-width `selected_bg` when `active`, otherwise `tab_bar_bg`, then
/// ` / ` in `muted_fg` followed by either `query` (in `foreground`) or
/// `placeholder` (in `muted_fg`, shown only when the query is empty and
/// the row is inactive). A one-cell `█` caret in `accent_fg` follows the
/// query when `active`.
#[allow(clippy::too_many_arguments)]
pub fn paint_settings_chrome_cells(
    surface: &mut dyn crate::CellSurface,
    theme: &crate::Theme,
    rect: Rect,
    header_text: &str,
    query: &str,
    placeholder: &str,
    active: bool,
) {
    let x0 = rect.x.max(0.0) as u16;
    let y0 = rect.y.max(0.0) as u16;
    let width = rect.width.max(0.0) as u16;
    let height = rect.height.max(0.0) as u16;
    if width == 0 || height == 0 {
        return;
    }
    let x_end = x0.saturating_add(width);
    let style = |fg, bg| crate::cell_surface::CellStyle {
        fg: Some(fg),
        bg: Some(bg),
        ..crate::cell_surface::CellStyle::default()
    };

    // Row 0: header.
    let header_style = style(theme.header_fg, theme.header_bg);
    for x in x0..x_end {
        surface.put_char(x, y0, ' ', header_style);
    }
    for (x, ch) in (x0..x_end).zip(header_text.chars()) {
        surface.put_char(x, y0, ch, header_style);
    }

    if height < 2 {
        return;
    }

    // Row 1: search input.
    let search_y = y0 + 1;
    let row_bg = if active {
        theme.selected_bg
    } else {
        theme.tab_bar_bg
    };
    for x in x0..x_end {
        surface.put_char(x, search_y, ' ', style(theme.foreground, row_bg));
    }

    let mut x = x0;
    for ch in [' ', '/', ' '] {
        if x >= x_end {
            break;
        }
        surface.put_char(x, search_y, ch, style(theme.muted_fg, row_bg));
        x += 1;
    }

    let show_placeholder = query.is_empty() && !placeholder.is_empty() && !active;
    let (text, fg) = if show_placeholder {
        (placeholder, theme.muted_fg)
    } else {
        (query, theme.foreground)
    };
    for ch in text.chars() {
        if x >= x_end {
            break;
        }
        surface.put_char(x, search_y, ch, style(fg, row_bg));
        x += 1;
    }

    if active && x < x_end {
        surface.put_char(x, search_y, '█', style(theme.accent_fg, row_bg));
    }
}

#[cfg(test)]
mod settings_chrome_tests {
    use super::*;
    use crate::backend::ImagePaintResult;
    use crate::event::Viewport;
    use crate::paint_surface::PaintSurface;
    use crate::types::Color;
    use crate::Image;
    use crate::Theme;

    /// Records every drawing verb `paint_settings_chrome` issues.
    #[derive(Default)]
    struct RecordingSurface {
        fills: Vec<(Rect, Color)>,
        text_runs: Vec<(Rect, String, Color)>,
    }

    impl PaintSurface for RecordingSurface {
        fn surface_begin_frame(&mut self, _viewport: Viewport) {}
        fn surface_end_frame(&mut self) {}
        fn surface_viewport(&self) -> Viewport {
            Viewport::new(300.0, 60.0, 1.0)
        }
        fn surface_line_height(&self) -> f32 {
            16.0
        }
        fn surface_char_width(&self) -> f32 {
            8.0
        }
        fn surface_measure_text(&self, text: &str) -> (f32, f32) {
            (text.chars().count() as f32 * 8.0, 14.0)
        }
        fn surface_fill_rect(&mut self, rect: Rect, color: Color) {
            self.fills.push((rect, color));
        }
        fn surface_fill_rounded_rect(&mut self, rect: Rect, _radius: f32, color: Color) {
            self.fills.push((rect, color));
        }
        fn surface_stroke_rect(&mut self, _rect: Rect, _color: Color, _stroke_width: f32) {}
        fn surface_stroke_rounded_rect(
            &mut self,
            _rect: Rect,
            _radius: f32,
            _color: Color,
            _stroke_width: f32,
        ) {
        }
        fn surface_draw_text_run(&mut self, rect: Rect, text: &str, color: Color) {
            self.text_runs.push((rect, text.to_string(), color));
        }
        fn surface_draw_line(
            &mut self,
            _from: crate::Point,
            _to: crate::Point,
            _color: Color,
            _stroke_width: f32,
        ) {
        }
        fn surface_push_clip(&mut self, _rect: Rect) {}
        fn surface_pop_clip(&mut self) {}
        fn surface_draw_image(&mut self, _rect: Rect, _image: &Image) -> ImagePaintResult {
            ImagePaintResult::Unsupported
        }
    }

    /// Lay `panel` out at 16px chrome rows and paint it through its own
    /// `Primitive` onto a recording surface.
    fn paint_panel(panel: &SidebarPanelBody, rect: Rect) -> RecordingSurface {
        let theme = Theme::default();
        let style = crate::Style::default();
        let ctx = LayoutCtx::new(16.0, 8.0, false);
        let layout = Primitive::layout(panel, &ctx, rect);
        let mut surface = RecordingSurface::default();
        Primitive::paint(panel, &layout, &mut surface, &PaintEnv::new(&theme, &style));
        surface
    }

    #[test]
    fn primitive_paints_background_first_then_the_settings_chrome() {
        let bg = Color::rgb(9, 9, 9);
        let rect = Rect::new(0.0, 0.0, 200.0, 120.0);
        let panel = SidebarPanelBody {
            background: Some(bg),
            chrome: SidebarPanelChrome::HeaderAndSearch {
                header: "EXTENSIONS".into(),
                query: String::new(),
                placeholder: "Search extensions".into(),
                active: false,
            },
            scrollbar_gutter: None,
        };
        let surface = paint_panel(&panel, rect);
        assert_eq!(surface.fills.first(), Some(&(rect, bg)));
        let texts: Vec<&str> = surface
            .text_runs
            .iter()
            .map(|(_, t, _)| t.as_str())
            .collect();
        assert!(texts.contains(&"EXTENSIONS"), "header painted: {texts:?}");
        assert!(
            texts.contains(&"Search extensions"),
            "placeholder painted: {texts:?}"
        );
    }

    #[test]
    fn primitive_paints_status_bar_chrome_through_each_bar() {
        let bar = |id: &str, text: &str| StatusBar {
            id: WidgetId::new(id),
            left_segments: vec![StatusBarSegment {
                text: text.into(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                action_id: Some(WidgetId::new(format!("{id}:act"))),
            }],
            right_segments: Vec::new(),
        };
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::StatusBars(vec![bar("t", "TITLE"), bar("a", "RUN")]),
            ..Default::default()
        };
        let rect = Rect::new(0.0, 0.0, 200.0, 120.0);
        let ctx = LayoutCtx::new(16.0, 8.0, false);
        let layout = Primitive::layout(&panel, &ctx, rect);
        assert_eq!(layout.bars.len(), 2);
        assert_eq!(layout.panel.status_bar_hit_regions.len(), 2);
        assert_eq!(layout.panel.status_bar_hit_regions[1].0.y, 16.0);

        // `StatusBar`'s pixel painter exists only on a pixel-backend build.
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        {
            let surface = paint_panel(&panel, rect);
            let texts: Vec<&str> = surface
                .text_runs
                .iter()
                .map(|(_, t, _)| t.as_str())
                .collect();
            let title = texts.iter().position(|t| t.contains("TITLE"));
            let run = texts.iter().position(|t| t.contains("RUN"));
            assert!(title.is_some() && run.is_some() && title < run, "{texts:?}");
        }
    }

    #[test]
    fn primitive_layout_matches_the_no_paint_geometry() {
        let panel = SidebarPanelBody {
            chrome: SidebarPanelChrome::Header("DEBUG".into()),
            scrollbar_gutter: Some(8.0),
            ..Default::default()
        };
        let rect = Rect::new(4.0, 2.0, 200.0, 120.0);
        let ctx = LayoutCtx::new(16.0, 8.0, false);
        let layout = Primitive::layout(&panel, &ctx, rect);
        assert_eq!(layout.panel, panel.layout(rect, 16.0));
    }

    fn paint_one_row(surface: &mut RecordingSurface, height: f32, active: bool) {
        paint_settings_chrome(
            surface,
            &Theme::default(),
            Rect::new(0.0, 0.0, 200.0, height),
            16.0,
            "Settings",
            "",
            "Search",
            active,
        );
    }

    /// A one-`line_height`-tall rect leaves no room for the search
    /// row — only the header paints.
    #[test]
    fn one_row_height_paints_no_search_row() {
        let mut surface = RecordingSurface::default();
        paint_one_row(&mut surface, 16.0, false);
        assert_eq!(surface.fills.len(), 1, "only the header bar should fill");
        assert!(surface.text_runs.iter().any(|(_, t, _)| t == "Settings"));
        assert!(!surface.text_runs.iter().any(|(_, t, _)| t == "Search"));
    }

    /// A two-`line_height`-tall rect leaves room for the search
    /// row, which shows the placeholder when `query` is empty and
    /// the row is inactive.
    #[test]
    fn two_row_height_paints_search_row_with_placeholder() {
        let mut surface = RecordingSurface::default();
        paint_one_row(&mut surface, 32.0, false);
        assert_eq!(
            surface.fills.len(),
            2,
            "header bar + search row should fill"
        );
        assert!(surface.text_runs.iter().any(|(_, t, _)| t == "Search"));
        assert!(surface
            .text_runs
            .iter()
            .any(|(_, t, _)| t == SETTINGS_SEARCH_PREFIX));
    }

    /// An active search row with a non-empty query paints the
    /// query (not the placeholder) plus a cursor fill.
    #[test]
    fn active_row_paints_query_and_cursor() {
        let mut surface = RecordingSurface::default();
        paint_settings_chrome(
            &mut surface,
            &Theme::default(),
            Rect::new(0.0, 0.0, 200.0, 32.0),
            16.0,
            "Settings",
            "abc",
            "Search",
            true,
        );
        let theme = Theme::default();
        assert!(surface.text_runs.iter().any(|(_, t, _)| t == "abc"));
        assert!(!surface.text_runs.iter().any(|(_, t, _)| t == "Search"));
        // Row bg + cursor are both fills distinct from the header fill.
        assert!(surface.fills.iter().any(|(_, c)| *c == theme.selected_bg));
        assert!(surface
            .fills
            .iter()
            .any(|(r, c)| *c == theme.accent_fg && r.width == SETTINGS_CURSOR_W));
    }

    /// Inactive row background is `tab_bar_bg`, not `selected_bg`.
    #[test]
    fn inactive_row_uses_tab_bar_bg() {
        let mut surface = RecordingSurface::default();
        paint_one_row(&mut surface, 32.0, false);
        let theme = Theme::default();
        assert!(surface.fills.iter().any(|(_, c)| *c == theme.tab_bar_bg));
        assert!(!surface.fills.iter().any(|(_, c)| *c == theme.selected_bg));
    }

    /// Zero-width rect is a no-op: nothing paints.
    #[test]
    fn zero_width_is_a_no_op() {
        let mut surface = RecordingSurface::default();
        paint_settings_chrome(
            &mut surface,
            &Theme::default(),
            Rect::new(0.0, 0.0, 0.0, 32.0),
            16.0,
            "Settings",
            "",
            "Search",
            false,
        );
        assert!(surface.fills.is_empty());
        assert!(surface.text_runs.is_empty());
    }

    /// Chrome paints at a non-zero origin, not hardcoded to (0, 0).
    #[test]
    fn paints_at_a_nonzero_origin() {
        let mut surface = RecordingSurface::default();
        paint_settings_chrome(
            &mut surface,
            &Theme::default(),
            Rect::new(7.0, 13.0, 200.0, 32.0),
            16.0,
            "Settings",
            "",
            "",
            false,
        );
        assert!(surface.fills.iter().all(|(r, _)| r.x == 7.0 && r.y >= 13.0));
    }

    #[cfg(feature = "tui")]
    mod cells {
        use super::super::paint_settings_chrome_cells;
        use crate::backend::ColorDepth;
        use crate::event::Rect;
        use crate::tui::cell_surface::TuiCellSurface;
        use crate::tui::ratatui_color;
        use crate::Theme;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect as RRect;

        fn paint(height: u16, query: &str, active: bool) -> Buffer {
            let mut buf = Buffer::empty(RRect::new(0, 0, 20, 3));
            let mut cursor = None;
            let mut surface = TuiCellSurface::over(&mut buf, ColorDepth::TrueColor, &mut cursor);
            paint_settings_chrome_cells(
                &mut surface,
                &Theme::default(),
                Rect::new(0.0, 0.0, 20.0, height as f32),
                "SETTINGS",
                query,
                "Search",
                active,
            );
            buf
        }

        fn row(buf: &Buffer, y: u16) -> String {
            (0..20).map(|x| buf[(x, y)].symbol().to_string()).collect()
        }

        #[test]
        fn one_row_paints_the_header_only() {
            let buf = paint(1, "", false);
            assert!(row(&buf, 0).starts_with("SETTINGS"));
            assert_eq!(
                buf[(0, 0)].bg,
                ratatui_color(Theme::default().header_bg),
                "the header row fills header_bg"
            );
            assert_eq!(row(&buf, 1).trim(), "", "no search row past a one-row rect");
            assert_ne!(buf[(0, 1)].bg, ratatui_color(Theme::default().tab_bar_bg));
        }

        #[test]
        fn two_rows_paint_the_placeholder_on_tab_bar_bg() {
            let buf = paint(2, "", false);
            assert!(row(&buf, 1).starts_with(" / Search"), "{:?}", row(&buf, 1));
            assert_eq!(buf[(5, 1)].bg, ratatui_color(Theme::default().tab_bar_bg));
            assert_eq!(buf[(5, 1)].fg, ratatui_color(Theme::default().muted_fg));
        }

        #[test]
        fn active_row_paints_the_query_and_a_caret_on_selected_bg() {
            let theme = Theme::default();
            let buf = paint(2, "ab", true);
            assert!(row(&buf, 1).starts_with(" / ab█"), "{:?}", row(&buf, 1));
            assert_eq!(buf[(3, 1)].fg, ratatui_color(theme.foreground));
            assert_eq!(buf[(5, 1)].fg, ratatui_color(theme.accent_fg));
            assert_eq!(buf[(0, 1)].bg, ratatui_color(theme.selected_bg));
        }
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

        // The background and settings chrome paint through the panel's
        // own `Primitive` (`draw_dyn`), so the only legacy call left is
        // the body widget's own fill.
        assert_eq!(backend.calls, vec!["draw_solid_fill"]);
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
        assert!(!backend.calls.contains(&"draw_settings_chrome"));
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
        let computed = panel.layout(rect(), backend.chrome_line_height());
        assert_eq!(rendered, computed);
    }

    // render_with: borrowed, non-Send, non-'static body

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
        assert_eq!(backend.calls, vec!["draw_solid_fill"]);
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

    // Search chrome: search-only row without header

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
    fn render_search_paints_background_then_one_status_bar_then_body() {
        let panel = SidebarPanelBody {
            background: Some(Color::rgb(10, 10, 10)),
            chrome: search_chrome("rust", "Search", true),
            ..Default::default()
        };
        let mut backend = RecordingBackend::new();
        panel.render(&mut backend, rect(), &RecordingWidget);

        // `draw_settings_chrome` must never fire for `Search` — its row 0
        // is unconditionally header-styled, which is exactly the
        // phantom-header bug this variant exists to avoid. The background
        // and search row paint through the panel's own `Primitive`, so
        // the only legacy call left is the body widget's own fill.
        assert_eq!(backend.calls, vec!["draw_solid_fill"]);
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

    // StatusBars chrome: title bar + action bars

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
        assert_eq!(backend.calls, vec!["draw_solid_fill"]);
    }

    /// The hit regions `render` surfaces on `SidebarPanelBodyLayout` must
    /// be exactly the hit regions each bar's widget layout resolved, for each
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
        // 1 unit_h) — resolve the same widget layout the paint used, and
        // translate by that row's origin, to get the expected entry.
        let bar_layout = backend
            .layout_primitive(Rect::new(0.0, 1.0, rect().width, 1.0), &actions)
            .layout;
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
