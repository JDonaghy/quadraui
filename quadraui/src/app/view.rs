//! [`View`]: an app written as state, a view of it, and an update.

use super::node::Node;
use crate::event::{Key, NamedKey, UiEvent};

/// An app in the model-view-update shape (Elm, iced, React): the
/// implementing type is the state, [`View::view`] turns it into a [`Node`]
/// tree every frame, and [`View::update`] applies the messages the tree's
/// handlers produce. Run one with `run_tui`/[`super::run_native`],
/// or wrap it in [`super::Run`] wherever an [`crate::AppLogic`] is taken.
///
/// ```
/// use quadraui::prelude::*;
///
/// #[derive(Default)]
/// struct Counter {
///     count: i32,
/// }
///
/// #[derive(Clone)]
/// enum Msg {
///     Inc,
/// }
///
/// impl View for Counter {
///     type Msg = Msg;
///     fn view(&self) -> Node<Msg> {
///         column([
///             text(format!("Count: {}", self.count)),
///             button("+").id("inc").on_click(Msg::Inc),
///         ])
///     }
///     fn update(&mut self, msg: Msg) -> Command {
///         match msg {
///             Msg::Inc => self.count += 1,
///         }
///         Command::None
///     }
/// }
/// ```
pub trait View {
    /// What the view's handlers report and [`View::update`] applies.
    type Msg: 'static;

    /// The widget tree for the current state. Called once per frame;
    /// nothing is cached between calls, so it reads the state as it is.
    fn view(&self) -> Node<Self::Msg>;

    /// Applies one message. Every message redraws.
    fn update(&mut self, msg: Self::Msg) -> Command;

    /// The message, if any, for an event no widget in the view claimed: a
    /// key press that no focused widget took, or a resize.
    fn on_event(&self, _event: &UiEvent) -> Option<Self::Msg> {
        None
    }

    /// Whether an event that produced no message closes the app. By
    /// default `q` and Escape do when no focused widget took or reported
    /// the key (typing `q` into a text field types it, and a focused list
    /// keeps `q` for itself), and Ctrl-C does unless a focused widget
    /// consumed it (a text field takes it as copy). Return `false` to keep
    /// running on every key.
    fn quits_on(&self, event: &UiEvent) -> bool {
        let UiEvent::KeyPressed { key, modifiers, .. } = event else {
            return false;
        };
        let plain = !(modifiers.ctrl || modifiers.alt || modifiers.cmd);
        match key {
            Key::Char('q') | Key::Named(NamedKey::Escape) => plain,
            _ => is_ctrl_c(event),
        }
    }
}

/// Ctrl-C, the quit key a focused widget's report of the key does not
/// hold back.
pub(crate) fn is_ctrl_c(event: &UiEvent) -> bool {
    matches!(
        event,
        UiEvent::KeyPressed {
            key: Key::Char('c' | 'C'),
            modifiers,
            ..
        } if modifiers.ctrl && !modifiers.alt && !modifiers.cmd
    )
}

/// What the runner does after [`View::update`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    /// Redraw and keep running.
    #[default]
    None,
    /// Close the app: the runner returns.
    Exit,
}
