// A custom JSX runtime for `@quadraui/node` -- no React dependency. Pairs
// with `./jsx-dev-runtime.ts` (the dev-mode sibling a bundler's own JSX
// transform picks in development) and the typed app layer in `./app/` --
// see that directory's own module doc (`./app/index.ts`) for `Node`,
// `Component`, and the node constructors (`Split`, `Tree`, `Button`, ...)
// this runtime builds on.
//
// Set `"jsx": "react-jsx"` and `"jsxImportSource": "@quadraui/node"` in
// `tsconfig.json` (or the equivalent esbuild/Bun flag -- see the package
// README's own "JSX" section for exact config) and a compiler/bundler
// rewrites `<Split ratio={0.3}>...</Split>` into
// `jsx(Split, {ratio: 0.3, children: ...})`, resolved against this module.
// `jsx`/`jsxs` and `IntrinsicElements` below are exactly the two halves of
// that contract: `jsx`/`jsxs` are the runtime half (what a JSX element
// actually evaluates to); `IntrinsicElements`/the `*Props` interfaces are
// the type-checking half (what `tsc` allows as an element's own props) --
// both are typed against the very same `*Options` interfaces `./app/nodes`
// already exports, so a prop typo or a wrong value fails `tsc` the same way
// a typo in a direct `Tree(rows, {...})` call would.
//
// Unlike every other module in this package, this file (and its
// `./jsx-dev-runtime.ts` sibling) is written as a genuine ES module --
// real `export`, not this package's usual `exports.X = ...` (see
// `./index.ts`'s own header comment for why that convention exists
// elsewhere). There is no tension with that rule here: a `.tsx` file can
// never run under Node's native TypeScript support regardless of this
// module's own export style (JSX syntax itself isn't valid JS), so nothing
// ever `require()`s this file directly the way the rest of the package's
// own tests do -- every real consumer reaches it exclusively through a
// JSX-aware transform (`tsc`, esbuild, Bun) that already rewrites `import`/
// `export` into whatever its own target module format needs. Writing this
// pair as plain ESM is what makes the auto-import those transforms emit
// (`import { jsx } from "<jsxImportSource>/jsx-runtime"`) resolve a real
// named export instead of failing `tsc`'s own module checking the way a
// `exports.jsx = ...` assignment would (`import`-ing a value out of a
// module that mixes `export interface` with `exports.X = ...` doesn't
// type-check -- only `import type` does, which is why `./app/nodes`'s own
// node constructors are pulled in below through `require`, not `import`).
//
// `./app/nodes`'s own constructors (`Split`, `Column`, ...) take their
// payload positionally (`Split(first, second, options)`); a JSX element
// only ever has one `props` object (`children` included). The
// `INTRINSICS` table below is exactly that adapter, one entry per lowercase
// intrinsic tag, each unpacking `props.children` into the right positional
// slot before calling through. A capitalized JSX element (`<Split>`, a
// consumer's own function, or a `Component` subclass) skips that table
// entirely -- see `createElement`'s own doc below for the three shapes
// `type` can take and how each is handled.
//
// `Tab` and `Toolbar` (`./app/nodes.ts`) are deliberately not intrinsic
// tags here: both return a plain data record (a tab-strip entry, a
// toolbar descriptor), not a `Node`/`Renderable` -- there is no wire
// concept of "one tab" or "one toolbar" on its own, only as part of a
// `Tabs`/`SidebarPanel`. Every JSX element's own type is uniformly
// `JSX.Element` (`Renderable`, see the `declare global` block below), so a
// JSX element that returned something else would silently mistype
// anywhere that result is used. Call `Tab(...)`/`Toolbar(...)` as plain
// functions instead -- `{tabs.map((t) => Tab(<table .../>, {title: t.name}))}`
// -- exactly like the non-JSX app layer already does; `./examples/
// file_manager_app_jsx.tsx` is a worked example.

declare function require(id: string): unknown;

import type {
  ButtonOptions,
  ColumnOptions,
  ComponentLike,
  Node as QuiNode,
  Renderable,
  RowOptions,
  ScrollOptions,
  SidebarPanelOptions,
  SplitOptions,
  StatusBarOptions,
  TableOptions,
  TabsOptions,
  TextDisplayOptions,
  TreeOptions,
} from "./app/nodes";
import type {
  ColumnDescriptor,
  DataRowDescriptor,
  StatusBarSegment,
  TextDisplayLineDescriptor,
  TreeRowDescriptor,
} from "./wire";

