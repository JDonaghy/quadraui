//! [`Style`] — non-colour visual tokens (spacing, radius, border width,
//! focus-ring weight, …), the geometry half of theming that
//! [`crate::Theme`] deliberately does not cover.
//!
//! # Why this is a separate struct, not new `Theme` fields
//!
//! `Theme`'s own module doc already states the rule: adding a `pub`
//! field to `Theme` is a breaking change, because `coord-tui` builds
//! three of its four palettes with exhaustive struct literals and no
//! `..Default::default()` spread. A flat colour struct and a flat
//! geometry struct would share that exact liability, for no benefit —
//! colour and geometry are independent axes an app may want to override
//! separately (a high-contrast *colour* theme with the *same* spacing,
//! or a compact-density *geometry* preset layered under three different
//! colour themes). Keeping them as two independently defaulted types
//! means a consumer who only ever wants to touch colours — today, both
//! of them — never has to look at this file at all, and a later field
//! goes through the ordinary additive path this file sets up from its
//! first commit.
//!
//! # Where each token maps on TUI
//!
//! D-014 in `docs/decisions/DECISIONS.md` requires every new capability
//! to declare its TUI story. Every [`Style`] token's is the same shape,
//! stated once here rather than per-field: TUI paints a whole-cell
//! grid, so a sub-cell geometry token has nothing to refine — see
//! [`crate::paint_surface`]'s module doc, "Why TUI stays out", for the
//! identical reasoning applied to the drawing-verb trait this struct's
//! tokens are consumed through. Concretely:
//!
//! - **No-op** — the token has no effect because the TUI rasteriser
//!   for that primitive doesn't read [`Style`] at all. [`Self::focus_ring_width`]
//!   is this: [`crate::tui::draw_focus_ring`] always paints a 1-cell
//!   box-drawing border regardless of what a GUI backend's style carries
//!   (its own doc already says so — this struct doesn't change that).
//! - **Cell-quantised** — a spacing/padding token rounds to whole cells
//!   rather than being dropped outright, the same way
//!   `LayoutMetrics::cell_quantum` already quantises hit-test geometry
//!   (`docs/decisions/DECISIONS.md` D-016's "Paint/click drift" note).
//!   [`Self::row_padding_x`]/[`Self::tab_padding_x`] are this — see
//!   their own docs for the exact rounding rule.
//! - **Pre-blend** — a token whose pixel-backend refinement is alpha
//!   compositing, which ratatui's `Buffer` has no primitive for.
//!   [`Self::backdrop_opacity`] is this: [`crate::tui::draw_palette`]
//!   pre-mixes the backdrop colour on the CPU via [`crate::Color::blend`]
//!   before writing opaque cells, the same technique
//!   [`crate::tui::draw_minimap`] already uses to simulate a translucent
//!   highlight tint.
//!
//! No [`Style`] token is physically absent on TUI the way a tray icon
//! or dock badge is (D-014's **N/A** tier) — every one of them is a
//! pixel-only *refinement* with a well-defined cell-grid fallback, so
//! **no-op**/**cell-quantised**/**pre-blend** cover the whole token set
//! by construction, not by accident.
//!
//! # Scope — the fixed VS Code token set
//!
//! This struct carries [`Self::focus_ring_width`] plus the rest of the
//! catalogue `CLAUDE.md` wants — padding, corner radius, border width,
//! shadow elevation and control height — as one fixed, VS-Code-shaped
//! set rather than a per-widget style sheet: every overlay primitive
//! reads the same five shared tokens, the same way every one of them
//! reads the same five-ish [`crate::Theme`] colours, instead of each
//! carrying its own bespoke radius/padding constant.
//! [`Self::backdrop_opacity`] is the one exception — only
//! [`crate::Palette`] reads it, because it is the one overlay whose
//! backdrop ever needs to be anything but fully opaque; see that field's
//! own doc. [`Self::default`]'s values are chosen to match VS Code's own
//! chrome metrics (hover cards, the quick-pick/command-palette widget,
//! notifications, menus) — see each field's own doc for the specific VS
//! Code surface it mirrors.
//!
//! [`Self::scrollbar_width`] is a further single-owner token, the same
//! shape as [`Self::backdrop_opacity`]: only `List`/`TreeView`/
//! `DataTable`'s vertical scrollbar reads it (once a future PR wires
//! the rasterisers to it — see that field's own doc), independent of
//! the chrome-metric group's `row_height`.
//!
//! A second group of five tokens — [`Self::tab_height`],
//! [`Self::status_bar_height`], [`Self::activity_bar_width`],
//! [`Self::row_height`] and [`Self::toolbar_height`] — sizes shell
//! *chrome* (`AppShell`'s tab strip/status bar/activity bar/sidebar
//! header, `SidebarPanel`'s own header, and `List`/`TreeView`/
//! `DataTable` row pitch) rather than overlay padding/radius/border; no
//! overlay primitive reads them. They follow the same cell-grid-no-op
//! TUI story as the rest of this struct — see each field's own
//! "TUI story" paragraph.
//!
//! [`Self::row_padding_x`] and [`Self::tab_padding_x`] are a third
//! group of two: horizontal content insets rather than chrome sizes or
//! overlay padding/radius/border. `row_padding_x` is read by the one
//! shared [`crate::paint_surface::PaintSurface`] painter `List`/
//! `TreeView`/`ContextMenu`/`Palette` all route every GTK/macOS/Win32
//! rasteriser through; `tab_padding_x` is read by each backend's own
//! `tab_bar.rs` painter instead, since tab-strip painting isn't on that
//! shared path. Both are the cell-quantised TUI case the mapping
//! section above describes — see each field's own doc for the rounding
//! rule.
//!
//! Each further token lands as its own additive field, the same shape
//! [`Self::focus_ring_width`] already has — `docs/decisions/DECISIONS.md`
//! D-017's one-token/primitive-at-a-time discipline applies to *adding*
//! a token, not to how many existing tokens a single consuming change
//! may read; `Toast`/`Tooltip`/`ContextMenu`/`Palette`/`Dialog`/
//! `Spinner`/`ProgressBar` all read these five together because that is
//! the one set of overlays the owner decision names, not a batch of
//! unrelated hardcoded-literal cleanups (`PRIMITIVE_RULES.md` rule 4 is
//! about unrelated changes riding one PR, not about a single design
//! decision's consistent rollout).
//!
//! See `docs/decisions/DECISIONS.md` D-017 for why this is a separate
//! struct and the TUI-mapping rule, and D-020 for the fixed token-set
//! catalogue and the verbs each token is painted through.

