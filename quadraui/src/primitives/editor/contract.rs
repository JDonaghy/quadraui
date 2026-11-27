//! [`Editor`] on the widget contract: [`crate::Primitive`] (layout, pixel
//! paint, cell paint) and [`crate::Interactive`] (pointer and wheel).
//!
//! The editor owns no editing behaviour: the app edits its buffer and
//! hands a fresh snapshot each frame. The widget reports where a gesture
//! landed ([`EditorEvent`]) and lets every key and committed text bubble
//! to the app unconsumed.
//!
//! The column a pointer event carries is [`EditorLayout::col_at_x`]'s,
//! measured through the host's text surface
//! ([`crate::interactive::EventCx::text_surface`]) on a pixel backend and
//! in display cells on a cell grid.

use super::{Editor, EditorHit, EditorLayout};
use crate::event::{Point, Rect};
use crate::interactive::PointerKind;
use crate::primitives::scrollbar::ScrollAxis;
use crate::types::{Modifiers, WidgetId};
use serde::{Deserialize, Serialize};

/// A gesture on an [`Editor`], resolved against its layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EditorEvent {
    /// A pointer press, double-click, release, or a move while a drag is
    /// in progress. `hit` is the zone under the pointer
    /// ([`EditorLayout::hit_test`]); `col` is the buffer column of the
    /// glyph under the pointer's x on the hit row
    /// ([`EditorLayout::col_at_x`]), which a text-area hit carries too. While a
    /// drag is captured the pointer is clamped onto the text area, so a
    /// drag past an edge reports the edge row or column.
    Pointer {
        hit: EditorHit,
        col: usize,
        kind: PointerKind,
        mods: Modifiers,
    },
    /// A press on a scrollbar track, or a move or left release while that
    /// press holds the pointer. `fraction` is the pointer's position along
    /// the track, `0.0` at its top (left) end and `1.0` at its bottom
    /// (right) end, clamped to that range. A drag that starts on a
    /// scrollbar keeps reporting this variant wherever the pointer goes,
    /// so the app scrolls rather than extending a selection.
    Scrollbar {
        axis: ScrollAxis,
        fraction: f32,
        kind: PointerKind,
        mods: Modifiers,
    },
    /// A scroll-wheel gesture over the editor, in the platform's wheel
    /// units; positive `dy` scrolls toward the top of the buffer.
    Wheel { dx: f32, dy: f32, mods: Modifiers },
}

impl crate::interactive::WidgetAction for EditorEvent {
    fn into_ui_event(self, id: WidgetId) -> crate::UiEvent {
        crate::UiEvent::Editor(id, self)
    }
}

/// An [`Editor`]'s per-widget gesture state: the buffer position a
/// left-button text drag started at, or the scrollbar a left-button
/// scrollbar drag started on, while it is in progress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct EditorState {
    /// `(line, col)` a text drag started at, or `None` with no text drag.
    pub drag_anchor: Option<(usize, usize)>,
    /// The scrollbar a drag started on, or `None` with no scrollbar drag.
    pub scrollbar_drag: Option<ScrollAxis>,
}

impl Editor {
    /// `(cell_width, line_height, v_scrollbar_w)` for `ctx`'s units: one
    /// cell each (and a one-cell scrollbar) on a cell grid; the editor
    /// font's advance and row height, and [`super::EditorPaintOptions::v_scrollbar_w`],
    /// on a pixel surface.
    fn cell_metrics(&self, ctx: &crate::primitive::LayoutCtx) -> (f32, f32, Option<f32>) {
        match ctx.units() {
            crate::primitive::Units::Cells => (1.0, 1.0, None),
            _ => (
                ctx.char_width,
                ctx.editor_line_height(),
                self.paint_options.v_scrollbar_w,
            ),
        }
    }
}

impl EditorLayout {
    /// The view row (index into [`Editor::lines`]) at absolute `y`.
    fn view_row_at(&self, y: f32) -> usize {
        if self.line_height > 0.0 {
            ((y - self.text_bounds.y) / self.line_height)
                .floor()
                .max(0.0) as usize
        } else {
            0
        }
    }

