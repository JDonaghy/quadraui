// Black-box coverage for `QuiApp.startNative()`'s `macos` arm: opens a real
// AppKit window via `quadraui::macos::MacRunner`, drives it through the
// same non-blocking `step()`/`pump()` cadence `tests/live_runner.test.ts`
// covers for the headless/`startTui` paths and `tests/live_runner_gtk.test.ts`
// covers for `gtk`, and tears it down deterministically rather than by a
// human -- see `quadraui-node/src/lib.rs`'s module doc's "Live run loop"
// section for the overall design.
//
// Only runs when the addon is built with the `macos` feature (and only
// does anything real on a macOS host -- `nativeBackend()` reports `null`
// for an addon built with `macos` enabled on a non-Apple `target_os`);
// skips (rather than fails) otherwise. Every scenario below shells out to
// its own standalone script rather than opening a `MacRunner` in this test
// process directly -- not because a second `MacRunner` in the same process
// is unsafe the way a second `GtkRunner` is (`NSApplication.sharedApplication()`
// is idempotent, unlike `GApplication::register()`), but so a crash or a
// lingering real window from one scenario can never bleed into the next,
// mirroring `tests/live_runner_gtk.test.ts`'s own isolation.
//
// Run against a build with the `macos` feature enabled:
//
//     npx napi build --platform --features macos
//     node --test tests/live_runner_macos.test.ts
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { QuiApp } = require("../js/index.ts");

const nativeBackend: string | null = QuiApp.nativeBackend();
const skip =
  nativeBackend !== "macos" &&
  `extension not built with the macos feature (nativeBackend() reported ${JSON.stringify(nativeBackend)})`;

/**
 * Runs `body` as its own standalone Node process (via `node -e`, not a
 * script file path argument) against a fresh process -- see this file's
 * own header doc for why a real `MacRunner` session gets one process per
 * scenario. Mirrors `tests/live_runner_gtk.test.ts`'s own `runGtkScript`,
 * except for *how* the script reaches that process: AppKit's own
 * `NSApplication` treats a lone existing-file path argument on `argv` as
 * a legacy "open this document" request and delivers it as a
 * `UiEvent::OpenRequested` the instant the window opens -- a `node
 * /path/to/script.js` invocation trips that the same way double-clicking
 * a document would, which would otherwise race every scenario below that
 * waits on "the next event this session delivers". `-e` carries the body
 * as a flag value instead of a file path, so no such argument is ever on
 * `argv` for AppKit to misread.
 */
function runMacScript(body: string): { stdout: string; stderr: string; status: number | null } {
  const result = spawnSync(process.execPath, ["-e", body], {
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    timeout: 30_000,
  });
  return { stdout: result.stdout ?? "", stderr: result.stderr ?? "", status: result.status };
}

const indexPath = require.resolve("../js/index.ts");

test(
  "startNative opens a real window, steps it, and tears it down via stopLive()",
  { skip },
  () => {
    const body = `
      const { QuiApp } = require(${JSON.stringify(indexPath)});
      const app = new QuiApp(10, 5);
      app.insertButton({ id: "demo:ok", label: "OK", focused: true });
      app.startNative();
      console.log("STEP1:" + app.step());
      console.log("STEP2:" + app.step());
      console.log("STOP_LIVE:" + app.stopLive());
      try {
        app.step();
        console.log("STEP_AFTER_STOP_DID_NOT_RAISE");
      } catch (err) {
        console.log("STEP_AFTER_STOP_RAISED:" + err.message);
      }
    `;
    const { stdout, status } = runMacScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /STEP1:true/);
    assert.match(stdout, /STEP2:true/);
    assert.match(stdout, /STOP_LIVE:true/);
    // `stopLive` is sticky (`live_exited`): the session this app once held
    // has ended for good, so a further `step()` must raise rather than
    // silently ticking an orphaned driver.
    assert.match(stdout, /STEP_AFTER_STOP_RAISED:LiveRunnerExitedError/);
  },
);

test(
  "a second startNative in a fresh QuiApp works after an earlier session in the same process has ended",
  { skip },
  () => {
    // Unlike `GApplication::register()`, `NSApplication.sharedApplication()`
    // is idempotent, so nothing here needs the per-scenario process
    // isolation this file's own header doc explains for the *why it's
    // still done this way* -- this scenario instead exercises the thing
    // that isolation would otherwise hide: that a *second* live `macos`
    // session really does work in a process that already tore one down.
    const body = `
      const { QuiApp } = require(${JSON.stringify(indexPath)});
      const first = new QuiApp(10, 5);
      first.startNative();
      first.step();
      console.log("FIRST_STOP:" + first.stopLive());
      const second = new QuiApp(10, 5);
      second.startNative();
      console.log("SECOND_STEP:" + second.step());
      console.log("SECOND_STOP:" + second.stopLive());
    `;
    const { stdout, status } = runMacScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /FIRST_STOP:true/);
    assert.match(stdout, /SECOND_STEP:true/);
    assert.match(stdout, /SECOND_STOP:true/);
  },
);