use serde::{Deserialize, Serialize};

/// Non-colour visual tokens shared by every pixel backend. See the
/// module doc for why this is a separate type from [`crate::Theme`],
/// which rasterisers that want styling read alongside it the same way
/// [`crate::Backend::style`] pairs with [`crate::Backend::theme`].
///
/// `#[non_exhaustive]`, from this struct's first commit: every future
/// token (padding, corner radius, border width, …) lands as a purely
/// additive field, with no retrofit later needed — the same
/// `Default`/`with_*` shape `TextInput`, `Toolbar` and `Editor` carry
/// (see [`Style::default`] and
/// [`Style::with_focus_ring_width`]). Unlike a plain struct,
/// `#[non_exhaustive]` blocks *any* struct-literal construction from
/// outside this crate — including `Style { focus_ring_width: 4.0,
/// ..Default::default() }` — so a downstream consumer always goes
/// through `Default::default()` plus a `with_*` builder, which is the
/// construction shape that keeps compiling as fields are added. Compare
/// `Theme`'s own doc, which explains why it *can't* adopt this attribute
/// while `coord-tui` builds it with exhaustive literals — `Style`
/// avoids that trap only because it is new.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Style {
    /// Stroke width, in surface-native pixels, of the focus ring
    /// [`crate::Backend::draw_focus_ring`] paints on every pixel
    /// backend. Consumed via [`crate::paint_surface::PaintSurface::surface_stroke_rect`]
    /// — see that trait's module doc for why this reaches `GtkBackend`,
    /// `MacBackend` and `WinBackend` from one call site.
    ///
    /// **TUI story: no-op.** [`crate::tui::draw_focus_ring`] always
    /// paints a 1-cell box-drawing border; there is no sub-cell stroke
    /// weight on a terminal grid to refine. See the module doc's *Where
    /// each token maps on TUI* section.
    ///
    /// Default `2.0` — the one stroke width all three pixel backends
    /// agree on, so [`Style::default()`] paints the stock focus ring.
    pub focus_ring_width: f32,

    /// Inner spacing, in surface-native pixels, between an overlay's
    /// border and its content — the gap `Toast`, `Tooltip`,
    /// `ContextMenu`, `Palette` and `Dialog` reserve on every edge
    /// before laying out their label/list/button content.
    ///
    /// **TUI story: cell-quantised.** A sub-cell padding has no grid
    /// position to paint into; a TUI rasteriser that wants to honour this
    /// token rounds it up to whole cells first — `ceil(padding /
    /// nominal_cell_px)`, **not** `padding / Backend::char_width()`:
    /// `char_width`/`line_height` report `1.0` on a cell-grid backend (the
    /// native unit already *is* one cell, architecturally) rather than a
    /// real font pixel size, so they can't supply the pixel reference this
    /// formula needs; a nominal "one cell ≈ 8×16px" estimate stands in
    /// instead, the same way `LayoutMetrics::cell_quantum` already
    /// quantises hit-test geometry. No TUI primitive reads this field yet
    /// — every one of the six overlay primitives' TUI rasterisers keeps
    /// its own pre-existing fixed cell gutter — so today this is a
    /// documented fallback rule, not yet a live call site.
    /// `quadraui-binding-core`'s own `layout::Spacing` (the
    /// `padding`/`margin`/`gap` keys a Python/JS render callback's
    /// `"row"`/`"column"` layout-tree node may declare) is a *live* call
    /// site of this identical rule, nominal constant and all — see that
    /// type's own doc for why ceiling to a whole native unit degrades
    /// correctly on both a pixel backend and TUI from the same formula.
    ///
    /// Default `8.0` — VS Code's own hover-card/quick-pick body padding.
    pub padding: f32,

    /// Corner radius, in surface-native pixels, every rounded-rect
    /// overlay rasterises its box with — fed to
    /// [`crate::paint_surface::PaintSurface::surface_fill_rounded_rect`]/
    /// [`crate::paint_surface::PaintSurface::surface_stroke_rounded_rect`].
    ///
    /// **TUI story: no-op.** A terminal cell grid has no sub-cell corner
    /// to round — the TUI rasteriser for every primitive this token
    /// touches keeps painting its square box-drawing border exactly as
    /// before. See the module doc's *Where each token maps on TUI*.
    ///
    /// Default `6.0` — VS Code's own widget corner radius (hover,
    /// quick-pick, notifications, context menus).
    pub corner_radius: f32,

    /// Border stroke width, in surface-native pixels, every bordered
    /// overlay strokes its box outline with — independent of
    /// [`Self::focus_ring_width`], which is specifically the *focus
    /// ring* weight, not a widget's resting border.
    ///
    /// **TUI story: no-op.** [`crate::tui::backend::TuiBackend`]'s
    /// box-drawing borders are always exactly one cell wide; there is no
    /// sub-cell stroke weight to refine.
    ///
    /// Default `1.0` — VS Code's own widget border width.
    pub border_width: f32,

    /// Drop-shadow elevation, `0`–`3`, every overlay passes to
    /// [`crate::paint_surface::PaintSurface::surface_draw_shadow`]: `0`
    /// paints no shadow at all, `1`–`3` paint a progressively larger,
    /// softer, more-offset shadow — VS Code's own hover (`1`) vs.
    /// quick-pick/menu (`2`) vs. modal dialog (`3`) elevation tiers.
    /// Values above `3` are clamped by every
    /// [`crate::paint_surface::PaintSurface::surface_draw_shadow`]
    /// implementation (see that method's doc).
    ///
    /// **TUI story: no-op.** A terminal has no pixel-level blur/offset
    /// to approximate a shadow with — `crate::tui`'s rasterisers paint
    /// no shadow at any elevation, matching
    /// [`crate::paint_surface::PaintSurface::surface_draw_shadow`]'s own
    /// module doc ("Why TUI stays out").
    ///
    /// Default `1` — the lightest tier, matching a hover-class overlay;
    /// primitives that want the heavier modal/menu tiers override it
    /// per call with [`Self::with_shadow_elevation`].
    pub shadow_elevation: u8,

    /// Height, in surface-native pixels, of a standard single-line
    /// control (a button, a text input, a list row) — the metric
    /// `Spinner`/`ProgressBar` and the other overlays size their own
    /// single-line chrome against instead of each hardcoding their own
    /// row height.
    ///
    /// **TUI story: cell-quantised.** A terminal row is always exactly
    /// one cell tall; a TUI rasteriser that wants to honour this token
    /// rounds it up to the nearest whole cell. No TUI primitive reads
    /// this field yet, for the same reason as [`Self::padding`] above.
    ///
    /// Default `26.0` — VS Code's own standard control height (input
    /// boxes, compact buttons, list rows).
    pub control_height: f32,

    /// Opacity, `0.0`–`1.0`, [`crate::Palette`]'s own popup backdrop is
    /// painted at — independent of every other overlay's own (always
    /// fully opaque) [`crate::Theme::surface_bg`] fill, since this is
    /// the one token only [`crate::primitives::palette`]'s `paint`
    /// reads; `Toast`/`Tooltip`/`ContextMenu`/`Dialog`/`Spinner`/
    /// `ProgressBar` are unaffected.
    ///
    /// **TUI story: pre-blend.** See the module doc's *Where each token
    /// maps on TUI* section. Concretely,
    /// [`crate::tui::palette::draw_palette_with_style`] blends
    /// [`crate::Theme::background`] with [`crate::Theme::surface_bg`] at
    /// this opacity — a static theme token, not whatever content the
    /// ratatui `Buffer` already holds at those cells — since `Buffer` has
    /// no read-back-and-composite primitive to blend against live screen
    /// content with.
    ///
    /// Default `1.0` — fully opaque, reproducing
    /// [`crate::Theme::surface_bg`] exactly, so every existing caller's
    /// popup looks identical to before this field existed.
    #[serde(default = "default_backdrop_opacity")]
    pub backdrop_opacity: f32,

    /// Height, in surface-native pixels, of a tab strip row — the
    /// editor tab bar, the bottom-panel tab row, and a `SidebarPanel`
    /// header slot all reserve this much height on a pixel backend
    /// instead of the chrome font's own line height, which is usually
    /// shorter than VS Code's actual tab height.
    ///
    /// **TUI story: no-op.** A terminal tab strip is always exactly one
    /// cell tall; every caller only reads this field once it has already
    /// confirmed it is on a pixel backend (`chrome_line_height() > 1.0`),
    /// so a TUI backend's cell-grid strip never consults it at all.
    ///
    /// Default `35.0` — VS Code's own tab bar height.
    #[serde(default = "default_tab_height")]
    pub tab_height: f32,

    /// Height, in surface-native pixels, of the status bar strip at the
    /// bottom of the window.
    ///
    /// **TUI story: no-op**, for the same reason as [`Self::tab_height`].
    ///
    /// Default `22.0` — VS Code's own status bar height.
    #[serde(default = "default_status_bar_height")]
    pub status_bar_height: f32,

    /// Width, in surface-native pixels, of the activity bar column on
    /// the window's edge.
    ///
    /// **TUI story: no-op**, for the same reason as [`Self::tab_height`].
    ///
    /// Default `48.0` — VS Code's own activity bar width.
    #[serde(default = "default_activity_bar_width")]
    pub activity_bar_width: f32,

    /// Height, in surface-native pixels, of one row in a `List`,
    /// `TreeView` or `DataTable` — independent of [`Self::control_height`],
    /// which is the single-line-control metric (buttons, inputs), not a
    /// multi-row list's own row pitch.
    ///
    /// **TUI story: no-op**, for the same reason as [`Self::tab_height`].
    ///
    /// Default `22.0` — VS Code's own list/tree row height.
    #[serde(default = "default_row_height")]
    pub row_height: f32,

    /// Height, in surface-native pixels, of a toolbar strip — the
    /// `Toolbar` header slot a `SidebarPanel` reserves above its content.
    ///
    /// **TUI story: no-op**, for the same reason as [`Self::tab_height`].
    ///
    /// Default `35.0` — VS Code's own toolbar height.
    #[serde(default = "default_toolbar_height")]
    pub toolbar_height: f32,

    /// Width, in surface-native pixels, of a `List`/`TreeView`/
    /// `DataTable` vertical scrollbar track — independent of
    /// [`Self::row_height`], since VS Code's overlay scrollbar is a
    /// fixed 10px regardless of the row pitch it scrolls.
    ///
    /// **TUI story: no-op.** A terminal scrollbar is always exactly one
    /// cell wide; every TUI rasteriser for `List`/`TreeView`/`DataTable`
    /// keeps that one-cell track regardless of this field.
    ///
    /// No pixel-backend rasteriser reads this field yet — `TreeView`
    /// currently paints its vertical scrollbar at the fixed
    /// [`crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH`]
    /// (`10.0`, already matching this field's default) through the single
    /// shared [`crate::primitives::tree::native_surface_paint`] paint path
    /// GTK, macOS and Win32 all call, so wiring the tree to this token is
    /// one change to that shared function, not one per backend.
    /// `DataTable` paints at the separate
    /// [`crate::primitives::layout_metrics::pixel::DATA_TABLE_SCROLLBAR_WIDTH`]
    /// (`8.0`), so wiring it to this field — for the same reason as
    /// [`Self::padding`] above, tracked separately from adding the
    /// themeable seam here — would widen that scrollbar from `8.0` to
    /// this field's `10.0` default unless that follow-up gives
    /// `DataTable` its own override.
    ///
    /// Default `10.0` — VS Code's own list/tree overlay scrollbar width,
    /// matching the
    /// [`crate::primitives::layout_metrics::pixel::TREE_VSCROLLBAR_WIDTH`]
    /// constant.
    #[serde(default = "default_scrollbar_width")]
    pub scrollbar_width: f32,

    /// Horizontal inset, in surface-native pixels, between a row's
    /// left edge and its leading chevron/icon/text — the gap `List`,
    /// `TreeView`, `ContextMenu` and `Palette` all reserve before
    /// painting a row's leading content. Consumed directly by the one
    /// shared [`crate::paint_surface::PaintSurface`] painter each of
    /// those four primitives routes every GTK/macOS/Win32 rasteriser
    /// through — `primitives::{list,tree,context_menu,palette}::native_surface_paint`'s
    /// own `paint`/`paint_with_glyphs` — so wiring this token there is
    /// one change per primitive, not one per backend, the same shape
    /// [`Self::scrollbar_width`]'s own doc describes for a future
    /// `TreeView` scrollbar wire-up.
    ///
    /// `ContextMenu` also uses this token as the trailing inset before
    /// a row's check-glyph/submenu-arrow/shortcut; `List`/`TreeView`/
    /// `Palette` still reserve their trailing detail/badge gap with a
    /// fixed literal — a host-set `row_padding_x` only moves those
    /// three primitives' leading edge today.
    ///
    /// **TUI story: cell-quantised, dropped below one cell.** A sub-cell
    /// inset has no grid column to paint into; a TUI rasteriser that
    /// wants to honour this token rounds it up to whole cells first —
    /// `ceil(row_padding_x / nominal_cell_px)`, the same nominal-cell-width
    /// fallback [`Self::padding`]'s own doc describes — *except* that a
    /// result below one whole cell is dropped to `0` rather than forced
    /// up to a 1-cell minimum: a terminal list row is already flush
    /// against its own border character, and forcing every row's text
    /// one full cell inward by default would look like a layout bug, not
    /// VS Code's much subtler sub-cell inset. No TUI primitive reads this
    /// field yet, for the same reason as [`Self::padding`] above.
    ///
    /// Default `8.0` — VS Code's own list/tree/menu row horizontal inset.
    #[serde(default = "default_row_padding_x")]
    pub row_padding_x: f32,

    /// Horizontal inset, in surface-native pixels, between a tab strip's
    /// edge and a tab label's text. Defined here alongside
    /// [`Self::row_padding_x`] but consumed by each GUI backend's own
    /// `tab_bar.rs` painter rather than a primitive shared across
    /// backends — see the per-backend follow-up issues this field's
    /// introducing change tracks.
    ///
    /// **TUI story: cell-quantised, dropped below one cell** — the same
    /// rule as [`Self::row_padding_x`]'s own doc.
    ///
    /// Default `10.0` — VS Code's own tab label horizontal inset.
    #[serde(default = "default_tab_padding_x")]
    pub tab_padding_x: f32,
}

