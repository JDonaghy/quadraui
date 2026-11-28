//! The app layer: write an app as state plus a [`View`] of it, and run it
//! with `run_tui` or [`run_native`].
//!
//! ```no_run
//! use quadraui::prelude::*;
//!
//! #[derive(Default)]
//! struct Counter {
//!     count: i32,
//! }
//!
//! #[derive(Clone)]
//! enum Msg {
//!     Inc,
//!     Dec,
//! }
//!
//! impl View for Counter {
//!     type Msg = Msg;
//!     fn view(&self) -> Node<Msg> {
//!         column([
//!             text(format!("Count: {}", self.count)).id("count"),
//!             row([
//!                 button("-").id("dec").on_click(Msg::Dec),
//!                 button("+").id("inc").on_click(Msg::Inc),
//!             ]),
//!         ])
//!     }
//!     fn update(&mut self, msg: Msg) -> Command {
//!         match msg {
//!             Msg::Inc => self.count += 1,
//!             Msg::Dec => self.count -= 1,
//!         }
//!         Command::None
//!     }
//! }
//!
//! fn main() -> Result<(), quadraui::RunError> {
//!     run_native(Counter::default()) // or run_tui, with the `tui` feature
//! }
//! ```
//!
//! This is the same shape as the Python and Node app layers
//! (`quadraui.app`, `@quadraui/node/app`): a view rebuilt every frame from
//! the state, laid out by `column`/`row` containers through the core stack
//! solver, with nothing to bind or invalidate. Where those attach a
//! callback to each node, a Rust node maps its events to a message value
//! that one `update` applies, as in Elm and iced.
//!
//! [`Run`] is the adapter underneath: it implements [`crate::AppLogic`],
//! so a `View` also runs under any backend's `run`, a test driver
//! (`TuiDriver::new(Run::new(app), w, h)`) or an embedding host.

mod node;
mod run;
mod view;

pub use node::{
    button, column, form, list, row, status_bar, text, text_field, widget, Field, Leaf, Node,
};
#[cfg(feature = "tui")]
pub use run::run_tui;
pub use run::{run_native, Run};
pub use view::{Command, View};
