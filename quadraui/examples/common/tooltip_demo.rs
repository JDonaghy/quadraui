//! Backend-agnostic app code for the tooltip example
//! ([`tui_tooltip`] / [`gtk_tooltip`]).
//!
//! Demonstrates `Tooltip`'s border/title vocabulary (#541): a consumer
//! picks `border` (`Sides` bars-only, `Full` closed box, `None` no
//! chrome) and an optional `title` embedded in `Full`'s top border row,
//! instead of each backend hardcoding its own answer — the gap
//! JDonaghy/vimcode#635 hit when its migrated help popup lost its border
//! and title with no way to ask for them back. `TooltipDemo` cycles
//! through the settings so a human (or a `TuiDriver` test) can see all
//! three border modes and the title toggle render identically in shape
//! on both backends.
//!
//! Controls:
//! - 1/2/3   choose Sides / Full / None border
//! - t       toggle a title (only visible when border is Full)
//! - q / Esc quit

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, Reaction, Rect, StatusBar, StatusBarSegment, Tooltip,
    TooltipBorder, TooltipMeasure, TooltipPlacement, UiEvent, WidgetId,
};

const ANCHOR_TEXT: &str = "hover target";
const TOOLTIP_TEXT: &str = "Tooltip content";
const TOOLTIP_TITLE: &str = "Info";

pub struct TooltipDemo {
    border: TooltipBorder,
    show_title: bool,
}

impl TooltipDemo {
    pub fn new() -> Self {
        Self {
            border: TooltipBorder::Full,
            show_title: true,
        }
    }

    fn tooltip(&self) -> Tooltip {
        Tooltip::new(WidgetId::new("tooltip-demo:tip"), TOOLTIP_TEXT)
            .with_placement(TooltipPlacement::Bottom)
    }

    /// `Full` needs a top and bottom border row on top of the one content
    /// row; `Sides`/`None` reserve no border rows at all (see the
    /// contract note on `TooltipMeasure`).
    fn rows(&self) -> f32 {
        match self.border {
            TooltipBorder::Full => 3.0,
            // `Sides`, `None`, and (defensively, since `TooltipBorder` is
            // `#[non_exhaustive]`) any future variant all reserve no
            // border rows.
            _ => 1.0,
        }
    }

    fn border_label(&self) -> &'static str {
        match self.border {
            TooltipBorder::Sides => "Sides",
            TooltipBorder::Full => "Full",
            TooltipBorder::None => "None",
            // `TooltipBorder` is `#[non_exhaustive]`; a future variant
            // shows up here rather than failing to build.
            _ => "?",
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " border: {} | title: {} ",
                    self.border_label(),
                    self.show_title
                ),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " 1=Sides 2=Full 3=None t=title q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }

    fn anchor_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("anchor"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {ANCHOR_TEXT} "),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(37, 37, 38),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }
}

impl Default for TooltipDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for TooltipDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();
        let cw = backend.char_width();

        // Anchor bar at the top — the element the tooltip "describes".
        let anchor = Rect::new(0.0, 0.0, viewport.width, lh);
        let _ = backend.draw_status_bar(anchor, &self.anchor_bar(), None, None);

        // Status bar at the bottom shows the current border/title choice
        // plus the key hint.
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);

        // Tooltip renders between the two bars, below the anchor.
        let clamp = Rect::new(0.0, lh, viewport.width, viewport.height - 2.0 * lh);
        let tooltip = self.tooltip();
        let measure = TooltipMeasure::new(
            cw * (TOOLTIP_TEXT.chars().count() as f32 + 4.0),
            lh * self.rows(),
        );
        let mut layout = tooltip
            .layout(anchor, clamp, measure, lh)
            .with_border(self.border);
        if self.show_title && matches!(self.border, TooltipBorder::Full) {
            layout = layout.with_title(TOOLTIP_TITLE);
        }
        backend.draw_tooltip(&tooltip, &layout);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Char('1'),
                ..
            } => {
                self.border = TooltipBorder::Sides;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('2'),
                ..
            } => {
                self.border = TooltipBorder::Full;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('3'),
                ..
            } => {
                self.border = TooltipBorder::None;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('t'),
                ..
            } => {
                self.show_title = !self.show_title;
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
