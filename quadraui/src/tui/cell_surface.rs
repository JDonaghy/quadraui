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
//!
//! # Graphics channel
//!
//! [`CellSurface::emit_graphics`] is the surface's image channel (behind
//! the `tui-images` Cargo feature, on a truecolor terminal). On a terminal
//! with confirmed Kitty graphics support, [`draw_image_kitty`] paints real
//! pixels: decode + downscale through [`crate::tui::image_decode`], fill
//! the placed cells with [`crate::tui::kitty_graphics`]'s Unicode
//! placeholder content, and queue that image's transmit/placement escape
//! bytes on [`KittyImageState::pending_writes`] for
//! [`crate::tui::run::flush_kitty_graphics_writes`] to write to the real
//! terminal once `terminal.draw` returns — raw APC bytes can't go through
//! `ratatui::buffer::Buffer`'s cell grid the way the placeholder glyphs
//! themselves do. On any other truecolor terminal,
//! [`draw_image_halfblock`] paints a lower-fidelity but recognisable
//! thumbnail: each cell prints `▀` with the foreground colour sampling the
//! image's top pixel and the background colour sampling its bottom pixel,
//! from the image downscaled to `(cols × 2·rows)`. There is no further
//! ASCII-art tier under the half-block mosaic; past it the channel reports
//! that it painted nothing and the widget paints its fallback text.

use ratatui::buffer::Buffer;
use ratatui::style::{Color as RatatuiColor, Modifier};

#[cfg(feature = "tui-images")]
use ratatui::layout::Rect as RRect;

