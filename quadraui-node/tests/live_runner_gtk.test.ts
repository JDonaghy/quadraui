// Black-box coverage for `QuiApp.startNative()`'s `gtk` arm: opens a real
// GTK window via `quadraui::gtk::GtkRunner`, drives it through the same
// non-blocking `step()`/`pump()` cadence `tests/live_runner.test.ts` covers
// for the headless/`startTui` paths, and tears it down deterministically
// rather than by a human -- see `quadraui-node/src/lib.rs`'s module doc's
// "Live run loop" section for the overall design.
//
// Only runs when the addon is built with the `gtk` feature; skips
// (rather than fails) otherwise. Every scenario below shells out to its
// own standalone script rather than opening a `GtkRunner` in this test
// process directly: GTK's own `Application::register()`/`gtk_init()` are
// one-time, process-global init (see `quadraui::gtk::display_available`'s
// own doc), so a second `GtkRunner` built in a process that already tore
// one down hits `Gtk-CRITICAL **: New application windows must be added
// after the GApplication::startup signal has been emitted` and its window
// never actually opens -- the exact hazard
// `quadraui-py/tests/test_run_native.py`'s own doc describes and avoids
// the same way, and `tests/live_runner.test.ts`'s own `runDetachedScript`
// helper avoids for `startTui`'s tty hazard.
//
// Run against a build with the `gtk` feature enabled, under a real or
// virtual display:
//
//     npx napi build --platform --features gtk
//     xvfb-run -a node --test tests/live_runner_gtk.test.ts
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { QuiApp } = require("../js/index.ts");

const nativeBackend: string | null = QuiApp.nativeBackend();
const skip =
  nativeBackend !== "gtk" &&
  `extension not built with the gtk feature (nativeBackend() reported ${JSON.stringify(nativeBackend)})`;

/**
 * Runs `body` as its own standalone Node script against a fresh process --
 * see this file's own header doc for why a real `GtkRunner` session needs
 * one script per scenario. Mirrors `tests/live_runner.test.ts`'s own
 * `runDetachedScript`, minus the `detached: true` tty isolation that
 * helper needs and this one doesn't (nothing here touches a terminal).
 */
function runGtkScript(body: string): { stdout: string; stderr: string; status: number | null } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-live-runner-gtk-"));
  try {
    const script = path.join(dir, "script.js");
    fs.writeFileSync(script, body);
    const result = spawnSync(process.execPath, [script], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      timeout: 30_000,
    });
    return { stdout: result.stdout ?? "", stderr: result.stderr ?? "", status: result.status };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

const indexPath = require.resolve("../js/index.ts");

test(
  "startNative opens a real window, steps it, and tears it down via stopLive()",
  { skip },
  () => {
    const body = `
      const { QuiApp } = require(${JSON.stringify(indexPath)});
      const app = new QuiApp(10, 5);
      app.setRender(() => ({ button: { id: "demo:ok", label: "OK", focused: true } }));
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
    const { stdout, status } = runGtkScript(body);
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
  "a registered handler receives a waker().post() delivered while the session is live",
  { skip },
  () => {
    // Unlike `macos`/`win` (see `QuiApp::start_native`'s own doc), `gtk`
    // swaps `QuiApp.current_waker` onto `GtkRunner::waker()` for the
    // session's duration, so a handle obtained once the window is open
    // still reaches this app's own registered handler -- not just the
    // dormant headless queue.
    const body = `
      const { QuiApp } = require(${JSON.stringify(indexPath)});
      const app = new QuiApp(10, 5);
      const received = [];
      app.setHandler((event) => received.push(event));
      app.startNative();
      app.step();
      app.waker().post("hello-from-live-gtk");
      // \`MainContext::invoke\` schedules the wake onto the same default
      // GLib main context \`step()\`'s own \`iteration(false)\` drains, so a
      // bounded handful of steps is enough to observe delivery -- no real
      // wall-clock wait needed (unlike the idle-tick scenario below).
      for (let i = 0; i < 50 && received.length === 0; i++) {
        app.step();
      }
      console.log("RECEIVED:" + JSON.stringify(received));
      console.log("STOP_LIVE:" + app.stopLive());
    `;
    const { stdout, status } = runGtkScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /RECEIVED:\[\{"User":"hello-from-live-gtk"\}\]/);
    assert.match(stdout, /STOP_LIVE:true/);
  },
);

test(
  "a waker().postAfter() delivered from a spawned background thread reaches the handler while live",
  { skip },
  () => {
    // `postAfter` spawns its own `std::thread` on the Rust side (see
    // `QuiAppWaker::post_after`'s own doc) that sleeps, then calls the
    // same `post` path a foreground caller would -- this is the "spawned
    // Rust thread" delivery path, exercised here against a live `gtk`
    // session instead of the headless one
    // `tests/live_runner.test.ts`'s own `postAfter` test covers.
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
    const { stdout, status } = runGtkScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /RECEIVED:\[\{"User":"from-background-thread"\}\]/);
    assert.match(stdout, /STOP_LIVE:true/);
  },
);

test(
  "pump(app) via setInterval does not starve a concurrent setTimeout while a gtk session is live",
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
    const { stdout, status } = runGtkScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(
      stdout,
      /TIMEOUT_FIRED:true/,
      "setTimeout never fired -- pumping a live gtk session starved the event loop",
    );
    assert.match(stdout, /STOP_LIVE:true/);
  },
);

test(
  "the app's own tick handler can request an exit, torn down without stopLive()",
  { skip },
  () => {
    // GTK's own `apply_reaction`/`NativeExitPolicy` (see that type's own
    // doc) honours a `Reaction::Exit` from any event or tick with no
    // wrapper help needed -- unlike the `macos` feature without `gtk`,
    // which downgrades it to a sticky flag instead. Exercises that an
    // app-requested exit (not `stopLive()`) is what actually closes the
    // window and flips `step()` over to raising.
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
      // GTK's own idle tick only fires on a recurring ~250ms timer (see
      // \`quadraui-py/tests/test_run_native.py\`'s own doc for the same
      // knob), so this needs real wall-clock time to elapse, not just a
      // bounded number of non-blocking \`step()\` passes.
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
    const { stdout, status } = runGtkScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /TICKS:2/);
    assert.match(
      stdout,
      /STEP_AFTER_EXIT_RAISED:LiveRunnerExitedError/,
      "an app-requested exit must have already ended this session",
    );
  },
);
