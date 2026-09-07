//! SGR colour-depth quantisation (quadraui#826).
//!
//! [`super::ratatui_color`] (and every rasteriser that calls it) always
//! produces `RatatuiColor::Rgb` — that conversion stays untouched here on
//! purpose, so a truecolor terminal's byte stream is unaffected by this
//! module's existence. What was missing was a choke point *downstream* of
//! every one of the ~150 `ratatui_color` call sites where the actual
//! terminal's colour fidelity (detected by [`super::caps::detect_color_depth`])
//! could be applied exactly once, rather than threading a `ColorDepth`
//! parameter through every rasteriser's signature.
//!
//! [`DepthLimitedBackend`] is that choke point: it wraps any ratatui
//! [`RtBackend`] and, on every [`RtBackend::draw`] call, quantises each
//! painted [`Cell`]'s `fg`/`bg`/`underline_color` to the wrapped
//! [`ColorDepth`] before delegating to the real backend. This runs after
//! ratatui's own frame-to-frame diffing has already decided which cells
//! changed, so it costs nothing extra on a truecolor terminal (the
//! [`ColorDepth::TrueColor`] arm is a plain pass-through — see
//! [`DepthLimitedBackend::draw`]) and correctly downgrades exactly the
//! cells that reach the wire on a 256- or 16-colour one.
//!
//! [`super::run::run`] is the one production call site: it wraps the real
//! `CrosstermBackend<Stdout>` in a `DepthLimitedBackend` seeded from
//! [`crate::tui::backend::TuiBackend::color_depth`]. `tests/tui_pty_smoke.rs`
//! observes the result over a real pty.

