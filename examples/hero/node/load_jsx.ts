// Loads `counter.tsx` for the runners in this directory. Node cannot run JSX
// directly, so this compiles it in memory with esbuild (a devDependency of
// `@quadraui/node`) using the same `jsx: "automatic"`,
// `jsxImportSource: "@quadraui/node"` settings the package README documents,
// and resolves `@quadraui/node/*` to this checkout's `quadraui-node/`. An app
// using the published package needs none of this: its own bundler or `tsc`
// compiles the JSX.

const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");

const PKG = path.resolve(__dirname, "../../../quadraui-node");
const pkgRequire = Module.createRequire(path.join(PKG, "package.json"));
const esbuild = pkgRequire("esbuild");

const SUBPATHS: Record<string, string> = {
  "@quadraui/node/app": path.join(PKG, "js/app/index.ts"),
  "@quadraui/node/jsx-runtime": path.join(PKG, "js/jsx-runtime.ts"),
  "@quadraui/node/testing": path.join(PKG, "js/testing.ts"),
};

let jsxRuntime: any = null;

/** Requires a `.ts`/`.tsx` file through esbuild, resolving `@quadraui/node/*` to this checkout. */
function load(file: string, asMain = false): any {
  const { code } = esbuild.transformSync(fs.readFileSync(file, "utf8"), {
    loader: file.endsWith(".tsx") ? "tsx" : "ts",
    format: "cjs",
    target: "es2020",
    jsx: "automatic",
    jsxImportSource: "@quadraui/node",
  });
  const mod = { exports: {} as any };
  const req = (id: string) => {
    if (id === "@quadraui/node/jsx-runtime") return (jsxRuntime ??= load(SUBPATHS[id]));
    if (id in SUBPATHS) return require(SUBPATHS[id]);
    return require(id.startsWith(".") ? path.resolve(path.dirname(file), id) : id);
  };
  // What the module's own `require.main === module` check reads.
  (req as any).main = asMain ? mod : undefined;
  new Function("require", "module", "exports", "__filename", "__dirname", code)(req, mod, mod.exports, file, path.dirname(file));
  return mod.exports;
}

/** `counter.tsx`'s exports; with `asMain`, run it as the entry module (its live TUI `main`). */
function loadHero(asMain = false): any {
  return load(path.join(__dirname, "counter.tsx"), asMain);
}

module.exports = { loadHero, SUBPATHS };
