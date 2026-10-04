//! Direct2D / DirectWrite rasteriser for [`crate::MultiSectionView`]
//! (issue #27).
//!
//! Paints the full chrome (per-section headers, optional aux rows,
//! per-section scrollbars, optional dividers) onto an `ID2D1RenderTarget`
//! and dispatches each section's body to the appropriate quadraui body
//! rasteriser (`super::tree::draw_tree`, `super::list::draw_list`,
//! `draw_form_body`, `crate::primitives::chart::paint` (#810),
//! `crate::primitives::terminal::paint` (#810),
//! `super::message_list::draw_message_list`) using the body bounds
//! returned by the primitive's [`crate::MultiSectionView::layout`].
//! `SectionBody::Terminal` / `SectionBody::MessageList` used to paint
//! background only, on the rationale that `super::terminal` /
//! `super::message_list` were still `todo!()` stubs — stale since #30
//! landed both rasterisers; #727 wired them in here.
//!
//! Mirrors [`crate::macos::multi_section_view`] in shape (the closest
//! existing pixel backend: no frame-scope requirement for layout, real
//! font measurement rather than TUI's cell grid): [`win_msv_metrics`]
//! computes the layout metrics for a given `line_height`,
//! [`win_msv_layout`] returns the resolved chrome layout, and
//! [`draw_multi_section_view`] consumes the same layout for paint.
//! `WinBackend::msv_layout` calls [`win_msv_layout`] directly so paint
//! and click share one source of truth (see `super::backend`'s
//! `msv_layout`/`msv_metrics` methods). Both, and the section
//! body-measurement they drive, are thin wrappers over
//! [`crate::primitives::layout_metrics`] (#499, adopted for `win/` by
//! #701) rather than a third hand-derived copy of the GTK/macOS math.
//!
//! Vertical-only in v1 (per #294 / D-003 in
//! `quadraui/docs/decisions/DECISIONS.md`); horizontal sections fall through to a
//! no-op, same as the GTK/macOS twins.
//!
//! Only compiled on `target_os = "windows"` — see `super::mod`'s
//! `#[cfg(target_os = "windows")] mod multi_section_view;` and
//! `backend.rs`'s module docs for why the rest of this repo's
//! `--features win` compile gate stays meaningful without a Windows
//! host. Colours come from `Theme::default()` rather than a live
//! `WinBackend` theme field, same as every other rasteriser in this
//! module (see `win::status_bar`'s doc for why).
//!
//! # Scope omissions (follow-up, matches `crate::macos::multi_section_view`)
//!
//! - **Custom-icon empty bodies** — the `EmptyBody::action` button is
//!   rendered as plain centred text, no clickable button chrome.
//! - **Caret blink** — `WinBackend` has no caret-blink timer
//!   infrastructure yet (unlike `macos::caret_blink`), so the
//!   `SectionAux::Input`/`Search` caret paints unconditionally whenever
//!   the input `has_focus`, matching `gtk::multi_section_view`'s
//!   simpler (non-blinking) convention rather than macOS's blink-aware
//!   one.
//! - ~~Translucent overlays are CPU-premixed, not native D2D alpha
//!   blending~~ — fixed by #1074: the per-section scrollbar now paints
//!   through the shared
//!   [`crate::primitives::multi_section_view::native_surface_paint::paint_scrollbar`],
//!   which fills with real alpha (`Color::with_alpha`) the same way
//!   GTK/macOS always have. [`super::surface::D2dSurface::surface_fill_rect`]
//!   has honoured real alpha since quadraui#791/#1072 — the CPU-premix
//!   convention this bullet used to document was legacy from before
//!   that adapter existed, not a remaining Direct2D limitation.

use windows::Win32::Graphics::Direct2D::ID2D1RenderTarget;

use super::text::{fill_rect, pop_clip, push_clip, DWrite};
use crate::event::Rect;
use crate::primitives::multi_section_view::{
    Axis, EmptyBody, MsvLayoutMetrics, MultiSectionView, MultiSectionViewLayout, SectionAux,
    SectionBody, SectionHeader,
};
use crate::theme::Theme;
use crate::types::StyledText;

/// Compute the Win-GUI metrics for a `MultiSectionView` from a
/// `line_height`. Hosts call this and the primitive's `layout()` with
/// the same metrics so paint and click resolve to the same bounds.
/// Matches `mac_msv_metrics` / `gtk::multi_section_view::metrics_for`'s
/// convention exactly.
///
/// Thin wrapper over [`crate::primitives::layout_metrics::msv_metrics`]
/// (#499) — identical math across every pixel backend.
pub fn win_msv_metrics(line_height: f32, allow_resize: bool) -> MsvLayoutMetrics {
    crate::primitives::layout_metrics::msv_metrics(line_height as f64, allow_resize)
}

