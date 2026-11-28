# Cross-language parity: one app, three languages, identical frames

quadraui says its Python and Node bindings work the same way as Rust. This
directory turns that into a test. It holds one small app, a todo list,
written once in each language:

| | App | Tracer |
|---|---|---|
| Rust | [`rust/src/main.rs`](rust/src/main.rs) | [`rust/src/bin/trace.rs`](rust/src/bin/trace.rs) |
| Python | [`python/todo.py`](python/todo.py) | [`python/trace.py`](python/trace.py) |
| Node (TSX) | [`node/todo.tsx`](node/todo.tsx) | [`node/trace.ts`](node/trace.ts) |

The app is a `New` text field, a list of todos, `Add` and `Clear` buttons and
a status bar. Typing edits the draft, and Enter or `Add` appends it.
Clicking a row selects it, and `Clear` empties the list. The script also
covers keyboard focus: a clicked button, the focused list and Tab back into
the form each paint their focus the same way in all three languages. Every update also
appends a line such as `edit Bu`, `add Tea` or `select 1` to the app's `log`,
the same vocabulary in every language.

Each tracer runs its app headless on the TUI backend at 40x12 cells and drives
it with [`script.json`](script.json): clicks at cell coordinates, typed text and
named keys. After the first paint and after every step it records the glyph,
foreground and background of every cell, plus the log lines that step added.
[`scripts/parity_check.py`](../../scripts/parity_check.py) runs all three and
fails on the first step where Rust or Node differs from Python. It prints both
frames side by side and lists every differing cell.

## Running it

```sh
scripts/parity_ci.sh                      # build the extension, the addon and the tracer, then check
scripts/parity_check.py --rust-bin PATH   # check only, with everything already built
scripts/parity_check.py --keep-going      # report every differing step, not just the first
```

CI runs `scripts/parity_ci.sh` in the `parity` job of
`.github/workflows/ci.yml` on Linux and uploads the three traces.

Running each app live: `cargo run -p quadraui-parity --features tui --bin todo`,
`python examples/parity/python/todo.py`, or load `node/todo.tsx` through
`examples/hero/node/load_jsx.ts` the way `node/trace.ts` does. Esc quits.

## Known divergences

Python and Node match exactly at every step. Rust does not yet, in one
place. [`known_divergences.json`](known_divergences.json) records each one
with the steps and the cell region where it shows. The check allows a
difference only there. If a listed step stops differing, the check fails until
that step is removed from the file, so a fix cannot go unnoticed.

| Id | What differs |
|---|---|
| `controlled-list-ignores-arrow-keys` | Down on a focused list whose selection the app pins moves the selection and fires `SelectionChanged` in Rust. Python and Node do nothing. |

## How the Rust version is built

It uses the Rust app layer (`quadraui::app`), the same shape as the Python
and Node versions: `column`/`row` with equal shares, `form`/`text_field`,
`list` and `status_bar` nodes whose handlers map events to a `Msg`, and one
`update`. The tracer wraps it in `Run`, the `AppLogic` adapter, to drive it
through `TuiDriver`. As in Python and Node, the form's focused field is left
to the runner.

## Limits of the comparison

- The bindings' `cell_colors` returns black for a terminal-default colour, so
  the Rust tracer maps those to black too. A default-vs-black difference
  would not show.
- Only glyph, fg and bg are compared. Bold, italic and underline are not,
  because the bindings do not expose them. A focused button's ring is an
  underline, so the check sees its focus through the accent-coloured
  brackets it also paints.
- Two clicks within 1.5 cells and 400ms fold into a double-click, measured on
  the wall clock. Python and Node cannot turn folding off, so `script.json`
  never puts two clicks that close in a row.