    /// `p` moved onto the nearest point inside the text area.
    fn clamp_to_text(&self, p: Point) -> Point {
        let t = self.text_bounds;
        let inset = |lo: f32, len: f32, v: f32| {
            if len <= 0.0 {
                lo
            } else {
                v.clamp(lo, lo + len - len.min(0.5))
            }
        };
        Point::new(inset(t.x, t.width, p.x), inset(t.y, t.height, p.y))
    }

    /// `p`'s position along `axis`'s scrollbar track as a fraction in
    /// `[0.0, 1.0]`, or `None` when the layout reserves no such track.
    fn scrollbar_fraction(&self, axis: ScrollAxis, p: Point) -> Option<f32> {
        let (track, at) = match axis {
            ScrollAxis::Vertical => {
                let t = self.v_scrollbar_bounds?;
                ((t.y, t.height), p.y)
            }
            ScrollAxis::Horizontal => {
                let t = self.h_scrollbar_bounds?;
                ((t.x, t.width), p.x)
            }
        };
        let (start, len) = track;
        if len <= 0.0 {
            return Some(0.0);
        }
        Some(((at - start) / len).clamp(0.0, 1.0))
    }

    /// The hit and text column at absolute `p`, the column measured
    /// through `text` and carried by a text-area hit too.
    fn resolve(
        &self,
        editor: &Editor,
        p: Point,
        text: Option<&mut dyn crate::PaintSurface>,
    ) -> (EditorHit, usize) {
        let mut hit = self.hit_test(p.x, p.y);
        let col = self.col_at_x(editor, self.view_row_at(p.y), p.x, text);
        if let EditorHit::BufferPos { col: hit_col, .. } = &mut hit {
            *hit_col = col;
        }
        (hit, col)
    }
}

impl crate::primitive::Primitive for Editor {
    type Layout = EditorLayout;
    type Event = EditorEvent;

    fn id(&self) -> Option<&WidgetId> {
        Some(&self.id)
    }

    /// Editor text is the user's content, so it paints in the editor font.
    fn font_role(&self) -> crate::FontRole {
        crate::FontRole::Editor
    }

    fn capture(&self) -> crate::frame::OwnedContent {
        crate::frame::OwnedContent::Editor(self.clone())
    }

    /// An editor fills the pane it is given.
    fn measure(
        &self,
        _ctx: &crate::primitive::LayoutCtx,
        _avail: crate::primitive::Size,
    ) -> crate::NaturalSize {
        crate::NaturalSize::NONE
    }

    /// Lays the editor out in `rect` (not [`Editor::rect`]) with the
    /// descriptor's [`Editor::paint_options`].
    fn layout(&self, ctx: &crate::primitive::LayoutCtx, rect: Rect) -> EditorLayout {
        let (cell_w, line_h, v_scrollbar_w) = self.cell_metrics(ctx);
        self.layout_with_options_and_reserve(
            rect,
            cell_w,
            line_h,
            self.paint_options,
            v_scrollbar_w,
            self.paint_options.right_reserve_w,
        )
    }

    fn paint(
        &self,
        layout: &EditorLayout,
        surface: &mut dyn crate::paint_surface::PaintSurface,
        env: &crate::PaintEnv,
    ) {
        let theme = env.theme();
        #[cfg(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        ))]
        super::paint_pixels::paint(self, layout, surface, theme);
        #[cfg(not(any(
            feature = "gtk",
            feature = "win",
            all(feature = "macos", target_os = "macos")
        )))]
        let _ = (layout, surface, theme);
    }

    #[cfg(feature = "tui")]
    fn paint_cells(
        &self,
        layout: &EditorLayout,
        cells: &mut dyn crate::CellSurface,
        env: &crate::PaintEnv,
    ) {
        let reserve = self.paint_options.right_reserve_w.max(0.0).round() as u16;
        super::paint_cells::paint(self, layout, reserve, cells, env.theme());
    }
}

impl crate::interactive::Interactive for Editor {
    type State = EditorState;
    type Action = EditorEvent;

    /// One focus stop: a focused editor is where the app sends keys.
    fn focus(&self) -> crate::interactive::FocusSpec {
        crate::interactive::FocusSpec::Stop
    }

