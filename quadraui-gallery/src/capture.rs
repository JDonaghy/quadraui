//! Headless capture mode (#1348): iterate every registered demo ×
//! variant × backend and write one image per combination, plus a
//! `manifest.json` describing what was (or wasn't) captured.
//!
//! [`run_capture`] is the entry point `src/main.rs`'s `--capture <dir>`
//! flag calls, and the same function a test calls directly (see
//! `tests/capture_driver.rs`) — there is no separate "CLI" code path to
//! drift from the one under test.
//!
//! ## Per-backend story
//!
//! - **TUI**: drives the real [`GalleryApp`] through
//!   [`quadraui::tui::testing::driver_with_shell`] (the exact path
//!   `tests/gallery_driver.rs` already exercises) and emits one **SVG**
//!   per (demo, variant) — cells become `<rect>` (background) +
//!   `<text>` (foreground) runs. This is the primary deliverable: it
//!   needs no system dependency beyond this crate's own `quadraui/tui`
//!   feature, and it is the one path CI can run on every OS.
//! - **macOS**: drives the same `GalleryApp` through
//!   [`quadraui::macos::testing::driver_with_shell`] into a headless
//!   [`quadraui::macos::headless::BitmapSurface`], then PNG-encodes its
//!   raw RGBA bytes via the `image` crate. Only compiles/runs under
//!   `all(feature = "macos", target_os = "macos")` — see that feature's
//!   `Cargo.toml` comment.
//! - **GTK / Windows**: neither backend has a headless capture path in
//!   this repo yet (GTK waits on `GtkDriver`, quadraui#301; Windows has
//!   no gallery-level driver at all). Every (demo, variant) combination
//!   gets a `"status": "capture-pending"` manifest entry instead of a
//!   faked image, regardless of which Cargo features this binary was
//!   built with — the gap is in the harness, not the build.
//!
//! When a backend's own feature isn't compiled into this binary at all
//! (e.g. a `tui`-only build has no `quadraui::macos` module to drive),
//! its entries get `"status": "unsupported"` instead — distinct from
//! `"capture-pending"` because it isn't a permanent gap in this crate,
//! just this particular build/host.

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;

use crate::registry::registry;
#[cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
use crate::{
    app::{group_panel_id, GROUPS},
    GalleryApp,
};

/// One row of `manifest.json`: which (demo, variant, backend)
/// combination this is, and either where its image landed (`path`,
/// relative to the capture directory) or why there isn't one
/// (`status`) — never both, never neither.
#[derive(Debug, Clone, Serialize)]
pub struct ManifestEntry {
    pub demo: String,
    pub group: String,
    pub variant: String,
    pub backend: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Human-readable reason for `status`, e.g. naming the tracking
    /// issue a `capture-pending` gap is blocked on. `None` whenever
    /// `status` is `None` (a real `path` entry needs no explanation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Cell grid size every TUI capture renders at — generous enough that
/// `GalleryApp`'s chrome (activity bar + sidebar + tab strip) and a
/// demo's own content both fit without the registry-driven loop having
/// to special-case any one demo's minimum size.
#[cfg(feature = "tui")]
const TUI_CAPTURE_WIDTH: u16 = 120;
#[cfg(feature = "tui")]
const TUI_CAPTURE_HEIGHT: u16 = 36;

/// Pixel surface size every macOS capture renders at — the point-unit
/// analogue of [`TUI_CAPTURE_WIDTH`]/[`TUI_CAPTURE_HEIGHT`], picked to
/// comfortably fit the same chrome at a readable scale.
#[cfg(all(feature = "macos", target_os = "macos"))]
const MAC_CAPTURE_WIDTH: u32 = 960;
#[cfg(all(feature = "macos", target_os = "macos"))]
const MAC_CAPTURE_HEIGHT: u32 = 600;

/// Capture every registered demo × variant × backend into `dir`,
/// writing one image per combination plus `dir/manifest.json`, and
/// return the manifest entries written.
///
/// Creates `dir` (and any missing parents) if it doesn't exist yet.
pub fn run_capture(dir: &Path) -> io::Result<Vec<ManifestEntry>> {
    fs::create_dir_all(dir)?;
    let mut manifest = Vec::new();

    #[cfg(feature = "tui")]
    capture_tui(dir, &mut manifest)?;
    #[cfg(not(feature = "tui"))]
    push_status(
        &mut manifest,
        "tui",
        "unsupported",
        "quadraui-gallery was built without the `tui` feature",
    );

    #[cfg(all(feature = "macos", target_os = "macos"))]
    capture_macos(dir, &mut manifest)?;
    #[cfg(not(all(feature = "macos", target_os = "macos")))]
    push_status(
        &mut manifest,
        "macos",
        "unsupported",
        "macOS headless capture needs --features macos on a target_os = \"macos\" host",
    );

    push_status(
        &mut manifest,
        "gtk",
        "capture-pending",
        "no headless GTK capture path exists yet — blocked on GtkDriver (quadraui#301)",
    );
    push_status(
        &mut manifest,
        "win",
        "capture-pending",
        "no headless Windows gallery driver exists yet",
    );

    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(dir.join("manifest.json"), manifest_json)?;
    Ok(manifest)
}

/// Append one `status`-only entry (no `path`) per registered (demo,
/// variant) for `backend` — the shared body behind every "this backend
/// has no image to show" branch in [`run_capture`], so `unsupported`
/// and `capture-pending` entries always carry the same demo/group/
/// variant shape the real captures do.
fn push_status(manifest: &mut Vec<ManifestEntry>, backend: &str, status: &str, why: &str) {
    for demo in registry() {
        let group = demo.group().to_string();
        for variant in demo.variants() {
            manifest.push(ManifestEntry {
                demo: demo.name().to_string(),
                group: group.clone(),
                variant: variant.to_string(),
                backend: backend.to_string(),
                path: None,
                status: Some(status.to_string()),
                note: Some(why.to_string()),
            });
        }
    }
}

/// Slugify `s` into a filesystem- and URL-safe fragment: lowercase,
/// non-alphanumeric runs collapsed to a single `-`. Used to build each
/// capture's filename from its demo/variant name.
#[cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !out.is_empty() {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("demo");
    }
    out
}

