//! Pure layout geometry behind `AppShell`'s title-bar / status-bar /
//! command-line / activity-bar / sidebar / divider / main-content carve.
//!
//! Split out of `app_shell.rs` so the carve reads as one function in its
//! own file instead of sitting in the middle of `AppShell`'s builder,
//! event and keyboard-navigation methods. [`compute_layout`] takes a
//! read-only snapshot of exactly the `AppShell` fields it needs
//! ([`ShellLayoutConfig`]) rather than `&AppShell` itself, so this module
//! owns no shell state of its own — `AppShell::layout` builds the
//! snapshot fresh on every call and is this function's only caller.

use crate::types::WidgetId;
use crate::{Rect, Style};

use super::app_shell::{AppShellLayout, BottomBand, ShellPosition};

/// Read-only snapshot of the `AppShell` fields [`compute_layout`] reads.
/// Borrows `bottom_bands` rather than cloning it — the carve only ever
/// iterates it once, in bottom-up order.
pub(crate) struct ShellLayoutConfig<'a> {
    pub(crate) activity_bar_width_px: Option<f32>,
    pub(crate) title_bar_min_px: Option<f32>,
    pub(crate) status_bar_height_px: Option<f32>,
    pub(crate) has_title_bar: bool,
    pub(crate) title_bar_height_lh: f32,
    pub(crate) has_status_bar: bool,
    pub(crate) status_bar_under_main_column: bool,
    pub(crate) has_command_line: bool,
    pub(crate) has_bottom_panel: bool,
    pub(crate) bottom_panel_visible: bool,
    pub(crate) bottom_panel_height_lh: f32,
    pub(crate) min_bottom_panel_height_lh: f32,
    pub(crate) max_bottom_panel_height_lh: f32,
    pub(crate) bottom_bands: &'a [BottomBand],
    pub(crate) sidebar_visible: bool,
    pub(crate) panels_empty: bool,
    pub(crate) position: ShellPosition,
    pub(crate) activity_bar_width: f32,
    pub(crate) sidebar_width: f32,
    pub(crate) min_sidebar_width: f32,
    pub(crate) max_sidebar_width: f32,
}

