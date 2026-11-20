//! Level 2 app-level command registry (see [`crate::control`]'s "Levels"
//! doc section): named commands that act on an [`crate::AppLogic`] app's
//! own model rather than its widgets — a selection index, a document's
//! contents, a counter — not a specific widget instance.
//!
//! An app that wants Level 2 commands builds one [`CommandRegistry`],
//! [`CommandRegistry::register`]s each command against it, and returns
//! it from [`crate::AppLogic::command_specs`]/[`crate::AppLogic::run_command`]
//! (typically stashed as a field on the app itself, swapped out for the
//! duration of a [`CommandRegistry::run`] call so the registered
//! closures can take `&mut A` without aliasing the field that holds
//! them — see [`CommandRegistry::run`]'s doc for that exact dance).
//! [`crate::control::ControlPort::service`] calls `run_command` for a
//! [`crate::control::ControlRequest::Command`], and `quadraui-mcp` calls
//! `command_specs` to extend its `tools/list` past its four built-in
//! tools. [`crate::control::cli`] calls `run_command` directly, with no
//! control channel, no event loop, and no backend at all.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Why running (or looking up) a registered command failed.
///
/// `#[non_exhaustive]` so a future variant doesn't break a consumer's
/// `match`.
#[non_exhaustive]
#[derive(Debug)]
pub enum CommandError {
    /// No command is registered under this name.
    NotFound(String),
    /// `args` didn't match what this command expects — a free-form
    /// message, since each command validates its own JSON shape.
    InvalidArgs(String),
    /// The command ran but failed for a reason of its own.
    Failed(String),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::NotFound(name) => write!(f, "unknown command: {name}"),
            CommandError::InvalidArgs(msg) => write!(f, "invalid arguments: {msg}"),
            CommandError::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// One registered command's static shape: what an MCP `tools/list` entry
/// and the `--json` CLI helper's own listing need, independent of
/// actually running it. Returned by [`CommandRegistry::specs`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// The command's name — what a caller passes to
    /// [`crate::control::ControlHandle::command`], an MCP `tools/call`,
    /// or the CLI helper's `<command>` argv.
    pub name: String,
    /// A human/model-readable description of what it does.
    pub description: String,
    /// A JSON-schema a caller validates its `args` against before
    /// calling — the same shape an MCP tool's own `inputSchema` is.
    pub schema: Value,
}

/// The closure shape every registered command runs: mutate `app`'s own
/// model against `args`, answering the JSON result a caller sees.
type CommandFn<A> = dyn FnMut(&mut A, Value) -> Result<Value, CommandError> + Send;

struct RegisteredCommand<A> {
    spec: CommandSpec,
    run: Box<CommandFn<A>>,
}

/// A registry of named, app-defined commands that act on `A`'s own model
/// — see the module doc for how an app wires this into
/// [`crate::AppLogic::command_specs`]/[`crate::AppLogic::run_command`].
pub struct CommandRegistry<A> {
    commands: Vec<RegisteredCommand<A>>,
}

impl<A> Default for CommandRegistry<A> {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
        }
    }
}

impl<A> CommandRegistry<A> {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one command under `name`. `schema` is the JSON-schema a
    /// caller validates its `args` against (an MCP tool's `inputSchema`,
    /// the CLI helper's own listing); `run` executes it against the
    /// app's own model, answering the JSON result.
    ///
    /// Registering a second command under a name already registered
    /// shadows the first — [`Self::run`] finds the most recently
    /// registered match.
    pub fn register(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
        run: impl FnMut(&mut A, Value) -> Result<Value, CommandError> + Send + 'static,
    ) -> &mut Self {
        self.commands.push(RegisteredCommand {
            spec: CommandSpec {
                name: name.into(),
                description: description.into(),
                schema,
            },
            run: Box::new(run),
        });
        self
    }

    /// Every registered command's static descriptor, in registration
    /// order — what `quadraui-mcp`'s `tools/list` appends after its own
    /// four built-in tools, and what the CLI helper lists.
    pub fn specs(&self) -> Vec<CommandSpec> {
        self.commands.iter().map(|c| c.spec.clone()).collect()
    }

