# quadraui — North Star

> **The source of truth for *intent*.** Design rationale lives in
> `quadraui/docs/`; this file says what we are ultimately trying to be, and is
> meant to bias planning, triage and issue-filing above any single primitive or
> backend. Keep it short and current. The *order* the remaining work lands in —
> releases, cross-epic gates, the 1.0 bar — is [`ROADMAP.md`](ROADMAP.md).
>
> _Last updated: 2026-10-08._

## The goal

**Rival Electron's capability breadth for desktop applications — minus the
JavaScript runtime and the web rendering engine.**

Most teams reach for Electron not because they want a browser in their app, but
because it is the only option that reliably gives them *everything they need in
one box*: a window, native dialogs, clipboard, notifications, menus, tray, drag
and drop, a custom title bar, file associations. They pay for that convenience
with a ~100MB runtime, hundreds of MB of RSS, and startup measured in seconds —
for an app that renders a tree, a list and some text.

quadraui's bet is that **the capability breadth is the actual product, and the
web stack is incidental**. An app built on quadraui should get that same breadth
natively, and any new app should get it *for free* — the work we do closing one
consumer's gap is permanently available to every consumer after it.

## Capability parity is the yardstick; the implementation is not

`quadraui/docs/UI_CRATE_DESIGN.md` §2 says quadraui is **"not a web-tech shim
like Tauri or Electron. No WebView. No HTML/CSS. No JavaScript."** That remains
true and is not in tension with this file — but read alone it invites the wrong
conclusion, that Electron is irrelevant to us. It is not.

Be precise about which half is rejected:

- **Rejected — the implementation.** WebView, HTML/CSS, JavaScript, a bundled
  browser, the memory and startup cost that comes with them. Also rejected:
  Electron's *web-only* surfaces (`webContents`, `session`, cookies, the
  renderer/main IPC split). Those are not gaps; they are things we do not want.
- **Adopted — the capability bar.** What an app developer can *do* from
  Electron's main process is a fair benchmark for what they should be able to do
  from quadraui, because that is the breadth that made Electron the default
  choice in the first place.

**The operational consequence:** *"Electron exposes this and quadraui does not"*
is, on its own, a legitimate basis for an issue. No further justification needed
beyond checking the capability is not web-specific. That reasoning is not
currently licensed anywhere in the docs, which is why this file exists.

## This is already what we have been building

The platform surface has been converging on Electron's shape without being named
as such. `PlatformServices` (`quadraui/src/backend.rs:3016`) today offers
clipboard, file open / save / folder dialogs, a message dialog, notifications and
`open_url` — very nearly a one-to-one map onto Electron's `dialog`, `clipboard`,
`Notification` and `shell.openExternal`.

The recent macOS run reads the same way once you look at it through this lens:

| quadraui | Electron equivalent |
|---|---|
| `register_font_from_memory` / `set_nerd_font_fallback` (#929) | `@font-face` / in-process font registration |
| `show_folder_open_dialog` (#935) | `dialog.showOpenDialog({properties:['openDirectory']})` |
| `show_message_dialog` (#936) | `dialog.showMessageBox` |
| client-side titlebar (#947) | `BrowserWindow({frame:false, titleBarStyle:'hiddenInset'})` |
| OS file drop (#834) | HTML5 drag-and-drop |
| `Backend::waker` (#831) | IPC from a background thread to the UI |
| `begin_window_drag` (#498) | `-webkit-app-region: drag` |

None of these were filed as "Electron parity" — they were filed as individual
defects found by a consumer. That is the forcing function working, but it is
also *reactive*: it only finds the gaps vimcode happens to walk into. A
deliberate audit against the benchmark would find the rest before a consumer
does.

## The visual target: VS Code on GUI, graceful degradation on the terminal

Capability breadth is half of why teams choose Electron; the other half is that
the result **looks like a modern desktop app**. A developer evaluating quadraui
will judge it by its GUI screenshots first. Today too many GUI widgets read as a
terminal program lifted onto a pixel canvas: monospace chrome, `[value]` inputs,
`[x]` toggles, text-glyph icons, sizes snapped to whole text lines. That is the
wrong way round.

- **VS Code is the reference for the GUI backends.** When a look-and-feel
  question comes up, compare against VS Code on the same display. Default
  density: activity bar 48 px, tab bar 35 px, status bar 22 px, list and tree
  rows about 22 px. Chrome uses the platform UI font. Icons are codicons.
  Overlays such as menus, hovers, the palette, toasts and dialogs have radius
  and elevation.
- **The terminal is the graceful degradation of that design, not its source.**
  A GUI painter must not draw terminal affordances (`[`, `]`, `[x]`, `─`, `▶`)
  as text, or size chrome in character cells. The TUI painter renders the same
  descriptor in the best way a terminal can: style tokens it cannot honour are
  ignored, transitions snap to their end state, and icons fall back to Nerd
  Font or ASCII. The portability commitment below still holds: it must
  *function*, it need not *look the same*.
- **Monospace is a property of the content, not the framework.** Only the
  editor-class primitives are monospace by intent: Editor, Terminal, DiffView,
  Minimap and CommandLine. Everything else is chrome and uses the UI font.
- **Chrome motion is in scope; a general animation framework is not.** Hover
  and press fades, toast slide-in, smooth scrolling and caret blink are expected
  on GUI. Tweening arbitrary app properties is not.
- **The widgets an app developer expects are in scope.** That means Button,
  radio, switch, a scroll container, number input, editable combo, date picker
  and multi-select. "App framework" is not credible without them, even though
  catalogue breadth for its own sake still is not the goal.

This is tracked as the GUI-look epic (#1371). The breaking half (px-first
sizing, f32 scroll offsets, `Default` + `#[non_exhaustive]` descriptors) rides
the v0.2 batch (#1095).

## What this does not change

- **The four-backend portability commitment stands, and TUI must actually
  work.** Anything a user interacts with — menus, file and folder pickers,
  dialogs, buttons, text boxes — has to *function* on TUI, not merely report its
  absence honestly. It need not look like the GUI. vimcode's TUI build is the
  existence proof: same `AppShell`, same activity bar, same omnibar.
  `Unsupported` is reserved for what is *physically* absent on a terminal (a
  tray icon, a dock badge, an OS global shortcut) — and the crate owns the
  degrade, in `compose/`, rather than telling each app to rebuild it. Electron
  parity is not a licence to add GUI-only surfaces. See CLAUDE.md's
  *Cross-backend portability commitment*.
- **The non-goals in `UI_CRATE_DESIGN.md` §2 stand** — not a general-purpose GUI
  framework, not a pixel-perfect renderer, not retained-mode, not a general
  animation framework. Breadth of *platform capability* is the target. Breadth of
  *widget catalogue* for its own sake is not, but the everyday app widgets above
  are.
- **vimcode stays the testbed, not the product.** It is the forcing function for
  these gaps. See code-coordinator's `GOAL.md`.