/// Carve `area` into the shell's chrome bands.
///
/// `snap` quantizes a native-unit extent to what the caller's backend
/// actually paints: the identity function on pixel backends (GUI paints
/// fractional heights exactly), `.round()` on a cell grid (TUI, which
/// snaps to whole rows). `AppShell::layout` derives this from `line_height`
/// alone (no [`crate::Backend`] in hand); `AppShell::render`/
/// `AppShell::handle` pass `backend.snap_height` directly — the two agree
/// because both apply the same above-one-line-height threshold.
///
/// Returns the resolved layout plus the per-band bottom-band bounds in the
/// same bottom-up order `cfg.bottom_bands` was given, for the caller to
/// cache (`AppShell::cached_bottom_band_bounds`).
pub(crate) fn compute_layout(
    cfg: &ShellLayoutConfig<'_>,
    area: Rect,
    line_height: f32,
    style: &Style,
    snap: &dyn Fn(f32) -> f32,
) -> (AppShellLayout, Vec<(WidgetId, Rect)>) {
    // `activity_bar_width_px`, `title_bar_min_px` and `status_bar_height_px`
    // are literal screen pixels. On a cell grid (TUI), `line_height` is
    // architecturally always exactly `1.0` — one character cell, never a
    // real pixel measurement — so a "48px" override would silently become
    // "48 columns". Suppress the three px fields on that backend so the
    // line-height-multiple fields (`activity_bar_width`,
    // `title_bar_height_lh`, and the implicit `snap(lh)` status-bar
    // fallback) keep driving TUI's layout instead.
    //
    // The same threshold also picks the *unset-override* defaults for
    // `status_h`, `ab_w` and `header_h` below: a pixel backend (GUI)
    // defaults to the matching VS Code `Style` chrome metric instead of a
    // `line_height` multiple, while TUI's cell grid keeps the pre-existing
    // `snap(lh)`/`activity_bar_width`-multiple behaviour (`Style`'s own
    // tokens are a documented no-op there — see that module's "Where each
    // token maps on TUI").
    let is_cell_grid = line_height <= 1.0;
    let activity_bar_width_px = cfg.activity_bar_width_px.filter(|_| !is_cell_grid);
    let title_bar_min_px = cfg.title_bar_min_px.filter(|_| !is_cell_grid);
    let status_bar_height_px = cfg.status_bar_height_px.filter(|_| !is_cell_grid);

    let lh = line_height.max(1.0);

    // ── Vertical carve: title bar (top), then status/cmd/bottom (bottom) ──

    let mut band_y = area.y;
    let mut band_h = area.height;

    let title_bar_bounds = if cfg.has_title_bar {
        let h = snap(cfg.title_bar_height_lh * lh).max(title_bar_min_px.unwrap_or(0.0));
        let r = Rect::new(area.x, band_y, area.width, h);
        band_y += h;
        band_h -= h;
        Some(r)
    } else {
        None
    };

    let status_h = if cfg.has_status_bar {
        status_bar_height_px.unwrap_or_else(|| {
            if is_cell_grid {
                snap(lh)
            } else {
                // Floored at `lh`, never shrunk below it — a chrome font
                // taller than the VS Code default must not be clipped
                // against a fixed strip.
                snap(lh.max(style.status_bar_height))
            }
        })
    } else {
        0.0
    };
    let cmd_h = if cfg.has_command_line { snap(lh) } else { 0.0 };

    // Default carve: the status bar and command line consume height from
    // the shared band before the activity bar/sidebar/main split below,
    // full window width — so those columns stop above them. Opted into
    // `status_bar_under_main_column`, the band stays full height here (the
    // activity bar and sidebar run to the area's bottom edge) and the two
    // bars are instead carved from each branch's own main-column rect
    // below, via `carve_status_chrome`.
    let (status_bar_bounds, command_line_bounds) = if cfg.status_bar_under_main_column {
        (None, None)
    } else {
        band_h -= status_h;
        let sb = cfg
            .has_status_bar
            .then(|| Rect::new(area.x, band_y + band_h, area.width, status_h));
        band_h -= cmd_h;
        let cl = cfg
            .has_command_line
            .then(|| Rect::new(area.x, band_y + band_h, area.width, cmd_h));
        (sb, cl)
    };

    // Bottom panel height — computed here but *not* subtracted from the
    // band. The panel docks only under main content (full width minus the
    // sidebar), so it is carved from the main column after the horizontal
    // split below. The sidebar and activity bar run the full band height.
    // In `status_bar_under_main_column` mode `band_h` has not yet lost the
    // status bar/command line height either (that happens per-branch
    // below), so the 60%-of-band clamp is sized against what the main
    // column will actually have left, not the taller shared band.
    let main_column_h = (band_h
        - if cfg.status_bar_under_main_column {
            status_h + cmd_h
        } else {
            0.0
        })
    .max(0.0);
    let bottom_panel_h = if cfg.has_bottom_panel && cfg.bottom_panel_visible {
        let h = snap(
            cfg.bottom_panel_height_lh.clamp(
                cfg.min_bottom_panel_height_lh,
                cfg.max_bottom_panel_height_lh,
            ) * lh,
        );
        Some(h.min(main_column_h * 0.6))
    } else {
        None
    };

    let band_h = band_h.max(0.0);

    // Carve the status bar then the command line from the bottom of a
    // main-column rect — used only in `status_bar_under_main_column` mode.
    // In the default mode those bars were already removed from `band_h`
    // above, at full window width, before this rect existed.
    let carve_status_chrome = |main: Rect| -> (Rect, Option<Rect>, Option<Rect>) {
        let mut remaining = main;
        let status = if cfg.has_status_bar {
            let h = status_h.min(remaining.height);
            let above_h = (remaining.height - h).max(0.0);
            let rect = Rect::new(remaining.x, remaining.y + above_h, remaining.width, h);
            remaining = Rect::new(remaining.x, remaining.y, remaining.width, above_h);
            Some(rect)
        } else {
            None
        };
        let cmd = if cfg.has_command_line {
            let h = cmd_h.min(remaining.height);
            let above_h = (remaining.height - h).max(0.0);
            let rect = Rect::new(remaining.x, remaining.y + above_h, remaining.width, h);
            remaining = Rect::new(remaining.x, remaining.y, remaining.width, above_h);
            Some(rect)
        } else {
            None
        };
        (remaining, status, cmd)
    };

    // Split a main-column rect into (content_above, bottom_panel + N
    // bottom_bands). Bottom bands are carved first, stacked bottom-up
    // flush against `main`'s bottom edge in list order (index 0 lowest);
    // the legacy single bottom-panel drawer is then carved from what
    // remains, so it always sits directly above every band — i.e. it
    // behaves as one more, implicit band on top of the list.
    let carve_bottom_chrome = |main: Rect| -> (Rect, Vec<(WidgetId, Rect)>, Option<Rect>) {
        let mut remaining = main;
        let mut band_bounds = Vec::new();
        for band in cfg.bottom_bands {
            if !band.visible {
                continue;
            }
            let h = snap(band.height_lh * lh).clamp(0.0, remaining.height);
            let above_h = (remaining.height - h).max(0.0);
            let rect = Rect::new(remaining.x, remaining.y + above_h, remaining.width, h);
            remaining = Rect::new(remaining.x, remaining.y, remaining.width, above_h);
            band_bounds.push((band.id, rect));
        }
        let bottom_panel_bounds = match bottom_panel_h {
            Some(h) => {
                let h = h.min(remaining.height);
                let above_h = (remaining.height - h).max(0.0);
                let panel = Rect::new(remaining.x, remaining.y + above_h, remaining.width, h);
                remaining = Rect::new(remaining.x, remaining.y, remaining.width, above_h);
                Some(panel)
            }
            None => None,
        };
        (remaining, band_bounds, bottom_panel_bounds)
    };

    // Combine the two carves above into the one step every branch below
    // runs on its own `main_bounds` rect: in `status_bar_under_main_column`
    // mode this carves the status bar and command line from the main
    // column first, then the bottom bands/panel above them, so the full
    // bottom-up stack (status bar, command line, bands, panel) lives
    // entirely within the main column; otherwise the status bar and
    // command line bounds computed above (full window width) pass through
    // untouched and only the bottom bands/panel are carved here.
    // (remaining main column, status bar bounds, command line bounds,
    // bottom band bounds, bottom panel bounds).
    type MainColumnChrome = (
        Rect,
        Option<Rect>,
        Option<Rect>,
        Vec<(WidgetId, Rect)>,
        Option<Rect>,
    );
    let finish_main_column = |main: Rect| -> MainColumnChrome {
        let (main, status, cmd) = if cfg.status_bar_under_main_column {
            carve_status_chrome(main)
        } else {
            (main, status_bar_bounds, command_line_bounds)
        };
        let (main, bands, panel) = carve_bottom_chrome(main);
        (main, status, cmd, bands, panel)
    };

    // ── Horizontal carve: activity bar + sidebar + divider + main ──
    //
    // Band widths (`ab_w`, `sidebar_w`, `divider_w`, `main_w`) are still
    // decided by hand — they depend on clamping/min/max rules that are
    // AppShell's own policy, not a layout concern. Once the widths are
    // known, turning them into left-to-right x-positions is exactly the
    // one-dimensional packing `crate::flex::pack_1d` already does for
    // `Toolbar` and `Form` (same shared engine, see those modules), so
    // `row_x_positions` delegates to it under the `layout` feature.

    let ab_w = activity_bar_width_px.unwrap_or_else(|| {
        if is_cell_grid {
            snap(cfg.activity_bar_width * lh)
        } else {
            snap(style.activity_bar_width)
        }
    });
    let divider_w = snap(lh * 0.25).clamp(1.0, 4.0);

    if !cfg.sidebar_visible || cfg.panels_empty {
        let main_w = (area.width - ab_w).max(0.0);
        let (ab_bounds, main_bounds) = match cfg.position {
            ShellPosition::Left => {
                let x = row_x_positions(area.x, &[ab_w, main_w]);
                (
                    Rect::new(x[0], band_y, ab_w, band_h),
                    Rect::new(x[1], band_y, main_w, band_h),
                )
            }
            ShellPosition::Right => {
                let x = row_x_positions(area.x, &[main_w, ab_w]);
                (
                    Rect::new(x[1], band_y, ab_w, band_h),
                    Rect::new(x[0], band_y, main_w, band_h),
                )
            }
        };
        let (
            main_bounds,
            status_bar_bounds,
            command_line_bounds,
            bottom_band_bounds,
            bottom_panel_bounds,
        ) = finish_main_column(main_bounds);
        return (
            AppShellLayout {
                window_bounds: area,
                title_bar_bounds,
                activity_bar_bounds: ab_bounds,
                sidebar_header_bounds: None,
                sidebar_content_bounds: None,
                divider_bounds: None,
                main_content_bounds: main_bounds,
                bottom_panel_bounds,
                command_line_bounds,
                status_bar_bounds,
            },
            bottom_band_bounds,
        );
    }

    let sidebar_w = snap(
        cfg.sidebar_width
            .clamp(cfg.min_sidebar_width, cfg.max_sidebar_width)
            * lh,
    );
    let remaining = (area.width - ab_w - divider_w).max(0.0);
    let sidebar_w = sidebar_w.min(remaining * 0.8);
    // The sidebar header strip (e.g. "EXPLORER") paints through the same
    // chrome rendering as a tab strip, so a pixel backend sizes it off
    // `Style::tab_height` instead of the raw chrome line height — floored
    // at `lh` so a chrome font taller than the token isn't clipped; TUI's
    // cell grid keeps the one-row default.
    let header_h = if is_cell_grid {
        lh
    } else {
        snap(lh.max(style.tab_height))
    };

    let main_w = (area.width - ab_w - sidebar_w - divider_w).max(0.0);

    match cfg.position {
        ShellPosition::Left => {
            let x = row_x_positions(area.x, &[ab_w, sidebar_w, divider_w, main_w]);
            let ab_bounds = Rect::new(x[0], band_y, ab_w, band_h);
            let sidebar_x = x[1];
            let header_bounds = Rect::new(sidebar_x, band_y, sidebar_w, header_h);
            let content_y = band_y + header_h;
            let content_h = (band_h - header_h).max(0.0);
            let content_bounds = Rect::new(sidebar_x, content_y, sidebar_w, content_h);
            let div_bounds = Rect::new(x[2], band_y, divider_w, band_h);
            let main_bounds = Rect::new(x[3], band_y, main_w, band_h);
            let (
                main_bounds,
                status_bar_bounds,
                command_line_bounds,
                bottom_band_bounds,
                bottom_panel_bounds,
            ) = finish_main_column(main_bounds);

            (
                AppShellLayout {
                    window_bounds: area,
                    title_bar_bounds,
                    activity_bar_bounds: ab_bounds,
                    sidebar_header_bounds: Some(header_bounds),
                    sidebar_content_bounds: Some(content_bounds),
                    divider_bounds: Some(div_bounds),
                    main_content_bounds: main_bounds,
                    bottom_panel_bounds,
                    command_line_bounds,
                    status_bar_bounds,
                },
                bottom_band_bounds,
            )
        }
        ShellPosition::Right => {
            let x = row_x_positions(area.x, &[main_w, divider_w, sidebar_w, ab_w]);
            let ab_bounds = Rect::new(x[3], band_y, ab_w, band_h);
            let sidebar_x = x[2];
            let header_bounds = Rect::new(sidebar_x, band_y, sidebar_w, header_h);
            let content_y = band_y + header_h;
            let content_h = (band_h - header_h).max(0.0);
            let content_bounds = Rect::new(sidebar_x, content_y, sidebar_w, content_h);
            let div_bounds = Rect::new(x[1], band_y, divider_w, band_h);
            let main_bounds = Rect::new(x[0], band_y, main_w, band_h);
            let (
                main_bounds,
                status_bar_bounds,
                command_line_bounds,
                bottom_band_bounds,
                bottom_panel_bounds,
            ) = finish_main_column(main_bounds);

            (
                AppShellLayout {
                    window_bounds: area,
                    title_bar_bounds,
                    activity_bar_bounds: ab_bounds,
                    sidebar_header_bounds: Some(header_bounds),
                    sidebar_content_bounds: Some(content_bounds),
                    divider_bounds: Some(div_bounds),
                    main_content_bounds: main_bounds,
                    bottom_panel_bounds,
                    command_line_bounds,
                    status_bar_bounds,
                },
                bottom_band_bounds,
            )
        }
    }
}

/// Turn a run of `sizes` into left-to-right x-positions starting at
/// `origin`, each the previous slot's x plus its width.
///
/// With the `layout` feature this runs through [`crate::flex`]'s shared
/// flexbox engine — the same one `Toolbar` and `Form` pack their rows
/// through. Without it, the same arithmetic is done by hand.
#[cfg(feature = "layout")]
fn row_x_positions(origin: f32, sizes: &[f32]) -> Vec<f32> {
    crate::flex::pack_1d(crate::flex::FlexDirection::Row, origin, sizes, 0.0, 0.0)
}

/// Same contract as the `layout`-feature arm above, computed by hand.
#[cfg(not(feature = "layout"))]
fn row_x_positions(origin: f32, sizes: &[f32]) -> Vec<f32> {
    let mut x = origin;
    let mut xs = Vec::with_capacity(sizes.len());
    for &s in sizes {
        xs.push(x);
        x += s;
    }
    xs
}