test(
  "startNative off the main thread raises a typed error instead of aborting the process",
  { skip },
  () => {
    // `MacRunner::new_with` requires `MainThreadMarker::new()`; a Node
    // worker thread is not Node's main thread, so this exercises the
    // `objc2::MainThreadMarker::new().is_none()` guard in `start_native`'s
    // `macos` arm -- without it, AppKit itself would `panic!`/abort the
    // whole process rather than let this call fail gracefully.
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-live-runner-macos-"));
    try {
      const workerScript = path.join(dir, "worker.js");
      fs.writeFileSync(
        workerScript,
        `
        const { QuiApp } = require(${JSON.stringify(indexPath)});
        const { parentPort } = require("node:worker_threads");
        const app = new QuiApp(10, 5);
        try {
          app.startNative();
          parentPort.postMessage({ raised: false });
        } catch (err) {
          parentPort.postMessage({ raised: true, message: err.message });
        }
        `,
      );
      const mainScript = path.join(dir, "main.js");
      fs.writeFileSync(
        mainScript,
        `
        const { Worker } = require("node:worker_threads");
        const worker = new Worker(${JSON.stringify(workerScript)});
        worker.on("message", (msg) => {
          console.log("MESSAGE:" + JSON.stringify(msg));
          worker.terminate();
        });
        worker.on("error", (err) => {
          console.log("WORKER_ERROR:" + err.message);
        });
        `,
      );
      const result = spawnSync(process.execPath, [mainScript], {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
        timeout: 30_000,
      });
      assert.equal(result.status, 0, `script did not exit cleanly: ${result.stdout}${result.stderr}`);
      // `internal_error`'s own `Error::from_reason` formats the type name
      // straight into `message` (there is no separate typed-error field
      // on the napi-rs side) -- mirrors every other `*Error:`-prefixed
      // assertion in this file and `tests/live_runner_gtk.test.ts`.
      assert.match(
        result.stdout,
        /MESSAGE:\{"raised":true,"message":"InternalError: startNative must be called from the main thread/,
      );
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  },
);

test(
  "a waker().postAfter() delivered from a spawned background thread reaches the handler while live",
  { skip },
  () => {
    // `postAfter` spawns its own `std::thread` on the Rust side (see
    // `QuiAppWaker::post_after`'s own doc) that sleeps, then calls the
    // same `post` path a foreground caller would -- the "spawned Rust
    // thread" delivery path this issue's own acceptance criteria names,
    // exercised here against a live `macos` session rather than the
    // headless one `tests/live_runner.test.ts`'s own `postAfter` test
    // covers. Reaching the handler at all depends on `start_native`'s
    // `macos` arm having swapped `QuiApp.current_waker` onto the waker
    // `NodeAppLogic::setup` captured off the live `MacBackend` -- without
    // that swap this would only ever post into the dormant headless
    // queue and `received` would stay empty until the deadline.
    const body = `
      const { QuiApp, pump } = require(${JSON.stringify(indexPath)});
      const app = new QuiApp(10, 5);
      const received = [];
      app.setHandler((event) => received.push(event));
      app.startNative();
      const handle = pump(app, 5);
      app.waker().postAfter(10, "from-background-thread");
      const deadline = Date.now() + 5_000;
      function poll() {
        if (received.length > 0 || Date.now() > deadline) {
          handle.stop();
          console.log("RECEIVED:" + JSON.stringify(received));
          console.log("STOP_LIVE:" + app.stopLive());
          return;
        }
        setTimeout(poll, 5);
      }
      poll();
    `;
    const { stdout, status } = runMacScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /RECEIVED:\[\{"User":"from-background-thread"\}\]/);
    assert.match(stdout, /STOP_LIVE:true/);
  },
);

test(
  "pump(app) via setInterval does not starve a concurrent setTimeout while a macos session is live",
  { skip },
  () => {
    const body = `
      const { QuiApp, pump } = require(${JSON.stringify(indexPath)});
      const app = new QuiApp(8, 3);
      app.startNative();
      const handle = pump(app, 1);
      const deadline = Date.now() + 5_000;
      let timeoutFired = false;
      setTimeout(() => {
        timeoutFired = true;
      }, 20);
      function poll() {
        if (timeoutFired || Date.now() > deadline) {
          handle.stop();
          console.log("TIMEOUT_FIRED:" + timeoutFired);
          console.log("STOP_LIVE:" + app.stopLive());
          return;
        }
        setTimeout(poll, 5);
      }
      poll();
    `;
    const { stdout, status } = runMacScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(
      stdout,
      /TIMEOUT_FIRED:true/,
      "setTimeout never fired -- pumping a live macos session starved the event loop",
    );
    assert.match(stdout, /STOP_LIVE:true/);
  },
);

test(
  "the app's own tick handler can request an exit, torn down without stopLive()",
  { skip },
  () => {
    // `NativeExitPolicy::tick`/`::handle` downgrade any non-`WindowClose`
    // `Reaction::Exit` to a sticky `exit_requested` flag on the `macos`
    // feature without `gtk` also enabled, rather than letting it reach
    // AppKit's own `[NSApp terminate:]` and end the whole process before
    // this session could -- see that type's own doc. `LiveRunner::step`
    // folds that flag into the returned `StepOutcome`, so this exercises
    // that an app-requested exit (not `stopLive()`) is what actually
    // closes the window and flips `step()` over to raising.
    const body = `
      const { QuiApp, pump } = require(${JSON.stringify(indexPath)});
      const app = new QuiApp(10, 5);
      let ticks = 0;
      app.setTickHandler(() => {
        ticks += 1;
        return ticks >= 2 ? false : undefined;
      });
      app.startNative();
      const handle = pump(app, 10);
      const deadline = Date.now() + 5_000;
      function poll() {
        if (ticks >= 2 || Date.now() > deadline) {
          handle.stop();
          console.log("TICKS:" + ticks);
          try {
            app.step();
            console.log("STEP_AFTER_EXIT_DID_NOT_RAISE");
          } catch (err) {
            console.log("STEP_AFTER_EXIT_RAISED:" + err.message);
          }
          return;
        }
        setTimeout(poll, 10);
      }
      poll();
    `;
    const { stdout, status } = runMacScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /TICKS:2/);
    assert.match(
      stdout,
      /STEP_AFTER_EXIT_RAISED:LiveRunnerExitedError/,
      "an app-requested exit must have already ended this session",
    );
  },
);
