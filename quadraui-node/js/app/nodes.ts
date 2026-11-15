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
  ControlWidth,
  DataRowDescriptor,
  FormSpacing,
  Icon,
  LabelAlign,
  LabelPosition,
  LayoutScrollPayload,
  ListItemDescriptor,
  PaddingValue,
  RowAlign,
  SelectionMode,
  StatusBarSegment,
  StyledText,
  TabBarSegmentDescriptor,
  TextDisplayLineDescriptor,
  ToolbarButtonDescriptor,
  ToolbarDescriptor,
  TreePath,
  TreeRowDescriptor,
  TreeStyle,
  ValidationState,
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
  /**
   * Extra id -> callbacks registrations this node owns besides its own
   * `id`/`callbacks` -- {@link Form}'s own per-field `on_change`/`on_submit`,
   * keyed by each field's own wire id rather than the `Form`'s. Takes the
   * resolved node id (the same value `build`'s own first parameter gets) so
   * a field with no explicit id can derive its auto-id from it. `./runtime.ts`'s
   * `walk` merges every entry this returns into the same registry
   * `callbacks` populates, so a later dispatch looks a field id up exactly
   * the way it looks up any other widget id.
   */
  extraCallbacks?(resolvedId: string): Readonly<Record<string, Readonly<Record<string, AnyCallback>>>>;
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

// --- composition: split / column / row / sidebarPanel / scroll / tabs -----

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
  axis?: LayoutScrollPayload["axis"];
  offset?: LayoutScrollPayload["offset"];
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

export interface ListOptions {
  id?: string;
  /**
   * The row index to render as selected -- defaults to `0`, the same as
   * the underlying `ListView` field it fills. While this stays unset, the
   * host owns and advances the cursor on its own (clicks, `j`/`k`, arrow
   * keys) -- the same uncontrolled-unless-pinned shape {@link TreeOptions}'
   * own `selectedPath` follows; pass a value on every render only to pin
   * the cursor from app state, and once sent it latches as controlled for
   * this widget id and does not go back.
   */
  selected?: number;
  title?: StyledText;
  hasFocus?: boolean;
  bordered?: boolean;
  onSelect?: (idx: number) => void;
  onActivate?: (idx: number) => void;
}

/**
 * A `ListViewDescriptor`. `onSelect` fires on `SelectionChanged` (a plain
 * click, or `j`/`k` moving the keyboard selection); `onActivate` fires on
 * either `ItemDoubleClicked` (a mouse double-click) or `ItemActivated`
 * (Enter while a row holds the selection) -- the same "open this row"
 * pairing {@link Table}'s own `onActivate` documents. `KeyPressed`/
 * `SelectionSet` on a `List` are not routed to either callback and fall
 * through to `onEvent` instead.
 */
function List(items: ListItemDescriptor[], options: ListOptions = {}): Node {
  const { id = null, selected, title, hasFocus = false, bordered, onSelect, onActivate } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = {
      id: nodeId,
      items,
      selected_idx: selected ?? 0,
      has_focus: hasFocus,
    };
    if (title !== undefined) payload.title = title;
    if (bordered !== undefined) payload.bordered = bordered;
    return { id: nodeId, list: payload };
  }

  const callbacks = callbacksFrom({
    on_select: onSelect as AnyCallback | undefined,
    on_activate: onActivate as AnyCallback | undefined,
  });
  return { kind: "list", id, callbacks, children: {}, build };
}
exports.List = List;

// --- forms / form fields -----------------------------------------------

/** `{text, fg: null, bg: null}` -- the plain-text `StyledText` shape every `label`/`hint` field option below accepts as a bare string, so a caller never has to spell out a `StyledText` literal just to label a field. */
function plainText(text: string): StyledText {
  return { spans: [{ text, fg: null, bg: null }] };
}

function fieldLabel(label: string | StyledText): StyledText {
  return typeof label === "string" ? plainText(label) : label;
}

/**
 * Options every field kind below shares -- the `FormFieldDescriptor` keys
 * that sit alongside `kind`, not inside it. `id` is the field's own wire
 * id; omitted, {@link Form} derives `${formId}:field:${index}` for it, the
 * same positional auto-id shape {@link Tabs}' own tabs use.
 */