// --- node-constructor bindings ---------------------------------------------

/**
 * `./app/nodes`'s own runtime exports, reached through `require` (not
 * `import`) for the reason this file's own header doc gives, with the
 * signature each constructor's own doc (`./app/nodes.ts`) already
 * documents restated here by hand -- `require`'s result is otherwise
 * untyped, so this cast is what gives every call below its real parameter
 * types instead of `any`.
 */
const nodeConstructors = require("./app/nodes.ts") as {
  Split(first: Renderable, second: Renderable, options?: SplitOptions): QuiNode;
  Column(items: Renderable[], options?: ColumnOptions): QuiNode;
  Row(items: Renderable[], options?: RowOptions): QuiNode;
  SidebarPanel(content: Renderable, options?: SidebarPanelOptions): QuiNode;
  Scroll(content: Renderable, options?: ScrollOptions): QuiNode;
  Tabs(tabs: TabEntry[], options?: TabsOptions): QuiNode;
  Tree(rows: TreeRowDescriptor[], options?: TreeOptions): QuiNode;
  Table(columns: ColumnDescriptor[], rows: DataRowDescriptor[], options?: TableOptions): QuiNode;
  StatusBar(leftSegments: StatusBarSegment[], rightSegments: StatusBarSegment[], options?: StatusBarOptions): QuiNode;
  TextDisplay(lines: TextDisplayLineDescriptor[], options?: TextDisplayOptions): QuiNode;
  Button(label: string, options?: ButtonOptions): QuiNode;
};

// --- intrinsic element props (the `tsc`-checked half) ----------------------

/** `Tab`'s own return shape (`./app/nodes.ts`'s internal, unexported `TabSpec`) -- structurally identical, so TS accepts either as the other. */
export interface TabEntry {
  content: Renderable;
  title: string;
  id?: string;
  closable?: boolean;
  isDirty?: boolean;
  isPreview?: boolean;
}

export interface SplitProps extends SplitOptions {
  /** `Split`'s own two positional panes, as JSX children -- exactly two, first then second. */
  children: readonly [Renderable, Renderable];
}

export interface ColumnProps extends ColumnOptions {
  children?: Renderable | readonly Renderable[];
}

export interface RowProps extends RowOptions {
  children?: Renderable | readonly Renderable[];
}

export interface SidebarPanelProps extends SidebarPanelOptions {
  children?: Renderable;
}

export interface ScrollProps extends ScrollOptions {
  children?: Renderable;
}

export interface TabsProps extends TabsOptions {
  /** `Tab(...)` call results -- see this file's own header doc for why `Tab` is a plain function, not a JSX element. */
  children?: TabEntry | readonly TabEntry[];
}

export interface TreeProps extends TreeOptions {
  rows: TreeRowDescriptor[];
}

export interface TableProps extends TableOptions {
  columns: ColumnDescriptor[];
  rows: DataRowDescriptor[];
}

export interface StatusBarProps extends StatusBarOptions {
  leftSegments: StatusBarSegment[];
  rightSegments: StatusBarSegment[];
}

export interface TextDisplayProps extends TextDisplayOptions {
  lines: TextDisplayLineDescriptor[];
}

export interface ButtonProps extends ButtonOptions {
  label: string;
}

/**
 * A local, exported `JSX` namespace -- not `declare global`. TS's automatic
 * JSX transform looks for a `JSX` export on the `jsxImportSource` module
 * before falling back to the ambient global one, so this is enough for
 * `"jsxImportSource": "@quadraui/node"` to resolve every `IntrinsicElements`
 * entry below without ever touching the global namespace -- a program that
 * also uses React/Preact in the same `tsc` run keeps its own `JSX.
 * IntrinsicElements` untouched by this package's.
 */
