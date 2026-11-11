// `Component`: a reusable unit of view + local state, usable as a node
// wherever a `Node`/plain wire object is accepted.
//
// A fresh `new SomeComponent()` is typically constructed every frame
// (inline, inside `view()`, the same way a node constructor is called
// every frame) -- it is cheap and holds no state of its own on `this`. Its
// *state* instead lives in a plain object `./runtime.ts`'s `App` owns and
// keys by this component's position in the tree (or its explicit `key`,
// when a list of components needs one that survives reordering):
// `view(state)` reads and mutates that object directly, and the next
// render passes the same object back in, so a `state.count += 1` inside an
// event callback survives to the component's next `view()` call exactly
// like a plain app's own instance fields would.
//
// No `Proxy`/reactive-wrapper machinery sits between a callback's mutation
// and the next render: `QuiApp` already repaints once per `step()` whenever
// a render callback is registered (same as `../index.ts`'s own `run`/`pump`
// doc), so a plain mutable object is all the "redraw on mutation" story
// needs -- there is nothing to intercept a write for, unlike a framework
// that only repaints when told a value changed.
//
// Runtime exports go through `exports.X = ...` rather than `export`, the
// same reason `../index.ts`'s own header comment gives.

import type { Renderable } from "./nodes";

/** One `Component`'s local-state object -- an app or callback may put whatever shape it wants in here. */
export type ComponentState = Record<string, unknown>;

/**
 * Subclass and override `initialState()` (the state object's shape the
 * first time this component's `key`/position is ever seen) and
 * `view(state)` (returns whatever this component renders as -- a `Node`,
 * another `Component`, a raw wire object, or `null`/`undefined`).
 *
 * `key`, if given to the constructor, identifies this component's state
 * slot directly, instead of the (constructor, tree-position) pair
 * `./runtime.ts`'s `walk` otherwise derives -- needed only when a list of
 * components can reorder (state should follow the item, not the slot it
 * happened to render into this frame).
 */
class Component {
  key: unknown;

  constructor(key?: unknown) {
    this.key = key;
  }

  initialState(): ComponentState {
    return {};
  }

  view(_state: ComponentState): Renderable {
    throw new Error("Component subclasses must override view()");
  }
}
exports.Component = Component;
