// `./jsx-runtime.ts`'s dev-mode sibling: the module a bundler's JSX
// transform resolves instead when told to emit "development" output (`tsc`
// has no separate dev/prod `jsx` setting of its own, but esbuild's
// `jsxDev: true` and Bun's dev-mode default both pick this file over
// `./jsx-runtime.ts`). The one entry point, `jsxDEV`, takes three extra
// diagnostic-only parameters no caller here needs -- a source-location
// object and the lexical `this` a transform captures for a better runtime
// warning -- and otherwise behaves exactly like `./jsx-runtime.ts`'s own
// `jsx`/`jsxs`.
//
// A real `import` of `jsx` (not the `require`-plus-manual-cast `./jsx-
// runtime.ts` itself needs for `./app/nodes`) is enough here: `jsx-
// runtime.ts` is a genuine ES module with a true `export function jsx`, so
// its own type and value are both visible this way -- see that file's own
// header doc for why pulling a value out of `./app/nodes` the same way
// does not type-check instead.
import { Fragment, jsx } from "./jsx-runtime";
import type { ElementType } from "./jsx-runtime";

export { Fragment };

/**
 * The dev-mode entry point every one of this pair's own callers (`tsc`,
 * esbuild, Bun) emits in place of `jsx`/`jsxs` when building for
 * development. `isStaticChildren`, `source`, and `self` are each purely
 * diagnostic (a transform's own hint that an element's children were a
 * literal JSX list, its file/line/column, and its lexical `this`) --
 * useful for a richer runtime warning on a malformed tree, not needed by
 * this runtime's own element resolution (`./jsx-runtime.ts`'s
 * `createElement`), so they are accepted and ignored rather than threaded
 * through.
 */
export function jsxDEV(
  type: ElementType,
  props: Record<string, unknown>,
  key?: unknown,
  _isStaticChildren?: boolean,
  _source?: unknown,
  _self?: unknown,
): unknown {
  return jsx(type, props, key);
}