export namespace JSX {
  export interface IntrinsicElements {
    split: SplitProps;
    column: ColumnProps;
    row: RowProps;
    sidebarPanel: SidebarPanelProps;
    scroll: ScrollProps;
    tabs: TabsProps;
    tree: TreeProps;
    table: TableProps;
    statusBar: StatusBarProps;
    textDisplay: TextDisplayProps;
    button: ButtonProps;
  }
  /** What any JSX expression (`<Foo/>`) evaluates to -- `createElement`'s own return type. */
  export type Element = Renderable;
  export interface ElementChildrenAttribute {
    children: Record<string, never>;
  }
  /**
   * Props every JSX element accepts regardless of its own `*Props`
   * interface. `key` is reachable from the runtime today (`createElement`
   * passes it straight to `new type(key)` for a `Component` class, which is
   * how a reorderable list keeps each instance's own state slot) but was
   * otherwise untyped, so `<MyComponent key={...}/>` failed `tsc` as an
   * unknown prop even though the runtime already supported it.
   */
  export interface IntrinsicAttributes {
    key?: unknown;
  }
}

// --- the runtime half: `jsx`/`jsxs`/`Fragment` ------------------------------

type IntrinsicTag = keyof JSX.IntrinsicElements;

type AnyProps = Record<string, unknown> & { children?: unknown };

/**
 * Normalizes a JSX children value (absent, one, or many) into an array --
 * the shape every `./app/nodes` list-taking constructor (`Column`, `Row`,
 * `Tabs`) wants. Deliberately not generic over the element type: `Renderable`
 * is itself a union that already includes `Renderable[]` as one of its own
 * variants, which makes a generic `T | readonly T[]` parameter ambiguous
 * for TS to infer against -- every call site below casts this function's
 * `unknown[]` result to the array type it actually needs instead.
 */
function childList(value: unknown): unknown[] {
  if (value === undefined) {
    return [];
  }
  return Array.isArray(value) ? [...value] : [value];
}

/**
 * Everything in `props` except `children` -- the options bag every
 * `./app/nodes` constructor's own second/third parameter expects. Not
 * constrained to `AnyProps`: every `*Props` interface above has its own
 * fully-typed fields rather than an index signature, so requiring one here
 * would reject them all.
 */
function withoutChildren<P extends { children?: unknown }>(props: P): Omit<P, "children"> {
  const { children: _children, ...rest } = props;
  return rest as Omit<P, "children">;
}

type IntrinsicBuilder = (props: AnyProps) => Renderable;

/**
 * One adapter per lowercase intrinsic tag: unpacks `props.children` into
 * whichever positional slot the matching `./app/nodes` constructor expects,
 * then forwards every other prop as that constructor's own options object.
 */
const INTRINSICS: Record<IntrinsicTag, IntrinsicBuilder> = {
  split: (props) => {
    const typed = props as unknown as SplitProps;
    const [first, second] = typed.children;
    return nodeConstructors.Split(first, second, withoutChildren(typed));
  },
  column: (props) => {
    const typed = props as unknown as ColumnProps;
    return nodeConstructors.Column(childList(typed.children) as Renderable[], withoutChildren(typed));
  },
  row: (props) => {
    const typed = props as unknown as RowProps;
    return nodeConstructors.Row(childList(typed.children) as Renderable[], withoutChildren(typed));
  },
  sidebarPanel: (props) => {
    const typed = props as unknown as SidebarPanelProps;
    return nodeConstructors.SidebarPanel(typed.children ?? null, withoutChildren(typed));
  },
  scroll: (props) => {
    const typed = props as unknown as ScrollProps;
    return nodeConstructors.Scroll(typed.children ?? null, withoutChildren(typed));
  },
  tabs: (props) => {
    const typed = props as unknown as TabsProps;
    return nodeConstructors.Tabs(childList(typed.children) as TabEntry[], withoutChildren(typed));
  },
  // The remaining tags are leaves -- `TreeProps`/`TableProps`/... carry no
  // `children` field at all (there is nothing for a JSX child to mean), so
  // each is forwarded to its constructor directly, with no
  // `withoutChildren` step.
  tree: (props) => {
    const { rows, ...options } = props as unknown as TreeProps;
    return nodeConstructors.Tree(rows, options);
  },
  table: (props) => {
    const { columns, rows, ...options } = props as unknown as TableProps;
    return nodeConstructors.Table(columns, rows, options);
  },
  statusBar: (props) => {
    const { leftSegments, rightSegments, ...options } = props as unknown as StatusBarProps;
    return nodeConstructors.StatusBar(leftSegments, rightSegments, options);
  },
  textDisplay: (props) => {
    const { lines, ...options } = props as unknown as TextDisplayProps;
    return nodeConstructors.TextDisplay(lines, options);
  },
  button: (props) => {
    const { label, ...options } = props as unknown as ButtonProps;
    return nodeConstructors.Button(label, options);
  },
};