/// `#[serde(default)]` value for [`Style::tab_height`] — a struct
/// serialized before this field existed deserializes at the VS Code
/// default, matching [`Style::default`].
fn default_tab_height() -> f32 {
    35.0
}

/// `#[serde(default)]` value for [`Style::status_bar_height`] — see
/// [`default_tab_height`].
fn default_status_bar_height() -> f32 {
    22.0
}

/// `#[serde(default)]` value for [`Style::activity_bar_width`] — see
/// [`default_tab_height`].
fn default_activity_bar_width() -> f32 {
    48.0
}

/// `#[serde(default)]` value for [`Style::row_height`] — see
/// [`default_tab_height`].
fn default_row_height() -> f32 {
    22.0
}

/// `#[serde(default)]` value for [`Style::toolbar_height`] — see
/// [`default_tab_height`].
fn default_toolbar_height() -> f32 {
    35.0
}

/// `#[serde(default)]` value for [`Style::backdrop_opacity`] — a struct
/// serialized before this field existed deserializes as fully opaque,
/// matching [`Style::default`].
fn default_backdrop_opacity() -> f32 {
    1.0
}

/// `#[serde(default)]` value for [`Style::scrollbar_width`] — see
/// [`default_tab_height`].
fn default_scrollbar_width() -> f32 {
    10.0
}

