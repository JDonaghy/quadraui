// Installs `@quadraui/node` the way a user gets it from npm and runs the
// hero counter from that install. Packs the root package and all three
// platform packages with `npm pack`, installs the root tarball plus this
// host's platform tarball into a fresh project outside the repo, and then,
// from that project's `node_modules`:
//
//   1. `require()`s the root and every runtime subpath, and checks that the
//      root and the app layer share one set of error classes;
//   2. `import`s the same entries from an ES module;
//   3. type-checks a TypeScript consumer of `@quadraui/node/app` under
//      `tsc --strict --module node16` (no `skipLibCheck`);
//   4. compiles `examples/hero/node/counter.tsx` with `tsc` and the package's
//      JSX runtime, then clicks `+`, `-`, `+` headless and expects `Count: 1`;
//   5. runs the compiled counter live in a private tmux server, waits for
//      `Count: 0`, presses `q` and expects a clean exit.
//
// Step 5 needs tmux. Without it the step is skipped with a message, unless
// `--live` is given, which makes a missing tmux an error.
//
// Needs this host's native addon built first (`npm run build` or
// `npm run build:debug`), and network access for the consumer project's
// `typescript` and `@types/node`. Nothing is published.
//
//     node scripts/pack_smoke.ts [--live] [--keep]

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawnSync } = require("node:child_process");

const PKG = path.resolve(__dirname, "..");
const REPO = path.resolve(PKG, "..");
const args = new Set(process.argv.slice(2));
const requireLive = args.has("--live");
const keep = args.has("--keep");
const isWindows = process.platform === "win32";

function fail(message: string): never {
  console.error(`pack smoke: FAIL: ${message}`);
  process.exit(1);
}

function run(cmd: string, argv: string[], cwd: string, input?: string): string {
  const result = spawnSync(cmd, argv, { cwd, encoding: "utf8", shell: isWindows, input, maxBuffer: 64 * 1024 * 1024 });
  if (result.status !== 0) {
    fail(`${cmd} ${argv.join(" ")} (in ${cwd}) exited ${result.status}\n${result.stdout ?? ""}${result.stderr ?? ""}`);
  }
  return result.stdout;
}

function npm(argv: string[], cwd: string): string {
  return run("npm", argv, cwd);
}

/** `npm pack` one package directory into `dest`; returns the tarball path and its file list. */
function pack(dir: string, dest: string): { tarball: string; files: string[] } {
  const [info] = JSON.parse(npm(["pack", "--json", "--pack-destination", dest], dir));
  return { tarball: path.join(dest, info.filename), files: info.files.map((f: { path: string }) => f.path) };
}

function hostPlatform(): string {
  const key = `${process.platform}-${process.arch}`;
  if (process.platform === "linux") return `${key}-gnu`;
  if (process.platform === "win32") return `${key}-msvc`;
  return key;
}

const work = fs.mkdtempSync(path.join(os.tmpdir(), "quadraui-node-pack-smoke-"));
const tarballs = path.join(work, "tarballs");
fs.mkdirSync(tarballs);
const host = hostPlatform();
const manifest = JSON.parse(fs.readFileSync(path.join(PKG, "package.json"), "utf8"));
const binaryName = `${manifest.napi.binaryName}.${host}.node`;
const binary = path.join(PKG, binaryName);
if (!fs.existsSync(binary)) fail(`${binaryName} not found: run \`npm run build:debug\` (or \`npm run build\`) in quadraui-node first`);

// --- pack ------------------------------------------------------------------

const root = pack(PKG, tarballs);
const strayFiles = root.files.filter((f) => (f.endsWith(".ts") && !f.endsWith(".d.ts")) || f.endsWith(".node"));
if (strayFiles.length > 0) fail(`root tarball ships source or native files: ${strayFiles.join(", ")}`);
console.log(`packed ${path.basename(root.tarball)} (${root.files.length} files)`);