#[cfg(feature = "tui-images")]
use super::qc;
use super::{char_cell_width, ratatui_color, set_cell_styled, set_cell_wide_styled};
use crate::backend::ColorDepth;
use crate::cell_surface::{BorderSet, CellStyle, CellSurface, CursorShape};
#[cfg(feature = "tui-images")]
use crate::primitives::image::Image;
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
    /// Where [`Self::emit_graphics`](CellSurface::emit_graphics) paints
    /// images; `None` reports every image unpainted.
    #[cfg(feature = "tui-images")]
    graphics: Option<GraphicsChannel>,
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
            #[cfg(feature = "tui-images")]
            graphics: None,
            _buf: std::marker::PhantomData,
        }
    }

    /// This surface with `graphics` as its image channel.
    #[cfg(feature = "tui-images")]
    pub(crate) fn with_graphics(self, graphics: GraphicsChannel) -> Self {
        Self {
            graphics: Some(graphics),
            ..self
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

    /// Kitty graphics when the channel has a measured cell pixel size,
    /// else the half-block mosaic — both truecolor only.
    #[cfg(feature = "tui-images")]
    fn emit_graphics(&mut self, rect: crate::Rect, image: &Image) -> bool {
        let Some(graphics) = self.graphics else {
            return false;
        };
        if self.color_depth != ColorDepth::TrueColor {
            return false;
        }
        let area = super::backend::q_rect_to_ratatui(rect);
        // SAFETY: see `GraphicsChannel::state`'s own doc.
        let state = unsafe { &mut *graphics.state };
        match graphics.kitty_cell_px {
            Some(cell_px) => draw_image_kitty(self.buf(), area, image, cell_px, &mut state.kitty),
            None => draw_image_halfblock(
                self.buf(),
                area,
                image,
                graphics.background,
                &mut state.halfblock,
            ),
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

/// Every image-graphics cache a backend keeps across frames: the Kitty
/// tier's and the half-block tier's. One instance per backend, reached by
/// each frame's surface through a [`GraphicsChannel`].
#[cfg(feature = "tui-images")]
pub(crate) struct ImageGraphicsState {
    pub(crate) kitty: KittyImageState,
    pub(crate) halfblock: HalfBlockImageState,
}

#[cfg(feature = "tui-images")]
impl ImageGraphicsState {
    pub(crate) fn new() -> Self {
        Self {
            kitty: KittyImageState::new(),
            halfblock: HalfBlockImageState::new(),
        }
    }
}

/// What a surface's [`CellSurface::emit_graphics`] paints with.
#[cfg(feature = "tui-images")]
#[derive(Clone, Copy)]
pub(crate) struct GraphicsChannel {
    /// The backend's [`ImageGraphicsState`]. A raw pointer for the same
    /// reason `BufferCellSurface::cursor_sink` is one: it points at a
    /// field on the backend itself, valid for the backend's lifetime, and
    /// nothing else borrows it while a surface is reachable.
    pub(crate) state: *mut ImageGraphicsState,
    /// The terminal cell's pixel size when Kitty graphics are confirmed,
    /// else `None` (the half-block tier).
    pub(crate) kitty_cell_px: Option<(u16, u16)>,
    /// The colour a translucent pixel is blended onto for the half-block
    /// tier, whose cells carry no alpha of their own.
    pub(crate) background: Color,
}

/// Per-session state [`draw_image_kitty`] threads across frames —
/// one instance per backend (inside [`ImageGraphicsState`]), never
/// per-call: the decode cache and id registry only pay off when the same
/// backend paints the same image again on a later frame.
#[cfg(feature = "tui-images")]
pub(crate) struct KittyImageState {
    /// Decoded-and-downscaled pixels, keyed by source + target cell size
    /// — see [`crate::tui::image_decode`]'s module doc, "Decode cache"
    /// section.
    pub(crate) cache: crate::image_cache::ImageCache<crate::tui::image_decode::DecodedImage>,
    /// Which kitty image ids are currently placed, and for how many
    /// frames each has gone untouched — see
    /// [`crate::tui::kitty_graphics::IdRegistry`].
    pub(crate) registry: crate::tui::kitty_graphics::IdRegistry,
    /// Transmit/placement/delete escape bytes queued this frame,
    /// drained by [`crate::tui::run::flush_kitty_graphics_writes`] once
    /// `terminal.draw` returns.
    pub(crate) pending_writes: Vec<u8>,
}

#[cfg(feature = "tui-images")]
impl KittyImageState {
    pub(crate) fn new() -> Self {
        Self {
            cache: crate::image_cache::ImageCache::default(),
            registry: crate::tui::kitty_graphics::IdRegistry::new(),
            pending_writes: Vec::new(),
        }
    }
}

/// Resolved cell-grid placement for an image paint: top-left cell
/// `(x0, y0)` plus `(cols, rows)`, already clipped to both `area` and
/// `buf`'s own extent. Shared geometry behind [`draw_image_kitty`] and
/// [`draw_image_halfblock`] — resolves [`Image::layout`] against `area`
/// scaled into pixel space by `cell_px` (terminal cells aren't square,
/// and `Image::intrinsic_size` is in pixels, so the fit math has to run
/// in the same units), then scales the result back down into cell units.
/// `None` when there is nothing to place: a zero-size `area`/`cell_px`,
/// or a zero-size clipped placement.
#[cfg(feature = "tui-images")]
fn resolve_cell_placement(
    buf: &Buffer,
    area: RRect,
    image: &Image,
    cell_px: (u16, u16),
) -> Option<(u16, u16, u32, u32)> {
    if area.width == 0 || area.height == 0 || cell_px.0 == 0 || cell_px.1 == 0 {
        return None;
    }
    // Clip against `buf`'s own extent too, same defensive posture as
    // `TuiBackend::register_opaque_rect` — `area` is normally already
    // inside `buf.area`, but a stale caller-supplied rect must never
    // index past the buffer's real size.
    let buf_area = buf.area;
    let clip_x0 = (area.x as i32).max(buf_area.x as i32);
    let clip_y0 = (area.y as i32).max(buf_area.y as i32);
    let clip_x1 =
        (area.x as i32 + area.width as i32).min(buf_area.x as i32 + buf_area.width as i32);
    let clip_y1 =
        (area.y as i32 + area.height as i32).min(buf_area.y as i32 + buf_area.height as i32);

    // Resolve `Image::layout` in pixel space, not cell space:
    // `Image::intrinsic_size` (and therefore the aspect-ratio math
    // behind `ImageFit::Contain`/`Cover`/`None`) is in pixels, and
    // terminal cells are not square — scaling `area` up by `cell_px`
    // keeps the fit math correct, then the result is scaled back down
    // into cell units before any cell-grid indexing happens.
    let bounds_px = crate::event::Rect::new(
        area.x as f32 * cell_px.0 as f32,
        area.y as f32 * cell_px.1 as f32,
        area.width as f32 * cell_px.0 as f32,
        area.height as f32 * cell_px.1 as f32,
    );
    let placed_px = image.layout(bounds_px).bounds;
    let placed = crate::event::Rect::new(
        placed_px.x / cell_px.0 as f32,
        placed_px.y / cell_px.1 as f32,
        placed_px.width / cell_px.0 as f32,
        placed_px.height / cell_px.1 as f32,
    );
    let x0 = (placed.x.round() as i32).max(clip_x0);
    let y0 = (placed.y.round() as i32).max(clip_y0);
    let x1 = ((placed.x + placed.width).round() as i32).min(clip_x1);
    let y1 = ((placed.y + placed.height).round() as i32).min(clip_y1);
    let cols = (x1 - x0).max(0) as u32;
    let rows = (y1 - y0).max(0) as u32;
    if cols == 0 || rows == 0 {
        return None;
    }
    Some((x0 as u16, y0 as u16, cols, rows))
}

/// Paint `image` for real through the Kitty graphics protocol: resolve
/// the placement via [`resolve_cell_placement`], decode and downscale to
/// that placement's pixel size (`cols * cell_px.0` × `rows * cell_px.1`),
/// fill the placed cells with Unicode placeholder content colored to
/// carry this image's id, and queue transmit + placement bytes on
/// `state.pending_writes` for an id not already live this session.
/// Returns `false`, touching no cell, when there is nothing to place or
/// the decode fails — the caller paints its own fallback.
#[cfg(feature = "tui-images")]
pub(crate) fn draw_image_kitty(
    buf: &mut Buffer,
    area: RRect,
    image: &Image,
    cell_px: (u16, u16),
    state: &mut KittyImageState,
) -> bool {
    let Some((x0, y0, cols, rows)) = resolve_cell_placement(buf, area, image, cell_px) else {
        return false;
    };

    let target_px = (cols * cell_px.0 as u32, rows * cell_px.1 as u32);
    let decoded = match crate::tui::image_decode::decode_and_downscale_cached(
        &image.source,
        target_px,
        1.0,
        &mut state.cache,
    ) {
        Ok(decoded) => decoded,
        Err(_) => return false,
    };

    let key = crate::tui::kitty_graphics::ImageKey::new(&image.source, cols, rows);
    let id = match state.registry.touch(key) {
        crate::tui::kitty_graphics::IdLookup::New(id) => {
            for seq in crate::tui::kitty_graphics::transmit_rgba(
                id,
                decoded.width,
                decoded.height,
                &decoded.rgba,
            ) {
                state.pending_writes.extend(seq.into_bytes());
            }
            state
                .pending_writes
                .extend(crate::tui::kitty_graphics::place_virtual(id, cols, rows).into_bytes());
            id
        }
        crate::tui::kitty_graphics::IdLookup::Live(id) => id,
    };

    let fg = qc(crate::tui::kitty_graphics::placeholder_fg_color(id));
    for row in 0..rows {
        for col in 0..cols {
            let Some(text) = crate::tui::kitty_graphics::placeholder_cell_text(row, col) else {
                // Past the diacritic table's range — see
                // `ROWCOL_DIACRITICS`'s doc. Leave the cell whatever it
                // was; a placement this large is already far past any
                // real terminal's viewport.
                continue;
            };
            let x = x0 + col as u16;
            let y = y0 + row as u16;
            let cell = &mut buf[(x, y)];
            cell.set_symbol(&text);
            cell.set_fg(fg);
        }
    }
    true
}

/// Per-session state [`draw_image_halfblock`] threads across frames —
/// one instance per backend (inside [`ImageGraphicsState`]), mirroring
/// [`KittyImageState`] but without an id registry or pending-writes
/// queue: a half-block mosaic is plain cell-grid content, nothing extra
/// to transmit to the real terminal once `terminal.draw` returns.
#[cfg(feature = "tui-images")]
pub(crate) struct HalfBlockImageState {
    /// Decoded-and-downscaled pixels, keyed by source + target sample
    /// size — see [`crate::tui::image_decode`]'s module doc, "Decode
    /// cache" section.
    pub(crate) cache: crate::image_cache::ImageCache<crate::tui::image_decode::DecodedImage>,
}

#[cfg(feature = "tui-images")]
impl HalfBlockImageState {
    pub(crate) fn new() -> Self {
        Self {
            cache: crate::image_cache::ImageCache::default(),
        }
    }
}

/// Each terminal cell's glyph grid is one column wide but two "pixel"
/// samples tall (an upper-half-block `▀` carries two independently
/// colourable halves) — see [`draw_image_halfblock`]'s doc.
#[cfg(feature = "tui-images")]
const HALFBLOCK_SAMPLE_PX: (u16, u16) = (1, 2);

/// Nearest-neighbour sample of `decoded` at `(x, y)` in a `(target_w,
/// target_h)` sample grid — `decoded`'s own pixel dimensions need not
/// match that grid exactly, since [`crate::tui::image_decode::decode_and_downscale`]
/// never upscales a source smaller than the requested target. Returns
/// `theme_bg` (fully opaque) for an empty `decoded` buffer, which only
/// happens for a zero-size source — everything else always has at least
/// one pixel to sample.
#[cfg(feature = "tui-images")]
fn sample_pixel(
    decoded: &crate::tui::image_decode::DecodedImage,
    x: u32,
    y: u32,
    target_w: u32,
    target_h: u32,
    theme_bg: Color,
) -> Color {
    if decoded.width == 0 || decoded.height == 0 {
        return theme_bg;
    }
    let src_x = (x * decoded.width / target_w.max(1)).min(decoded.width - 1);
    let src_y = (y * decoded.height / target_h.max(1)).min(decoded.height - 1);
    let idx = ((src_y * decoded.width + src_x) * 4) as usize;
    let px = &decoded.rgba[idx..idx + 4];
    let raw = Color::rgba(px[0], px[1], px[2], px[3]);
    // Pre-blend the source's own alpha onto the theme background —
    // `set_fg`/`set_bg` below carry no alpha channel of their own, so an
    // image with transparency must already be composited by the time it
    // reaches the cell grid.
    theme_bg.blend(raw, px[3] as f64 / 255.0)
}

/// Paint `image` as a half-block mosaic: resolve the placement via
/// [`resolve_cell_placement`] (cell units, with [`HALFBLOCK_SAMPLE_PX`]
/// standing in for the real per-cell pixel size `draw_image_kitty` uses
/// — a `▀` glyph's own fixed 1-wide/2-tall sample shape, not the
/// terminal's actual font metrics), decode and downscale to
/// `(cols, 2 * rows)` samples, and fill each placed cell with `▀`:
/// foreground sampling the image's top pixel, background sampling its
/// bottom pixel, both alpha-blended onto `background`. Returns `false`,
/// touching no cell, when there is nothing to place or the decode fails
/// — the same two escape hatches [`draw_image_kitty`] has.
#[cfg(feature = "tui-images")]
pub(crate) fn draw_image_halfblock(
    buf: &mut Buffer,
    area: RRect,
    image: &Image,
    background: Color,
    state: &mut HalfBlockImageState,
) -> bool {
    let Some((x0, y0, cols, rows)) = resolve_cell_placement(buf, area, image, HALFBLOCK_SAMPLE_PX)
    else {
        return false;
    };

    let target_w = cols;
    let target_h = rows * 2;
    let decoded = match crate::tui::image_decode::decode_and_downscale_cached(
        &image.source,
        (target_w, target_h),
        1.0,
        &mut state.cache,
    ) {
        Ok(decoded) => decoded,
        Err(_) => return false,
    };

    for row in 0..rows {
        for col in 0..cols {
            let top = sample_pixel(decoded, col, row * 2, target_w, target_h, background);
            let bottom = sample_pixel(decoded, col, row * 2 + 1, target_w, target_h, background);
            let x = x0 + col as u16;
            let y = y0 + row as u16;
            let cell = &mut buf[(x, y)];
            cell.set_symbol("▀");
            cell.set_fg(qc(top));
            cell.set_bg(qc(bottom));
        }
    }
    true
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

#[cfg(all(test, feature = "tui-images"))]
mod kitty_tests {
    use super::*;
    use crate::primitives::image::{ImageFit, ImageSource};
    use crate::types::WidgetId;
    use ratatui::layout::Rect;

    /// A tiny solid-colour PNG, generated at test time — same approach as
    /// `image_decode`'s own test helper.
    fn tiny_png_bytes() -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([0, 255, 0, 255]));
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("encode tiny test PNG");
        out
    }

    fn image(source: ImageSource) -> Image {
        Image {
            id: WidgetId::new("logo"),
            source,
            intrinsic_size: Some((4, 4)),
            fit: ImageFit::Fill,
            fallback_text: "[LOGO]".into(),
        }
    }

    const CELL_PX: (u16, u16) = (8, 16);

    #[test]
    fn paints_placeholder_cells_and_reports_painted() {
        let area = Rect::new(0, 0, 4, 4);
        let mut buf = Buffer::empty(area);
        let mut state = KittyImageState::new();
        let img = image(ImageSource::Bytes(tiny_png_bytes()));

        let result = draw_image_kitty(&mut buf, area, &img, CELL_PX, &mut state);

        assert!(result);
        // Every cell in the placed rect carries the placeholder base
        // character, not the fallback text.
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                assert!(
                    buf[(x, y)].symbol().starts_with('\u{10EEEE}'),
                    "cell ({x},{y}) should carry the Unicode placeholder base char, got {:?}",
                    buf[(x, y)].symbol()
                );
            }
        }
        assert!(
            !state.pending_writes.is_empty(),
            "a newly transmitted image must queue transmit + placement bytes"
        );
    }

    #[test]
    fn same_image_twice_transmits_only_once() {
        let area = Rect::new(0, 0, 4, 4);
        let mut state = KittyImageState::new();
        let img = image(ImageSource::Bytes(tiny_png_bytes()));

        let mut buf1 = Buffer::empty(area);
        draw_image_kitty(&mut buf1, area, &img, CELL_PX, &mut state);
        let first_len = state.pending_writes.len();
        state.registry.begin_frame();

        let mut buf2 = Buffer::empty(area);
        let result = draw_image_kitty(&mut buf2, area, &img, CELL_PX, &mut state);

        assert!(result);
        assert_eq!(
            state.pending_writes.len(),
            first_len,
            "an id already live must not be re-transmitted"
        );
    }

    #[test]
    fn decode_failure_reports_unpainted_and_leaves_cells_alone() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let mut state = KittyImageState::new();
        let img = image(ImageSource::Bytes(vec![0, 1, 2, 3]));

        let result = draw_image_kitty(&mut buf, area, &img, CELL_PX, &mut state);

        assert!(!result);
        assert!(state.pending_writes.is_empty());
        assert_eq!(buf, Buffer::empty(area));
    }

    #[test]
    fn zero_size_placement_falls_back_without_panicking() {
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        let mut state = KittyImageState::new();
        let img = image(ImageSource::Bytes(tiny_png_bytes()));

        let result = draw_image_kitty(&mut buf, area, &img, CELL_PX, &mut state);

        assert!(!result);
    }

    #[test]
    fn contain_resolves_in_pixel_space_against_non_square_cells() {
        // 200x100 (2:1) source into a 20x10-cell rect on 8x16 cells: the
        // bounds in pixels is 160x160 (square), so `Contain`'s
        // aspect-preserving fit picks the width-limited scale
        // (160/200 = 0.8) and letterboxes to 20 cols x 5 rows, not the
        // whole 20x10 area a cell-unit (unconverted) layout would give.
        let area = Rect::new(0, 0, 20, 10);
        let mut buf = Buffer::empty(area);
        let mut state = KittyImageState::new();
        let img = Image {
            id: WidgetId::new("logo"),
            source: ImageSource::Bytes(tiny_png_bytes()),
            intrinsic_size: Some((200, 100)),
            fit: ImageFit::Contain,
            fallback_text: "[LOGO]".into(),
        };

        let result = draw_image_kitty(&mut buf, area, &img, CELL_PX, &mut state);

        assert!(result);
        let mut painted_rows = std::collections::BTreeSet::new();
        let mut painted_cols = std::collections::BTreeSet::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if buf[(x, y)].symbol().starts_with('\u{10EEEE}') {
                    painted_rows.insert(y);
                    painted_cols.insert(x);
                }
            }
        }
        assert_eq!(
            painted_rows.len(),
            5,
            "letterboxed placement should be 5 cell rows tall, got rows {painted_rows:?}"
        );
        assert_eq!(
            painted_cols.len(),
            20,
            "placement should span the full 20-cell width, got cols {painted_cols:?}"
        );
    }

    #[test]
    fn none_resolves_intrinsic_pixel_size_into_cells_not_whole_area() {
        // Intrinsic 16x32 px on 8x16 cells is exactly 2 cols x 2 rows —
        // `ImageFit::None` must place that small footprint anchored at
        // the top-left, not the literal pixel count's worth of cells
        // clipped down to the whole area, which is what resolving the
        // layout against cell units (instead of pixels) would give.
        let area = Rect::new(0, 0, 10, 10);
        let mut buf = Buffer::empty(area);
        let mut state = KittyImageState::new();
        let img = Image {
            id: WidgetId::new("logo"),
            source: ImageSource::Bytes(tiny_png_bytes()),
            intrinsic_size: Some((16, 32)),
            fit: ImageFit::None,
            fallback_text: "[LOGO]".into(),
        };

        let result = draw_image_kitty(&mut buf, area, &img, CELL_PX, &mut state);

        assert!(result);
        let mut painted = 0;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if buf[(x, y)].symbol().starts_with('\u{10EEEE}') {
                    painted += 1;
                    assert!(
                        x < 2 && y < 2,
                        "None should anchor within a 2x2-cell footprint, painted ({x},{y})"
                    );
                }
            }
        }
        assert_eq!(
            painted, 4,
            "None should place exactly intrinsic_size/cell_px = 2x2 cells, got {painted}"
        );
    }
}