/// `#[serde(default)]` value for [`Style::row_padding_x`] — see
/// [`default_tab_height`].
fn default_row_padding_x() -> f32 {
    8.0
}

/// `#[serde(default)]` value for [`Style::tab_padding_x`] — see
/// [`default_tab_height`].
fn default_tab_padding_x() -> f32 {
    10.0
}

impl Default for Style {
    fn default() -> Self {
        Self {
            focus_ring_width: 2.0,
            padding: 8.0,
            corner_radius: 6.0,
            border_width: 1.0,
            shadow_elevation: 1,
            control_height: 26.0,
            backdrop_opacity: 1.0,
            tab_height: 35.0,
            status_bar_height: 22.0,
            activity_bar_width: 48.0,
            row_height: 22.0,
            toolbar_height: 35.0,
            scrollbar_width: 10.0,
            row_padding_x: 8.0,
            tab_padding_x: 10.0,
        }
    }
}

impl Style {
    /// Set [`Self::focus_ring_width`]. A consumer starts from
    /// [`Style::default`] and chains `with_*` builders, e.g.
    /// `Style::default().with_focus_ring_width(4.0)` — see the module
    /// doc for why `#[non_exhaustive]` makes this the only construction
    /// shape.
    #[must_use]
    pub fn with_focus_ring_width(mut self, focus_ring_width: f32) -> Self {
        self.focus_ring_width = focus_ring_width;
        self
    }

