// Black-box coverage for the package's `exports` map: every subpath a
// consumer can import points at compiled JavaScript with a declaration
// next to it, the root entry and the app layer share one compiled module
// (one set of error classes), and a TypeScript consumer gets real types
// from `@quadraui/node/app`. `scripts/pack_smoke.ts` repeats these checks
// from a `node_modules` install of the packed tarballs.
//
// Run with `npm test`, which builds `dist/` first.
const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const PKG = path.join(__dirname, "..");
const manifest = JSON.parse(fs.readFileSync(path.join(PKG, "package.json"), "utf8"));

/** Every file path an `exports` entry names, with `*` patterns expanded against the files they match. */
function exportTargets(): string[] {
  const targets: string[] = [];
  const visit = (value: unknown) => {
    if (typeof value === "string") targets.push(value);
    else if (value && typeof value === "object") Object.values(value).forEach(visit);
  };
  visit(manifest.exports);
  return targets.flatMap((target) => {
    if (!target.includes("*")) return [target];
    const [prefix, suffix] = target.split("*");
    const dir = path.join(PKG, path.dirname(prefix));
    return fs
      .readdirSync(dir)
      .filter((name: string) => name.endsWith(suffix))
      .map((name: string) => "./" + path.posix.join(path.posix.dirname(prefix), name));
  });
}

test("no exports target is a TypeScript source file, and every target exists", () => {
  for (const target of exportTargets()) {
    assert.ok(target.endsWith(".js") || target.endsWith(".d.ts") || target === "./package.json", `${target} is not compiled output`);
    assert.ok(fs.existsSync(path.join(PKG, target)), `${target} does not exist`);
  }
});

test("every JavaScript exports entry has a types condition next to it", () => {
  for (const [subpath, entry] of Object.entries(manifest.exports)) {
    if (subpath === "./package.json") continue;
    assert.equal(typeof entry, "object", `${subpath} has no conditions`);
    assert.ok((entry as Record<string, string>).types, `${subpath} has no types condition`);
  }
});

test("the root entry and the app layer share one set of error classes", () => {
  const root = require("@quadraui/node");
  const { App } = require("@quadraui/node/app");
  assert.throws(
    () => new App(0, 3, { view: () => null }),
    (err: unknown) => err instanceof root.InvalidDimensionsError && err instanceof root.QuadrauiError,
  );
});

test("every runtime subpath loads through require() and import()", async () => {
  for (const id of ["@quadraui/node", "@quadraui/node/app", "@quadraui/node/testing", "@quadraui/node/jsx-runtime", "@quadraui/node/jsx-dev-runtime"]) {
    const required = require(id);
    const imported = await import(id);
    assert.ok(Object.keys(required).length > 0, `${id} has no exports`);
    for (const name of Object.keys(required)) {
      assert.equal(imported[name], required[name], `${id}: import() is missing ${name}`);
    }
  }
});

test("a TypeScript consumer gets typed App and Button from @quadraui/node/app", () => {
  const tsc = path.join(PKG, "node_modules", "typescript", "bin", "tsc");
  const fixture = path.join(__dirname, "fixtures", "typed_app_consumer.ts");
  execFileSync(
    process.execPath,
    [tsc, "--strict", "--noEmit", "--module", "node16", "--moduleResolution", "node16", "--target", "es2020", "--types", "node", fixture],
    { cwd: PKG, stdio: "pipe" },
  );
});
