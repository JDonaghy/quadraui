//! Headless capture mode: iterate every registered demo × variant ×
//! backend and write one image per combination, plus a `manifest.json`
//! describing what was (or wasn't) captured.
//!
//! ## Manifest composition across capture runs
//!
//! Each run of [`run_capture`] writes one `manifest.json` covering every
//! backend compiled into *that* binary — a `tui`-only build's manifest
//! marks `macos` (and every other backend it can't reach) `unsupported`,
//! and a `macos`-only build's manifest marks `tui` the same way. The
//! Linux and macOS CI legs each produce their own capture directory and
//! manifest for exactly this reason. A publishing step that wants one
//! gallery covering every backend needs to merge manifests keyed by
//! `(demo, group, variant, backend)`, preferring any row with a `path`
//! over the same key's `unsupported`/`capture-pending` row from another
//! run — this module does not do that merge itself.
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
//!
//! ## Per-(demo, variant) failure isolation
//!
//! A single demo/variant that fails to capture — an I/O error writing
//! its image, or a panic inside the click sequence that drives it to
//! the right screen (e.g. a label that isn't painted at the capture
//! size) — gets its own `"status": "error"` manifest row (with a
//! `note` carrying the failure's own message) instead of aborting the
//! whole run. Every other demo/variant/backend, and `manifest.json`
//! itself, still gets written.

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
    /// Human-readable reason for `status`: the tracking issue a
    /// `capture-pending` gap is blocked on, or the error/panic message
    /// for an `"error"` row. `None` whenever `status` is `None` (a real
    /// `path` entry needs no explanation).
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
    capture_tui_over(registry, dir, manifest)
}

/// [`capture_tui`]'s body, generic over the demo-list constructor so a
/// test can drive it with a demo double instead of the production
/// [`registry`] — in particular to exercise the `variants.len() > 1`
/// click branch, which [`registry`] itself never reaches today (its one
/// entry, `ToastDemo`, has a single variant). Takes a `fn() ->
/// Vec<Box<dyn Demo>>` (the same shape as [`registry`] itself), not an
/// already-built `Vec`, because each (demo, variant) iteration needs
/// its own fresh [`GalleryApp`] built from the *whole* demo list (so
/// its sidebar/activity-bar navigation works), not just the one demo
/// being captured that iteration.
#[cfg(feature = "tui")]
fn capture_tui_over(
    demos_fn: fn() -> Vec<Box<dyn crate::Demo>>,
    dir: &Path,
    manifest: &mut Vec<ManifestEntry>,
) -> io::Result<()> {
    use quadraui::testing::{Anchor, ConformanceDriver};
    use quadraui::tui::testing::driver_with_shell;

    for demo in demos_fn() {
        let group = demo.group().to_string();
        let group_idx = group_index_of(demo.as_ref());
        let variants = demo.variants();
        for &variant in variants {
            let filename = format!(
                "tui__{}__{}__{}.svg",
                slug(&group),
                slug(demo.name()),
                slug(variant)
            );
            let dest = dir.join(&filename);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut driver = driver_with_shell(
                    GalleryApp::from_demos(demos_fn()),
                    GalleryApp::config(),
                    TUI_CAPTURE_WIDTH,
                    TUI_CAPTURE_HEIGHT,
                );
                click_group_zone(&mut driver, group_idx);
                driver.click_text_at(demo.name(), Anchor::Center);
                if variants.len() > 1 {
                    driver.click_text_at(variant, Anchor::Center);
                }
                write_tui_svg(&driver, TUI_CAPTURE_WIDTH, TUI_CAPTURE_HEIGHT, &dest)
            }));

            manifest.push(capture_outcome_entry(
                demo.name(),
                &group,
                variant,
                "tui",
                filename,
                outcome,
            ));
        }
    }
    Ok(())
}

