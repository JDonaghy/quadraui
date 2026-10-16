//! Curated one-line import for a first quadraui app.
//!
//! quadraui re-exports roughly 500 names flat at the crate root — every
//! primitive, every layout struct, every hit-test enum, every backend
//! trait. That's the right shape for a consumer that already knows the
//! crate (grep `quadraui::` and take whatever comes back), but it's a
//! wall for a first-time reader who just wants to get `examples/hello.rs`
//! on screen and has no way to tell `StatusBar` (the thing they need)
//! from `SidebarPanelMeasure` (the thing they don't, yet).
//!
//! `quadraui::prelude::*` is not a replacement for the crate root — it's
//! a subset, picked for "building your first `AppLogic` app": the two
//! runner traits, the event/geometry types every `handle`/`render`
//! signature touches, and a handful of primitives representative enough
//! to build a real two-pane app (see `docs/GUIDE.md`) without reaching
//! for the full re-export list. Reach past the prelude to `quadraui::*`
//! (or a specific `quadraui::primitives::*` module) the moment you need
//! a primitive it doesn't carry — nothing here is hidden or renamed,
//! it's the same items the crate root already exports.
//!
//! ```
//! use quadraui::prelude::*;
//! ```

// ── Runner (quadraui::{tui,gtk}::run) ───────────────────────────────────
pub use crate::runner::{AppLogic, Reaction, StepOutcome};

// ── Backend trait + capability query ────────────────────────────────────
pub use crate::backend::{Backend, BackendCaps};

// ── Events + geometry ────────────────────────────────────────────────────
pub use crate::event::{Key, MouseButton, NamedKey, Point, Rect, ScrollDelta, UiEvent, Viewport};

// ── Core value types every primitive is built from ──────────────────────
pub use crate::theme::Theme;
pub use crate::types::{Color, Icon, Modifiers, StyledSpan, StyledText, WidgetId};

// ── Hover/pressed state keyed by WidgetId ───────────────────────────────
pub use crate::interaction::InteractionState;

// ── ShellApp — the other runner (see docs/GUIDE.md "which runner?") ─────
pub use crate::shell::{ShellApp, ShellConfig, ShellContext};

// ── Declarative frame assembly (ScreenLayout::push(Surface::...) + draw)
// — the canonical path a `ShellApp::render_content` uses to paint more
// than one primitive into a single frame; see `frame.rs`'s module doc.
pub use crate::frame::{ScreenLayout, Surface};

// ── AppShell — the VS Code-style activity-bar/sidebar/editor frame.
// `AppShellLayout` is what `ShellApp::render_content` receives each
// frame (see the crate root `README.md` hello-world); `PanelDefinition`
// and `FormController`/`FormControllerEvent` are the other `compose`
// pieces a typical multi-pane app wires up alongside it.
pub use crate::compose::{AppShellLayout, FormController, FormControllerEvent, PanelDefinition};

// ── A representative sample of primitives (the crate root table's own
// "representative sample", kept in sync with it — see `lib.rs`'s module
// doc) — enough to build the two-pane app `docs/GUIDE.md` walks through.
// Every other primitive lives at `quadraui::<Name>` exactly as it always
// has; this list is deliberately not exhaustive.
pub use crate::primitives::form::{FieldKind, Form, FormField, FormLayout};
pub use crate::primitives::list::{ListItem, ListView, ListViewLayout};
pub use crate::primitives::panel::{Panel, PanelAction, PanelLayout};
pub use crate::primitives::split::{Split, SplitDirection, SplitLayout};
pub use crate::primitives::status_bar::{StatusBar, StatusBarLayout, StatusBarSegment};
pub use crate::primitives::text_display::{TextDisplay, TextDisplayLine};
pub use crate::primitives::text_input::{TextInput, TextInputLayout};
pub use crate::primitives::tree::{TreeRow, TreeView, TreeViewLayout};