/// The activity-bar group index [`GROUPS`] registers `demo`'s own
/// [`crate::Demo::group`] under. Panics (rather than skipping) on a
/// mismatch — the same invariant `tests/gallery_driver.rs`'s
/// registry-driven test already asserts, so a typo'd group name fails
/// loudly here too instead of silently capturing nothing for that demo.
#[cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
fn group_index_of(demo: &dyn crate::Demo) -> usize {
    GROUPS
        .iter()
        .position(|g| *g == demo.group())
        .unwrap_or_else(|| {
            panic!(
                "{}'s group {:?} is not one of GROUPS — fix its Demo::group impl",
                demo.name(),
                demo.group()
            )
        })
}

// ── TUI capture ──────────────────────────────────────────────────────

#[cfg(feature = "tui")]
fn capture_tui(dir: &Path, manifest: &mut Vec<ManifestEntry>) -> io::Result<()> {
    use quadraui::testing::{Anchor, ConformanceDriver};
    use quadraui::tui::testing::driver_with_shell;

    for demo in registry() {
        let group = demo.group().to_string();
        let group_idx = group_index_of(demo.as_ref());
        let variants = demo.variants();
        for &variant in variants {
            let mut driver = driver_with_shell(
                GalleryApp::new(),
                GalleryApp::config(),
                TUI_CAPTURE_WIDTH,
                TUI_CAPTURE_HEIGHT,
            );
            click_group_zone(&mut driver, group_idx);
            driver.click_text_at(demo.name(), Anchor::Center);
            if variants.len() > 1 {
                driver.click_text_at(variant, Anchor::Center);
            }

            let filename = format!("tui__{}__{}.svg", slug(demo.name()), slug(variant));
            write_tui_svg(
                &driver,
                TUI_CAPTURE_WIDTH,
                TUI_CAPTURE_HEIGHT,
                &dir.join(&filename),
            )?;

            manifest.push(ManifestEntry {
                demo: demo.name().to_string(),
                group: group.clone(),
                variant: variant.to_string(),
                backend: "tui".to_string(),
                path: Some(filename),
                status: None,
                note: None,
            });
        }
    }
    Ok(())
}

/// Click [`GROUPS`]`[index]`'s activity-bar item at its real painted
/// bounds, resolved from [`quadraui::testing::ConformanceDriver::inventory`]'s
/// registered zone for [`group_panel_id`] — the same pattern
/// `tests/gallery_driver.rs::click_group_zone` uses, generic here over
/// any driver that implements both `ConformanceDriver` (for
/// `inventory()`) and `DriverInput` (for `click`), so this one body
/// drives TUI and macOS navigation alike.
#[cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
fn click_group_zone<D>(driver: &mut D, index: usize)
where
    D: quadraui::testing::ConformanceDriver + quadraui::testing::DriverInput,
{
    let id = group_panel_id(index);
    let zone = driver
        .inventory()
        .zones()
        .iter()
        .find(|z| z.id == id)
        .unwrap_or_else(|| panic!("no registered zone for group panel {index}"))
        .bounds;
    driver.click(zone.x + zone.width / 2.0, zone.y + zone.height / 2.0);
}