/// Turn one (demo, variant, backend) capture attempt's outcome into its
/// manifest row: a successful write gets a real `path`; an I/O failure
/// or a caught panic (e.g.
/// [`quadraui::testing::ConformanceDriver::click_text_at`] not finding
/// its label at this capture size) gets a `status: "error"` row
/// carrying the failure's own message instead — so one broken demo
/// can't take down every other row in the same capture run, including
/// `manifest.json` itself.
#[cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
fn capture_outcome_entry(
    demo: &str,
    group: &str,
    variant: &str,
    backend: &str,
    filename: String,
    outcome: std::thread::Result<io::Result<()>>,
) -> ManifestEntry {
    let (path, status, note) = match outcome {
        Ok(Ok(())) => (Some(filename), None, None),
        Ok(Err(io_err)) => (None, Some("error".to_string()), Some(io_err.to_string())),
        Err(panic_payload) => (
            None,
            Some("error".to_string()),
            Some(panic_message(&panic_payload)),
        ),
    };
    ManifestEntry {
        demo: demo.to_string(),
        group: group.to_string(),
        variant: variant.to_string(),
        backend: backend.to_string(),
        path,
        status,
        note,
    }
}

/// Extract a human-readable message from a caught panic payload.
/// `&'static str` (`panic!("literal")`) and `String`
/// (`panic!("{}", ...)`/`unwrap_or_else`) cover every panicking call
/// site reachable from this module's own capture path; anything else
/// falls back to a generic description rather than losing the manifest
/// row entirely.
#[cfg(any(feature = "tui", all(feature = "macos", target_os = "macos")))]
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "capture panicked with a non-string payload".to_string()
    }
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
         font-size=\"{}\" xml:space=\"preserve\">\n",
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
                    "<text x=\"{}\" y=\"{}\" textLength=\"{}\" lengthAdjust=\"spacingAndGlyphs\" \
                     fill=\"#{r:02x}{g:02x}{b:02x}\"{weight}>{}</text>\n",
                    x as u32 * CELL_W,
                    u32::from(y) * CELL_H + CELL_H - 4,
                    (end - x) as u32 * CELL_W,
                    xml_escape(&text),
                ));
            }
            x = end;
        }
    }

    svg.push_str("</svg>\n");
    fs::write(path, svg)
}

/// The standard 16-colour ANSI palette, in `Color::Black..=Color::White`
/// order — shared by [`ansi_rgb`] (named variants) and [`indexed_to_rgb`]
/// (indices `0..=15`) so the two tables can't drift apart.
#[cfg(feature = "tui")]
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

/// Approximate RGB for a ratatui [`Color`](quadraui::tui::testing::Color)
/// — [`BASIC16`] plus the 256-colour cube/grayscale ramp, matching
/// common terminal defaults closely enough for a visual capture. `None`
/// for `Reset`, so callers fall back to the SVG's own base fill /
/// default text fill instead of painting an opinionated "default
/// colour" rect/run.
#[cfg(feature = "tui")]
fn ansi_rgb(color: quadraui::tui::testing::Color) -> Option<(u8, u8, u8)> {
    use quadraui::tui::testing::Color::*;
    Some(match color {
        Reset => return None,
        Black => BASIC16[0],
        Red => BASIC16[1],
        Green => BASIC16[2],
        Yellow => BASIC16[3],
        Blue => BASIC16[4],
        Magenta => BASIC16[5],
        Cyan => BASIC16[6],
        Gray => BASIC16[7],
        DarkGray => BASIC16[8],
        LightRed => BASIC16[9],
        LightGreen => BASIC16[10],
        LightYellow => BASIC16[11],
        LightBlue => BASIC16[12],
        LightMagenta => BASIC16[13],
        LightCyan => BASIC16[14],
        White => BASIC16[15],
        Rgb(r, g, b) => (r, g, b),
        Indexed(i) => indexed_to_rgb(i),
    })
}

/// xterm 256-colour index → RGB: `0..=15` are [`BASIC16`], `16..=231`
/// are the 6×6×6 colour cube, `232..=255` the grayscale ramp.
#[cfg(feature = "tui")]
fn indexed_to_rgb(i: u8) -> (u8, u8, u8) {
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
            let filename = format!(
                "macos__{}__{}__{}.png",
                slug(&group),
                slug(demo.name()),
                slug(variant)
            );
            let dest = dir.join(&filename);
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
                write_mac_png(
                    driver.surface(),
                    MAC_CAPTURE_WIDTH,
                    MAC_CAPTURE_HEIGHT,
                    &dest,
                )
            }));

            manifest.push(capture_outcome_entry(
                demo.name(),
                &group,
                variant,
                "macos",
                filename,
                outcome,
            ));
        }
    }
    Ok(())
}

