//! [`Run`]: the [`AppLogic`] adapter that runs a [`View`] on any backend.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use super::node::{Metrics, Placed};
use super::view::{is_ctrl_c, Command, View};
use crate::backend::Backend;
use crate::event::{Rect, UiEvent};
use crate::frame::{record_captured_frame, OwnedFrame, ScreenLayout, Surface};
use crate::frame_tree::FrameTree;
use crate::primitives::form::{Form, FormEvent};
use crate::runner::{AppLogic, Reaction};
use crate::types::WidgetId;
use crate::RunError;

/// Runs a [`View`] as an [`AppLogic`], so any backend runner, test driver
/// or embedding host takes it.
///
/// Each frame builds the view once: `render` lays the tree out against the
/// backend's viewport and metrics, paints the placed widgets in place, and
/// keeps them. Routing (`frame_tree`, `extra_tab_stops`) and `handle` use
/// that kept frame; only `frame_tree`, whose tree owns its widgets, copies
/// them. A message marks the frame stale, so an event routed before the
/// next paint sees a tree rebuilt from the updated state at the same
/// metrics.
///
/// `Run` derefs to the view, so a test reads `driver.app().count`.
pub struct Run<V: View> {
    view: V,
    frame: RefCell<Frame<V::Msg>>,
}

struct Frame<M> {
    /// The metrics of the last paint; `None` before the first one.
    metrics: Option<Metrics>,
    placed: Vec<Placed<M>>,
    /// The state changed since `placed` was built.
    stale: bool,
    /// The widget holding keyboard focus, as of the last paint or event.
    focused: Option<WidgetId>,
    /// The focused field of each form the view leaves uncontrolled.
    form_fields: HashMap<WidgetId, WidgetId>,
    /// The last event was a focused widget's own report of a key press,
    /// so the raw key press the runtime delivers next was taken by it.
    key_reported: bool,
}

impl<V: View> Run<V> {
    /// Wraps `view` for a runner.
    pub fn new(view: V) -> Self {
        Self {
            view,
            frame: RefCell::new(Frame {
                metrics: None,
                placed: Vec::new(),
                stale: true,
                focused: None,
                form_fields: HashMap::new(),
                key_reported: false,
            }),
        }
    }

    /// The wrapped view.
    pub fn into_inner(self) -> V {
        self.view
    }

    /// Builds the view and lays it out at `metrics`.
    fn rebuild(&self, frame: &mut Frame<V::Msg>, metrics: Metrics) {
        let mut placed = self.view.view().place(&metrics);
        for leaf in &mut placed {
            if let Some(form) = leaf.widget.as_any_mut().downcast_mut::<Form>() {
                frame.keep_form_focus(form);
            }
        }
        frame.placed = placed;
        frame.metrics = Some(metrics);
        frame.stale = false;
    }

    /// The kept frame, rebuilt first if the state changed since it was
    /// laid out.
    fn current(&self) -> std::cell::RefMut<'_, Frame<V::Msg>> {
        let mut frame = self.frame.borrow_mut();
        if frame.stale {
            if let Some(metrics) = frame.metrics {
                self.rebuild(&mut frame, metrics);
            }
        }
        frame
    }
}

impl<M> Frame<M> {
    /// The placed widgets at their rects, borrowed.
    fn widgets(&self) -> impl Iterator<Item = (Rect, &dyn crate::primitive::DynWidget)> {
        self.placed.iter().map(|leaf| {
            (
                leaf.rect,
                leaf.widget.as_ref() as &dyn crate::primitive::DynWidget,
            )
        })
    }

    /// A copy of the placed widgets, for a consumer that owns them.
    fn screen_layout(&self) -> ScreenLayout {
        let mut layout = ScreenLayout::new();
        for leaf in &self.placed {
            layout.push(Surface::Widget {
                rect: leaf.rect,
                widget: leaf.widget.clone_dyn(),
            });
        }
        layout
    }

    /// Gives a form the view left uncontrolled the field it last reported
    /// focused, while keyboard focus is on the form.
    fn keep_form_focus(&self, form: &mut Form) {
        if form.focused_field.is_some() || self.focused != Some(form.id) {
            return;
        }
        if let Some(field) = self.form_fields.get(&form.id) {
            form.focused_field = Some(*field);
            form.has_focus = true;
        }
    }
}

impl<V: View> Deref for Run<V> {
    type Target = V;
    fn deref(&self) -> &V {
        &self.view
    }
}

impl<V: View> DerefMut for Run<V> {
    /// Marks the kept frame stale: the caller may change the state.
    fn deref_mut(&mut self) -> &mut V {
        self.frame.get_mut().stale = true;
        &mut self.view
    }
}