export interface FormFieldCommonOptions {
  id?: string;
  hint?: string | StyledText;
  disabled?: boolean;
  validation?: ValidationState | null;
  labelPosition?: LabelPosition | null;
  labelAlign?: LabelAlign | null;
  labelWidth?: number | null;
  tabIndex?: number | null;
  controlWidth?: ControlWidth | null;
  minControlWidth?: number | null;
  rowAlign?: RowAlign | null;
}

/**
 * One {@link Form} field, awaiting its own wire id -- built by one of
 * `TextInputField`/`DropdownField`/`ToggleField`/`SliderField`/
 * `PasswordField`/`TextAreaField` below. Not a {@link Node}: a
 * `FormFieldDescriptor` is a flat entry in `FormDescriptor.fields`, not a
 * child in the layout tree, so it has no `children` slot of its own and
 * resolves its id from {@link Form}'s own `build` directly rather than
 * through `./runtime.ts`'s `walk`.
 */
export interface FormFieldSpec {
  id: string | null;
  callbacks: Readonly<Record<string, AnyCallback>>;
  build(fieldId: string): Record<string, unknown>;
}

function fieldPayload(
  fieldId: string,
  label: string | StyledText,
  kind: Record<string, unknown> | string,
  common: FormFieldCommonOptions,
): Record<string, unknown> {
  const payload: Record<string, unknown> = { id: fieldId, label: fieldLabel(label), kind };
  if (common.hint !== undefined) payload.hint = fieldLabel(common.hint);
  if (common.disabled !== undefined) payload.disabled = common.disabled;
  if (common.validation !== undefined) payload.validation = common.validation;
  if (common.labelPosition !== undefined) payload.label_position = common.labelPosition;
  if (common.labelAlign !== undefined) payload.label_align = common.labelAlign;
  if (common.labelWidth !== undefined) payload.label_width = common.labelWidth;
  if (common.tabIndex !== undefined) payload.tab_index = common.tabIndex;
  if (common.controlWidth !== undefined) payload.control_width = common.controlWidth;
  if (common.minControlWidth !== undefined) payload.min_control_width = common.minControlWidth;
  if (common.rowAlign !== undefined) payload.row_align = common.rowAlign;
  return payload;
}

export interface TextInputFieldOptions extends FormFieldCommonOptions {
  placeholder?: string;
  cursor?: number | null;
  selectionAnchor?: number | null;
  /** Fires on `FormEvent::TextInputChanged` -- every keystroke. */
  onChange?: (value: string) => void;
  /** Fires on `FormEvent::TextInputCommitted` -- Enter while this field is focused. */
  onSubmit?: (value: string) => void;
}

/** A `FieldKind::TextInput` field: a single-line editable value with its own label. */
function TextInputField(label: string | StyledText, value: string, options: TextInputFieldOptions = {}): FormFieldSpec {
  const { id = null, placeholder, cursor, selectionAnchor, onChange, onSubmit, ...common } = options;

  function build(fieldId: string): Record<string, unknown> {
    const kind: Record<string, unknown> = { value };
    if (placeholder !== undefined) kind.placeholder = placeholder;
    if (cursor !== undefined) kind.cursor = cursor;
    if (selectionAnchor !== undefined) kind.selection_anchor = selectionAnchor;
    return fieldPayload(fieldId, label, { TextInput: kind }, common);
  }

  const callbacks = callbacksFrom({
    on_change: onChange as AnyCallback | undefined,
    on_submit: onSubmit as AnyCallback | undefined,
  });
  return { id, callbacks, build };
}
exports.TextInputField = TextInputField;

export interface PasswordFieldOptions extends FormFieldCommonOptions {
  placeholder?: string;
  cursor?: number | null;
  maskChar?: string;
  /** Fires on `FormEvent::TextInputChanged` -- every keystroke. */
  onChange?: (value: string) => void;
  /** Fires on `FormEvent::TextInputCommitted` -- Enter while this field is focused. */
  onSubmit?: (value: string) => void;
}

/** A `FieldKind::PasswordInput` field -- {@link TextInputField}'s masked sibling. */
function PasswordField(label: string | StyledText, value: string, options: PasswordFieldOptions = {}): FormFieldSpec {
  const { id = null, placeholder, cursor, maskChar, onChange, onSubmit, ...common } = options;

  function build(fieldId: string): Record<string, unknown> {
    const kind: Record<string, unknown> = { value };
    if (placeholder !== undefined) kind.placeholder = placeholder;
    if (cursor !== undefined) kind.cursor = cursor;
    if (maskChar !== undefined) kind.mask_char = maskChar;
    return fieldPayload(fieldId, label, { PasswordInput: kind }, common);
  }

  const callbacks = callbacksFrom({
    on_change: onChange as AnyCallback | undefined,
    on_submit: onSubmit as AnyCallback | undefined,
  });
  return { id, callbacks, build };
}
exports.PasswordField = PasswordField;

