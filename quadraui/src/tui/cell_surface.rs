//! TUI's [`CellSurface`] implementation — the cell-grid counterpart of
//! [`crate::PaintSurface`], backed by a live `ratatui::buffer::Buffer`.
//!
//! Not a widget: this is the TUI half of the backend-neutral paint seam
//! `crate::cell_surface` declares, reached through
//! [`crate::backend::Backend::cell_surface`] and consumed by
//! [`crate::primitive::Primitive::paint_cells`]. Every glyph it puts goes
//! through the same `super::set_cell_styled` / `super::set_cell_wide_styled`
//! writers the hand-written `draw_*` rasterisers in this directory use, so
//! a primitive painted through the seam and one painted by its own
//! rasteriser land identical bytes in the buffer.

use ratatui::buffer::Buffer;
use ratatui::style::{Color as RatatuiColor, Modifier};

use super::{char_cell_width, ratatui_color, set_cell_styled, set_cell_wide_styled};
use crate::backend::ColorDepth;
use crate::cell_surface::{BorderSet, CellStyle, CellSurface, CursorShape};
use crate::types::Color;

/// A box-drawing glyph family's six characters — the table
/// [`border_glyphs`] picks a row out of for [`TuiCellSurface::draw_border`].
struct BorderGlyphs {
    top_left: char,
    top_right: char,
    bottom_left: char,
    bottom_right: char,
    horizontal: char,
    vertical: char,
}

/// The one place every [`BorderSet`] variant's literal glyphs live.
/// `Rounded` matches the glyphs `crate::tui::dialog`/`crate::tui::board`/
/// `crate::tui::focus_ring` already paint by hand; `Square`/`Double`/
/// `Heavy` are new — [`TuiCellSurface::draw_border`] is their first
/// caller.
fn border_glyphs(set: BorderSet) -> BorderGlyphs {
    match set {
        BorderSet::Square => BorderGlyphs {
            top_left: '┌',
            top_right: '┐',
            bottom_left: '└',
            bottom_right: '┘',
            horizontal: '─',
            vertical: '│',
        },
        BorderSet::Rounded => BorderGlyphs {
            top_left: '╭',
            top_right: '╮',
            bottom_left: '╰',
            bottom_right: '╯',
            horizontal: '─',
            vertical: '│',
        },
        BorderSet::Double => BorderGlyphs {
            top_left: '╔',
            top_right: '╗',
            bottom_left: '╚',
            bottom_right: '╝',
            horizontal: '═',
            vertical: '║',
        },
        BorderSet::Heavy => BorderGlyphs {
            top_left: '┏',
            top_right: '┓',
            bottom_left: '┗',
            bottom_right: '┛',
            horizontal: '━',
            vertical: '┃',
        },
    }
}

/// `style`'s colour/modifier fields, resolved to the trio
/// [`set_cell_styled`]/[`set_cell_wide_styled`] take: `fg`/`bg` default to
/// [`RatatuiColor::Reset`] (the terminal's own default, SGR `39`/`49`) when
/// `None` — see [`CellStyle`]'s own doc for why `None` means that rather
/// than "unspecified".
fn resolve_cell_style(
    style: CellStyle,
) -> (RatatuiColor, RatatuiColor, Modifier, Option<RatatuiColor>) {
    let fg = style.fg.map(ratatui_color).unwrap_or(RatatuiColor::Reset);
    let bg = style.bg.map(ratatui_color).unwrap_or(RatatuiColor::Reset);
    let mut modifier = Modifier::empty();
    if style.bold {
        modifier |= Modifier::BOLD;
    }
    if style.italic {
        modifier |= Modifier::ITALIC;
    }
    if style.dim {
        modifier |= Modifier::DIM;
    }
    if style.reverse {
        modifier |= Modifier::REVERSED;
    }
    let underline_color = style.underline.map(ratatui_color);
    if underline_color.is_some() {
        modifier |= Modifier::UNDERLINED;
    }
    (fg, bg, modifier, underline_color)
}

/// Recover a [`Color`] from a ratatui [`RatatuiColor`] that one of the
/// `set_cell*` writers wrote (always [`RatatuiColor::Rgb`] — see
/// [`ratatui_color`]). Returns `None` for any other variant (`Reset`,
/// a named colour, `Indexed`), which carries no literal RGB to recover —
/// the same restriction `crate::tui::solid_fill`'s own `color_from_ratatui`
/// documents.
fn color_from_ratatui(c: RatatuiColor) -> Option<Color> {
    match c {
        RatatuiColor::Rgb(r, g, b) => Some(Color::rgb(r, g, b)),
        _ => None,
    }
}