    /// Whether any command is registered.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Runs the most recently registered command named `name` against
    /// `app`, or [`CommandError::NotFound`] if nothing matches.
    ///
    /// Takes `app: &mut A` as a separate argument rather than storing it
    /// on the registry itself: a registry normally lives as a field on
    /// `A` (so a command's own closure can be `move`-captured once and
    /// still see `A`'s later mutations), and Rust's borrow checker can't
    /// see that a `&mut CommandRegistry<A>` borrowed out of one of `A`'s
    /// own fields is disjoint from the rest of `A`. The caller-side fix
    /// is the standard "swap the field out, call, swap it back" dance:
    ///
    /// ```ignore
    /// let mut registry = std::mem::take(&mut self.commands);
    /// let result = registry.run(self, name, args);
    /// self.commands = registry;
    /// result
    /// ```
    pub fn run(&mut self, app: &mut A, name: &str, args: Value) -> Result<Value, CommandError> {
        match self.commands.iter_mut().rev().find(|c| c.spec.name == name) {
            Some(cmd) => (cmd.run)(app, args),
            None => Err(CommandError::NotFound(name.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A trivial app: just a selected index and an item count, enough to
    /// exercise `select(index)`/`count()` without needing a real
    /// `AppLogic`/`Backend` at all — `CommandRegistry` itself has no
    /// dependency on either.
    #[derive(Default)]
    struct Model {
        items: Vec<&'static str>,
        selected: usize,
    }

    fn demo_registry() -> CommandRegistry<Model> {
        let mut registry = CommandRegistry::new();
        registry.register(
            "select",
            "Selects an item by index.",
            json!({"type": "object", "properties": {"index": {"type": "integer"}}}),
            |model: &mut Model, args: Value| {
                let index = args
                    .get("index")
                    .and_then(Value::as_u64)
                    .ok_or_else(|| CommandError::InvalidArgs("\"index\" is required".into()))?
                    as usize;
                if index >= model.items.len() {
                    return Err(CommandError::Failed(format!("index {index} out of range")));
                }
                model.selected = index;
                Ok(json!({"selected": index}))
            },
        );
        registry.register(
            "count",
            "Returns the number of items.",
            json!({"type": "object", "properties": {}}),
            |model: &mut Model, _args: Value| Ok(json!({"count": model.items.len()})),
        );
        registry
    }

    #[test]
    fn specs_lists_every_registered_command_in_order() {
        let registry = demo_registry();
        let names: Vec<String> = registry.specs().into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["select", "count"]);
    }

    #[test]
    fn run_mutates_the_app_and_answers_its_own_json() {
        let mut model = Model {
            items: vec!["alpha", "beta", "gamma"],
            selected: 0,
        };
        let mut registry = demo_registry();

        let reply = registry
            .run(&mut model, "select", json!({"index": 2}))
            .expect("select succeeds");
        assert_eq!(reply, json!({"selected": 2}));
        assert_eq!(model.selected, 2);

        let reply = registry
            .run(&mut model, "count", json!({}))
            .expect("count succeeds");
        assert_eq!(reply, json!({"count": 3}));
    }

    #[test]
    fn run_unknown_name_is_not_found() {
        let mut model = Model::default();
        let mut registry = demo_registry();
        let err = registry
            .run(&mut model, "bogus", json!({}))
            .expect_err("unknown command");
        assert!(matches!(err, CommandError::NotFound(name) if name == "bogus"));
    }

    #[test]
    fn run_propagates_the_command_s_own_error() {
        let mut model = Model {
            items: vec!["only"],
            selected: 0,
        };
        let mut registry = demo_registry();
        let err = registry
            .run(&mut model, "select", json!({"index": 9}))
            .expect_err("out of range");
        assert!(matches!(err, CommandError::Failed(_)));
    }

    #[test]
    fn a_later_registration_shadows_an_earlier_one_of_the_same_name() {
        let mut model = Model::default();
        let mut registry: CommandRegistry<Model> = CommandRegistry::new();
        registry.register("ping", "first", json!({}), |_m: &mut Model, _a: Value| {
            Ok(json!("first"))
        });
        registry.register("ping", "second", json!({}), |_m: &mut Model, _a: Value| {
            Ok(json!("second"))
        });
        assert_eq!(
            registry.run(&mut model, "ping", json!({})).unwrap(),
            json!("second")
        );
    }
}