    /// - **Press** (any button) takes focus. On a scrollbar track it
    ///   reports [`EditorEvent::Scrollbar`], elsewhere
    ///   [`EditorEvent::Pointer`]. A left press inside the editor also
    ///   captures the pointer and starts a drag: a scrollbar drag on a
    ///   track, a text drag anywhere else.
    /// - **Move** during a text drag reports the pointer clamped onto the
    ///   text area; during a scrollbar drag it reports
    ///   [`EditorEvent::Scrollbar`] for the pressed track. A move with no
    ///   drag is ignored.
    /// - **Left release** ends the drag, releases the capture and reports
    ///   the release in the drag's own variant.
    /// - **Double-click** reports [`EditorEvent::Pointer`] with
    ///   [`PointerKind::DoubleClick`].
    /// - **Wheel** reports [`EditorEvent::Wheel`].
    /// - **Actions**: `Focus`/`Click` take focus; `ScrollUp`/`ScrollDown`
    ///   report one wheel notch.
    ///
    /// Keys and committed text are left unconsumed so they reach the app.
    fn handle(
        &self,
        state: &mut EditorState,
        event: &crate::interactive::RoutedEvent,
        layout: &EditorLayout,
        cx: &mut crate::interactive::EventCx,
    ) -> crate::Handled<EditorEvent> {
        use crate::a11y::A11yAction;
        use crate::interactive::RoutedEvent;
        use crate::Handled;

        let origin = Point::new(layout.bounds.x, layout.bounds.y);
        let absolute = |local: &Point| Point::new(origin.x + local.x, origin.y + local.y);

        match event {
            RoutedEvent::Pointer {
                kind: kind @ PointerKind::Down(button),
                local,
                mods,
                ..
            } => {
                cx.request_focus(None);
                let p = absolute(local);
                let (hit, col) = layout.resolve(self, p, cx.text_surface());
                let axis = match hit {
                    EditorHit::VScrollbar => Some(ScrollAxis::Vertical),
                    EditorHit::HScrollbar => Some(ScrollAxis::Horizontal),
                    _ => None,
                };
                if let Some(axis) = axis {
                    if *button == crate::MouseButton::Left {
                        cx.capture_pointer();
                        state.drag_anchor = None;
                        state.scrollbar_drag = Some(axis);
                    }
                    return Handled::Event(EditorEvent::Scrollbar {
                        axis,
                        fraction: layout.scrollbar_fraction(axis, p).unwrap_or(0.0),
                        kind: *kind,
                        mods: *mods,
                    });
                }
                if *button == crate::MouseButton::Left && hit != EditorHit::Empty {
                    cx.capture_pointer();
                    let line = match hit {
                        EditorHit::BufferPos { line, .. } => line,
                        EditorHit::Gutter { line_idx, .. } => line_idx,
                        _ => layout.scroll_top,
                    };
                    state.scrollbar_drag = None;
                    state.drag_anchor = Some((line, col));
                }
                Handled::Event(EditorEvent::Pointer {
                    hit,
                    col,
                    kind: *kind,
                    mods: *mods,
                })
            }
            RoutedEvent::Pointer {
                kind: kind @ PointerKind::DoubleClick(_),
                local,
                mods,
                ..
            } => {
                let (hit, col) = layout.resolve(self, absolute(local), cx.text_surface());
                Handled::Event(EditorEvent::Pointer {
                    hit,
                    col,
                    kind: *kind,
                    mods: *mods,
                })
            }
            RoutedEvent::Pointer {
                kind: PointerKind::Move,
                local,
                mods,
                ..
            } => {
                if let Some(axis) = state.scrollbar_drag {
                    let Some(fraction) = layout.scrollbar_fraction(axis, absolute(local)) else {
                        return Handled::Ignored;
                    };
                    return Handled::Event(EditorEvent::Scrollbar {
                        axis,
                        fraction,
                        kind: PointerKind::Move,
                        mods: *mods,
                    });
                }
                if state.drag_anchor.is_none() {
                    return Handled::Ignored;
                }
                let (hit, col) = layout.resolve(
                    self,
                    layout.clamp_to_text(absolute(local)),
                    cx.text_surface(),
                );
                Handled::Event(EditorEvent::Pointer {
                    hit,
                    col,
                    kind: PointerKind::Move,
                    mods: *mods,
                })
            }
            RoutedEvent::Pointer {
                kind: kind @ PointerKind::Up(crate::MouseButton::Left),
                local,
                mods,
                ..
            } => {
                if let Some(axis) = state.scrollbar_drag.take() {
                    cx.release_pointer();
                    let Some(fraction) = layout.scrollbar_fraction(axis, absolute(local)) else {
                        return Handled::Consumed;
                    };
                    return Handled::Event(EditorEvent::Scrollbar {
                        axis,
                        fraction,
                        kind: *kind,
                        mods: *mods,
                    });
                }
                if state.drag_anchor.take().is_none() {
                    return Handled::Ignored;
                }
                cx.release_pointer();
                let (hit, col) = layout.resolve(
                    self,
                    layout.clamp_to_text(absolute(local)),
                    cx.text_surface(),
                );
                Handled::Event(EditorEvent::Pointer {
                    hit,
                    col,
                    kind: *kind,
                    mods: *mods,
                })
            }
            RoutedEvent::Wheel { dx, dy, mods, .. } => Handled::Event(EditorEvent::Wheel {
                dx: *dx,
                dy: *dy,
                mods: *mods,
            }),
            RoutedEvent::Action(A11yAction::Focus | A11yAction::Click) => {
                cx.request_focus(None);
                Handled::Consumed
            }
            RoutedEvent::Action(A11yAction::ScrollUp) => Handled::Event(EditorEvent::Wheel {
                dx: 0.0,
                dy: 1.0,
                mods: Modifiers::default(),
            }),
            RoutedEvent::Action(A11yAction::ScrollDown) => Handled::Event(EditorEvent::Wheel {
                dx: 0.0,
                dy: -1.0,
                mods: Modifiers::default(),
            }),
            RoutedEvent::FocusOut => {
                let had_text_drag = state.drag_anchor.take().is_some();
                if state.scrollbar_drag.take().is_some() || had_text_drag {
                    cx.release_pointer();
                }
                Handled::Ignored
            }
            _ => Handled::Ignored,
        }
    }

