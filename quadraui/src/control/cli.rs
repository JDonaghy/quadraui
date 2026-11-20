//! `--json` CLI helper: runs one [`crate::AppLogic::run_command`] against
//! an app instance from argv and prints the JSON result to stdout — no
//! control channel, no event loop, and no backend at all. This is the
//! same "Level 2" named-command surface `quadraui-mcp`'s `tools/call`
//! routes [`crate::control::ControlRequest::Command`] to on a *running*
//! app; this helper runs it directly against an app the caller just
//! constructed, for a one-shot `app --json <command> <args>` invocation
//! that never opens a UI.
//!
//! ```no_run
//! # struct MyApp;
//! # impl quadraui::AppLogic for MyApp {
//! #     type AreaId = ();
//! #     fn render(&self, _backend: &mut dyn quadraui::Backend, _area: ()) {}
//! #     fn handle(&mut self, _event: quadraui::UiEvent, _backend: &mut dyn quadraui::Backend) -> quadraui::Reaction {
//! #         quadraui::Reaction::Continue
//! #     }
//! # }
//! // `my_app --json select '{"index": 1}'`
//! let args: Vec<String> = std::env::args().skip(1).collect();
//! if args.first().map(String::as_str) == Some("--json") {
//!     let mut app = MyApp;
//!     std::process::exit(quadraui::control::cli::run(&mut app, &args[1..]));
//! }
//! // ...otherwise fall through to the app's normal UI startup.
//! ```

use serde_json::Value;

use crate::runner::AppLogic;

/// Runs `argv` (everything after the leading `--json`, i.e.
/// `[<command>, <json-args>?]`) against `app.run_command`, prints the
/// result — or, on failure, the error message to stderr — as JSON, and
/// returns the process exit code a caller passes straight to
/// [`std::process::exit`]: `0` on success, `1` on a usage or command
/// error.
///
/// `argv[0]` is the command's name; `argv[1]`, if present, is parsed as
/// JSON and passed as the command's `args` (defaults to `{}` when
/// absent). `app` is driven directly on the calling thread — this
/// function never opens a control channel, an event loop, or a backend.
// `quadraui/src/lib.rs`'s crate-wide `deny(clippy::print_stdout,
// clippy::print_stderr)` exists so a stray print never lands in a host's
// live terminal UI; this call site is the opposite case — a `--json`
// invocation prints straight to a human (or a script) running the binary
// directly, with no UI ever opened, the same shape as that lint's other
// documented exceptions.
#[allow(clippy::print_stdout, clippy::print_stderr)]
pub fn run<A: AppLogic>(app: &mut A, argv: &[String]) -> i32 {
    match run_result(app, argv) {
        Ok(value) => {
            println!("{value}");
            0
        }
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}

/// [`run`]'s result, without the printing/exit-code wrapping — for a
/// caller that wants the JSON (or error message) itself, e.g. to compare
/// against what the same command/args answer over a live app's control
/// channel.
pub fn run_result<A: AppLogic>(app: &mut A, argv: &[String]) -> Result<Value, String> {
    let name = argv
        .first()
        .ok_or_else(|| "usage: --json <command> [json-args]".to_string())?;
    let args: Value = match argv.get(1) {
        Some(raw) => {
            serde_json::from_str(raw).map_err(|e| format!("invalid JSON args {raw:?}: {e}"))?
        }
        None => serde_json::json!({}),
    };
    app.run_command(name, args).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::control::{CommandError, CommandRegistry, CommandSpec};
    use crate::event::UiEvent;
    use crate::runner::Reaction;

    /// Mirrors `control::mod`'s own `CommandApp` test fixture: a selected
    /// index plus a count, driven entirely through `run_command` with no
    /// control channel at all.
    struct CommandApp {
        items: Vec<&'static str>,
        selected: usize,
        commands: CommandRegistry<CommandApp>,
    }

    impl CommandApp {
        fn new() -> Self {
            let mut commands = CommandRegistry::new();
            commands.register(
                "select",
                "Selects an item by index.",
                serde_json::json!({"type": "object"}),
                |app: &mut CommandApp, args: Value| {
                    let index = args
                        .get("index")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| CommandError::InvalidArgs("\"index\" required".into()))?
                        as usize;
                    app.selected = index;
                    Ok(serde_json::json!({"selected": index}))
                },
            );
            commands.register(
                "count",
                "Returns the number of items.",
                serde_json::json!({"type": "object"}),
                |app: &mut CommandApp, _args: Value| {
                    Ok(serde_json::json!({"count": app.items.len()}))
                },
            );
            Self {
                items: vec!["alpha", "beta", "gamma"],
                selected: 0,
                commands,
            }
        }
    }

    impl AppLogic for CommandApp {
        type AreaId = ();

        fn render(&self, _backend: &mut dyn Backend, _area: ()) {}

        fn handle(&mut self, _event: UiEvent, _backend: &mut dyn Backend) -> Reaction {
            Reaction::Continue
        }

        fn command_specs(&self) -> Vec<CommandSpec> {
            self.commands.specs()
        }

        fn run_command(&mut self, name: &str, args: Value) -> Result<Value, CommandError> {
            let mut commands = std::mem::take(&mut self.commands);
            let result = commands.run(self, name, args);
            self.commands = commands;
            result
        }
    }

    #[test]
    fn run_executes_the_named_command_and_mutates_the_app() {
        let mut app = CommandApp::new();
        let code = run(
            &mut app,
            &["select".to_string(), r#"{"index": 2}"#.to_string()],
        );
        assert_eq!(code, 0);
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn run_result_defaults_absent_args_to_an_empty_object() {
        let mut app = CommandApp::new();
        let value = run_result(&mut app, &["count".to_string()]).expect("count succeeds");
        assert_eq!(value, serde_json::json!({"count": 3}));
    }

    #[test]
    fn run_result_matches_what_the_control_channel_s_command_request_would_answer() {
        // The same command, same args, run through `run_command` directly
        // (what this CLI helper does) and through a fresh app of its own —
        // proving the two call sites `quadraui::control::ControlRequest::Command`
        // and this helper share answer identically for the same starting
        // state, not just by construction.
        let mut cli_app = CommandApp::new();
        let cli_value = run_result(
            &mut cli_app,
            &["select".to_string(), r#"{"index": 1}"#.to_string()],
        )
        .expect("cli path succeeds");

        let mut control_app = CommandApp::new();
        let control_value = control_app
            .run_command("select", serde_json::json!({"index": 1}))
            .expect("control path succeeds");

        assert_eq!(cli_value, control_value);
    }

    #[test]
    fn run_result_with_no_command_name_is_a_usage_error() {
        let mut app = CommandApp::new();
        let err = run_result(&mut app, &[]).expect_err("missing command name");
        assert!(err.contains("usage"));
    }

    #[test]
    fn run_result_with_invalid_json_args_is_a_clean_error() {
        let mut app = CommandApp::new();
        let err = run_result(&mut app, &["select".to_string(), "not json".to_string()])
            .expect_err("invalid JSON");
        assert!(err.contains("invalid JSON args"));
    }

    #[test]
    fn run_result_with_unknown_command_is_a_clean_error() {
        let mut app = CommandApp::new();
        let err = run_result(&mut app, &["bogus".to_string()]).expect_err("unknown command");
        assert!(err.contains("bogus"));
    }
}
