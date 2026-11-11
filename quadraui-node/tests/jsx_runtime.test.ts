// Black-box coverage for `../js/jsx-runtime.ts`/`../js/jsx-dev-runtime.ts`:
// the `jsx`/`jsxs`/`jsxDEV`/`Fragment` runtime contract every JSX-aware
// transform (`tsc`'s `"jsx": "react-jsx"`, esbuild's `jsx: "automatic"`,
// Bun's built-in transform) calls into for a `.tsx` file whose
// `jsxImportSource` is `@quadraui/node`. Also runs the real example
// (`../examples/file_manager_app_jsx.tsx`) through `tsc --strict`, the
// acceptance bar `../package.json`'s own `typecheck:jsx` script names.
//
// Neither runtime file can be `require()`d directly under Node's own
// native TypeScript support -- both are genuine ES modules (real
// `export`, not this package's usual `exports.X = ...`; see
// `../js/jsx-runtime.ts`'s own header doc for why) -- so each is run here
// through `esbuild.transformSync` first (down-leveling `export`/`import`
// into a `module.exports`/`require` a plain `new Function` can execute),
// never `bundle`d: bundling would also try to inline `../js/app/nodes.ts`
// (this package's usual `exports.X = ...` convention, which esbuild's own
// module-type inference mishandles once a sibling `export interface`
// makes it think the whole file is ESM) -- transform-only is both
// sufficient (`jsx-runtime.ts`'s own logic is what this file tests) and
// the one mode that sidesteps that unrelated problem. Each transformed
// module's own `require` calls are intercepted by a small shim below
// instead, so this test needs no built native addon and no real
// `./app/nodes` -- it is pure unit coverage of the adapter/dispatch logic
// in `createElement`.
//
// Run with `npm test` (no addon build needed for this file specifically).
const { test } = require("node:test");
const assert = require("node:assert/strict");
const path = require("node:path");
const fs = require("node:fs");
const { execFileSync } = require("node:child_process");
const esbuild = require("esbuild");

const JSX_RUNTIME_PATH = path.join(__dirname, "..", "js", "jsx-runtime.ts");
const JSX_DEV_RUNTIME_PATH = path.join(__dirname, "..", "js", "jsx-dev-runtime.ts");
const REPO_ROOT = path.join(__dirname, "..");

type RequireShim = (id: string) => unknown;

/**
 * Transforms one `.ts`/`.tsx` file (never bundles -- see this file's own
 * header doc) and executes the result with `require` replaced by
 * `shim`, returning that module's own `exports`. `requireShim` below is
 * the shim every test here actually passes.
 */
function loadTransformed(absPath: string, shim: RequireShim): Record<string, unknown> {
  const source = fs.readFileSync(absPath, "utf8");
  const loader = absPath.endsWith(".tsx") ? "tsx" : "ts";
  // `jsx`/`jsxImportSource` matter only for the `.tsx` branch (a `.ts`
  // file has no JSX syntax to transform), and must match the package
  // README's own documented config (`"jsx": "react-jsx"`,
  // `"jsxImportSource": "@quadraui/node"`) so a transformed `.tsx` file's
  // `require("@quadraui/node/jsx-runtime")` call is exactly what a real
  // `tsc`/esbuild/Bun consumer emits, for `requireShim` and
  // `realRequireShim` below to intercept.
  const { code } = esbuild.transformSync(source, {
    loader,
    format: "cjs",
    target: "es2020",
    jsx: "automatic",
    jsxImportSource: "@quadraui/node",
  });
  const mod: { exports: Record<string, unknown> } = { exports: {} };
  const wrapper = new Function("require", "module", "exports", "__filename", "__dirname", code);
  wrapper(shim, mod, mod.exports, absPath, path.dirname(absPath));
  return mod.exports;
}

