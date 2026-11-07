// Black-box coverage for `QuiApp.startNative()`'s `win` arm: opens a real
// Win32 window via `quadraui::win::WinRunner`, drives it through the same
// non-blocking `step()`/`pump()` cadence `tests/live_runner.test.ts` covers
// for the headless/`startTui` paths and `tests/live_runner_gtk.test.ts`/
// `tests/live_runner_macos.test.ts` cover for `gtk`/`macos`, and tears it
// down deterministically rather than by a human -- see
// `quadraui-node/src/lib.rs`'s module doc's "Live run loop" section for the
// overall design.
//
// Only runs on a real Windows host built with the `win` feature.
// `nativeBackend()` alone is not enough to decide that -- its own doc says
// so explicitly: `"win"` is reported for *any* `target_os`, including a
// Linux/macOS typecheck build where `startNative` can only raise
// `InternalError` for the platform mismatch, never open a real window. This
// file's `skip` condition therefore also checks `process.platform`. Every
// scenario below shells out to its own standalone script rather than
// opening a `WinRunner` in this test process directly -- not because a
// second `WinRunner` in the same process is unsafe the way a second
// `GtkRunner` is, but so a crash or a lingering real window from one
// scenario can never bleed into the next, mirroring
// `tests/live_runner_gtk.test.ts`/`tests/live_runner_macos.test.ts`'s own
// isolation.
//
// Run against a build with the `win` feature enabled, on a real Windows
// host:
//
//     npx napi build --platform --features win
//     node --test tests/live_runner_win.test.ts
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");

const { QuiApp } = require("../js/index.ts");

const nativeBackend: string | null = QuiApp.nativeBackend();
const skip =
  (nativeBackend !== "win" || process.platform !== "win32") &&
  `extension not built with the win feature on a real Windows host (nativeBackend() ` +
    `reported ${JSON.stringify(nativeBackend)}, process.platform is ${JSON.stringify(process.platform)})`;

/**
 * Runs `body` as its own standalone Node process (via `node -e`), mirroring
 * `tests/live_runner_gtk.test.ts`'s own `runGtkScript`/
 * `tests/live_runner_macos.test.ts`'s own `runMacScript` -- see this file's
 * own header doc for why a real `WinRunner` session gets one process per
 * scenario.
 */
function runWinScript(body: string): { stdout: string; stderr: string; status: number | null } {
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
    const { stdout, status } = runWinScript(body);
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
    const { stdout, status } = runWinScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /FIRST_STOP:true/);
    assert.match(stdout, /SECOND_STEP:true/);
    assert.match(stdout, /SECOND_STOP:true/);
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
    // exercised here against a live `win` session rather than the
    // headless one `tests/live_runner.test.ts`'s own `postAfter` test
    // covers. Reaching the handler at all depends on `start_native`'s
    // `win` arm having swapped `QuiApp.current_waker` onto the waker
    // `NodeAppLogic::setup` captured off the live `WinBackend` -- without
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
    const { stdout, status } = runWinScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /RECEIVED:\[\{"User":"from-background-thread"\}\]/);
    assert.match(stdout, /STOP_LIVE:true/);
  },
);

test(
  "pump(app) via setInterval does not starve a concurrent setTimeout while a win session is live",
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
    const { stdout, status } = runWinScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(
      stdout,
      /TIMEOUT_FIRED:true/,
      "setTimeout never fired -- pumping a live win session starved the event loop",
    );
    assert.match(stdout, /STOP_LIVE:true/);
  },
);

test(
  "the app's own tick handler can request an exit, torn down without stopLive()",
  { skip },
  () => {
    // `NativeExitPolicy::tick`/`::handle` have no `exit_requested` field on
    // the `win` feature (unlike `macos` without `gtk`) -- `win::run`'s own
    // dispatch/tick already closes the window for *any* `Reaction::Exit`,
    // not only one reached through `WindowClose` (see that type's own
    // doc). This exercises that an app-requested exit (not `stopLive()`)
    // really does close the window and flip `step()` over to raising.
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
    const { stdout, status } = runWinScript(body);
    assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
    assert.match(stdout, /TICKS:2/);
    assert.match(
      stdout,
      /STEP_AFTER_EXIT_RAISED:LiveRunnerExitedError/,
      "an app-requested exit must have already ended this session",
    );
  },
);