impl<V: View> AppLogic for Run<V> {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _: ()) {
        let viewport = backend.viewport();
        let metrics = Metrics {
            area: Rect::new(0.0, 0.0, viewport.width, viewport.height),
            char_width: backend.char_width(),
            line_height: backend.line_height(),
            nerd_fonts: backend.nerd_fonts_enabled(),
        };
        let mut frame = self.frame.borrow_mut();
        frame.focused = backend.focus_tree().focused_id().copied();
        self.rebuild(&mut frame, metrics);
        for (rect, widget) in frame.widgets() {
            crate::frame::draw_widget(backend, rect, widget);
        }
        record_captured_frame(backend, || OwnedFrame::from_layout(&frame.screen_layout()));
    }

    fn frame_tree(&self, _: ()) -> FrameTree {
        self.current().screen_layout().into_frame_tree()
    }

    fn extra_tab_stops(&self, _: ()) -> Vec<(WidgetId, Rect)> {
        ScreenLayout::tab_stops_of(self.current().widgets())
    }

    fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
        let frame = self.frame.get_mut();
        let focused = backend.focus_tree().focused_id().copied();
        frame.focused = focused;
        let forms_before = frame.form_fields.len();
        frame.form_fields.retain(|form, _| Some(*form) == focused);
        let key_taken =
            std::mem::take(&mut frame.key_reported) && matches!(event, UiEvent::KeyPressed { .. });
        frame.key_reported = is_key_report(&event);
        let mut reaction = Reaction::Continue;
        if let UiEvent::Form(form, FormEvent::FocusChanged { id }) = &event {
            frame.form_fields.insert(*form, *id);
            reaction = Reaction::Redraw;
        }
        if matches!(event, UiEvent::FocusChanged(_)) {
            reaction = Reaction::Redraw;
        }
        if frame.form_fields.len() != forms_before || reaction == Reaction::Redraw {
            frame.stale = true;
        }
        let message = frame
            .placed
            .iter()
            .find_map(|leaf| {
                let id = *leaf.widget.id()?;
                leaf.handlers.iter().find_map(|handler| handler(id, &event))
            })
            .or_else(|| self.view.on_event(&event));
        let Some(message) = message else {
            let quit_candidate = !key_taken || is_ctrl_c(&event);
            if quit_candidate && self.view.quits_on(&event) {
                return Reaction::Exit;
            }
            return reaction;
        };
        frame.stale = true;
        match self.view.update(message) {
            Command::Exit => Reaction::Exit,
            _ => Reaction::Redraw,
        }
    }
}

/// A focused widget's own report of a key press it did not otherwise use,
/// which the runtime follows with the raw key press itself.
fn is_key_report(event: &UiEvent) -> bool {
    use crate::compose::chat::ChatEvent;
    use crate::primitives::pipeline_view::PipelineEvent;
    use crate::{
        ActivityBarEvent, FormEvent, ListViewEvent, PaletteEvent, StatusBarEvent, TabBarEvent,
        TerminalEvent, TextDisplayEvent, TreeEvent,
    };
    matches!(
        event,
        UiEvent::List(_, ListViewEvent::KeyPressed { .. })
            | UiEvent::Tree(_, TreeEvent::KeyPressed { .. })
            | UiEvent::Form(_, FormEvent::KeyPressed { .. })
            | UiEvent::Palette(_, PaletteEvent::KeyPressed { .. })
            | UiEvent::TabBar(_, TabBarEvent::KeyPressed { .. })
            | UiEvent::StatusBar(_, StatusBarEvent::KeyPressed { .. })
            | UiEvent::ActivityBar(_, ActivityBarEvent::KeyPressed { .. })
            | UiEvent::Terminal(_, TerminalEvent::KeyPressed { .. })
            | UiEvent::TextDisplay(_, TextDisplayEvent::KeyPressed { .. })
            | UiEvent::Chat(_, ChatEvent::KeyPressed { .. })
            | UiEvent::PipelineView(_, PipelineEvent::KeyPressed { .. })
    )
}

/// Runs `view` in the terminal until it exits: `quadraui::tui::run` over
/// [`Run`].
#[cfg(feature = "tui")]
#[cfg_attr(docsrs, doc(cfg(feature = "tui")))]
pub fn run_tui<V: View>(view: V) -> Result<(), RunError> {
    crate::tui::run(Run::new(view))
}

/// The native window [`run_native_with`] opens: its title and, on GTK,
/// its application id. A field left `None` takes the backend's default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NativeOptions {
    /// The window title.
    pub title: Option<String>,
    /// The GTK application id (`org.example.App`); other backends have
    /// none.
    pub app_id: Option<String>,
}

impl NativeOptions {
    /// Options for a window titled `title`.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            app_id: None,
        }
    }

    /// Sets the GTK application id.
    pub fn app_id(mut self, app_id: impl Into<String>) -> Self {
        self.app_id = Some(app_id.into());
        self
    }
}

/// Runs `view` in a native window until it closes, on the native backend
/// this build enables: macOS (on a Mac), then GTK, then Win32 (on
/// Windows). With none enabled it returns
/// `RunError::Backend(BackendError::Unsupported)`, so an app can fall back
/// to `run_tui`. [`run_native_with`] sets the window title.
pub fn run_native<V: View + 'static>(view: V) -> Result<(), RunError> {
    run_native_with(view, NativeOptions::default())
}

/// [`run_native`] with a window title and app id:
/// `run_native_with(app, NativeOptions::new("Counter"))`.
#[allow(unreachable_code, unused_variables)]
pub fn run_native_with<V: View + 'static>(view: V, options: NativeOptions) -> Result<(), RunError> {
    #[cfg(all(feature = "macos", target_os = "macos"))]
    {
        let defaults = crate::macos::RunConfig::default();
        let config = crate::macos::RunConfig::new(options.title.unwrap_or(defaults.title));
        return crate::macos::run_with(Run::new(view), config);
    }
    #[cfg(feature = "gtk")]
    {
        let defaults = crate::gtk::RunConfig::default();
        let config = crate::gtk::RunConfig::new(
            options.app_id.unwrap_or(defaults.app_id),
            options.title.unwrap_or(defaults.title),
        );
        return crate::gtk::run_with(Run::new(view), config);
    }
    #[cfg(all(feature = "win", target_os = "windows"))]
    {
        let defaults = crate::win::RunConfig::default();
        let config = crate::win::RunConfig::new(options.title.unwrap_or(defaults.title));
        return crate::win::run_with(Run::new(view), config);
    }
    Err(RunError::Backend(crate::backend::BackendError::Unsupported))
}
