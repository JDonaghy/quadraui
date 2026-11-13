// Typed node constructors: small functions that build a `Node` -- a tiny,
// backend-agnostic description of one layout-tree element -- instead of a
// raw wire dict. Each constructor takes the same fields the matching
// `../wire` type names, plus an options object carrying whichever `on*`
// callback keys the element's own `UiEvent` shapes can raise (see
// `./runtime.ts` for how those get dispatched). `Node.build` closes over
// everything except its children, so `./runtime.ts`'s `walk` only has to
// resolve children and an id before calling it.
//
// A constructor's payload parameters (`rows`, `columns`, `style`, ...) are
// typed against the generated wire types (`../wire.d.ts`) directly, rather
// than left as `unknown`/a bare object -- a malformed one is still rejected
// only at the native extension's own decode step, but a caller's own editor
// catches a typo before that.
//
// Runtime exports go through `exports.X = ...` rather than `export`, the
// same reason `../index.ts`'s own header comment gives: this package's
// `"type": "commonjs"` makes top-level runtime `export` syntax a hard parse
// error under Node's native TypeScript support. `export interface`/
// `export type` are exempt -- erasable, no runtime value.

import type {
  ButtonIconPosition,
  ButtonVariant,
  Color,
  ColumnDescriptor,
  DataRowDescriptor,
  Icon,
  PaddingValue,
  SelectionMode,
  StatusBarSegment,
  TabBarSegmentDescriptor,
  TextDisplayLineDescriptor,
  ToolbarButtonDescriptor,
  ToolbarDescriptor,
  TreePath,
  TreeRowDescriptor,
  TreeStyle,
  WidgetId,
} from "../wire";

/**
 * A renderable value: a {@link Node}, a `Component` (duck-typed through
 * {@link ComponentLike} -- this module never imports `./component.ts`, so
 * there is no import cycle), a raw wire object/`null`/`undefined` (passed
 * through unchanged, the escape hatch into the lower-level API), or an
 * array of any of those (a repeated child slot, e.g. {@link Tabs}' own
 * `tabs`).
 */
export type Renderable =
  | Node
  | ComponentLike
  | Record<string, unknown>
  | null
  | undefined
  | Renderable[];

/**
 * The structural shape `./runtime.ts`'s `walk` needs out of a `Component`
 * instance -- matched by duck typing (an `instanceof` check would need an
 * import this module deliberately avoids). `./component.ts`'s `Component`
 * class satisfies this shape exactly.
 */
export interface ComponentLike {
  key?: unknown;
  initialState(): Record<string, unknown>;
  view(state: Record<string, unknown>): Renderable;
  /**
   * Optional: called once per surviving state slot on the first render
   * after `App` hot-reloads this component's code (see `./hotReload.ts`),
   * returning the state object the reloaded `view(state)` sees from then
   * on. Left undefined, `walk` merges in any key the current
   * `initialState()` has that the surviving state lacks.
   */
  migrateState?(state: Record<string, unknown>): Record<string, unknown>;
}

/**
 * One layout-tree element awaiting an id and a place in the tree. `kind` is
 * cosmetic (debugging only); `children` maps each named child slot this
 * node has (`"first"`/`"second"` for {@link Split}, `"tabs"` for
 * {@link Tabs}, ...) to whatever {@link Renderable} was passed in.
 * `callbacks` names the `on*` options keys this node's own constructor was
 * given, by the same name `./runtime.ts`'s event-dispatch table looks them
 * up under. `build(resolvedId, resolvedChildren)` returns the wire object:
 * every constructor below supplies its own closure, so a `Node` itself
 * never needs to know the wire shape of anything but its own children
 * slots.
 */
export interface Node {
  kind: string;
  id: string | null;
  callbacks: Readonly<Record<string, (...args: unknown[]) => void>>;
  children: Readonly<Record<string, Renderable>>;
  build(resolvedId: string, resolvedChildren: Record<string, unknown>): Record<string, unknown>;
}

type AnyCallback = (...args: unknown[]) => void;

function callbacksFrom(named: Record<string, AnyCallback | undefined>): Record<string, AnyCallback> {
  const result: Record<string, AnyCallback> = {};
  for (const [name, fn] of Object.entries(named)) {
    if (fn !== undefined) {
      result[name] = fn;
    }
  }
  return result;
}