/// Compute the layout for a `MultiSectionView` using the Win-GUI
/// metrics the rasteriser would use itself. Hosts call this to drive
/// hit-testing without re-computing — paint and click share this single
/// layout per frame. No `DWrite` handle needed: body measurement below
/// is estimate-based (row counts × `line_height`), not real text
/// measurement, mirroring `mac_msv_metrics`'s twin body measure.
///
/// Thin wrapper over [`crate::primitives::layout_metrics::msv_layout`]
/// (#499) / [`crate::primitives::layout_metrics::msv_body_measure`],
/// which is also where the `MessageList` real-content-height fix (a
/// pre-#499 macOS drift — see `msv_body_measure`'s doc) lives, so `win/`
/// inherits it automatically instead of re-deriving the old `0.0` stub.
pub fn win_msv_layout(
    view: &MultiSectionView,
    bounds: Rect,
    line_height: f32,
) -> MultiSectionViewLayout {
    crate::primitives::layout_metrics::msv_layout(view, bounds, line_height as f64)
}

/// Draw a [`MultiSectionView`] into `rect` (DIPs) on `target`.
///
/// # Visual contract
///
/// See [`crate::macos::multi_section_view::draw_multi_section_view`]'s
/// doc — this rasteriser matches its chrome layout (header/aux/body/
/// scrollbar/divider) byte-for-byte modulo the scope omissions listed
/// in this module's doc.
pub fn draw_multi_section_view(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    rect: Rect,
    view: &MultiSectionView,
    line_height: f32,
    char_width: f32,
) {
    if rect.width <= 0.0 || rect.height <= 0.0 || view.axis == Axis::Horizontal {
        return;
    }

    let theme = Theme::default();
    push_clip(target, rect);
    let _ = fill_rect(target, rect, theme.background);

    let view_layout = win_msv_layout(view, rect, line_height);

    for s_layout in &view_layout.sections {
        let section = &view.sections[s_layout.section_idx];

        paint_header(
            target,
            dwrite,
            s_layout.header_bounds,
            &section.header,
            section.collapsed,
            &theme,
        );

        if !s_layout.collapsed {
            if let (Some(aux), Some(aux_b)) = (&section.aux, s_layout.aux_bounds) {
                paint_aux(target, dwrite, aux_b, aux, &theme);
            }

            paint_body(
                target,
                dwrite,
                s_layout.body_bounds,
                &section.body,
                &theme,
                line_height,
                char_width,
            );

            if let Some(sb_b) = s_layout.scrollbar_bounds {
                paint_section_scrollbar(target, sb_b, s_layout.thumb_bounds, &theme);
            }
        }
    }

    if view.allow_resize {
        for d in &view_layout.dividers {
            let _ = fill_rect(target, d.bounds, theme.separator);
        }
    }

    pop_clip(target);

    // Panel-level scrollbar (WholePanel mode) painted outside the
    // panel clip so it isn't itself clipped — matches
    // `mac_msv`/`gtk_msv`'s posture. Thumb geometry comes straight from
    // `view_layout.panel_scrollbar_thumb` (computed once, via
    // `fit_thumb`, in `MultiSectionView::layout`) — see
    // `paint_panel_scrollbar`'s doc for why this used to be a fourth,
    // disagreeing formula (quadraui#820).
    if let Some(panel_sb) = view_layout.panel_scrollbar {
        paint_panel_scrollbar(target, panel_sb, view_layout.panel_scrollbar_thumb, &theme);
    }
}

// #1074 (PaintSurface Phase 4): header/aux/text/empty/scrollbar/divider
// chrome painting moved to the shared
// [`crate::primitives::multi_section_view::native_surface_paint`] — these
// wrappers just build a [`super::surface::D2dSurface`] adapter and
// forward. `paint_body` (below) stays here: `Tree`/`List`/`MessageList`
// bodies still dispatch to this backend's own `super::tree::draw_tree`/
// `super::list::draw_list`/`super::message_list::draw_message_list`, which
// take a raw `(&ID2D1RenderTarget, &DWrite)` pair, not `&mut dyn
// PaintSurface`.
//
// Pre-port, this rasteriser already clipped the title paint the same way
// the shared [`crate::primitives::multi_section_view::native_surface_paint::paint_header`]
// does — see that function's doc for the GTK-only drift this port fixes
// elsewhere, not here.

