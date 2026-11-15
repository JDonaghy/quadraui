# @quadraui/node

Node.js bindings (via [napi-rs](https://napi.rs/)) for
[quadraui](https://github.com/JDonaghy/quadraui), a cross-platform Rust UI
toolkit. This package lets a Node/TypeScript program build `quadraui`
descriptors and drive them headlessly, or take over a real terminal/native
window with them.

```ts
const { QuiApp } = require("@quadraui/node");
```

See *Install* below for how to get `@quadraui/node` itself.

## Install

```bash
npm install @quadraui/node
```

`npm install` pulls in a prebuilt native addon for the installing machine's
platform as an optional dependency — see *Prebuilt binaries* below for which
platforms have one, and what happens on a platform that doesn't.

### From source

A platform with no prebuilt binary (Linux arm64, musl, macOS x86_64, Windows
arm64, …), or a local change to the Rust side of this package, needs a
build from source instead of `npm install @quadraui/node`:

```bash
git clone https://github.com/JDonaghy/quadraui.git
cd quadraui/quadraui-node
npm install   # pulls this package's own devDependencies, not @quadraui/node itself
npm run build # napi build --platform --release — compiles the Rust addon for this machine
```

`npm run build` needs a Rust toolchain (`cargo`/`rustc`, usually installed
via `rustup`) on `PATH`; everything else it needs (`@napi-rs/cli`,
`typescript`, …) comes from `npm install`. Use `npm run build:debug`
instead for a faster, unoptimized build while iterating locally.

## Run

The examples live in this repo's `examples/` directory, which is not part of
the published npm package — running them means a clone of the repo, built
from source as above (`### From source`), not a plain `npm install
@quadraui/node` into your own project.

Every `.ts` file directly under `examples/` runs on its own, with no
separate compile step, once `npm run build` has built the native addon:

```bash
node examples/appshell_demo.ts
node examples/file_manager_app.ts
node examples/sample_app.ts
```

(`examples/file_manager_app_jsx.tsx` is the one exception — Node doesn't
type-strip `.tsx`, so it needs a transform first; see *JSX* below.
`examples/hot_reload/notes_view.ts` is a module another example requires,
not a runnable demo on its own.)

Running directly relies on Node's own native TypeScript support (type
stripping — no `ts-node`/`tsx`/`esbuild-register` needed); see
*CommonJS vs. ESM — the `.ts`/`require` constraint* below for the one thing
that support doesn't do, and why every file here works around it the same
way.

`npm test` runs this package's own `node --test` suite
(`tests/*.test.ts`), and `npm run typecheck:examples` type-checks every
`.ts` example with `tsc --strict` without running any of them (the `.tsx`
example is covered separately by `npm run typecheck:jsx`; see *JSX* below).

## Version compatibility

This package's minor version always equals the `quadraui` Rust crate's own
minor version; the patch component moves independently in either direction
(see
[`BINDING_VERSION_POLICY.md`](https://github.com/JDonaghy/quadraui/blob/main/quadraui/docs/decisions/BINDING_VERSION_POLICY.md)).
A released `quadraui` version is always `npm install`-able as the matching
`@quadraui/node` minor:

| `quadraui` (Rust crate) | `@quadraui/node` |
|---|---|
| 0.*Y*.x | 0.*Y*.x — any patch |

For example, both `quadraui` crate `0.10.1` and `0.10.3` pair with any
`@quadraui/node` `0.10.x` version; a crate `0.11.0` needs a `0.11.x`
`package.json` bump, not `0.10.x`.

## Prebuilt binaries

`npm install @quadraui/node` installs one of three prebuilt native addons as
an optional dependency, matching the installing machine's platform:

- **macOS (arm64)** — `@quadraui/node-darwin-arm64`
- **Linux (x86_64, glibc)** — `@quadraui/node-linux-x64-gnu`
- **Windows (x86_64)** — `@quadraui/node-win32-x64-msvc`

All three are headless/TUI builds — `startNative` raises `NoNativeBackendError`
rather than opening a real window on any of them, the same as building from
source with no native-window feature enabled. A different platform (Linux
arm64, musl, macOS x86_64, Windows arm64, etc.) has no prebuilt binary;
`npm install` still succeeds
there (optional dependencies that don't match the install platform are
silently skipped) but the generated loader then fails at runtime with
"Failed to load native binding" — see *Install > From source* above.

## CommonJS vs. ESM — the `.ts`/`require` constraint

This package's `package.json` sets `"type": "commonjs"`, and every example
and in-repo consumer (`js/app/runtime.ts`, every test) loads this package's
own TypeScript sources with `require("../js/index.ts")`, `.ts` extension
and all, instead of `import`. Both of those are load-bearing, not stylistic:

- **Node resolves a standalone `.ts` file's module format the same way it
  resolves a `.js` file's** — by the nearest `package.json`'s `"type"`
  field. With `"type": "commonjs"` (this package's own setting, and the
  default when a `package.json` omits `"type"` entirely), a top-level
  `import`/runtime `export` statement in a `.ts` file run directly is a
  **hard parse error**, not something Node silently transpiles into
  `require`/`module.exports` for you. `export type`/`import type` are
  exempt — they're erasable (type-only, no runtime value), so they
  disappear during type-stripping rather than needing ESM semantics — but
  a value-level `import`/`export` is not.
- That's why `js/index.ts`, every file under `examples/`, and this
  package's own tests all use `require`/`exports.X = ...` for runtime
  values, reserving `import type`/`export type` for types only: it's the
  one shape that runs unchanged under Node's native TypeScript support
  (type stripping — see
  [Node's TypeScript documentation](https://nodejs.org/api/typescript.html))
  with **no compile step**, which is what lets `node examples/appshell_demo.ts`
  just work. This repo's CI runs that support on Node 24; consult Node's own
  docs for the minimum version your Node needs it unflagged.

A consumer writing plain `.js`/pre-compiled TypeScript against the
*published* package is unaffected either way: `require("@quadraui/node")`
and `import { QuiApp } from "@quadraui/node"` both work, because the
published `main`/`exports` entry point (`dist/index.js`, compiled by this
package's own `prepack` hook) is a plain compiled CommonJS file Node's
ESM loader already knows how to import named exports from. The constraint
above only bites a file in *this repo* that `require()`s a sibling `.ts`
file directly, or a consumer who copies that same pattern into their own
`"type": "commonjs"` project to run TypeScript with no build step at all.

## Building and testing an app

`@quadraui/node/app` (`js/app/`) is a thin layer over `QuiApp`: a `view()`
returning typed node trees (`Split`, `Tree`, `List`, `Form`, `Button`, ...),
`on*` callbacks on those nodes instead of one app-wide event switch,
`App.confirm`/`App.alert` for an in-canvas dialog that resolves a promise
instead of a hand-written `MessageDialogResolved` branch, `Component` for
reusable view + local state, and `App.enableHotReload(...)` to pick up
edits to your view code while the app runs, keeping the state you built up.
See `js/app/index.ts`'s own module doc for the full contract,
`examples/sample_app.ts` for the smallest list + detail form + confirm
dialog + status bar app that uses all of those, and
`examples/file_manager_app.ts` for a larger worked example.

`js/testing.ts` is the harness you write that app's own tests against — it
queries the frame the app actually rendered and synthesises the gestures a
user performs, so a test never spells out a wire event or a coordinate.

## JSX

`@quadraui/node` ships a custom JSX runtime — no React dependency — so
`<Split ratio={0.3}><Tree rows={rows} onActivate={open}/><Table .../></Split>`
compiles straight to the app layer's typed node trees. It works with
`tsc`'s own `"jsx": "react-jsx"`, esbuild's `jsx: "automatic"`, and Bun's
built-in JSX transform — any tool that implements the standard
["automatic" JSX runtime](https://www.typescriptlang.org/docs/handbook/jsx.html#using-the-react-jsx-and-react-jsxdev-factories)
contract.

### `tsconfig.json`

```json
{
  "compilerOptions": {
    "jsx": "react-jsx",
    "jsxImportSource": "@quadraui/node"
  }
}
```

That's the whole setup: no plugin, no Babel preset. `tsc --strict` then
type-checks a `.tsx` file's own elements against the generated wire types
the same way it checks a direct `Tree(rows, {...})` call — a wrong prop name
(`<Tree row={...} />` instead of `rows`) fails `tsc` the same way the
equivalent typo in a direct constructor call would.

`@quadraui/node/jsx-runtime` and `@quadraui/node/jsx-dev-runtime` resolve
only through this package's `exports` map, which `tsc`'s legacy
`"moduleResolution": "node"`/`"node10"` ignores — set `"node16"`,
`"nodenext"`, or `"bundler"` instead, or TS reports `TS2307` on the
implicit `<jsxImportSource>/jsx-runtime` import the transform emits.

esbuild needs the same two settings passed as build options instead (or via
the `tsconfig.json` above, which esbuild also reads):

```ts
esbuild.build({
  jsx: "automatic",
  jsxImportSource: "@quadraui/node",
  // ...
});
```

Bun picks up `tsconfig.json`'s `jsx`/`jsxImportSource` automatically; no
separate Bun-specific configuration is needed.

**Bundling (not just transforming) with esbuild**: esbuild's own
module-type inference mishandles `js/app/nodes.ts` once it sees a sibling
`export interface` in that file — it infers the whole module is ESM and
emits a namespace with no callable values, so a *bundled* esbuild build
hits `nodeConstructors.Split is not a function` at runtime even though the
JSX itself transforms cleanly. This is a property of `js/app/nodes.ts`'s
own `exports.X = ...` convention, not of the JSX runtime; it only shows up
once esbuild inlines that file instead of leaving it to `require` at
runtime. Transform-only (`esbuild.transformSync`/`.transform`, or a build
with `js/app/*` left external) avoids it.

### Intrinsic elements vs. components

A lowercase tag (`<split>`, `<tree>`, `<button>`, ...) is an *intrinsic
element*: one entry per layout-tree node and leaf descriptor the app layer's
own node constructors (`js/app/nodes.ts`) build, typed from the same
generated wire types (`js/wire.d.ts`) those constructors already use — see
`js/jsx-runtime.ts`'s own `IntrinsicElements`/`*Props` declarations for the
full list and exact prop shapes. A capitalized tag is either:

- a plain function component — `function Row(props: {label: string}) {
  return <row><button label={props.label}/></row>; }` — called with its own
  `props` object (`children` included), or
- a `Component` subclass (`js/app/component.ts`) — instantiated with `new`,
  exactly like using `new MyComponent()` directly as a node.

`Tab(...)` and `Toolbar(...)` (`js/app/nodes.ts`) are called as plain
functions inside a JSX expression (`<tabs>{tabs.map((t) => Tab(<table
.../>, {title: t.name}))}</tabs>`), not written as JSX elements themselves —
each builds a data record (a tab-strip entry, a toolbar descriptor), not a
layout-tree node, so there is nothing for `<tab>`/`<toolbar>` markup to mean
on its own.

`examples/file_manager_app_jsx.tsx` is a JSX port of
`examples/file_manager_app.ts`, behaviour-identical to it — see that file's
own header doc, and run `npm run typecheck:jsx` to type-check it (needs
`npm run build:debug` first, since the app layer's own types resolve
through the generated native-addon `index.d.ts`).