// --- composition: split / column / row / sidebarPanel / tabs --------------

export interface SplitOptions {
  id?: string;
  orientation?: "horizontal" | "vertical";
  ratio?: number;
  firstMin?: number;
  secondMin?: number;
}

/**
 * A resizable two-pane divider. `ratio` is read back as *uncontrolled* from
 * the second frame on as long as every render resends the same literal --
 * a user drag then persists in the framework's own widget-state store with
 * no state this layer needs to track; see `QuiApp.widgetState`'s own doc.
 */
function Split(first: Renderable, second: Renderable, options: SplitOptions = {}): Node {
  const { id = null, orientation = "horizontal", ratio = 0.5, firstMin, secondMin } = options;

  function build(nodeId: string, children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = {
      id: nodeId,
      orientation,
      ratio,
      first: children.first,
      second: children.second,
    };
    if (firstMin !== undefined) payload.first_min = firstMin;
    if (secondMin !== undefined) payload.second_min = secondMin;
    return { split: payload };
  }

  return { kind: "split", id, callbacks: {}, children: { first, second }, build };
}
exports.Split = Split;

export interface ColumnOptions {
  padding?: PaddingValue;
  margin?: number;
  gap?: number;
}

/**
 * A vertical stack. Unaddressable on the wire (`LayoutColumnNode` has no
 * `id` of its own), so it never takes callbacks. `items` is a plain array
 * rather than Python's variadic `*items` -- the idiomatic TS shape here,
 * since an options object (this constructor's own `padding`/`margin`/
 * `gap`) can't follow a rest parameter.
 */
function Column(items: Renderable[], options: ColumnOptions = {}): Node {
  const { padding, margin, gap } = options;

  function build(_nodeId: string, children: Record<string, unknown>): Record<string, unknown> {
    const wire: Record<string, unknown> = { column: children.items };
    if (padding !== undefined) wire.padding = padding;
    if (margin !== undefined) wire.margin = margin;
    if (gap !== undefined) wire.gap = gap;
    return wire;
  }

  return { kind: "column", id: null, callbacks: {}, children: { items: [...items] }, build };
}
exports.Column = Column;

export interface RowOptions {
  padding?: PaddingValue;
  margin?: number;
  gap?: number;
}

/** A horizontal stack -- {@link Column}'s sibling, same unaddressable shape. */
function Row(items: Renderable[], options: RowOptions = {}): Node {
  const { padding, margin, gap } = options;

  function build(_nodeId: string, children: Record<string, unknown>): Record<string, unknown> {
    const wire: Record<string, unknown> = { row: children.items };
    if (padding !== undefined) wire.padding = padding;
    if (margin !== undefined) wire.margin = margin;
    if (gap !== undefined) wire.gap = gap;
    return wire;
  }

  return { kind: "row", id: null, callbacks: {}, children: { items: [...items] }, build };
}
exports.Row = Row;

export interface SidebarPanelOptions {
  id?: string;
  toolbar?: ToolbarDescriptor;
  toolbarHeight?: number;
}

/**
 * A reserved-top-slot chrome wrapper -- `toolbar` is a raw
 * `ToolbarDescriptor` (build one with {@link Toolbar}), not a child `Node`:
 * it paints above `content` but isn't itself a layout-tree child.
 */
function SidebarPanel(content: Renderable, options: SidebarPanelOptions = {}): Node {
  const { id = null, toolbar, toolbarHeight } = options;

  function build(nodeId: string, children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = { id: nodeId, content: children.content };
    if (toolbar !== undefined) payload.toolbar = toolbar;
    if (toolbarHeight !== undefined) payload.toolbar_height = toolbarHeight;
    return { sidebar_panel: payload };
  }

  return { kind: "sidebar_panel", id, callbacks: {}, children: { content }, build };
}
exports.SidebarPanel = SidebarPanel;

export interface ScrollOptions {
  id?: string;
  axis?: "vertical" | "horizontal";
  offset?: number;
  padding?: PaddingValue;
  margin?: number;
}