    fn a11y_actions(&self, _state: &EditorState) -> Vec<crate::a11y::A11yAction> {
        use crate::a11y::A11yAction;
        vec![
            A11yAction::Focus,
            A11yAction::ScrollUp,
            A11yAction::ScrollDown,
        ]
    }
}

/// Click-to-column and caret placement on every pixel backend's headless
/// driver, through the runtime's widget routing: for each column the
/// painted caret's x is read back from the frame, the host API
/// ([`EditorLayout::x_at_col`] over [`crate::Backend::paint_surface`])
/// must agree with it, and a click a quarter and three quarters into each
/// glyph must come back as that glyph's column.
#[cfg(all(
    test,
    any(
        feature = "gtk",
        all(feature = "macos", target_os = "macos"),
        all(feature = "win", target_os = "windows")
    )
))]
mod pixel_driver_tests {
    use super::super::{CursorPos, CursorShape, EditorCursor, EditorLine, EditorStyledSpan, Style};
    use super::*;
    use crate::{AppLogic, Backend, FrameTree, Reaction, ScreenLayout, Surface, UiEvent};

    const RECT: Rect = Rect::new(12.0, 6.0, 560.0, 60.0);

    /// One editor with a one-line buffer, recording each pointer column.
    pub(super) struct ClickApp {
        editor: Editor,
        font: Option<&'static str>,
        cols: Vec<usize>,
    }

    /// `(start, end, bold, italic, scale)` styles over byte ranges.
    type Spans<'a> = &'a [(usize, usize, bool, bool, f32)];

    impl ClickApp {
        pub(super) fn new(text: &str, spans: Spans, font: Option<&'static str>) -> Self {
            let style = |bold, italic, font_scale| Style {
                fg: crate::Color::rgb(200, 40, 40),
                bg: None,
                bold,
                italic,
                font_scale,
            };
            let mut all = vec![EditorStyledSpan {
                start_byte: 0,
                end_byte: text.len(),
                style: style(false, false, 1.0),
            }];
            all.extend(spans.iter().map(|&(s, e, b, i, k)| EditorStyledSpan {
                start_byte: s,
                end_byte: e,
                style: style(b, i, k),
            }));
            let line = EditorLine {
                raw_text: text.to_string(),
                gutter_text: String::new(),
                spans: all,
                line_idx: 0,
                is_current_line: false,
                is_fold_header: false,
                folded_line_count: 0,
                git_diff: None,
                diff_status: None,
                diagnostics: vec![],
                spell_errors: vec![],
                is_breakpoint: false,
                is_conditional_bp: false,
                is_dap_current: false,
                is_wrap_continuation: false,
                segment_col_offset: 0,
                annotation: None,
                ghost_suffix: None,
                is_ghost_continuation: false,
                indent_guides: vec![],
                colorcolumns: vec![],
            };
            let editor = Editor::new(WidgetId::new("editor:click"), RECT)
                .with_lines(vec![line])
                .with_total_lines(1);
            Self {
                editor,
                font,
                cols: Vec::new(),
            }
        }