/** Fake `../js/app/nodes.ts`: each stub records exactly the positional args it was called with, so a test can assert on them directly instead of needing the real wire-shape builders. */
function stubNodeConstructors(): Record<string, (...args: unknown[]) => unknown> {
  const stub = (name: string) => (...args: unknown[]) => ({ __call: name, args });
  return {
    Split: stub("Split"),
    Column: stub("Column"),
    Row: stub("Row"),
    SidebarPanel: stub("SidebarPanel"),
    Tabs: stub("Tabs"),
    Tree: stub("Tree"),
    Table: stub("Table"),
    StatusBar: stub("StatusBar"),
    TextDisplay: stub("TextDisplay"),
    Button: stub("Button"),
  };
}

/** `jsx-runtime.ts`'s own `require("./app/nodes.ts")` resolved against {@link stubNodeConstructors}; anything else falls through to real `require`. */
function requireShim(stubs: Record<string, unknown>): RequireShim {
  return (id: string) => {
    if (id === "./app/nodes.ts") {
      return stubs;
    }
    if (id === "./jsx-runtime" || id === "./jsx-runtime.ts") {
      return loadJsxRuntime();
    }
    return require(id);
  };
}

let cachedRuntime: Record<string, unknown> | null = null;
let cachedStubs: Record<string, unknown> | null = null;

function loadJsxRuntime(): Record<string, unknown> {
  if (cachedRuntime === null) {
    cachedStubs = stubNodeConstructors();
    cachedRuntime = loadTransformed(JSX_RUNTIME_PATH, requireShim(cachedStubs));
  }
  return cachedRuntime;
}

function freshRuntime(): { runtime: Record<string, unknown>; stubs: Record<string, unknown> } {
  const stubs = stubNodeConstructors();
  const runtime = loadTransformed(JSX_RUNTIME_PATH, requireShim(stubs));
  return { runtime, stubs };
}

// --- intrinsic adapters -------------------------------------------------

test("<split> unpacks exactly two children into Split's own positional slots", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: string, props: Record<string, unknown>) => { __call: string; args: unknown[] };
  const result = jsx("split", { ratio: 0.3, children: ["first", "second"] });
  assert.equal(result.__call, "Split");
  assert.deepEqual(result.args, ["first", "second", { ratio: 0.3 }]);
});

test("<column> normalizes a single child into a one-element list", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: string, props: Record<string, unknown>) => { __call: string; args: unknown[] };
  const result = jsx("column", { children: "only-child" });
  assert.equal(result.__call, "Column");
  assert.deepEqual(result.args, [["only-child"], {}]);
});

test("<column> passes an absent children prop through as an empty list", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: string, props: Record<string, unknown>) => { __call: string; args: unknown[] };
  const result = jsx("column", { gap: 2 });
  assert.deepEqual(result.args, [[], { gap: 2 }]);
});

test("<tree> forwards its own leaf props with no children slot to unpack", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: string, props: Record<string, unknown>) => { __call: string; args: unknown[] };
  const rows = [{ path: [0], indent: 0 }];
  const result = jsx("tree", { rows, id: "fm:tree", hasFocus: true });
  assert.equal(result.__call, "Tree");
  assert.deepEqual(result.args, [rows, { id: "fm:tree", hasFocus: true }]);
});

test("<button> splits its required label out of the options bag", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: string, props: Record<string, unknown>) => { __call: string; args: unknown[] };
  const result = jsx("button", { label: "Save", variant: "Primary" });
  assert.deepEqual(result.args, ["Save", { variant: "Primary" }]);
});

test("an unknown intrinsic tag throws instead of silently returning something unrenderable", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: string, props: Record<string, unknown>) => unknown;
  assert.throws(() => jsx("not-a-real-tag", {}), /unknown quadraui JSX intrinsic element/);
});

test("jsxs is the same function as jsx -- both ways a transform calls into this runtime behave identically", () => {
  const { runtime } = freshRuntime();
  assert.equal(runtime.jsxs, runtime.jsx);
});

// --- Fragment -------------------------------------------------------------

test("a Fragment element unwraps to its own children, unchanged", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: unknown, props: Record<string, unknown>) => unknown;
  const children = ["a", "b"];
  assert.equal(jsx(runtime.Fragment, { children }), children);
});

