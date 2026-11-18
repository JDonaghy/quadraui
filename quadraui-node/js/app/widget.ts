// `defineWidget`: the app layer's counterpart to a hand-written
// `Primitive` -- a script-authored node whose own `measure`/`paint`/
// `onEvent`/`a11y` build the display list directly (reach for `Component`
// first; composing existing nodes covers almost everything a `Component`
// needs). Wires its four callbacks through
// `quadraui_binding_core::scripted::ScriptedPrimitive`, the same seam
// `quadraui-py`'s `quadraui.app.widget.Widget` wraps, via this binding's
// own `registerScriptedWidget` native export (`../index.ts`) -- see
// `quadraui-node/src/lib.rs`'s "Script widgets" module section for the
// full contract that native call relies on.
//
// **Local state, the `Component` way.** Unlike `quadraui-py`'s `Widget`
// (whose own `state` is just a plain dict the *app* has to persist
// itself, re-passed in on every `view()` call -- see that class's own
// doc), the object `defineWidget`'s returned factory builds satisfies
// `./nodes.ts`'s `ComponentLike` shape: `./runtime.ts`'s `walk` already
// knows how to key a `ComponentLike`'s state by its `key`/tree position
// and thread the very same object back in on every later render (see that
// module's own doc). A widget's `onEvent(event, props, state)` mutates
// `state` in place -- no return value needed -- and that mutation
// survives to the next render exactly like a `Component`'s own `view`
// callback mutating `state` does. This state is separate from (and,
// unlike `quadraui-py`'s wrapped `ScriptedPrimitive`, never round-tripped
// through) the lower-level `props`/`state` fields `ScriptedPrimitive`
// itself carries: `build` below closes over this frame's `props`/`state`
// directly in the four callbacks it hands to `registerScriptedWidget`,
// so there is nothing for that lower seam's own `props`/`state` arguments
// to add.
//
// **The JSX intrinsic.** `defineWidget`'s returned factory already
// satisfies `../jsx-runtime.ts`'s `FunctionComponent` shape (an options
// object in, a `Renderable` out), so a capitalized JSX element already
// works with no further wiring: `<RatingControl id="..." value={3}
// onRate={...} />`. `../jsx-runtime.ts`'s own lowercase `widget` intrinsic
// is the ad hoc counterpart -- a one-off script widget with no named
// component of its own, `<widget measure={...} paint={...} props={...}
// />` -- and calls straight back into `defineWidget` here rather than
// duplicating `build`'s own registration logic.

import type { ComponentLike, Node } from "./nodes";
import type {
  CanvasEvent as WireCanvasEvent,
  CanvasRegion,
  DrawOp,
  Key as WireKey,
} from "../wire";

const nodeIndex = require("../index.ts") as {
  registerScriptedWidget(
    id: string,
    measure: (packed: unknown) => unknown,
    paint: (packed: unknown) => unknown,
    onEvent: (packed: unknown) => unknown,
    a11y: (packed: unknown) => unknown,
    focusable: boolean,
  ): void;
};

/** `measure`/`paint`'s own render-time facts -- the napi binding's own packed `ctx`, camelCased. */
export interface WidgetLayoutCtx {
  lineHeight: number;
  charWidth: number;
  nerdFontsEnabled: boolean;
}

/** Available space `measure` sizes into. `null` on an axis means unconstrained on that axis. */
export interface WidgetConstraints {
  maxWidth: number | null;
  maxHeight: number | null;
}

/** This widget's own resolved area, in `ctx`'s units -- `paint`'s own `(0, 0)` is this rect's own top-left corner, not the screen's. */
export interface WidgetRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** `measure`'s own answer, in `ctx`'s units. */
export interface WidgetSize {
  width: number;
  height: number;
}

/**
 * `paint`'s own answer -- the same `ops`/`regions` shape
 * `../wire.ts`'s own `CanvasDescriptor` carries. Every coordinate is
 * LOCAL to the `rect` `paint` was given, not the screen.
 */
export interface WidgetPaint {
  ops: DrawOp[];
  regions?: CanvasRegion[];
}

/**
 * A region click/double-click (the same payload shape a plain `canvas`
 * node's own region event carries, minus the leading widget id -- this
 * widget already knows which one it is) or a key pressed while this
 * widget holds focus.
 */
export type WidgetEvent = { Canvas: WireCanvasEvent } | { Key: WireKey };

/**
 * An optional accessibility node this widget contributes:
 * `info`'s fields mirror `quadraui::A11yInfo` (every field optional,
 * defaulting to "not asserted" -- `a11y_role`, `a11y_label`, `value`,
 * `value_range`, ...); `children` nests further `A11yChild`-shaped nodes.
 * `null` reports nothing -- correct for a widget with no accessible
 * surface of its own.
 */
export interface WidgetA11yNode {
  info: Record<string, unknown>;
  children?: unknown[];
}

/**
 * A `defineWidget` spec: `measure`/`paint` are required (mirroring
 * `quadraui_binding_core::scripted::ScriptWidget`'s own required
 * methods); `initialState`/`onEvent`/`a11y` default to "no local state",
 * "consumes nothing", and "nothing to report", respectively.
 */
export interface WidgetSpec<P = Record<string, unknown>, S = Record<string, unknown>> {
  /** This widget's local-state object the first time its `key`/position is ever seen -- `{}` if omitted. */
  initialState?(props: P): S;
  /** Sizes this widget for the layout solver. */
  measure(ctx: WidgetLayoutCtx, props: P, constraints: WidgetConstraints): WidgetSize;
  /** Builds this frame's display list and hit-testable regions. */
  paint(ctx: WidgetLayoutCtx, props: P, rect: WidgetRect): WidgetPaint;
  /** Handles a region click/double-click or a focused key press. Returns whether this widget consumed it. */
  onEvent?(event: WidgetEvent, props: P, state: S): boolean;
  /** This widget's own accessibility node, or `null`/`undefined` to report nothing. */
  a11y?(props: P): WidgetA11yNode | null | undefined;
}

