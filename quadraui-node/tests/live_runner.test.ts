// Black-box coverage for the non-blocking run-loop surface
// (`QuiApp.waker()`/`QuiAppWaker`, the `pump()` JS shim, and `startTui`'s
// own fail-fast-without-a-real-tty contract) -- see
// `quadraui-node/src/lib.rs`'s module doc's "Live run loop" section for the
// overall design this ports `quadraui-py/tests/test_run_tui.py`'s own
// doc from.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");
const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const { QuiApp, pump } = require("../js/index.ts");

/**
 * Races `promise` against a bounded timer, rejecting with `message` if
 * `promise` hasn't settled by then -- so the exact regression a test below
 * exists to catch (a starved event loop, or a `pump` that never advances)
 * surfaces as a failing assertion instead of hanging the suite forever
 * (`node:test` applies no default per-test timeout). Mirrors the
 * `postAfter` test's own bounded `deadline` poll loop, generalized to wrap
 * an arbitrary promise instead of a `while` loop.
 */
function withTimeout<T>(promise: Promise<T>, ms: number, message: string): Promise<T> {
  let timer: NodeJS.Timeout;
  const timedOut = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(message)), ms);
  });
  return Promise.race([promise, timedOut]).finally(() => clearTimeout(timer));
}

test("waker().post delivers a {User: value} event to the registered handler", () => {
  const app = new QuiApp(5, 5);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => {
    received.push(event);
  });

  const waker = app.waker();
  waker.post(42);
  assert.equal(app.step(), true, "delivering a User event must not exit the app");
  assert.deepEqual(received, [{ User: 42 }]);
});

test("waker().wake delivers a {User: null} event", () => {
  const app = new QuiApp(5, 5);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => {
    received.push(event);
  });

  app.waker().wake();
  app.step();
  assert.deepEqual(received, [{ User: null }]);
});

// Also the one place in this suite that exercises background-thread ->
// JS-handler delivery end to end (the Rust-side `#[test]`s in `lib.rs`
// cover the waker/queue plumbing itself without a live `Env`/registered
// handler -- see `waker_post_from_a_background_thread_delivers_a_user_event`'s
// own doc for why).
test("waker().postAfter delivers once the delay elapses", async () => {
  const app = new QuiApp(5, 5);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => {
    received.push(event);
  });

  app.waker().postAfter(10, "later");
  // Poll with `step()` on a short timer until the delayed post lands, with
  // a generous ceiling so this can't hang the suite if it never does.
  const deadline = Date.now() + 2_000;
  while (received.length === 0 && Date.now() < deadline) {
    app.step();
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
  assert.deepEqual(received, [{ User: "later" }]);
});

test("waker() keeps returning a usable handle after this QuiApp has run several steps", () => {
  // `waker()` only ever reads the app's current `current_waker` cell, so
  // obtaining a fresh handle well after construction (and after this app
  // has already stepped) must still work, rather than being a one-shot
  // "obtain before anything else" affordance.
  const app = new QuiApp(5, 5);
  app.step();
  app.step();
  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.waker().post("late-bound");
  app.step();
  assert.deepEqual(received, [{ User: "late-bound" }]);
});

test("stopLive on an app with no live session is a harmless no-op", () => {
  const app = new QuiApp(5, 5);
  assert.equal(app.stopLive(), false);
  // The headless app must still work afterwards.
  assert.equal(app.step(), true);
});

test("pump(app) via setInterval does not starve a concurrent setTimeout", async () => {
  const app = new QuiApp(8, 3);
  let timeoutFired = false;
  const timeoutPromise = new Promise<void>((resolve) => {
    setTimeout(() => {
      timeoutFired = true;
      resolve();
    }, 20);
  });

  const handle = pump(app, 1);
  await withTimeout(
    timeoutPromise,
    5_000,
    "setTimeout never fired -- pump(app) starved the event loop",
  );
  handle.stop();

  assert.equal(timeoutFired, true);
});