/// The [`CellSurface`] [`crate::tui::backend::TuiBackend::cell_surface`]
/// builds fresh each call: a [`BufferCellSurface`] whose cursor writes go
/// to a sink the backend owns, with no lifetime tying it to the buffer.
/// See `crate::cell_surface`'s module doc for scope.
pub(crate) type TuiCellSurface = BufferCellSurface<'static>;

/// A [`CellSurface`] over a caller-owned buffer, for tests and headless
/// paint: wrap a `ratatui` [`Buffer`] with [`Self::new`] and hand it to
/// [`crate::Primitive::paint_cells`] with no backend in reach. The TUI
/// backend paints through this same type, so a primitive painted here
/// lands exactly the cells it does on screen.
///
/// Holds `buf` as a raw pointer rather than `&mut Buffer` so the backend's
/// [`TuiCellSurface`] carries no lifetime of its own: [`crate::tui::backend::TuiBackend`]
/// itself has none (matching every other `Backend` implementor), so a
/// field holding the `&mut dyn CellSurface`
/// [`crate::tui::backend::TuiBackend::cell_surface`] hands back could not
/// otherwise be stored on it between calls. Mirrors the type-erased
/// `*mut ()` [`crate::tui::backend::TuiBackend::current_frame_ptr`] already
/// stores for the identical reason — see that field's own doc, "Frame-scope
/// mechanism" — except `Buffer` itself has no lifetime to erase, so the raw
/// pointer here is simply `*mut Buffer`, no `()` type-erasure cast needed.
pub struct BufferCellSurface<'a> {
    /// Valid for `'a` when built by [`Self::new`]. Built by [`Self::over`],
    /// it carries no lifetime: valid only while the caller's buffer is —
    /// for the backend, while the
    /// [`crate::tui::backend::TuiBackend::enter_frame_scope`] call that
    /// produced it is still on the stack, the same invariant
    /// [`crate::tui::backend::TuiBackend::current_frame_mut`]'s own doc
    /// documents for the `Frame` pointer this is ultimately derived from.
    /// Every accessor reaches it only through [`Self::buf`].
    buf: *mut Buffer,
    color_depth: ColorDepth,
    /// Where [`Self::set_cursor`](CellSurface::set_cursor) forwards the
    /// position/shape it receives — `&mut TuiBackend::last_cell_surface_cursor`
    /// at construction time, taken by
    /// [`crate::tui::backend::TuiBackend::take_last_cell_surface_cursor`]
    /// once per frame, the same "buffer-only paint → frame-level side
    /// effect" handoff `TuiBackend::last_cursor_position` uses for
    /// `draw_editor`'s caret. Unlike `buf`, this points at a field on
    /// `TuiBackend` itself, not at frame-scoped storage, so it stays valid
    /// for `self`'s entire lifetime — rebuilding this surface on every
    /// [`crate::tui::backend::TuiBackend::cell_surface`] call can't drop a
    /// write in flight.
    cursor_sink: *mut Option<(u16, u16, CursorShape)>,
    /// Whether `cursor_sink` is this value's own allocation, freed on drop.
    owns_sink: bool,
    _buf: std::marker::PhantomData<&'a mut Buffer>,
}

impl<'a> BufferCellSurface<'a> {
    /// A surface over `buf` for as long as this value lives. Reports
    /// [`ColorDepth::TrueColor`]; read the last cursor placement back with
    /// [`Self::cursor`].
    pub fn new(buf: &'a mut Buffer) -> Self {
        let mut surface = Self::over(buf, ColorDepth::TrueColor, Box::into_raw(Box::new(None)));
        surface.owns_sink = true;
        surface
    }

    /// Where the painted content last placed the cursor, if anywhere.
    pub fn cursor(&self) -> Option<(u16, u16, CursorShape)> {
        // SAFETY: see the `cursor_sink` field's own doc.
        unsafe { *self.cursor_sink }
    }