    /// Set [`Self::padding`].
    #[must_use]
    pub fn with_padding(mut self, padding: f32) -> Self {
        self.padding = padding;
        self
    }

    /// Set [`Self::corner_radius`].
    #[must_use]
    pub fn with_corner_radius(mut self, corner_radius: f32) -> Self {
        self.corner_radius = corner_radius;
        self
    }

    /// Set [`Self::border_width`].
    #[must_use]
    pub fn with_border_width(mut self, border_width: f32) -> Self {
        self.border_width = border_width;
        self
    }

    /// Set [`Self::shadow_elevation`]. Not clamped here — every
    /// [`crate::paint_surface::PaintSurface::surface_draw_shadow`]
    /// implementation clamps its own `elevation` argument to `0..=3`
    /// (see that method's doc), so a caller that sets a larger value is
    /// harmless, just redundant.
    #[must_use]
    pub fn with_shadow_elevation(mut self, shadow_elevation: u8) -> Self {
        self.shadow_elevation = shadow_elevation;
        self
    }

    /// Set [`Self::control_height`].
    #[must_use]
    pub fn with_control_height(mut self, control_height: f32) -> Self {
        self.control_height = control_height;
        self
    }

    /// Set [`Self::backdrop_opacity`]. Not clamped here — both
    /// consuming sides clamp on their own: [`crate::Color::with_alpha`]
    /// clamps to `0.0..=1.0` before scaling, and
    /// [`crate::Color::blend`] clamps its own fraction the same way.
    #[must_use]
    pub fn with_backdrop_opacity(mut self, backdrop_opacity: f32) -> Self {
        self.backdrop_opacity = backdrop_opacity;
        self
    }