test("pump(app) stops itself once step() reports the app has exited", async () => {
  const app = new QuiApp(5, 5);
  let stepCalls = 0;
  app.setTickHandler(() => {
    stepCalls += 1;
    return stepCalls >= 3 ? false : undefined;
  });

  await withTimeout(
    new Promise<void>((resolve) => {
      pump(app, 1);
      const check = setInterval(() => {
        if (stepCalls >= 3) {
          clearInterval(check);
          resolve();
        }
      }, 1);
    }),
    5_000,
    "pump(app) never drove step() to the expected 3 calls",
  );

  const callsWhenStopped = stepCalls;
  await new Promise((resolve) => setTimeout(resolve, 20));
  assert.equal(
    stepCalls,
    callsWhenStopped,
    "pump must stop calling step() once the app has exited",
  );
});

function runDetachedScript(body: string): { stdout: string; status: number | null } {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-live-runner-"));
  try {
    const script = path.join(dir, "script.js");
    fs.writeFileSync(script, body);
    // `detached: true` (POSIX `setsid()`) puts the child in its own session
    // with no controlling terminal at all, so crossterm's `open("/dev/tty")`
    // fallback fails with `ENXIO` instead of seizing whatever real terminal
    // launched this test suite -- the exact hazard
    // `quadraui-py/tests/test_run_tui.py`'s own doc describes for the
    // equivalent Python test, worked around the same way here.
    const result = spawnSync(process.execPath, [script], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      detached: true,
      timeout: 30_000,
    });
    return { stdout: result.stdout ?? "", status: result.status };
  } finally {
    fs.rmSync(dir, { recursive: true, force: true });
  }
}

test("startTui raises without a real tty and leaves the headless app usable", () => {
  const indexPath = require.resolve("../js/index.ts");
  const body = `
    const { QuiApp } = require(${JSON.stringify(indexPath)});
    const app = new QuiApp(10, 10);
    app.insertButton({ id: "demo:ok", label: "OK", focused: true });
    try {
      app.startTui();
      console.log("START_TUI_DID_NOT_RAISE");
    } catch (err) {
      console.log("START_TUI_RAISED:" + err.message);
    }
    app.screen();
    console.log("SCREEN_STILL_WORKS");
  `;
  const { stdout, status } = runDetachedScript(body);
  assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
  assert.match(stdout, /START_TUI_RAISED:InternalError: startTui:/);
  assert.match(stdout, /SCREEN_STILL_WORKS/);
});

test("startTui accepts mouse: false and fails the same way without a real tty", () => {
  const indexPath = require.resolve("../js/index.ts");
  const body = `
    const { QuiApp } = require(${JSON.stringify(indexPath)});
    const app = new QuiApp(10, 10);
    try {
      app.startTui(false);
      console.log("START_TUI_DID_NOT_RAISE");
    } catch (err) {
      console.log("START_TUI_RAISED:" + err.message);
    }
  `;
  const { stdout, status } = runDetachedScript(body);
  assert.equal(status, 0, `script did not exit cleanly: ${stdout}`);
  assert.match(stdout, /START_TUI_RAISED:InternalError: startTui:/);
});

{
  const { QuiApp, NoNativeBackendError } = require("../js/index.ts");
  // `QuiApp.nativeBackend()` reports which native feature (if any) this
  // build was compiled with -- see that method's own doc. With one
  // enabled, `startNative` takes a real backend's own arm instead of the
  // "no native backend compiled" one this test exercises; that arm's own
  // behaviour is each backend's own capability-lane test (e.g.
  // `tests/live_runner_gtk.test.ts` for `gtk`), not this platform-neutral
  // suite.
  const nativeBackend: string | null = QuiApp.nativeBackend();
  test(
    "startNative raises NoNativeBackendError on a build with no gtk/macos/win feature, and leaves the headless app usable",
    { skip: nativeBackend !== null && `built with the ${nativeBackend} native feature` },
    () => {
      const app = new QuiApp(10, 10);
      app.insertButton({ id: "demo:ok", label: "OK", focused: true });
      assert.throws(
        () => app.startNative(),
        (err: unknown) => {
          assert.ok(err instanceof NoNativeBackendError);
          assert.match((err as Error).message, /NoNativeBackendError: startNative/);
          return true;
        },
      );
      // The headless app must still work afterwards -- `startNative` never
      // moved anything out of it on this failure path.
      assert.match(app.screen(), /OK/);
    },
  );
}