fn paint_header(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    header: &SectionHeader,
    collapsed: bool,
    theme: &Theme,
) {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_header(
        &mut surface,
        bounds,
        header,
        collapsed,
        theme,
    );
}

fn paint_aux(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    aux: &SectionAux,
    theme: &Theme,
) {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    // `caret_visible: true` — see this module's "Caret blink" scope-
    // omission note: Win-GUI has no blink timer for MSV aux inputs, so
    // the caret always paints while focused, matching this rasteriser's
    // pre-port behaviour exactly.
    crate::primitives::multi_section_view::native_surface_paint::paint_aux(
        &mut surface,
        bounds,
        aux,
        theme,
        true,
    );
}

#[allow(clippy::too_many_arguments)]
fn paint_body(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    body: &SectionBody,
    theme: &Theme,
    line_height: f32,
    char_width: f32,
) {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    // Clip to body bounds so inner primitives can't paint past the
    // section boundary.
    push_clip(target, bounds);

    match body {
        SectionBody::Tree(t) => {
            // #804 fixed the nerd-fonts gap in `win::tree::draw_tree`
            // itself; wiring `nerd_fonts_enabled` through
            // `draw_multi_section_view`'s own call chain (it isn't a
            // parameter here yet, unlike TUI/GTK's MSV) is separate,
            // unstarted scope — passing `false` preserves today's
            // fallback-only behaviour for tree bodies nested in an MSV.
            //
            // `theme` here is `draw_multi_section_view`'s own local
            // `Theme::default()` (see this function's caller) —
            // `multi_section_view.rs` is not one of the Win-GUI
            // rasterisers wired to `current_theme`, so this stays scoped
            // to *compiling* against `super::tree::draw_tree`'s `&Theme`
            // parameter rather than threading the live theme through
            // MSV's own call chain — that is still separate, unstarted
            // scope, same as the `nerd_fonts_enabled` gap noted above.
            let _ = super::tree::draw_tree(target, dwrite, bounds, t, line_height, false, theme);
        }
        SectionBody::List(l) => {
            let _ = super::list::draw_list(target, dwrite, bounds, l, line_height);
        }
        SectionBody::Form(f) => {
            draw_form_body(target, dwrite, bounds, f, theme, line_height);
        }
        SectionBody::Chart(c) => {
            // #810: painting moved to the shared
            // `crate::primitives::chart::paint`; this raw
            // `(&ID2D1RenderTarget, &DWrite)` call site (no live
            // `WinBackend` on hand) reuses the shared
            // `super::surface::D2dSurface` adapter (#1072).
            let chart_layout = super::chart::win_chart_layout(c, bounds, char_width, line_height);
            let mut surface = super::surface::D2dSurface {
                target,
                dwrite: Some(dwrite),
            };
            crate::primitives::chart::paint(c, &chart_layout, &mut surface, theme, None, None);
        }
        SectionBody::Terminal(t) => {
            // #810: painting moved to the shared
            // `crate::primitives::terminal::paint`; reuses the shared
            // `super::surface::D2dSurface` adapter like the `Chart` arm
            // above.
            let mut surface = super::surface::D2dSurface {
                target,
                dwrite: Some(dwrite),
            };
            crate::primitives::terminal::paint(
                t,
                &mut surface,
                theme,
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                line_height,
                char_width,
                None,
            );
        }
        SectionBody::MessageList(m) => {
            super::message_list::draw_message_list(
                target,
                dwrite,
                m,
                bounds.x,
                bounds.y,
                bounds.y + bounds.height,
                line_height,
            );
        }
        SectionBody::Text(lines) => {
            paint_text_lines(target, dwrite, bounds, lines, theme, line_height);
        }
        SectionBody::Empty(empty) => {
            paint_empty_body(target, dwrite, bounds, empty, theme, line_height);
        }
        SectionBody::Custom(_) => {
            // Host paints in body bounds.
        }
    }
    pop_clip(target);
}