test("a Fragment with no children at all resolves to null, not undefined", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: unknown, props: Record<string, unknown>) => unknown;
  assert.equal(jsx(runtime.Fragment, {}), null);
});

// --- function components ---------------------------------------------------

test("a plain function type is called with its own props object directly", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: unknown, props: Record<string, unknown>) => unknown;
  function Greeting(props: { name: string }) {
    return `hello, ${props.name}`;
  }
  assert.equal(jsx(Greeting, { name: "ada" }), "hello, ada");
});

// --- Component-class elements ----------------------------------------------

test("a Component-shaped class is instantiated with `new`, not called", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: unknown, props: Record<string, unknown>, key?: unknown) => unknown;
  class FakeComponent {
    key: unknown;
    constructor(key?: unknown) {
      this.key = key;
    }
    initialState() {
      return {};
    }
    view() {
      return null;
    }
  }
  const instance = jsx(FakeComponent, {}, "row-7") as FakeComponent;
  assert.ok(instance instanceof FakeComponent);
  assert.equal(instance.key, "row-7");
});

test("a plain function (no view/initialState pair) is never mistaken for a Component class", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: unknown, props: Record<string, unknown>) => unknown;
  function NotAComponent(props: { x: number }) {
    return props.x * 2;
  }
  assert.equal(jsx(NotAComponent, { x: 21 }), 42);
});

test("a type that is neither a string, a function, nor a class throws", () => {
  const { runtime } = freshRuntime();
  const jsx = runtime.jsx as (type: unknown, props: Record<string, unknown>) => unknown;
  assert.throws(() => jsx(42 as unknown as string, {}), /not a renderable quadraui JSX element type/);
});

// --- jsx-dev-runtime --------------------------------------------------------

test("jsxDEV ignores its dev-only diagnostic arguments and behaves exactly like jsx", () => {
  const stubs = stubNodeConstructors();
  const shim = requireShim(stubs);
  const devRuntime = loadTransformed(JSX_DEV_RUNTIME_PATH, shim);
  const jsxDEV = devRuntime.jsxDEV as (
    type: string,
    props: Record<string, unknown>,
    key?: unknown,
    isStaticChildren?: boolean,
    source?: unknown,
    self?: unknown,
  ) => { __call: string; args: unknown[] };
  const result = jsxDEV("button", { label: "Save" }, undefined, false, { fileName: "x.tsx", lineNumber: 1, columnNumber: 1 }, undefined);
  assert.deepEqual(result.args, ["Save", {}]);
});

test("jsx-dev-runtime re-exports the same Fragment marker as jsx-runtime", () => {
  // Both sides must resolve `./jsx-runtime` through the very same cached
  // evaluation (`loadJsxRuntime`): each independent `loadTransformed` call
  // re-executes the module from scratch, so the two `Symbol("quadraui.jsx.Fragment")`
  // calls would otherwise never be `===` -- an artifact of this test's own
  // harness, not of the real module system (a real `require` cache gives
  // every real consumer exactly one evaluation).
  const prodRuntime = loadJsxRuntime();
  const shim = requireShim(cachedStubs as Record<string, unknown>);
  const devRuntime = loadTransformed(JSX_DEV_RUNTIME_PATH, shim);
  assert.equal(devRuntime.Fragment, prodRuntime.Fragment);
});

// --- `tsc --strict` acceptance: the real example ---------------------------
//
// Mirrors the issue's own acceptance bullets directly: the JSX port of the
// file-manager example type-checks under `tsc --strict`, and a wrong prop
// name on one of its elements fails that same check. Needs the native
// addon built first (`npm run build:debug`) -- `../js/app/runtime.ts`
// type-imports `../index`'s own generated `index.d.ts` -- same precondition
// every other `*.test.ts` file in this directory already documents.

