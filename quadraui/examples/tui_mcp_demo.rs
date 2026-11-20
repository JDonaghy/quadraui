//! Reference example for `quadraui-mcp`'s module doc, "Driving a quadraui
//! app from an agent": a three-pane app (list, detail, text) that starts
//! a real `quadraui-mcp` server on a Unix socket alongside its normal
//! interactive TUI, so an agent can attach from a second process while a
//! person watches the same live terminal.
//!
//! - `cargo run --example tui_mcp_demo --features tui,control` — the
//!   normal interactive TUI: Up/Down moves the list selection, which
//!   updates the detail pane; `q`/Esc quits. On startup it also binds a
//!   Unix-domain socket (`$QUADRAUI_MCP`, or a per-process path under
//!   the OS temp dir if unset — printed to stderr before the TUI takes
//!   over the screen) and serves `quadraui-mcp` on it in the background.
//! - An agent attaches to that socket from a second process — see
//!   `quadraui_mcp`'s crate doc for the Claude Code config snippet and
//!   the full `initialize` → `tools/list` → `tools/call` walkthrough.
//!   `snapshot`/`act`/`key`/`type` (Level 1, built on `quadraui::a11y`)
//!   work immediately; `filter`/`clear_filter` (Level 2, this file's own
//!   [`CommandRegistry`]) are the two app-defined commands this demo
//!   registers.
//!
//! This file is the pattern other apps copy: attach the control channel
//! via [`quadraui::tui::TuiRunner::attach_control`], hand the resulting
//! [`quadraui::control::ControlHandle`] to [`quadraui_mcp::serve`] on a
//! background thread, and keep pumping the runner on the calling thread
//! exactly like [`quadraui::tui::run`] does internally — see that
//! function's own source for the loop this file's `main` mirrors.

use quadraui::control::{CommandError, CommandRegistry, CommandSpec};
use quadraui::{
    AppLogic, Backend, Decoration, Key, ListItem, ListView, ListViewEvent, NamedKey, Reaction,
    Rect, ScreenLayout, StyledSpan, StyledText, Surface, TextDisplay, TextDisplayLine, UiEvent,
    WidgetId,
};
use serde_json::{json, Value};

/// One row of the demo's fixed catalogue: a name the list pane shows, and
/// a longer description the detail pane shows once that row is selected.
struct Item {
    name: &'static str,
    detail: &'static str,
}

/// The whole catalogue `filter`/`clear_filter` search over. Deliberately
/// small and fixed — this demo is about the control plane, not about
/// being a realistic data browser.
const ITEMS: &[Item] = &[
    Item {
        name: "apple",
        detail: "A crisp, sweet fruit that keeps well in cold storage.",
    },
    Item {
        name: "banana",
        detail: "A curved yellow fruit, high in potassium.",
    },
    Item {
        name: "cherry",
        detail: "A small stone fruit, often paired with desserts.",
    },
    Item {
        name: "date",
        detail: "A deeply sweet fruit that dries well for snacking.",
    },
    Item {
        name: "fig",
        detail: "A soft fruit with edible seeds throughout its flesh.",
    },
];

/// The demo app: a filterable list over [`ITEMS`], a detail pane that
/// shows the selected row's description, and a static notes pane — the
/// "list, detail, text" three-pane shape the module doc describes.
pub struct McpDemo {
    /// Indices into [`ITEMS`] currently matching `filter_query`, in
    /// catalogue order.
    filtered: Vec<usize>,
    /// Index into `filtered`, not into [`ITEMS`] directly.
    selected: usize,
    filter_query: String,
    commands: CommandRegistry<McpDemo>,
}

impl McpDemo {
    pub fn new() -> Self {
        let mut commands = CommandRegistry::new();
        commands.register(
            "filter",
            "Filters the list to items whose name contains the given \
             case-insensitive substring, and resets the selection to the \
             first match.",
            json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"],
            }),
            |app: &mut McpDemo, args: Value| {
                let query = args
                    .get("query")
                    .and_then(Value::as_str)
                    .ok_or_else(|| CommandError::InvalidArgs("\"query\" is required".into()))?;
                app.filter_query = query.to_string();
                app.recompute_filter();
                Ok(json!({"matches": app.filtered.len()}))
            },
        );
        commands.register(
            "clear_filter",
            "Clears any active filter, restoring the full catalogue.",
            json!({"type": "object", "properties": {}}),
            |app: &mut McpDemo, _args: Value| {
                app.filter_query.clear();
                app.recompute_filter();
                Ok(json!({"matches": app.filtered.len()}))
            },
        );
        let mut app = Self {
            filtered: Vec::new(),
            selected: 0,
            filter_query: String::new(),
            commands,
        };
        app.recompute_filter();
        app
    }

    /// Rebuilds `filtered` from `filter_query` against [`ITEMS`] (an empty
    /// query matches everything) and snaps the selection back to the
    /// first match — the same "filtering moves the selection" behaviour a
    /// search box gives a person typing into it.
    fn recompute_filter(&mut self) {
        let query = self.filter_query.to_lowercase();
        self.filtered = ITEMS
            .iter()
            .enumerate()
            .filter(|(_, item)| query.is_empty() || item.name.to_lowercase().contains(&query))
            .map(|(idx, _)| idx)
            .collect();
        self.selected = 0;
    }

    fn selected_item(&self) -> Option<&'static Item> {
        self.filtered.get(self.selected).map(|&idx| &ITEMS[idx])
    }
}