    /// `cursor_sink` takes a raw pointer (not `&mut Option<…>`) so a
    /// caller can obtain it in a statement before borrowing `self` again
    /// for `buf` — matching this struct's own `buf: *mut Buffer` field,
    /// which exists for the identical reason (see this type's own doc). A
    /// plain `&mut` reference coerces to this parameter type at every call
    /// site, so constructing one is never `unsafe` on its own.
    pub(crate) fn over(
        buf: &mut Buffer,
        color_depth: ColorDepth,
        cursor_sink: *mut Option<(u16, u16, CursorShape)>,
    ) -> Self {
        Self {
            buf: buf as *mut Buffer,
            color_depth,
            cursor_sink,
            owns_sink: false,
            _buf: std::marker::PhantomData,
        }
    }

    /// Reborrow the live buffer. See [`Self::buf`]'s own field doc for the
    /// validity invariant this relies on — callers reach `TuiCellSurface`
    /// only through [`crate::backend::Backend::cell_surface`], which never
    /// answers `Some` outside that invariant's window.
    fn buf(&mut self) -> &mut Buffer {
        // SAFETY: see this type's own doc and the `buf` field's doc.
        unsafe { &mut *self.buf }
    }
}

impl Drop for BufferCellSurface<'_> {
    fn drop(&mut self) {
        if self.owns_sink {
            // SAFETY: an owned sink came from `Box::into_raw` in `new` and
            // is freed exactly once, here.
            drop(unsafe { Box::from_raw(self.cursor_sink) });
        }
    }
}

