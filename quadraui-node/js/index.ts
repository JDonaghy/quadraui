// Hand-maintained facade over the native napi-rs addon this package builds
// at `../index.js`/`../index.d.ts`/`../*.node` (via `npm run build`, which
// runs `napi build --platform --release` -- see package.json's `build`
// script). Those three are generated build artifacts, not checked in; this
// file is this package's actual `main`/`types` entry, so a consumer's
// import path never names the generated loader directly, and any future
// TS-only ergonomics (overloads, a narrower public type) have exactly one
// place to be added without touching the generated file.
//
// Runtime exports go through `require`/`exports` rather than `import`/
// `export`: Node's native TypeScript support determines a `.ts` file's
// module format the same way it would a `.js` file with this package's
// `"type": "commonjs"`, so top-level `import`/runtime `export` syntax here
// would be a hard parse error, not silently transpiled -- there is no
// `tsc`/bundler step between this file and the Node process that runs it.
// `export type` below is exempt: it is erasable (type-only, no runtime
// value), so it disappears entirely rather than needing ESM semantics.
const native: typeof import("../index.js") = require("../index.js");

/**
 * A headless quadraui app, driven entirely from JavaScript/TypeScript.
 *
 * Construct it with a `width`/`height` cell grid and inspect the rendered
 * frame with `screen()`/`screenContains()`. See the native binding's own
 * doc comment (`quadraui-node/src/lib.rs`'s `QuiApp`) for the full method
 * list and panic/poison behaviour.
 */
export type QuiApp = InstanceType<typeof native.QuiApp>;
exports.QuiApp = native.QuiApp;