/// Render `driver`'s current `width`×`height` screen to an SVG file at
/// `path`: one `<rect>` per run of same-background cells, one `<text>`
/// per run of same-foreground/same-style cells — the smallest
/// cells-to-vector-art mapping that still reads as a terminal
/// screenshot (background fills behind glyph runs, not per-cell
/// boxes).
#[cfg(feature = "tui")]
fn write_tui_svg<A: quadraui::AppLogic>(
    driver: &quadraui::tui::testing::TuiDriver<A>,
    width: u16,
    height: u16,
    path: &Path,
) -> io::Result<()> {
    use quadraui::tui::testing::Modifier;

    // Cell pixel size — a 1:2 width:height ratio typical of monospace
    // terminal fonts (e.g. Menlo, SF Mono) at a readable size.
    const CELL_W: u32 = 9;
    const CELL_H: u32 = 18;
    let px_w = u32::from(width) * CELL_W;
    let px_h = u32::from(height) * CELL_H;

    let mut svg = String::new();
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{px_w}\" height=\"{px_h}\" \
         viewBox=\"0 0 {px_w} {px_h}\" font-family=\"'SF Mono','DejaVu Sans Mono',monospace\" \
         font-size=\"{}\">\n",
        CELL_H - 4
    ));
    svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"#101010\"/>\n");

    for y in 0..height {
        let row = driver.styled_row(y);

        // Background runs.
        let mut x = 0usize;
        while x < row.len() {
            let bg = row[x].1.bg;
            let mut end = x + 1;
            while end < row.len() && row[end].1.bg == bg {
                end += 1;
            }
            if let Some((r, g, b)) = ansi_rgb(bg) {
                svg.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{CELL_H}\" fill=\"#{r:02x}{g:02x}{b:02x}\"/>\n",
                    x as u32 * CELL_W,
                    u32::from(y) * CELL_H,
                    (end - x) as u32 * CELL_W,
                ));
            }
            x = end;
        }

        // Foreground glyph runs.
        let mut x = 0usize;
        while x < row.len() {
            let style0 = row[x].1;
            let mut end = x + 1;
            while end < row.len()
                && row[end].1.fg == style0.fg
                && row[end].1.modifiers == style0.modifiers
            {
                end += 1;
            }
            let text: String = row[x..end].iter().map(|(ch, _)| *ch).collect();
            if !text.trim().is_empty() {
                let (r, g, b) = ansi_rgb(style0.fg).unwrap_or((230, 230, 230));
                let weight = if style0.modifiers.contains(Modifier::BOLD) {
                    " font-weight=\"bold\""
                } else {
                    ""
                };
                svg.push_str(&format!(
                    "<text x=\"{}\" y=\"{}\" fill=\"#{r:02x}{g:02x}{b:02x}\"{weight}>{}</text>\n",
                    x as u32 * CELL_W,
                    u32::from(y) * CELL_H + CELL_H - 4,
                    xml_escape(&text),
                ));
            }
            x = end;
        }
    }

    svg.push_str("</svg>\n");
    fs::write(path, svg)
}

/// Approximate RGB for a ratatui [`Color`](quadraui::tui::testing::Color)
/// — the standard 16-colour ANSI palette plus the 256-colour cube/
/// grayscale ramp, matching common terminal defaults closely enough for
/// a visual capture. `None` for `Reset`, so callers fall back to the
/// SVG's own base fill / default text fill instead of painting an
/// opinionated "default colour" rect/run.
#[cfg(feature = "tui")]
fn ansi_rgb(color: quadraui::tui::testing::Color) -> Option<(u8, u8, u8)> {
    use quadraui::tui::testing::Color::*;
    Some(match color {
        Reset => return None,
        Black => (12, 12, 12),
        Red => (205, 49, 49),
        Green => (13, 188, 121),
        Yellow => (229, 229, 16),
        Blue => (36, 114, 200),
        Magenta => (188, 63, 188),
        Cyan => (17, 168, 205),
        Gray => (229, 229, 229),
        DarkGray => (102, 102, 102),
        LightRed => (241, 76, 76),
        LightGreen => (35, 209, 139),
        LightYellow => (245, 245, 67),
        LightBlue => (59, 142, 234),
        LightMagenta => (214, 112, 214),
        LightCyan => (41, 184, 219),
        White => (255, 255, 255),
        Rgb(r, g, b) => (r, g, b),
        Indexed(i) => indexed_to_rgb(i),
    })
}