export interface TextAreaFieldOptions extends FormFieldCommonOptions {
  placeholder?: string;
  cursor?: number | null;
  visibleRows?: number;
  /** Fires on `FormEvent::TextInputChanged` -- every keystroke. */
  onChange?: (value: string) => void;
}

/**
 * A `FieldKind::TextArea` field. No `onSubmit` -- unlike
 * {@link TextInputField}/{@link PasswordField}, Enter inserts a newline on a
 * focused `TextArea` instead of committing, so it never emits
 * `FormEvent::TextInputCommitted`.
 */
function TextAreaField(label: string | StyledText, value: string, options: TextAreaFieldOptions = {}): FormFieldSpec {
  const { id = null, placeholder, cursor, visibleRows, onChange, ...common } = options;

  function build(fieldId: string): Record<string, unknown> {
    const kind: Record<string, unknown> = { value };
    if (placeholder !== undefined) kind.placeholder = placeholder;
    if (cursor !== undefined) kind.cursor = cursor;
    if (visibleRows !== undefined) kind.visible_rows = visibleRows;
    return fieldPayload(fieldId, label, { TextArea: kind }, common);
  }

  const callbacks = callbacksFrom({ on_change: onChange as AnyCallback | undefined });
  return { id, callbacks, build };
}
exports.TextAreaField = TextAreaField;

export interface ToggleFieldOptions extends FormFieldCommonOptions {
  /** Fires on `FormEvent::ToggleChanged`. */
  onChange?: (value: boolean) => void;
}

/** A `FieldKind::Toggle` field -- an on/off field, the Form-embedded sibling of the standalone {@link Checkbox} leaf. */
function ToggleField(label: string | StyledText, value: boolean, options: ToggleFieldOptions = {}): FormFieldSpec {
  const { id = null, onChange, ...common } = options;

  function build(fieldId: string): Record<string, unknown> {
    return fieldPayload(fieldId, label, { Toggle: { value } }, common);
  }

  const callbacks = callbacksFrom({ on_change: onChange as AnyCallback | undefined });
  return { id, callbacks, build };
}
exports.ToggleField = ToggleField;

export interface SliderFieldOptions extends FormFieldCommonOptions {
  step?: number;
  /** Fires on `FormEvent::SliderChanged`. */
  onChange?: (value: number) => void;
}

/** A `FieldKind::Slider` field. */
function SliderField(
  label: string | StyledText,
  value: number,
  min: number,
  max: number,
  options: SliderFieldOptions = {},
): FormFieldSpec {
  const { id = null, step, onChange, ...common } = options;

  function build(fieldId: string): Record<string, unknown> {
    const kind: Record<string, unknown> = { value, min, max };
    if (step !== undefined) kind.step = step;
    return fieldPayload(fieldId, label, { Slider: kind }, common);
  }

  const callbacks = callbacksFrom({ on_change: onChange as AnyCallback | undefined });
  return { id, callbacks, build };
}
exports.SliderField = SliderField;

export interface DropdownFieldOptions extends FormFieldCommonOptions {
  /** Fires on `FormEvent::DropdownChanged`. */
  onChange?: (selectedIdx: number) => void;
}

/** A `FieldKind::Dropdown` field -- a closed-options "select" field. */
function DropdownField(
  label: string | StyledText,
  choices: StyledText[],
  selectedIdx: number,
  options: DropdownFieldOptions = {},
): FormFieldSpec {
  const { id = null, onChange, ...common } = options;

  function build(fieldId: string): Record<string, unknown> {
    return fieldPayload(fieldId, label, { Dropdown: { options: choices, selected_idx: selectedIdx } }, common);
  }

  const callbacks = callbacksFrom({ on_change: onChange as AnyCallback | undefined });
  return { id, callbacks, build };
}
exports.DropdownField = DropdownField;