    /// Set [`Self::tab_height`].
    #[must_use]
    pub fn with_tab_height(mut self, tab_height: f32) -> Self {
        self.tab_height = tab_height;
        self
    }

    /// Set [`Self::status_bar_height`].
    #[must_use]
    pub fn with_status_bar_height(mut self, status_bar_height: f32) -> Self {
        self.status_bar_height = status_bar_height;
        self
    }

    /// Set [`Self::activity_bar_width`].
    #[must_use]
    pub fn with_activity_bar_width(mut self, activity_bar_width: f32) -> Self {
        self.activity_bar_width = activity_bar_width;
        self
    }

    /// Set [`Self::row_height`].
    #[must_use]
    pub fn with_row_height(mut self, row_height: f32) -> Self {
        self.row_height = row_height;
        self
    }

    /// Set [`Self::toolbar_height`].
    #[must_use]
    pub fn with_toolbar_height(mut self, toolbar_height: f32) -> Self {
        self.toolbar_height = toolbar_height;
        self
    }

    /// Set [`Self::scrollbar_width`].
    #[must_use]
    pub fn with_scrollbar_width(mut self, scrollbar_width: f32) -> Self {
        self.scrollbar_width = scrollbar_width;
        self
    }

    /// Set [`Self::row_padding_x`].
    #[must_use]
    pub fn with_row_padding_x(mut self, row_padding_x: f32) -> Self {
        self.row_padding_x = row_padding_x;
        self
    }