/**
 * A scrollable viewport around a single `content` subtree, built on
 * `quadraui::ScrollView`. `axis` picks which dimension scrolls (defaults
 * to `"vertical"`); `offset` is the same controlled/uncontrolled
 * surface-pixel scroll position `scrollOffset` is elsewhere -- omit it to
 * let the framework own the position across frames (mouse wheel,
 * PageUp/PageDown, a scrollbar drag), or pass it on every render to pin
 * it from the app's own state. `padding`/`margin` inset the viewport
 * before `content` is measured/painted; there's no `gap` -- a scroll has
 * only one content subtree, nothing to space it from.
 */
function Scroll(content: Renderable, options: ScrollOptions = {}): Node {
  const { id = null, axis, offset, padding, margin } = options;

  function build(nodeId: string, children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = { id: nodeId, content: children.content };
    if (axis !== undefined && axis !== "vertical") payload.axis = axis;
    if (offset !== undefined) payload.offset = offset;
    const wire: Record<string, unknown> = { scroll: payload };
    if (padding !== undefined) wire.padding = padding;
    if (margin !== undefined) wire.margin = margin;
    return wire;
  }

  return { kind: "scroll", id, callbacks: {}, children: { content }, build };
}
exports.Scroll = Scroll;

interface TabSpec {
  content: Renderable;
  title: string;
  id?: string;
  closable?: boolean;
  isDirty?: boolean;
  isPreview?: boolean;
}

export interface TabOptions {
  title: string;
  id?: string;
  closable?: boolean;
  isDirty?: boolean;
  isPreview?: boolean;
}

/**
 * One {@link Tabs} entry. `id` defaults to `"<tabsId>:tab:<index>"` if
 * omitted -- stable across re-render as long as this tab's position among
 * its siblings doesn't change, the same rule `Node`'s own auto-id follows.
 */
function Tab(content: Renderable, options: TabOptions): TabSpec {
  const { title, id, closable, isDirty, isPreview } = options;
  return { content, title, id, closable, isDirty, isPreview };
}
exports.Tab = Tab;

export interface TabsOptions {
  id?: string;
  closable?: boolean;
  /**
   * Pins which tab paints active, both the strip's own highlight and the
   * content pane -- the wire's `LayoutTabsPayload.selected`. Omit it (the
   * default) to leave this tab group uncontrolled: a click persists which
   * tab is active in the framework's own widget-state store, and this
   * node never overrides it. Pass it on every render (updating it from
   * {@link TabsOptions.onActivate} too, so a real click's own choice and
   * this pinned value never fight) to drive which tab is active from the
   * app's own state instead -- the one way to focus a tab programmatically
   * (e.g. "open this folder and switch to its tab") without a framework
   * method for applying that one click outcome directly into widget state
   * the way `quadraui-py`'s `QuiAppActions.apply_tab_activated` does.
   */
  activeIndex?: number;
  rightSegments?: TabBarSegmentDescriptor[];
  activeAccent?: Color;
  compact?: boolean;
  onClose?: (index: number) => void;
  onActivate?: (index: number) => void;
}

/**
 * A closable tab strip. Which tab is *active* is framework-owned widget
 * state by default (a click already applies `TabActivated` before this
 * layer's own `onActivate` would even see it) -- `onActivate` exists for
 * the rarer case an app wants to react to the switch itself (logging,
 * say), not to pin the index back into this node; see `Node` callbacks
 * for how it is looked up. `onClose` fires on the tab's own `TabClosed`;
 * this node never removes the tab itself -- the caller still owns `tabs`.
 */