#[cfg(all(test, feature = "tui-images"))]
mod halfblock_tests {
    use super::*;
    use crate::primitives::image::{ImageFit, ImageSource};
    use crate::theme::Theme;
    use crate::types::WidgetId;
    use ratatui::layout::Rect;

    /// A 2×4 px PNG split into four 2×1 solid-colour quadrants — red/green
    /// on top, blue/yellow on bottom. Each quadrant maps onto exactly one
    /// 1×1 cell of a 2×2-cell placement, so the painted cell's fg and bg
    /// should both equal that quadrant's one colour exactly (no blending
    /// ambiguity from two different shades sharing a cell).
    fn quadrant_png_bytes() -> Vec<u8> {
        let mut img = image::RgbaImage::new(2, 4);
        let red = image::Rgba([255, 0, 0, 255]);
        let green = image::Rgba([0, 255, 0, 255]);
        let blue = image::Rgba([0, 0, 255, 255]);
        let yellow = image::Rgba([255, 255, 0, 255]);
        for y in 0..2 {
            img.put_pixel(0, y, red);
            img.put_pixel(1, y, green);
        }
        for y in 2..4 {
            img.put_pixel(0, y, blue);
            img.put_pixel(1, y, yellow);
        }
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .expect("encode tiny test PNG");
        out
    }