impl CellSurface for BufferCellSurface<'_> {
    fn size(&self) -> (u16, u16) {
        // SAFETY: see `Self::buf`'s own doc — read-only use, no reference
        // escapes this method.
        let area = unsafe { &*self.buf }.area;
        (area.width, area.height)
    }

    fn put_char(&mut self, x: u16, y: u16, ch: char, style: CellStyle) {
        let (fg, bg, modifier, underline_color) = resolve_cell_style(style);
        set_cell_styled(self.buf(), x, y, ch, fg, bg, modifier, underline_color);
    }

    fn put_wide(&mut self, x: u16, y: u16, ch: char, style: CellStyle) {
        let (fg, bg, modifier, underline_color) = resolve_cell_style(style);
        set_cell_wide_styled(self.buf(), x, y, ch, fg, bg, modifier, underline_color);
    }

    fn put_grapheme(&mut self, x: u16, y: u16, grapheme: &str, style: CellStyle) {
        let (fg, bg, modifier, underline_color) = resolve_cell_style(style);
        let buf = self.buf();
        let area = buf.area;
        if x < area.x || y < area.y || x >= area.x + area.width || y >= area.y + area.height {
            return;
        }
        let cell = &mut buf[(x, y)];
        // ratatui debug-asserts on ASCII control characters in a cell
        // symbol, so such a cluster paints as a blank.
        if grapheme.chars().any(|c| c.is_ascii_control()) {
            cell.set_char(' ');
        } else {
            cell.set_symbol(grapheme);
        }
        cell.set_fg(fg).set_bg(bg);
        cell.modifier = modifier;
        cell.underline_color = underline_color.unwrap_or(RatatuiColor::Reset);
    }

    fn put_str_clipped(
        &mut self,
        x: u16,
        y: u16,
        text: &str,
        max_cols: u16,
        style: CellStyle,
    ) -> u16 {
        let mut col = 0u16;
        for ch in text.chars() {
            let w = char_cell_width(ch);
            if col.saturating_add(w) > max_cols {
                break;
            }
            if w == 2 {
                self.put_wide(x + col, y, ch, style);
            } else {
                self.put_char(x + col, y, ch, style);
            }
            col += w;
        }
        col
    }

    fn fill(&mut self, rect: crate::Rect, style: CellStyle) {
        let area = super::backend::q_rect_to_ratatui(rect);
        let (x0, y0) = (area.x, area.y);
        for y in y0..y0.saturating_add(area.height) {
            for x in x0..x0.saturating_add(area.width) {
                self.put_char(x, y, ' ', style);
            }
        }
    }

    fn draw_border(&mut self, rect: crate::Rect, set: BorderSet, style: CellStyle) {
        let area = super::backend::q_rect_to_ratatui(rect);
        let (x0, y0, width, height) = (area.x, area.y, area.width, area.height);
        if width < 2 || height < 2 {
            return;
        }
        let x1 = x0 + width - 1;
        let y1 = y0 + height - 1;
        let glyphs = border_glyphs(set);
        self.put_char(x0, y0, glyphs.top_left, style);
        self.put_char(x1, y0, glyphs.top_right, style);
        self.put_char(x0, y1, glyphs.bottom_left, style);
        self.put_char(x1, y1, glyphs.bottom_right, style);
        for x in (x0 + 1)..x1 {
            self.put_char(x, y0, glyphs.horizontal, style);
            self.put_char(x, y1, glyphs.horizontal, style);
        }
        for y in (y0 + 1)..y1 {
            self.put_char(x0, y, glyphs.vertical, style);
            self.put_char(x1, y, glyphs.vertical, style);
        }
    }

    fn get(&self, x: u16, y: u16) -> Option<(char, CellStyle)> {
        // SAFETY: see `Self::buf`'s own doc — read-only use, no reference
        // escapes this method.
        let buf = unsafe { &*self.buf };
        let area = buf.area;
        if x < area.x || y < area.y || x >= area.x + area.width || y >= area.y + area.height {
            return None;
        }
        let cell = &buf[(x, y)];
        let ch = cell.symbol().chars().next().unwrap_or(' ');
        let style = CellStyle {
            fg: color_from_ratatui(cell.fg),
            bg: color_from_ratatui(cell.bg),
            bold: cell.modifier.contains(Modifier::BOLD),
            italic: cell.modifier.contains(Modifier::ITALIC),
            underline: if cell.modifier.contains(Modifier::UNDERLINED) {
                color_from_ratatui(cell.underline_color)
            } else {
                None
            },
            dim: cell.modifier.contains(Modifier::DIM),
            reverse: cell.modifier.contains(Modifier::REVERSED),
        };
        Some((ch, style))
    }

    fn set_cursor(&mut self, x: u16, y: u16, shape: CursorShape) {
        // SAFETY: see the `cursor_sink` field's own doc — it points at a
        // `TuiBackend` field that outlives this call, and nothing else
        // holds a reference into it while this surface is reachable.
        unsafe {
            *self.cursor_sink = Some((x, y, shape));
        }
    }

    fn color_depth(&self) -> ColorDepth {
        self.color_depth
    }

    #[cfg(feature = "tui")]
    fn ratatui_buffer(&mut self) -> Option<&mut Buffer> {
        Some(self.buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn tui_cell_surface_put_wide_blanks_continuation_cell() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut cursor_sink = None;
        let mut surface = TuiCellSurface::over(&mut buf, ColorDepth::TrueColor, &mut cursor_sink);
        surface.put_wide(2, 1, '中', CellStyle::default());
        assert_eq!(buf[(2, 1)].symbol(), "中");
        assert_eq!(
            buf[(3, 1)].symbol(),
            "",
            "the continuation cell must be blanked, not left stale"
        );
    }

    #[test]
    fn tui_cell_surface_put_str_clipped_never_splits_a_double_width_glyph() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut cursor_sink = None;
        let mut surface = TuiCellSurface::over(&mut buf, ColorDepth::TrueColor, &mut cursor_sink);
        // "日本語" is 3 double-width glyphs (6 columns). A 5-column budget
        // fits two glyphs (4 columns) but not a third's two columns.
        let written = surface.put_str_clipped(0, 0, "日本語", 5, CellStyle::default());
        assert_eq!(
            written, 4,
            "only the first two double-width glyphs fit a 5-column budget"
        );
        assert_eq!(buf[(0, 0)].symbol(), "日");
        assert_eq!(buf[(2, 0)].symbol(), "本");
        // Nothing painted past the written width.
        assert_eq!(buf[(4, 0)].symbol(), " ");
    }

    #[test]
    fn tui_cell_surface_draw_border_paints_every_glyph_set() {
        let cases = [
            (BorderSet::Square, '┌', '┐', '└', '┘', '─', '│'),
            (BorderSet::Rounded, '╭', '╮', '╰', '╯', '─', '│'),
            (BorderSet::Double, '╔', '╗', '╚', '╝', '═', '║'),
            (BorderSet::Heavy, '┏', '┓', '┗', '┛', '━', '┃'),
        ];
        for (set, top_left, top_right, bottom_left, bottom_right, horizontal, vertical) in cases {
            let mut buf = Buffer::empty(Rect::new(0, 0, 6, 4));
            let mut cursor_sink = None;
            let mut surface =
                TuiCellSurface::over(&mut buf, ColorDepth::TrueColor, &mut cursor_sink);
            surface.draw_border(
                crate::Rect::new(1.0, 1.0, 4.0, 3.0),
                set,
                CellStyle::default(),
            );
            assert_eq!(
                buf[(1, 1)].symbol(),
                top_left.to_string(),
                "{set:?} top-left"
            );
            assert_eq!(
                buf[(4, 1)].symbol(),
                top_right.to_string(),
                "{set:?} top-right"
            );
            assert_eq!(
                buf[(1, 3)].symbol(),
                bottom_left.to_string(),
                "{set:?} bottom-left"
            );
            assert_eq!(
                buf[(4, 3)].symbol(),
                bottom_right.to_string(),
                "{set:?} bottom-right"
            );
            assert_eq!(
                buf[(2, 1)].symbol(),
                horizontal.to_string(),
                "{set:?} top edge"
            );
            assert_eq!(
                buf[(1, 2)].symbol(),
                vertical.to_string(),
                "{set:?} left edge"
            );
        }
    }

    #[test]
    fn tui_cell_surface_draw_border_too_small_paints_nothing() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 4));
        let mut cursor_sink = None;
        let mut surface = TuiCellSurface::over(&mut buf, ColorDepth::TrueColor, &mut cursor_sink);
        surface.draw_border(
            crate::Rect::new(1.0, 1.0, 1.0, 1.0),
            BorderSet::Square,
            CellStyle::default(),
        );
        assert_eq!(
            buf[(1, 1)].symbol(),
            " ",
            "a 1x1 rect has no border to draw"
        );
    }

    #[test]
    fn tui_cell_surface_get_round_trips_put_char() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut cursor_sink = None;
        let mut surface = TuiCellSurface::over(&mut buf, ColorDepth::TrueColor, &mut cursor_sink);
        let style = CellStyle {
            fg: Some(Color::rgb(10, 20, 30)),
            bg: Some(Color::rgb(40, 50, 60)),
            bold: true,
            italic: false,
            underline: Some(Color::rgb(70, 80, 90)),
            dim: false,
            reverse: true,
        };
        surface.put_char(3, 1, 'z', style);
        let (ch, got) = surface.get(3, 1).expect("cell was just painted");
        assert_eq!(ch, 'z');
        assert_eq!(got, style);
    }

    #[test]
    fn tui_cell_surface_get_out_of_bounds_is_none() {
        let mut buf = Buffer::empty(Rect::new(5, 5, 10, 10));
        let mut cursor_sink = None;
        let surface = TuiCellSurface::over(&mut buf, ColorDepth::TrueColor, &mut cursor_sink);
        assert!(surface.get(0, 0).is_none());
    }

    #[test]
    fn tui_cell_surface_set_cursor_forwards_position_and_shape_to_the_sink() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut cursor_sink = None;
        let mut surface = TuiCellSurface::over(&mut buf, ColorDepth::TrueColor, &mut cursor_sink);
        assert_eq!(cursor_sink, None);
        surface.set_cursor(4, 2, CursorShape::Bar);
        assert_eq!(cursor_sink, Some((4, 2, CursorShape::Bar)));
    }

    #[test]
    fn tui_cell_surface_color_depth_reports_the_constructor_value() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut cursor_sink = None;
        let surface = TuiCellSurface::over(&mut buf, ColorDepth::Ansi16, &mut cursor_sink);
        assert_eq!(surface.color_depth(), ColorDepth::Ansi16);
    }

    #[test]
    fn buffer_cell_surface_paints_like_the_backend_surface_and_keeps_the_cursor() {
        let mut direct = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut sink = None;
        TuiCellSurface::over(&mut direct, ColorDepth::TrueColor, &mut sink).put_str_clipped(
            1,
            1,
            "日a",
            5,
            CellStyle::default(),
        );
        let mut wrapped = Buffer::empty(Rect::new(0, 0, 10, 3));
        let mut surface = BufferCellSurface::new(&mut wrapped);
        assert_eq!(surface.cursor(), None);
        surface.put_str_clipped(1, 1, "日a", 5, CellStyle::default());
        surface.set_cursor(3, 2, CursorShape::Block);
        assert_eq!(surface.cursor(), Some((3, 2, CursorShape::Block)));
        assert!(surface.ratatui_buffer().is_some());
        drop(surface);
        assert_eq!(wrapped, direct);
    }
}