function Tabs(tabs: TabSpec[], options: TabsOptions = {}): Node {
  const { id = null, closable, activeIndex, rightSegments, activeAccent, compact, onClose, onActivate } = options;

  function build(nodeId: string, children: Record<string, unknown>): Record<string, unknown> {
    const contents = children.tabs as unknown[];
    const wiredTabs = tabs.map((spec, i) => {
      const entry: Record<string, unknown> = {
        id: spec.id ?? `${nodeId}:tab:${i}`,
        title: spec.title,
        content: contents[i],
      };
      if (spec.closable !== undefined) entry.closable = spec.closable;
      if (spec.isDirty !== undefined) entry.is_dirty = spec.isDirty;
      if (spec.isPreview !== undefined) entry.is_preview = spec.isPreview;
      return entry;
    });
    const payload: Record<string, unknown> = { id: nodeId, tabs: wiredTabs };
    if (closable !== undefined) payload.closable = closable;
    if (activeIndex !== undefined) payload.selected = activeIndex;
    if (rightSegments !== undefined) payload.right_segments = rightSegments;
    if (activeAccent !== undefined) payload.active_accent = activeAccent;
    if (compact !== undefined) payload.compact = compact;
    return { tabs: payload };
  }

  const callbacks = callbacksFrom({ on_close: onClose as AnyCallback | undefined, on_activate: onActivate as AnyCallback | undefined });
  const children = { tabs: tabs.map((spec) => spec.content) };
  return { kind: "tabs", id, callbacks, children, build };
}
exports.Tabs = Tabs;

// --- leaves -----------------------------------------------------------------

/** `TreeViewDescriptor.style` is a required field on the wire -- used whenever {@link Tree} doesn't pass its own `style`. */
const DEFAULT_TREE_STYLE: TreeStyle = {
  indent: 2,
  show_chevrons: true,
  chevron_expanded: "▾",
  chevron_collapsed: "▸",
};

export interface TreeOptions {
  id?: string;
  selectionMode?: SelectionMode;
  selectedPath?: TreePath;
  hasFocus?: boolean;
  style?: TreeStyle;
  onRowClick?: (path: TreePath) => void;
  onToggleExpand?: (path: TreePath) => void;
  onActivate?: (path: TreePath) => void;
  onSelect?: (path: TreePath) => void;
}

/**
 * A `TreeRowDescriptor` list. `onRowClick` fires on `RowClicked` (a row
 * body click -- select it); `onToggleExpand` on `RowToggleExpand` (the
 * chevron); `onActivate` on `RowDoubleClicked`; `onSelect` on
 * `SelectionChanged`. Scroll is framework-owned; nothing here ever needs to
 * read or write it.
 */
function Tree(rows: TreeRowDescriptor[], options: TreeOptions = {}): Node {
  const {
    id = null,
    selectionMode = "Single",
    selectedPath,
    hasFocus = false,
    style,
    onRowClick,
    onToggleExpand,
    onActivate,
    onSelect,
  } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = {
      id: nodeId,
      rows,
      selection_mode: selectionMode,
      selected_path: selectedPath ?? [],
      has_focus: hasFocus,
      style: style ?? DEFAULT_TREE_STYLE,
    };
    return { id: nodeId, tree: payload };
  }

  const callbacks = callbacksFrom({
    on_row_click: onRowClick as AnyCallback | undefined,
    on_toggle_expand: onToggleExpand as AnyCallback | undefined,
    on_activate: onActivate as AnyCallback | undefined,
    on_select: onSelect as AnyCallback | undefined,
  });
  return { kind: "tree", id, callbacks, children: {}, build };
}
exports.Tree = Tree;

export interface TableOptions {
  id?: string;
  onActivate?: (idx: number) => void;
  onSelect?: (idx: number) => void;
  onHeaderClick?: (col: number) => void;
}

/**
 * A `DataTableDescriptor`. `onActivate` fires on either `RowDoubleClicked`
 * or `RowActivated` (a mouse double-click and an Enter-to-activate both
 * mean "open this row"); `onSelect` on `RowSelected`; `onHeaderClick` on
 * `HeaderClicked`.
 */
function Table(columns: ColumnDescriptor[], rows: DataRowDescriptor[], options: TableOptions = {}): Node {
  const { id = null, onActivate, onSelect, onHeaderClick } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    return { id: nodeId, data_table: { id: nodeId, columns, rows } };
  }

  const callbacks = callbacksFrom({
    on_activate: onActivate as AnyCallback | undefined,
    on_select: onSelect as AnyCallback | undefined,
    on_header_click: onHeaderClick as AnyCallback | undefined,
  });
  return { kind: "data_table", id, callbacks, children: {}, build };
}
exports.Table = Table;

export interface StatusBarOptions {
  id?: string;
  onSegmentClick?: (segmentId: WidgetId) => void;
}

