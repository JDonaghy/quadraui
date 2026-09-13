# quadraui — North Star

> **The source of truth for *intent*.** Design rationale lives in
> `quadraui/docs/`; this file says what we are ultimately trying to be, and is
> meant to bias planning, triage and issue-filing above any single primitive or
> backend. Keep it short and current.
>
> _Last updated: 2026-09-13._

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

## What this does not change

- **The four-backend portability commitment stands.** A capability that cannot
  be expressed on TUI degrades honestly (`BackendCaps`, defaulted no-op trait
  methods per rule 7) rather than being refused. Electron parity is not a licence
  to add GUI-only surfaces without a degradation story.
- **The non-goals in `UI_CRATE_DESIGN.md` §2 stand** — not a general-purpose GUI
  framework, not a pixel-perfect renderer, not retained-mode, not an animation
  framework. Breadth of *platform capability* is the target; breadth of *widget
  catalogue* is not.
- **vimcode stays the testbed, not the product.** It is the forcing function for
  these gaps. See code-coordinator's `GOAL.md`.
