//! [`Editor`] on the widget contract: [`crate::Primitive`] (layout, pixel
//! paint, cell paint) and [`crate::Interactive`] (pointer and wheel).
//!
//! The editor owns no editing behaviour: the app edits its buffer and
//! hands a fresh snapshot each frame. The widget reports where a gesture
//! landed ([`EditorEvent`]) and lets every key and committed text bubble
//! to the app unconsumed.
//!
//! The column a pointer event carries is [`EditorLayout::col_at_x`]'s
//! monospace column: exact on a cell grid, and an approximation on a
//! pixel surface whose spans change the glyph advance.

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
    /// ([`EditorLayout::hit_test`]); `col` is the buffer column at the
    /// pointer's x on the hit row ([`EditorLayout::col_at_x`]). While a
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

    /// The hit and monospace column at absolute `p`.
    fn resolve(&self, editor: &Editor, p: Point) -> (EditorHit, usize) {
        let hit = self.hit_test(p.x, p.y);
        let col = self.col_at_x(editor, self.view_row_at(p.y), p.x);
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
                let (hit, col) = layout.resolve(self, p);
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
                let (hit, col) = layout.resolve(self, absolute(local));
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
                let (hit, col) = layout.resolve(self, layout.clamp_to_text(absolute(local)));
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
                let (hit, col) = layout.resolve(self, layout.clamp_to_text(absolute(local)));
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
