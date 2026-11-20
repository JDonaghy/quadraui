//! Demonstrates "Level 2" app-level named commands (`quadraui::control`'s
//! "Levels" doc section): `select(index)` and `count()` registered
//! against a [`quadraui::control::CommandRegistry`] stashed on the app
//! itself, wired into [`quadraui::AppLogic::command_specs`]/
//! [`quadraui::AppLogic::run_command`]. The same `CommandsDemo` app runs
//! two ways with zero code difference between them — this file's whole
//! job is proving that:
//!
//! - `cargo run --example tui_commands_demo --features tui,control` —
//!   the normal interactive TUI (Up/Down to move the selection, `q`/Esc
//!   to quit), going through [`quadraui::tui::run`] exactly like any
//!   other example (see `quadraui::runner`'s module doc).
//! - `cargo run --example tui_commands_demo --features tui,control --
//!   --json select '{"index": 1}'` — the `--json` CLI helper
//!   ([`quadraui::control::cli::run`]), which runs one command against a
//!   fresh instance and prints its JSON result, with no UI, no event
//!   loop, and no backend at all.
//! - `cargo run --example tui_commands_demo --features tui,control --
//!   --json count` — same helper, the other registered command.
//!
//! A real AI agent drives a *live* instance of an app like this one over
//! MCP instead of a fresh one: `quadraui-mcp`'s `tools/list`/`tools/call`
//! route to the same two commands through `command_specs`/`run_command`
//! once the app calls `TuiRunner::attach_control` and hands the
//! resulting `ControlHandle` to `quadraui_mcp::serve`. That wiring isn't
//! shown in this file — `quadraui`'s own examples can't depend on
//! `quadraui-mcp`, which depends on `quadraui` — see
//! `quadraui-mcp/tests/mcp_tui_roundtrip.rs` for that end-to-end path
//! against a headless driver instead.

use quadraui::control::{CommandError, CommandRegistry, CommandSpec};
use quadraui::{
    Backend, Decoration, Key, ListItem, ListView, ListViewEvent, NamedKey, Reaction, Rect,
    ScreenLayout, StyledText, Surface, UiEvent, WidgetId,
};
use serde_json::{json, Value};

/// The demo app: a four-row list whose selected index is the whole
/// model, reachable from the UI (Up/Down), from a live control channel
/// (`quadraui-mcp`'s `tools/call`) and from the `--json` CLI helper.
pub struct CommandsDemo {
    items: Vec<&'static str>,
    selected: usize,
    commands: CommandRegistry<CommandsDemo>,
}

impl CommandsDemo {
    /// A fresh instance with `select`/`count` registered and row 0
    /// selected.
    pub fn new() -> Self {
        let mut commands = CommandRegistry::new();
        commands.register(
            "select",
            "Selects an item by index.",
            json!({
                "type": "object",
                "properties": {"index": {"type": "integer"}},
                "required": ["index"],
            }),
            |app: &mut CommandsDemo, args: Value| {
                let index = args
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| CommandError::InvalidArgs("\"index\" is required".into()))?
                    as usize;
                if index >= app.items.len() {
                    return Err(CommandError::Failed(format!("index {index} out of range")));
                }
                app.selected = index;
                Ok(json!({"selected": index}))
            },
        );
        commands.register(
            "count",
            "Returns the number of items.",
            json!({"type": "object", "properties": {}}),
            |app: &mut CommandsDemo, _args: Value| Ok(json!({"count": app.items.len()})),
        );
        Self {
            items: vec!["alpha", "beta", "gamma", "delta"],
            selected: 0,
            commands,
        }
    }
}

impl Default for CommandsDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl quadraui::AppLogic for CommandsDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let mut layout = ScreenLayout::new();
        layout.push(Surface::List {
            rect: Rect::new(0.0, 0.0, 34.0, 10.0),
            list: ListView {
                id: WidgetId::new("rows"),
                title: Some(StyledText::plain("Level 2 commands — Up/Down, q to quit")),
                items: self
                    .items
                    .iter()
                    .map(|text| ListItem {
                        text: StyledText::plain(*text),
                        icon: None,
                        detail: None,
                        decoration: Decoration::default(),
                    })
                    .collect(),
                selected_idx: self.selected,
                scroll_offset: 0,
                has_focus: true,
                bordered: true,
                h_scroll: 0,
                max_content_width: None,
                show_v_scrollbar: false,
            },
        });
        layout.draw(backend);
    }

    fn handle(&mut self, event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
        match event {
            UiEvent::List(_, ListViewEvent::ItemActivated { idx }) => {
                self.selected = idx;
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Char('q'),
                ..
            }
            | UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Escape),
                ..
            } => Reaction::Exit,
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Down),
                ..
            } => {
                if self.selected + 1 < self.items.len() {
                    self.selected += 1;
                }
                Reaction::Redraw
            }
            UiEvent::KeyPressed {
                key: Key::Named(NamedKey::Up),
                ..
            } => {
                self.selected = self.selected.saturating_sub(1);
                Reaction::Redraw
            }
            _ => Reaction::Continue,
        }
    }

    fn command_specs(&self) -> Vec<CommandSpec> {
        self.commands.specs()
    }

    fn run_command(&mut self, name: &str, args: Value) -> Result<Value, CommandError> {
        // `CommandRegistry::run`'s documented dance: swap the registry
        // out so its closures can take `&mut self` without aliasing the
        // field that holds them, then swap it back.
        let mut commands = std::mem::take(&mut self.commands);
        let result = commands.run(self, name, args);
        self.commands = commands;
        result
    }
}

fn main() -> std::io::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("--json") {
        std::process::exit(quadraui::control::cli::run(
            &mut CommandsDemo::new(),
            &argv[1..],
        ));
    }
    quadraui::tui::run(CommandsDemo::new())
}
