// Hand-maintained facade over the native napi-rs addon this package builds
// at `../index.js`/`../index.d.ts`/`../*.node` (via `npm run build`, which
// runs `napi build --platform --release` -- see package.json's `build`
// script). Those three are generated build artifacts, not checked in; this
// file is this package's actual `main`/`types` entry, so a consumer's
// import path never names the generated loader directly, and any future
// TS-only ergonomics (overloads, a narrower public type) have exactly one
// place to be added without touching the generated file.
//
// Runtime exports go through `require`/`exports` rather than `import`/
// `export`: Node's native TypeScript support determines a `.ts` file's
// module format the same way it would a `.js` file with this package's
// `"type": "commonjs"`, so top-level `import`/runtime `export` syntax here
// would be a hard parse error, not silently transpiled -- there is no
// `tsc`/bundler step between this file and the Node process that runs it.
// `export type` below is exempt: it is erasable (type-only, no runtime
// value), so it disappears entirely rather than needing ESM semantics.
const native: typeof import("../index.js") = require("../index.js");

/**
 * A headless quadraui app, driven entirely from JavaScript/TypeScript.
 *
 * Construct it with a `width`/`height` cell grid, insert descriptors with
 * `insertButton`/`insertTextDisplay`/.../`insertExtra`, and inspect the
 * rendered frame with `screen()`/`screenContains()`. See the native
 * binding's own doc comment (`quadraui-node/src/lib.rs`'s `QuiApp`) for the
 * full method list and panic/poison behaviour.
 */
export type QuiApp = InstanceType<typeof native.QuiApp>;
exports.QuiApp = native.QuiApp;

/**
 * A headless `ShellApp`/`ShellConfig`-backed app: an AppShell (activity bar
 * + sidebar + optional bottom-panel tab strip) around JS-registered
 * content and event handlers. Built by `QuiApp.runShell(width, height,
 * config, ...)`, not constructed directly. See the native binding's own
 * doc comment (`quadraui-node/src/lib.rs`'s `QuiShellApp`) for the full
 * method list.
 */
export type QuiShellApp = InstanceType<typeof native.QuiShellApp>;

/**
 * A thread-safe handle, returned by `QuiApp.waker()`, that a background
 * thread calls `post`/`wake`/`postAfter` on to deliver a value into the
 * owning `QuiApp`'s event stream. See the native binding's own doc comment
 * (`quadraui-node/src/lib.rs`'s `QuiAppWaker`) for the full method list.
 */
export type QuiAppWaker = InstanceType<typeof native.QuiAppWaker>;

/**
 * A handle returned by `pump()`: stops the `setInterval` loop it started.
 * Calling `stop()` more than once, or after the loop already stopped
 * itself (because `step()` reported the app exited), is a harmless no-op --
 * `clearInterval` on an already-cleared handle does nothing.
 */
export interface PumpHandle {
  stop(): void;
}

/**
 * Drives `app.step()` repeatedly on a timer, cooperatively yielding to the
 * rest of Node's event loop between calls instead of looping synchronously
 * -- the JS-side half of the non-blocking run-loop contract `startTui`/
 * `step` establish on the Rust side (see `quadraui-node/src/lib.rs`'s
 * module doc's "Live run loop" section). `step()` itself never blocks
 * (`startTui` moves this app onto a non-blocking `TuiRunner::step`, and the
 * headless path was always non-blocking), so a `setInterval` cadence here
 * never starves a concurrent timer/microtask/I/O callback the way a tight
 * synchronous loop calling `step()` forever would.
 *
 * Stops automatically once `step()` reports the app has exited (returns
 * `false`), or earlier if the returned handle's `stop()` is called.
 * `intervalMs` defaults to 16 (roughly 60Hz); pass a larger value to trade
 * input latency for CPU.
 */
function pump(app: { step(): boolean }, intervalMs = 16): PumpHandle {
  const handle = setInterval(() => {
    if (!app.step()) {
      clearInterval(handle);
    }
  }, intervalMs);
  return {
    stop(): void {
      clearInterval(handle);
    },
  };
}
exports.pump = pump;