    /// Set [`Self::tab_padding_x`].
    #[must_use]
    pub fn with_tab_padding_x(mut self, tab_padding_x: f32) -> Self {
        self.tab_padding_x = tab_padding_x;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins the stock `2.0` stroke width every pixel backend paints, so
    /// a future edit to this default is a deliberate, reviewed visual
    /// change — not an accidental one.
    #[test]
    fn default_focus_ring_width_matches_pre_style_constant() {
        assert_eq!(Style::default().focus_ring_width, 2.0);
    }

    /// The `Default` + `with_*` construction shape the module doc
    /// promises stays compiling as fields are added — unlike a struct
    /// literal, unaffected by `#[non_exhaustive]`.
    #[test]
    fn with_focus_ring_width_builder_overrides_only_that_field() {
        let style = Style::default().with_focus_ring_width(4.0);
        assert_eq!(style.focus_ring_width, 4.0);
    }

    /// Pins the VS-Code-matching defaults for these five tokens, so a
    /// future edit to any of them is a deliberate, reviewed visual
    /// change — not an accidental one. See each field's own doc comment
    /// for which VS Code surface it mirrors.
    #[test]
    fn defaults_match_vs_code_token_set() {
        let style = Style::default();
        assert_eq!(style.padding, 8.0);
        assert_eq!(style.corner_radius, 6.0);
        assert_eq!(style.border_width, 1.0);
        assert_eq!(style.shadow_elevation, 1);
        assert_eq!(style.control_height, 26.0);
    }

    /// Pins the VS-Code-matching GUI chrome-metric defaults, so a future
    /// edit to any of them is a deliberate, reviewed visual change.
    #[test]
    fn defaults_match_vs_code_chrome_metrics() {
        let style = Style::default();
        assert_eq!(style.tab_height, 35.0);
        assert_eq!(style.status_bar_height, 22.0);
        assert_eq!(style.activity_bar_width, 48.0);
        assert_eq!(style.row_height, 22.0);
        assert_eq!(style.toolbar_height, 35.0);
    }

    /// Each chrome-metric `with_*` builder overrides only its own field.
    #[test]
    fn chrome_metric_builders_each_override_only_their_own_field() {
        let defaults = Style::default();

        let tab_height = defaults.with_tab_height(40.0);
        assert_eq!(tab_height.tab_height, 40.0);
        assert_eq!(tab_height.status_bar_height, defaults.status_bar_height);

        let status_bar_height = defaults.with_status_bar_height(28.0);
        assert_eq!(status_bar_height.status_bar_height, 28.0);
        assert_eq!(
            status_bar_height.activity_bar_width,
            defaults.activity_bar_width
        );

        let activity_bar_width = defaults.with_activity_bar_width(56.0);
        assert_eq!(activity_bar_width.activity_bar_width, 56.0);
        assert_eq!(activity_bar_width.row_height, defaults.row_height);

        let row_height = defaults.with_row_height(28.0);
        assert_eq!(row_height.row_height, 28.0);
        assert_eq!(row_height.toolbar_height, defaults.toolbar_height);

        let toolbar_height = defaults.with_toolbar_height(40.0);
        assert_eq!(toolbar_height.toolbar_height, 40.0);
        assert_eq!(toolbar_height.tab_height, defaults.tab_height);
    }

    /// A JSON payload serialized before the chrome-metric fields existed
    /// has no such keys; `#[serde(default = "…")]` must deserialize them
    /// at the VS Code defaults rather than erroring, so a consumer's
    /// persisted config from an earlier `quadraui` version keeps loading.
    #[test]
    fn deserializing_without_chrome_metrics_defaults_to_vs_code_values() {
        let json = r#"{
            "focus_ring_width": 2.0,
            "padding": 8.0,
            "corner_radius": 6.0,
            "border_width": 1.0,
            "shadow_elevation": 1,
            "control_height": 26.0
        }"#;
        let style: Style = serde_json::from_str(json).expect("deserialize without chrome metrics");
        assert_eq!(style.tab_height, 35.0);
        assert_eq!(style.status_bar_height, 22.0);
        assert_eq!(style.activity_bar_width, 48.0);
        assert_eq!(style.row_height, 22.0);
        assert_eq!(style.toolbar_height, 35.0);
    }

