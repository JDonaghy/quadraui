//! Screenshots of the hero counter on every backend this build enables, for
//! the README image strip and the site (`scripts/capture_hero.sh` drives it).
//!
//! Each backend's headless test driver paints the identical `Counter` from
//! `src/main.rs` off screen; nothing opens a window. Output, one file per
//! backend, into the directory given as the only argument:
//!
//! - `hero-tui.svg` — the terminal cell grid as SVG (`scripts/tui_svg2png.py`
//!   rasterises it with Pango so icon glyphs fall back the way a terminal's do);
//! - `hero-gtk.png`, `hero-macos.png`, `hero-win.png` — the native surfaces'
//!   pixels, composited over white.

#[allow(dead_code)]
#[path = "../main.rs"]
mod hero;

use std::path::{Path, PathBuf};

/// The count every capture shows, set directly so the image does not depend
/// on click timing.
const SHOWN_COUNT: i32 = 3;

fn counter() -> quadraui::app::Run<hero::Counter> {
    quadraui::app::Run::new(hero::Counter { count: SHOWN_COUNT })
}

fn main() {
    let out = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: hero-capture <out-dir>"),
    );
    std::fs::create_dir_all(&out).expect("create out dir");
    let mut wrote = Vec::new();
    #[cfg(feature = "tui")]
    wrote.push(tui::capture(&out));
    #[cfg(feature = "gtk")]
    wrote.push(gtk::capture(&out));
    #[cfg(all(feature = "macos", target_os = "macos"))]
    wrote.push(macos::capture(&out));
    #[cfg(all(feature = "win", target_os = "windows"))]
    wrote.push(win::capture(&out));
    for path in wrote {
        println!("{}", path.display());
    }
}

/// Native capture size in pixels: room for the label and both buttons at
/// the backends' default UI font, with a margin.
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
const NATIVE_W: u32 = 220;
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
const NATIVE_H: u32 = 80;

/// Premultiplied pixels (`[b, g, r, a]` when `bgra`, else `[r, g, b, a]`),
/// rows `stride` bytes apart, flattened onto white and saved as PNG.
#[cfg(any(
    feature = "gtk",
    all(feature = "macos", target_os = "macos"),
    all(feature = "win", target_os = "windows")
))]
fn save_png(data: &[u8], width: u32, height: u32, stride: usize, bgra: bool, path: &Path) {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height as usize {
        for px in data[y * stride..y * stride + width as usize * 4].chunks_exact(4) {
            let (r, g, b) = if bgra {
                (px[2], px[1], px[0])
            } else {
                (px[0], px[1], px[2])
            };
            let inv = 255 - px[3];
            rgba.extend_from_slice(&[
                r.saturating_add(inv),
                g.saturating_add(inv),
                b.saturating_add(inv),
                255,
            ]);
        }
    }
    image::RgbaImage::from_raw(width, height, rgba)
        .expect("buffer is width * height * 4")
        .save(path)
        .expect("write png");
}

#[cfg(feature = "tui")]
mod tui {
    use super::*;
    use quadraui::tui::testing::{Color, Modifier, TuiDriver};

    const COLS: u16 = 24;
    const ROWS: u16 = 4;
    const CELL_W: u32 = 9;
    const CELL_H: u32 = 18;