/// Paint an embedded [`crate::Form`] section body. #808: field-kind
/// painting goes through the shared [`crate::primitives::form::paint`]
/// via [`super::surface::D2dSurface`] (this call site has only a raw
/// `target`/`dwrite`, not a live [`super::WinBackend`]) —
/// `FieldKind::Toolbar` is painted separately below, same as
/// `WinBackend::draw_form`, for the same reason (see that fn's doc).
fn draw_form_body(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    form: &crate::Form,
    theme: &Theme,
    line_height: f32,
) {
    let flayout = super::form::win_form_layout(dwrite, bounds, form, line_height);
    let origin = crate::Point::new(bounds.x, bounds.y);
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::form::paint(form, &flayout, &mut surface, theme, origin);

    for vf in &flayout.visible_fields {
        let Some(field) = form.fields.get(vf.field_idx) else {
            continue;
        };
        let crate::FieldKind::Toolbar(toolbar) = &field.kind else {
            continue;
        };
        let field_fg = if field.disabled {
            theme.muted_fg
        } else {
            theme.foreground
        };
        for (item_id, item_rect) in &vf.item_bounds {
            let btn = toolbar.buttons.iter().find_map(|b| {
                super::form::toolbar_item(&field.id, b)
                    .filter(|(id, _)| id == item_id)
                    .map(|_| b)
            });
            let r = Rect::new(
                origin.x + item_rect.x,
                origin.y + item_rect.y,
                item_rect.width,
                item_rect.height,
            );
            match btn {
                Some(crate::primitives::toolbar::ToolbarButton::Action {
                    label, enabled, ..
                }) => {
                    let fg = if *enabled { field_fg } else { theme.muted_fg };
                    let (tw, th) = dwrite.measure_text(label).unwrap_or((0.0, 0.0));
                    let ty = r.y + (r.height - th) / 2.0;
                    let _ = dwrite.draw_text(target, label, Rect::new(r.x, ty, tw, th), fg);
                }
                Some(crate::primitives::toolbar::ToolbarButton::Label { text, fg }) => {
                    let color = fg.unwrap_or(field_fg);
                    let (tw, th) = dwrite.measure_text(text).unwrap_or((0.0, 0.0));
                    let ty = r.y + (r.height - th) / 2.0;
                    let _ = dwrite.draw_text(target, text, Rect::new(r.x, ty, tw, th), color);
                }
                _ => {}
            }
        }
    }
}

fn paint_text_lines(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    lines: &[StyledText],
    theme: &Theme,
    line_height: f32,
) {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_text_lines(
        &mut surface,
        bounds,
        lines,
        theme,
        line_height,
    );
}

fn paint_empty_body(
    target: &ID2D1RenderTarget,
    dwrite: &DWrite,
    bounds: Rect,
    empty: &EmptyBody,
    theme: &Theme,
    line_height: f32,
) {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: Some(dwrite),
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_empty_body(
        &mut surface,
        bounds,
        empty,
        theme,
        line_height,
    );
}

/// Per-section scrollbar gutter — 50%-alpha track, 90%-alpha thumb.
///
/// Pre-port this premixed both fills against `theme.background` via
/// [`crate::types::Color::blend`] rather than painting a real
/// translucent brush, on the (once-true) rationale that Direct2D fills
/// here were opaque-only. That stopped being true when
/// [`super::surface::D2dSurface::surface_fill_rect`] started honouring
/// real alpha (quadraui#791/#1072 — see that adapter's module doc: "no
/// fill-translucency divergence to preserve"), so routing this scrollbar
/// through the shared, real-alpha
/// [`crate::primitives::multi_section_view::native_surface_paint::paint_scrollbar`]
/// unifies Win-GUI onto the same real alpha blend GTK/macOS already use
/// here, rather than continuing to approximate it against a hardcoded
/// destination colour.
fn paint_section_scrollbar(
    target: &ID2D1RenderTarget,
    gutter: Rect,
    thumb_bounds: Option<Rect>,
    theme: &Theme,
) {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: None,
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_scrollbar(
        &mut surface,
        gutter,
        thumb_bounds,
        theme,
    );
}