use ratatui::backend::{Backend as RtBackend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::Color as RatatuiColor;

use crate::backend::ColorDepth;

// ─── Quantisation ───────────────────────────────────────────────────────

/// Downgrade `color` to the nearest colour representable at `depth`.
///
/// Identity for [`ColorDepth::TrueColor`] (every variant passes through
/// unchanged) and for any already-coarse-enough variant at a lower depth
/// (e.g. a named 16-colour value is already representable in a 256-colour
/// palette, so [`ColorDepth::Indexed256`] leaves it alone). Only
/// `RatatuiColor::Rgb` — the one variant [`super::ratatui_color`] ever
/// produces — and, for completeness, `RatatuiColor::Indexed` actually get
/// quantised.
pub(crate) fn quantize(color: RatatuiColor, depth: ColorDepth) -> RatatuiColor {
    match (depth, color) {
        (ColorDepth::TrueColor, c) => c,
        (ColorDepth::Indexed256, RatatuiColor::Rgb(r, g, b)) => {
            RatatuiColor::Indexed(rgb_to_indexed256(r, g, b))
        }
        (ColorDepth::Indexed256, c) => c,
        (ColorDepth::Ansi16, RatatuiColor::Rgb(r, g, b)) => rgb_to_ansi16(r, g, b),
        (ColorDepth::Ansi16, RatatuiColor::Indexed(idx)) => {
            let (r, g, b) = indexed256_to_rgb(idx);
            rgb_to_ansi16(r, g, b)
        }
        (ColorDepth::Ansi16, c) => c,
    }
}

/// The 6 intensity steps xterm's 256-colour cube (indices 16-231) uses on
/// each of the R/G/B axes.
const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Squared Euclidean distance between two RGB triples — cheap and
/// monotonic with true distance, which is all nearest-colour picking
/// needs.
fn dist2(a: (u8, u8, u8), b: (u8, u8, u8)) -> i32 {
    let dr = a.0 as i32 - b.0 as i32;
    let dg = a.1 as i32 - b.1 as i32;
    let db = a.2 as i32 - b.2 as i32;
    dr * dr + dg * dg + db * db
}

/// Nearest xterm 256-colour palette index for `(r, g, b)`.
///
/// Searches the 6×6×6 colour cube (indices 16-231) and the 24-step
/// grayscale ramp (indices 232-255) and returns whichever is closer.
/// Deliberately does not also search the 16 basic colours (0-15): their
/// exact RGB values are terminal-theme-dependent (unlike the cube and
/// ramp, which xterm defines exactly), and the cube already places a
/// cube-corner color within one step of each primary/secondary hue, so
/// omitting them costs at most a shade of precision while keeping this
/// function's output independent of any one terminal's theme.
pub(crate) fn rgb_to_indexed256(r: u8, g: u8, b: u8) -> u8 {
    let level = |v: u8| -> usize {
        CUBE_LEVELS
            .iter()
            .enumerate()
            .min_by_key(|(_, &lv)| (v as i32 - lv as i32).abs())
            .map(|(i, _)| i)
            .expect("CUBE_LEVELS is non-empty")
    };
    let (rl, gl, bl) = (level(r), level(g), level(b));
    let cube_idx = 16 + 36 * rl + 6 * gl + bl;
    let cube_rgb = (CUBE_LEVELS[rl], CUBE_LEVELS[gl], CUBE_LEVELS[bl]);

    // 24-step grayscale ramp: index 232+i has value 8 + 10*i, i in 0..24.
    let avg = (r as i32 + g as i32 + b as i32) / 3;
    let gray_i = (((avg - 8).max(0)) / 10).clamp(0, 23) as usize;
    let gray_val = (8 + 10 * gray_i as i32) as u8;
    let gray_idx = 232 + gray_i;

    let target = (r, g, b);
    if dist2(target, cube_rgb) <= dist2(target, (gray_val, gray_val, gray_val)) {
        cube_idx as u8
    } else {
        gray_idx as u8
    }
}

/// `(r, g, b)` for xterm 256-colour palette index `idx` — the inverse of
/// [`rgb_to_indexed256`]'s cube/ramp math, plus the basic-16 table for
/// indices 0-15 (only reachable from [`quantize`]'s `Indexed` arm, which
/// only fires for an `Indexed` value this crate never itself produces —
/// present for completeness against any future caller).
fn indexed256_to_rgb(idx: u8) -> (u8, u8, u8) {
    match idx {
        0..=15 => ANSI16_PALETTE[idx as usize].1,
        16..=231 => {
            let i = idx - 16;
            let r = CUBE_LEVELS[(i / 36) as usize];
            let g = CUBE_LEVELS[((i / 6) % 6) as usize];
            let b = CUBE_LEVELS[(i % 6) as usize];
            (r, g, b)
        }
        232..=255 => {
            let v = 8 + 10 * (idx - 232) as u16;
            (v as u8, v as u8, v as u8)
        }
    }
}

/// The standard xterm default 16-colour palette, paired with the
/// [`RatatuiColor`] SGR variant each maps to. Order matches SGR 30-37
/// then 90-97 (i.e. `RatatuiColor::Black..=Gray` then
/// `DarkGray..=White`), which is also array index 0-15.
const ANSI16_PALETTE: [(RatatuiColor, (u8, u8, u8)); 16] = [
    (RatatuiColor::Black, (0, 0, 0)),
    (RatatuiColor::Red, (205, 0, 0)),
    (RatatuiColor::Green, (0, 205, 0)),
    (RatatuiColor::Yellow, (205, 205, 0)),
    (RatatuiColor::Blue, (0, 0, 238)),
    (RatatuiColor::Magenta, (205, 0, 205)),
    (RatatuiColor::Cyan, (0, 205, 205)),
    (RatatuiColor::Gray, (229, 229, 229)),
    (RatatuiColor::DarkGray, (127, 127, 127)),
    (RatatuiColor::LightRed, (255, 0, 0)),
    (RatatuiColor::LightGreen, (0, 255, 0)),
    (RatatuiColor::LightYellow, (255, 255, 0)),
    (RatatuiColor::LightBlue, (92, 92, 255)),
    (RatatuiColor::LightMagenta, (255, 0, 255)),
    (RatatuiColor::LightCyan, (0, 255, 255)),
    (RatatuiColor::White, (255, 255, 255)),
];

/// Nearest of the 16 basic ANSI colours for `(r, g, b)`.
///
/// This narrows the *palette* to the 16 basic ANSI colours, but does not
/// change the *escape-sequence family*: crossterm 0.29's `Colored` `Display`
/// impl formats every named [`RatatuiColor`] variant — including
/// `Black..=White` — via the extended 8-bit form (`38;5;n` / `48;5;n`), the
/// same family [`rgb_to_indexed256`] produces; there is no crossterm API
/// path that emits classic `30-37`/`90-97` SGR codes. So this fallback
/// helps a terminal that supports extended-256 syntax but only renders/
/// themes 16 colours; it does not by itself help a terminal that only
/// understands classic 3/4-bit SGR (a real vt100, a bare serial console).
pub(crate) fn rgb_to_ansi16(r: u8, g: u8, b: u8) -> RatatuiColor {
    ANSI16_PALETTE
        .iter()
        .min_by_key(|(_, rgb)| dist2((r, g, b), *rgb))
        .map(|(c, _)| *c)
        .expect("ANSI16_PALETTE is non-empty")
}

// ─── The wrapper backend ────────────────────────────────────────────────

/// Wraps a ratatui [`RtBackend`], quantising every painted cell's colours
/// to `depth` before delegating. See the module doc for why this — not a
/// `ColorDepth` parameter threaded through every rasteriser — is the
/// chosen choke point.
pub struct DepthLimitedBackend<B> {
    inner: B,
    depth: ColorDepth,
}

impl<B> DepthLimitedBackend<B> {
    /// Wrap `inner`, quantising to `depth`.
    pub fn new(inner: B, depth: ColorDepth) -> Self {
        Self { inner, depth }
    }

    /// The depth this wrapper currently quantises to.
    pub fn depth(&self) -> ColorDepth {
        self.depth
    }

    /// Change the depth future `draw` calls quantise to.
    pub fn set_depth(&mut self, depth: ColorDepth) {
        self.depth = depth;
    }

    /// Unwrap back to the underlying backend.
    pub fn into_inner(self) -> B {
        self.inner
    }
}

impl<B: RtBackend> RtBackend for DepthLimitedBackend<B> {
    type Error = B::Error;

    /// Quantise every cell's `fg`/`bg`/`underline_color`, then delegate.
    ///
    /// [`ColorDepth::TrueColor`] takes a fast path straight to `inner`
    /// with no allocation or per-cell work at all — this is what keeps a
    /// truecolor terminal's output byte-identical to before this wrapper
    /// existed (quadraui#826's "cannot regress the common case"
    /// acceptance item).
    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        if self.depth == ColorDepth::TrueColor {
            return self.inner.draw(content);
        }
        let quantized: Vec<(u16, u16, Cell)> = content
            .map(|(x, y, cell)| {
                let mut cell = cell.clone();
                cell.fg = quantize(cell.fg, self.depth);
                cell.bg = quantize(cell.bg, self.depth);
                cell.underline_color = quantize(cell.underline_color, self.depth);
                (x, y, cell)
            })
            .collect();
        self.inner
            .draw(quantized.iter().map(|(x, y, cell)| (*x, *y, cell)))
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}

/// Forwards straight to the wrapped backend so `crossterm`'s `execute!`/
/// `queue!` macros (used by [`super::run`] for raw-mode/alt-screen/mouse
/// setup and teardown, and the keyboard-enhancement push/pop) work on
/// `&mut DepthLimitedBackend<CrosstermBackend<W>>` exactly as they did on
/// the bare `CrosstermBackend<W>` before this wrapper existed.
impl<B: std::io::Write> std::io::Write for DepthLimitedBackend<B> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── rgb_to_indexed256 ──────────────────────────────────────────────

    #[test]
    fn pure_red_maps_to_a_cube_index() {
        // Pure red (255,0,0) is a cube corner: level 5 on R, level 0 on
        // G/B -> 16 + 36*5 + 6*0 + 0 = 196.
        assert_eq!(rgb_to_indexed256(255, 0, 0), 196);
    }

    #[test]
    fn pure_black_maps_to_cube_origin() {
        assert_eq!(rgb_to_indexed256(0, 0, 0), 16);
    }

    #[test]
    fn pure_white_maps_to_cube_corner() {
        assert_eq!(rgb_to_indexed256(255, 255, 255), 231);
    }

    #[test]
    fn mid_gray_prefers_the_grayscale_ramp_over_the_cube() {
        // (128,128,128) is much closer to a ramp step than to any cube
        // corner (cube levels are 0/95/135/175/215/255 per axis).
        let idx = rgb_to_indexed256(128, 128, 128);
        assert!(
            (232..=255).contains(&idx),
            "expected a ramp index, got {idx}"
        );
    }

    #[test]
    fn indexed256_round_trips_within_one_cube_step() {
        // rgb_to_indexed256(indexed256_to_rgb(i)) should return i itself
        // for every cube/ramp index — the palette's own colours are their
        // own nearest neighbour.
        for idx in 16u16..=255 {
            let (r, g, b) = indexed256_to_rgb(idx as u8);
            assert_eq!(
                rgb_to_indexed256(r, g, b),
                idx as u8,
                "index {idx} ({r},{g},{b}) did not round-trip"
            );
        }
    }

    // ─── rgb_to_ansi16 ──────────────────────────────────────────────────

    #[test]
    fn pure_red_maps_to_light_red() {
        // (255,0,0) is closer to LightRed's (255,0,0) than to Red's
        // (205,0,0).
        assert_eq!(rgb_to_ansi16(255, 0, 0), RatatuiColor::LightRed);
    }

    #[test]
    fn dim_red_maps_to_basic_red() {
        assert_eq!(rgb_to_ansi16(205, 0, 0), RatatuiColor::Red);
    }

    #[test]
    fn pure_black_maps_to_black() {
        assert_eq!(rgb_to_ansi16(0, 0, 0), RatatuiColor::Black);
    }

    #[test]
    fn pure_white_maps_to_white() {
        assert_eq!(rgb_to_ansi16(255, 255, 255), RatatuiColor::White);
    }

    // ─── quantize ───────────────────────────────────────────────────────

    #[test]
    fn truecolor_is_identity_for_every_variant() {
        for c in [
            RatatuiColor::Rgb(12, 34, 56),
            RatatuiColor::Indexed(200),
            RatatuiColor::Red,
            RatatuiColor::Reset,
        ] {
            assert_eq!(quantize(c, ColorDepth::TrueColor), c);
        }
    }

    #[test]
    fn indexed256_leaves_named_colours_alone() {
        assert_eq!(
            quantize(RatatuiColor::LightBlue, ColorDepth::Indexed256),
            RatatuiColor::LightBlue
        );
    }

    #[test]
    fn indexed256_downgrades_rgb() {
        assert_eq!(
            quantize(RatatuiColor::Rgb(255, 0, 0), ColorDepth::Indexed256),
            RatatuiColor::Indexed(196)
        );
    }

    #[test]
    fn ansi16_downgrades_rgb() {
        assert_eq!(
            quantize(RatatuiColor::Rgb(255, 0, 0), ColorDepth::Ansi16),
            RatatuiColor::LightRed
        );
    }

    #[test]
    fn ansi16_downgrades_indexed() {
        // Index 196 is the pure-red cube corner -> nearest 16-colour is
        // LightRed, same as quantising the raw RGB directly.
        assert_eq!(
            quantize(RatatuiColor::Indexed(196), ColorDepth::Ansi16),
            RatatuiColor::LightRed
        );
    }

    // ─── DepthLimitedBackend ────────────────────────────────────────────

    #[test]
    fn truecolor_draw_is_byte_identical_to_the_wrapped_backend() {
        use ratatui::backend::TestBackend;

        let mut direct = TestBackend::new(10, 1);
        let mut wrapped = DepthLimitedBackend::new(TestBackend::new(10, 1), ColorDepth::TrueColor);

        let mut cell = Cell::default();
        cell.set_char('x');
        cell.fg = RatatuiColor::Rgb(10, 20, 30);
        cell.bg = RatatuiColor::Rgb(40, 50, 60);
        let content = [(0u16, 0u16, &cell)];

        direct.draw(content.iter().copied()).unwrap();
        wrapped.draw(content.iter().copied()).unwrap();

        assert_eq!(direct.buffer(), wrapped.into_inner().buffer());
    }

    #[test]
    fn indexed256_draw_quantises_the_painted_cell() {
        use ratatui::backend::TestBackend;

        let mut wrapped = DepthLimitedBackend::new(TestBackend::new(10, 1), ColorDepth::Indexed256);

        let mut cell = Cell::default();
        cell.set_char('x');
        cell.fg = RatatuiColor::Rgb(255, 0, 0);
        let content = [(0u16, 0u16, &cell)];
        wrapped.draw(content.iter().copied()).unwrap();

        let inner = wrapped.into_inner();
        assert_eq!(inner.buffer()[(0, 0)].fg, RatatuiColor::Indexed(196));
    }

    /// `Theme::default().background` is `Color::rgb(20, 22, 30)` —
    /// `tests/tui_pty_smoke.rs`'s SGR-family fixtures assert on the exact
    /// indices this produces (`233` / `Black`), so this pins the two
    /// numbers here where a future palette-algorithm tweak would notice
    /// the drift before that pty test does (a much slower feedback loop).
    /// `Theme::default().{foreground, surface_bg}` are the `(fg, bg)` pair
    /// `draw_pipeline_view` paints stage names with — `tests/tui_pty_smoke.rs`
    /// asserts on the exact combined SGR sequence these two produce at
    /// each depth (`38;5;253;48;5;234` / `38;5;7;48;5;0` /
    /// `38;2;220;220;220;48;2;28;32;44`), so this pins the four numbers
    /// here where a future palette-algorithm tweak would notice the drift
    /// before that much-slower pty test does.
    #[test]
    fn theme_default_pipeline_colors_quantise_to_the_values_the_pty_fixture_expects() {
        assert_eq!(rgb_to_indexed256(220, 220, 220), 253); // foreground
        assert_eq!(rgb_to_ansi16(220, 220, 220), RatatuiColor::Gray);
        assert_eq!(rgb_to_indexed256(28, 32, 44), 234); // surface_bg
        assert_eq!(rgb_to_ansi16(28, 32, 44), RatatuiColor::Black);
    }
}