    /// A JSON payload serialized before [`Style::scrollbar_width`]
    /// existed has no such key; `#[serde(default = "…")]` must
    /// deserialize it at the VS Code default rather than erroring.
    #[test]
    fn deserializing_without_scrollbar_width_defaults_to_ten_pixels() {
        let json = r#"{
            "focus_ring_width": 2.0,
            "padding": 8.0,
            "corner_radius": 6.0,
            "border_width": 1.0,
            "shadow_elevation": 1,
            "control_height": 26.0
        }"#;
        let style: Style = serde_json::from_str(json).expect("deserialize without scrollbar_width");
        assert_eq!(style.scrollbar_width, 10.0);
    }

    /// Every one of these `with_*` builders overrides only its own
    /// field — the same proof `with_focus_ring_width_builder_overrides_only_that_field`
    /// already gives for the original token, extended to the rest of
    /// the set so a copy/paste mistake between builders (e.g. two of
    /// them writing the same field) would fail here.
    #[test]
    fn style_token_builders_each_override_only_their_own_field() {
        let defaults = Style::default();

        let padding = defaults.with_padding(20.0);
        assert_eq!(padding.padding, 20.0);
        assert_eq!(padding.corner_radius, defaults.corner_radius);

        let corner_radius = defaults.with_corner_radius(12.0);
        assert_eq!(corner_radius.corner_radius, 12.0);
        assert_eq!(corner_radius.border_width, defaults.border_width);

        let border_width = defaults.with_border_width(3.0);
        assert_eq!(border_width.border_width, 3.0);
        assert_eq!(border_width.shadow_elevation, defaults.shadow_elevation);

        let shadow_elevation = defaults.with_shadow_elevation(3);
        assert_eq!(shadow_elevation.shadow_elevation, 3);
        assert_eq!(shadow_elevation.control_height, defaults.control_height);

        let control_height = defaults.with_control_height(32.0);
        assert_eq!(control_height.control_height, 32.0);
        assert_eq!(control_height.focus_ring_width, defaults.focus_ring_width);

        let backdrop_opacity = defaults.with_backdrop_opacity(0.5);
        assert_eq!(backdrop_opacity.backdrop_opacity, 0.5);
        assert_eq!(backdrop_opacity.padding, defaults.padding);
    }

    /// Pins the fully-opaque default so every existing `Palette` caller's
    /// popup keeps looking identical to before this field existed.
    #[test]
    fn default_backdrop_opacity_is_fully_opaque() {
        assert_eq!(Style::default().backdrop_opacity, 1.0);
    }

    /// Same proof as `with_focus_ring_width_builder_overrides_only_that_field`,
    /// for the one token added after the original fixed VS-Code batch.
    #[test]
    fn with_backdrop_opacity_builder_overrides_only_that_field() {
        let style = Style::default().with_backdrop_opacity(0.3);
        assert_eq!(style.backdrop_opacity, 0.3);
        assert_eq!(style.shadow_elevation, Style::default().shadow_elevation);
    }

    /// Same proof as `with_focus_ring_width_builder_overrides_only_that_field`,
    /// for [`Style::scrollbar_width`].
    #[test]
    fn with_scrollbar_width_builder_overrides_only_that_field() {
        let style = Style::default().with_scrollbar_width(14.0);
        assert_eq!(style.scrollbar_width, 14.0);
        assert_eq!(style.row_height, Style::default().row_height);
    }

    /// A JSON payload serialized before `backdrop_opacity` existed has no
    /// such key; `#[serde(default = "default_backdrop_opacity")]` must
    /// deserialize it as fully opaque rather than erroring, so a
    /// consumer's persisted config from an earlier `quadraui` version
    /// keeps loading.
    #[test]
    fn deserializing_without_backdrop_opacity_field_defaults_to_fully_opaque() {
        let json = r#"{
            "focus_ring_width": 2.0,
            "padding": 8.0,
            "corner_radius": 6.0,
            "border_width": 1.0,
            "shadow_elevation": 1,
            "control_height": 26.0
        }"#;
        let style: Style =
            serde_json::from_str(json).expect("deserialize without backdrop_opacity");
        assert_eq!(style.backdrop_opacity, 1.0);
    }

    /// Pins the VS-Code-matching defaults for the row/tab padding
    /// tokens, so a future edit to either is a deliberate, reviewed
    /// visual change.
    #[test]
    fn defaults_match_vs_code_row_and_tab_padding() {
        let style = Style::default();
        assert_eq!(style.row_padding_x, 8.0);
        assert_eq!(style.tab_padding_x, 10.0);
    }

    /// Same proof as `with_focus_ring_width_builder_overrides_only_that_field`,
    /// for [`Style::row_padding_x`] and [`Style::tab_padding_x`].
    #[test]
    fn row_and_tab_padding_builders_each_override_only_their_own_field() {
        let defaults = Style::default();

        let row_padding_x = defaults.with_row_padding_x(12.0);
        assert_eq!(row_padding_x.row_padding_x, 12.0);
        assert_eq!(row_padding_x.tab_padding_x, defaults.tab_padding_x);

        let tab_padding_x = defaults.with_tab_padding_x(14.0);
        assert_eq!(tab_padding_x.tab_padding_x, 14.0);
        assert_eq!(tab_padding_x.row_padding_x, defaults.row_padding_x);
    }

    /// A JSON payload serialized before [`Style::row_padding_x`] /
    /// [`Style::tab_padding_x`] existed has no such keys;
    /// `#[serde(default = "…")]` must deserialize them at the VS Code
    /// defaults rather than erroring.
    #[test]
    fn deserializing_without_row_or_tab_padding_defaults_to_vs_code_values() {
        let json = r#"{
            "focus_ring_width": 2.0,
            "padding": 8.0,
            "corner_radius": 6.0,
            "border_width": 1.0,
            "shadow_elevation": 1,
            "control_height": 26.0
        }"#;
        let style: Style = serde_json::from_str(json).expect("deserialize without row/tab padding");
        assert_eq!(style.row_padding_x, 8.0);
        assert_eq!(style.tab_padding_x, 10.0);
    }
}