const platformTarballs: Record<string, string> = {};
for (const platform of fs.readdirSync(path.join(PKG, "npm"))) {
  // A copy, so dropping this host's binary in never touches the checkout.
  const staged = path.join(work, "platform", platform);
  fs.cpSync(path.join(PKG, "npm", platform), staged, { recursive: true });
  if (platform === host) fs.copyFileSync(binary, path.join(staged, binaryName));
  const packed = pack(staged, tarballs);
  platformTarballs[platform] = packed.tarball;
  console.log(`packed ${path.basename(packed.tarball)} (${packed.files.length} files)`);
}
if (!platformTarballs[host]) fail(`no npm/${host} platform package in quadraui-node/npm`);

// --- install into a clean project -----------------------------------------

const consumer = path.join(work, "consumer");
fs.mkdirSync(consumer);
fs.writeFileSync(path.join(consumer, "package.json"), JSON.stringify({ name: "quadraui-smoke", private: true, type: "commonjs" }, null, 2));
const toolVersion = (name: string) => JSON.parse(fs.readFileSync(path.join(PKG, "node_modules", name, "package.json"), "utf8")).version;
npm(
  [
    "install",
    "--no-audit",
    "--no-fund",
    root.tarball,
    platformTarballs[host],
    `typescript@${toolVersion("typescript")}`,
    `@types/node@${toolVersion("@types/node")}`,
  ],
  consumer,
);
console.log(`installed into ${consumer}`);

function node(file: string): string {
  return run(process.execPath, [file], consumer);
}

// --- 1. require() ----------------------------------------------------------

fs.writeFileSync(
  path.join(consumer, "require_check.cjs"),
  `
const assert = require("node:assert/strict");
const ids = ["@quadraui/node", "@quadraui/node/app", "@quadraui/node/app/nodes", "@quadraui/node/testing", "@quadraui/node/jsx-runtime", "@quadraui/node/jsx-dev-runtime"];
for (const id of ids) {
  const resolved = require.resolve(id);
  assert.ok(resolved.includes("node_modules"), id + " resolved outside node_modules: " + resolved);
  assert.ok(resolved.endsWith(".js"), id + " resolved to " + resolved);
  assert.ok(Object.keys(require(id)).length > 0, id + " has no exports");
}
const root = require("@quadraui/node");
const { App, Button } = require("@quadraui/node/app");
const { harness } = require("@quadraui/node/testing");
assert.throws(() => new App(0, 3, { view: () => null }), (err) => err instanceof root.InvalidDimensionsError && err instanceof root.QuadrauiError);
const raw = new root.QuiApp(20, 3);
raw.setRender(() => ({ button: { not: "a button" } }));
assert.throws(() => raw.screen(), (err) => err instanceof root.InvalidDescriptorError);
let clicks = 0;
const h = harness(new App(20, 3, { view: () => Button("Go", { id: "go", onClick: () => clicks++ }) }));
h.clickText("Go");
assert.equal(clicks, 1);
console.log("require: ok");
`,
);
console.log(node("require_check.cjs").trim());

// --- 2. import -------------------------------------------------------------

fs.writeFileSync(
  path.join(consumer, "import_check.mjs"),
  `
import assert from "node:assert/strict";
import { QuiApp, QuadrauiError } from "@quadraui/node";
import { App, Button, Column } from "@quadraui/node/app";
import { harness } from "@quadraui/node/testing";
import { jsx, Fragment } from "@quadraui/node/jsx-runtime";
import { jsxDEV } from "@quadraui/node/jsx-dev-runtime";
for (const value of [QuiApp, QuadrauiError, App, Button, Column, harness, jsx, jsxDEV]) assert.equal(typeof value, "function");
assert.equal(typeof Fragment, "symbol");
let clicks = 0;
const h = harness(new App(20, 3, { view: () => jsx("button", { id: "go", label: "Go", onClick: () => clicks++ }) }));
h.clickText("Go");
assert.equal(clicks, 1);
console.log("import: ok");
`,
);
console.log(node("import_check.mjs").trim());