/** Resolves field `i`'s own wire id: its own explicit id, or `${formId}:field:${i}` -- {@link Tabs}' own `${nodeId}:tab:${i}` auto-id, restated for a field. */
function formFieldId(formId: string, spec: FormFieldSpec, index: number): string {
  return spec.id ?? `${formId}:field:${index}`;
}

export interface FormOptions {
  id?: string;
  focusedField?: WidgetId | null;
  scrollOffset?: number;
  hasFocus?: boolean;
  labelPosition?: LabelPosition;
  labelAlign?: LabelAlign;
  labelWidth?: number | null;
  controlWidth?: ControlWidth;
  minControlWidth?: number | null;
  spacing?: FormSpacing;
}

/**
 * A `FormDescriptor`: a vertical list of labeled fields built from
 * `TextInputField`/`DropdownField`/`ToggleField`/`SliderField`/
 * `PasswordField`/`TextAreaField` above. Each field's own `onChange`/
 * `onSubmit` is registered under that field's own resolved id (see
 * `Node.extraCallbacks`'s own doc) -- `Form` itself takes no callbacks,
 * since nothing on the wire targets the form as a whole the way a field
 * change does.
 */
function Form(fields: FormFieldSpec[], options: FormOptions = {}): Node {
  const {
    id = null,
    focusedField,
    scrollOffset,
    hasFocus,
    labelPosition,
    labelAlign,
    labelWidth,
    controlWidth,
    minControlWidth,
    spacing,
  } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    const wiredFields = fields.map((spec, i) => spec.build(formFieldId(nodeId, spec, i)));
    const payload: Record<string, unknown> = { id: nodeId, fields: wiredFields };
    if (focusedField !== undefined) payload.focused_field = focusedField;
    if (scrollOffset !== undefined) payload.scroll_offset = scrollOffset;
    if (hasFocus !== undefined) payload.has_focus = hasFocus;
    if (labelPosition !== undefined) payload.label_position = labelPosition;
    if (labelAlign !== undefined) payload.label_align = labelAlign;
    if (labelWidth !== undefined) payload.label_width = labelWidth;
    if (controlWidth !== undefined) payload.control_width = controlWidth;
    if (minControlWidth !== undefined) payload.min_control_width = minControlWidth;
    if (spacing !== undefined) payload.spacing = spacing;
    return { id: nodeId, form: payload };
  }

  function extraCallbacks(nodeId: string): Record<string, Record<string, AnyCallback>> {
    const result: Record<string, Record<string, AnyCallback>> = {};
    fields.forEach((spec, i) => {
      if (Object.keys(spec.callbacks).length > 0) {
        result[formFieldId(nodeId, spec, i)] = spec.callbacks;
      }
    });
    return result;
  }

  return { kind: "form", id, callbacks: {}, children: {}, build, extraCallbacks };
}
exports.Form = Form;

// --- standalone leaves matching a Form field kind -----------------------
//
// Each of these mirrors a `Form` field kind above but stands on its own in
// the layout tree as a `LayoutLeafNode` -- not a `FormFieldDescriptor` --
// with no `label`/`hint`/validation chrome of its own. Despite that, each
// still emits its change through the same `FormEvent` shapes (see
// `./runtime.ts`'s `FORM_FIELD_EVENT_MAP`), wrapped in the same `"Form"`
// `UiEvent` key a real `Form` field uses -- the backend reuses one field
// controller for both, keyed by this leaf's own id.
//
// `TextInput`/`Toggle` have no standalone-leaf constructor here: the
// standalone `text_input`/`switch` wire kinds are their own, differently
// shaped descriptors (`TextInputDescriptor.lines: string[]`, a
// multi-line editor; `SwitchDescriptor`), not a `FieldKind::TextInput`/
// `FieldKind::Toggle` payload -- out of scope for this additive pass.

export interface CheckboxOptions {
  id?: string;
  disabled?: boolean;
  focused?: boolean;
  /** Fires on `FormEvent::ToggleChanged`. */
  onChange?: (value: boolean) => void;
}

/** A `CheckboxDescriptor` leaf. */
function Checkbox(value: boolean, options: CheckboxOptions = {}): Node {
  const { id = null, disabled, focused, onChange } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = { id: nodeId, value };
    if (disabled !== undefined) payload.disabled = disabled;
    if (focused !== undefined) payload.focused = focused;
    return { id: nodeId, checkbox: payload };
  }

  const callbacks = callbacksFrom({ on_change: onChange as AnyCallback | undefined });
  return { kind: "checkbox", id, callbacks, children: {}, build };
}
exports.Checkbox = Checkbox;