/** A `StatusBarDescriptor`. `onSegmentClick` fires on `SegmentClicked`, with that segment's own `id`. */
function StatusBar(
  leftSegments: StatusBarSegment[],
  rightSegments: StatusBarSegment[],
  options: StatusBarOptions = {},
): Node {
  const { id = null, onSegmentClick } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    return {
      id: nodeId,
      status_bar: { id: nodeId, left_segments: leftSegments, right_segments: rightSegments },
    };
  }

  const callbacks = callbacksFrom({ on_segment_click: onSegmentClick as AnyCallback | undefined });
  return { kind: "status_bar", id, callbacks, children: {}, build };
}
exports.StatusBar = StatusBar;

export interface TextDisplayOptions {
  id?: string;
}

/** A `TextDisplayDescriptor` leaf -- plain read-only text. */
function TextDisplay(lines: TextDisplayLineDescriptor[], options: TextDisplayOptions = {}): Node {
  const { id = null } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    return { id: nodeId, text_display: { id: nodeId, lines } };
  }

  return { kind: "text_display", id, callbacks: {}, children: {}, build };
}
exports.TextDisplay = TextDisplay;

export interface ButtonOptions {
  id?: string;
  onClick?: () => void;
  variant?: ButtonVariant;
  icon?: Icon;
  iconPosition?: ButtonIconPosition;
  tooltip?: string;
  focused?: boolean;
  disabled?: boolean;
}

/**
 * A `ButtonDescriptor` leaf. `quadraui` never turns a raw `MouseDown` into
 * "this button was clicked" on its own (there is no dedicated
 * `ButtonPressed` `UiEvent`) -- it only hit-tests the click and attaches
 * *which* button it landed on (`widget` plus a `button_body` `hit`). So
 * `onClick` is dispatched by `./runtime.ts`'s `App` itself, from whichever
 * of the two real signals a backend actually sends: that hit-tested
 * `MouseDown`, or an `Enter`/`Space` `KeyPressed` while this button is
 * focused.
 */
function Button(label: string, options: ButtonOptions = {}): Node {
  const { id = null, onClick, variant, icon, iconPosition, tooltip, focused, disabled } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = { id: nodeId, label };
    if (variant !== undefined) payload.variant = variant;
    if (icon !== undefined) payload.icon = icon;
    if (iconPosition !== undefined) payload.icon_position = iconPosition;
    if (tooltip !== undefined) payload.tooltip = tooltip;
    if (focused !== undefined) payload.focused = focused;
    if (disabled !== undefined) payload.disabled = disabled;
    return { id: nodeId, button: payload };
  }

  const callbacks = callbacksFrom({ on_click: onClick as AnyCallback | undefined });
  return { kind: "button", id, callbacks, children: {}, build };
}
exports.Button = Button;

let toolbarAutoIds = 0;

export interface ToolbarOptions {
  id?: string;
}

/**
 * A raw `ToolbarDescriptor` -- not a `Node`: every place a toolbar can go
 * ({@link SidebarPanel}'s own `toolbar`, a `LayoutLeafNode`'s `"toolbar"`
 * key) wants the descriptor object directly, and the bindings don't yet
 * route toolbar button clicks to an event (see `ButtonDescriptor`'s own
 * doc on why `onClick` lives on {@link Button}, not here).
 *
 * Because this result never passes through `Node`/`./runtime.ts`'s `walk`,
 * omitting `id` can't fall back to a tree-position auto-id the way every
 * other constructor here does -- there is no tree position to derive it
 * from. It falls back to a call-order counter instead, stable only as long
 * as this call site runs the same number of times, in the same order, on
 * every render. That holds for the common case (build one `Toolbar(...)`
 * once, e.g. as a module-level constant reused every frame); give an
 * explicit `id` if constructing a fresh one inside `view()` itself.
 */
function Toolbar(buttons: ToolbarButtonDescriptor[], options: ToolbarOptions = {}): ToolbarDescriptor {
  const resolvedId = options.id ?? `auto:toolbar:${toolbarAutoIds++}`;
  return { id: resolvedId, buttons };
}
exports.Toolbar = Toolbar;