/// Panel-level scrollbar (`ScrollMode::WholePanel`) — opaque track and
/// thumb, no blending needed since it's painted outside any body clip.
/// Thumb geometry comes from `thumb_bounds` — computed once by
/// [`crate::primitives::multi_section_view::MultiSectionView::layout`]
/// (`fit_thumb`) and published as
/// [`crate::primitives::multi_section_view::MultiSectionViewLayout::panel_scrollbar_thumb`].
///
/// Pre-quadraui#820 this function computed thumb size/position itself
/// from `(scroll, total_content)` with its own formula that disagreed
/// with the layout's `fit_thumb`-based one in two ways: the caller
/// passed `total_content` as *just* the summed section sizes, silently
/// dropping divider strips; and a hardcoded `20.0`-pixel minimum thumb
/// here disagreed with `panel_thumb_min`'s `metrics.scrollbar_size.max(8.0)`
/// used everywhere else. Mirrors [`paint_section_scrollbar`]'s
/// (per-section) pattern of consuming pre-computed bounds instead of
/// re-deriving them.
fn paint_panel_scrollbar(
    target: &ID2D1RenderTarget,
    bounds: Rect,
    thumb_bounds: Option<Rect>,
    theme: &Theme,
) {
    let mut surface = super::surface::D2dSurface {
        target,
        dwrite: None,
    };
    crate::primitives::multi_section_view::native_surface_paint::paint_panel_scrollbar(
        &mut surface,
        bounds,
        thumb_bounds,
        theme,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::multi_section_view::{
        InlineInput, MultiSectionViewHit, ScrollMode, ScrollbarHit, Section, SectionHeader,
        SectionSize,
    };
    use crate::primitives::tree::{TreeRow, TreeView};
    use crate::types::{Decoration, SelectionMode, StyledText, TreeStyle, WidgetId};
    use crate::win::testing::HeadlessSurface;

    const W: f32 = 240.0;
    const H: f32 = 320.0;
    const LINE_HEIGHT: f32 = 16.0;

    fn leaf(idx: u16, label: &str) -> TreeRow {
        TreeRow {
            path: vec![idx],
            indent: 0,
            icon: None,
            text: StyledText::plain(label),
            badge: None,
            is_expanded: None,
            decoration: Decoration::Normal,
            edit: None,
        }
    }

    fn tree_section(name: &str, n: usize) -> Section {
        Section {
            id: name.into(),
            header: SectionHeader {
                icon: None,
                title: StyledText::plain(name),
                badge: None,
                actions: vec![],
                show_chevron: true,
            },
            body: SectionBody::Tree(TreeView {
                id: WidgetId::new(format!("tree:{}", name)),
                rows: (0..n)
                    .map(|i| leaf(i as u16, &format!("{}-{}", name, i)))
                    .collect(),
                selection_mode: SelectionMode::Single,
                selected_path: None,
                scroll_offset: 0,
                style: TreeStyle::default(),
                has_focus: false,
            }),
            aux: None,
            size: SectionSize::EqualShare,
            collapsed: false,
            min_size: None,
            max_size: None,
        }
    }

    fn two_section_view() -> MultiSectionView {
        MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![tree_section("alpha", 5), tree_section("beta", 3)],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        }
    }

    fn paint_via(view: &MultiSectionView) -> (HeadlessSurface, MultiSectionViewLayout) {
        let surface = HeadlessSurface::new(W as u32, H as u32).expect("create surface");
        let (dwrite, _, _) = DWrite::new("Segoe UI", 10.0, None).expect("create DWrite");
        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = surface
            .paint(|target| {
                draw_multi_section_view(target, &dwrite, rect, view, LINE_HEIGHT, 8.0);
            })
            .map(|_| win_msv_layout(view, rect, LINE_HEIGHT))
            .expect("paint msv");
        (surface, layout)
    }

    #[test]
    fn header_strip_paints_header_bg() {
        let view = two_section_view();
        let (surface, layout) = paint_via(&view);
        let theme = Theme::default();
        let hdr = layout.sections[0].header_bounds;
        // Probe near right edge of the first header (past chevron/title).
        let px = (hdr.x + hdr.width - 4.0) as u32;
        let py = (hdr.y + hdr.height / 2.0) as u32;
        let c = surface.pixel_at(px, py);
        assert_eq!(
            (c.r, c.g, c.b),
            (theme.header_bg.r, theme.header_bg.g, theme.header_bg.b),
        );
    }

    #[test]
    fn two_sections_stack_vertically_without_overlap() {
        let view = two_section_view();
        let (_surface, layout) = paint_via(&view);
        let s0 = &layout.sections[0];
        let s1 = &layout.sections[1];
        let s0_bottom = s0.body_bounds.y + s0.body_bounds.height;
        assert!(
            s1.header_bounds.y >= s0_bottom - 0.5,
            "section 1 must stack below section 0; s0_bottom={}, s1_header_y={}",
            s0_bottom,
            s1.header_bounds.y,
        );
    }

    #[test]
    fn hit_test_resolves_header_click_to_section() {
        let view = two_section_view();
        let (_surface, layout) = paint_via(&view);
        let hdr = layout.sections[0].header_bounds;
        let cx = hdr.x + hdr.width * 0.5;
        let cy = hdr.y + hdr.height * 0.5;
        let hit = layout.hit_test(cx, cy);
        assert!(
            matches!(hit, MultiSectionViewHit::Header { section: 0, .. }),
            "header click hit was {:?}",
            hit,
        );
    }

    /// Paint↔click round trip at a non-zero origin — a header click AND
    /// a body click must resolve back to section 0. Regression guard for
    /// the "layout helpers must return coords in the same frame across
    /// backends" class of bug (see `macos::multi_section_view`'s
    /// analogous test).
    #[test]
    fn hit_test_resolves_header_and_body_at_nonzero_origin() {
        let view = two_section_view();
        let origin = Rect::new(7.0, 13.0, W, H);
        let layout = win_msv_layout(&view, origin, LINE_HEIGHT);

        let hdr = layout.sections[0].header_bounds;
        let cx = hdr.x + hdr.width * 0.5;
        let cy = hdr.y + hdr.height * 0.5;
        let hit = layout.hit_test(cx, cy);
        assert!(
            matches!(hit, MultiSectionViewHit::Header { section: 0, .. }),
            "header click hit was {:?}",
            hit,
        );

        let body = layout.sections[0].body_bounds;
        assert!(
            body.width > 0.0 && body.height > 0.0,
            "section 0 body must have non-zero size to round-trip a click",
        );
        let bx = body.x + body.width * 0.5;
        let by = body.y + body.height * 0.5;
        let hit = layout.hit_test(bx, by);
        assert!(
            matches!(hit, MultiSectionViewHit::Body { section: 0 }),
            "body click hit was {:?}",
            hit,
        );
    }

    #[test]
    fn collapsed_section_zero_body_height() {
        let mut view = two_section_view();
        view.sections[0].collapsed = true;
        let (_surface, layout) = paint_via(&view);
        let s0 = &layout.sections[0];
        assert_eq!(
            s0.body_bounds.height, 0.0,
            "collapsed section must report zero body height",
        );
    }

    #[test]
    fn metrics_match_gtk_macos_convention() {
        let m = win_msv_metrics(16.0, false);
        assert!((m.header_size - 22.4).abs() < 0.01);
        assert_eq!(m.scrollbar_size, 8.0);
        assert_eq!(m.divider_size, 0.0);
        let m_resize = win_msv_metrics(16.0, true);
        assert_eq!(m_resize.divider_size, 1.0);
    }

    /// No-paint layout must agree byte-for-byte with the layout used
    /// during paint — `win_msv_layout` is a pure fn, so a second call
    /// with the same inputs must produce identical bounds.
    #[test]
    fn no_paint_layout_matches_paint_layout() {
        let view = two_section_view();
        let rect = Rect::new(0.0, 0.0, W, H);
        let (_surface, painted) = paint_via(&view);
        let no_paint = win_msv_layout(&view, rect, LINE_HEIGHT);
        assert_eq!(painted, no_paint);
    }

    /// A section's per-section scrollbar track should paint when its
    /// tree body overflows the section's resolved height — probes a
    /// pixel inside the scrollbar gutter and checks it differs from the
    /// plain background (i.e. something painted there).
    #[test]
    fn overflowing_section_paints_a_scrollbar_track() {
        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![tree_section("alpha", 100)],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };
        let (surface, layout) = paint_via(&view);
        let theme = Theme::default();
        let sb = layout.sections[0]
            .scrollbar_bounds
            .expect("100-row tree section must overflow and get a scrollbar");
        let px = (sb.x + sb.width / 2.0) as u32;
        let py = (sb.y + 2.0) as u32;
        let c = surface.pixel_at(px, py);
        assert_ne!(
            (c.r, c.g, c.b),
            (theme.background.r, theme.background.g, theme.background.b),
            "scrollbar gutter should paint something other than plain background",
        );
    }

    /// Panel-level (`WholePanel`) scrollbar: the painted thumb pixel
    /// matches `theme.scrollbar_thumb`, the track pixel below it matches
    /// `theme.scrollbar_track`, and hit-testing the painted thumb pixel
    /// agrees with the layout. Win-side mirror of GTK's
    /// `gtk_panel_scrollbar_thumb_paints_at_layout_position`.
    ///
    /// Regression coverage for quadraui#820: pre-fix, this backend's
    /// `paint_panel_scrollbar` recomputed thumb geometry itself from
    /// `layout.sections.iter().map(|s| s.resolved_size).sum()` (dropping
    /// the divider strip from the total) and a hardcoded `20.0`-pixel
    /// minimum thumb, instead of reusing `panel_scrollbar_thumb` — the
    /// same geometry `fit_thumb` + `panel_thumb_min` already computed
    /// for hit-testing.
    #[test]
    fn panel_scrollbar_thumb_paints_at_layout_position() {
        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![tree_section("alpha", 11), tree_section("beta", 11)],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: true,
            allow_collapse: true,
            scroll_mode: ScrollMode::WholePanel,
            has_focus: true,
            panel_scroll: 0.0,
        };
        let (surface, layout) = paint_via(&view);
        let theme = Theme::default();

        let panel_sb = layout
            .panel_scrollbar
            .expect("two 11-row sections in a 320px-tall canvas should overflow");
        let thumb = layout
            .panel_scrollbar_thumb
            .expect("panel_scrollbar present implies panel_scrollbar_thumb present");
        assert!(
            thumb.height < panel_sb.height - 4.0,
            "test assumes the thumb doesn't fill the whole track (thumb={thumb:?}, \
             track={panel_sb:?})"
        );

        let px = (panel_sb.x + panel_sb.width / 2.0) as u32;

        // A pixel well inside the painted thumb is the thumb colour.
        let thumb_py = (thumb.y + 3.0) as u32;
        let thumb_c = surface.pixel_at(px, thumb_py);
        assert_eq!(
            (thumb_c.r, thumb_c.g, thumb_c.b),
            (
                theme.scrollbar_thumb.r,
                theme.scrollbar_thumb.g,
                theme.scrollbar_thumb.b
            ),
            "pixel inside panel_scrollbar_thumb should be the thumb colour"
        );

        // A pixel in the track below the thumb is the track colour.
        let track_py = (panel_sb.y + panel_sb.height - 3.0) as u32;
        let track_c = surface.pixel_at(px, track_py);
        assert_eq!(
            (track_c.r, track_c.g, track_c.b),
            (
                theme.scrollbar_track.r,
                theme.scrollbar_track.g,
                theme.scrollbar_track.b
            ),
            "pixel in the track below the thumb should be the track colour"
        );

        // hit_test at the sampled thumb pixel agrees with paint.
        match layout.hit_test(px as f32 + 0.5, thumb_py as f32 + 0.5) {
            MultiSectionViewHit::PanelScrollbar {
                kind: ScrollbarHit::Thumb,
            } => {}
            other => panic!(
                "hit at painted panel-thumb pixel ({}, {}) returned {:?}",
                px, thumb_py, other
            ),
        }
    }

    #[test]
    fn focused_input_paints_caret_bar() {
        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![Section {
                id: "search".into(),
                header: SectionHeader {
                    icon: None,
                    title: StyledText::plain("Search"),
                    badge: None,
                    actions: vec![],
                    show_chevron: true,
                },
                aux: Some(SectionAux::Search(InlineInput {
                    id: WidgetId::new("query"),
                    text: String::new(),
                    caret: 0,
                    placeholder: None,
                    has_focus: true,
                })),
                body: SectionBody::Tree(TreeView {
                    id: WidgetId::new("tree:search"),
                    rows: vec![],
                    selection_mode: SelectionMode::Single,
                    selected_path: None,
                    scroll_offset: 0,
                    style: TreeStyle::default(),
                    has_focus: false,
                }),
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: false,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };
        let (surface, layout) = paint_via(&view);
        let theme = Theme::default();
        let aux = layout.sections[0].aux_bounds.expect("aux bounds present");
        let px = (aux.x + 4.0) as u32;
        let py = (aux.y + aux.height / 2.0) as u32;
        let c = surface.pixel_at(px, py);
        assert_eq!(
            (c.r, c.g, c.b),
            (theme.foreground.r, theme.foreground.g, theme.foreground.b),
            "focused empty input should paint the caret bar at x=aux.x+4",
        );
    }

    /// #701 regression guard: `win_msv_layout`'s body measurement for a
    /// `MessageList` section must agree with
    /// [`crate::primitives::layout_metrics::msv_body_measure`] — the
    /// exact case that drifted pre-#499 (GTK measured real content,
    /// macOS returned `0.0`). `win/` used to re-derive this match arm by
    /// hand (and, before #701, returned `0.0` for `MessageList` too); a
    /// `SectionSize::Content` section's `resolved_size` is
    /// `header_size + body content_size` (no aux), so comparing it
    /// against the shared function directly catches a re-introduced
    /// hand-rolled copy.
    #[test]
    fn msv_body_measure_message_list_matches_shared_layout_metrics() {
        use crate::primitives::message_list::{MessageList, MessageRow};
        use crate::types::Color;

        let body = SectionBody::MessageList(MessageList {
            id: WidgetId::new("messages"),
            rows: vec![
                MessageRow::new("line one\nline two", Color::rgb(255, 255, 255), 0.0),
                MessageRow::new("single line", Color::rgb(255, 255, 255), 0.0),
            ],
            scroll_top: 0,
        });

        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![Section {
                id: "messages".into(),
                header: SectionHeader {
                    icon: None,
                    title: StyledText::plain("Messages"),
                    badge: None,
                    actions: vec![],
                    show_chevron: true,
                },
                body,
                aux: None,
                size: SectionSize::Content,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };

        let rect = Rect::new(0.0, 0.0, W, H);
        let layout = win_msv_layout(&view, rect, LINE_HEIGHT);

        let shared_measure = crate::primitives::layout_metrics::msv_body_measure(
            &view.sections[0].body,
            &view.sections[0].aux,
            LINE_HEIGHT as f64,
        );
        let metrics = win_msv_metrics(LINE_HEIGHT, view.allow_resize);
        let expected_resolved = metrics.header_size + shared_measure.content_size;

        assert!(
            shared_measure.content_size > 0.0,
            "MessageList section must measure non-zero height, got {}",
            shared_measure.content_size
        );
        assert!(
            (layout.sections[0].resolved_size - expected_resolved).abs() < 0.01,
            "win_msv_layout resolved_size {} did not match \
             header_size + layout_metrics::msv_body_measure content_size {} \
             (header_size={}, content_size={})",
            layout.sections[0].resolved_size,
            expected_resolved,
            metrics.header_size,
            shared_measure.content_size,
        );
    }

    /// #727: `SectionBody::MessageList` used to paint background only.
    /// A real MSV paint of a `MessageList` section must now reach
    /// `super::message_list::draw_message_list` and record a text run
    /// for each row — the same `inventory().text_runs()` contract
    /// quadraui#721 established for `WinDriver`, checked here directly
    /// against the recording sink `paint_via` doesn't install.
    #[test]
    fn message_list_section_paints_row_text() {
        use crate::primitives::message_list::{MessageList, MessageRow};
        use crate::types::Color;

        let body = SectionBody::MessageList(MessageList {
            id: WidgetId::new("messages"),
            rows: vec![MessageRow::new(
                "hello from win msv",
                Color::rgb(255, 255, 255),
                0.0,
            )],
            scroll_top: 0,
        });
        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![Section {
                id: "messages".into(),
                header: SectionHeader {
                    icon: None,
                    title: StyledText::plain("Messages"),
                    badge: None,
                    actions: vec![],
                    show_chevron: true,
                },
                body,
                aux: None,
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };

        let previous = crate::testing::install_text_run_sink();
        let (_surface, _layout) = paint_via(&view);
        let runs = crate::testing::take_text_run_sink(previous);

        assert!(
            runs.iter().any(|r| r.text.contains("hello from win msv")),
            "expected a text run for the MessageList row, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
    }

    /// #727: `SectionBody::Terminal` used to paint background only. A
    /// real MSV paint of a `Terminal` section must now reach
    /// `crate::primitives::terminal::paint` (#810) and record a text run
    /// for each non-blank cell — same contract as
    /// `message_list_section_paints_row_text` above.
    #[test]
    fn terminal_section_paints_cell_glyphs() {
        use crate::primitives::terminal::{Terminal, TerminalCell, TerminalCursorShape};
        use crate::types::Color;

        fn cell(ch: char) -> TerminalCell {
            TerminalCell {
                text: ch.to_string(),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(0, 0, 0),
                bold: false,
                italic: false,
                underline: false,
                dim: false,
                selected: false,
                is_cursor: false,
                is_find_match: false,
                is_find_active: false,
                cursor_shape: TerminalCursorShape::Block,
                cursor_blinking: false,
            }
        }

        let body = SectionBody::Terminal(Terminal {
            id: WidgetId::new("term"),
            cells: vec![vec![cell('Q'), cell('X')]],
            scrollbar: None,
        });
        let view = MultiSectionView {
            id: WidgetId::new("msv"),
            sections: vec![Section {
                id: "term".into(),
                header: SectionHeader {
                    icon: None,
                    title: StyledText::plain("Terminal"),
                    badge: None,
                    actions: vec![],
                    show_chevron: true,
                },
                body,
                aux: None,
                size: SectionSize::EqualShare,
                collapsed: false,
                min_size: None,
                max_size: None,
            }],
            active_section: Some(0),
            axis: Axis::Vertical,
            allow_resize: false,
            allow_collapse: true,
            scroll_mode: ScrollMode::PerSection,
            has_focus: true,
            panel_scroll: 0.0,
        };

        let previous = crate::testing::install_text_run_sink();
        let (_surface, _layout) = paint_via(&view);
        let runs = crate::testing::take_text_run_sink(previous);

        assert!(
            runs.iter().any(|r| r.text.contains('Q')),
            "expected a text run for the Terminal cell glyphs, got {:?}",
            runs.iter().map(|r| r.text.as_str()).collect::<Vec<_>>()
        );
    }
}