export interface SliderOptions {
  id?: string;
  step?: number;
  disabled?: boolean;
  focused?: boolean;
  /** Fires on `FormEvent::SliderChanged`. */
  onChange?: (value: number) => void;
}

/** A `SliderDescriptor` leaf. */
function Slider(value: number, min: number, max: number, options: SliderOptions = {}): Node {
  const { id = null, step, disabled, focused, onChange } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = { id: nodeId, value, min, max };
    if (step !== undefined) payload.step = step;
    if (disabled !== undefined) payload.disabled = disabled;
    if (focused !== undefined) payload.focused = focused;
    return { id: nodeId, slider: payload };
  }

  const callbacks = callbacksFrom({ on_change: onChange as AnyCallback | undefined });
  return { kind: "slider", id, callbacks, children: {}, build };
}
exports.Slider = Slider;

export interface SelectOptions {
  id?: string;
  selectedIdx?: number;
  disabled?: boolean;
  focused?: boolean;
  /** Fires on `FormEvent::DropdownChanged`. */
  onChange?: (selectedIdx: number) => void;
}

/** A `SelectDescriptor` leaf -- the standalone sibling of {@link DropdownField}. */
function Select(choices: string[], options: SelectOptions = {}): Node {
  const { id = null, selectedIdx, disabled, focused, onChange } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = { id: nodeId, options: choices };
    if (selectedIdx !== undefined) payload.selected_idx = selectedIdx;
    if (disabled !== undefined) payload.disabled = disabled;
    if (focused !== undefined) payload.focused = focused;
    return { id: nodeId, select: payload };
  }

  const callbacks = callbacksFrom({ on_change: onChange as AnyCallback | undefined });
  return { kind: "select", id, callbacks, children: {}, build };
}
exports.Select = Select;

export interface PasswordOptions {
  id?: string;
  cursor?: number | null;
  hasFocus?: boolean;
  maskChar?: string;
  /** Fires on `FormEvent::TextInputChanged` -- every keystroke. */
  onChange?: (value: string) => void;
  /** Fires on `FormEvent::TextInputCommitted` -- Enter while this leaf is focused. */
  onSubmit?: (value: string) => void;
}

/** A `PasswordDescriptor` leaf -- the standalone sibling of {@link PasswordField}. */
function Password(value: string, options: PasswordOptions = {}): Node {
  const { id = null, cursor, hasFocus, maskChar, onChange, onSubmit } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = { id: nodeId, value };
    if (cursor !== undefined) payload.cursor = cursor;
    if (hasFocus !== undefined) payload.has_focus = hasFocus;
    if (maskChar !== undefined) payload.mask_char = maskChar;
    return { id: nodeId, password: payload };
  }

  const callbacks = callbacksFrom({
    on_change: onChange as AnyCallback | undefined,
    on_submit: onSubmit as AnyCallback | undefined,
  });
  return { kind: "password", id, callbacks, children: {}, build };
}
exports.Password = Password;

export interface TextAreaOptions {
  id?: string;
  placeholder?: string | null;
  cursor?: number | null;
  hasFocus?: boolean;
  visibleRows?: number;
  /** Fires on `FormEvent::TextInputChanged` -- every keystroke. */
  onChange?: (value: string) => void;
}

/** A `TextAreaDescriptor` leaf -- the standalone sibling of {@link TextAreaField}; see that field's own doc for why there is no `onSubmit`. */
function TextArea(value: string, options: TextAreaOptions = {}): Node {
  const { id = null, placeholder, cursor, hasFocus, visibleRows, onChange } = options;

  function build(nodeId: string, _children: Record<string, unknown>): Record<string, unknown> {
    const payload: Record<string, unknown> = { id: nodeId, value };
    if (placeholder !== undefined) payload.placeholder = placeholder;
    if (cursor !== undefined) payload.cursor = cursor;
    if (hasFocus !== undefined) payload.has_focus = hasFocus;
    if (visibleRows !== undefined) payload.visible_rows = visibleRows;
    return { id: nodeId, textarea: payload };
  }

  const callbacks = callbacksFrom({ on_change: onChange as AnyCallback | undefined });
  return { kind: "textarea", id, callbacks, children: {}, build };
}
exports.TextArea = TextArea;

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
