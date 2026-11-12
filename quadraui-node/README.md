# @quadraui/node

Node.js bindings (via [napi-rs](https://napi.rs/)) for
[quadraui](https://github.com/JDonaghy/quadraui), a cross-platform Rust UI
toolkit. This package lets a Node/TypeScript program build `quadraui`
descriptors and drive them headlessly, or take over a real terminal/native
window with them.

```ts
const { QuiApp } = require("@quadraui/node");
```

## Version compatibility

This package's minor version always equals the `quadraui` Rust crate's own
minor version; the patch component moves independently in either direction
(see
[`BINDING_VERSION_POLICY.md`](https://github.com/JDonaghy/quadraui/blob/main/quadraui/docs/decisions/BINDING_VERSION_POLICY.md)).
`@quadraui/node` is not yet published to npm (tracked by #2239) — this
package stays `"private": true` until then — but its `package.json`
`"version"` field already follows this table:

| `quadraui` (Rust crate) | `@quadraui/node` |
|---|---|
| 0.*Y*.x | 0.*Y*.x — any patch |

For example, both `quadraui` crate `0.10.1` and `0.10.3` pair with any
`@quadraui/node` `0.10.x` version; a crate `0.11.0` needs a `0.11.x`
`package.json` bump, not `0.10.x`.

## Building and testing an app

`@quadraui/node/app` (`js/app/`) is a thin layer over `QuiApp`: a `view()`
returning typed node trees (`Split`, `Tree`, `Button`, ...), `on*` callbacks
on those nodes instead of one app-wide event switch, `Component` for
reusable view + local state, and `App.enableHotReload(...)` to pick up
edits to your view code while the app runs, keeping the state you built up.
See `js/app/index.ts`'s own module doc for the full contract, and
`examples/file_manager_app.ts` for a worked example.

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