        fn set_caret(&mut self, col: Option<usize>) {
            self.editor.cursor = col.map(|col| EditorCursor {
                pos: CursorPos { view_line: 0, col },
                shape: CursorShape::Bar,
            });
        }
    }

    impl AppLogic for ClickApp {
        type AreaId = ();

        fn setup(&mut self, backend: &mut dyn Backend) {
            if let Some(font) = self.font {
                backend.set_editor_font(font, 14.0);
            }
        }

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            let mut frame = ScreenLayout::new();
            frame.push(Surface::Widget {
                rect: RECT,
                widget: Box::new(self.editor.clone()),
            });
            frame.draw(backend);
        }

        fn frame_tree(&self, _area: ()) -> FrameTree {
            let mut tree = FrameTree::new();
            tree.push(self.editor.id, RECT, Box::new(self.editor.clone()));
            tree
        }

        fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            if let UiEvent::Editor(_, EditorEvent::Pointer { col, .. }) = event {
                self.cols.push(col);
            }
            Reaction::Continue
        }
    }

    /// What a test needs from a backend's headless driver.
    pub(super) trait ClickDriver {
        fn app(&mut self) -> &mut ClickApp;
        fn repaint(&mut self);
        fn click_at(&mut self, x: f32, y: f32);
        /// The painted RGB at `(x, y)`.
        fn rgb(&mut self, x: u32, y: u32) -> (u8, u8, u8);
        /// `col`'s x through the host API, outside any frame.
        fn host_x_at_col(&mut self, col: usize) -> f32;
    }

    /// The editor row's pixels along `y`, as last painted.
    fn row(d: &mut dyn ClickDriver, y: u32) -> Vec<(u8, u8, u8)> {
        (RECT.x as u32..(RECT.x + RECT.width) as u32)
            .map(|x| d.rgb(x, y))
            .collect()
    }

    /// The painted caret's left edge, in whole pixels: the first pixel
    /// along `y` that differs from `bare`, the row painted with no caret.
    fn painted_caret_x(d: &mut dyn ClickDriver, y: u32, bare: &[(u8, u8, u8)]) -> f32 {
        let at = row(d, y)
            .iter()
            .zip(bare)
            .position(|(a, b)| a != b)
            .expect("the bar caret paints on the row");
        RECT.x + at as f32
    }

    /// Every column of the app's line: the caret paints where the host
    /// API places it, and clicks inside each glyph resolve to it.
    pub(super) fn assert_columns_follow_glyphs(d: &mut dyn ClickDriver, case: &str) {
        let text = d.app().editor.lines[0].raw_text.clone();
        let n = text.chars().count();
        let caret_y = (RECT.y + 3.0) as u32;
        d.app().set_caret(None);
        d.repaint();
        let bare = row(d, caret_y);
        let xs: Vec<f32> = (0..=n)
            .map(|col| {
                d.app().set_caret(Some(col));
                d.repaint();
                let painted = painted_caret_x(d, caret_y, &bare);
                let host = d.host_x_at_col(col);
                assert!(
                    (painted - host).abs() <= 1.0,
                    "{case}: caret at col {col} painted at x={painted}, host API says {host}"
                );
                host
            })
            .collect();
        let widths: Vec<f32> = xs.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            widths.iter().any(|w| (w - widths[0]).abs() > 1.0),
            "{case}: every glyph is one advance wide ({widths:?}), so the case proves nothing"
        );
        let y = RECT.y + 3.0;
        for (col, (&x, &w)) in xs.iter().zip(&widths).enumerate() {
            if w <= 0.0 {
                continue;
            }
            for frac in [0.25, 0.75] {
                d.click_at(x + w * frac, y);
                let got = d.app().cols.last().copied();
                assert_eq!(
                    got,
                    Some(col),
                    "{case}: click {frac} into {:?} (col {col}, x {x}..{})",
                    text.chars().nth(col),
                    x + w
                );
            }
        }
    }

    /// The four lines every backend runs: wide glyphs in the monospace
    /// editor font, a proportional font, and bold, italic and scaled runs.
    pub(super) fn run_all(make: &mut dyn FnMut(ClickApp) -> Box<dyn ClickDriver>) {
        let wide = "a\u{2705}b\u{4e2d}\u{6587}x";
        assert_columns_follow_glyphs(&mut *make(ClickApp::new(wide, &[], None)), "wide glyphs");
        let prop = "iiWWmil.";
        assert_columns_follow_glyphs(
            &mut *make(ClickApp::new(prop, &[], Some("sans-serif"))),
            "proportional font",
        );
        let styled = "abcdefghij";
        let spans: Spans = &[
            (0, 2, true, false, 1.0),
            (2, 4, false, true, 1.0),
            (4, 6, false, false, 1.25),
            (6, 8, true, true, 1.0),
        ];
        assert_columns_follow_glyphs(
            &mut *make(ClickApp::new(styled, spans, Some("sans-serif"))),
            "bold, italic and scaled runs",
        );
        assert_columns_follow_glyphs(
            &mut *make(ClickApp::new(wide, &[(0, 4, true, false, 1.0)], None)),
            "a bold wide glyph",
        );
    }

    /// Implements [`ClickDriver`] for a driver type whose `pixel` read is
    /// adapted by `$rgb`.
    macro_rules! click_driver {
        ($driver:ty, $rgb:expr) => {
            impl ClickDriver for $driver {
                fn app(&mut self) -> &mut ClickApp {
                    self.app_mut()
                }
                fn repaint(&mut self) {
                    self.render();
                }
                fn click_at(&mut self, x: f32, y: f32) {
                    self.click(x, y);
                }
                fn rgb(&mut self, x: u32, y: u32) -> (u8, u8, u8) {
                    let rgb: fn(&mut $driver, u32, u32) -> (u8, u8, u8) = $rgb;
                    rgb(self, x, y)
                }
                fn host_x_at_col(&mut self, col: usize) -> f32 {
                    use crate::backend::DrawsPrimitives;
                    let (backend, app) = self.parts_mut();
                    let layout = backend.layout_primitive(RECT, &app.editor);
                    layout.x_at_col(&app.editor, 0, col, backend.paint_surface())
                }
            }
        };
    }

    #[cfg(feature = "gtk")]
    click_driver!(crate::gtk::testing::GtkDriver<ClickApp>, |d, x, y| d
        .pixel(x as i32, y as i32));

    #[cfg(feature = "gtk")]
    #[test]
    fn gtk_clicks_and_carets_follow_shaped_glyphs() {
        run_all(&mut |app| {
            let mut d = crate::gtk::testing::GtkDriver::new(app, 600, 80);
            d.set_double_click_folding(false);
            Box::new(d)
        });
    }

    #[cfg(all(feature = "macos", target_os = "macos"))]
    click_driver!(crate::macos::testing::MacDriver<ClickApp>, |d, x, y| {
        let (r, g, b, _) = d.pixel(x, y);
        (r, g, b)
    });

    #[cfg(all(feature = "macos", target_os = "macos"))]
    #[test]
    fn macos_clicks_and_carets_follow_shaped_glyphs() {
        run_all(&mut |app| Box::new(crate::macos::testing::MacDriver::new(app, 600, 80)));
    }

    #[cfg(all(feature = "win", target_os = "windows"))]
    click_driver!(crate::win::testing::WinDriver<ClickApp>, |d, x, y| {
        let c = d.pixel(x, y);
        (c.r, c.g, c.b)
    });

    #[cfg(all(feature = "win", target_os = "windows"))]
    #[test]
    fn win_clicks_and_carets_follow_shaped_glyphs() {
        run_all(&mut |app| {
            let mut d = crate::win::testing::WinDriver::new(app, 600, 80);
            // The DirectWrite formats are built when a surface attaches,
            // which precedes `setup`'s font choice in the driver, so
            // attach again to paint in that font.
            let target = d.surface().target().clone();
            d.backend_mut()
                .attach_headless(target, 600, 80)
                .expect("re-attach the headless surface");
            d.render();
            Box::new(d)
        });
    }
}