/// xterm 256-colour index → RGB: the first 16 mirror the ANSI palette
/// (via [`ansi_rgb`]'s own table, re-derived here rather than shared —
/// `Color` has no `From<u8>` that round-trips through the named
/// variants), 16–231 are the 6×6×6 colour cube, 232–255 the grayscale
/// ramp.
#[cfg(feature = "tui")]
fn indexed_to_rgb(i: u8) -> (u8, u8, u8) {
    const BASIC16: [(u8, u8, u8); 16] = [
        (12, 12, 12),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    match i {
        0..=15 => BASIC16[i as usize],
        16..=231 => {
            let i = i - 16;
            let r = i / 36;
            let g = (i % 36) / 6;
            let b = i % 6;
            let scale = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            (scale(r), scale(g), scale(b))
        }
        232..=255 => {
            let v = 8 + u16::from(i - 232) * 10;
            (v as u8, v as u8, v as u8)
        }
    }
}

/// Escape `&`/`<`/`>` for safe inclusion inside an SVG `<text>` element.
#[cfg(feature = "tui")]
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

// ── macOS capture ────────────────────────────────────────────────────

#[cfg(all(feature = "macos", target_os = "macos"))]
fn capture_macos(dir: &Path, manifest: &mut Vec<ManifestEntry>) -> io::Result<()> {
    use quadraui::macos::testing::driver_with_shell;
    use quadraui::testing::{Anchor, ConformanceDriver};

    for demo in registry() {
        let group = demo.group().to_string();
        let group_idx = group_index_of(demo.as_ref());
        let variants = demo.variants();
        for &variant in variants {
            let mut driver = driver_with_shell(
                GalleryApp::new(),
                GalleryApp::config(),
                MAC_CAPTURE_WIDTH,
                MAC_CAPTURE_HEIGHT,
            );
            click_group_zone(&mut driver, group_idx);
            driver.click_text_at(demo.name(), Anchor::Center);
            if variants.len() > 1 {
                driver.click_text_at(variant, Anchor::Center);
            }

            let filename = format!("macos__{}__{}.png", slug(demo.name()), slug(variant));
            write_mac_png(
                driver.surface(),
                MAC_CAPTURE_WIDTH,
                MAC_CAPTURE_HEIGHT,
                &dir.join(&filename),
            )?;

            manifest.push(ManifestEntry {
                demo: demo.name().to_string(),
                group: group.clone(),
                variant: variant.to_string(),
                backend: "macos".to_string(),
                path: Some(filename),
                status: None,
                note: None,
            });
        }
    }
    Ok(())
}

/// PNG-encode a [`BitmapSurface`](quadraui::macos::headless::BitmapSurface)'s
/// raw RGBA bytes (see that type's module doc for the byte layout —
/// top-down scanlines, 8bpc RGBA, which is exactly what
/// [`image::RgbaImage::from_raw`] expects).
#[cfg(all(feature = "macos", target_os = "macos"))]
fn write_mac_png(
    surface: &quadraui::macos::headless::BitmapSurface,
    width: u32,
    height: u32,
    path: &Path,
) -> io::Result<()> {
    let img = image::RgbaImage::from_raw(width, height, surface.bytes().to_vec())
        .expect("BitmapSurface byte length must be exactly width * height * 4");
    img.save(path)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

// Every test below exercises either the TUI-only SVG-encoding helpers or
// `slug`, which is itself only compiled under the same `any(tui, macos)`
// gate as the capture functions that call it — see those items' own `#[cfg]`
// above. Gating the whole module keeps a `gtk`/`win`-only or no-feature
// build (which compiles none of those helpers) from failing on an unused
// `use super::*` instead of just having nothing to test here.
#[cfg(all(
    test,
    any(feature = "tui", all(feature = "macos", target_os = "macos"))
))]
mod tests {
    use super::*;

    #[test]
    fn slug_lowercases_and_collapses_separators() {
        assert_eq!(slug("Toast"), "toast");
        assert_eq!(slug("Two Variant Demo!"), "two-variant-demo");
        assert_eq!(slug("default"), "default");
        assert_eq!(slug("---"), "demo");
        assert_eq!(slug(""), "demo");
    }

    #[cfg(feature = "tui")]
    #[test]
    fn ansi_rgb_maps_reset_to_none_and_rgb_through() {
        use quadraui::tui::testing::Color;
        assert_eq!(ansi_rgb(Color::Reset), None);
        assert_eq!(ansi_rgb(Color::Rgb(1, 2, 3)), Some((1, 2, 3)));
        assert_eq!(ansi_rgb(Color::White), Some((255, 255, 255)));
    }

    #[cfg(feature = "tui")]
    #[test]
    fn indexed_to_rgb_matches_basic16_for_low_indices() {
        assert_eq!(
            indexed_to_rgb(1),
            ansi_rgb(quadraui::tui::testing::Color::Red).unwrap()
        );
        assert_eq!(
            indexed_to_rgb(15),
            ansi_rgb(quadraui::tui::testing::Color::White).unwrap()
        );
    }

    #[cfg(feature = "tui")]
    #[test]
    fn xml_escape_handles_reserved_characters() {
        assert_eq!(xml_escape("a & b <c> d"), "a &amp; b &lt;c&gt; d");
    }
}
