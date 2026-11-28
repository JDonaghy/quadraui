// Loads `counter.tsx` for the runners in this directory. Node cannot run JSX
// directly, so this compiles it in memory with esbuild (a devDependency of
// `@quadraui/node`) using the same `jsx: "automatic"`,
// `jsxImportSource: "@quadraui/node"` settings the package README documents,
// and resolves `@quadraui/node/*` to this checkout's compiled
// `quadraui-node/dist/` (`npm run build:dist` there). An app using the
// published package needs none of this: its own bundler or `tsc` compiles
// the JSX.

const fs = require("node:fs");
const path = require("node:path");
const Module = require("node:module");

const PKG = path.resolve(__dirname, "../../../quadraui-node");
const pkgRequire = Module.createRequire(path.join(PKG, "package.json"));
const esbuild = pkgRequire("esbuild");

/** Resolves an `@quadraui/node` specifier the way an installed package would, through its `exports` map. */
function resolvePackage(id: string): string {
  try {
    return pkgRequire.resolve(id);
  } catch (err) {
    throw new Error(`cannot resolve ${id}: build quadraui-node/dist first (cd quadraui-node && npm run build:debug && npm run build:dist)\n${err}`);
  }
}

const SUBPATHS: Record<string, string> = Object.fromEntries(
  ["@quadraui/node/app", "@quadraui/node/jsx-runtime", "@quadraui/node/testing"].map((id) => [id, resolvePackage(id)]),
);

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
    if (id === "@quadraui/node" || id.startsWith("@quadraui/node/")) return require(resolvePackage(id));
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

module.exports = { load, loadHero, SUBPATHS };