    pub fn capture(out: &Path) -> PathBuf {
        let mut driver = TuiDriver::new(counter(), COLS, ROWS);
        driver.render();
        let (w, h) = (u32::from(COLS) * CELL_W, u32::from(ROWS) * CELL_H);
        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" \
             viewBox=\"0 0 {w} {h}\" font-family=\"'SF Mono','DejaVu Sans Mono',monospace\" \
             font-size=\"{}\" xml:space=\"preserve\">\n\
             <rect width=\"100%\" height=\"100%\" fill=\"#101010\"/>\n",
            CELL_H - 4
        );
        for y in 0..ROWS {
            let row = driver.styled_row(y);
            let top = u32::from(y) * CELL_H;
            let mut x = 0;
            while x < row.len() {
                let bg = row[x].1.bg;
                let end = run_end(&row, x, |a, b| a.1.bg == b.1.bg);
                if let Some((r, g, b)) = rgb(bg) {
                    svg += &format!(
                        "<rect x=\"{}\" y=\"{top}\" width=\"{}\" height=\"{CELL_H}\" \
                         fill=\"#{r:02x}{g:02x}{b:02x}\"/>\n",
                        x as u32 * CELL_W,
                        (end - x) as u32 * CELL_W
                    );
                }
                x = end;
            }
            let mut x = 0;
            while x < row.len() {
                let style = row[x].1;
                let end = run_end(&row, x, |a, b| {
                    a.1.fg == b.1.fg && a.1.modifiers == b.1.modifiers
                });
                let text: String = row[x..end].iter().map(|(c, _)| *c).collect();
                if !text.trim().is_empty() {
                    let (r, g, b) = rgb(style.fg).unwrap_or((230, 230, 230));
                    let bold = if style.modifiers.contains(Modifier::BOLD) {
                        " font-weight=\"bold\""
                    } else {
                        ""
                    };
                    svg += &format!(
                        "<text x=\"{}\" y=\"{}\" textLength=\"{}\" lengthAdjust=\"spacingAndGlyphs\" \
                         fill=\"#{r:02x}{g:02x}{b:02x}\"{bold}>{}</text>\n",
                        x as u32 * CELL_W,
                        top + CELL_H - 4,
                        (end - x) as u32 * CELL_W,
                        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;"),
                    );
                }
                x = end;
            }
        }
        svg += "</svg>\n";
        let path = out.join("hero-tui.svg");
        std::fs::write(&path, svg).expect("write svg");
        path
    }

    fn run_end<T>(row: &[T], start: usize, same: impl Fn(&T, &T) -> bool) -> usize {
        let mut end = start + 1;
        while end < row.len() && same(&row[start], &row[end]) {
            end += 1;
        }
        end
    }

    /// Common terminal defaults for the 16 named colours, plus the xterm
    /// 256-colour cube and grey ramp. `None` is the terminal's own default.
    fn rgb(color: Color) -> Option<(u8, u8, u8)> {
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
        use Color::*;
        let i = match color {
            Reset => return None,
            Rgb(r, g, b) => return Some((r, g, b)),
            Black => 0,
            Red => 1,
            Green => 2,
            Yellow => 3,
            Blue => 4,
            Magenta => 5,
            Cyan => 6,
            Gray => 7,
            DarkGray => 8,
            LightRed => 9,
            LightGreen => 10,
            LightYellow => 11,
            LightBlue => 12,
            LightMagenta => 13,
            LightCyan => 14,
            White => 15,
            Indexed(i) => i,
        };
        Some(match i {
            0..=15 => BASIC16[i as usize],
            16..=231 => {
                let i = i - 16;
                let s = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
                (s(i / 36), s((i % 36) / 6), s(i % 6))
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                (v, v, v)
            }
        })
    }
}

#[cfg(feature = "gtk")]
mod gtk {
    use super::*;
    use quadraui::gtk::testing::GtkDriver;

    pub fn capture(out: &Path) -> PathBuf {
        let mut driver = GtkDriver::new(counter(), NATIVE_W as i32, NATIVE_H as i32);
        driver.render();
        let stride = driver.stride() as usize;
        let path = out.join("hero-gtk.png");
        save_png(&driver.screen(), NATIVE_W, NATIVE_H, stride, true, &path);
        path
    }
}

#[cfg(all(feature = "macos", target_os = "macos"))]
mod macos {
    use super::*;
    use quadraui::macos::testing::MacDriver;

    pub fn capture(out: &Path) -> PathBuf {
        let mut driver = MacDriver::new(counter(), NATIVE_W, NATIVE_H);
        driver.render();
        let path = out.join("hero-macos.png");
        let (px, stride) = (driver.surface().bytes(), NATIVE_W as usize * 4);
        save_png(px, NATIVE_W, NATIVE_H, stride, false, &path);
        path
    }
}

#[cfg(all(feature = "win", target_os = "windows"))]
mod win {
    use super::*;
    use quadraui::win::testing::WinDriver;

    pub fn capture(out: &Path) -> PathBuf {
        let mut driver = WinDriver::new(counter(), NATIVE_W, NATIVE_H);
        driver.render();
        let stride = driver.stride() as usize;
        // The Direct2D DIB leaves alpha at 0 where nothing blended; every
        // pixel the window shows is opaque.
        let mut data = driver.screen();
        for px in data.chunks_exact_mut(4) {
            px[3] = 255;
        }
        let path = out.join("hero-win.png");
        save_png(&data, NATIVE_W, NATIVE_H, stride, true, &path);
        path
    }
}