/** Marks a JSX fragment (`<>...</>`): `createElement` unwraps it to its own `children`, unchanged -- there is no wire node for a fragment itself. */
export const Fragment: unique symbol = Symbol("quadraui.jsx.Fragment");

/** A JSX function component: receives the element's own props (`children` included) and returns whatever it renders as. */
export type FunctionComponent = (props: AnyProps) => Renderable;

/** A JSX class component: the app layer's own `Component` (`./app/component.ts`) or any subclass of it -- see `createElement`'s own doc for why this is checked by shape, not `instanceof`. */
export type ComponentClass = new (key?: unknown) => ComponentLike;

export type ElementType = IntrinsicTag | typeof Fragment | FunctionComponent | ComponentClass;

/**
 * Duck-types a JSX `type` as a `Component` subclass (`./app/component.ts`)
 * without importing that module -- the same reason `./app/nodes.ts`'s own
 * `ComponentLike` is matched by shape there, restated here for a
 * constructor instead of an instance: a `view`/`initialState` pair on the
 * prototype is enough, since only `./app/component.ts`'s own `Component`
 * and its subclasses ever carry both.
 */
function isComponentClass(type: unknown): type is ComponentClass {
  if (typeof type !== "function") {
    return false;
  }
  const proto = (type as { prototype?: unknown }).prototype;
  return (
    typeof proto === "object" &&
    proto !== null &&
    typeof (proto as ComponentLike).view === "function" &&
    typeof (proto as ComponentLike).initialState === "function"
  );
}

/**
 * Resolves one JSX element (`<Type {...props}/>`) into a {@link Renderable},
 * the same tree shape `./app/runtime.ts`'s own `walk` already consumes --
 * so a JSX tree needs no runtime support beyond this function; `App`'s own
 * render path is unchanged. `type` is exactly one of:
 *
 * - the {@link Fragment} marker -- returns `props.children` unwrapped;
 * - a lowercase intrinsic tag (`"split"`, `"tree"`, ...) -- looked up in
 *   {@link INTRINSICS} and adapted onto the matching `./app/nodes`
 *   constructor;
 * - a `Component` subclass (checked by {@link isComponentClass}) -- `new
 *   type(key)`, the same shape `./app/nodes.ts`'s own `ComponentLike` doc
 *   says a node constructor can return;
 * - any other function -- a plain JSX function component, called with its
 *   own `props` directly.
 */
function createElement(type: ElementType, props: AnyProps, key?: unknown): Renderable {
  if (type === Fragment) {
    return (props.children as Renderable | undefined) ?? null;
  }
  if (typeof type === "string") {
    // `hasOwnProperty`, not a bare `INTRINSICS[type]` lookup: `INTRINSICS`
    // is a plain object literal, so an inherited `Object.prototype` key
    // (e.g. `"toString"`) would otherwise slip past an `=== undefined`
    // check and silently call through to `Object.prototype.toString`
    // instead of hitting the "unknown intrinsic" error below.
    if (!Object.prototype.hasOwnProperty.call(INTRINSICS, type)) {
      throw new TypeError(`unknown quadraui JSX intrinsic element: <${type}>`);
    }
    return INTRINSICS[type as IntrinsicTag](props);
  }
  if (isComponentClass(type)) {
    return new type(key);
  }
  if (typeof type === "function") {
    return (type as FunctionComponent)(props);
  }
  throw new TypeError(`not a renderable quadraui JSX element type: ${String(type)}`);
}

/**
 * Called by `tsc`'s `"jsx": "react-jsx"`, esbuild's `jsx: "automatic"`, and
 * Bun's own built-in JSX transform for an element with zero or one
 * children.
 */
export function jsx(type: ElementType, props: AnyProps, key?: unknown): Renderable {
  return createElement(type, props, key);
}

/** The same transform's own call for an element with two or more (statically known) children -- identical here, since {@link createElement} never distinguishes the two. */
export const jsxs: typeof jsx = jsx;
