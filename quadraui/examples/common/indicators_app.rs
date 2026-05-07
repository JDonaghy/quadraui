//! Backend-agnostic app code for the progress + spinner example
//! ([`tui_indicators`] / [`gtk_indicators`]).
//!
//! [`IndicatorsApp`] demonstrates a [`ProgressBar`] and a [`Spinner`]
//! side by side.
//!
//! Controls:
//! - space       advance progress 10%
//! - r           reset progress
//! - i           toggle indeterminate mode
//! - c           toggle cancel button
//! - q / Esc     quit
//!
//! The spinner auto-advances its frame on every redraw.

use quadraui::{
    AppLogic, Backend, Color, Key, NamedKey, ProgressBar, ProgressBarHit, Reaction, Rect, Spinner,
    StatusBar, StatusBarSegment, UiEvent, WidgetId,
};

pub struct IndicatorsApp {
    progress: f32,
    indeterminate: bool,
    cancellable: bool,
    spinner_frame: usize,
    last_message: String,
}

impl IndicatorsApp {
    pub fn new() -> Self {
        Self {
            progress: 0.3,
            indeterminate: false,
            cancellable: false,
            spinner_frame: 0,
            last_message: "space=+10% r=reset i=indet c=cancel".into(),
        }
    }

    fn progress_bar(&self) -> ProgressBar {
        ProgressBar {
            id: WidgetId::new("prog"),
            label: if self.indeterminate {
                "Working...".into()
            } else {
                format!("{:.0}%", self.progress * 100.0)
            },
            value: if self.indeterminate {
                None
            } else {
                Some(self.progress)
            },
            frame_idx: self.spinner_frame,
            cancellable: self.cancellable,
            accent: None,
        }
    }

    fn spinner(&self) -> Spinner {
        Spinner {
            id: WidgetId::new("spin"),
            label: "Loading...".into(),
            frame_idx: self.spinner_frame,
            accent: None,
        }
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("status"),
            left_segments: vec![StatusBarSegment {
                text: format!(" {} ", self.last_message),
                fg: Color::rgb(255, 255, 255),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![StatusBarSegment {
                text: " q=quit ".into(),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 80, 120),
                bold: false,
                action_id: None,
            }],
        }
    }
}

impl Default for IndicatorsApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AppLogic for IndicatorsApp {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let viewport = backend.viewport();
        let lh = backend.line_height();

        // Spinner on row 1.
        let spinner_rect = Rect::new(1.0, 1.0, viewport.width - 2.0, lh);
        let _ = backend.draw_spinner(spinner_rect, &self.spinner());

        // Progress bar on row 3.
        let progress_rect = Rect::new(1.0, lh * 3.0, viewport.width - 2.0, lh);
        let _ = backend.draw_progress(progress_rect, &self.progress_bar());

        // Status bar at bottom.
        let status_rect = Rect::new(0.0, viewport.height - lh, viewport.width, lh);
        let _ = backend.draw_status_bar(status_rect, &self.status_bar(), None, None);
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
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
                key: Key::Char(' '),
                ..
            } => {
                self.progress = (self.progress + 0.1).min(1.0);
                self.spinner_frame += 1;
                self.last_message = format!("Progress: {:.0}%", self.progress * 100.0);
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('r'),
                ..
            } => {
                self.progress = 0.0;
                self.last_message = "Reset".into();
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('i'),
                ..
            } => {
                self.indeterminate = !self.indeterminate;
                self.last_message = if self.indeterminate {
                    "Indeterminate".into()
                } else {
                    "Determinate".into()
                };
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('c'),
                ..
            } => {
                self.cancellable = !self.cancellable;
                self.last_message = if self.cancellable {
                    "Cancel enabled".into()
                } else {
                    "Cancel disabled".into()
                };
                Reaction::Redraw
            }
            UiEvent::MouseDown { position, .. } => {
                let viewport = backend.viewport();
                let lh = backend.line_height();
                let progress_rect = Rect::new(1.0, lh * 3.0, viewport.width - 2.0, lh);
                let bar = self.progress_bar();
                let layout = backend.progress_layout(progress_rect, &bar);
                match layout.hit_test(position.x, position.y) {
                    ProgressBarHit::Cancel(_) => {
                        self.last_message = "Cancelled!".into();
                        self.progress = 0.0;
                        self.indeterminate = false;
                    }
                    ProgressBarHit::Body(_) => {
                        self.last_message = "Bar clicked".into();
                    }
                    ProgressBarHit::Empty => return Reaction::Continue,
                }
                Reaction::Redraw
            }
            UiEvent::WindowResized { .. } => Reaction::Redraw,
            _ => Reaction::Continue,
        }
    }
}