    #[test]
    fn paints_exact_quadrant_colours_into_matching_cells() {
        let area = Rect::new(0, 0, 2, 2);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut state = HalfBlockImageState::new();
        let img = Image {
            id: WidgetId::new("photo"),
            source: ImageSource::Bytes(quadrant_png_bytes()),
            intrinsic_size: Some((2, 4)),
            fit: ImageFit::Fill,
            fallback_text: "[PHOTO]".into(),
        };

        let result = draw_image_halfblock(&mut buf, area, &img, theme.background, &mut state);

        assert!(result);
        let expect_cell = |x: u16, y: u16, rgb: (u8, u8, u8)| {
            let cell = &buf[(x, y)];
            assert_eq!(
                cell.symbol(),
                "▀",
                "cell ({x},{y}) should carry a half-block glyph"
            );
            assert_eq!(
                cell.fg,
                qc(Color::rgb(rgb.0, rgb.1, rgb.2)),
                "cell ({x},{y}) fg"
            );
            assert_eq!(
                cell.bg,
                qc(Color::rgb(rgb.0, rgb.1, rgb.2)),
                "cell ({x},{y}) bg"
            );
        };
        expect_cell(0, 0, (255, 0, 0));
        expect_cell(1, 0, (0, 255, 0));
        expect_cell(0, 1, (0, 0, 255));
        expect_cell(1, 1, (255, 255, 0));
    }