function runTsc(args: string[]): { status: number | null; output: string } {
  // `node_modules/.bin/tsc` is a POSIX shell shim on every platform (the
  // Windows executable forms are `tsc.cmd`/`tsc.ps1`), so `execFileSync`
  // without `shell: true` can't launch it there. Invoking the real script
  // (`node_modules/typescript/bin/tsc`, a `#!/usr/bin/env node` file) with
  // this process's own `node` binary works identically on every platform --
  // the same pattern every other spawning test in this directory already
  // uses (`live_runner.test.ts`, `live_runner_win.test.ts`, ...).
  const tscScript = path.join(REPO_ROOT, "node_modules", "typescript", "bin", "tsc");
  try {
    const output = execFileSync(process.execPath, [tscScript, ...args], {
      cwd: REPO_ROOT,
      encoding: "utf8",
    });
    return { status: 0, output };
  } catch (error) {
    const err = error as { status: number | null; stdout?: string };
    return { status: err.status, output: err.stdout ?? "" };
  }
}

test("the JSX file-manager example type-checks clean under tsc --strict", { skip: !fs.existsSync(path.join(REPO_ROOT, "index.d.ts")) }, () => {
  const result = runTsc(["-p", "tsconfig.jsx.json"]);
  assert.equal(result.status, 0, result.output);
});

test("a wrong prop name on an intrinsic element fails tsc --strict", { skip: !fs.existsSync(path.join(REPO_ROOT, "index.d.ts")) }, () => {
  const original = fs.readFileSync(path.join(REPO_ROOT, "examples", "file_manager_app_jsx.tsx"), "utf8");
  assert.ok(original.includes('leftSegments={left}'), "fixture source has drifted -- update this test's own substitution");
  const broken = original.replace("leftSegments={left}", "leftSegmentsTypo={left}");
  // Suffixed with this process's own pid (see the top-level `.gitignore`
  // entries for these fixtures) so two concurrent runs of this test never
  // write the same path out from under each other.
  const suffix = `.${process.pid}`;
  const brokenPath = path.join(REPO_ROOT, "examples", `file_manager_app_jsx.broken_fixture${suffix}.tsx`);
  fs.writeFileSync(brokenPath, broken);
  const tsconfigPath = path.join(REPO_ROOT, `tsconfig.jsx.broken_fixture${suffix}.json`);
  try {
    const base = JSON.parse(fs.readFileSync(path.join(REPO_ROOT, "tsconfig.jsx.json"), "utf8"));
    base.include = [`examples/file_manager_app_jsx.broken_fixture${suffix}.tsx`];
    fs.writeFileSync(tsconfigPath, JSON.stringify(base));
    const result = runTsc(["-p", `tsconfig.jsx.broken_fixture${suffix}.json`]);
    assert.notEqual(result.status, 0);
    assert.match(result.output, /leftSegmentsTypo/);
  } finally {
    fs.rmSync(brokenPath, { force: true });
    fs.rmSync(tsconfigPath, { force: true });
  }
});

// --- the real app layer, end to end ----------------------------------------
//
// Every test above substitutes `stubNodeConstructors()` for
// `./app/nodes.ts`, so none of them would catch a swapped positional pair
// in one of `INTRINSICS`' own adapters (e.g. `StatusBar`'s two segment
// lists in the wrong order): a bug like that type-checks and passes every
// stub-based test above, then silently renders wrong. This test drives the
// real `../examples/file_manager_app_jsx.tsx` -- transformed through this
// runtime's own real `require("./app/nodes.ts")`, no stub -- through the
// same scripted actions as its non-JSX twin, `../examples/
// file_manager_app.ts`, and asserts the two headless screens stay
// byte-for-byte identical after every step: the behavioural half of the
// issue's "behaviour-identical" acceptance bullet that `tsc --strict`
// alone can't check. Needs the native addon built first, the same
// precondition the `tsc`-based tests above already document.

const REAL_JSX_RUNTIME_SPECIFIER = "@quadraui/node/jsx-runtime";

let cachedRealRuntime: Record<string, unknown> | null = null;

/** `../js/jsx-runtime.ts`, transformed the same way as {@link loadJsxRuntime} above, but with its own `require("./app/nodes.ts")` resolved to the real module -- what actually builds real wire nodes, not a `{__call, args}` stub. */
function loadRealJsxRuntime(): Record<string, unknown> {
  if (cachedRealRuntime === null) {
    cachedRealRuntime = loadTransformed(JSX_RUNTIME_PATH, realRequireShim(JSX_RUNTIME_PATH));
  }
  return cachedRealRuntime;
}