// --- 3. TypeScript consumer ------------------------------------------------

const tsc = path.join(consumer, "node_modules", "typescript", "bin", "tsc");
fs.copyFileSync(path.join(PKG, "tests", "fixtures", "typed_app_consumer.ts"), path.join(consumer, "typed_app_consumer.ts"));
run(process.execPath, [tsc, "--strict", "--noEmit", "--module", "node16", "--moduleResolution", "node16", "--target", "es2020", "--types", "node", "typed_app_consumer.ts"], consumer);
console.log("tsc --strict --module node16: ok");

// --- 4. hero counter, headless ---------------------------------------------

fs.copyFileSync(path.join(REPO, "examples", "hero", "node", "counter.tsx"), path.join(consumer, "counter.tsx"));
fs.writeFileSync(
  path.join(consumer, "tsconfig.json"),
  JSON.stringify(
    {
      compilerOptions: {
        strict: true,
        module: "node16",
        moduleResolution: "node16",
        target: "es2020",
        jsx: "react-jsx",
        jsxImportSource: "@quadraui/node",
        types: ["node"],
        outDir: "out",
      },
      files: ["counter.tsx"],
    },
    null,
    2,
  ),
);
run(process.execPath, [tsc, "-p", "."], consumer);
fs.writeFileSync(
  path.join(consumer, "hero_headless.cjs"),
  `
const { Counter } = require("./out/counter.js");
const { App } = require("@quadraui/node/app");
const { harness } = require("@quadraui/node/testing");
const counter = new Counter();
const h = harness(new App(30, 5, { view: () => counter.view() }));
if (!h.contains("Count: 0")) throw new Error(h.screen());
for (const label of ["+", "-", "+"]) h.clickText(label);
if (!h.contains("Count: 1")) throw new Error(h.screen());
console.log(h.screen());
console.log("hero headless: ok");
`,
);
console.log(node("hero_headless.cjs").trim());

// --- 5. hero counter, live TUI ---------------------------------------------

function hasTmux(): boolean {
  return !isWindows && spawnSync("tmux", ["-V"], { encoding: "utf8" }).status === 0;
}

function sleep(ms: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
}

if (hasTmux()) {
  const socket = `quadraui-pack-smoke-${process.pid}`;
  const tmux = (...argv: string[]) => spawnSync("tmux", ["-L", socket, "-f", "/dev/null", ...argv], { encoding: "utf8" });
  const pane = () => tmux("capture-pane", "-p", "-t", "smoke").stdout ?? "";
  const waitFor = (needle: string, what: string) => {
    for (let i = 0; i < 100; i++) {
      if (pane().includes(needle)) return;
      sleep(100);
    }
    tmux("kill-server");
    fail(`live TUI: timed out waiting for ${what}; pane:\n${pane()}`);
  };
  try {
    const command = `cd ${JSON.stringify(consumer)} && ${JSON.stringify(process.execPath)} out/counter.js; echo "EXIT:$?"; sleep 30`;
    const started = tmux("new-session", "-d", "-s", "smoke", "-x", "40", "-y", "10", command);
    if (started.status !== 0) fail(`tmux new-session failed: ${started.stderr}`);
    waitFor("Count: 0", "the counter to paint");
    tmux("send-keys", "-t", "smoke", "q");
    waitFor("EXIT:", "the counter to exit on q");
    const final = pane();
    if (!final.includes("EXIT:0")) {
      tmux("kill-server");
      fail(`live TUI: counter exited non-zero; pane:\n${final}`);
    }
  } finally {
    tmux("kill-server");
  }
  console.log("hero live TUI: ok");
} else if (requireLive) {
  fail("--live given but tmux is not available");
} else {
  console.log("hero live TUI: SKIPPED (no tmux on this host)");
}

if (keep) console.log(`kept ${work}`);
else fs.rmSync(work, { recursive: true, force: true });
console.log("pack smoke: ok");
