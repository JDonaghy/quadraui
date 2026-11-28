# @quadraui/node

Node.js bindings (via [napi-rs](https://napi.rs/)) for
[quadraui](https://github.com/JDonaghy/quadraui), a cross-platform Rust UI
toolkit. This package lets a Node/TypeScript program build `quadraui`
descriptors and drive them headlessly, or take over a real terminal/native
window with them.

```ts
import { QuiApp } from "@quadraui/node";
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

Every `.ts` file directly under `examples/` runs on its own once
`npm run build` has built the native addon and `npm run build:dist` has
compiled the package:

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
stripping — no `ts-node`/`tsx`/`esbuild-register` needed) and on the
compiled `dist/`; see *TypeScript, `require` and `import`* below.

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

## TypeScript, `require` and `import`

Every entry point — `@quadraui/node`, `@quadraui/node/app`,
`@quadraui/node/testing`, `@quadraui/node/jsx-runtime` and
`@quadraui/node/jsx-dev-runtime` — is compiled CommonJS in `dist/` with a
generated `.d.ts` beside it, so `require(...)` and `import ... from ...` both
work from `node_modules`, and TypeScript sees the real types:

```ts
import { App, Button } from "@quadraui/node/app";
import { harness } from "@quadraui/node/testing";
```

TypeScript reads the package's `exports` map only with `"module"` set to
`"node16"`, `"nodenext"` or `"bundler"`; the legacy `"node"` resolution
reports `TS2307` for every subpath.

In this repo, the examples and tests are `.ts` files Node runs directly
(type stripping), and they import the package by its own name, so they run
against `dist/`. `npm test` rebuilds `dist/` first; after editing anything
under `js/` before running an example, run `npm run build:dist`.
`scripts/pack_smoke.ts` (`npm run smoke:pack`) packs the root and platform
packages, installs them into a clean project and runs the hero counter
from there, the same check CI runs before a release.

## Building and testing an app

`@quadraui/node/app` is a thin layer over `QuiApp`: a `view()`
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

`@quadraui/node/testing` is the harness you write that app's own tests against — it
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