/** `require`, with every relative id resolved against `fromAbsPath`'s own directory and handed to Node's real `require` (so a plain `.ts` file like `./app/nodes.ts` resolves through Node's own native TypeScript support, the same way every non-JSX `*.test.ts` file in this directory already requires one), and `@quadraui/node/jsx-runtime` resolved to {@link loadRealJsxRuntime} -- exactly the specifier a real `tsc`/esbuild/Bun consumer's own automatic-JSX import would resolve, per the package's `exports` map. */
function realRequireShim(fromAbsPath: string): RequireShim {
  const fromDir = path.dirname(fromAbsPath);
  return (id: string) => {
    if (id === REAL_JSX_RUNTIME_SPECIFIER) {
      return loadRealJsxRuntime();
    }
    if (id.startsWith(".")) {
      return require(path.resolve(fromDir, id));
    }
    return require(id);
  };
}

type FileManagerCtor = new (root: string, width?: number, height?: number) => { app: unknown };

/** The fixed directory tree `../examples/file_manager_app.test.ts`'s own `treeRoot()` builds, duplicated here rather than shared so this test's own tree shape can't drift silently if that helper changes. */
function exampleTreeRoot(): string {
  const root = fs.mkdtempSync(path.join(require("node:os").tmpdir(), "quadraui-node-jsx-example-"));
  const alpha = path.join(root, "alpha");
  fs.mkdirSync(alpha);
  fs.writeFileSync(path.join(alpha, "a1.txt"), "one");
  fs.writeFileSync(path.join(alpha, "a2.txt"), "two");
  const beta = path.join(root, "beta");
  fs.mkdirSync(beta);
  fs.writeFileSync(path.join(beta, "b1.txt"), "bee");
  fs.writeFileSync(path.join(root, "readme.md"), "# readme");
  return root;
}

test(
  "the JSX file-manager example renders byte-for-byte identical screens to its non-JSX twin",
  { skip: !fs.existsSync(path.join(REPO_ROOT, "index.d.ts")) },
  () => {
    const jsxExamplePath = path.join(REPO_ROOT, "examples", "file_manager_app_jsx.tsx");
    const jsxExample = loadTransformed(jsxExamplePath, realRequireShim(jsxExamplePath)) as {
      FileManager: FileManagerCtor;
    };
    const plainExample = require("../examples/file_manager_app.ts") as { FileManager: FileManagerCtor };
    const { harness } = require("../js/testing.ts") as { harness: (app: unknown) => any };

    const root = exampleTreeRoot();
    try {
      const jsxHarness = harness(new jsxExample.FileManager(root, 80, 24).app);
      const plainHarness = harness(new plainExample.FileManager(root, 80, 24).app);
      assert.equal(jsxHarness.screen(), plainHarness.screen());

      // Walks the tree, opens two folders into details tabs, closes one --
      // touches every intrinsic the example's own `_view`/`_detailsPane`/
      // `_statusLeaf` use (`split`, `sidebarPanel`, `tree`, `tabs`,
      // `table`, `textDisplay`, `statusBar`).
      const actions: Array<(h: any) => void> = [
        (h) => h.press("Down"), // root -> alpha
        (h) => h.press("Right"), // expand alpha (loads it)
        (h) => h.press("Enter"), // open alpha's details tab
        (h) => h.press("Down"), // alpha -> sub/a1.txt row depends on tree, stay folders-only -> beta
        (h) => h.press("Down"),
        (h) => h.press("Enter"), // open a second details tab
        (h) => h.press("Tab"), // move focus between tree/details, if bound
        (h) => h.clickText("alpha"), // click back onto the alpha row
        (h) => h.press("Enter"),
      ];
      for (const action of actions) {
        action(jsxHarness);
        action(plainHarness);
        assert.equal(jsxHarness.screen(), plainHarness.screen());
      }
    } finally {
      fs.rmSync(root, { recursive: true, force: true });
    }
  },
);