    fn image(fit: ImageFit, intrinsic_size: Option<(u32, u32)>) -> Image {
        Image {
            id: WidgetId::new("photo"),
            source: ImageSource::Bytes(quadrant_png_bytes()),
            intrinsic_size,
            fit,
            fallback_text: "[PHOTO]".into(),
        }
    }

    #[test]
    fn contain_letterboxes_against_the_halfblock_sample_grid() {
        // 2x4 (1:2) source into a 6x2-cell area: the sample-grid bounds
        // is 6 cols x 4 half-rows (2 samples per cell row). Width scale
        // 6/2 = 3, height scale 4/4 = 1 — `Contain` picks the smaller (1),
        // landing on 2 cols x 4 half-rows (2 cols x 2 cell rows),
        // letterboxed within the 6-col-wide area rather than spanning it.
        let area = Rect::new(0, 0, 6, 2);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut state = HalfBlockImageState::new();
        let img = image(ImageFit::Contain, Some((2, 4)));

        let result = draw_image_halfblock(&mut buf, area, &img, theme.background, &mut state);

        assert!(result);
        let mut painted_cols = std::collections::BTreeSet::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if buf[(x, y)].symbol() == "▀" {
                    painted_cols.insert(x);
                }
            }
        }
        // Sample-grid bounds: 6 cols x 4 half-rows. Source is 2x4 (1:2);
        // width scale = 6/2 = 3, height scale = 4/4 = 1 — contain picks
        // the smaller (1), landing on 2 cols x 4 half-rows = 1 cell row's
        // worth of width (2 cols), letterboxed within the 6-col area.
        assert_eq!(
            painted_cols.len(),
            2,
            "letterboxed placement should be 2 cols wide, got cols {painted_cols:?}"
        );
    }

    #[test]
    fn decode_failure_reports_unpainted_and_leaves_cells_alone() {
        let area = Rect::new(0, 0, 20, 3);
        let mut buf = Buffer::empty(area);
        let theme = Theme::default();
        let mut state = HalfBlockImageState::new();
        let img = Image {
            id: WidgetId::new("photo"),
            source: ImageSource::Bytes(vec![0, 1, 2, 3]),
            intrinsic_size: Some((4, 4)),
            fit: ImageFit::Fill,
            fallback_text: "[PHOTO]".into(),
        };

        let result = draw_image_halfblock(&mut buf, area, &img, theme.background, &mut state);

        assert!(!result);
        assert_eq!(buf, Buffer::empty(area));
    }

    #[test]
    fn zero_size_placement_falls_back_without_panicking() {
        let area = Rect::new(0, 0, 0, 0);
        let mut buf = Buffer::empty(Rect::new(0, 0, 1, 1));
        let theme = Theme::default();
        let mut state = HalfBlockImageState::new();
        let img = image(ImageFit::Fill, Some((4, 4)));

        let result = draw_image_halfblock(&mut buf, area, &img, theme.background, &mut state);

        assert!(!result);
    }
}
