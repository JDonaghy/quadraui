// Black-box coverage for the published TypeScript surface of the error
// taxonomy: `package.json`'s `"types"` points at `dist/index.d.ts`, which
// `build:dist` generates from `js/index.ts`. A runtime error class with no
// matching `export declare class` is invisible to `vimcode`/`coord-tui`:
// naming it, or narrowing a `catch` onto it under `--strict`, fails with
// `TS2305: Module has no exported member`.
//
// These tests read both files as text rather than importing them. Run with
// `npm test`, which builds `dist/` first.

const { test } = require("node:test");
const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { join } = require("node:path");

const runtimeSource: string = readFileSync(
  join(__dirname, "..", "js", "index.ts"),
  "utf8",
);
const typesSource: string = readFileSync(
  join(__dirname, "..", "dist", "index.d.ts"),
  "utf8",
);

function matchAll(source: string, pattern: RegExp): string[][] {
  return [...source.matchAll(pattern)].map((m) => [...m]);
}

/** Every `export class <Name>Error` the runtime facade publishes. */
function runtimeErrorClasses(): string[] {
  return matchAll(runtimeSource, /^export class ([A-Z]\w*Error)\b/gm)
    .map((m) => m[1])
    .sort();
}

/** Every `export declare class <Name>Error` the published types declare. */
function declaredErrorClasses(): string[] {
  return matchAll(typesSource, /^export declare class ([A-Z]\w*Error)\b/gm)
    .map((m) => m[1])
    .sort();
}

/** `Name -> code` for each class that pins a literal `code` tag. */
function codeTags(source: string, assign: string): Map<string, string> {
  const pattern = new RegExp(
    String.raw`class (\w+Error) extends [^{]*\{\s*readonly code${assign} "([^"]+)";`,
    "g",
  );
  return new Map(matchAll(source, pattern).map((m) => [m[1], m[2]]));
}

test("every runtime error class has a matching declaration in index.d.ts", () => {
  assert.deepEqual(declaredErrorClasses(), runtimeErrorClasses());
});

test("DecodeFailedError is part of the published type surface", () => {
  assert.ok(runtimeErrorClasses().includes("DecodeFailedError"));
  assert.ok(declaredErrorClasses().includes("DecodeFailedError"));
});

test("declared code tags match the runtime code tags", () => {
  const runtime = codeTags(runtimeSource, " =");
  const declared = codeTags(typesSource, "(?: =|:)");
  assert.deepEqual([...declared].sort(), [...runtime].sort());
  // Guard against both sides matching because neither parsed: the
  // `DecodeError` taxonomy pins six codes.
  assert.equal(runtime.get("DecodeFailedError"), "borrow");
  assert.equal(declared.get("DecodeFailedError"), "borrow");
  assert.equal(runtime.size, 6);
});