/// PNG-encode a [`BitmapSurface`](quadraui::macos::headless::BitmapSurface)'s
/// raw pixels (see that type's module doc for the byte layout —
/// top-down scanlines, 8bpc RGBA, `kCGImageAlphaPremultipliedLast`).
/// [`composite_over_white`] first flattens that premultiplied-alpha
/// buffer onto an opaque white backdrop: `BitmapSurface` starts
/// transparent-black and only chrome the shell actually painted gets
/// any coverage, so encoding the raw bytes straight through would (a)
/// leave every unpainted margin transparent instead of showing the
/// window's own background, and (b) read any partially-covered edge
/// pixel's premultiplied RGB as a straight-alpha PNG would, which comes
/// out darker than what was actually on screen.
#[cfg(all(feature = "macos", target_os = "macos"))]
fn write_mac_png(
    surface: &quadraui::macos::headless::BitmapSurface,
    width: u32,
    height: u32,
    path: &Path,
) -> io::Result<()> {
    let flattened = composite_over_white(surface.bytes());
    let img = image::RgbaImage::from_raw(width, height, flattened)
        .expect("BitmapSurface byte length must be exactly width * height * 4");
    img.save(path)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
}

/// Composite premultiplied-alpha RGBA bytes (`BitmapSurface`'s own
/// layout — see [`write_mac_png`]) onto an opaque white backdrop,
/// returning straight (`alpha = 255`) RGBA bytes ready for
/// [`image::RgbaImage::from_raw`]. For a premultiplied pixel
/// `(r, g, b, a)` composited over opaque white, the "over" operator
/// reduces to `channel + (255 - a)` — and since a premultiplied
/// channel never exceeds its own alpha, that sum never exceeds 255, so
/// no clamping is needed.
#[cfg(all(feature = "macos", target_os = "macos"))]
fn composite_over_white(premultiplied_rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(premultiplied_rgba.len());
    for px in premultiplied_rgba.chunks_exact(4) {
        let (r, g, b, a) = (px[0], px[1], px[2], px[3]);
        let inv = 255 - a;
        out.push(r.saturating_add(inv));
        out.push(g.saturating_add(inv));
        out.push(b.saturating_add(inv));
        out.push(255);
    }
    out
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

    #[test]
    fn panic_message_extracts_str_and_string_payloads() {
        let str_payload: Box<dyn std::any::Any + Send> = Box::new("boom");
        assert_eq!(panic_message(&*str_payload), "boom");

        let string_payload: Box<dyn std::any::Any + Send> = Box::new(String::from("kaboom"));
        assert_eq!(panic_message(&*string_payload), "kaboom");

        let other_payload: Box<dyn std::any::Any + Send> = Box::new(42i32);
        assert_eq!(
            panic_message(&*other_payload),
            "capture panicked with a non-string payload"
        );
    }

    #[test]
    fn capture_outcome_entry_reports_error_status_on_io_failure_without_aborting() {
        let io_err = io::Error::other("disk full");
        let entry = capture_outcome_entry(
            "Demo",
            "Group",
            "default",
            "tui",
            "demo.svg".to_string(),
            Ok(Err(io_err)),
        );
        assert_eq!(entry.path, None);
        assert_eq!(entry.status.as_deref(), Some("error"));
        assert_eq!(entry.note.as_deref(), Some("disk full"));
    }

    #[test]
    fn capture_outcome_entry_reports_a_real_path_on_success() {
        let entry = capture_outcome_entry(
            "Demo",
            "Group",
            "default",
            "tui",
            "demo.svg".to_string(),
            Ok(Ok(())),
        );
        assert_eq!(entry.path.as_deref(), Some("demo.svg"));
        assert_eq!(entry.status, None);
        assert_eq!(entry.note, None);
    }

    #[cfg(all(feature = "macos", target_os = "macos"))]
    #[test]
    fn composite_over_white_fills_fully_transparent_pixels_with_opaque_white() {
        let transparent_black = [0u8, 0, 0, 0];
        let out = composite_over_white(&transparent_black);
        assert_eq!(out, vec![255, 255, 255, 255]);
    }

    #[cfg(all(feature = "macos", target_os = "macos"))]
    #[test]
    fn composite_over_white_passes_fully_opaque_pixels_through() {
        let opaque_red_premultiplied = [255u8, 0, 0, 255];
        let out = composite_over_white(&opaque_red_premultiplied);
        assert_eq!(out, vec![255, 0, 0, 255]);
    }

    /// A two-variant [`crate::Demo`] test double — the production
    /// [`registry`] never exercises `variants.len() > 1`, since its one
    /// entry (`ToastDemo`) has a single variant. Pins the variant-click
    /// branch in [`capture_tui_over`] (and the variant slug landing in
    /// the filename) against regressing silently.
    #[cfg(feature = "tui")]
    struct TwoVariantDemo;

    #[cfg(feature = "tui")]
    impl crate::Demo for TwoVariantDemo {
        fn name(&self) -> &'static str {
            "TwoVariant"
        }

        fn group(&self) -> &'static str {
            "Overlays"
        }

        fn variants(&self) -> &'static [&'static str] {
            &["Alpha", "Beta"]
        }

        fn render(
            &self,
            variant: usize,
            backend: &mut dyn quadraui::Backend,
            area: quadraui::Rect,
        ) {
            use quadraui::{Color, InteractionState, StatusBar, StatusBarSegment, WidgetId};
            let bar = StatusBar {
                id: WidgetId::new("test:two-variant"),
                left_segments: vec![StatusBarSegment {
                    text: format!(" variant={} ", self.variants()[variant]),
                    fg: Color::rgb(255, 255, 255),
                    bg: Color::rgb(0, 0, 0),
                    bold: false,
                    action_id: None,
                }],
                right_segments: vec![],
            };
            let _ = backend.draw_status_bar_interactive(area, &bar, &InteractionState::new());
        }

        fn handle(
            &mut self,
            _variant: usize,
            _event: &quadraui::UiEvent,
            _backend: &mut dyn quadraui::Backend,
            _area: quadraui::Rect,
        ) -> quadraui::Reaction {
            quadraui::Reaction::Continue
        }

        fn source(&self) -> &'static str {
            "struct TwoVariantDemo;"
        }

        fn data(&self, _variant: usize) -> serde_json::Value {
            serde_json::Value::Null
        }
    }

    #[cfg(feature = "tui")]
    fn two_variant_registry() -> Vec<Box<dyn crate::Demo>> {
        vec![Box::new(TwoVariantDemo)]
    }

    #[cfg(feature = "tui")]
    #[test]
    fn capture_tui_selects_the_clicked_variant_and_names_it_in_the_filename() {
        let dir = tempfile::tempdir().expect("create temp capture dir");
        let mut manifest = Vec::new();

        capture_tui_over(two_variant_registry, dir.path(), &mut manifest)
            .expect("capture_tui_over");

        assert_eq!(manifest.len(), 2, "one row per variant: {manifest:#?}");
        for variant in ["Alpha", "Beta"] {
            let entry = manifest
                .iter()
                .find(|e| e.variant == variant)
                .unwrap_or_else(|| {
                    panic!("missing manifest row for variant {variant}: {manifest:#?}")
                });
            assert!(
                entry.status.is_none(),
                "variant {variant} should have captured cleanly: {entry:#?}"
            );
            let path = entry
                .path
                .as_deref()
                .unwrap_or_else(|| panic!("variant {variant} should have a path: {entry:#?}"));
            assert!(
                path.contains(&slug(variant)),
                "filename {path} should name its own variant ({variant})"
            );

            let svg = fs::read_to_string(dir.path().join(path)).expect("read captured SVG");
            assert!(
                svg.contains(&format!("variant={variant}")),
                "SVG for {variant} should show that variant's own rendered content:\n{svg}"
            );
        }
    }
}