impl Default for McpDemo {
    fn default() -> Self {
        Self::new()
    }
}

/// One line of plain, unstyled text — the same shape every
/// `TextDisplay` pane below is built from.
fn plain_line(text: impl Into<String>) -> TextDisplayLine {
    TextDisplayLine {
        spans: vec![StyledSpan::plain(text)],
        decoration: Decoration::default(),
        timestamp: None,
    }
}

impl AppLogic for McpDemo {
    type AreaId = ();

    fn render(&self, backend: &mut dyn Backend, _area: ()) {
        let vp = backend.viewport();
        let list_w = (vp.width * 0.3).max(16.0);
        let detail_w = (vp.width * 0.4).max(16.0);
        let text_w = (vp.width - list_w - detail_w).max(16.0);

        let mut layout = ScreenLayout::new();
        layout.push(Surface::List {
            rect: Rect::new(0.0, 0.0, list_w, vp.height),
            list: ListView {
                id: WidgetId::new("items"),
                title: Some(StyledText::plain("Items")),
                items: self
                    .filtered
                    .iter()
                    .map(|&idx| ListItem {
                        text: StyledText::plain(ITEMS[idx].name),
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

        let detail_lines = match self.selected_item() {
            Some(item) => vec![plain_line(item.detail)],
            None => vec![plain_line("No items match the current filter.")],
        };
        layout.push(Surface::TextDisplay {
            rect: Rect::new(list_w, 0.0, detail_w, vp.height),
            td: TextDisplay {
                lines: detail_lines,
                title: Some(StyledText::plain("Detail")),
                ..TextDisplay::new(WidgetId::new("detail"))
            },
        });

        layout.push(Surface::TextDisplay {
            rect: Rect::new(list_w + detail_w, 0.0, text_w, vp.height),
            td: TextDisplay {
                lines: vec![
                    plain_line("Up/Down select, q/Esc quit."),
                    plain_line("An agent can drive this app over MCP:"),
                    plain_line("- snapshot/act/key/type (Level 1, UI)"),
                    plain_line("- filter/clear_filter (Level 2, model)"),
                ],
                title: Some(StyledText::plain("Notes")),
                ..TextDisplay::new(WidgetId::new("notes"))
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
                if self.selected + 1 < self.filtered.len() {
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

/// Where this demo serves `quadraui-mcp`: `$QUADRAUI_MCP` if set and
/// parseable (see [`quadraui_mcp::Transport::parse`] for the accepted
/// spec strings), otherwise a per-process Unix-domain socket under the
/// OS temp dir so the demo works with zero configuration.
fn mcp_transport() -> quadraui_mcp::Transport {
    std::env::var("QUADRAUI_MCP")
        .ok()
        .and_then(|spec| quadraui_mcp::Transport::parse(&spec))
        .unwrap_or_else(|| {
            quadraui_mcp::Transport::Unix(
                std::env::temp_dir().join(format!("quadraui-mcp-demo-{}.sock", std::process::id())),
            )
        })
}

fn main() -> std::io::Result<()> {
    let mut runner = quadraui::tui::TuiRunner::new(McpDemo::new())?;
    let handle = runner.attach_control();

    let transport = mcp_transport();
    eprintln!("quadraui-mcp listening on {transport:?}");
    std::thread::spawn(move || {
        if let Err(e) = quadraui_mcp::serve(handle, transport) {
            eprintln!("quadraui-mcp server exited: {e}");
        }
    });

    loop {
        match runner.pump(std::time::Duration::from_millis(50))? {
            quadraui::tui::StepOutcome::Continue => {}
            quadraui::tui::StepOutcome::Exited => break,
            _ => {}
        }
    }
    Ok(())
}