/** Framework-reserved keys every `defineWidget` factory call accepts alongside its own domain props. */
export interface WidgetFrameworkOptions {
  /** This widget's own wire id, reused every frame -- an auto-id (position-derived, see `./runtime.ts`'s own doc) works but is fragile the same way it is for any other node; give one explicitly for a widget with declared `CanvasRegion`s. */
  id?: string;
  /** Identifies this widget's local-state slot directly, instead of the (factory, tree-position) pair `./runtime.ts`'s `walk` otherwise derives -- needed only when a list of widgets can reorder. */
  key?: unknown;
  /** Whether this widget is a tab stop -- `./runtime.ts`'s `walk` reads this the same way it reads any other focusable leaf's declared flag. */
  focusable?: boolean;
}

/** A `defineWidget(spec)` factory's own parameter: domain props plus the three framework-reserved keys above. */
export type WidgetOptions<P> = P & WidgetFrameworkOptions;

/**
 * Builds the `Node` `view(state)` returns: `build(resolvedId, ...)`
 * registers this frame's four callbacks (closing over `props`/`state`
 * directly -- see this module's own header doc) against `resolvedId` and
 * returns the `"scripted_widget"` wire leaf
 * `quadraui_binding_core::scripted`'s own `"scripted_widget"` decode arm
 * resolves back into the `ScriptedPrimitive` this call just registered.
 */
function buildWidgetNode<P, S>(
  id: string | undefined,
  props: P,
  state: S,
  focusable: boolean,
  spec: WidgetSpec<P, S>,
): Node {
  return {
    kind: "widget",
    id: id ?? null,
    callbacks: {},
    children: {},
    build(resolvedId: string): Record<string, unknown> {
      nodeIndex.registerScriptedWidget(
        resolvedId,
        (packed: unknown) => {
          const { ctx, constraints } = packed as {
            ctx: WidgetLayoutCtx;
            constraints: WidgetConstraints;
          };
          return spec.measure(ctx, props, constraints);
        },
        (packed: unknown) => {
          const { ctx, rect } = packed as { ctx: WidgetLayoutCtx; rect: WidgetRect };
          return spec.paint(ctx, props, rect);
        },
        (packed: unknown) => {
          if (spec.onEvent === undefined) {
            return false;
          }
          return spec.onEvent(packed as WidgetEvent, props, state);
        },
        () => {
          if (spec.a11y === undefined) {
            return null;
          }
          return spec.a11y(props) ?? null;
        },
        focusable,
      );
      return { id: resolvedId, scripted_widget: {} };
    },
  };
}

/**
 * Builds the `ComponentLike` a `defineWidget` factory call (or
 * `../jsx-runtime.ts`'s `widget` intrinsic) returns -- shared by both so
 * neither duplicates the `initialState`/`view` keying contract.
 */
function widgetComponent<P, S>(
  spec: WidgetSpec<P, S>,
  id: string | undefined,
  key: unknown,
  focusable: boolean,
  props: P,
): ComponentLike {
  return {
    key: key !== undefined ? key : id,
    initialState(): Record<string, unknown> {
      return (spec.initialState ? spec.initialState(props) : {}) as Record<string, unknown>;
    },
    view(state: Record<string, unknown>): Node {
      return buildWidgetNode(id, props, state as S, focusable, spec);
    },
  };
}

/**
 * Defines a reusable script-authored widget: `defineWidget({measure,
 * paint, onEvent, a11y})` returns a factory usable as a node constructor
 * (`RatingControl({id: "rating:stars", value: 3, onRate: fn})`) or
 * directly as a JSX function component (`<RatingControl id="rating:stars"
 * value={3} onRate={fn} />` -- see this module's own header doc, "The JSX
 * intrinsic"). `options` is this widget's own domain props plus
 * `id`/`key`/`focusable` (`WidgetFrameworkOptions`); everything else is
 * forwarded to `spec.measure`/`paint`/`onEvent`/`a11y` as `props`,
 * unchanged, every frame.
 */
function defineWidget<P extends object, S = Record<string, unknown>>(
  spec: WidgetSpec<P, S>,
): (options: WidgetOptions<P>) => ComponentLike {
  return function Widget(options: WidgetOptions<P>): ComponentLike {
    const { id, key, focusable, ...rest } = options as WidgetFrameworkOptions & Record<string, unknown>;
    return widgetComponent(spec, id, key, focusable ?? false, rest as P);
  };
}
exports.defineWidget = defineWidget;

/**
 * `../jsx-runtime.ts`'s own `widget` intrinsic adapter calls this
 * directly -- an ad hoc script widget with no named `defineWidget`
 * component of its own, props passed as one nested bag (matching
 * `quadraui-py`'s own `Widget` constructor shape) rather than flattened
 * the way `defineWidget`'s returned factory accepts them, since there is
 * no `spec`'s own prop names to flatten against here.
 */
function widgetFromIntrinsic<P extends object, S = Record<string, unknown>>(
  spec: WidgetSpec<P, S>,
  options: WidgetFrameworkOptions & { props?: P },
): ComponentLike {
  return widgetComponent(spec, options.id, options.key, options.focusable ?? false, (options.props ?? {}) as P);
}
exports.widgetFromIntrinsic = widgetFromIntrinsic;
