# Bindings wire format (v1)

A `quadraui-py`/`quadraui-node` render callback returns, and a `quadraui-py`/
`quadraui-node` event callback receives, plain `dict`/object values -- JSON-
shaped data, not Rust types. This page is the reference for that shape: the
layout tree a render callback returns, the leaf descriptors it can embed, the
two shapes a `UiEvent` can take on the wire, the controlled/uncontrolled
resolution rule that applies to several descriptor fields, and the
`wire_version` compatibility policy.

The per-kind field tables below (*Generated reference*) are rendered by
`scripts/gen_wire_types.py`'s Markdown renderer from the JSON Schema that
`quadraui-binding-core`'s `export_wire_schema` derives from the Rust wire
types (`quadraui-binding-core/src/wire/schema.rs`) -- the same schema that
produces `quadraui-py/python/quadraui/wire.pyi` and
`quadraui-node/js/wire.d.ts`. A hand-edit to the generated section is
overwritten the next time the script runs, and `scripts/gen_wire_types.py
--check` (wired into CI's `wire-types-check` job) fails if the committed
page doesn't match a fresh generation. Everything above *Generated
reference* is hand-written prose and is left untouched by the generator.

## Node kinds

A render callback returns a `LayoutNode` tree (or the flat `RenderFrameDict`
shape some older callbacks still return instead -- see that def's own doc
in the generated reference). Every `LayoutNode` is a dict with exactly one
of these keys, naming the node's kind:

| Key | Generated def | Shape |
| --- | --- | --- |
| `"row"` | `LayoutRowNode` | A list of child nodes laid out horizontally. |
| `"column"` | `LayoutColumnNode` | A list of child nodes laid out vertically. |
| `"grid"` | `LayoutGridNode` | Two independent `columns`/`rows` track lists, filled row-major by `cells`. |
| `"split"` | `LayoutSplitNode` | Two children (`first`/`second`) either side of a draggable divider. |
| `"tabs"` | `LayoutTabsNode` | A tab strip, each tab's `content` itself a `LayoutNode`. |
| `"sidebar_panel"` | `LayoutSidebarPanelNode` | Chrome (toolbar, accent) wrapper around one child. |
| `"split_tree"` | `LayoutSplitTreeNode` | A recursive tree of splits (`SplitTreeNode`), for more than two panes. |
| `"scroll"` | `LayoutScrollNode` | A scrollable viewport around one content child, built on `ScrollView`. |
| `"leaf"` | `LayoutLeafNode` | A primitive widget -- see *Leaf kinds* below. |

A `"row"`/`"column"`/`"grid"`/`"split"`/`"tabs"`/
`"sidebar_panel"`/`"split_tree"`/`"scroll"` node also accepts the sizing keys `"size"`,
`"min"`, `"max"` when it's itself a `"row"`/`"column"` child, and the
chrome keys `"padding"`, `"margin"`, `"gap"` where that node kind supports
them -- see each node's own field table in the generated reference for
exactly which it accepts.

Every node kind, including a `"leaf"`, also accepts an optional `"style"`
key (`StyleValue` in the generated reference): every `Theme` colour field
and every `Style` metric field is a key here, under that field's own name
-- a colour token (six of which have the semantic aliases `"bg"`, `"fg"`,
`"border"`, `"accent"`, plus `"surface_bg"`/`"accent_bg"`, which already
match their `Theme` field names) takes a `Color` object or a
`"#rrggbb"`/`"#rrggbbaa"` hex string; a metric token takes a number. Every
key is applied as a derived theme/style to that node's own subtree only --
siblings, and anything above it in the tree, are unaffected. A nested
`"style"` composes with an ancestor's: it only overrides the tokens it
names, and everything it doesn't name falls through to whatever the
ancestor already set.

**Only `"padding"` feeds back into layout, and only for a `"button"`
leaf.** Overriding it changes that leaf's own measured size (so a
`"Content"`-sized sibling, or this scroll's own content length, can shift
because of it), not just how it paints. Every other metric token
(`"corner_radius"`, `"border_width"`, and the rest) still affects paint
only, never this frame's layout -- a `"Content"`-sized sibling never
shifts because of one of those deeper in the tree.

**Which token a leaf actually reads varies by kind and, for `"button"`, by
variant** -- a `"bg"` override reaches a plain `"row"`/`"column"`/`"split"`/
`"tabs"`/`"split_tree"` area, a `"button"`'s `Icon` variant, and a
non-bordered `"list"`/`"tree"`, but **not** a `"button"`'s `Primary`/
`Secondary`/`Danger`/`Warning` fill, a bordered `"list"`, or
`"dialog"`/`"toast"`/`"palette"`/`"toolbar"`/`"float"` chrome -- those read
`"surface_bg"` (`Secondary`/`Danger`/`Warning`/disabled) or `"accent_bg"`
(`Primary`, and `"switch"`/`"progress"`'s own active fill) instead:

| Wire key | Resolves onto | Read by |
| --- | --- | --- |
| `"bg"` | `Theme.background` | plain `"row"`/`"column"`/`"split"`/`"tabs"`/`"split_tree"` areas; a `"button"`'s `Icon` variant; a non-bordered `"list"`/`"tree"` |
| `"fg"` | `Theme.foreground` | generic chrome text (`"tab_bar"`'s dirty-tab glyph, `"list"`/`"tree"`/`"data_table"` body/header text) |
| `"border"` | `Theme.border_fg` | bordered-surface border glyphs (`"list"`, `"dialog"`) |
| `"accent"` | `Theme.accent_fg` | focus/active-state accents (a `"form"` toggle, a slider fill, `"tab_bar"`'s active-tab underline) |
| `"surface_bg"` | `Theme.surface_bg` | a `"button"`'s `Secondary`/`Danger`/`Warning` variants and disabled state, a bordered `"list"`, `"dialog"`/`"toast"`/`"palette"`/`"toolbar"`/`"float"` chrome |
| `"accent_bg"` | `Theme.accent_bg` | a `"button"`'s `Primary` variant's solid fill, `"switch"`/`"progress"`'s active fill |
| `"padding"` | `Style.padding` | inner inset shared by `"button"`/`"toast"`/`"tooltip"`/`"context_menu"`/`"palette"`/`"dialog"` |
| `"corner_radius"` | `Style.corner_radius` | rounded-rect radius shared by every rounded overlay |
| `"border_width"` | `Style.border_width` | border stroke width on a pixel backend (no-op on TUI) |

Every other `Theme` colour field (`"tree"`/`"data_table"`/`"tab_bar"`'s
own row/header fill, `Theme.tab_bar_bg`; `"list"`'s header fill,
`Theme.header_bg`; and the rest) and every other `Style` metric field are
overridable too, under that field's own name -- see `StyleValue`'s own
field table in the generated reference for the full list.

**`"font_family"`/`"font_size"`/`"font_weight"` override the node's font**
(a string, a number of surface-native pixels, and `"normal"`/`"bold"`
respectively) -- carrying no matching `Theme`/`Style` field, unlike every
token above. **`"hover"`/`"pressed"`/`"focus"` are each a nested
`StyleValue`**, applied over this override's own base tokens while the
node is in that interaction state (a binding's own hit-test/focus tracking
decides which states currently apply). A state branch accepts the same
colour/metric/font tokens an inline `"style"` key does, but **may not
itself nest a further `"hover"`/`"pressed"`/`"focus"` branch** -- an
unknown-key decode error, the same policy below.

**An unknown key inside `"style"` is always a decode error** — stricter
than the warn-by-default policy an unrecognised sibling key on the
surrounding node dict itself gets; see *Unknown optional wire key policy*
below for both.

### Named style sets: `define_style` + `"class"`

Restating the same `"style"` tokens on every node that wants them gets old
fast, so a host can register a **named style set** once and refer to it by
name from any node afterwards -- the CSS `class=` analogue of the inline
`"style"` attribute above:

```python
app.define_style("card", {"surface_bg": "#1b1f27", "border": "#39404d",
                          "padding": 2.0})
```

```js
app.defineStyle("card", { surface_bg: "#1b1f27", border: "#39404d",
                          padding: 2.0 });
```

`overrides` takes exactly the `StyleValue` keys the table above lists, with
the same closed vocabulary (an unknown key is a decode error). Any node in
any later frame's tree then names the set through an optional `"class"`
sibling key (`ClassValue` in the generated reference), valid everywhere
`"style"` is:

```python
{"id": "c", "button": {...}, "class": "card"}          # one name
{"id": "c", "button": {...}, "class": "card danger"}   # whitespace-separated
{"id": "c", "button": {...}, "class": ["card", "danger"]}  # or a list
```

**Precedence, lowest to highest:** the classes in list order (a later name
wins a token an earlier one also named), then the node's own inline
`"style"` dict. A token no class and no inline `"style"` names falls
through to whatever the enclosing subtree already had, exactly like a
partial `"style"` key does. The result is one derived theme/style scoped to
that node's subtree -- the same mechanism `"style"` alone goes through, so
everything above about metric overrides not re-running layout, and about
which token a leaf actually reads, applies unchanged.

**An unknown class *name* is always ignored, not reported** -- the opposite
end of the spectrum from an unknown key *inside* `"style"`; see *Unknown
optional wire key policy* below for both.

**Four sets exist before any `define_style` call**, each derived from the
app's live `Theme` (so they follow a `set_theme`/`setTheme` call) rather
than from fixed colours:

| Name | Tokens | Intent |
| --- | --- | --- |
| `"primary"` | `accent_bg`/`surface_bg` = `Theme.accent_bg`, `accent` = `Theme.accent_fg` | a `"button"`'s `Primary` variant fill, extended to every filled surface in the subtree |
| `"secondary"` | `accent_bg`/`surface_bg` = `Theme.surface_bg`, `fg` = `Theme.surface_fg` | a `"button"`'s `Secondary` variant fill |
| `"danger"` | `accent_bg`/`surface_bg` = `Theme.button_danger_bg`, `border`/`accent` = `Theme.error_fg` | a `"button"`'s `Danger` variant fill |
| `"subtle"` | `fg`/`border` = `Theme.muted_fg`, `accent_bg`/`surface_bg` = `Theme.background` | de-emphasised chrome: muted text, no raised surface |

Each sets *both* `accent_bg` and `surface_bg`, because which of the two a
`"button"` reads depends on its variant (see the token table above);
naming only one would make `{"class": "danger"}` land on some buttons and
silently no-op on others. `define_style` under one of those four names
replaces it.

**Redefining a set applies from the next frame on.** Nothing caches a
resolved override -- every `"class"` key re-resolves through the registry
during each frame's own decode, with nothing further to do. In Python, a
registered handler redefines a set through
`app.actions().define_style(...)` rather than `app.define_style` directly
(the same "already borrowed" restriction every other `actions()` method
exists for -- see `QuiAppActions`'s own rustdoc for the full story);
applying that queued redefinition requests a repaint on the real backend
a running `run_tui` loop is driving, so it repaints immediately rather
than waiting for the next unrelated event. Node's `defineStyle` has no
such deferred counterpart: it rejects outright while
`startTui`/`startNative` is live (`check_not_live`), so there is never a
running loop for it to nudge. A built-in set resolves against the
app-wide theme read at the start of the frame; an ancestor node's own
`"style"` key narrowing `surface_bg` for its subtree does not re-derive a
descendant's `{"class": "secondary"}` against that narrowed theme.

## Leaf kinds

A `"leaf"` node (`LayoutLeafNode`) is `{"id": <str>, "<kind>": <descriptor>}`
-- exactly one `<kind>` key is present alongside `"id"`. `LayoutLeafNode`'s
own field table in the generated reference lists every registered kind key
and the descriptor type it carries: the six fixed kinds
(`quadraui-binding-core::layout::LEAF_KIND_KEYS` -- `"button"`,
`"text_display"`, `"list"`, `"tree"`, `"data_table"`, `"tab_bar"`),
`"status_bar"`, and every additive kind registered in
`quadraui-binding-core::extra_paint::EXTRA_PRIMITIVE_KINDS` (`"progress"`,
`"spinner"`, `"image"`, `"form"`, `"text_input"`, `"number_input"`,
`"combo_box"`, `"date_picker"`, `"radio_group"`, `"switch"`, `"chart"`,
`"message_list"`, `"board"`, `"pipeline_view"`, `"minimap"`,
`"multi_section_view"`, `"diff_view"`, `"activity_bar"`, `"toolbar"`,
`"menu_bar"`, `"command_line"`, `"command_center"`, `"palette"`,
`"scrollbar"`, `"editor"`, `"terminal"`, `"canvas"`, `"scroll_view"`,
`"find_replace"`). Adding a kind to either Rust-side list is what adds its
row to `LayoutLeafNode`'s table below -- this page never lists them by hand.

`RenderFrameDict` is the older, flat, per-slot shape some callbacks return
instead of a `LayoutNode` tree: the same kind keys as `LayoutLeafNode`, but
every one is nullable (a missing key and an explicit `null` both mean "draw
nothing in this slot this frame") rather than exactly-one-present.

## Events: bare vs. widget-wrapped

A `UiEvent` the app's event callback receives takes one of two JSON shapes:

1. **Bare string** -- `WindowClose`, `DisplaysChanged`, and
   `ContextMenuDismissed` carry no payload at all, so they're the literal
   JSON string `"WindowClose"` etc., not an object.
2. **Single-key tagged object** -- every other variant is
   `{"<Tag>": <payload>}`, optionally alongside a sibling `"hit"` key (the
   hit-tested sub-region a pointer landed on, when the binding attached one
   -- see `_UiEventDict`'s own doc in the generated reference). Most tags'
   payload is the event's own data directly (e.g. `MouseDown`'s payload has
   its own nullable `widget` field, since a click can land outside any
   widget).

A subset of the tagged variants are **widget-wrapped**: their payload is a
`[widget_id, sub_event]` 2-tuple rather than a flat object, because the
event belongs to one specific widget's own event type rather than
`UiEvent`'s shared vocabulary -- `Tree`, `List`, `Form`, `Palette`,
`TabBar`, `StatusBar`, `ActivityBar`, `Terminal`, `TextDisplay`, `Chart`,
`DataTable`, and `Toolbar`. An app dispatches on `widget_id` first, then
hands `sub_event` to that widget kind's own event handling -- and
`sub_event` can itself be a bare string (e.g. `TerminalEvent`'s
`"SelectEnd"`, `PaletteEvent`'s `"Closed"`) or a tagged object, by the exact
same two-shape rule as `UiEvent` itself. See each `*Event` def's own table
in the generated reference for its variants.

## Controlled vs. uncontrolled fields

Several leaf descriptors carry a field that mirrors a piece of state
`quadraui-binding-core::widget_state::WidgetState` also tracks per widget
across frames -- a scroll offset, a checked flag, a focused index. Every
such field is optional (`field_opt` in the generated reference: the key may
be omitted) and follows the same resolution rule, applied per widget id by
`WidgetState::resolve`:

- **Controlled** -- the descriptor sets the field (key present, non-null).
  The app's value wins outright for this frame, and is written back into the
  framework-owned store, so a later frame that goes back to omitting the key
  resumes from this value rather than an older one.
- **Uncontrolled** -- the descriptor leaves the field unset (key absent).
  Whatever's already stored for that widget id wins, or a kind-specific
  default the first time that id is seen.

This lets a render callback either drive a field explicitly every frame
(controlled) or let the framework own it entirely (uncontrolled) without the
binding author having to implement the choice itself. `scroll_offset`,
`focused_index`, and `checked` across the leaf descriptors in the generated
reference below are the common examples.

### `Form.focused_field`

A `"form"` extra's own `focused_field` follows the rule above, with a
richer uncontrolled behaviour than a plain scroll offset or checked flag:

- **Controlled** -- the render callback sets `focused_field` to a field id
  every frame. That id wins outright for this frame no matter what a click
  or keypress just did inside the form, the same strong pin
  `LayoutScroll::offset` gets.
- **Uncontrolled** -- the render callback leaves `focused_field` unset
  (omitted, or explicitly `null`). The host's `InteractionEngine` then owns
  moving it itself: Tab/Shift-Tab cycle between the form's own fields and
  leave the form once a cycle at either end has nowhere left to go inside
  it (handing focus to the next app-level Tab stop instead); Up/Down move
  between fields the currently-focused one doesn't already consume for
  itself (a `DatePicker`/`ComboBox`/`NumberInput`/`RadioGroup` field
  claims Up/Down internally); and clicking a field focuses it. Either way,
  a `FormEvent::FocusChanged` is dispatched on every change, so a
  controlled render callback has something to assign back on the next
  frame, and an uncontrolled one can observe the move without needing to.

An app that never sets `focused_field` at all keeps every other `"form"`
interaction (typing into whichever field is focused, toggling a switch,
clicking a button) working with no app code tracking focus at all -- the
form is never a dead end just because the key was left `null`.

### `has_focus` is not a controlled/uncontrolled field

`has_focus` on a `"text_input"`/`"password"`/`"textarea"` extra (and on a
`Form`'s own fields) looks like it belongs in the list above, but it isn't
tracked by `WidgetState::resolve` at all -- it is purely cosmetic, read only
when painting the caret/focus-ring chrome. Which widget actually *receives*
a typed key or click is decided entirely by the engine's own focus state
(`backend.focus_tree().focused_id()`, set by click-to-focus or
`FocusTree::focus_next`/`focus_prev`), never by this field. A render
callback that never echoes `has_focus: true` back after a `FocusChanged`
dispatch still gets every subsequent keystroke delivered to that widget --
only the focus-ring/caret rendering for that one frame would be stale, not
the routing. Echo it back anyway for a frame-accurate focus ring; just don't
rely on it (or worry about its absence) for keys to keep arriving.

## `wire_version` policy

Every top-level `RenderOutput` (any `LayoutNode` variant, or
`RenderFrameDict`) may carry an optional `"wire_version"` integer key --
never on a nested child node. `quadraui_binding_core::WIRE_VERSION`
is the host's own version; `quadraui-binding-core::wire::check_wire_version`
checks an incoming tree's `"wire_version"` against it:

- **Absent key** -- treated as version `1`. Every render callback written
  before this field existed, in either binding, keeps decoding unchanged.
- **`wire_version` <= the host's constant** -- accepted. The wire shape only
  ever grows additively (new optional keys, never a changed or removed one
  -- see `quadraui/docs/PRIMITIVE_RULES.md` rule 8), so a tree built against
  an older version is still fully decodable by a newer host.
- **`wire_version` > the host's constant** -- rejected with a decode error
  naming both versions, since a binding newer than this host may emit a
  shape this decoder doesn't know about yet.

The constant only moves alongside a real additive wire-shape change, and
only once both `quadraui-py` and `quadraui-node` emit the new value.

## Unknown optional wire key policy

Every wire struct `quadraui-binding-core` decodes by hand (every
`LayoutNode` variant, `LayoutLeafNode`, `RenderFrameDict`, the `run_shell`/
`runShell` `config` object, ...) reports a key outside its own documented
vocabulary — a misspelt optional key like `"cursr"` or `"focussed"` — rather
than either silently dropping it or always hard-failing:

- **Default: warn.** The key reaches a binding's own diagnostic hook (if
  one is registered — `QuiApp`'s `on_unknown_key`/`onUnknownKey`) at most
  once per distinct key name for that app's whole lifetime, and decoding
  continues unaffected. With no hook registered, nothing is user-visible
  beyond that dedup bookkeeping — this is the default, so no app written
  before this policy existed breaks.
- **`strict=True`/`strict: true`: reject.** The same finding instead raises
  a decode error immediately, naming the key and the field it appeared on
  — the same failure mode a missing *required* field already has.

A render callback's returned descriptor and `run_shell`/`runShell`'s
`config` argument each opt into `strict` and register a hook independently
— see `QuiApp`'s own constructor/`run_shell` doc (`quadraui-py`) or
`setStrict`/`setOnUnknownKey`/`runShell` doc (`quadraui-node`) for the exact
surface. A *required* field or a wrongly-typed value is always a decode
error regardless of this policy — it only covers an otherwise-harmless
unrecognised key a decoder would otherwise have had to either drop or
reject with no way to choose.

**An unknown key inside `"style"` stays a hard decode error unconditionally**
— not subject to `strict`/warn at all — stricter than an unrecognised
sibling key on the surrounding node dict itself gets (e.g.
`{"row": [...], "bogus": 1}` reports `"bogus"` through the policy above,
rather than always failing outright): `"style"`'s whole point is a closed,
documented vocabulary of theme/style tokens, where a typo'd key silently
doing nothing would be a worse failure mode than this policy's warn default
allows.

**An unknown class *name* is always ignored, not reported** — the opposite
end of the spectrum from `"style"`'s hard rejection, and not subject to
`strict`/warn either. A class name is open-ended host data (an app defines
its own names at runtime via `define_style`), while `"style"`'s keys are a
fixed, documented token list where a typo can only ever be a typo.

## Generated reference

<!-- BEGIN GENERATED: scripts/gen_wire_types.py (wire schema reference) -->

<!-- toc -->

- [`A11yLiveValue`](#wire-a11ylivevalue)
- [`Accelerator`](#wire-accelerator)
- [`AcceleratorId`](#wire-acceleratorid)
- [`AcceleratorScope`](#wire-acceleratorscope)
- [`ActivityBar`](#wire-activitybar)
- [`ActivityBarEvent`](#wire-activitybarevent)
- [`ActivityBarStyle`](#wire-activitybarstyle)
- [`ActivityItem`](#wire-activityitem)
- [`AlignValue`](#wire-alignvalue)
- [`Anchor`](#wire-anchor)
- [`AppMenu`](#wire-appmenu)
- [`AppMenuEvent`](#wire-appmenuevent)
- [`AppMenuView`](#wire-appmenuview)
- [`AuxHit`](#wire-auxhit)
- [`Axis`](#wire-axis)
- [`BackendNativeEvent`](#wire-backendnativeevent)
- [`Badge`](#wire-badge)
- [`BadgeStatus`](#wire-badgestatus)
- [`BoardAction`](#wire-boardaction)
- [`BoardCard`](#wire-boardcard)
- [`BoardColumn`](#wire-boardcolumn)
- [`BoardModel`](#wire-boardmodel)
- [`BottomPanelConfigDescriptor`](#wire-bottompanelconfigdescriptor)
- [`BottomPanelEvent`](#wire-bottompanelevent)
- [`BottomPanelTabDescriptor`](#wire-bottompaneltabdescriptor)
- [`Button`](#wire-button)
- [`ButtonIconPosition`](#wire-buttoniconposition)
- [`ButtonMask`](#wire-buttonmask)
- [`ButtonRowItem`](#wire-buttonrowitem)
- [`ButtonSpec`](#wire-buttonspec)
- [`ButtonVariant`](#wire-buttonvariant)
- [`CalendarDate`](#wire-calendardate)
- [`Canvas`](#wire-canvas)
- [`CanvasCursor`](#wire-canvascursor)
- [`CanvasEvent`](#wire-canvasevent)
- [`CanvasFontRole`](#wire-canvasfontrole)
- [`CanvasRegion`](#wire-canvasregion)
- [`CardBadge`](#wire-cardbadge)
- [`Cell`](#wire-cell)
- [`CellAlign`](#wire-cellalign)
- [`CellEditState`](#wire-celleditstate)
- [`CellGrid`](#wire-cellgrid)
- [`CellGridEvent`](#wire-cellgridevent)
- [`CellOverflowValue`](#wire-celloverflowvalue)
- [`CellStyle`](#wire-cellstyle)
- [`Chart`](#wire-chart)
- [`ChartEvent`](#wire-chartevent)
- [`ChartKind`](#wire-chartkind)
- [`ChatEvent`](#wire-chatevent)
- [`Checkbox`](#wire-checkbox)
- [`ClassValue`](#wire-classvalue)
- [`Color`](#wire-color)
- [`Column`](#wire-column)
- [`ColumnAlign`](#wire-columnalign)
- [`ColumnWidth`](#wire-columnwidth)
- [`ComboBox`](#wire-combobox)
- [`CommandCenter`](#wire-commandcenter)
- [`CommandCenterTarget`](#wire-commandcentertarget)
- [`CommandLine`](#wire-commandline)
- [`ContextMenu`](#wire-contextmenu)
- [`ContextMenuGlyphs`](#wire-contextmenuglyphs)
- [`ContextMenuItem`](#wire-contextmenuitem)
- [`ContextMenuOverlayDescriptor`](#wire-contextmenuoverlaydescriptor)
- [`ContextMenuPlacement`](#wire-contextmenuplacement)
- [`ControlWidth`](#wire-controlwidth)
- [`CursorPos`](#wire-cursorpos)
- [`CursorShape`](#wire-cursorshape)
- [`DataRow`](#wire-datarow)
- [`DataTable`](#wire-datatable)
- [`DataTableEvent`](#wire-datatableevent)
- [`DataTableGlyphs`](#wire-datatableglyphs)
- [`DataTableLeaf`](#wire-datatableleaf)
- [`DateFormat`](#wire-dateformat)
- [`DateLocale`](#wire-datelocale)
- [`DatePicker`](#wire-datepicker)
- [`Decoration`](#wire-decoration)
- [`DiagnosticMark`](#wire-diagnosticmark)
- [`DiagnosticSeverity`](#wire-diagnosticseverity)
- [`DialogButton`](#wire-dialogbutton)
- [`DialogChromeWire`](#wire-dialogchromewire)
- [`DiffEditability`](#wire-diffeditability)
- [`DiffHunk`](#wire-diffhunk)
- [`DiffLine`](#wire-diffline)
- [`DiffMode`](#wire-diffmode)
- [`DiffPane`](#wire-diffpane)
- [`DiffRow`](#wire-diffrow)
- [`DiffRowKind`](#wire-diffrowkind)
- [`DiffView`](#wire-diffview)
- [`DiffViewEvent`](#wire-diffviewevent)
- [`DragGestureWireEvent`](#wire-draggesturewireevent)
- [`DrawOp`](#wire-drawop)
- [`DropEdge`](#wire-dropedge)
- [`DualModePaletteAction`](#wire-dualmodepaletteaction)
- [`Editor`](#wire-editor)
- [`EditorCursor`](#wire-editorcursor)
- [`EditorEvent`](#wire-editorevent)
- [`EditorHit`](#wire-editorhit)
- [`EditorLine`](#wire-editorline)
- [`EditorPaintOptions`](#wire-editorpaintoptions)
- [`EditorSelection`](#wire-editorselection)
- [`EditorStyledSpan`](#wire-editorstyledspan)
- [`EmptyBody`](#wire-emptybody)
- [`FieldKind`](#wire-fieldkind)
- [`FindReplaceClickTarget`](#wire-findreplaceclicktarget)
- [`FindReplaceEvent`](#wire-findreplaceevent)
- [`FindReplaceField`](#wire-findreplacefield)
- [`FindReplacePanel`](#wire-findreplacepanel)
- [`FindReplaceToggle`](#wire-findreplacetoggle)
- [`Float`](#wire-float)
- [`FontWeightValue`](#wire-fontweightvalue)
- [`Form`](#wire-form)
- [`FormEvent`](#wire-formevent)
- [`FormField`](#wire-formfield)
- [`FormSpacing`](#wire-formspacing)
- [`FrHitRegion`](#wire-frhitregion)
- [`GitLineStatus`](#wire-gitlinestatus)
- [`GlobalShortcutId`](#wire-globalshortcutid)
- [`GridTrack`](#wire-gridtrack)
- [`HeaderAction`](#wire-headeraction)
- [`HeaderHit`](#wire-headerhit)
- [`HelpOverlayAction`](#wire-helpoverlayaction)
- [`HitDetail`](#wire-hitdetail)
- [`Icon`](#wire-icon)
- [`IconGrid`](#wire-icongrid)
- [`IconGridItem`](#wire-icongriditem)
- [`Image`](#wire-image)
- [`ImageFit`](#wire-imagefit)
- [`ImageSource`](#wire-imagesource)
- [`InlineInput`](#wire-inlineinput)
- [`ItemsWindow`](#wire-itemswindow)
- [`JustifyValue`](#wire-justifyvalue)
- [`Key`](#wire-key)
- [`KeyBinding`](#wire-keybinding)
- [`LabelAlign`](#wire-labelalign)
- [`LabelPosition`](#wire-labelposition)
- [`LayoutColumnNode`](#wire-layoutcolumnnode)
- [`LayoutDialogDescriptor`](#wire-layoutdialogdescriptor)
- [`LayoutGridNode`](#wire-layoutgridnode)
- [`LayoutGridPayload`](#wire-layoutgridpayload)
- [`LayoutLeafNode`](#wire-layoutleafnode)
- [`LayoutNode`](#wire-layoutnode)
- [`LayoutRowNode`](#wire-layoutrownode)
- [`LayoutScrollNode`](#wire-layoutscrollnode)
- [`LayoutScrollPayload`](#wire-layoutscrollpayload)
- [`LayoutSidebarPanelNode`](#wire-layoutsidebarpanelnode)
- [`LayoutSidebarPanelPayload`](#wire-layoutsidebarpanelpayload)
- [`LayoutSplitNode`](#wire-layoutsplitnode)
- [`LayoutSplitPayload`](#wire-layoutsplitpayload)
- [`LayoutSplitTreeNode`](#wire-layoutsplittreenode)
- [`LayoutSplitTreePayload`](#wire-layoutsplittreepayload)
- [`LayoutTabEntryNode`](#wire-layouttabentrynode)
- [`LayoutTabsNode`](#wire-layouttabsnode)
- [`LayoutTabsPayload`](#wire-layouttabspayload)
- [`LeafA11y`](#wire-leafa11y)
- [`ListItem`](#wire-listitem)
- [`ListView`](#wire-listview)
- [`ListViewEvent`](#wire-listviewevent)
- [`ListViewLeaf`](#wire-listviewleaf)
- [`Markdown`](#wire-markdown)
- [`MenuDef`](#wire-menudef)
- [`MessageDialogOverlayDescriptor`](#wire-messagedialogoverlaydescriptor)
- [`MessageList`](#wire-messagelist)
- [`MessageListEvent`](#wire-messagelistevent)
- [`MessageRow`](#wire-messagerow)
- [`Minimap`](#wire-minimap)
- [`MinimapEvent`](#wire-minimapevent)
- [`MinimapLine`](#wire-minimapline)
- [`MinimapScale`](#wire-minimapscale)
- [`MinimapSizing`](#wire-minimapsizing)
- [`MinimapSpan`](#wire-minimapspan)
- [`Modifiers`](#wire-modifiers)
- [`MouseButton`](#wire-mousebutton)
- [`MoveDir`](#wire-movedir)
- [`MultiSectionView`](#wire-multisectionview)
- [`MultiSectionViewEvent`](#wire-multisectionviewevent)
- [`NamedKey`](#wire-namedkey)
- [`NumberInput`](#wire-numberinput)
- [`OrientationValue`](#wire-orientationvalue)
- [`PaddingAxis`](#wire-paddingaxis)
- [`PaddingSides`](#wire-paddingsides)
- [`PaddingValue`](#wire-paddingvalue)
- [`Palette`](#wire-palette)
- [`PaletteEvent`](#wire-paletteevent)
- [`PaletteItem`](#wire-paletteitem)
- [`PaletteMode`](#wire-palettemode)
- [`PalettePreview`](#wire-palettepreview)
- [`PanelDefinitionDescriptor`](#wire-paneldefinitiondescriptor)
- [`PasswordInput`](#wire-passwordinput)
- [`PathPickerAction`](#wire-pathpickeraction)
- [`PipelineEvent`](#wire-pipelineevent)
- [`PipelineStage`](#wire-pipelinestage)
- [`PipelineView`](#wire-pipelineview)
- [`Point`](#wire-point)
- [`PointerKind`](#wire-pointerkind)
- [`PopoverDescriptor`](#wire-popoverdescriptor)
- [`PowerEventKind`](#wire-powereventkind)
- [`ProgressBar`](#wire-progressbar)
- [`RadioGroup`](#wire-radiogroup)
- [`RadioOption`](#wire-radiooption)
- [`Rect`](#wire-rect)
- [`RenderFrameDict`](#wire-renderframedict)
- [`RenderOutput`](#wire-renderoutput)
- [`RowAlign`](#wire-rowalign)
- [`RowsRequestedEvent`](#wire-rowsrequestedevent)
- [`ScriptedWidgetDescriptor`](#wire-scriptedwidgetdescriptor)
- [`ScrollAxis`](#wire-scrollaxis)
- [`ScrollAxisValue`](#wire-scrollaxisvalue)
- [`ScrollDelta`](#wire-scrolldelta)
- [`ScrollMode`](#wire-scrollmode)
- [`ScrollView`](#wire-scrollview)
- [`ScrollViewEvent`](#wire-scrollviewevent)
- [`Scrollbar`](#wire-scrollbar)
- [`Section`](#wire-section)
- [`SectionAux`](#wire-sectionaux)
- [`SectionBody`](#wire-sectionbody)
- [`SectionHeader`](#wire-sectionheader)
- [`SectionSize`](#wire-sectionsize)
- [`Select`](#wire-select)
- [`SelectionKind`](#wire-selectionkind)
- [`SelectionMode`](#wire-selectionmode)
- [`Series`](#wire-series)
- [`ShellConfigDescriptor`](#wire-shellconfigdescriptor)
- [`ShellEvent`](#wire-shellevent)
- [`Side`](#wire-side)
- [`SidebarAction`](#wire-sidebaraction)
- [`SidebarPositionValue`](#wire-sidebarpositionvalue)
- [`Slider`](#wire-slider)
- [`SortDirection`](#wire-sortdirection)
- [`SpellMark`](#wire-spellmark)
- [`Spinner`](#wire-spinner)
- [`SplitDirection`](#wire-splitdirection)
- [`SplitTreeNode`](#wire-splittreenode)
- [`SplitTreeSplitPayload`](#wire-splittreesplitpayload)
- [`StageStatus`](#wire-stagestatus)
- [`StatusBar`](#wire-statusbar)
- [`StatusBarEvent`](#wire-statusbarevent)
- [`StatusBarSegment`](#wire-statusbarsegment)
- [`Style`](#wire-style)
- [`StyleColorValue`](#wire-stylecolorvalue)
- [`StyleValue`](#wire-stylevalue)
- [`StyledSpan`](#wire-styledspan)
- [`StyledText`](#wire-styledtext)
- [`Switch`](#wire-switch)
- [`SystemTheme`](#wire-systemtheme)
- [`TabBar`](#wire-tabbar)
- [`TabBarEvent`](#wire-tabbarevent)
- [`TabBarGlyphs`](#wire-tabbarglyphs)
- [`TabBarSegment`](#wire-tabbarsegment)
- [`TabChrome`](#wire-tabchrome)
- [`TabDropInstruction`](#wire-tabdropinstruction)
- [`TabFrame`](#wire-tabframe)
- [`TabGroupEvent`](#wire-tabgroupevent)
- [`TabIcon`](#wire-tabicon)
- [`TabItem`](#wire-tabitem)
- [`TabSeparator`](#wire-tabseparator)
- [`Terminal`](#wire-terminal)
- [`TerminalCell`](#wire-terminalcell)
- [`TerminalCursorShape`](#wire-terminalcursorshape)
- [`TerminalEvent`](#wire-terminalevent)
- [`TerminalScrollbar`](#wire-terminalscrollbar)
- [`TextArea`](#wire-textarea)
- [`TextDisplay`](#wire-textdisplay)
- [`TextDisplayEvent`](#wire-textdisplayevent)
- [`TextDisplayLeaf`](#wire-textdisplayleaf)
- [`TextDisplayLine`](#wire-textdisplayline)
- [`TextField`](#wire-textfield)
- [`TextInput`](#wire-textinput)
- [`TimeOfDay`](#wire-timeofday)
- [`ToggleGroupItem`](#wire-togglegroupitem)
- [`Toolbar`](#wire-toolbar)
- [`ToolbarButton`](#wire-toolbarbutton)
- [`ToolbarEvent`](#wire-toolbarevent)
- [`ToolbarItemWidth`](#wire-toolbaritemwidth)
- [`ToolbarOrientation`](#wire-toolbarorientation)
- [`ToolbarOverflow`](#wire-toolbaroverflow)
- [`ToolbarOverflowPriorities`](#wire-toolbaroverflowpriorities)
- [`ToolbarOverflowPriorityEntry`](#wire-toolbaroverflowpriorityentry)
- [`ToolbarVAlign`](#wire-toolbarvalign)
- [`TreeEvent`](#wire-treeevent)
- [`TreeRow`](#wire-treerow)
- [`TreeRowEditState`](#wire-treeroweditstate)
- [`TreeStyle`](#wire-treestyle)
- [`TreeTable`](#wire-treetable)
- [`TreeTableControllerDescriptor`](#wire-treetablecontrollerdescriptor)
- [`TreeTableControllerNode`](#wire-treetablecontrollernode)
- [`TreeTableControllerWireEvent`](#wire-treetablecontrollerwireevent)
- [`TreeTableEvent`](#wire-treetableevent)
- [`TreeTableNode`](#wire-treetablenode)
- [`TreeView`](#wire-treeview)
- [`TreeViewLeaf`](#wire-treeviewleaf)
- [`UiEvent`](#wire-uievent)
- [`UpdateInfoWire`](#wire-updateinfowire)
- [`UserPayload`](#wire-userpayload)
- [`ValidationState`](#wire-validationstate)
- [`Viewport`](#wire-viewport)
- [`Weekday`](#wire-weekday)
- [`WidgetId`](#wire-widgetid)
- [`WidgetKind`](#wire-widgetkind)
- [`WidgetStateSnapshot`](#wire-widgetstatesnapshot)
- [`WorkspaceEvent`](#wire-workspaceevent)
- [`_AcceleratorScopeDict`](#wire-_acceleratorscopedict)
- [`_ActivityBarEventItemClicked`](#wire-_activitybareventitemclicked)
- [`_ActivityBarEventKeyPressed`](#wire-_activitybareventkeypressed)
- [`_AuxHitDict`](#wire-_auxhitdict)
- [`_BoardActionDict`](#wire-_boardactiondict)
- [`_BottomPanelEventDict`](#wire-_bottompaneleventdict)
- [`_CanvasEventRegionClicked`](#wire-_canvaseventregionclicked)
- [`_CanvasEventRegionDoubleClicked`](#wire-_canvaseventregiondoubleclicked)
- [`_CanvasEventRegionHovered`](#wire-_canvaseventregionhovered)
- [`_CellGridEventCellEditCancelled`](#wire-_cellgrideventcelleditcancelled)
- [`_CellGridEventCellEditChanged`](#wire-_cellgrideventcelleditchanged)
- [`_CellGridEventCellEditCommitted`](#wire-_cellgrideventcelleditcommitted)
- [`_CellGridEventCellEditRequested`](#wire-_cellgrideventcelleditrequested)
- [`_CellGridEventCellEditStarted`](#wire-_cellgrideventcelleditstarted)
- [`_CellGridEventCellsRequested`](#wire-_cellgrideventcellsrequested)
- [`_CellGridEventCursorMoved`](#wire-_cellgrideventcursormoved)
- [`_CellGridEventNavigated`](#wire-_cellgrideventnavigated)
- [`_ChartEventClicked`](#wire-_charteventclicked)
- [`_ChartEventDataPointClicked`](#wire-_charteventdatapointclicked)
- [`_ChartEventDataPointHovered`](#wire-_charteventdatapointhovered)
- [`_ChartEventLegendClicked`](#wire-_charteventlegendclicked)
- [`_ChatEventDict`](#wire-_chateventdict)
- [`_ChatEventFocusedTurnChanged`](#wire-_chateventfocusedturnchanged)
- [`_ChatEventInputChanged`](#wire-_chateventinputchanged)
- [`_ChatEventKeyPressed`](#wire-_chateventkeypressed)
- [`_ChatEventStreamChunk`](#wire-_chateventstreamchunk)
- [`_ChatEventSubmit`](#wire-_chateventsubmit)
- [`_ChatEventTranscriptScrolled`](#wire-_chateventtranscriptscrolled)
- [`_ChatEventTurnClicked`](#wire-_chateventturnclicked)
- [`_ChatEventTurnCollapsed`](#wire-_chateventturncollapsed)
- [`_ColumnWidthContent`](#wire-_columnwidthcontent)
- [`_ControlWidthDict`](#wire-_controlwidthdict)
- [`_DataTableEventColumnFocused`](#wire-_datatableeventcolumnfocused)
- [`_DataTableEventColumnResized`](#wire-_datatableeventcolumnresized)
- [`_DataTableEventContextMenuRequested`](#wire-_datatableeventcontextmenurequested)
- [`_DataTableEventHeaderClicked`](#wire-_datatableeventheaderclicked)
- [`_DataTableEventRowActivated`](#wire-_datatableeventrowactivated)
- [`_DataTableEventRowDoubleClicked`](#wire-_datatableeventrowdoubleclicked)
- [`_DataTableEventRowSelected`](#wire-_datatableeventrowselected)
- [`_DataTableEventScroll`](#wire-_datatableeventscroll)
- [`_DataTableEventScrollChanged`](#wire-_datatableeventscrollchanged)
- [`_DataTableEventSelectionSet`](#wire-_datatableeventselectionset)
- [`_DiffViewEventPaneFocused`](#wire-_diffvieweventpanefocused)
- [`_DiffViewEventScrolled`](#wire-_diffvieweventscrolled)
- [`_DragGestureWireEventDragOver`](#wire-_draggesturewireeventdragover)
- [`_DragGestureWireEventDragStarted`](#wire-_draggesturewireeventdragstarted)
- [`_DragGestureWireEventDropped`](#wire-_draggesturewireeventdropped)
- [`_DrawOpArc`](#wire-_drawoparc)
- [`_DrawOpBezier`](#wire-_drawopbezier)
- [`_DrawOpDashedLine`](#wire-_drawopdashedline)
- [`_DrawOpDict`](#wire-_drawopdict)
- [`_DrawOpEllipse`](#wire-_drawopellipse)
- [`_DrawOpImage`](#wire-_drawopimage)
- [`_DrawOpLine`](#wire-_drawopline)
- [`_DrawOpPath`](#wire-_drawoppath)
- [`_DrawOpPolygon`](#wire-_drawoppolygon)
- [`_DrawOpPushClip`](#wire-_drawoppushclip)
- [`_DrawOpRect`](#wire-_drawoprect)
- [`_DrawOpRoundedRect`](#wire-_drawoproundedrect)
- [`_DrawOpStrokeRect`](#wire-_drawopstrokerect)
- [`_DrawOpStyledTextRun`](#wire-_drawopstyledtextrun)
- [`_DrawOpTextRun`](#wire-_drawoptextrun)
- [`_DualModePaletteActionDict`](#wire-_dualmodepaletteactiondict)
- [`_DualModePaletteActionItemConfirmed`](#wire-_dualmodepaletteactionitemconfirmed)
- [`_DualModePaletteActionModeToggled`](#wire-_dualmodepaletteactionmodetoggled)
- [`_DualModePaletteActionQueryChanged`](#wire-_dualmodepaletteactionquerychanged)
- [`_DualModePaletteActionSelectionChanged`](#wire-_dualmodepaletteactionselectionchanged)
- [`_DualModePaletteActionTextConfirmed`](#wire-_dualmodepaletteactiontextconfirmed)
- [`_EditorEventPointer`](#wire-_editoreventpointer)
- [`_EditorEventScrollbar`](#wire-_editoreventscrollbar)
- [`_EditorEventWheel`](#wire-_editoreventwheel)
- [`_EditorHitBufferPos`](#wire-_editorhitbufferpos)
- [`_EditorHitDict`](#wire-_editorhitdict)
- [`_EditorHitGutter`](#wire-_editorhitgutter)
- [`_FieldKindButtonRow`](#wire-_fieldkindbuttonrow)
- [`_FieldKindColorPicker`](#wire-_fieldkindcolorpicker)
- [`_FieldKindDict`](#wire-_fieldkinddict)
- [`_FieldKindDropdown`](#wire-_fieldkinddropdown)
- [`_FieldKindPasswordInput`](#wire-_fieldkindpasswordinput)
- [`_FieldKindReadOnly`](#wire-_fieldkindreadonly)
- [`_FieldKindSegmentedControl`](#wire-_fieldkindsegmentedcontrol)
- [`_FieldKindSlider`](#wire-_fieldkindslider)
- [`_FieldKindTextArea`](#wire-_fieldkindtextarea)
- [`_FieldKindTextInput`](#wire-_fieldkindtextinput)
- [`_FieldKindToggle`](#wire-_fieldkindtoggle)
- [`_FieldKindToggleGroup`](#wire-_fieldkindtogglegroup)
- [`_FindReplaceClickTargetDict`](#wire-_findreplaceclicktargetdict)
- [`_FindReplaceEventDict`](#wire-_findreplaceeventdict)
- [`_FindReplaceEventFieldEdited`](#wire-_findreplaceeventfieldedited)
- [`_FindReplaceEventToggled`](#wire-_findreplaceeventtoggled)
- [`_FormEventButtonClicked`](#wire-_formeventbuttonclicked)
- [`_FormEventColorChanged`](#wire-_formeventcolorchanged)
- [`_FormEventComboBoxCancelled`](#wire-_formeventcomboboxcancelled)
- [`_FormEventComboBoxCommitted`](#wire-_formeventcomboboxcommitted)
- [`_FormEventComboBoxHighlightChanged`](#wire-_formeventcomboboxhighlightchanged)
- [`_FormEventComboBoxTextChanged`](#wire-_formeventcomboboxtextchanged)
- [`_FormEventDateChanged`](#wire-_formeventdatechanged)
- [`_FormEventDateFocusMoved`](#wire-_formeventdatefocusmoved)
- [`_FormEventDateOpenChanged`](#wire-_formeventdateopenchanged)
- [`_FormEventDropdownChanged`](#wire-_formeventdropdownchanged)
- [`_FormEventFocusChanged`](#wire-_formeventfocuschanged)
- [`_FormEventKeyPressed`](#wire-_formeventkeypressed)
- [`_FormEventNumberCancelled`](#wire-_formeventnumbercancelled)
- [`_FormEventNumberChanged`](#wire-_formeventnumberchanged)
- [`_FormEventNumberCommitted`](#wire-_formeventnumbercommitted)
- [`_FormEventNumberDraftChanged`](#wire-_formeventnumberdraftchanged)
- [`_FormEventRadioChanged`](#wire-_formeventradiochanged)
- [`_FormEventSegmentedControlChanged`](#wire-_formeventsegmentedcontrolchanged)
- [`_FormEventSliderChanged`](#wire-_formeventsliderchanged)
- [`_FormEventTextInputChanged`](#wire-_formeventtextinputchanged)
- [`_FormEventTextInputCommitted`](#wire-_formeventtextinputcommitted)
- [`_FormEventToggleChanged`](#wire-_formeventtogglechanged)
- [`_FormEventToolbarButtonClicked`](#wire-_formeventtoolbarbuttonclicked)
- [`_HeaderHitDict`](#wire-_headerhitdict)
- [`_HitDetailCanvasRegion`](#wire-_hitdetailcanvasregion)
- [`_HitDetailDataTableFooter`](#wire-_hitdetaildatatablefooter)
- [`_HitDetailDataTableHeader`](#wire-_hitdetaildatatableheader)
- [`_HitDetailDataTableHeaderDivider`](#wire-_hitdetaildatatableheaderdivider)
- [`_HitDetailDataTableRow`](#wire-_hitdetaildatatablerow)
- [`_HitDetailExtraBody`](#wire-_hitdetailextrabody)
- [`_HitDetailListItem`](#wire-_hitdetaillistitem)
- [`_HitDetailListTitle`](#wire-_hitdetaillisttitle)
- [`_HitDetailScrollBody`](#wire-_hitdetailscrollbody)
- [`_HitDetailScrollScrollbarThumb`](#wire-_hitdetailscrollscrollbarthumb)
- [`_HitDetailScrollScrollbarTrackAfter`](#wire-_hitdetailscrollscrollbartrackafter)
- [`_HitDetailScrollScrollbarTrackBefore`](#wire-_hitdetailscrollscrollbartrackbefore)
- [`_HitDetailSplitDivider`](#wire-_hitdetailsplitdivider)
- [`_HitDetailSplitFirstPane`](#wire-_hitdetailsplitfirstpane)
- [`_HitDetailSplitSecondPane`](#wire-_hitdetailsplitsecondpane)
- [`_HitDetailSplitTreeDivider`](#wire-_hitdetailsplittreedivider)
- [`_HitDetailTabBarRightSegment`](#wire-_hitdetailtabbarrightsegment)
- [`_HitDetailTabBarScrollLeft`](#wire-_hitdetailtabbarscrollleft)
- [`_HitDetailTabBarScrollRight`](#wire-_hitdetailtabbarscrollright)
- [`_HitDetailTabBarTab`](#wire-_hitdetailtabbartab)
- [`_HitDetailTabBarTabClose`](#wire-_hitdetailtabbartabclose)
- [`_HitDetailTextDisplayLine`](#wire-_hitdetailtextdisplayline)
- [`_HitDetailTextDisplayScrollbarThumb`](#wire-_hitdetailtextdisplayscrollbarthumb)
- [`_HitDetailTextDisplayScrollbarTrackAfter`](#wire-_hitdetailtextdisplayscrollbartrackafter)
- [`_HitDetailTextDisplayScrollbarTrackBefore`](#wire-_hitdetailtextdisplayscrollbartrackbefore)
- [`_HitDetailTreeChevron`](#wire-_hitdetailtreechevron)
- [`_HitDetailTreeRow`](#wire-_hitdetailtreerow)
- [`_HitDetailTreeScrollbarThumb`](#wire-_hitdetailtreescrollbarthumb)
- [`_HitDetailTreeScrollbarTrackAfter`](#wire-_hitdetailtreescrollbartrackafter)
- [`_HitDetailTreeScrollbarTrackBefore`](#wire-_hitdetailtreescrollbartrackbefore)
- [`_HitDetailWidgetBody`](#wire-_hitdetailwidgetbody)
- [`_Icon1`](#wire-_icon1)
- [`_Icon2`](#wire-_icon2)
- [`_KeyBindingDict`](#wire-_keybindingdict)
- [`_ListViewEventItemActivated`](#wire-_listvieweventitemactivated)
- [`_ListViewEventItemDoubleClicked`](#wire-_listvieweventitemdoubleclicked)
- [`_ListViewEventKeyPressed`](#wire-_listvieweventkeypressed)
- [`_ListViewEventSelectionChanged`](#wire-_listvieweventselectionchanged)
- [`_ListViewEventSelectionSet`](#wire-_listvieweventselectionset)
- [`_MessageListEventScrolled`](#wire-_messagelisteventscrolled)
- [`_MinimapEventSeek`](#wire-_minimapeventseek)
- [`_MinimapSizingDict`](#wire-_minimapsizingdict)
- [`_MinimapSizingVsCodeParity`](#wire-_minimapsizingvscodeparity)
- [`_MouseButtonDict`](#wire-_mousebuttondict)
- [`_MultiSectionViewEventAuxActivated`](#wire-_multisectionvieweventauxactivated)
- [`_MultiSectionViewEventAuxInputChanged`](#wire-_multisectionvieweventauxinputchanged)
- [`_MultiSectionViewEventAuxInputSubmitted`](#wire-_multisectionvieweventauxinputsubmitted)
- [`_MultiSectionViewEventCollapseChanged`](#wire-_multisectionvieweventcollapsechanged)
- [`_MultiSectionViewEventHeaderActivated`](#wire-_multisectionvieweventheaderactivated)
- [`_MultiSectionViewEventScrollChanged`](#wire-_multisectionvieweventscrollchanged)
- [`_MultiSectionViewEventSectionResized`](#wire-_multisectionvieweventsectionresized)
- [`_NamedKeyDict`](#wire-_namedkeydict)
- [`_PaletteEventDict`](#wire-_paletteeventdict)
- [`_PaletteEventExpandToggled`](#wire-_paletteeventexpandtoggled)
- [`_PaletteEventItemConfirmed`](#wire-_paletteeventitemconfirmed)
- [`_PaletteEventKeyPressed`](#wire-_paletteeventkeypressed)
- [`_PaletteEventQueryChanged`](#wire-_paletteeventquerychanged)
- [`_PaletteEventSelectionChanged`](#wire-_paletteeventselectionchanged)
- [`_PathPickerActionConfirmed`](#wire-_pathpickeractionconfirmed)
- [`_PathPickerActionDict`](#wire-_pathpickeractiondict)
- [`_PathPickerActionNavigated`](#wire-_pathpickeractionnavigated)
- [`_PathPickerActionQueryChanged`](#wire-_pathpickeractionquerychanged)
- [`_PathPickerActionSelectionChanged`](#wire-_pathpickeractionselectionchanged)
- [`_PipelineEventKeyPressed`](#wire-_pipelineeventkeypressed)
- [`_PipelineEventStageAction`](#wire-_pipelineeventstageaction)
- [`_PipelineEventStageFocused`](#wire-_pipelineeventstagefocused)
- [`_PipelineEventStageSelected`](#wire-_pipelineeventstageselected)
- [`_PointerKindDict`](#wire-_pointerkinddict)
- [`_ScrollViewEventScrolled`](#wire-_scrollvieweventscrolled)
- [`_SectionSizeDict`](#wire-_sectionsizedict)
- [`_ShellEventActivityCursorMoved`](#wire-_shelleventactivitycursormoved)
- [`_ShellEventBottomItemClicked`](#wire-_shelleventbottomitemclicked)
- [`_ShellEventBottomPanelResized`](#wire-_shelleventbottompanelresized)
- [`_ShellEventDict`](#wire-_shelleventdict)
- [`_ShellEventPanelChanged`](#wire-_shelleventpanelchanged)
- [`_ShellEventSidebarResized`](#wire-_shelleventsidebarresized)
- [`_SidebarActionActiveSectionChanged`](#wire-_sidebaractionactivesectionchanged)
- [`_SidebarActionCollapseChanged`](#wire-_sidebaractioncollapsechanged)
- [`_SidebarActionContextMenuRequested`](#wire-_sidebaractioncontextmenurequested)
- [`_SidebarActionEditCancelled`](#wire-_sidebaractioneditcancelled)
- [`_SidebarActionEditChanged`](#wire-_sidebaractioneditchanged)
- [`_SidebarActionEditConfirmed`](#wire-_sidebaractioneditconfirmed)
- [`_SidebarActionForm`](#wire-_sidebaractionform)
- [`_SidebarActionHeaderActivated`](#wire-_sidebaractionheaderactivated)
- [`_SidebarActionPanelScrolled`](#wire-_sidebaractionpanelscrolled)
- [`_SidebarActionRowActivated`](#wire-_sidebaractionrowactivated)
- [`_SidebarActionRowSelected`](#wire-_sidebaractionrowselected)
- [`_SidebarActionRowToggleExpand`](#wire-_sidebaractionrowtoggleexpand)
- [`_SidebarActionScrollChanged`](#wire-_sidebaractionscrollchanged)
- [`_StatusBarEventKeyPressed`](#wire-_statusbareventkeypressed)
- [`_StatusBarEventSegmentClicked`](#wire-_statusbareventsegmentclicked)
- [`_TabBarEventButtonClicked`](#wire-_tabbareventbuttonclicked)
- [`_TabBarEventFocusMoved`](#wire-_tabbareventfocusmoved)
- [`_TabBarEventKeyPressed`](#wire-_tabbareventkeypressed)
- [`_TabBarEventTabActivated`](#wire-_tabbareventtabactivated)
- [`_TabBarEventTabClosed`](#wire-_tabbareventtabclosed)
- [`_TabDropInstructionDict`](#wire-_tabdropinstructiondict)
- [`_TabDropInstructionMoveToPane`](#wire-_tabdropinstructionmovetopane)
- [`_TabDropInstructionReorder`](#wire-_tabdropinstructionreorder)
- [`_TabDropInstructionSplitToNewPane`](#wire-_tabdropinstructionsplittonewpane)
- [`_TabGroupEventDividerResized`](#wire-_tabgroupeventdividerresized)
- [`_TabGroupEventNewTabRequested`](#wire-_tabgroupeventnewtabrequested)
- [`_TabGroupEventPaneAdded`](#wire-_tabgroupeventpaneadded)
- [`_TabGroupEventPaneCollapsed`](#wire-_tabgroupeventpanecollapsed)
- [`_TabGroupEventPaneFocused`](#wire-_tabgroupeventpanefocused)
- [`_TabGroupEventTabActivated`](#wire-_tabgroupeventtabactivated)
- [`_TabGroupEventTabClosed`](#wire-_tabgroupeventtabclosed)
- [`_TabGroupEventTabDropped`](#wire-_tabgroupeventtabdropped)
- [`_TabGroupEventTabMovedToPane`](#wire-_tabgroupeventtabmovedtopane)
- [`_TabGroupEventTabReordered`](#wire-_tabgroupeventtabreordered)
- [`_TabGroupEventTabSplitToNewPane`](#wire-_tabgroupeventtabsplittonewpane)
- [`_TabIcon1`](#wire-_tabicon1)
- [`_TabIcon2`](#wire-_tabicon2)
- [`_TerminalEventDict`](#wire-_terminaleventdict)
- [`_TerminalEventKeyPressed`](#wire-_terminaleventkeypressed)
- [`_TerminalEventScroll`](#wire-_terminaleventscroll)
- [`_TerminalEventSelectExtend`](#wire-_terminaleventselectextend)
- [`_TerminalEventSelectStart`](#wire-_terminaleventselectstart)
- [`_TextDisplayEventAutoScrollToggled`](#wire-_textdisplayeventautoscrolltoggled)
- [`_TextDisplayEventCopied`](#wire-_textdisplayeventcopied)
- [`_TextDisplayEventKeyPressed`](#wire-_textdisplayeventkeypressed)
- [`_TextDisplayEventScrolled`](#wire-_textdisplayeventscrolled)
- [`_ToolbarButtonAction`](#wire-_toolbarbuttonaction)
- [`_ToolbarButtonDict`](#wire-_toolbarbuttondict)
- [`_ToolbarButtonField`](#wire-_toolbarbuttonfield)
- [`_ToolbarButtonGroup`](#wire-_toolbarbuttongroup)
- [`_ToolbarButtonLabel`](#wire-_toolbarbuttonlabel)
- [`_ToolbarButtonMenuButton`](#wire-_toolbarbuttonmenubutton)
- [`_ToolbarEventButtonClicked`](#wire-_toolbareventbuttonclicked)
- [`_ToolbarEventFieldChanged`](#wire-_toolbareventfieldchanged)
- [`_ToolbarEventFieldCleared`](#wire-_toolbareventfieldcleared)
- [`_ToolbarEventFieldCommitted`](#wire-_toolbareventfieldcommitted)
- [`_ToolbarEventGroupSelected`](#wire-_toolbareventgroupselected)
- [`_ToolbarEventMenuRequested`](#wire-_toolbareventmenurequested)
- [`_ToolbarEventOverflowRequested`](#wire-_toolbareventoverflowrequested)
- [`_ToolbarItemWidthFlex`](#wire-_toolbaritemwidthflex)
- [`_TreeEventContextMenuRequested`](#wire-_treeeventcontextmenurequested)
- [`_TreeEventKeyPressed`](#wire-_treeeventkeypressed)
- [`_TreeEventRowActivated`](#wire-_treeeventrowactivated)
- [`_TreeEventRowClicked`](#wire-_treeeventrowclicked)
- [`_TreeEventRowDoubleClicked`](#wire-_treeeventrowdoubleclicked)
- [`_TreeEventRowRenameCancelled`](#wire-_treeeventrowrenamecancelled)
- [`_TreeEventRowRenameChanged`](#wire-_treeeventrowrenamechanged)
- [`_TreeEventRowRenameCommitted`](#wire-_treeeventrowrenamecommitted)
- [`_TreeEventRowToggleExpand`](#wire-_treeeventrowtoggleexpand)
- [`_TreeEventScrollChanged`](#wire-_treeeventscrollchanged)
- [`_TreeEventSelectionChanged`](#wire-_treeeventselectionchanged)
- [`_TreeEventSelectionSet`](#wire-_treeeventselectionset)
- [`_TreeTableControllerWireEventChildrenRequested`](#wire-_treetablecontrollerwireeventchildrenrequested)
- [`_TreeTableControllerWireEventRowActivated`](#wire-_treetablecontrollerwireeventrowactivated)
- [`_TreeTableControllerWireEventRowSelected`](#wire-_treetablecontrollerwireeventrowselected)
- [`_TreeTableControllerWireEventSortChanged`](#wire-_treetablecontrollerwireeventsortchanged)
- [`_TreeTableControllerWireEventToggleExpand`](#wire-_treetablecontrollerwireeventtoggleexpand)
- [`_TreeTableEventChildrenRequested`](#wire-_treetableeventchildrenrequested)
- [`_TreeTableEventColumnResized`](#wire-_treetableeventcolumnresized)
- [`_TreeTableEventContextMenuRequested`](#wire-_treetableeventcontextmenurequested)
- [`_TreeTableEventRowActivated`](#wire-_treetableeventrowactivated)
- [`_TreeTableEventRowDoubleClicked`](#wire-_treetableeventrowdoubleclicked)
- [`_TreeTableEventRowSelected`](#wire-_treetableeventrowselected)
- [`_TreeTableEventScroll`](#wire-_treetableeventscroll)
- [`_TreeTableEventScrollChanged`](#wire-_treetableeventscrollchanged)
- [`_TreeTableEventSelectionSet`](#wire-_treetableeventselectionset)
- [`_TreeTableEventSortChanged`](#wire-_treetableeventsortchanged)
- [`_TreeTableEventToggleExpanded`](#wire-_treetableeventtoggleexpanded)
- [`_UiEventButtonActivated`](#wire-_uieventbuttonactivated)
- [`_UiEventCommandCenterActivated`](#wire-_uieventcommandcenteractivated)
- [`_UiEventDict`](#wire-_uieventdict)
- [`_UiEventDoubleClick`](#wire-_uieventdoubleclick)
- [`_UiEventFilesDropped`](#wire-_uieventfilesdropped)
- [`_UiEventKeyPressed`](#wire-_uieventkeypressed)
- [`_UiEventMouseDown`](#wire-_uieventmousedown)
- [`_UiEventMouseEntered`](#wire-_uieventmouseentered)
- [`_UiEventMouseLeft`](#wire-_uieventmouseleft)
- [`_UiEventMouseMoved`](#wire-_uieventmousemoved)
- [`_UiEventMouseUp`](#wire-_uieventmouseup)
- [`_UiEventNotificationActivated`](#wire-_uieventnotificationactivated)
- [`_UiEventOpenRequested`](#wire-_uieventopenrequested)
- [`_UiEventPowerStateChanged`](#wire-_uieventpowerstatechanged)
- [`_UiEventRadioGroupSelectionChanged`](#wire-_uieventradiogroupselectionchanged)
- [`_UiEventScroll`](#wire-_uieventscroll)
- [`_UiEventScrollOffsetChanged`](#wire-_uieventscrolloffsetchanged)
- [`_UiEventScrollbarMoved`](#wire-_uieventscrollbarmoved)
- [`_UiEventSplitDividerDragged`](#wire-_uieventsplitdividerdragged)
- [`_UiEventSwitchToggled`](#wire-_uieventswitchtoggled)
- [`_UiEventTextSelectionChanged`](#wire-_uieventtextselectionchanged)
- [`_UiEventTrayClicked`](#wire-_uieventtrayclicked)
- [`_UiEventWindowResized`](#wire-_uieventwindowresized)
- [`_UiEventWindowStateChanged`](#wire-_uieventwindowstatechanged)
- [`_WorkspaceEventActivated`](#wire-_workspaceeventactivated)
- [`_WorkspaceEventClosed`](#wire-_workspaceeventclosed)
- [`_WorkspaceEventOpened`](#wire-_workspaceeventopened)
- [`_WorkspaceEventPromoted`](#wire-_workspaceeventpromoted)
- [`_WorkspaceEventReordered`](#wire-_workspaceeventreordered)

<!-- /toc -->

<a id="wire-a11ylivevalue"></a>
#### `A11yLiveValue`

Alias for `"off" | "polite" | "assertive"`.

<a id="wire-accelerator"></a>
#### `Accelerator`

A declared accelerator: its stable ID, the keys that trigger it, the scope in which it's active, and an optional display label for menus.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `binding` | `KeyBinding` | yes |  |
| `id` | `AcceleratorId` | yes |  |
| `label` | `string \| null` | no | Human-readable label for menus, tooltips, help. |
| `scope` | `AcceleratorScope` | yes |  |

<a id="wire-acceleratorid"></a>
#### `AcceleratorId`

Stable identifier for a registered accelerator. Apps match on this in the `UiEvent::Accelerator` arm of their dispatcher.

Alias for `string`.

<a id="wire-acceleratorscope"></a>
#### `AcceleratorScope`

When an `Accelerator` should fire relative to app state.

Alias for `"Global" | _AcceleratorScopeDict`.

<a id="wire-activitybar"></a>
#### `ActivityBar`

Declarative description of an activity bar.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `active_accent` | `Color \| null` | no | Colour of the left-edge accent bar on active items. |
| `bottom_items` | `ActivityItem[]` | no | Bottom-pinned items rendered from the bottom edge upward. |
| `id` | `WidgetId` | yes |  |
| `is_keyboard_focused` | `boolean` | no | When `true` the bar is accepting keyboard input. |
| `selection_bg` | `Color \| null` | no | Background colour for keyboard-selected items (arrow-nav highlight). |
| `style` | `ActivityBarStyle` | no | Paint-time style request: the active row's fill colour and the icon glyph size. |
| `top_items` | `ActivityItem[]` | yes | Top items rendered starting at the top edge, one row per item. |

<a id="wire-activitybarevent"></a>
#### `ActivityBarEvent`

Events an `ActivityBar` emits back to the app. Currently unused by vimcode (click path dispatches by row arithmetic + engine-side `SidebarPanel` enum), but defined for plugin invariants §10.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `ItemClicked` | `_ActivityBarEventItemClicked` | An item was clicked (or activated via Enter while keyboard-focused). |
| `KeyPressed` | `_ActivityBarEventKeyPressed` | A key was pressed with the activity bar focused and the primitive didn't consume it. |

<a id="wire-activitybarstyle"></a>
#### `ActivityBarStyle`

Paint-time style for an `ActivityBar`, carried on the descriptor as `ActivityBar::style`: the active-item row-fill colour, VS Code style (no line, a soft chip on the row itself), and the icon glyph size.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `active_bg` | `Color \| null` | no | Background fill colour for the active item's row. |
| `icon_size_px` | `number \| null` | no | Icon glyph size, in device-independent pixels. |

<a id="wire-activityitem"></a>
#### `ActivityItem`

One icon entry in an `ActivityBar`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `icon` | `Icon` | yes | Icon to render — `glyph` for a Nerd Font / icon-font variant, `fallback` for ASCII/basic-Unicode. |
| `id` | `WidgetId` | yes | Opaque widget id for click routing. |
| `is_active` | `boolean` | no |  |
| `is_keyboard_selected` | `boolean` | no | Keyboard-focused selection highlight. |
| `tooltip` | `string` | no | Hover tooltip text. |

<a id="wire-alignvalue"></a>
#### `AlignValue`

Alias for `"start" | "center" | "end" | "stretch"`.

<a id="wire-anchor"></a>
#### `Anchor`

Common shape for every overlay primitive that positions a box relative to an anchor rect (or point) inside a containing viewport: `Tooltip`, `Completions`, `ContextMenu`, `RichTextPopup`, `Dialog`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `margin` | `number` | yes | Gap between `rect` and the overlay along the placement axis. |
| `preferred` | `Side` | yes | Side of `rect` the overlay prefers to open on. |
| `rect` | `Rect` | yes | Bounds of the element the overlay is positioned against, in ABSOLUTE (target-surface) coordinates. |

<a id="wire-appmenu"></a>
#### `AppMenu`

An application menu bar: the top-level menus, which one is open, and its dropdown and submenu cascade. Serialises with the same shape as a `MenuBar` (`id`, `items`, `open_item`, `focused_item`).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `dropdown_selected` | `number` | no | Highlighted item in the open dropdown. |
| `focused_item` | `number \| null` | no | The bar item keyboard or pointer focus sits on. |
| `id` | `WidgetId` | yes | The menu's id, also the painted bar's. |
| `items` | `MenuDef[]` | yes | The top-level menus, in bar order. |
| `native` | `boolean` | no | Whether the host's effective menu style is native (`Backend::effective_menu_style`): the bar is then the platform's own, nothing paints in the window and no dropdown opens. |
| `open_item` | `number \| null` | no | The menu whose dropdown is open. |
| `submenu_path` | `number[]` | no | Depth-first path of open submenus within the open dropdown: `submenu_path[d]` is the item at depth `d` whose child is open. |
| `submenu_selected` | `number[]` | no | Highlighted item at each open submenu depth, parallel to `submenu_path`. |
| `typeahead` | `boolean` | no | Whether a plain printable key in an open dropdown is type-ahead (on by default). |

<a id="wire-appmenuevent"></a>
#### `AppMenuEvent`

What an `AppMenu` reports.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Activated` | `WidgetId` | An action item was activated. |
| `Changed` | `AppMenuView` | The open menu, a highlight or the submenu path changed: the host applies the new view (`AppMenu::apply`). |

<a id="wire-appmenuview"></a>
#### `AppMenuView`

Which menu is open and where in it the highlight sits — the visible state an `AppMenu` reports through `AppMenuEvent::Changed`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `dropdown_selected` | `number` | no | Highlighted item in the open dropdown. |
| `focused_item` | `number \| null` | no | The bar item keyboard or pointer focus sits on. |
| `open_item` | `number \| null` | no | The menu whose dropdown is open. |
| `submenu_path` | `number[]` | no | Depth-first path of open submenus within the open dropdown: `submenu_path[d]` is the item at depth `d` whose child is open. |
| `submenu_selected` | `number[]` | no | Highlighted item at each open submenu depth, parallel to `submenu_path`. |

<a id="wire-auxhit"></a>
#### `AuxHit`

Sub-zone of a section aux that was hit.

Alias for `("Input" | "Custom") | _AuxHitDict`.

<a id="wire-axis"></a>
#### `Axis`

Layout direction for the section stack.

Alias for `"Vertical" | "Horizontal"`.

<a id="wire-backendnativeevent"></a>
#### `BackendNativeEvent`

Backend-specific event the crate couldn't normalise.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `backend` | `string` | yes | Backend identifier — matches `Backend::platform_name`. |
| `kind` | `string` | yes | Short name for the native event, e.g. |
| `payload` | `string` | yes | Opaque payload. |

<a id="wire-badge"></a>
#### `Badge`

Right-aligned status indicator on a row: short text plus optional foreground/background colouring. Used by `TreeView` / `ListView` / `DataTable` for git-status letters, item counts, conflict markers, etc.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bg` | `Color \| null` | no |  |
| `fg` | `Color \| null` | no |  |
| `text` | `string` | yes |  |

<a id="wire-badgestatus"></a>
#### `BadgeStatus`

Status of a single inline badge on a card.

Alias for `"Pending" | "Running" | "Passed" | "Warning" | "Blocked"`.

<a id="wire-boardaction"></a>
#### `BoardAction`

Actions emitted by the `Board` widget back to the host.

Alias for `("JumpToTop" | "JumpToBottom") | _BoardActionDict`.

<a id="wire-boardcard"></a>
#### `BoardCard`

A single card displayed in a board column.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `badges` | `CardBadge[]` | no | Inline badge row — one entry per host-defined workflow step. |
| `hint` | `string \| null` | no | One-line callout rendered at the bottom of the card (host-defined content, e.g. |
| `id` | `WidgetId` | yes | Unique identifier for this card (used for selection + actions). |
| `labels` | `string[]` | no | Short prefix labels (e.g. |
| `title` | `string` | yes | Display title (truncated by the rasteriser if too wide). |

<a id="wire-boardcolumn"></a>
#### `BoardColumn`

A single kanban column.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cards` | `BoardCard[]` | yes | Cards in this column (full list; the rasteriser slices by `scroll_offset` and available height). |
| `id` | `WidgetId` | yes | Unique identifier for this column (used in `BoardHit::ColumnHeader`). |
| `scroll_offset` | `number` | no | Vertical scroll offset: index of the first card to show. |
| `title` | `string` | yes | Column header title (e.g. |

<a id="wire-boardmodel"></a>
#### `BoardModel`

Top-level model for a `Board` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col_scroll_offset` | `number` | no | Horizontal scroll: index of the first visible column. |
| `columns` | `BoardColumn[]` | yes | Ordered list of columns rendered left-to-right. |
| `id` | `WidgetId` | yes | Unique identifier for this board widget. |
| `selected_card_id` | `WidgetId \| null` | no | The currently selected card, if any (host-owned). |

<a id="wire-bottompanelconfigdescriptor"></a>
#### `BottomPanelConfigDescriptor`

`run_shell`'s `"bottom_panel"` dict.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `active_tab_id` | `string \| null` | no |  |
| `height_fraction` | `number \| null` | no |  |
| `maximised` | `boolean \| null` | no |  |
| `tabs` | `BottomPanelTabDescriptor[] \| null` | no |  |

<a id="wire-bottompanelevent"></a>
#### `BottomPanelEvent`

Semantic events emitted by the bottom panel controller.

Alias for `"MaximiseToggled" | _BottomPanelEventDict`.

<a id="wire-bottompaneltabdescriptor"></a>
#### `BottomPanelTabDescriptor`

One tab of `run_shell`'s tabbed bottom panel.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `badge` | `string \| null` | no |  |
| `closable` | `boolean \| null` | no |  |
| `id` | `string` | yes |  |
| `label` | `string \| null` | no |  |

<a id="wire-button"></a>
#### `Button`

Declarative description of a standalone button.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no | When `true`, `Button::handle_key` and `Button::click` both return `None` — same "disabled means inert, not hidden" contract every other primitive's `disabled` flag follows. |
| `focused` | `boolean` | no | Whether this button currently holds keyboard focus. |
| `icon` | `Icon \| null` | no | Optional icon painted alongside `Button::label`, at `Button::icon_position`. |
| `icon_gap` | `number` | no | Gap, in surface-native pixels, between `Button::icon` and `Button::label`. |
| `icon_position` | `ButtonIconPosition` | no | Where `Button::icon` paints relative to `Button::label`. |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes | Visible label. |
| `tooltip` | `string \| null` | no | Hover tooltip text, shown via a host's own `crate::compose::HoverTooltipTracker` after `crate::Style::tooltip_delay_ms` of continuous pointer hover and hidden on pointer leave, a click, or a timeout. |
| `variant` | `ButtonVariant` | no |  |

<a id="wire-buttoniconposition"></a>
#### `ButtonIconPosition`

Icon placement relative to `Button::label` when `Button::icon` is set. No effect when `icon` is `None`.

Alias for `"Leading" | "Trailing"`.

<a id="wire-buttonmask"></a>
#### `ButtonMask`

Bitmask of mouse buttons currently held down during a `MouseMoved` event.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `left` | `boolean` | no |  |
| `middle` | `boolean` | no |  |
| `right` | `boolean` | no |  |

<a id="wire-buttonrowitem"></a>
#### `ButtonRowItem`

One button in a `FieldKind::ButtonRow`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no |  |
| `icon` | `Icon \| null` | no | Optional icon rendered before the label. |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |
| `spec` | `ButtonSpec \| null` | no | Optional `crate::ButtonSpec` override, carrying the B3a vocabulary (variant intent, icon placement/gap, a tooltip) this item's own legacy fields above can't express. |

<a id="wire-buttonspec"></a>
#### `ButtonSpec`

Shared, kind-agnostic button description — the fields `Button` itself carries beyond `id`/`focused` (which only make sense on a standalone control), lifted out so `crate::ToolbarButton::Action`, `crate::ButtonRowItem`, `crate::DialogButton` and `crate::ToastButton` can each carry one through an optional `spec` field without pulling in `Button`'s own id/focus-tracking concerns.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no |  |
| `icon` | `Icon \| null` | no |  |
| `icon_gap` | `number` | no |  |
| `icon_position` | `ButtonIconPosition` | no |  |
| `label` | `string` | yes |  |
| `tooltip` | `string \| null` | no |  |
| `variant` | `ButtonVariant` | no |  |

<a id="wire-buttonvariant"></a>
#### `ButtonVariant`

Visual emphasis variant. Purely descriptive — `Button` carries no backend painting, so a rasteriser reads this field to choose its own chrome (filled vs. outlined vs. icon-only).

Alias for `"Primary" | "Secondary" | "Icon" | "Danger" | "Warning"`.

<a id="wire-calendardate"></a>
#### `CalendarDate`

A calendar date in the proleptic Gregorian calendar — no timezone, no clock. `month` is `1..=12`, `day` is `1..=31` (bounded by `CalendarDate::days_in_month` for the given `year`/`month`).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `day` | `number` | yes | `1..=31`, bounded by `CalendarDate::days_in_month` for `year`/`month`. |
| `month` | `number` | yes | `1..=12`. |
| `year` | `number` | yes | Proleptic Gregorian year; may be negative (there is no year 0). |

<a id="wire-canvas"></a>
#### `Canvas`

Declarative description of an app-drawn canvas: an ordered list of `DrawOp`s, painted in order (later ops paint over earlier ones, same "last writer wins" rule every backend's own native 2D API already applies to overlapping draws).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `focusable` | `boolean` | no | Whether this canvas should take part in `AppLogic::tab_stops` keyboard-focus cycling. |
| `id` | `WidgetId` | yes |  |
| `ops` | `DrawOp[]` | yes |  |
| `regions` | `CanvasRegion[]` | no | Named, hit-testable sub-areas layered on top of `ops` — see the module doc's "What this is not" section for why this is the one named exception to "no hit-testing of individual ops". |

<a id="wire-canvascursor"></a>
#### `CanvasCursor`

Serde-friendly mirror of `crate::PointerShape` for `CanvasRegion::cursor` — `PointerShape` itself has no `Serialize`/`Deserialize` derive, and `Canvas`/`DrawOp` are descriptor types that must round-trip through serde (`PRIMITIVE_RULES.md` rule 8's descriptor-shape guidance), the same reasoning `CanvasFontRole` already carries just below. Omits `crate::PointerShape::Resize`: that variant names a *window edge*, a concept a canvas region (a sub-rect of one already-placed `Canvas`) has no analogue for. `Self::to_pointer_shape` is the only place this maps back to the real enum.

Alias for `"Default" | "Text" | "Pointer" | "Grab" | "Grabbing" | "Crosshair" | "Wait" | "NotAllowed" | "ColResize" | "RowResize"`.

<a id="wire-canvasevent"></a>
#### `CanvasEvent`

Events a `crate::Canvas` with declared `crate::CanvasRegion`s emits, carried by `UiEvent::Canvas`. A plain `Canvas` with an empty `crate::Canvas::regions` list never produces one of these — a click or hover against it resolves to a bare `crate::CanvasHit::Inside`/ `Outside`, which a binding threads through as the generic extra-body hit it already reports for any other additive primitive with no typed click event of its own.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `RegionClicked` | `_CanvasEventRegionClicked` | A declared region was clicked (any mouse button — see `button`). |
| `RegionDoubleClicked` | `_CanvasEventRegionDoubleClicked` | A declared region was double-clicked. |
| `RegionHovered` | `_CanvasEventRegionHovered` | The hovered region changed, including to/from no region at all. |

<a id="wire-canvasfontrole"></a>
#### `CanvasFontRole`

Serde-friendly mirror of `crate::FontRole` for `DrawOp::StyledTextRun` — `FontRole` itself has no `Serialize`/`Deserialize` derive (see `crate::font_role`'s module doc), and `Canvas`/`DrawOp` are descriptor types that must round-trip through serde (`PRIMITIVE_RULES.md` rule 8's descriptor-shape guidance). `Self::to_font_role` is the only place this maps back to the real enum, kept private to this module since nothing outside the native-paint seam needs it.

Alias for `"Chrome" | "Editor"`.

<a id="wire-canvasregion"></a>
#### `CanvasRegion`

One named, hit-testable sub-area of a `Canvas`, declared in `Canvas::regions`. `rect` is LOCAL to the canvas — the same frame every `DrawOp` coordinate already uses (module doc's "Coordinate frame" section) — and resolved through `CanvasLayout::region_hit_test`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `CanvasCursor \| null` | no | Advisory pointer-hover hint for this region. |
| `id` | `WidgetId` | yes |  |
| `rect` | `Rect` | yes |  |

<a id="wire-cardbadge"></a>
#### `CardBadge`

A single inline badge on a card (e.g. one step of a host-defined workflow).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `label` | `string` | yes | Short display label rendered next to the status icon. |
| `status` | `BadgeStatus` | yes | Status driving the badge's icon + colour. |

<a id="wire-cell"></a>
#### `Cell`

One cell's already-formatted display text plus its optional style.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `style` | `CellStyle` | no |  |
| `text` | `string` | yes |  |

<a id="wire-cellalign"></a>
#### `CellAlign`

Horizontal alignment for one cell's text.

Alias for `"Left" | "Center" | "Right" | "General"`.

<a id="wire-celleditstate"></a>
#### `CellEditState`

Single-line inline edit state for `CellGrid::editing` — the text buffer, a byte-offset cursor into it, an optional byte-offset selection anchor, and a placeholder shown when `text` is empty.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number` | no | Byte offset into `text`. |
| `placeholder` | `string \| null` | no | Shown in muted style when `text` is empty. |
| `selection_anchor` | `number \| null` | no | Byte offset into `text`. |
| `text` | `string` | yes |  |

<a id="wire-cellgrid"></a>
#### `CellGrid`

Declarative description of a `CellGrid` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cells` | `Cell[][]` | no | The cells to paint this frame — see the module doc for how a `(row, col)` here maps to an absolute sheet position via `CellGridLayout::rows`/`CellGridLayout::cols`. |
| `col_count` | `number` | yes | Total columns in the sheet — drives column-letter headers and the horizontal scrollbar's content extent. |
| `col_labels` | `Record<string, unknown>` | no | Custom column header labels, keyed by absolute column index. |
| `col_scroll` | `number` | no | How many columns after the frozen block are skipped before the body starts painting — the body's first visible column is `frozen_cols + col_scroll`, not `col_scroll` itself. |
| `col_widths` | `Record<string, unknown>` | no | Per-column width overrides, keyed by absolute column index. |
| `column_separators` | `boolean` | no | Paint a `theme.separator` column divider at every body/header column's right-edge cell on TUI — the same reservation `crate::tui::data_table`'s own `usable_width` makes for `DataTable`'s separator column, and the way `Self`'s own frozen-column boundary divider already does. |
| `cursor` | `[number, number]` | no | The active cell, as an absolute `(row, col)`. |
| `default_col_width` | `number` | yes | Column width in surface-native units for any column with no `Self::col_widths` entry. |
| `default_row_height` | `number` | yes | Row height in surface-native units (cells on TUI, pixels on a pixel backend) for any row with no `Self::row_heights` entry. |
| `editing` | `CellEditState \| null` | no | Active inline edit of `Self::cursor`, if any. |
| `frozen_cols` | `number` | no | Leading columns that never scroll, clamped to `col_count`. |
| `frozen_rows` | `number` | no | Leading rows that never scroll, clamped to `row_count` by `Self::layout`. |
| `has_focus` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `loaded_col_start` | `number` | no | Absolute column the scrolled (non-frozen) portion of `Self::cells` starts at — see `Self::loaded_row_start`. |
| `loaded_row_start` | `number` | no | Absolute row the scrolled (non-frozen) portion of `Self::cells` starts at — set by whichever caller filled `cells`, independent of `Self::row_scroll`. |
| `row_count` | `number` | yes | Total rows in the sheet — drives row-number headers and the vertical scrollbar's content extent. |
| `row_heights` | `Record<string, unknown>` | no | Per-row height overrides, keyed by absolute row index. |
| `row_labels` | `Record<string, unknown>` | no | Custom row header labels, keyed by absolute row index. |
| `row_scroll` | `number` | no | How many rows after the frozen block are skipped before the body starts painting — the body's first visible row is `frozen_rows + row_scroll`, not `row_scroll` itself. |
| `selection_anchor` | `[number, number] \| null` | no | The other corner of a rectangular selection, as an absolute `(row, col)`. |
| `show_col_headers` | `boolean` | no | Paint the column-letter header strip at all. |
| `show_row_headers` | `boolean` | no | Paint the row-number header strip at all. |
| `show_scrollbar` | `boolean` | no | Reserve a strip on the right/bottom edges for vertical/horizontal scrollbars. |
| `striped` | `boolean` | no | Tint every other non-frozen body row with `crate::theme::Theme::row_alt_bg` ("zebra striping"), on every backend. |

<a id="wire-cellgridevent"></a>
#### `CellGridEvent`

Events a `CellGrid` emits back to the app.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `CursorMoved` | `_CellGridEventCursorMoved` | The active cell moved to a new absolute `(row, col)`. |
| `CellsRequested` | `_CellGridEventCellsRequested` | The viewport needs the absolute sheet block `[row_start, row_start + rows)` × `[col_start, col_start + cols)` painted this frame, and `CellGrid::cells_window` says it isn't loaded yet — see `CellGrid::cells_requested`. |
| `Navigated` | `_CellGridEventNavigated` | `CellGrid`'s own `crate::interactive::Interactive::handle` moved the cursor and/or changed the selection/scroll — every value a host writes back onto its own `CellGrid::cursor`/ `CellGrid::selection_anchor`/`CellGrid::row_scroll`/ `CellGrid::col_scroll` fields before the next frame. |
| `CellEditRequested` | `_CellGridEventCellEditRequested` | F2 (or any other host-chosen "edit this cell" trigger) was pressed at `(row, col)` with no edit already active. |
| `CellEditStarted` | `_CellGridEventCellEditStarted` | An inline edit began at `(row, col)` — either the host answered `Self::CellEditRequested`, or a plain character typed at the cursor with no active edit started a fresh one seeded with just that character. |
| `CellEditChanged` | `_CellGridEventCellEditChanged` | The in-progress edit's text buffer changed (insert/delete/select) — `edit` is what the host sets `CellGrid::editing` to. |
| `CellEditCommitted` | `_CellGridEventCellEditCommitted` | The user confirmed the in-progress edit — Enter, Tab or an arrow key, the same commit-and-move convention a real spreadsheet follows. |
| `CellEditCancelled` | `_CellGridEventCellEditCancelled` | The user cancelled the in-progress edit (Escape) — the cell's value is unchanged and the cursor hasn't moved; the host clears `CellGrid::editing`. |

<a id="wire-celloverflowvalue"></a>
#### `CellOverflowValue`

Alias for `"ellipsis" | "clip"`.

<a id="wire-cellstyle"></a>
#### `CellStyle`

Optional visual overrides for one `Cell`. Every field defaults to "inherit the theme/column default" when `None`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `align` | `CellAlign \| null` | no | `None` behaves exactly like `Some(CellAlign::General)` — see `CellAlign::resolve`. |
| `bg` | `Color \| null` | no |  |
| `bold` | `boolean \| null` | no |  |
| `fg` | `Color \| null` | no |  |
| `italic` | `boolean \| null` | no |  |
| `underline` | `boolean \| null` | no |  |

<a id="wire-chart"></a>
#### `Chart`

Declarative description of a chart widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `crosshair_x` | `number \| null` | no | The data index (fractional) a vertical crosshair is painted at, with each series' value labelled beside it. |
| `hovered_point` | `[number, number] \| null` | no | The data point painted with a hover marker, as `(series_idx, data_idx)`. |
| `id` | `WidgetId` | yes |  |
| `kind` | `ChartKind` | yes |  |
| `series` | `Series[]` | yes |  |
| `show_grid` | `boolean` | no | Show horizontal grid lines at y-tick positions. |
| `show_legend` | `boolean` | no |  |
| `x_label` | `string \| null` | no |  |
| `x_range` | `[number, number] \| null` | no | Explicit x-axis range, used to compute x-tick *values*. |
| `x_ticks` | `number \| null` | no | Number of x-axis tick marks. |
| `y_label` | `string \| null` | no |  |
| `y_range` | `[number, number] \| null` | no | Explicit y-axis range. |
| `y_ticks` | `number \| null` | no | Number of y-axis tick marks. |

<a id="wire-chartevent"></a>
#### `ChartEvent`

Events a `Chart` emits.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `DataPointHovered` | `_ChartEventDataPointHovered` | The data point under the pointer changed, including to no point at all (`point: None`) — the pointer left every point's snap radius or left the chart. |
| `Clicked` | `_ChartEventClicked` | User clicked the chart body. |
| `DataPointClicked` | `_ChartEventDataPointClicked` | User clicked a specific data point. |
| `LegendClicked` | `_ChartEventLegendClicked` | User clicked a legend entry. |

<a id="wire-chartkind"></a>
#### `ChartKind`

Chart visualisation kind.

Alias for `"Sparkline" | "Line" | "Bar" | "BarGrouped"`.

<a id="wire-chatevent"></a>
#### `ChatEvent`

What a `Chat` reports.

Alias for `("Cancelled" | "StopRequested") | _ChatEventDict`.

<a id="wire-checkbox"></a>
#### `Checkbox`

A standalone boolean toggle.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no | A disabled checkbox paints dimmed and ignores every gesture. |
| `focused` | `boolean` | no | Whether to paint the focused-row wash. |
| `id` | `WidgetId` | yes |  |
| `value` | `boolean` | no | Whether the box is checked. |

<a id="wire-classvalue"></a>
#### `ClassValue`

One or more named style sets: a name, a whitespace-separated list of names, or an array of names.

Alias for `string | string[]`.

<a id="wire-color"></a>
#### `Color`

RGBA colour, 0-255 each channel.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `a` | `number` | yes |  |
| `b` | `number` | yes |  |
| `g` | `number` | yes |  |
| `r` | `number` | yes |  |

<a id="wire-column"></a>
#### `Column`

Column definition for a `DataTable`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `align` | `ColumnAlign` | no | Horizontal text alignment within the column. |
| `title` | `string` | yes |  |
| `width` | `ColumnWidth` | no | Sizing strategy for this column. |

<a id="wire-columnalign"></a>
#### `ColumnAlign`

Horizontal text alignment within a column cell.

Alias for `"Left" | "Center" | "Right"`.

<a id="wire-columnwidth"></a>
#### `ColumnWidth`

Column width strategy.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Native` | `number` | Fixed width in the backend's **native unit**: exactly `n` cells on TUI and exactly `n` pixels/DIPs on GTK/macOS/Win. |
| `FixedCells` | `number` | Fixed width in **cells**: exactly `n` cells on TUI, and `n` times the pixel backend's own average glyph width on GTK/macOS/Win — so the same declared width reads as roughly the same number of average-width characters on every backend. |
| `FixedPx` | `number` | Fixed width in **pixels**: exactly `n` pixels/DIPs on a pixel backend (GTK/macOS/Win), and `n` divided by a nominal 8px terminal cell on TUI — so a pixel-tuned width degrades to a proportionate cell count on a terminal rather than being read as cells outright. |
| `Flex` | `number` | Flex weight — columns share remaining space proportionally. |
| `Content` | `_ColumnWidthContent` | Size to content with optional min/max clamps. |

<a id="wire-combobox"></a>
#### `ComboBox`

Declarative description of an editable combo box: free text plus a filtered dropdown of fixed options.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `committed` | `string` | no | Last confirmed value. |
| `disabled` | `boolean` | no | When `true`, every method below returns `None` — same "disabled means inert, not hidden" contract every other primitive's `disabled` flag follows. |
| `focused` | `boolean` | no | Whether this field currently holds keyboard focus. |
| `highlighted` | `number \| null` | no | Cursor index into the *filtered* list (from `ComboBox::filtered_indices`), not the raw `options` list. |
| `id` | `WidgetId` | yes |  |
| `open` | `boolean` | no | Whether the filtered dropdown is currently shown. |
| `options` | `string[]` | yes | Fixed candidate list `ComboBox::filtered_indices` matches against. |
| `text` | `string` | no | Live edit buffer — what the field currently displays. |

<a id="wire-commandcenter"></a>
#### `CommandCenter`

Declarative description of a command center strip.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `back_enabled` | `boolean` | yes |  |
| `forward_enabled` | `boolean` | yes |  |
| `id` | `WidgetId` | yes |  |
| `search_label` | `string` | no | Text shown inside the search box (e.g. |

<a id="wire-commandcentertarget"></a>
#### `CommandCenterTarget`

Which part of a `CommandCenter` an activation landed on — the payload of `crate::UiEvent::CommandCenterActivated`.

Alias for `"Back" | "Forward" | "Search"`.

<a id="wire-commandline"></a>
#### `CommandLine`

Declarative description of a command line surface.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor_offset` | `number \| null` | no | Byte offset within `text` at which to draw the insert cursor. |
| `id` | `WidgetId` | yes |  |
| `right_align` | `boolean` | no | When `true`, right-align the text (used for count/match displays). |
| `selection` | `[number, number] \| null` | no | Active selection as a `(start, end)` byte-offset pair into `Self::text`, either order. |
| `text` | `string` | yes | Full display text (includes prompt character if any, e.g. |

<a id="wire-contextmenu"></a>
#### `ContextMenu`

Declarative description of a context menu.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bg` | `Color \| null` | no | Background colour override. |
| `glyphs` | `ContextMenuGlyphs` | no | Check-mark / submenu-arrow glyph overrides for this popup. |
| `id` | `WidgetId` | yes |  |
| `items` | `ContextMenuItem[]` | yes |  |
| `placement` | `ContextMenuPlacement` | no | How to position the menu relative to the anchor point. |
| `selected_idx` | `number` | yes | Index of the keyboard-selected item. |

<a id="wire-contextmenuglyphs"></a>
#### `ContextMenuGlyphs`

Optional app-supplied override for the glyphs a context menu paints in place of the default codicon check mark and submenu pull-right arrow.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `checked` | `Icon \| null` | no |  |
| `submenu_arrow` | `Icon \| null` | no |  |

<a id="wire-contextmenuitem"></a>
#### `ContextMenuItem`

One entry in a `ContextMenu`. Also re-exported as `MenuItem` for use as the shared shape across menu-bar dropdowns and right-click menus.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `checked` | `boolean \| null` | no | Checkbox / radio state. |
| `detail` | `StyledText \| null` | no | Optional right-aligned detail (e.g. |
| `disabled` | `boolean` | no | When true, the item is rendered dimmed and click emits no event. |
| `icon` | `Icon \| null` | no | Optional leading icon, painted in the same reserved gutter column `Self::checked`'s `✓` uses. |
| `id` | `WidgetId \| null` | no | `None` = separator (non-interactive); `Some(id)` = action. |
| `key_equivalent` | `Accelerator \| null` | no | Structured keyboard shortcut for the item. |
| `label` | `StyledText` | yes | Label for the item. |
| `submenu` | `ContextMenuItem[] \| null` | no | Nested submenu. |

<a id="wire-contextmenuoverlaydescriptor"></a>
#### `ContextMenuOverlayDescriptor`

The `"context_menu"` `show_overlay` descriptor: the menu's items.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `items` | `ContextMenuItem[]` | yes |  |

<a id="wire-contextmenuplacement"></a>
#### `ContextMenuPlacement`

Preferred placement of a `ContextMenu` relative to its anchor.

Alias for `"AnchorPoint" | "Below" | "Above"`.

<a id="wire-controlwidth"></a>
#### `ControlWidth`

How a `FormField`'s control column (the checkbox / input / slider / dropdown etc., as opposed to its label) is sized. Set on `Form::control_width` as the form-wide default, or on `FormField::control_width` to override a single field. Every non-item-based field kind (`Toggle`, `TextInput`, `PasswordInput`, `TextArea`, `ReadOnly`, `Slider`, `ColorPicker`, `Dropdown`, `ComboBox`, `DatePicker`, `Button`) starts its control at the same column, right after the shared label column, in every mode but `ControlWidth::ContentEnd`, which places it against the row's right edge instead.

Alias for `("Fill" | "Content" | "ContentEnd") | _ControlWidthDict`.

<a id="wire-cursorpos"></a>
#### `CursorPos`

Cursor position within the visible window area (view-line + char column).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes | Column (character index within the line). |
| `view_line` | `number` | yes | Index into `Editor.lines` (0 = topmost visible line). |

<a id="wire-cursorshape"></a>
#### `CursorShape`

Shape of the editor cursor.

Alias for `"Block" | "Bar" | "Underline"`.

<a id="wire-datarow"></a>
#### `DataRow`

One row in a `DataTable`. `cells` must have the same length as the table's `columns`. Missing cells are treated as empty; extra cells are ignored.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cells` | `StyledText[]` | yes |  |
| `decoration` | `Decoration` | no |  |

<a id="wire-datatable"></a>
#### `DataTable`

Declarative description of a `DataTable` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bordered` | `boolean` | no | Reserve a 1-cell/pixel margin on every edge of the table's viewport for a decorative frame, mirroring `crate::primitives::list::ListView::bordered` and `crate::types::TreeStyle::bordered`. |
| `column_overrides` | `(number \| null)[]` | no | Per-column width overrides from user drag. |
| `columns` | `Column[]` | yes |  |
| `footer` | `DataRow \| null` | no | Optional pinned summary/totals row, laid out against the same resolved columns as the body. |
| `glyphs` | `DataTableGlyphs \| null` | no | Sort-direction glyph overrides for the header — see `DataTableGlyphs`. |
| `h_scroll` | `number` | no | Horizontal scroll offset in surface-native units (pixels for GTK, cells for TUI). |
| `has_focus` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `min_total_width` | `number \| null` | no | Minimum total width for all columns. |
| `rows` | `DataRow[]` | yes |  |
| `scroll_offset` | `number` | no |  |
| `selected_idx` | `number \| null` | no |  |
| `show_scrollbar` | `boolean` | no | Show a vertical scrollbar when rows exceed the viewport. |
| `sort` | `[number, SortDirection] \| null` | no | Which column is sorted, and in which direction. |
| `striped` | `boolean` | no | Tint every other body row with `crate::theme::Theme::row_alt_bg` ("zebra striping"), keyed on the absolute row index so a row keeps its shade while scrolling. |

<a id="wire-datatableevent"></a>
#### `DataTableEvent`

Events a `DataTable` emits back to the app.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `HeaderClicked` | `_DataTableEventHeaderClicked` | User clicked a column header — app should toggle sort. |
| `RowActivated` | `_DataTableEventRowActivated` | User pressed Enter while the table had focus and a row was selected — the keyboard equivalent of `Self::RowDoubleClicked`'s mouse gesture. |
| `RowSelected` | `_DataTableEventRowSelected` | User selected a row — arrow-key navigation or a plain single click. |
| `RowDoubleClicked` | `_DataTableEventRowDoubleClicked` | User double-clicked a row — distinct from `RowSelected` (a plain click) so an app can tell "select" apart from "open" without both folding into `RowActivated`. |
| `Scroll` | `_DataTableEventScroll` | User scrolled the table. |
| `ColumnResized` | `_DataTableEventColumnResized` | User dragged a column divider to resize. |
| `SelectionSet` | `_DataTableEventSelectionSet` | The full multi-selection changed (`SelectionMode::Multi`): Ctrl/Cmd click toggled a row, Shift click/extend selected a range, or a keyboard extension grew the set. |
| `ScrollChanged` | `_DataTableEventScrollChanged` | The widget scrolled — a wheel, a scrollbar gesture, or a cursor move brought its row into view. |
| `ColumnFocused` | `_DataTableEventColumnFocused` | Left/Right moved the keyboard's column cursor to `col`. |
| `ContextMenuRequested` | `_DataTableEventContextMenuRequested` | The user asked for row `idx`'s context menu (right-click); `position` is where, in the same coordinates the table was laid out in. |

<a id="wire-datatableglyphs"></a>
#### `DataTableGlyphs`

Optional app-supplied override for the sort-direction glyphs `native_surface_paint` appends to a sorted column's header title.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `sort_ascending` | `Icon \| null` | no |  |
| `sort_descending` | `Icon \| null` | no |  |

<a id="wire-datatableleaf"></a>
#### `DataTableLeaf`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bordered` | `boolean` | no | Reserve a 1-cell/pixel margin on every edge of the table's viewport for a decorative frame, mirroring `crate::primitives::list::ListView::bordered` and `crate::types::TreeStyle::bordered`. |
| `column_overrides` | `(number \| null)[]` | no | Per-column width overrides from user drag. |
| `columns` | `Column[]` | yes |  |
| `footer` | `DataRow \| null` | no | Optional pinned summary/totals row, laid out against the same resolved columns as the body. |
| `glyphs` | `DataTableGlyphs \| null` | no | Sort-direction glyph overrides for the header — see `DataTableGlyphs`. |
| `h_scroll` | `number` | no | Horizontal scroll offset in surface-native units (pixels for GTK, cells for TUI). |
| `has_focus` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `min_total_width` | `number \| null` | no | Minimum total width for all columns. |
| `rows` | `DataRow[]` | yes |  |
| `scroll_offset` | `number` | no |  |
| `selected_idx` | `number \| null` | no |  |
| `show_scrollbar` | `boolean` | no | Show a vertical scrollbar when rows exceed the viewport. |
| `sort` | `[number, SortDirection] \| null` | no | Which column is sorted, and in which direction. |
| `striped` | `boolean` | no | Tint every other body row with `crate::theme::Theme::row_alt_bg` ("zebra striping"), keyed on the absolute row index so a row keeps its shade while scrolling. |
| `items_window` | `ItemsWindow \| null` | no |  |
| `scroll_px` | `number` | no |  |

<a id="wire-dateformat"></a>
#### `DateFormat`

How a `CalendarDate` renders as text.

Alias for `"Iso8601" | "UsSlash" | "EuSlash"`.

<a id="wire-datelocale"></a>
#### `DateLocale`

Locale rules a `DatePicker` renders and navigates by: which weekday starts a week, and how a date formats as text.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `first_weekday` | `Weekday` | yes | The weekday a week starts on (e.g. |
| `format` | `DateFormat` | yes | How `CalendarDate::format` renders a date. |

<a id="wire-datepicker"></a>
#### `DatePicker`

Declarative description of a `DatePicker` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `dropdown` | `boolean` | no | Present the picker as a one-row date field whose calendar opens in a popup below it, instead of as the calendar itself. |
| `focused` | `CalendarDate` | yes | The keyboard-navigable cursor within the grid. |
| `has_focus` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `locale` | `DateLocale` | yes |  |
| `open` | `boolean` | no | Whether a `Self::dropdown` picker's calendar popup is showing. |
| `selected` | `CalendarDate \| null` | no | The confirmed date, if any (set by an `Enter` / click `DatePickerEvent::DateSelected`). |
| `time` | `TimeOfDay \| null` | no | Optional time-of-day field rendered alongside the date grid. |
| `visible_month` | `CalendarDate` | yes | The month this picker displays. |

<a id="wire-decoration"></a>
#### `Decoration`

Optional visual decoration applied to a row beyond its text colour.

Alias for `"Normal" | "Header" | "Muted" | "Error" | "Warning" | "Modified"`.

<a id="wire-diagnosticmark"></a>
#### `DiagnosticMark`

LSP diagnostic span on one line — drives inline underline / squiggle painting plus tooltip-on-hover. `start_col` / `end_col` are character indices within the line's `raw_text`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `end_col` | `number` | yes |  |
| `message` | `string` | yes |  |
| `severity` | `DiagnosticSeverity` | yes |  |
| `start_col` | `number` | yes |  |

<a id="wire-diagnosticseverity"></a>
#### `DiagnosticSeverity`

LSP diagnostic severity, mirrored from `vimcode::core::lsp::DiagnosticSeverity`.

Alias for `"Error" | "Warning" | "Information" | "Hint"`.

<a id="wire-dialogbutton"></a>
#### `DialogButton`

One button on a dialog.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `is_cancel` | `boolean` | no | When true, Escape activates this button (cancel-button convention). |
| `is_default` | `boolean` | no | When true, Enter activates this button (and backends typically style it as the primary). |
| `label` | `string` | yes |  |
| `spec` | `ButtonSpec \| null` | no | Optional `crate::ButtonSpec` override, carrying the B3a vocabulary (variant intent — e.g. |
| `tint` | `Color \| null` | no | Override colour for destructive actions ("Delete", "Discard") on the legacy plain highlighted-row paint path. |

<a id="wire-dialogchromewire"></a>
#### `DialogChromeWire`

A layout dialog's `"dialog"` chrome key, as a binding sends it: plain `"title"`/`"body"` strings rather than `quadraui::StyledText` objects (`"body"` splitting on `/n` into one line each, exactly like `quadraui::compose::MessageDialog::new` does with `quadraui::backend::MessageDialogOptions::body`) and a friendly lowercase `"severity"` word, so the shape matches the `"message_dialog"` overlay kind a binding already exposes. `"buttons"` is omittable — a content-bearing dialog may rely on its own subtree (a form's own Cancel/Save row, a close icon) rather than the chrome's button row to resolve. An empty list routes every event into the content subtree first and leaves the dialog open on an Escape neither the subtree nor a cancel/default button claims; closing then comes from the content itself or the overlay being dismissed by id.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `body` | `string` | no |  |
| `buttons` | `DialogButton[]` | no |  |
| `id` | `string` | yes |  |
| `severity` | `string \| null` | no |  |
| `title` | `string` | no |  |
| `vertical_buttons` | `boolean` | no |  |

<a id="wire-diffeditability"></a>
#### `DiffEditability`

Whether the right pane of the diff is editable.

Alias for `"ReadOnly" | "RightEditable"`.

<a id="wire-diffhunk"></a>
#### `DiffHunk`

A contiguous group of rows to display, bracketed by context lines.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `left_start` | `number` | yes | 1-based line number of the first row's left content in the original left text. |
| `right_start` | `number` | yes | 1-based line number of the first row's right content in the right text. |
| `rows` | `DiffRow[]` | yes | Ordered display rows for this hunk (context + changed lines). |

<a id="wire-diffline"></a>
#### `DiffLine`

Per-line two-way-diff status, mirrored from `vimcode::core::engine::DiffLine`. `None` on `EditorLine.diff_status` means diff mode is not active for the line's window.

Alias for `"Same" | "Added" | "Removed" | "Padding"`.

<a id="wire-diffmode"></a>
#### `DiffMode`

Display mode for the diff widget.

Alias for `"SideBySide" | "Unified"`.

<a id="wire-diffpane"></a>
#### `DiffPane`

Which pane has keyboard focus.

Alias for `"Left" | "Right"`.

<a id="wire-diffrow"></a>
#### `DiffRow`

A single display row in a `DiffHunk`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `DiffRowKind` | yes | Change kind — drives background / foreground colour selection. |
| `left` | `string \| null` | no | Content from the left (original) file, or `None` for padding. |
| `right` | `string \| null` | no | Content from the right (modified) file, or `None` for padding. |

<a id="wire-diffrowkind"></a>
#### `DiffRowKind`

The kind of change a `DiffRow` represents.

Alias for `"Same" | "Changed" | "Removed" | "Added"`.

<a id="wire-diffview"></a>
#### `DiffView`

The `DiffView` primitive: a two-pane or unified diff viewer.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `editability` | `DiffEditability` | yes | Editability of the right pane. |
| `focused_pane` | `DiffPane` | yes | Which pane currently has keyboard focus. |
| `has_focus` | `boolean` | yes | Whether the widget as a whole has keyboard focus. |
| `hunks` | `DiffHunk[]` | yes | Pre-computed diff hunks. |
| `id` | `WidgetId` | yes | Stable identifier for this widget instance. |
| `left` | `string` | yes | Original (left-side) text. |
| `left_label` | `string \| null` | no | Optional label shown above the left pane (e.g. |
| `mode` | `DiffMode` | yes | Display mode — side-by-side (default) or unified. |
| `right` | `string` | yes | Proposed (right-side) text. |
| `right_label` | `string \| null` | no | Optional label shown above the right pane. |
| `scroll_offset` | `number` | yes | Row scroll offset (from the top of the first hunk). |

<a id="wire-diffviewevent"></a>
#### `DiffViewEvent`

What a `DiffView` reports. `scroll_offset` and `focused_pane` are the app's: it writes each back onto the descriptor.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Scrolled` | `_DiffViewEventScrolled` | A wheel notch or PageUp/PageDown scrolled the view to `scroll_offset`, already clamped so the last line stays reachable. |
| `PaneFocused` | `_DiffViewEventPaneFocused` | Tab or a click moved keyboard focus to `pane` (side-by-side mode). |

<a id="wire-draggesturewireevent"></a>
#### `DragGestureWireEvent`

Wire-shaped drag-gesture event `crate::engine::InteractionEngine::handle` dispatches through `crate::engine::EventSink::dispatch_drag_gesture` — mirrors `crate::engine::TreeTableControllerWireEvent`'s own `EventSink` default-method rollout: that method defaults to ignoring the event and reporting `quadraui::Reaction::Continue`, so every existing `crate::engine::EventSink` impl keeps compiling unchanged until a binding deliberately overrides it to thread this to its own callback. Derives `Serialize` only, not `Deserialize`: `HitDetail` — the shape every `*_detail` field below reuses rather than inventing a second drag-specific hit description — has no `Deserialize` of its own either, since neither type has a reverse (host -> engine) decode path.

Alias for `_DragGestureWireEventDragStarted | _DragGestureWireEventDragOver | _DragGestureWireEventDropped`.

<a id="wire-drawop"></a>
#### `DrawOp`

One drawing instruction. Coordinates are LOCAL to the canvas — see the module doc's "Coordinate frame" section.

Alias for `"PopClip" | _DrawOpDict`.

<a id="wire-dropedge"></a>
#### `DropEdge`

Cardinal direction for a split drop.

Alias for `"Left" | "Right" | "Top" | "Bottom"`.

<a id="wire-dualmodepaletteaction"></a>
#### `DualModePaletteAction`

What a `DualModePalette` reports. `DualModePalette::apply` stores the query, mode and highlight changes; the confirmations and the dismissal are the host's to act on.

Alias for `"Cancelled" | _DualModePaletteActionDict`.

<a id="wire-editor"></a>
#### `Editor`

Declarative description of one editor viewport (window / pane).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `active_indent_col` | `number \| null` | no | Indent-guide column highlighted as "active" (cursor's scope). |
| `bracket_match_positions` | `[number, number][]` | no | Bracket-pair positions to highlight (cursor bracket + match). |
| `code_action_lines` | `number[]` | no | Lines with available LSP code actions (lightbulb gutter glyph). |
| `cursor` | `EditorCursor \| null` | no | Primary cursor (position + shape). |
| `cursorline` | `boolean` | yes | Whether to draw cursorline highlight. |
| `diagnostic_gutter` | `Record<string, unknown>` | no | Per-line worst diagnostic severity. |
| `extra_cursors` | `CursorPos[]` | no | Secondary cursor positions for multi-cursor (Alt-D). |
| `extra_selections` | `EditorSelection[]` | no | Extra selections for Ctrl+D word-multi-cursor. |
| `gutter_char_width` | `number` | no | Width of the line-number gutter in *character cells* (0 = no gutter). |
| `has_breakpoints` | `boolean` | yes | Whether to show the breakpoint gutter column. |
| `has_git_diff` | `boolean` | yes | Whether the buffer has git diff data (controls git column). |
| `id` | `WidgetId` | yes |  |
| `is_active` | `boolean` | yes | Whether this is the focused window. |
| `lightbulb_glyph` | `string` | no | Glyph the rasteriser draws in the gutter for lines with an available LSP code action. |
| `lines` | `EditorLine[]` | yes | Visible lines, one per row. |
| `max_col` | `number` | no | Max line length in the buffer (character cells, excl. |
| `paint_options` | `EditorPaintOptions` | no | Scrollbar and reserved-strip geometry — see `EditorPaintOptions`. |
| `rect` | `Rect` | yes | Pixel-space rectangle (used by GTK / Win-GUI; TUI ignores). |
| `scroll_left` | `number` | no | Number of character columns scrolled horizontally. |
| `scroll_top` | `number` | yes | Index of the first visible buffer line. |
| `selection` | `EditorSelection \| null` | no | Active visual selection. |
| `show_active_bg` | `boolean` | yes | Whether to render with the slightly-different active-window background — only true when `is_active` AND there are multiple windows. |
| `tabstop` | `number` | yes | Tab stop width for expanding `/t` to spaces (TUI rendering). |
| `total_lines` | `number` | yes | Total lines in the buffer (drives vertical scrollbar geometry). |
| `yank_highlight` | `EditorSelection \| null` | no | Transient post-yank flash region. |

<a id="wire-editorcursor"></a>
#### `EditorCursor`

Primary cursor: position plus shape.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `pos` | `CursorPos` | yes |  |
| `shape` | `CursorShape` | yes |  |

<a id="wire-editorevent"></a>
#### `EditorEvent`

A gesture on an `Editor`, resolved against its layout.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Pointer` | `_EditorEventPointer` | A pointer press, double-click, release, or a move while a drag is in progress. |
| `Scrollbar` | `_EditorEventScrollbar` | A press on a scrollbar track, or a move or left release while that press holds the pointer. |
| `Wheel` | `_EditorEventWheel` | A scroll-wheel gesture over the editor, in the platform's wheel units; positive `dy` scrolls toward the top of the buffer. |

<a id="wire-editorhit"></a>
#### `EditorHit`

Classification of a hit-test result within an editor viewport.

Alias for `("VScrollbar" | "HScrollbar" | "Empty") | _EditorHitDict`.

<a id="wire-editorline"></a>
#### `EditorLine`

One visible line in an `Editor` viewport, ready for the rasteriser.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `annotation` | `string \| null` | no | Inline annotation (virtual text — git blame, plugin output, etc.) shown after line content in `annotation_fg`. |
| `colorcolumns` | `number[]` | no | Column positions where the colorcolumn background should be painted. |
| `diagnostics` | `DiagnosticMark[]` | no | LSP diagnostic spans on this line (may be empty). |
| `diff_status` | `DiffLine \| null` | no | Two-way diff status. |
| `folded_line_count` | `number` | no | Number of lines hidden in the fold (0 unless `is_fold_header`). |
| `ghost_suffix` | `string \| null` | no | AI ghost text shown after the cursor on this line (Insert mode). |
| `git_diff` | `GitLineStatus \| null` | no | Git diff status (`Added` / `Modified` / `Deleted`). |
| `gutter_text` | `string` | yes | Pre-formatted gutter text (e.g. |
| `indent_guides` | `number[]` | no | Column positions where indent-guide rules should be drawn. |
| `is_breakpoint` | `boolean` | no | True when there is a DAP breakpoint set on this line. |
| `is_conditional_bp` | `boolean` | no | True when the breakpoint has a condition / hit count / log message (rasteriser draws ◆ instead of ●). |
| `is_current_line` | `boolean` | no | True when the cursor is on this line (drives gutter highlight). |
| `is_dap_current` | `boolean` | no | True when the DAP adapter is currently stopped at this line. |
| `is_fold_header` | `boolean` | no | True when this is the header of a closed fold. |
| `is_ghost_continuation` | `boolean` | no | True for virtual rows showing AI completion continuation lines. |
| `is_wrap_continuation` | `boolean` | no | True when this row is a wrap-continuation (the 2nd+ visual row of a long buffer line). |
| `line_idx` | `number` | yes | Buffer line index this rendered row corresponds to. |
| `raw_text` | `string` | yes | Raw UTF-8 text (may include a trailing `/n`). |
| `segment_col_offset` | `number` | no | Character offset within the buffer line where this visual segment begins. |
| `spans` | `EditorStyledSpan[]` | yes | Syntax-highlight + search-match spans (byte-offset based, into `raw_text`). |
| `spell_errors` | `SpellMark[]` | no | Spell-check error spans on this line (may be empty). |

<a id="wire-editorpaintoptions"></a>
#### `EditorPaintOptions`

Geometry options an `Editor` carries on its descriptor (`Editor::paint_options`): which scrollbar to reserve, how wide, and what strip to leave free beside the text. `crate::Primitive::layout` reads them, so the layout a host hit-tests is the one paint used.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `right_reserve_w` | `number` | no | Width of the strip reserved between the text area and the vertical scrollbar, in the surface's own units — see `Editor::layout_with_options_and_reserve`. |
| `suppress_v_scrollbar` | `boolean` | no | Suppress this editor's own vertical scrollbar even though the buffer overflows the viewport. |
| `v_scrollbar_w` | `number \| null` | no | Width of the vertical scrollbar column on a pixel surface, in the surface's own units — see `Editor::layout_with_options_and_v_scrollbar_w`. |

<a id="wire-editorselection"></a>
#### `EditorSelection`

A normalised visual-selection range (start ≤ end) in buffer coordinates.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `end_col` | `number` | yes | Last selected column (Char / Block; ignored for Line). |
| `end_line` | `number` | yes | Last selected buffer line (inclusive). |
| `kind` | `SelectionKind` | yes |  |
| `start_col` | `number` | yes | First selected column (Char / Block; ignored for Line). |
| `start_line` | `number` | yes | First selected buffer line. |

<a id="wire-editorstyledspan"></a>
#### `EditorStyledSpan`

A styled byte-range within a line's text.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `end_byte` | `number` | yes |  |
| `start_byte` | `number` | yes |  |
| `style` | `Style` | yes |  |

<a id="wire-emptybody"></a>
#### `EmptyBody`

Empty-state body. Rendered centered (cross-axis) and vertically centered (main-axis) within the body bounds. Covers everything from a plain "No data" up to a VSCode-style welcome view ("Open Folder" / "Clone Repository" buttons).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `action` | `HeaderAction \| null` | no | Optional clickable call-to-action button below the hint. |
| `hint` | `StyledText \| null` | no | Secondary hint line — smaller / dimmer. |
| `icon` | `Icon \| null` | no | Optional centered icon above the message. |
| `text` | `StyledText` | yes | Primary message line. |

<a id="wire-fieldkind"></a>
#### `FieldKind`

The input variant carried by a `FormField`.

Alias for `("Label" | "Button") | _FieldKindDict`.

<a id="wire-findreplaceclicktarget"></a>
#### `FindReplaceClickTarget`

Click target within the find/replace overlay. `FindReplacePanel::hit_test` resolves a native point to one of these, and the panel's own `crate::interactive::Interactive::handle` turns it into a `FindReplaceEvent`.

Alias for `("ToggleCase" | "ToggleWholeWord" | "ToggleRegex" | "PrevMatch" | "NextMatch" | "ToggleInSelection" | "Close" | "TogglePreserveCase" | "ReplaceCurrent" | "ReplaceAll" | "Chevron") | _FindReplaceClickTargetDict`.

<a id="wire-findreplaceevent"></a>
#### `FindReplaceEvent`

What a `FindReplacePanel` reports. The panel is controlled: every field is the host's, and `FindReplacePanel::apply_event` writes one of these back onto it.

Alias for `("PrevMatch" | "NextMatch" | "ReplaceCurrent" | "ReplaceAll" | "Close") | _FindReplaceEventDict`.

<a id="wire-findreplacefield"></a>
#### `FindReplaceField`

One of the panel's two text fields.

Alias for `"Find" | "Replace"`.

<a id="wire-findreplacepanel"></a>
#### `FindReplacePanel`

The inline find/replace overlay displayed at the top-right of the active editor group.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `case_sensitive` | `boolean` | no | Toggle button states (find row). |
| `cursor` | `number` | no | Cursor position within the focused field (char offset). |
| `focus` | `number` | no | Which field has focus: 0 = find, 1 = replace. |
| `group_bounds` | `Rect` | yes | Bounding rect of the active editor group, in the surface's own units. |
| `hit_regions` | `[FrHitRegion, FindReplaceClickTarget][]` | no | Hit regions for click handling, in char-cell units relative to the panel content corner (inside borders). |
| `id` | `WidgetId` | no | This overlay's id — the one its `FindReplaceEvent`s are reported under. |
| `in_selection` | `boolean` | no | Find in selection mode. |
| `match_info` | `string` | no | "N of M" match count display, or "No results" / empty. |
| `panel_width` | `number` | no | Panel width in char cells, borders included. |
| `preserve_case` | `boolean` | no | Toggle button states (replace row). |
| `query` | `string` | yes | Current query text in the find field. |
| `replace_all_glyph` | `string` | no | App-supplied glyph string for the "replace all" button. |
| `replace_one_glyph` | `string` | no | App-supplied glyph string for the "replace current" button. |
| `replacement` | `string` | no | Current replacement text (only shown when `show_replace` is true). |
| `sel_anchor` | `number \| null` | no | Selection anchor in the focused field. |
| `show_replace` | `boolean` | no | Whether the replace row is visible. |
| `use_regex` | `boolean` | no |  |
| `whole_word` | `boolean` | no |  |

<a id="wire-findreplacetoggle"></a>
#### `FindReplaceToggle`

One of the panel's on/off switches.

Alias for `"ShowReplace" | "CaseSensitive" | "WholeWord" | "Regex" | "PreserveCase" | "InSelection"`.

<a id="wire-float"></a>
#### `Float`

Declarative description of a float.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `anchor` | `Anchor` | yes | Where to anchor the float — relative to a point, a cell, or another widget's rect. |
| `bg` | `Color \| null` | no | Background colour override. |
| `border` | `boolean` | no | Whether to paint a 1-unit border stroke around the float. |
| `focusable` | `boolean` | no | Whether this float takes keyboard focus while open. |
| `id` | `WidgetId` | yes |  |
| `transparent` | `boolean` | no | Skip the background fill entirely, leaving a border-only (when `border` is set) overlay through which whatever is already painted underneath shows. |

<a id="wire-fontweightvalue"></a>
#### `FontWeightValue`

Alias for `"normal" | "bold"`.

<a id="wire-form"></a>
#### `Form`

Declarative description of a `Form` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `control_width` | `ControlWidth` | no | Form-wide default for how a field's control column is sized. |
| `fields` | `FormField[]` | yes |  |
| `focused_field` | `WidgetId \| null` | no | `WidgetId` of the field that currently has keyboard focus. |
| `has_focus` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `label_align` | `LabelAlign` | no | Form-wide default alignment for `Left`-positioned labels inside their label column. |
| `label_position` | `LabelPosition` | no | Form-wide default for where a field's label renders. |
| `label_width` | `number \| null` | no | Form-wide fixed width (cells for TUI, device pixels for GTK / macOS / Win) reserved for the label column when a field resolves to `Left` positioning. |
| `min_control_width` | `number \| null` | no | Form-wide floor (cells for TUI, device pixels for GTK / macOS / Win) under which a field's resolved control column never shrinks, regardless of `ControlWidth` mode — guards against a `Content`-sized or explicit `Fixed` column too narrow to show its value. |
| `scroll_offset` | `number` | no | How many rows have been scrolled past. |
| `spacing` | `FormSpacing` | no | Form-wide padding/item-gap/row-alignment knobs — see `FormSpacing`. |

<a id="wire-formevent"></a>
#### `FormEvent`

Events a `Form` emits back to the app.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `ToggleChanged` | `_FormEventToggleChanged` | A `Toggle` field's value changed. |
| `TextInputChanged` | `_FormEventTextInputChanged` | A `TextInput` field's text changed. |
| `TextInputCommitted` | `_FormEventTextInputCommitted` | A `TextInput` received Enter while focused. |
| `SliderChanged` | `_FormEventSliderChanged` | A `Slider` field's value changed (drag, arrow key, Home/End). |
| `ColorChanged` | `_FormEventColorChanged` | A `ColorPicker` field produced a new colour. |
| `DropdownChanged` | `_FormEventDropdownChanged` | A `Dropdown` field committed a new selection. |
| `SegmentedControlChanged` | `_FormEventSegmentedControlChanged` | A `SegmentedControl` field committed a new selection. |
| `DateChanged` | `_FormEventDateChanged` | A `DatePicker` field confirmed a new date (Enter on the focused day). |
| `DateFocusMoved` | `_FormEventDateFocusMoved` | A `DatePicker`'s keyboard cursor moved to a different day without confirming it — the arrow-key / `PageUp` / `PageDown` scrubbing `crate::primitives::date_picker::DatePickerEvent::FocusMoved` reports. |
| `DateOpenChanged` | `_FormEventDateOpenChanged` | A dropdown `DatePicker` asked for its calendar popup to open or close — `crate::primitives::date_picker::DatePickerEvent::OpenChanged`. |
| `FocusChanged` | `_FormEventFocusChanged` | Keyboard focus moved to a different field. |
| `ButtonClicked` | `_FormEventButtonClicked` | A `Button` was clicked or activated with Enter / Space. |
| `ToolbarButtonClicked` | `_FormEventToolbarButtonClicked` | An action button inside a `FieldKind::Toolbar` field was clicked. |
| `KeyPressed` | `_FormEventKeyPressed` | A key was pressed while the form had focus and the primitive did not consume it. |
| `ComboBoxTextChanged` | `_FormEventComboBoxTextChanged` | A `ComboBox` field's text buffer changed (typing filters the dropdown). |
| `ComboBoxHighlightChanged` | `_FormEventComboBoxHighlightChanged` | A `ComboBox` field's highlighted dropdown item changed (arrow-key navigation while the dropdown is open). |
| `ComboBoxCommitted` | `_FormEventComboBoxCommitted` | A `ComboBox` field confirmed a value (Enter). |
| `ComboBoxCancelled` | `_FormEventComboBoxCancelled` | A `ComboBox` field's in-progress edit was cancelled (Escape), reverting the live `text` buffer back to `committed`. |
| `NumberChanged` | `_FormEventNumberChanged` | A `NumberInput` field's committed value changed via a stepper click or arrow-key stepping — immediate, not waiting for a confirm step. |
| `NumberDraftChanged` | `_FormEventNumberDraftChanged` | A `NumberInput` field's in-progress typed draft changed. |
| `NumberCommitted` | `_FormEventNumberCommitted` | A `NumberInput` field's typed draft was confirmed (Enter): parsed, clamped to `[min, max]`, and rounded to the embedded input's `precision`. |
| `NumberCancelled` | `_FormEventNumberCancelled` | A `NumberInput` field's in-progress typed draft was cancelled (Escape), discarded without touching the committed value. |
| `RadioChanged` | `_FormEventRadioChanged` | A `RadioGroup` field's selection changed, via either the arrow keys or a click on one of its options. |

<a id="wire-formfield"></a>
#### `FormField`

One row in a `Form`: a label + an input.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `control_width` | `ControlWidth \| null` | no | Overrides the form-wide `Form::control_width` for this field only. |
| `disabled` | `boolean` | no | When true, the field is rendered dimmed and will not emit events. |
| `hint` | `StyledText` | no | Tooltip / hint text rendered below the field (or to the right, depending on backend). |
| `id` | `WidgetId` | yes |  |
| `kind` | `FieldKind` | yes |  |
| `label` | `StyledText` | yes | Rendered to the left of the field. |
| `label_align` | `LabelAlign \| null` | no | Overrides the form-wide `Form::label_align` for this field only. |
| `label_position` | `LabelPosition \| null` | no | Overrides the form-wide `Form::label_position` for this field only. |
| `label_width` | `number \| null` | no | Overrides the form-wide `Form::label_width` for this field. |
| `min_control_width` | `number \| null` | no | Overrides the form-wide `Form::min_control_width` for this field. |
| `row_align` | `RowAlign \| null` | no | Overrides the form-wide `Form::spacing`'s `FormSpacing::row_align` for this field only. |
| `tab_index` | `number \| null` | no | Explicit position in the Tab/Shift-Tab focus cycle. |
| `validation` | `ValidationState \| null` | no | When set, backends render a red/yellow indicator + message. |

<a id="wire-formspacing"></a>
#### `FormSpacing`

Form-wide padding/item-gap/row-alignment knobs. Set on `Form::spacing`; every field defaults to `0.0`/`None`/`Start`, reproducing a `Form` built before this type existed bit-for-bit.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `item_gap` | `number \| null` | no | Overrides every item-bearing field's own backend-supplied gap between items (`ToggleGroup` / `ButtonRow` / `SegmentedControl` / `Toolbar`'s buttons). |
| `padding` | `number` | no | Uniform inset, applied on all four edges of the form's content area before any field is laid out. |
| `row_align` | `RowAlign` | no | Form-wide default for how an item-bearing field's items pack within their row's available width (see `RowAlign`). |

<a id="wire-frhitregion"></a>
#### `FrHitRegion`

A hit region within the find/replace panel, expressed in character-cell units relative to the panel's top-left **content** corner (inside borders).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes | Column offset from panel content-left edge. |
| `row` | `number` | yes | Row: 0 = find row, 1 = replace row. |
| `width` | `number` | yes | Width of this region in char cells. |

<a id="wire-gitlinestatus"></a>
#### `GitLineStatus`

Per-line git diff status, mirrored from `vimcode::core::git::GitLineStatus`.

Alias for `"Added" | "Modified" | "Deleted"`.

<a id="wire-globalshortcutid"></a>
#### `GlobalShortcutId`

Opaque handle to a registered OS-wide global shortcut, returned by `crate::PlatformServices::register_global_shortcut`, consumed by `crate::PlatformServices::unregister_global_shortcut`, and carried by `crate::UiEvent::GlobalShortcut` when the OS delivers the hotkey.

Alias for `number`.

<a id="wire-gridtrack"></a>
#### `GridTrack`

One column or row track's sizing strategy for `resolve_grid_tracks`.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Fixed` | `number` | Exact size in the container's native units (cells / pixels). |
| `Percent` | `number` | Share of the *original* container extent along this axis, `0.0..=1.0`. |
| `Weight` | `number` | Proportional weight (CSS `fr` unit). |
| `Content` | `number` | Exact natural-content size the caller already measured (e.g. |

<a id="wire-headeraction"></a>
#### `HeaderAction`

Right-aligned action button in a section header (or in a toolbar aux).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `enabled` | `boolean` | no | Disabled actions render dimmed and are hit-test-inert (clicks fall through to `HeaderHit::TitleArea`). |
| `icon` | `Icon` | yes |  |
| `id` | `string` | yes |  |
| `tooltip` | `string \| null` | no |  |

<a id="wire-headerhit"></a>
#### `HeaderHit`

Sub-zone of a section header that was hit.

Alias for `("Chevron" | "TitleArea") | _HeaderHitDict`.

<a id="wire-helpoverlayaction"></a>
#### `HelpOverlayAction`

The `HelpOverlay` widget's action: what `HelpOverlay::handle_event` and its `Interactive` report, and what `HelpOverlay::apply` writes back.

Alias for `"Opened" | "Closed"`.

<a id="wire-hitdetail"></a>
#### `HitDetail`

The sub-region of a primitive an incoming pointer event landed on, attached to the enriched event's `"hit"` key (alongside `widget`, the primitive's own id) so the binding's own event handler can react to "row 3" or "the divider" without re-deriving hit geometry itself.

Alias for `_HitDetailTreeRow | _HitDetailTreeChevron | _HitDetailListTitle | _HitDetailListItem | _HitDetailDataTableHeader | _HitDetailDataTableHeaderDivider | _HitDetailDataTableRow | _HitDetailDataTableFooter | _HitDetailTabBarTab | _HitDetailTabBarTabClose | _HitDetailTabBarScrollLeft | _HitDetailTabBarScrollRight | _HitDetailTabBarRightSegment | _HitDetailTextDisplayLine | _HitDetailTextDisplayScrollbarThumb | _HitDetailTextDisplayScrollbarTrackBefore | _HitDetailTextDisplayScrollbarTrackAfter | _HitDetailSplitFirstPane | _HitDetailSplitSecondPane | _HitDetailSplitDivider | _HitDetailSplitTreeDivider | _HitDetailTreeScrollbarTrackBefore | _HitDetailTreeScrollbarThumb | _HitDetailTreeScrollbarTrackAfter | _HitDetailExtraBody | _HitDetailWidgetBody | _HitDetailCanvasRegion | _HitDetailScrollBody | _HitDetailScrollScrollbarTrackBefore | _HitDetailScrollScrollbarThumb | _HitDetailScrollScrollbarTrackAfter`.

<a id="wire-icon"></a>
#### `Icon`

`Icon`'s two accepted wire shapes. `serde` tries an `#[serde(untagged)]` enum's variants in declaration order, so `Codicon` (the only shape with a `codicon` field) is listed first.

Alias for `_Icon1 | _Icon2`.

<a id="wire-icongrid"></a>
#### `IconGrid`

Declarative description of an `IconGrid` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `has_focus` | `boolean` | no | Whether this grid currently holds keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `items` | `IconGridItem[]` | yes |  |
| `scroll_offset` | `number` | no | Scroll offset in whole tile-rows (not items) — a "row" here is one horizontal band of tiles. |
| `selected_idx` | `number` | no |  |
| `tile_size` | `[number, number] \| null` | no | Optional caller-supplied tile size hint, in the layout caller's native units (cells on TUI, pixels on GUI — same convention as `crate::primitives::list::ListItemMeasure::height`). |

<a id="wire-icongriditem"></a>
#### `IconGridItem`

One tile in an `IconGrid`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `decoration` | `Decoration` | no |  |
| `detail` | `StyledText \| null` | no |  |
| `icon` | `Icon` | yes |  |
| `id` | `WidgetId` | yes |  |
| `image` | `Image \| null` | no | Optional thumbnail, painted in place of `Self::icon` on a GUI backend (TUI always paints `icon`, never this — see the module doc's "TUI story"). |
| `label` | `StyledText` | yes | Label shown under the icon/thumbnail, truncated with `…` if it overflows the tile. |

<a id="wire-image"></a>
#### `Image`

Declarative description of an `Image` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `fallback_text` | `string` | yes | Required, not optional: TUI paints this in place of real pixels whenever it can't rasterise them — a bare `tui` build (no `tui-images` feature), a terminal that hasn't confirmed Kitty- protocol graphics support, or a real decode failure — and it's what GTK/macOS paint nothing in place of on a decode failure (bad path, corrupt bytes), so a host can still show *something* on every backend and every failure mode. |
| `fit` | `ImageFit` | yes |  |
| `id` | `WidgetId` | yes |  |
| `intrinsic_size` | `[number, number] \| null` | no | The image's natural pixel dimensions, if known up front. |
| `source` | `ImageSource` | yes |  |

<a id="wire-imagefit"></a>
#### `ImageFit`

How an image's intrinsic size maps onto its target rect. Named after the equivalent CSS `object-fit` keywords since the semantics are identical.

Alias for `"Contain" | "Cover" | "Fill" | "None"`.

<a id="wire-imagesource"></a>
#### `ImageSource`

Where the image's encoded bytes come from.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Bytes` | `number[]` | Raw encoded image bytes (PNG/JPEG/SVG/...) — the backend's decoder sniffs the format from content, so no separate MIME hint is carried here. |
| `Path` | `string` | Filesystem path to an image file, decoded lazily by the backend at the given paint size/scale and cached for reuse. |

<a id="wire-inlineinput"></a>
#### `InlineInput`

Single-line text input rendered inline (used for SC commit messages, search-within-section, etc.).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `caret` | `number` | no | Caret position in chars from the start of `text`. |
| `has_focus` | `boolean` | no | Whether this input currently has keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `placeholder` | `string \| null` | no |  |
| `text` | `string` | yes |  |

<a id="wire-itemswindow"></a>
#### `ItemsWindow`

A `{start, total}` virtualised window lifted out of a `"list"`/`"tree"`/ `"data_table"`/`"text_display"` leaf's own descriptor dict by `items_window_from_value` — `start` is the index, into the widget's true row/item/line count `total`, that this frame's `items`/`rows`/ `lines` payload begins at. Like `crate::scroll_px_from_value`, this is **not** a field any of `ListView`/`TreeView`/`DataTable`/`TextDisplay` declares: a render callback that never sets `"items_window"` is completely unaffected, and one that does only sends the slice of rows currently visible (plus whatever prefetch margin it chooses) instead of every row up front.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `start` | `number` | yes |  |
| `total` | `number` | yes |  |

<a id="wire-justifyvalue"></a>
#### `JustifyValue`

Alias for `"start" | "center" | "end" | "space_between"`.

<a id="wire-key"></a>
#### `Key`

Keyboard key identity — a printable character or a named non-printable.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Char` | `string` | Printable character (after keyboard layout resolution). |
| `Named` | `NamedKey` | Named non-printable key. |

<a id="wire-keybinding"></a>
#### `KeyBinding`

The key combination to bind.

Alias for `("Save" | "Open" | "New" | "Close" | "Copy" | "Cut" | "Paste" | "Find" | "Replace" | "Quit" | "Undo" | "Redo" | "SelectAll") | _KeyBindingDict`.

<a id="wire-labelalign"></a>
#### `LabelAlign`

Horizontal alignment of a `Left`-positioned label inside its label column. Only meaningful when the resolved `LabelPosition` is `Left` and a label column width is established (either an explicit `Form::label_width`/`FormField::label_width` or the backend's own widest-label measurement).

Alias for `"Start" | "Center" | "End"`.

<a id="wire-labelposition"></a>
#### `LabelPosition`

Where a field's label renders relative to its input. Set on `Form::label_position` as the form-wide default, or on `FormField::label_position` to override a single field. Modeled on Ant Design's `Form` `layout` prop (`horizontal` / `vertical` / `inline`), with `Hidden` added for label-less rows.

Alias for `"Left" | "Top" | "Inline" | "Hidden"`.

<a id="wire-layoutcolumnnode"></a>
#### `LayoutColumnNode`

The sibling keys every node dict may carry.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `align` | `AlignValue \| null` | no |  |
| `class` | `ClassValue \| null` | no |  |
| `column` | `LayoutNode[]` | yes |  |
| `gap` | `number \| null` | no |  |
| `justify` | `JustifyValue \| null` | no |  |
| `margin` | `number \| null` | no |  |
| `max` | `number \| null` | no |  |
| `min` | `number \| null` | no |  |
| `padding` | `PaddingValue \| null` | no |  |
| `size` | `SectionSize \| null` | no |  |
| `style` | `StyleValue \| null` | no |  |
| `wire_version` | `number \| null` | no |  |
| `wrap` | `boolean \| null` | no |  |

<a id="wire-layoutdialogdescriptor"></a>
#### `LayoutDialogDescriptor`

A `{"dialog": {...}, "content": <node>}` layout-dialog overlay.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `content` | `LayoutNode` | yes |  |
| `dialog` | `DialogChromeWire` | yes |  |

<a id="wire-layoutgridnode"></a>
#### `LayoutGridNode`

The sibling keys every node dict may carry.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `class` | `ClassValue \| null` | no |  |
| `grid` | `LayoutGridPayload` | yes |  |
| `margin` | `number \| null` | no |  |
| `max` | `number \| null` | no |  |
| `min` | `number \| null` | no |  |
| `padding` | `PaddingValue \| null` | no |  |
| `size` | `SectionSize \| null` | no |  |
| `style` | `StyleValue \| null` | no |  |
| `wire_version` | `number \| null` | no |  |

<a id="wire-layoutgridpayload"></a>
#### `LayoutGridPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cells` | `LayoutNode[]` | yes |  |
| `columns` | `GridTrack[]` | yes |  |
| `rows` | `GridTrack[]` | yes |  |

<a id="wire-layoutleafnode"></a>
#### `LayoutLeafNode`

A widget leaf: an `id` plus exactly one widget-kind key holding that kind's descriptor.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `a11y` | `LeafA11y \| null` | no |  |
| `activity_bar` | `ActivityBar` | no |  |
| `board` | `BoardModel` | no |  |
| `button` | `Button` | no |  |
| `canvas` | `Canvas` | no |  |
| `cell_grid` | `CellGrid` | no |  |
| `chart` | `Chart` | no |  |
| `checkbox` | `Checkbox` | no |  |
| `class` | `ClassValue \| null` | no |  |
| `combo_box` | `ComboBox` | no |  |
| `command_center` | `CommandCenter` | no |  |
| `command_line` | `CommandLine` | no |  |
| `context_menu` | `ContextMenu` | no |  |
| `data_table` | `DataTable \| DataTableLeaf` | no |  |
| `date_picker` | `DatePicker` | no |  |
| `diff_view` | `DiffView` | no |  |
| `draggable` | `boolean \| null` | no |  |
| `drop_target` | `boolean \| null` | no |  |
| `editor` | `Editor` | no |  |
| `find_replace` | `FindReplacePanel` | no |  |
| `form` | `Form` | no |  |
| `icon_grid` | `IconGrid` | no |  |
| `id` | `string` | yes |  |
| `image` | `Image` | no |  |
| `list` | `ListView \| ListViewLeaf` | no |  |
| `markdown` | `Markdown` | no |  |
| `max` | `number \| null` | no |  |
| `menu_bar` | `AppMenu` | no |  |
| `message_list` | `MessageList` | no |  |
| `min` | `number \| null` | no |  |
| `minimap` | `Minimap` | no |  |
| `multi_section_view` | `MultiSectionView` | no |  |
| `number_input` | `NumberInput` | no |  |
| `palette` | `Palette` | no |  |
| `password` | `PasswordInput` | no |  |
| `pipeline_view` | `PipelineView` | no |  |
| `progress` | `ProgressBar` | no |  |
| `radio_group` | `RadioGroup` | no |  |
| `scripted_widget` | `ScriptedWidgetDescriptor` | no |  |
| `scroll_view` | `ScrollView` | no |  |
| `scrollbar` | `Scrollbar` | no |  |
| `select` | `Select` | no |  |
| `size` | `SectionSize \| null` | no |  |
| `slider` | `Slider` | no |  |
| `spinner` | `Spinner` | no |  |
| `status_bar` | `StatusBar` | no |  |
| `style` | `StyleValue \| null` | no |  |
| `switch` | `Switch` | no |  |
| `tab_bar` | `TabBar` | no |  |
| `terminal` | `Terminal` | no |  |
| `text_display` | `TextDisplay \| TextDisplayLeaf` | no |  |
| `text_field` | `TextField` | no |  |
| `text_input` | `TextInput` | no |  |
| `textarea` | `TextArea` | no |  |
| `toolbar` | `Toolbar` | no |  |
| `tree` | `TreeView \| TreeViewLeaf` | no |  |
| `tree_table` | `TreeTable` | no |  |
| `tree_table_controller` | `TreeTableControllerDescriptor` | no |  |
| `wire_version` | `number \| null` | no |  |

<a id="wire-layoutnode"></a>
#### `LayoutNode`

One layout-tree node: a container or a widget leaf.

Alias for `LayoutRowNode | LayoutColumnNode | LayoutGridNode | LayoutSplitNode | LayoutTabsNode | LayoutSidebarPanelNode | LayoutScrollNode | LayoutSplitTreeNode | LayoutLeafNode`.

<a id="wire-layoutrownode"></a>
#### `LayoutRowNode`

The sibling keys every node dict may carry.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `align` | `AlignValue \| null` | no |  |
| `class` | `ClassValue \| null` | no |  |
| `gap` | `number \| null` | no |  |
| `justify` | `JustifyValue \| null` | no |  |
| `margin` | `number \| null` | no |  |
| `max` | `number \| null` | no |  |
| `min` | `number \| null` | no |  |
| `padding` | `PaddingValue \| null` | no |  |
| `row` | `LayoutNode[]` | yes |  |
| `size` | `SectionSize \| null` | no |  |
| `style` | `StyleValue \| null` | no |  |
| `wire_version` | `number \| null` | no |  |
| `wrap` | `boolean \| null` | no |  |

<a id="wire-layoutscrollnode"></a>
#### `LayoutScrollNode`

The sibling keys every node dict may carry.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `class` | `ClassValue \| null` | no |  |
| `margin` | `number \| null` | no |  |
| `max` | `number \| null` | no |  |
| `min` | `number \| null` | no |  |
| `padding` | `PaddingValue \| null` | no |  |
| `scroll` | `LayoutScrollPayload` | yes |  |
| `size` | `SectionSize \| null` | no |  |
| `style` | `StyleValue \| null` | no |  |
| `wire_version` | `number \| null` | no |  |

<a id="wire-layoutscrollpayload"></a>
#### `LayoutScrollPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `axis` | `ScrollAxisValue \| null` | no |  |
| `content` | `LayoutNode` | yes |  |
| `id` | `string` | yes |  |
| `offset` | `number \| null` | no |  |

<a id="wire-layoutsidebarpanelnode"></a>
#### `LayoutSidebarPanelNode`

The sibling keys every node dict may carry.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `class` | `ClassValue \| null` | no |  |
| `max` | `number \| null` | no |  |
| `min` | `number \| null` | no |  |
| `sidebar_panel` | `LayoutSidebarPanelPayload` | yes |  |
| `size` | `SectionSize \| null` | no |  |
| `style` | `StyleValue \| null` | no |  |
| `wire_version` | `number \| null` | no |  |

<a id="wire-layoutsidebarpanelpayload"></a>
#### `LayoutSidebarPanelPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `content` | `LayoutNode` | yes |  |
| `id` | `string` | yes |  |
| `toolbar` | `Toolbar \| null` | no |  |
| `toolbar_height` | `number \| null` | no |  |

<a id="wire-layoutsplitnode"></a>
#### `LayoutSplitNode`

The sibling keys every node dict may carry.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `class` | `ClassValue \| null` | no |  |
| `gap` | `number \| null` | no |  |
| `margin` | `number \| null` | no |  |
| `max` | `number \| null` | no |  |
| `min` | `number \| null` | no |  |
| `padding` | `PaddingValue \| null` | no |  |
| `size` | `SectionSize \| null` | no |  |
| `split` | `LayoutSplitPayload` | yes |  |
| `style` | `StyleValue \| null` | no |  |
| `wire_version` | `number \| null` | no |  |

<a id="wire-layoutsplitpayload"></a>
#### `LayoutSplitPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `first` | `LayoutNode` | yes |  |
| `first_min` | `number \| null` | no |  |
| `id` | `string` | yes |  |
| `orientation` | `OrientationValue` | yes |  |
| `ratio` | `number` | yes |  |
| `second` | `LayoutNode` | yes |  |
| `second_min` | `number \| null` | no |  |

<a id="wire-layoutsplittreenode"></a>
#### `LayoutSplitTreeNode`

The sibling keys every node dict may carry.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `class` | `ClassValue \| null` | no |  |
| `max` | `number \| null` | no |  |
| `min` | `number \| null` | no |  |
| `size` | `SectionSize \| null` | no |  |
| `split_tree` | `LayoutSplitTreePayload` | yes |  |
| `style` | `StyleValue \| null` | no |  |
| `wire_version` | `number \| null` | no |  |

<a id="wire-layoutsplittreepayload"></a>
#### `LayoutSplitTreePayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `root` | `SplitTreeNode` | yes |  |

<a id="wire-layouttabentrynode"></a>
#### `LayoutTabEntryNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `closable` | `boolean \| null` | no |  |
| `content` | `LayoutNode` | yes |  |
| `id` | `string` | yes |  |
| `is_dirty` | `boolean \| null` | no |  |
| `is_preview` | `boolean \| null` | no |  |
| `title` | `string` | yes |  |

<a id="wire-layouttabsnode"></a>
#### `LayoutTabsNode`

The sibling keys every node dict may carry.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `class` | `ClassValue \| null` | no |  |
| `margin` | `number \| null` | no |  |
| `max` | `number \| null` | no |  |
| `min` | `number \| null` | no |  |
| `padding` | `PaddingValue \| null` | no |  |
| `size` | `SectionSize \| null` | no |  |
| `style` | `StyleValue \| null` | no |  |
| `tabs` | `LayoutTabsPayload` | yes |  |
| `wire_version` | `number \| null` | no |  |

<a id="wire-layouttabspayload"></a>
#### `LayoutTabsPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `active_accent` | `Color \| null` | no |  |
| `closable` | `boolean \| null` | no |  |
| `compact` | `boolean \| null` | no |  |
| `focused_index` | `number \| null` | no |  |
| `id` | `string` | yes |  |
| `right_segments` | `TabBarSegment[] \| null` | no |  |
| `scroll_offset` | `number \| null` | no |  |
| `selected` | `number \| null` | no |  |
| `tabs` | `LayoutTabEntryNode[]` | yes |  |

<a id="wire-leafa11y"></a>
#### `LeafA11y`

A leaf's `"a11y"` override.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `description` | `string \| null` | no |  |
| `label` | `string \| null` | no |  |
| `live` | `A11yLiveValue \| null` | no |  |

<a id="wire-listitem"></a>
#### `ListItem`

One row in a `ListView`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `decoration` | `Decoration` | no |  |
| `detail` | `StyledText \| null` | no | Optional right-aligned secondary text. |
| `icon` | `Icon \| null` | no | Optional left-aligned icon before the text. |
| `text` | `StyledText` | yes | Primary row text. |

<a id="wire-listview"></a>
#### `ListView`

Declarative description of a `ListView` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bordered` | `boolean` | no | When true, backends draw a `╭─╮ │ │ ╰─╯` frame around the list and inset items by 1 cell on each side. |
| `h_scroll` | `number` | no | Horizontal scroll offset in **display cells** (terminal columns — a CJK/emoji glyph counts as 2, matching `crate::text_util::char_cell_width`), not characters. |
| `has_focus` | `boolean` | no | Whether this list currently holds keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `items` | `ListItem[]` | yes |  |
| `max_content_width` | `number \| null` | no | Total width (in **display cells**, not characters — see `crate::text_util::char_cell_width`) of the widest item row, including the 2-cell selection prefix, any icon, and the main text. |
| `scroll_offset` | `number` | no |  |
| `selected_idx` | `number` | yes |  |
| `show_v_scrollbar` | `boolean` | no | When `true`, backends reserve the rightmost column of the list area for a vertical scrollbar and paint a track/thumb there. |
| `striped` | `boolean` | no | Tint every other non-selected, non-header item with `crate::theme::Theme::row_alt_bg` ("zebra striping"), keyed on the absolute item index so an item keeps its shade while scrolling. |
| `title` | `StyledText \| null` | no | Optional header row shown above the items. |

<a id="wire-listviewevent"></a>
#### `ListViewEvent`

Events a `ListView` emits back to the app.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `SelectionChanged` | `_ListViewEventSelectionChanged` | Keyboard / mouse moved selection to a different row. |
| `ItemActivated` | `_ListViewEventItemActivated` | User pressed Enter while the list had focus and a row was selected — the keyboard equivalent of `Self::ItemDoubleClicked`'s mouse gesture, and the event `crate::a11y::A11yAction::Click` routes to for a screen reader's own "activate" action. |
| `ItemDoubleClicked` | `_ListViewEventItemDoubleClicked` | User double-clicked a row — distinct from `SelectionChanged` (a plain click) so an app can tell "select" apart from "open" without both folding into `ItemActivated`. |
| `KeyPressed` | `_ListViewEventKeyPressed` | A key was pressed while the list had focus and the primitive did not consume it. |
| `SelectionSet` | `_ListViewEventSelectionSet` | The full multi-selection changed (`SelectionMode::Multi`): Ctrl/Cmd click toggled a row, Shift click/extend selected a range, or a keyboard extension grew the set. |

<a id="wire-listviewleaf"></a>
#### `ListViewLeaf`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bordered` | `boolean` | no | When true, backends draw a `╭─╮ │ │ ╰─╯` frame around the list and inset items by 1 cell on each side. |
| `h_scroll` | `number` | no | Horizontal scroll offset in **display cells** (terminal columns — a CJK/emoji glyph counts as 2, matching `crate::text_util::char_cell_width`), not characters. |
| `has_focus` | `boolean` | no | Whether this list currently holds keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `items` | `ListItem[]` | yes |  |
| `max_content_width` | `number \| null` | no | Total width (in **display cells**, not characters — see `crate::text_util::char_cell_width`) of the widest item row, including the 2-cell selection prefix, any icon, and the main text. |
| `scroll_offset` | `number` | no |  |
| `selected_idx` | `number` | yes |  |
| `show_v_scrollbar` | `boolean` | no | When `true`, backends reserve the rightmost column of the list area for a vertical scrollbar and paint a track/thumb there. |
| `striped` | `boolean` | no | Tint every other non-selected, non-header item with `crate::theme::Theme::row_alt_bg` ("zebra striping"), keyed on the absolute item index so an item keeps its shade while scrolling. |
| `title` | `StyledText \| null` | no | Optional header row shown above the items. |
| `items_window` | `ItemsWindow \| null` | no |  |
| `scroll_px` | `number` | no |  |

<a id="wire-markdown"></a>
#### `Markdown`

A scrollable, read-only markdown viewer.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `scroll_offset` | `number` | no | Lines scrolled off the top. |
| `source` | `string` | yes | Raw markdown text. |

<a id="wire-menudef"></a>
#### `MenuDef`

One top-level menu and its dropdown items.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no | When true, the item is rendered dimmed and does not open. |
| `id` | `WidgetId` | yes | The menu's id, the bar item's. |
| `label` | `string` | yes | Display label, e.g. |
| `submenu` | `ContextMenuItem[] \| null` | no | The dropdown's items, in order. |

<a id="wire-messagedialogoverlaydescriptor"></a>
#### `MessageDialogOverlayDescriptor`

The `"message_dialog"` `show_overlay` descriptor. `severity` is `"info"`, `"question"`, `"warning"` or `"error"`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `body` | `string` | yes |  |
| `buttons` | `DialogButton[]` | yes |  |
| `severity` | `string \| null` | no |  |
| `title` | `string` | yes |  |

<a id="wire-messagelist"></a>
#### `MessageList`

Declarative description of a scrollable styled-row list.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `rows` | `MessageRow[]` | yes |  |
| `scroll_top` | `number` | no | Index of the first row to draw at the top of the visible area. |

<a id="wire-messagelistevent"></a>
#### `MessageListEvent`

What a `MessageList` reports. `scroll_top` is the app's: it writes it back onto the descriptor.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Scrolled` | `_MessageListEventScrolled` | A wheel notch scrolled the list to `scroll_top`, clamped so the last row stays on screen. |

<a id="wire-messagerow"></a>
#### `MessageRow`

A single row in a `MessageList`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `fg` | `Color` | yes |  |
| `indent` | `number` | no | Left-indent offset in surface units (cells / pixels). |
| `scale` | `number` | no | Font-scale multiplier (`1.0` = body, `2.0`/`1.5`/`1.2` for H1/H2/H3). |
| `spans` | `StyledSpan[]` | no | Per-span rich styling. |
| `text` | `string` | yes |  |

<a id="wire-minimap"></a>
#### `Minimap`

Declarative description of a `Minimap` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `lines` | `MinimapLine[]` | yes | Pre-aggregated lines (app does the sampling, e.g. |
| `scale` | `MinimapScale` | no | The on-screen size of one character cell on a pixel surface — see `MinimapScale`. |
| `sizing` | `MinimapSizing \| null` | no | The row-pitch strategy `crate::Primitive::layout` uses. |
| `syntax_spans` | `MinimapSpan[]` | yes | Syntax colour spans, already aggregated to a dominant colour per *cell* by `aggregate_spans` — one backend-sized cell, not one buffer line. |
| `total_buffer_lines` | `number` | yes | Total buffer lines (used to compute the scroll thumb). |
| `visible_row_count` | `number` | yes | Number of entries in `lines` the editor's viewport currently shows. |
| `visible_row_start` | `number` | yes | Index into `lines` at the top of the editor's current viewport. |

<a id="wire-minimapevent"></a>
#### `MinimapEvent`

A gesture on a `Minimap`, resolved against its layout.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Seek` | `_MinimapEventSeek` | Scroll the editor to `fraction` (`0.0` top, `1.0` bottom) of the file: the pointer's height within the strip. |

<a id="wire-minimapline"></a>
#### `MinimapLine`

One sampled line in a `Minimap`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `line_idx` | `number` | yes | Original buffer line number this entry stands in for. |
| `text` | `string` | yes |  |

<a id="wire-minimapscale"></a>
#### `MinimapScale`

VS Code-parity minimap render scale (`editor.minimap.scale`) — how large a logical pixel cell one character occupies in `MinimapRenderMode::Characters`.

Alias for `"One" | "Two"`.

<a id="wire-minimapsizing"></a>
#### `MinimapSizing`

How `Minimap::layout_with_sizing` sizes rows across `bounds.height` — issue #667.

Alias for `"Fill" | _MinimapSizingDict`.

<a id="wire-minimapspan"></a>
#### `MinimapSpan`

One aggregated colour span — the *output* of `aggregate_spans`, at whatever cell granularity the target backend paints (one entry per coloured cell, not per raw syntax token).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `end_col` | `number` | yes |  |
| `line_idx` | `number` | yes |  |
| `start_col` | `number` | yes |  |

<a id="wire-modifiers"></a>
#### `Modifiers`

Keyboard modifier state carried alongside input events.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `alt` | `boolean` | no |  |
| `cmd` | `boolean` | no |  |
| `ctrl` | `boolean` | no |  |
| `shift` | `boolean` | no |  |

<a id="wire-mousebutton"></a>
#### `MouseButton`

Mouse button identity.

Alias for `("Left" | "Right" | "Middle" | "X2" | "X1") | _MouseButtonDict`.

<a id="wire-movedir"></a>
#### `MoveDir`

Semantic direction for keyboard navigation.

Alias for `"Up" | "Down" | "Left" | "Right"`.

<a id="wire-multisectionview"></a>
#### `MultiSectionView`

Declarative description of a `MultiSectionView` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `active_section` | `number \| null` | no | Index of the section that has keyboard focus (for arrow-key navigation between sections, etc.). |
| `allow_collapse` | `boolean` | no | Whether clicking a header (or pressing Space on a focused header) toggles its `collapsed` flag. |
| `allow_resize` | `boolean` | no | Whether dividers between sections are user-draggable. |
| `axis` | `Axis` | yes |  |
| `has_focus` | `boolean` | no | Whether the whole view has keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `panel_scroll` | `number` | no | Panel-level scroll offset in main-axis units. |
| `scroll_mode` | `ScrollMode` | yes |  |
| `sections` | `Section[]` | yes |  |

<a id="wire-multisectionviewevent"></a>
#### `MultiSectionViewEvent`

What a `MultiSectionView` reports through its `crate::interactive::Interactive` impl. Every visible change it asks for is the app's to apply: the view keeps collapse, scroll, sizes and aux-input text as descriptor fields.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `HeaderActivated` | `_MultiSectionViewEventHeaderActivated` | A header was clicked, or activated from the keyboard (Enter or Space on its focus stop): its title area, its chevron, or one of its action buttons. |
| `CollapseChanged` | `_MultiSectionViewEventCollapseChanged` | A header asked to collapse or expand its section; the app writes `collapsed` onto `Section::collapsed`. |
| `SectionResized` | `_MultiSectionViewEventSectionResized` | A divider drag resized the two sections either side of it; the app writes both sizes as `SectionSize::Fixed`. |
| `ScrollChanged` | `_MultiSectionViewEventScrollChanged` | A scrollbar gesture asked to scroll. |
| `AuxActivated` | `_MultiSectionViewEventAuxActivated` | An aux-row toolbar button or a custom aux was clicked. |
| `AuxInputChanged` | `_MultiSectionViewEventAuxInputChanged` | Editing changed an aux input's text or caret; the app writes both onto its `InlineInput` (`caret` in chars, like the field). |
| `AuxInputSubmitted` | `_MultiSectionViewEventAuxInputSubmitted` | Enter was pressed in an aux input. |
| `Body` | `[number, UiEvent]` | An event section `.0`'s body widget reported, as that widget's own `crate::UiEvent` under its own id. |

<a id="wire-namedkey"></a>
#### `NamedKey`

Non-printable keyboard keys that have a stable cross-platform name.

Alias for `("Escape" | "Tab" | "BackTab" | "Enter" | "Backspace" | "Delete" | "Insert" | "Home" | "End" | "PageUp" | "PageDown" | "Up" | "Down" | "Left" | "Right" | "NumLock" | "ScrollLock" | "CapsLock" | "Menu") | _NamedKeyDict`.

<a id="wire-numberinput"></a>
#### `NumberInput`

Declarative description of a bounded numeric field with stepper buttons.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no | When `true`, every method below returns `None` — same "disabled means inert, not hidden" contract every other primitive's `disabled` flag follows. |
| `draft` | `string \| null` | no | In-progress typed text overriding the displayed value until committed or cancelled. |
| `focused` | `boolean` | no | Whether this field currently holds keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `max` | `number` | yes |  |
| `min` | `number` | yes |  |
| `precision` | `number` | no | Decimal places `value` is rounded to on commit/step. |
| `step` | `number` | no | Amount `NumberInput::increment`/`NumberInput::decrement` and `NumberInput::handle_wheel` step by. |
| `value` | `number` | no | Last committed value. |

<a id="wire-orientationvalue"></a>
#### `OrientationValue`

Alias for `"horizontal" | "vertical"`.

<a id="wire-paddingaxis"></a>
#### `PaddingAxis`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `x` | `number \| null` | no |  |
| `y` | `number \| null` | no |  |

<a id="wire-paddingsides"></a>
#### `PaddingSides`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bottom` | `number \| null` | no |  |
| `left` | `number \| null` | no |  |
| `right` | `number \| null` | no |  |
| `top` | `number \| null` | no |  |

<a id="wire-paddingvalue"></a>
#### `PaddingValue`

A `"padding"` value: one number for every side, an `{x, y}` pair, or per-side numbers.

Alias for `number | PaddingAxis | PaddingSides`.

<a id="wire-palette"></a>
#### `Palette`

Declarative description of a `Palette` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `create_label` | `string \| null` | no | When set, a pinned "create" action row is rendered below the scrollable item list (e.g. |
| `has_focus` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `items` | `PaletteItem[]` | yes | Filtered, pre-scored visible items in display order. |
| `mode` | `PaletteMode` | no | Interaction mode — `List` (default) or `Input`. |
| `preview` | `PalettePreview \| null` | no | When present, the palette renders a split layout: item list on the left, preview content on the right. |
| `query` | `string` | yes | Current search query text. |
| `query_cursor` | `number` | no | Cursor byte offset in `query`. |
| `scroll_offset` | `number` | no | How many rows have been scrolled past. |
| `selected_idx` | `number` | yes | Index into `items` of the currently highlighted row. |
| `show_query` | `boolean` | no | When `false`, the query input row and separator are hidden — producing a popup-style list (tab switcher, branch picker). |
| `title` | `string` | yes | Header text shown above the query input, e.g. |
| `total_count` | `number` | no | Total number of items in the underlying source (before filtering). |

<a id="wire-paletteevent"></a>
#### `PaletteEvent`

Events a `Palette` emits back to the app.

Alias for `"Closed" | _PaletteEventDict`.

<a id="wire-paletteitem"></a>
#### `PaletteItem`

One row in a `Palette`'s filtered result list.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `depth` | `number` | no | Indentation level for tree-structured items. |
| `detail` | `StyledText \| null` | no | Optional right-aligned secondary text (line number, shortcut, file path suffix). |
| `expandable` | `boolean` | no | Whether this item shows an expand/collapse arrow. |
| `expanded` | `boolean` | no | Arrow direction when `expandable` is true (`▾` vs `▸`). |
| `icon` | `Icon \| null` | no | Optional left-aligned icon. |
| `match_positions` | `number[]` | no | Character positions inside `text` that match the current query. |
| `text` | `StyledText` | yes | Primary row text (file name, command name, buffer label). |

<a id="wire-palettemode"></a>
#### `PaletteMode`

Which interaction mode the palette is operating in.

Alias for `"List" | "Input"`.

<a id="wire-palettepreview"></a>
#### `PalettePreview`

Preview pane content shown alongside the item list when the palette operates in split-layout mode (file pickers, symbol pickers).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `highlight_line` | `number \| null` | no | Line to visually highlight (e.g. |
| `lines` | `StyledText[]` | yes | Syntax-highlighted content lines. |
| `scroll_offset` | `number` | no | Scroll offset into `lines`. |
| `title` | `string \| null` | no | Optional title shown above the preview content (e.g. |

<a id="wire-paneldefinitiondescriptor"></a>
#### `PanelDefinitionDescriptor`

One `run_shell` activity-bar panel entry.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `icon` | `string \| null` | no |  |
| `id` | `string` | yes |  |
| `title` | `string \| null` | no |  |
| `tooltip` | `string \| null` | no |  |

<a id="wire-passwordinput"></a>
#### `PasswordInput`

A standalone masked single-line input. The value is held in plaintext; only painting masks it.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number \| null` | no | The caret as a byte offset into `value`; `None` puts it at the end. |
| `has_focus` | `boolean` | no | Whether to paint the focused wash and caret. |
| `id` | `WidgetId` | yes |  |
| `mask_char` | `string` | no | The glyph painted for each character of the value. |
| `placeholder` | `string` | no | Shown dimmed while the value is empty. |
| `value` | `string` | no | The plaintext value. |

<a id="wire-pathpickeraction"></a>
#### `PathPickerAction`

What a path picker reports: a confirmed path or a dismissal, plus the browsing changes the host applies onto the picker's `PathPickerState` (the pickers' `apply` does it).

Alias for `"Cancelled" | _PathPickerActionDict`.

<a id="wire-pipelineevent"></a>
#### `PipelineEvent`

Events emitted by a `PipelineView`.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `StageAction` | `_PipelineEventStageAction` | User clicked (or pressed Enter on) the action button of a stage. |
| `StageSelected` | `_PipelineEventStageSelected` | User clicked the body of a stage box (not the action button). |
| `KeyPressed` | `_PipelineEventKeyPressed` | Key pressed while the widget has focus but not consumed by nav. |
| `StageFocused` | `_PipelineEventStageFocused` | Left/Right moved keyboard focus to stage `index`; the host writes it back onto `PipelineView::focused_stage`. |

<a id="wire-pipelinestage"></a>
#### `PipelineStage`

A single stage in a `PipelineView`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `action` | `string \| null` | no | Optional action button text shown at the bottom of the box (e.g. |
| `label` | `string` | yes | Display label for this stage (e.g. |
| `status` | `StageStatus` | yes | Execution status controlling colour and icon. |

<a id="wire-pipelineview"></a>
#### `PipelineView`

Declarative description of a horizontal pipeline widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `focused_stage` | `number \| null` | no | Index of the keyboard-focused stage, if any. |
| `id` | `WidgetId` | yes |  |
| `stages` | `PipelineStage[]` | yes | Ordered list of stages rendered left-to-right. |

<a id="wire-point"></a>
#### `Point`

Cursor position in the backend's native units.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `x` | `number` | yes |  |
| `y` | `number` | yes |  |

<a id="wire-pointerkind"></a>
#### `PointerKind`

Which pointer gesture a `RoutedEvent::Pointer` carries.

Alias for `("Move" | "Enter" | "Leave") | _PointerKindDict`.

<a id="wire-popoverdescriptor"></a>
#### `PopoverDescriptor`

A `{"float": {...}, "content": <node>}` popover overlay.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `content` | `LayoutNode` | yes |  |
| `float` | `Float` | yes |  |

<a id="wire-powereventkind"></a>
#### `PowerEventKind`

The payload of `crate::UiEvent::PowerStateChanged` — which power transition occurred. `Serialize`/`Deserialize` (unlike `PowerSaveKind`/ `ThermalState` above) because it travels inside a `UiEvent`, and every `UiEvent` payload satisfies that bound — see `event.rs`'s module doc.

Alias for `"Suspend" | "Resume" | "OnBattery" | "OnAc" | "LockScreen" | "UnlockScreen"`.

<a id="wire-progressbar"></a>
#### `ProgressBar`

Declarative description of a progress bar.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `accent` | `Color \| null` | no | Override the fill colour. |
| `cancellable` | `boolean` | no | When true, a cancel affordance is drawn at the trailing edge (see `ProgressBarHit::Cancel`). |
| `frame_idx` | `number` | no | Animation frame (same convention as `Spinner`) for indeterminate mode. |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | no | Label rendered above or inline with the bar. |
| `value` | `number \| null` | no | `Some(f)` = determinate at fraction `f` (clamped `0.0..=1.0`); `None` = indeterminate, use `frame_idx` for animation. |

<a id="wire-radiogroup"></a>
#### `RadioGroup`

Declarative description of a standalone radio group.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `focused` | `boolean` | no | Whether this group currently holds keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `options` | `RadioOption[]` | yes |  |
| `selected` | `number \| null` | no | Index into `RadioGroup::options` that is both the current selection and the keyboard cursor. |

<a id="wire-radiooption"></a>
#### `RadioOption`

A single option within a `RadioGroup`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |

<a id="wire-rect"></a>
#### `Rect`

Rectangular region in the backend's native units.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `height` | `number` | yes |  |
| `width` | `number` | yes |  |
| `x` | `number` | yes |  |
| `y` | `number` | yes |  |

<a id="wire-renderframedict"></a>
#### `RenderFrameDict`

A flat render frame: one optional descriptor per widget kind.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `activity_bar` | `ActivityBar \| null` | no |  |
| `board` | `BoardModel \| null` | no |  |
| `button` | `Button \| null` | no |  |
| `canvas` | `Canvas \| null` | no |  |
| `cell_grid` | `CellGrid \| null` | no |  |
| `chart` | `Chart \| null` | no |  |
| `checkbox` | `Checkbox \| null` | no |  |
| `combo_box` | `ComboBox \| null` | no |  |
| `command_center` | `CommandCenter \| null` | no |  |
| `command_line` | `CommandLine \| null` | no |  |
| `context_menu` | `ContextMenu \| null` | no |  |
| `data_table` | `(DataTable \| DataTableLeaf) \| null` | no |  |
| `date_picker` | `DatePicker \| null` | no |  |
| `diff_view` | `DiffView \| null` | no |  |
| `editor` | `Editor \| null` | no |  |
| `find_replace` | `FindReplacePanel \| null` | no |  |
| `form` | `Form \| null` | no |  |
| `icon_grid` | `IconGrid \| null` | no |  |
| `image` | `Image \| null` | no |  |
| `list` | `(ListView \| ListViewLeaf) \| null` | no |  |
| `markdown` | `Markdown \| null` | no |  |
| `menu_bar` | `AppMenu \| null` | no |  |
| `message_list` | `MessageList \| null` | no |  |
| `minimap` | `Minimap \| null` | no |  |
| `multi_section_view` | `MultiSectionView \| null` | no |  |
| `number_input` | `NumberInput \| null` | no |  |
| `palette` | `Palette \| null` | no |  |
| `password` | `PasswordInput \| null` | no |  |
| `pipeline_view` | `PipelineView \| null` | no |  |
| `progress` | `ProgressBar \| null` | no |  |
| `radio_group` | `RadioGroup \| null` | no |  |
| `scroll_view` | `ScrollView \| null` | no |  |
| `scrollbar` | `Scrollbar \| null` | no |  |
| `select` | `Select \| null` | no |  |
| `slider` | `Slider \| null` | no |  |
| `spinner` | `Spinner \| null` | no |  |
| `status_bar` | `StatusBar \| null` | no |  |
| `switch` | `Switch \| null` | no |  |
| `tab_bar` | `TabBar \| null` | no |  |
| `terminal` | `Terminal \| null` | no |  |
| `text_display` | `(TextDisplay \| TextDisplayLeaf) \| null` | no |  |
| `text_field` | `TextField \| null` | no |  |
| `text_input` | `TextInput \| null` | no |  |
| `textarea` | `TextArea \| null` | no |  |
| `toolbar` | `Toolbar \| null` | no |  |
| `tree` | `(TreeView \| TreeViewLeaf) \| null` | no |  |
| `tree_table` | `TreeTable \| null` | no |  |
| `tree_table_controller` | `TreeTableControllerDescriptor \| null` | no |  |
| `wire_version` | `number` | no |  |

<a id="wire-renderoutput"></a>
#### `RenderOutput`

What a render callback returns: a layout tree, or a flat frame with one optional descriptor per widget kind.

Alias for `LayoutNode | RenderFrameDict`.

<a id="wire-rowalign"></a>
#### `RowAlign`

How an item-bearing `FieldKind`'s items (`ToggleGroup` / `ButtonRow` / `SegmentedControl` / `Toolbar`) pack within their row's available width — the space between `items_start_x` (from the field's `FormFieldMeasure`) and the row's right edge. Set on `FormSpacing::row_align` as the form-wide default, or on `FormField::row_align` to override a single field.

Alias for `"Start" | "Center" | "End" | "Stretch"`.

<a id="wire-rowsrequestedevent"></a>
#### `RowsRequestedEvent`

The `{id, start, count}` a binding should ask its app's render callback for next, computed by `rows_requested_for_window` once scrolling moves outside the slice an `ItemsWindow` says is currently loaded.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `count` | `number` | yes |  |
| `id` | `WidgetId` | yes |  |
| `start` | `number` | yes |  |

<a id="wire-scriptedwidgetdescriptor"></a>
#### `ScriptedWidgetDescriptor`

The wire shape a `"scripted_widget"` layout-tree leaf's own descriptor dict carries: just this frame's `"props"`, defaulting to `Value::Null` when absent (so a bare `{}` descriptor, for a widget with no props of its own, decodes cleanly) — no `measure`/`paint`/`on_event`/`a11y` shape of its own, since those live on the `ScriptWidget` object a binding already registered against this leaf's id via `register_scripted_widget` before the tree containing it was decoded (see this module's own doc, "Splicing into a decoded tree without a `Deserialize` impl").

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `props` | `unknown` | no |  |

<a id="wire-scrollaxis"></a>
#### `ScrollAxis`

Orientation of a scrollbar.

Alias for `"Vertical" | "Horizontal"`.

<a id="wire-scrollaxisvalue"></a>
#### `ScrollAxisValue`

Alias for `"vertical" | "horizontal"`.

<a id="wire-scrolldelta"></a>
#### `ScrollDelta`

Scroll-wheel delta. Positive `y` = scroll up (toward the top of content). Backends that report scroll in lines/cells/pixels normalise to their native unit before emitting.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `x` | `number` | yes |  |
| `y` | `number` | yes |  |

<a id="wire-scrollmode"></a>
#### `ScrollMode`

Scroll model for a `MultiSectionView`.

Alias for `"PerSection" | "WholePanel"`.

<a id="wire-scrollview"></a>
#### `ScrollView`

Declarative scroll-offset state for a generic scrollable region.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `axis` | `ScrollAxis` | no |  |
| `content_len` | `number` | yes | Total scrollable extent of the child content, in the host's own units (rows, wrapped lines, pixels — see the module doc). |
| `focused` | `boolean` | no | Whether this view currently holds keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `scroll_offset` | `number` | no | First visible content unit. |
| `viewport` | `Rect` | yes | The clip region, in surface-native units. |

<a id="wire-scrollviewevent"></a>
#### `ScrollViewEvent`

Semantic event produced by `ScrollView::handle_wheel`, `ScrollView::handle_key`, or `ScrollView::scroll_by`.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Scrolled` | `_ScrollViewEventScrolled` | The resolved new offset, already clamped to `[0, max_offset()]`. |

<a id="wire-scrollbar"></a>
#### `Scrollbar`

Declarative description of a scrollbar.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `axis` | `ScrollAxis` | yes |  |
| `dragging` | `boolean` | no | User is actively dragging the thumb — rasterisers may apply an even brighter / fully opaque highlight. |
| `hovered` | `boolean` | no | Cursor is hovering over the scrollbar — rasterisers may brighten the thumb. |
| `id` | `WidgetId` | yes |  |
| `thumb_len` | `number` | yes |  |
| `thumb_start` | `number` | yes |  |
| `track` | `Rect` | yes |  |

<a id="wire-section"></a>
#### `Section`

One section in a `MultiSectionView`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `aux` | `SectionAux \| null` | no | Optional widget rendered between header and body (commit input, search, toolbar). |
| `body` | `SectionBody` | yes |  |
| `collapsed` | `boolean` | no |  |
| `header` | `SectionHeader` | yes |  |
| `id` | `string` | yes |  |
| `max_size` | `number \| null` | no | Ceiling on resolved size in main-axis units. |
| `min_size` | `number \| null` | no | Floor on resolved size in main-axis units (after sizing strategy is applied). |
| `size` | `SectionSize` | yes |  |

<a id="wire-sectionaux"></a>
#### `SectionAux`

Auxiliary widget rendered between a section's header and its body.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Input` | `InlineInput` | Single-line text input (e.g. |
| `Toolbar` | `HeaderAction[]` | Inline action toolbar. |
| `Search` | `InlineInput` | Search-within-section input. |
| `Custom` | `WidgetId` | Escape hatch — host paints in returned aux bounds. |

<a id="wire-sectionbody"></a>
#### `SectionBody`

Body content of a section. Composes existing quadraui primitives; `Custom` is the escape hatch for app-defined widgets the rasteriser can't paint (host paints in returned `SectionLayout::body_bounds`).

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Tree` | `TreeView` |  |
| `List` | `ListView` |  |
| `Form` | `Form` |  |
| `Terminal` | `Terminal` |  |
| `MessageList` | `MessageList` |  |
| `Text` | `StyledText[]` | Static styled-line content. |
| `Chart` | `Chart` | Chart (sparkline, line, bar). |
| `Empty` | `EmptyBody` | Welcome / empty-state view. |
| `Custom` | `WidgetId` | Custom widget — host paints in `body_bounds` after consulting the layout. |

<a id="wire-sectionheader"></a>
#### `SectionHeader`

Header row content for a section.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `actions` | `HeaderAction[]` | no | Right-aligned action buttons. |
| `badge` | `StyledText \| null` | no | Right-of-title status indicator (e.g. |
| `icon` | `Icon \| null` | no |  |
| `show_chevron` | `boolean` | no | Whether to draw the leading chevron (▾/▸). |
| `title` | `StyledText` | yes |  |

<a id="wire-sectionsize"></a>
#### `SectionSize`

Sizing strategy for a single section along the main axis.

Alias for `("Content" | "EqualShare") | _SectionSizeDict`.

<a id="wire-select"></a>
#### `Select`

A standalone single-choice option picker.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no | A disabled select paints dimmed and ignores every gesture. |
| `focused` | `boolean` | no | Whether to paint the focused-row wash. |
| `id` | `WidgetId` | yes |  |
| `options` | `string[]` | yes | The options, in order. |
| `selected_idx` | `number` | no | The chosen option's index into `options`. |

<a id="wire-selectionkind"></a>
#### `SelectionKind`

Visual-selection mode.

Alias for `"Char" | "Line" | "Block"`.

<a id="wire-selectionmode"></a>
#### `SelectionMode`

Selection behaviour for row-oriented widgets (`TreeView`, `ListView`, `DataTable`).

Alias for `"None" | "Single" | "Multi"`.

<a id="wire-series"></a>
#### `Series`

One data series in a chart.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color \| null` | no | Override colour. |
| `data` | `number[]` | yes | Y-values, evenly spaced along the x-axis. |
| `fill` | `boolean` | no | Fill the area under the line with a translucent wash of the series colour on pixel backends, and with braille dots down to the plot floor on TUI (`ChartKind::Line` only; ignored for Sparkline and the bar kinds). |
| `label` | `string` | yes |  |

<a id="wire-shellconfigdescriptor"></a>
#### `ShellConfigDescriptor`

`run_shell`'s `config` dict.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `activity_bar_width` | `number \| null` | no |  |
| `bottom_items` | `PanelDefinitionDescriptor[] \| null` | no |  |
| `bottom_panel` | `BottomPanelConfigDescriptor \| null` | no |  |
| `bottom_panel_height_lh` | `number \| null` | no |  |
| `default_sidebar_width` | `number \| null` | no |  |
| `has_bottom_panel` | `boolean \| null` | no |  |
| `has_command_line` | `boolean \| null` | no |  |
| `has_status_bar` | `boolean \| null` | no |  |
| `has_title_bar` | `boolean \| null` | no |  |
| `max_sidebar_width` | `number \| null` | no |  |
| `min_sidebar_width` | `number \| null` | no |  |
| `panels` | `PanelDefinitionDescriptor[] \| null` | no |  |
| `position` | `SidebarPositionValue \| null` | no |  |
| `sidebar_visible` | `boolean \| null` | no |  |
| `title` | `string` | yes |  |
| `title_bar_height_lh` | `number \| null` | no |  |

<a id="wire-shellevent"></a>
#### `ShellEvent`

Semantic events emitted by `ShellView::handle_event`.

Alias for `("SidebarHidden" | "BottomPanelHidden" | "Consumed" | "Ignored") | _ShellEventDict`.

<a id="wire-side"></a>
#### `Side`

Side of an `Anchor`'s `rect` that an overlay prefers to open on.

Alias for `"Top" | "Bottom" | "Left" | "Right" | "AtPoint"`.

<a id="wire-sidebaraction"></a>
#### `SidebarAction`

What a `Sidebar` reports.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `HeaderActivated` | `_SidebarActionHeaderActivated` | A header was clicked, or the empty space of a section body: the section becomes the active one, with no row selected. |
| `ActiveSectionChanged` | `_SidebarActionActiveSectionChanged` | Tab or Shift+Tab moved the active section. |
| `CollapseChanged` | `_SidebarActionCollapseChanged` | A header click collapsed or expanded its section. |
| `RowSelected` | `_SidebarActionRowSelected` | A tree row was clicked or selected from the keyboard: the section becomes active with `path` selected. |
| `RowActivated` | `_SidebarActionRowActivated` | A row was double-clicked, or Enter pressed on the selected row. |
| `RowToggleExpand` | `_SidebarActionRowToggleExpand` | A branch row's chevron was clicked, or Space (or Left/Right) pressed on it. |
| `ContextMenuRequested` | `_SidebarActionContextMenuRequested` | A right-click on a row — or, with `SidebarSection::empty_space_context_menu` set, on the empty space below a tree's rows, with an empty `path`. |
| `ScrollChanged` | `_SidebarActionScrollChanged` | A tree section scrolled to row `offset`. |
| `PanelScrolled` | `_SidebarActionPanelScrolled` | The panel scrolled to `offset` (`ScrollMode::WholePanel`). |
| `EditChanged` | `_SidebarActionEditChanged` | A keystroke changed the row being renamed: its new text, caret and selection anchor (byte offsets). |
| `EditConfirmed` | `_SidebarActionEditConfirmed` | Enter confirmed a rename. |
| `EditCancelled` | `_SidebarActionEditCancelled` | Escape cancelled a rename. |
| `Form` | `_SidebarActionForm` | A form section's form reported `event`. |

<a id="wire-sidebarpositionvalue"></a>
#### `SidebarPositionValue`

Alias for `"left" | "right"`.

<a id="wire-slider"></a>
#### `Slider`

A standalone numeric slider over `[min, max]`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no | A disabled slider paints dimmed and ignores every gesture. |
| `focused` | `boolean` | no | Whether to paint the focused-row wash. |
| `id` | `WidgetId` | yes |  |
| `max` | `number` | yes | The high end of the range. |
| `min` | `number` | yes | The low end of the range. |
| `step` | `number` | no | How far one arrow key moves the value. |
| `value` | `number` | yes | The current value. |

<a id="wire-sortdirection"></a>
#### `SortDirection`

Sort direction indicator for column headers.

Alias for `"Ascending" | "Descending"`.

<a id="wire-spellmark"></a>
#### `SpellMark`

Misspelled-word span on one line — drives spell-check underline.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `end_col` | `number` | yes |  |
| `start_col` | `number` | yes |  |

<a id="wire-spinner"></a>
#### `Spinner`

Declarative description of an indeterminate spinner.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `accent` | `Color \| null` | no | Optional accent colour for the glyph. |
| `frame_idx` | `number` | no | Animation frame index. |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | no | Label shown next to the spinner, e.g. |

<a id="wire-splitdirection"></a>
#### `SplitDirection`

Orientation of a `Split`.

Alias for `"Horizontal" | "Vertical"`.

<a id="wire-splittreenode"></a>
#### `SplitTreeNode`

One `"split_tree"` node: a `{"leaf": <node>}` pane or a nested `{"split": {...}}`.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `leaf` | `LayoutNode` |  |
| `split` | `SplitTreeSplitPayload` |  |

<a id="wire-splittreesplitpayload"></a>
#### `SplitTreeSplitPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `first` | `SplitTreeNode` | yes |  |
| `orientation` | `OrientationValue` | yes |  |
| `ratio` | `number` | yes |  |
| `second` | `SplitTreeNode` | yes |  |

<a id="wire-stagestatus"></a>
#### `StageStatus`

Status of a single pipeline stage.

Alias for `"Pending" | "Active" | "Done" | "Failed" | "Skipped" | "Stale"`.

<a id="wire-statusbar"></a>
#### `StatusBar`

Declarative description of a status bar.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `left_segments` | `StatusBarSegment[]` | yes | Left-aligned segments, **most-important first**: a narrow bar squeezes and then drops from the end. |
| `right_segments` | `StatusBarSegment[]` | yes | Right-aligned segments, **most-important last**: a narrow bar drops from the front. |

<a id="wire-statusbarevent"></a>
#### `StatusBarEvent`

Events a `StatusBar` emits back to the app.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `SegmentClicked` | `_StatusBarEventSegmentClicked` | A clickable segment was activated (mouse click, or future enter-on-focus). |
| `KeyPressed` | `_StatusBarEventKeyPressed` | A key was pressed while the bar had focus and the primitive didn't consume it. |

<a id="wire-statusbarsegment"></a>
#### `StatusBarSegment`

One styled segment in a `StatusBar`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `action_id` | `WidgetId \| null` | no | `None` = non-interactive. |
| `bg` | `Color` | yes |  |
| `bold` | `boolean` | no |  |
| `fg` | `Color` | yes |  |
| `text` | `string` | yes |  |

<a id="wire-style"></a>
#### `Style`

Per-span text style used by the editor's byte-range `EditorStyledSpan`. Mirrors `vimcode::render::Style`, with `font_scale` narrowed from `f64` → `f32` so derives of `PartialEq` / `Serialize` work without `Eq` blocking us.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bg` | `Color \| null` | no |  |
| `bold` | `boolean` | no |  |
| `fg` | `Color` | yes |  |
| `font_scale` | `number` | yes | Font scale factor (1.0 = normal). |
| `italic` | `boolean` | no |  |

<a id="wire-stylecolorvalue"></a>
#### `StyleColorValue`

A `"style"` colour token: a `Color` object, or a `"#rrggbb"` / `"#rrggbbaa"` hex string.

Alias for `Color | string`.

<a id="wire-stylevalue"></a>
#### `StyleValue`

A node's `"style"` override, scoped to its subtree: every `Theme` colour token and `Style` metric token is an optional key (with `bg`/`fg`/`border`/`accent` aliasing `Theme::background`/`foreground`/`border_fg`/`accent_fg`), plus the font keys and the `hover`/`pressed`/`focus` interaction-state branches.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `accent` | `StyleColorValue \| null` | no |  |
| `accent_bg` | `StyleColorValue \| null` | no |  |
| `activity_bar_width` | `number \| null` | no |  |
| `annotation_fg` | `StyleColorValue \| null` | no |  |
| `backdrop_opacity` | `number \| null` | no |  |
| `badge_blocked` | `StyleColorValue \| null` | no |  |
| `badge_passed` | `StyleColorValue \| null` | no |  |
| `badge_running` | `StyleColorValue \| null` | no |  |
| `badge_warning` | `StyleColorValue \| null` | no |  |
| `bg` | `StyleColorValue \| null` | no |  |
| `board_col_header_bg` | `StyleColorValue \| null` | no |  |
| `board_selected_card_bg` | `StyleColorValue \| null` | no |  |
| `border` | `StyleColorValue \| null` | no |  |
| `border_width` | `number \| null` | no |  |
| `bracket_match_bg` | `StyleColorValue \| null` | no |  |
| `button_radius` | `number \| null` | no |  |
| `card_hint_bg` | `StyleColorValue \| null` | no |  |
| `card_hint_fg` | `StyleColorValue \| null` | no |  |
| `colorcolumn_bg` | `StyleColorValue \| null` | no |  |
| `command_line_bg` | `StyleColorValue \| null` | no |  |
| `command_line_fg` | `StyleColorValue \| null` | no |  |
| `completion_bg` | `StyleColorValue \| null` | no |  |
| `completion_border` | `StyleColorValue \| null` | no |  |
| `completion_fg` | `StyleColorValue \| null` | no |  |
| `completion_selected_bg` | `StyleColorValue \| null` | no |  |
| `control_height` | `number \| null` | no |  |
| `corner_radius` | `number \| null` | no |  |
| `cursor` | `StyleColorValue \| null` | no |  |
| `cursor_normal_alpha` | `number \| null` | no |  |
| `cursorline_bg` | `StyleColorValue \| null` | no |  |
| `dap_stopped_bg` | `StyleColorValue \| null` | no |  |
| `diagnostic_error` | `StyleColorValue \| null` | no |  |
| `diagnostic_hint` | `StyleColorValue \| null` | no |  |
| `diagnostic_info` | `StyleColorValue \| null` | no |  |
| `diagnostic_warning` | `StyleColorValue \| null` | no |  |
| `diff_added_bg` | `StyleColorValue \| null` | no |  |
| `diff_padding_bg` | `StyleColorValue \| null` | no |  |
| `diff_removed_bg` | `StyleColorValue \| null` | no |  |
| `editor_active_background` | `StyleColorValue \| null` | no |  |
| `error_fg` | `StyleColorValue \| null` | no |  |
| `fg` | `StyleColorValue \| null` | no |  |
| `focus` | `StyleValue \| null` | no |  |
| `focus_ring_width` | `number \| null` | no |  |
| `font_family` | `string \| null` | no |  |
| `font_size` | `number \| null` | no |  |
| `font_weight` | `FontWeightValue \| null` | no |  |
| `ghost_text_fg` | `StyleColorValue \| null` | no |  |
| `git_added` | `StyleColorValue \| null` | no |  |
| `git_deleted` | `StyleColorValue \| null` | no |  |
| `git_modified` | `StyleColorValue \| null` | no |  |
| `header_bg` | `StyleColorValue \| null` | no |  |
| `header_fg` | `StyleColorValue \| null` | no |  |
| `hover` | `StyleValue \| null` | no |  |
| `hover_bg` | `StyleColorValue \| null` | no |  |
| `hover_border` | `StyleColorValue \| null` | no |  |
| `hover_fg` | `StyleColorValue \| null` | no |  |
| `inactive_fg` | `StyleColorValue \| null` | no |  |
| `inactive_selected_bg` | `StyleColorValue \| null` | no |  |
| `indent_guide_active_fg` | `StyleColorValue \| null` | no |  |
| `indent_guide_fg` | `StyleColorValue \| null` | no |  |
| `input_bg` | `StyleColorValue \| null` | no |  |
| `lightbulb` | `StyleColorValue \| null` | no |  |
| `line_number_active_fg` | `StyleColorValue \| null` | no |  |
| `line_number_fg` | `StyleColorValue \| null` | no |  |
| `link_fg` | `StyleColorValue \| null` | no |  |
| `list_cell_overflow` | `CellOverflowValue \| null` | no |  |
| `match_fg` | `StyleColorValue \| null` | no |  |
| `muted_fg` | `StyleColorValue \| null` | no |  |
| `padding` | `number \| null` | no |  |
| `pressed` | `StyleValue \| null` | no |  |
| `query_fg` | `StyleColorValue \| null` | no |  |
| `row_height` | `number \| null` | no |  |
| `row_padding_x` | `number \| null` | no |  |
| `scrollbar_thumb` | `StyleColorValue \| null` | no |  |
| `scrollbar_track` | `StyleColorValue \| null` | no |  |
| `scrollbar_width` | `number \| null` | no |  |
| `selected_bg` | `StyleColorValue \| null` | no |  |
| `selection` | `StyleColorValue \| null` | no |  |
| `selection_alpha` | `number \| null` | no |  |
| `selection_bg` | `StyleColorValue \| null` | no |  |
| `separator` | `StyleColorValue \| null` | no |  |
| `shadow_elevation` | `number \| null` | no |  |
| `spell_error` | `StyleColorValue \| null` | no |  |
| `status_bar_cell_gap` | `number \| null` | no |  |
| `status_bar_cell_overflow` | `CellOverflowValue \| null` | no |  |
| `status_bar_height` | `number \| null` | no |  |
| `surface_bg` | `StyleColorValue \| null` | no |  |
| `surface_fg` | `StyleColorValue \| null` | no |  |
| `tab_active_bg` | `StyleColorValue \| null` | no |  |
| `tab_active_fg` | `StyleColorValue \| null` | no |  |
| `tab_bar_bg` | `StyleColorValue \| null` | no |  |
| `tab_height` | `number \| null` | no |  |
| `tab_inactive_fg` | `StyleColorValue \| null` | no |  |
| `tab_padding_x` | `number \| null` | no |  |
| `tab_preview_active_fg` | `StyleColorValue \| null` | no |  |
| `tab_preview_inactive_fg` | `StyleColorValue \| null` | no |  |
| `title_fg` | `StyleColorValue \| null` | no |  |
| `toolbar_height` | `number \| null` | no |  |
| `tooltip_auto_dismiss_ms` | `number \| null` | no |  |
| `tooltip_delay_ms` | `number \| null` | no |  |
| `warning_fg` | `StyleColorValue \| null` | no |  |
| `yank_highlight_alpha` | `number \| null` | no |  |
| `yank_highlight_bg` | `StyleColorValue \| null` | no |  |

<a id="wire-styledspan"></a>
#### `StyledSpan`

A contiguous run of text sharing a single style.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bg` | `Color \| null` | no |  |
| `bold` | `boolean` | no |  |
| `fg` | `Color \| null` | no |  |
| `italic` | `boolean` | no |  |
| `text` | `string` | yes |  |
| `underline` | `boolean` | no |  |

<a id="wire-styledtext"></a>
#### `StyledText`

A sequence of styled spans rendered inline on one line.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `spans` | `StyledSpan[]` | yes |  |

<a id="wire-switch"></a>
#### `Switch`

Declarative description of a standalone on/off switch.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `disabled` | `boolean` | no | When `true`, `Switch::handle_key` and `Switch::click` both return `None` — same "disabled means inert, not hidden" contract every other primitive's `disabled` flag follows. |
| `focused` | `boolean` | no | Whether this switch currently holds keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `value` | `boolean` | no | Current on/off state. |

<a id="wire-systemtheme"></a>
#### `SystemTheme`

The OS-level theme preference `PlatformServices::system_theme` reports — light/dark, accent colour, and high-contrast.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `accent` | `Color \| null` | no | The OS accent colour, when the platform exposes one and the backend can read it. |
| `dark` | `boolean` | yes | `true` when the OS is set to a dark appearance (Windows "Dark" app mode, macOS Dark Appearance, GTK's `prefer-dark-theme`, or — on TUI — `COLORFGBG`'s background index reads as dark). |
| `high_contrast` | `boolean` | yes | `true` when the OS high-contrast accessibility setting is active (Windows High Contrast mode, GTK's `HighContrast*` theme names). |
| `reduce_motion` | `boolean` | no | `true` when the OS "reduce motion" accessibility preference is active (macOS `accessibilityDisplayShouldReduceMotion`, Windows `SPI_GETCLIENTAREAANIMATION` inverted, GTK's `gtk-enable-animations` inverted). |

<a id="wire-tabbar"></a>
#### `TabBar`

Declarative description of a tab bar.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `active_accent` | `Color \| null` | no | Optional colour used to underline the active tab's filename portion. |
| `chrome` | `TabChrome` | no | Active-tab framing and keyboard-focus request for this bar — the decoration `crate::primitive::Primitive::paint` / `crate::primitive::Primitive::paint_cells` draw around the active tab, and the tab index they paint the focus-visible treatment on. |
| `compact` | `boolean` | no | When `true`, the GTK rasteriser uses minimal padding (2px) and zero gap between tabs. |
| `glyphs` | `TabBarGlyphs` | no | Close / dirty glyph overrides for every tab in this bar. |
| `icons` | `(TabIcon \| null)[]` | no | Optional per-tab leading icons, index-aligned with `Self::tabs`. |
| `id` | `WidgetId` | yes |  |
| `right_segments` | `TabBarSegment[]` | no | Right-aligned trailing segments, drawn in order from left to right starting at `bar_width - sum(widths)`. |
| `scroll_offset` | `number` | no | Index of the first visible tab when tabs overflow. |
| `show_tab_close` | `boolean` | no | When `false`, per-tab close buttons (×/●) are suppressed — the measurer returns `close_width: 0.0` and rasterisers skip the glyph. |
| `tabs` | `TabItem[]` | yes |  |

<a id="wire-tabbarevent"></a>
#### `TabBarEvent`

Events a `TabBar` emits back to the app. Currently unused by vimcode (click path goes through the engine's `TabBarClickTarget` enum), but defined for plugin invariants §10 — plugin-declared tab bars will consume events directly.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `TabActivated` | `_TabBarEventTabActivated` | User clicked a tab body (not its close button) — index is into `tabs`, matching the visible order (scroll_offset still applies). |
| `TabClosed` | `_TabBarEventTabClosed` | User clicked a tab's close button. |
| `ButtonClicked` | `_TabBarEventButtonClicked` | User clicked a right-side segment with a non-`None` id. |
| `KeyPressed` | `_TabBarEventKeyPressed` | A key was pressed with the tab bar focused and the primitive didn't consume it. |
| `FocusMoved` | `_TabBarEventFocusMoved` | Left/Right moved the keyboard-focused tab to this new index. |

<a id="wire-tabbarglyphs"></a>
#### `TabBarGlyphs`

Optional app-supplied override for the close and dirty (unsaved) marks a tab bar paints.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `close` | `Icon \| null` | no |  |
| `dirty` | `Icon \| null` | no |  |

<a id="wire-tabbarsegment"></a>
#### `TabBarSegment`

One right-aligned segment in a `TabBar`. Either a clickable button (with an `id`) or a non-interactive label (`id = None`).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId \| null` | no | `None` = non-interactive. |
| `is_active` | `boolean` | no | Highlighted (toggled-on) state, e.g. |
| `text` | `string` | yes | Text / icon glyph to render (e.g. |
| `width_cells` | `number` | yes |  |

<a id="wire-tabchrome"></a>
#### `TabChrome`

Per-bar chrome request: which active-tab frame a backend should paint. Carried by `TabBar::chrome`, so it reaches both `crate::primitive::Primitive::layout` (which reserves the frame's extra width) and the painters.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `active_frame` | `TabFrame` | no | Which frame to paint around the active tab. |
| `focused_index` | `number \| null` | no | Index into `TabBar::tabs` of the keyboard-focused tab, if any. |
| `separator` | `TabSeparator` | no | What the cell-grid painter puts after a tab with no close glyph. |

<a id="wire-tabdropinstruction"></a>
#### `TabDropInstruction`

A position-based drop instruction: the result of resolving a tab drag-and-drop gesture against pane geometry, expressed purely as indices and a split direction — never as a mutation applied to an owned model.

Alias for `"NoOp" | _TabDropInstructionDict`.

<a id="wire-tabframe"></a>
#### `TabFrame`

Active-tab framing vocabulary: which decoration, if any, should enclose the active tab's full content — label *and* close glyph.

Alias for `"None" | "Brackets"`.

<a id="wire-tabgroupevent"></a>
#### `TabGroupEvent`

What a `TabGroup` reports, and what `TabGroup::apply` answers after applying it.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `TabActivated` | `_TabGroupEventTabActivated` | A tab in `pane_idx` was activated. |
| `TabClosed` | `_TabGroupEventTabClosed` | A tab in `pane_idx` was closed. |
| `PaneCollapsed` | `_TabGroupEventPaneCollapsed` | The last tab in `pane_idx` was closed. |
| `PaneAdded` | `_TabGroupEventPaneAdded` | A new pane was added (split). |
| `PaneFocused` | `_TabGroupEventPaneFocused` | A press inside a pane's body moved keyboard focus to that pane. |
| `DividerResized` | `_TabGroupEventDividerResized` | A divider was dragged. |
| `NewTabRequested` | `_TabGroupEventNewTabRequested` | The "+" new-tab button in a pane was clicked. |
| `TabDropped` | `_TabGroupEventTabDropped` | A dragged tab was dropped: the widget's intent, carrying the `resolve_tab_drop` instruction for it. |
| `TabReordered` | `_TabGroupEventTabReordered` | A tab was reordered within its pane via drag-and-drop. |
| `TabMovedToPane` | `_TabGroupEventTabMovedToPane` | A tab was dragged from one pane to another (merge). |
| `TabSplitToNewPane` | `_TabGroupEventTabSplitToNewPane` | A tab was dragged to a group edge, creating a new adjacent pane. |

<a id="wire-tabicon"></a>
#### `TabIcon`

`TabIcon`'s two accepted wire shapes, `Codicon` first for the same reason as `crate::Icon`'s.

Alias for `_TabIcon1 | _TabIcon2`.

<a id="wire-tabitem"></a>
#### `TabItem`

One tab in a `TabBar`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `is_active` | `boolean` | no |  |
| `is_closable` | `boolean` | no | When `true`, this tab renders an individual `×` / `●` close button (subject to `TabBar::show_tab_close` also being `true`). |
| `is_dirty` | `boolean` | no |  |
| `is_preview` | `boolean` | no |  |
| `label` | `string` | yes | Display label, e.g. |

<a id="wire-tabseparator"></a>
#### `TabSeparator`

What a cell-grid tab bar paints after a tab that has no close glyph (`show_tab_close: false`, or a non-closable tab) and no bracket frame.

Alias for `"Rule" | "None"`.

<a id="wire-terminal"></a>
#### `Terminal`

Declarative description of a terminal cell grid.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cells` | `TerminalCell[][]` | yes |  |
| `id` | `WidgetId` | yes |  |
| `scrollbar` | `TerminalScrollbar \| null` | no | When present, the terminal paints a themed vertical `crate::Scrollbar` in a gutter at its right edge. |

<a id="wire-terminalcell"></a>
#### `TerminalCell`

One styled cell in a `Terminal`. Carries the rendered grapheme cluster, RGB foreground/background, attributes, and overlay flags (cursor / selection / find match) that the backend interprets visually.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bg` | `Color` | yes |  |
| `bold` | `boolean` | no |  |
| `cursor_blinking` | `boolean` | no | `true` when the child requested a *blinking* DECSCUSR variant (odd `Ps` values `1`/`3`/`5`) rather than steady (`2`/`4`/`6`). |
| `cursor_shape` | `TerminalCursorShape` | no | Shape of the VT100 cursor at this cell, from the child program's most recent DECSCUSR request (`ESC [ Ps SP q`) — see `crate::terminal_engine::TerminalSession::cursor_shape`. |
| `dim` | `boolean` | no | SGR 2 (faint/dim) — `vt100::Cell::dim()`. |
| `fg` | `Color` | yes |  |
| `is_cursor` | `boolean` | no | Cell holds the VT100 cursor position. |
| `is_find_active` | `boolean` | no | Cell is part of the currently-selected find match (bright highlight). |
| `is_find_match` | `boolean` | no | Cell is part of a non-active find match (dim highlight). |
| `italic` | `boolean` | no |  |
| `selected` | `boolean` | no | Cell is part of the user's mouse selection. |
| `text` | `string` | yes | The cell's full grapheme cluster — a base character plus any combining marks (accents, etc.) vt100 appended to it, e.g. |
| `underline` | `boolean` | no |  |

<a id="wire-terminalcursorshape"></a>
#### `TerminalCursorShape`

Shape of the VT100 cursor, as steered by the child program's DECSCUSR (`ESC [ Ps SP q`) request — see `crate::terminal_engine::TerminalSession::cursor_shape`.

Alias for `"Block" | "Underline" | "Bar"`.

<a id="wire-terminalevent"></a>
#### `TerminalEvent`

Events a `Terminal` emits back to the app, as `crate::UiEvent::Terminal`. Its `crate::interactive::Interactive` impl produces the selection and scroll variants; the app applies them to its own session (e.g. `TerminalEvent::Scroll` through the engine's `handle_wheel`, which decides between forwarding the wheel to the child and scrolling local history).

Alias for `"SelectEnd" | _TerminalEventDict`.

<a id="wire-terminalscrollbar"></a>
#### `TerminalScrollbar`

Scrollbar state for a `Terminal`. Paint builds a `crate::Scrollbar::vertical` from these values on the gutter `TerminalLayout::scrollbar` reserved, and paints it with the scrollbar's own painter.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `inverted` | `boolean` | no | When `true`, `scroll_offset = 0` means "at the bottom" (live view) and increasing offset scrolls up into history. |
| `scroll_offset` | `number` | yes |  |
| `total_lines` | `number` | yes |  |
| `visible_lines` | `number` | yes |  |
| `width` | `number \| null` | no | Scrollbar width in surface-native units (pixels on a pixel backend, cells on TUI). |

<a id="wire-textarea"></a>
#### `TextArea`

A standalone multi-line input over a flat `value` (lines separated by `'/n'`).

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number \| null` | no | The caret as a byte offset into `value`; `None` puts it at the end. |
| `has_focus` | `boolean` | no | Whether to paint the focused wash. |
| `id` | `WidgetId` | yes |  |
| `placeholder` | `string` | no | Shown dimmed while the value is empty. |
| `value` | `string` | no | The text, lines separated by `'/n'`. |
| `visible_rows` | `number` | no | Rows tall — the viewport height, independent of how many lines the value holds. |

<a id="wire-textdisplay"></a>
#### `TextDisplay`

Declarative description of a `TextDisplay`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `auto_scroll` | `boolean` | no | When true, backends auto-scroll to keep the latest line visible. |
| `has_focus` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `lines` | `TextDisplayLine[]` | yes |  |
| `max_lines` | `number` | no | Maximum lines to retain in the ring buffer. |
| `scroll_offset` | `number` | no | Index of the first visible line. |
| `show_scrollbar` | `boolean` | no | When true, a vertical scrollbar is rendered at the trailing edge of the viewport. |
| `title` | `StyledText \| null` | no | Optional title row painted above the body. |

<a id="wire-textdisplayevent"></a>
#### `TextDisplayEvent`

Events a `TextDisplay` emits back to the app.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Scrolled` | `_TextDisplayEventScrolled` | User scrolled the view (mouse wheel, PageUp/Down, etc.). |
| `AutoScrollToggled` | `_TextDisplayEventAutoScrollToggled` | User toggled auto-scroll (typically via a keyboard shortcut or click on a "Follow" indicator). |
| `Copied` | `_TextDisplayEventCopied` | User initiated a copy of selected lines. |
| `KeyPressed` | `_TextDisplayEventKeyPressed` | A key was pressed with the display focused and the primitive did not consume it. |

<a id="wire-textdisplayleaf"></a>
#### `TextDisplayLeaf`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `auto_scroll` | `boolean` | no | When true, backends auto-scroll to keep the latest line visible. |
| `has_focus` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `lines` | `TextDisplayLine[]` | yes |  |
| `max_lines` | `number` | no | Maximum lines to retain in the ring buffer. |
| `scroll_offset` | `number` | no | Index of the first visible line. |
| `show_scrollbar` | `boolean` | no | When true, a vertical scrollbar is rendered at the trailing edge of the viewport. |
| `title` | `StyledText \| null` | no | Optional title row painted above the body. |
| `items_window` | `ItemsWindow \| null` | no |  |

<a id="wire-textdisplayline"></a>
#### `TextDisplayLine`

One line in a `TextDisplay`. Carries styled spans plus an optional decoration tag (Error/Warning/Muted/Header) for log-level styling and an optional left-aligned timestamp string the backend renders in a dim colour.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `decoration` | `Decoration` | no |  |
| `spans` | `StyledSpan[]` | yes |  |
| `timestamp` | `string \| null` | no | Optional timestamp prefix (e.g. |

<a id="wire-textfield"></a>
#### `TextField`

Standalone single-line framed text field. See the module doc.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `clearable` | `boolean` | no | Whether a clear (×) button shows when `Self::value` is non-empty. |
| `cursor` | `number \| null` | no | Byte offset into `Self::value`. |
| `has_focus` | `boolean` | no | Whether the field has keyboard focus — same convention as `crate::Button::focused`: carries no "how did focus arrive" information. |
| `id` | `WidgetId` | yes |  |
| `leading_icon` | `Icon \| null` | no | Icon painted inside the left bracket, before the text — e.g. |
| `placeholder` | `string` | no | Shown, dimmed, in place of an empty `Self::value`. |
| `selection_anchor` | `number \| null` | no | Byte offset into `Self::value`. |
| `value` | `string` | no | Current text. |

<a id="wire-textinput"></a>
#### `TextInput`

Multi-line text input. Text is stored as one entry per line — empty lines are empty strings. Cursor is `(line, col)` in *char columns* (not bytes); the rasterisers convert as needed.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor_col` | `number` | no | Cursor column within the cursor line, in *char columns*. |
| `cursor_line` | `number` | no | Cursor line index. |
| `has_focus` | `boolean` | no | Whether the input has keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `lines` | `string[]` | yes | One entry per logical line. |
| `placeholder` | `string \| null` | no | Optional placeholder shown when `lines` is empty or contains a single empty string. |
| `scroll_col` | `number` | no | Horizontal scroll, in char columns. |
| `scroll_offset` | `number` | no | First visible line index. |

<a id="wire-timeofday"></a>
#### `TimeOfDay`

Optional time-of-day field shown alongside the date grid. `hour` is `0..=23`, `minute` is `0..=59`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `hour` | `number` | yes | `0..=23`. |
| `minute` | `number` | yes | `0..=59`. |

<a id="wire-togglegroupitem"></a>
#### `ToggleGroupItem`

One toggle in a `FieldKind::ToggleGroup`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |
| `value` | `boolean` | yes |  |

<a id="wire-toolbar"></a>
#### `Toolbar`

Declarative description of a horizontal toolbar.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bg` | `Color \| null` | no | Optional background colour. |
| `buttons` | `ToolbarButton[]` | yes |  |
| `focused_index` | `number \| null` | no | Index of the keyboard-focused button within `Self::buttons`. |
| `id` | `WidgetId` | yes |  |
| `orientation` | `ToolbarOrientation` | no | Main-axis direction the buttons flow along. |
| `overflow` | `ToolbarOverflow` | no | How items that don't fit the bar are handled. |
| `overflow_priorities` | `ToolbarOverflowPriorities` | no | Per-item drop order for `ToolbarOverflow::Menu`, keyed by the owning `Action`/`Group`'s id. |
| `pin_trailing_overflow_to_origin` | `boolean` | no | Which edge an over-long trailing group (the buttons after a `ToolbarButton::Spacer`) packs from — see `ToolbarLayoutOptions::pin_trailing_overflow_to_origin`. |
| `valign` | `ToolbarVAlign` | no | Where button/label text sits within a slot taller than one text row. |

<a id="wire-toolbarbutton"></a>
#### `ToolbarButton`

One item in a `Toolbar`.

Alias for `("Separator" | "Spacer") | _ToolbarButtonDict`.

<a id="wire-toolbarevent"></a>
#### `ToolbarEvent`

Semantic event a `Toolbar` click or Enter/Space activation stands for.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `ButtonClicked` | `_ToolbarEventButtonClicked` | An `ToolbarButton::Action` button was activated. |
| `GroupSelected` | `_ToolbarEventGroupSelected` | A child of an **exclusive** `ToolbarButton::Group` (`group`) was clicked — the segmented-control case, where exactly one child is `is_active` at a time. |
| `FieldChanged` | `_ToolbarEventFieldChanged` | A `ToolbarButton::Field`'s value changed while editing (every keystroke) — the host's own equivalent of `TextField::value` after applying the edit, not a byte-diff. |
| `FieldCommitted` | `_ToolbarEventFieldCommitted` | A `ToolbarButton::Field` was committed — Enter while it has focus. |
| `FieldCleared` | `_ToolbarEventFieldCleared` | A `ToolbarButton::Field`'s clear (×) control was activated, or Escape was pressed while it had focus and a non-empty value — the field's value is now empty. |
| `MenuRequested` | `_ToolbarEventMenuRequested` | A `ToolbarButton::MenuButton`'s pull-down was asked for — a click anywhere on its pill (body or `▾` affordance), or Enter/Space while the keyboard cursor sits on it. |
| `OverflowRequested` | `_ToolbarEventOverflowRequested` | The trailing `»` affordance a `ToolbarOverflow::Menu` layout paints was activated. |

<a id="wire-toolbaritemwidth"></a>
#### `ToolbarItemWidth`

Main-axis width policy for a `ToolbarButton::Field`, passed to `Toolbar::layout_with_options` via `measure_button`/`tui_item_width`.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Fixed` | `number` | Always this wide: cells for TUI, device px/DIPs for GTK / macOS / Win — the same dual-unit convention `crate::primitives::form::ControlWidth::Fixed` already uses, so a single `f32` reused verbatim by `tui_item_width` (cells) and `measure_button` (px). |
| `Flex` | `_ToolbarItemWidthFlex` | At least `min`, growing up to `max` to absorb main-axis room that would otherwise sit blank — see `distribute_flex_width`. |

<a id="wire-toolbarorientation"></a>
#### `ToolbarOrientation`

Main-axis direction a `Toolbar` lays its buttons out along, passed to `Toolbar::layout_with_options`.

Alias for `"Horizontal" | "Vertical"`.

<a id="wire-toolbaroverflow"></a>
#### `ToolbarOverflow`

How a `Toolbar` handles buttons that don't fit `bar_width`/`bar_height`, passed via `ToolbarLayoutOptions::with_overflow` and consumed by `Toolbar::layout_with_overflow` (`layout`/`layout_with_options` keep ignoring this field — see that method's own doc for why).

Alias for `"Clip" | "Wrap" | "Menu"`.

<a id="wire-toolbaroverflowpriorities"></a>
#### `ToolbarOverflowPriorities`

Alias for `ToolbarOverflowPriorityEntry[]`.

<a id="wire-toolbaroverflowpriorityentry"></a>
#### `ToolbarOverflowPriorityEntry`

One `ToolbarOverflowPriorities` registration, as it appears on the wire. Private: `ToolbarOverflowPriorities` keeps its own `(WidgetId, i32)` pair storage, and this is only the serde shape it maps each pair through.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `priority` | `number` | yes |  |

<a id="wire-toolbarvalign"></a>
#### `ToolbarVAlign`

Vertical alignment of button/label text within a toolbar slot taller than one text row.

Alias for `"Top" | "Center" | "Bottom"`.

<a id="wire-treeevent"></a>
#### `TreeEvent`

Events a `TreeView` emits back to the app.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `RowClicked` | `_TreeEventRowClicked` | Single-click (or Enter on the keyboard) on a row. |
| `RowDoubleClicked` | `_TreeEventRowDoubleClicked` | Double-click on a row (typically "open" / "activate"). |
| `RowToggleExpand` | `_TreeEventRowToggleExpand` | The chevron was clicked, or Space/arrow-keys expanded/collapsed a branch. |
| `SelectionChanged` | `_TreeEventSelectionChanged` | Keyboard selection moved to a new row. |
| `KeyPressed` | `_TreeEventKeyPressed` | A key was pressed while the tree had focus and the primitive did not consume it. |
| `SelectionSet` | `_TreeEventSelectionSet` | The full multi-selection changed (`SelectionMode::Multi`): Ctrl/Cmd click toggled a row, Shift click/extend selected a range, or a keyboard extension grew the set. |
| `RowRenameChanged` | `_TreeEventRowRenameChanged` | A keystroke was applied to the row at `path`'s `TreeRow::edit` — a character typed/deleted, or the cursor/selection moved. |
| `RowRenameCommitted` | `_TreeEventRowRenameCommitted` | Enter was pressed while renaming the row at `path`: the app should apply `text` as the row's new name and clear `edit` back to `None`. |
| `RowRenameCancelled` | `_TreeEventRowRenameCancelled` | Escape was pressed while renaming the row at `path`: the app should discard the in-progress text and clear `edit` back to `None` without renaming anything. |
| `RowActivated` | `_TreeEventRowActivated` | Enter was pressed on the selected row (typically "open"). |
| `ScrollChanged` | `_TreeEventScrollChanged` | A wheel, scrollbar or keyboard move scrolled the viewport: the app should store `scroll_offset` as the tree's new `TreeView::scroll_offset`. |
| `ContextMenuRequested` | `_TreeEventContextMenuRequested` | Right-click on a row. |

<a id="wire-treerow"></a>
#### `TreeRow`

One visible row in a `TreeView`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `badge` | `Badge \| null` | no | Right-aligned status indicator (e.g. |
| `decoration` | `Decoration` | no |  |
| `edit` | `TreeRowEditState \| null` | no | When `Some`, backends render an inline text input in place of `text` and `badge`. |
| `icon` | `Icon \| null` | no |  |
| `indent` | `number` | yes | Visual indent level in `style.indent` units. |
| `is_expanded` | `boolean \| null` | no | `None` marks a leaf; `Some(true)` marks an expanded branch; `Some(false)` marks a collapsed branch. |
| `path` | `number[]` | yes |  |
| `text` | `StyledText` | yes |  |

<a id="wire-treeroweditstate"></a>
#### `TreeRowEditState`

Inline editing state for a tree row. When present on a `TreeRow`, backends render a text input in place of the normal row label.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number` | yes | Cursor position as a byte offset into `text`. |
| `placeholder` | `string \| null` | no | Shown in muted style when `text` is empty (e.g. |
| `selection_anchor` | `number \| null` | no | Selection anchor as a byte offset. |
| `text` | `string` | yes |  |

<a id="wire-treestyle"></a>
#### `TreeStyle`

Visual style configuration for a `TreeView`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bordered` | `boolean` | no | Reserve a 1-cell/pixel margin on every edge of the tree's viewport for a decorative frame, mirroring `crate::primitives::list::ListView::bordered`. |
| `chevron_collapsed` | `string` | yes | Chevron drawn for a collapsed branch. |
| `chevron_expanded` | `string` | yes | Chevron drawn for an expanded branch. |
| `indent` | `number` | yes | Cells / pixels per visual indent level. |
| `row_height` | `number \| null` | no | Host override for non-header row height, in the backend's native unit (pixels for GTK / macOS / Win-GUI). |
| `show_chevrons` | `boolean` | yes | Draw expand/collapse chevrons on branches. |
| `show_scrollbar` | `boolean` | no | Paint a vertical scrollbar when the rows overflow the viewport. |
| `vim_keys` | `boolean` | no | Treat an unmodified `j` / `k` as Down / Up in the tree's own key handling. |

<a id="wire-treetable"></a>
#### `TreeTable`

Declarative description of a `TreeTable` widget: a `DataTable` plus per-row tree metadata. Every `DataTable` field keeps its existing meaning unchanged — selection, scroll, sort, focus, h-scroll, column overrides, footer, `bordered` and `striped` all behave exactly as they do on a plain `DataTable`. `tree` only ever adds the column-0 tree cell on top.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `table` | `DataTable` | yes |  |
| `tree` | `TreeTableNode[]` | yes | Parallel to `table.rows`: `tree[i]` is `table.rows[i]`'s tree shape. |
| `vim_keys` | `boolean` | no | `j`/`k` move the selection like Down/Up (while no Ctrl/Alt/Cmd is held), instead of starting a type-to-jump search. |

<a id="wire-treetablecontrollerdescriptor"></a>
#### `TreeTableControllerDescriptor`

A `"tree_table_controller"` extra: the hierarchical-model counterpart of `"tree_table"` (`TreeTable`/`quadraui::TreeTableNode`'s flat, host-flattened shape). Unlike every other additive primitive on this trait, decoding this is only ever the SEED for a `crate::engine::InteractionEngine`-cached `TreeTableController` — `crate::engine::InteractionEngine::sync_tree_tables` reads `nodes`/ `selected_id` exactly once, the first frame this `id` is seen (`Self::seed_controller`), then leaves the controller's own live expand/selection/sort state alone on every later frame, the same "seed once, then the richer controller owns it" rule that method's own doc gives for a `Form.scroll_offset`. `Self::paint` (the bare, non-interactive fallback every `ExtraPaint` impl gets painted with before that sync ever runs) builds a throwaway controller the same way, so the very first frame — before any controller is cached at all — still paints the full seeded model rather than nothing.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `bordered` | `boolean` | no |  |
| `columns` | `Column[]` | no |  |
| `folders_first` | `boolean` | no |  |
| `has_focus` | `boolean` | no |  |
| `id` | `WidgetId` | yes |  |
| `nodes` | `TreeTableControllerNode[]` | no |  |
| `selected_id` | `string \| null` | no |  |
| `show_scrollbar` | `boolean` | no |  |

<a id="wire-treetablecontrollernode"></a>
#### `TreeTableControllerNode`

One node in a `"tree_table_controller"` extra's hierarchical model — the wire mirror of `TreeTableNodeData`, which has no `Deserialize` of its own (its fields are private; only its `TreeTableNodeData::leaf`/`TreeTableNodeData::branch`/ `TreeTableNodeData::lazy_branch` constructors are public), so a wire node decodes into this plain mirror first and `Self::into_node_data` then picks the matching constructor. `has_children: false` is always a `TreeTableNodeData::leaf` (`children` ignored); `has_children: true` with `children` present (including an empty list) is a `TreeTableNodeData::branch`; `has_children: true` with `children` absent is a `TreeTableNodeData::lazy_branch` — the same three-way split `quadraui::compose::tree_table_controller`'s own module doc describes for *Lazy loading*.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cells` | `StyledText[]` | no |  |
| `children` | `TreeTableControllerNode[] \| null` | no |  |
| `has_children` | `boolean` | no |  |
| `icon` | `Icon \| null` | no |  |
| `id` | `string` | yes |  |

<a id="wire-treetablecontrollerwireevent"></a>
#### `TreeTableControllerWireEvent`

Wire-shaped mirror of `TreeTableControllerEvent` — the same field shapes, but `Serialize`/`Deserialize` (`TreeTableControllerEvent` itself has neither; it's an in-crate-only `quadraui::compose` type), and `Consumed`/`Ignored` dropped: `InteractionEngine::handle` only ever calls `EventSink::dispatch_tree_table_controller` for the five variants below, folding `Consumed`/`Ignored` straight into its own `Reaction` instead (see that method's own `TreeTableController` routing blocks).

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `RowSelected` | `_TreeTableControllerWireEventRowSelected` | Row selected — click or keyboard navigation. |
| `RowActivated` | `_TreeTableControllerWireEventRowActivated` | Enter pressed, or a row/chevron double-clicked. |
| `ToggleExpand` | `_TreeTableControllerWireEventToggleExpand` | A branch node's expand state flipped; its children were already loaded (or this collapses). |
| `ChildrenRequested` | `_TreeTableControllerWireEventChildrenRequested` | A lazy branch was expanded for the first time with no loaded children — the host should fetch them and answer via `InteractionEngine::set_tree_table_children`. |
| `SortChanged` | `_TreeTableControllerWireEventSortChanged` | A header click (re-)sorted every loaded level's children by `col`'s cell text — see `TreeTableController::header_clicked`'s own doc for how a host overrides the comparator via `InteractionEngine::set_tree_table_sort_override`. |

<a id="wire-treetableevent"></a>
#### `TreeTableEvent`

Events a `TreeTable` emits back to the app. Row events carry the row's `TreePath` alongside its flat index.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `SortChanged` | `_TreeTableEventSortChanged` | A header click asked for a sort by `col`: ascending, unless the table is already sorted ascending by `col`. |
| `RowActivated` | `_TreeTableEventRowActivated` | Enter (or the assistive "activate" action) on the selected row. |
| `RowSelected` | `_TreeTableEventRowSelected` | User selected a row — arrow-key navigation or a plain single click. |
| `RowDoubleClicked` | `_TreeTableEventRowDoubleClicked` | User double-clicked a row. |
| `Scroll` | `_TreeTableEventScroll` | User scrolled the table. |
| `ScrollChanged` | `_TreeTableEventScrollChanged` | The widget scrolled — a wheel, a scrollbar gesture, or a move that brought the selection into view. |
| `ColumnResized` | `_TreeTableEventColumnResized` | User dragged a column divider to resize. |
| `SelectionSet` | `_TreeTableEventSelectionSet` | The full multi-selection changed. |
| `ToggleExpanded` | `_TreeTableEventToggleExpanded` | The chevron was clicked, or Left/Right/Space resolved to an expand/collapse. |
| `ChildrenRequested` | `_TreeTableEventChildrenRequested` | Expanding a `TreeTableNode::children_pending` branch: the app should load its children and supply them, clearing the flag. |
| `ContextMenuRequested` | `_TreeTableEventContextMenuRequested` | The user asked for row `idx`'s context menu (right-click); `position` is where, in the coordinates the table was laid out in. |

<a id="wire-treetablenode"></a>
#### `TreeTableNode`

Per-row tree metadata for one `TreeTable` row, parallel to `DataTable::rows` on `TreeTable::table`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `children_pending` | `boolean` | no | A branch whose children the app has not supplied yet: expanding it reports `TreeTableEvent::ChildrenRequested` as well as the expand itself. |
| `icon` | `Icon \| null` | no | Optional leading icon, painted between the chevron and the label. |
| `indent` | `number` | yes | Visual indent level, in the backend's indent unit. |
| `is_expanded` | `boolean \| null` | no | `None` marks a leaf; `Some(true)`/`Some(false)` an expanded/collapsed branch. |
| `path` | `number[]` | yes | This row's path. |
| `placeholder` | `boolean` | no | A row standing in for a pending branch's children while they load (typically a dim `…`): painted like any row, but never selected, activated or expanded. |

<a id="wire-treeview"></a>
#### `TreeView`

Declarative description of a `TreeView` widget.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `has_focus` | `boolean` | no | Whether this tree currently holds keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `rows` | `TreeRow[]` | yes | Pre-flattened, pre-expanded rows in visual order. |
| `scroll_offset` | `number` | no | How many rows have been scrolled past (app-owned in v1; primitive-owned scroll state with `ScrollState::id(widget_id)` is a later stage per `docs/UI_CRATE_DESIGN.md` §3.1). |
| `selected_path` | `number[] \| null` | no |  |
| `selection_mode` | `SelectionMode` | yes |  |
| `style` | `TreeStyle` | yes |  |

<a id="wire-treeviewleaf"></a>
#### `TreeViewLeaf`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `has_focus` | `boolean` | no | Whether this tree currently holds keyboard focus. |
| `id` | `WidgetId` | yes |  |
| `rows` | `TreeRow[]` | yes | Pre-flattened, pre-expanded rows in visual order. |
| `scroll_offset` | `number` | no | How many rows have been scrolled past (app-owned in v1; primitive-owned scroll state with `ScrollState::id(widget_id)` is a later stage per `docs/UI_CRATE_DESIGN.md` §3.1). |
| `selected_path` | `number[] \| null` | no |  |
| `selection_mode` | `SelectionMode` | yes |  |
| `style` | `TreeStyle` | yes |  |
| `items_window` | `ItemsWindow \| null` | no |  |
| `scroll_px` | `number` | no |  |

<a id="wire-uievent"></a>
#### `UiEvent`

Everything a user (or platform) can do that an app might care about.

Alias for `("WindowClose" | "DisplaysChanged" | "ContextMenuDismissed") | _UiEventDict`.

<a id="wire-updateinfowire"></a>
#### `UpdateInfoWire`

Wire-friendly mirror of `quadraui::UpdateInfo` — that type carries no `Serialize`/`Deserialize` of its own (no consumer needed it before this module), so `update_check_for_update` hands a binding this instead.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `notes` | `string` | yes |  |
| `signature` | `string` | yes |  |
| `url` | `string` | yes |  |
| `version` | `string` | yes |  |

<a id="wire-userpayload"></a>
#### `UserPayload`

Alias for `unknown`.

<a id="wire-validationstate"></a>
#### `ValidationState`

Validation state rendered as a colored indicator + message.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Error` | `string` |  |
| `Warning` | `string` |  |

<a id="wire-viewport"></a>
#### `Viewport`

Backend viewport dimensions in native units.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `height` | `number` | yes |  |
| `scale` | `number` | yes |  |
| `width` | `number` | yes |  |

<a id="wire-weekday"></a>
#### `Weekday`

Day of the week. `index()` maps `Sunday` to `0`, matching the civil-calendar weekday formula used throughout this module.

Alias for `"Sunday" | "Monday" | "Tuesday" | "Wednesday" | "Thursday" | "Friday" | "Saturday"`.

<a id="wire-widgetid"></a>
#### `WidgetId`

Alias for `string`.

<a id="wire-widgetkind"></a>
#### `WidgetKind`

Every leaf widget kind key a layout-tree leaf node may carry.

Alias for `"activity_bar" | "board" | "button" | "canvas" | "cell_grid" | "chart" | "checkbox" | "combo_box" | "command_center" | "command_line" | "context_menu" | "data_table" | "date_picker" | "diff_view" | "editor" | "find_replace" | "form" | "icon_grid" | "image" | "list" | "markdown" | "menu_bar" | "message_list" | "minimap" | "multi_section_view" | "number_input" | "palette" | "password" | "pipeline_view" | "progress" | "radio_group" | "scripted_widget" | "scroll_view" | "scrollbar" | "select" | "slider" | "spinner" | "status_bar" | "switch" | "tab_bar" | "terminal" | "text_display" | "text_field" | "text_input" | "textarea" | "toolbar" | "tree" | "tree_table" | "tree_table_controller"`.

<a id="wire-widgetstatesnapshot"></a>
#### `WidgetStateSnapshot`

A read-only merge of whatever `WidgetState` currently holds for one id, across every shape this module tracks — `resolve_tree_cursor`/ `resolve_list_cursor`/`resolve_data_table_cursor`'s cursor + scroll, `resolve_split_ratio`'s ratio, `resolve_tab_bar_active`/ `resolve_layout_tabs_active`'s active tab, and `resolve_tree_expansion`'s per-row expand set — for a binding's own introspection surface (`quadraui-py`'s `QuiApp.widget_state`) rather than for resolving a frame's descriptor. Each field is independently `None` when that shape isn't what `id` (or, for `expanded`, `id`'s derived expansion key — see `expanded_state_id`) is storing; a tree widget populates `selected_path`/`scroll_offset` from its own id and `expanded` from the derived one in the same snapshot, since both can be live for the same tree at once.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `active_tab` | `number \| null` | no | A `TabBar`/`LayoutTabs`' stored active tab index, if `id` is one being tracked by `resolve_tab_bar_active`/ `resolve_layout_tabs_active`. |
| `expanded` | `number[][] \| null` | no | A `Tree`'s stored set of currently-expanded row paths, if `id` is one being tracked by `resolve_tree_expansion` — sorted for a deterministic read, since the underlying store is a `HashMap`. |
| `ratio` | `number \| null` | no | A `Split`'s stored divider ratio, if `id` is one being tracked by `resolve_split_ratio`. |
| `scroll_offset` | `number \| null` | no | A `Tree`/`List`/`DataTable`'s stored scroll offset, alongside whichever of `selected_path`/`selected_idx` applies. |
| `selected_idx` | `number \| null` | no | A `List`/`DataTable`'s stored cursor row, if `id` is one being tracked by `resolve_list_cursor`/`resolve_data_table_cursor`. |
| `selected_path` | `number[] \| null` | no | A `Tree`'s stored cursor row, if `id` is a tree being tracked by `resolve_tree_cursor`. |

<a id="wire-workspaceevent"></a>
#### `WorkspaceEvent`

What the workspace just did. Returned from every mutating entry point so the host can react without diffing the workspace.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Opened` | `_WorkspaceEventOpened` | A document was added to the strip and made active. |
| `Activated` | `_WorkspaceEventActivated` | A different document became the active one. |
| `Closed` | `_WorkspaceEventClosed` | A document was removed from the strip. |
| `Reordered` | `_WorkspaceEventReordered` | A document moved within the strip. |
| `Promoted` | `_WorkspaceEventPromoted` | The preview document became permanent — see the module doc's *Preview tabs* section for the six triggers that cause this. |

<a id="wire-_acceleratorscopedict"></a>
#### `_AcceleratorScopeDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Widget` | `WidgetId` | Fires only when the given widget (or its descendants) have focus. |
| `Mode` | `string` | Fires only when the given mode is active. |

<a id="wire-_activitybareventitemclicked"></a>
#### `_ActivityBarEventItemClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_activitybareventkeypressed"></a>
#### `_ActivityBarEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_auxhitdict"></a>
#### `_AuxHitDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Action` | `string` | Hit on a toolbar action button. |

<a id="wire-_boardactiondict"></a>
#### `_BoardActionDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `SelectCard` | `WidgetId` | Selection cursor moved to this card (click or keyboard). |
| `OpenIssue` | `WidgetId` | Enter / double-click: open the card's issue detail. |
| `ContextMenu` | `[WidgetId, Point]` | Right-click: show context menu at pixel anchor. |
| `MoveSelection` | `MoveDir` | Keyboard navigation request — host updates `selected_card_id`, per-column `scroll_offset`, and `col_scroll_offset` accordingly. |
| `OpenReview` | `WidgetId` | Open the review for a card. |

<a id="wire-_bottompaneleventdict"></a>
#### `_BottomPanelEventDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `TabActivated` | `string` | User switched to a different tab. |
| `TabClosed` | `string` | User clicked the `×` close button on a closable tab. |
| `Resized` | `number` | User dragged the resize grip (forwarded from `ShellEvent::BottomPanelResized`). |

<a id="wire-_canvaseventregionclicked"></a>
#### `_CanvasEventRegionClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `button` | `MouseButton` | yes |  |
| `modifiers` | `Modifiers` | yes |  |
| `region` | `WidgetId` | yes |  |
| `x` | `number` | yes |  |
| `y` | `number` | yes |  |

<a id="wire-_canvaseventregiondoubleclicked"></a>
#### `_CanvasEventRegionDoubleClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `region` | `WidgetId` | yes |  |
| `x` | `number` | yes |  |
| `y` | `number` | yes |  |

<a id="wire-_canvaseventregionhovered"></a>
#### `_CanvasEventRegionHovered`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `region` | `string \| null` | no |  |

<a id="wire-_cellgrideventcelleditcancelled"></a>
#### `_CellGridEventCellEditCancelled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_cellgrideventcelleditchanged"></a>
#### `_CellGridEventCellEditChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `edit` | `CellEditState` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_cellgrideventcelleditcommitted"></a>
#### `_CellGridEventCellEditCommitted`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `row` | `number` | yes |  |
| `text` | `string` | yes |  |

<a id="wire-_cellgrideventcelleditrequested"></a>
#### `_CellGridEventCellEditRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_cellgrideventcelleditstarted"></a>
#### `_CellGridEventCellEditStarted`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `edit` | `CellEditState` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_cellgrideventcellsrequested"></a>
#### `_CellGridEventCellsRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col_start` | `number` | yes |  |
| `cols` | `number` | yes |  |
| `row_start` | `number` | yes |  |
| `rows` | `number` | yes |  |

<a id="wire-_cellgrideventcursormoved"></a>
#### `_CellGridEventCursorMoved`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_cellgrideventnavigated"></a>
#### `_CellGridEventNavigated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col_scroll` | `number` | yes |  |
| `cursor` | `[number, number]` | yes |  |
| `row_scroll` | `number` | yes |  |
| `selection_anchor` | `[number, number] \| null` | no |  |

<a id="wire-_charteventclicked"></a>
#### `_ChartEventClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_charteventdatapointclicked"></a>
#### `_ChartEventDataPointClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `data_idx` | `number` | yes |  |
| `id` | `WidgetId` | yes |  |
| `series_idx` | `number` | yes |  |

<a id="wire-_charteventdatapointhovered"></a>
#### `_ChartEventDataPointHovered`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `point` | `[number, number] \| null` | no |  |

<a id="wire-_charteventlegendclicked"></a>
#### `_ChartEventLegendClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `series_idx` | `number` | yes |  |

<a id="wire-_chateventdict"></a>
#### `_ChatEventDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Submit` | `_ChatEventSubmit` | The input was submitted. |
| `StreamChunk` | `_ChatEventStreamChunk` | Stream chunk (reserved for a backend-push path; an app that owns the session pipe updates `Chat::transcript` directly). |
| `KeyPressed` | `_ChatEventKeyPressed` | A key press the chat did not consume, for the app's own hotkeys. |
| `TurnClicked` | `_ChatEventTurnClicked` | A click landed on transcript row `row_in_turn` of turn `turn_idx` (`0` = the role header row; the trailing blank separator row counts as part of the turn). |
| `InputChanged` | `_ChatEventInputChanged` | The input buffer, caret or selection changed. |
| `TranscriptScrolled` | `_ChatEventTranscriptScrolled` | The transcript scrolled to `scroll_top`; `stuck_to_bottom` says whether it now follows the tail. |
| `FocusedTurnChanged` | `_ChatEventFocusedTurnChanged` | The keyboard focus cursor moved onto turn `turn`, or back to the input (`None`). |
| `TurnCollapsed` | `_ChatEventTurnCollapsed` | Turn `turn_idx` was collapsed or expanded from the keyboard. |

<a id="wire-_chateventfocusedturnchanged"></a>
#### `_ChatEventFocusedTurnChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `turn` | `number \| null` | no |  |

<a id="wire-_chateventinputchanged"></a>
#### `_ChatEventInputChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `anchor` | `[number, number] \| null` | no |  |
| `cursor` | `[number, number]` | yes |  |
| `lines` | `string[]` | yes |  |

<a id="wire-_chateventkeypressed"></a>
#### `_ChatEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_chateventstreamchunk"></a>
#### `_ChatEventStreamChunk`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |

<a id="wire-_chateventsubmit"></a>
#### `_ChatEventSubmit`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |

<a id="wire-_chateventtranscriptscrolled"></a>
#### `_ChatEventTranscriptScrolled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `scroll_top` | `number` | yes |  |
| `stuck_to_bottom` | `boolean` | yes |  |

<a id="wire-_chateventturnclicked"></a>
#### `_ChatEventTurnClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `row_in_turn` | `number` | yes |  |
| `turn_idx` | `number` | yes |  |

<a id="wire-_chateventturncollapsed"></a>
#### `_ChatEventTurnCollapsed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `collapsed` | `boolean` | yes |  |
| `turn_idx` | `number` | yes |  |

<a id="wire-_columnwidthcontent"></a>
#### `_ColumnWidthContent`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `max` | `number` | yes |  |
| `min` | `number` | yes |  |

<a id="wire-_controlwidthdict"></a>
#### `_ControlWidthDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Fixed` | `number` | An exact width: cells for TUI, device pixels for GTK / macOS / Win (the same dual-unit convention as `Form::label_width`/`FormField::label_width`). |

<a id="wire-_datatableeventcolumnfocused"></a>
#### `_DataTableEventColumnFocused`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |

<a id="wire-_datatableeventcolumnresized"></a>
#### `_DataTableEventColumnResized`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `width` | `number` | yes |  |

<a id="wire-_datatableeventcontextmenurequested"></a>
#### `_DataTableEventContextMenuRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |
| `position` | `Point` | yes |  |

<a id="wire-_datatableeventheaderclicked"></a>
#### `_DataTableEventHeaderClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |

<a id="wire-_datatableeventrowactivated"></a>
#### `_DataTableEventRowActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_datatableeventrowdoubleclicked"></a>
#### `_DataTableEventRowDoubleClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_datatableeventrowselected"></a>
#### `_DataTableEventRowSelected`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_datatableeventscroll"></a>
#### `_DataTableEventScroll`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `delta` | `number` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_datatableeventscrollchanged"></a>
#### `_DataTableEventScrollChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `scroll_offset` | `number` | yes |  |

<a id="wire-_datatableeventselectionset"></a>
#### `_DataTableEventSelectionSet`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `indices` | `number[]` | yes |  |

<a id="wire-_diffvieweventpanefocused"></a>
#### `_DiffViewEventPaneFocused`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `pane` | `DiffPane` | yes |  |

<a id="wire-_diffvieweventscrolled"></a>
#### `_DiffViewEventScrolled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `scroll_offset` | `number` | yes |  |

<a id="wire-_draggesturewireeventdragover"></a>
#### `_DragGestureWireEventDragOver`

The pointer moved onto a NEW leaf `drag_roles` marks `crate::wire::DragRole::drop_target` while the gesture above is still armed — dispatched once per target change (including back to the same leaf under a different `HitDetail`, e.g. a different `List` row), never on every `MouseMoved` pixel that lands on the unchanged target.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"drag_over"` | yes |  |
| `source` | `WidgetId` | yes |  |
| `source_detail` | `HitDetail` | yes |  |
| `target` | `WidgetId` | yes |  |
| `target_detail` | `HitDetail` | yes |  |

<a id="wire-_draggesturewireeventdragstarted"></a>
#### `_DragGestureWireEventDragStarted`

A `MouseDown` landed on a leaf `crate::engine::InteractionEngine::drag_roles` marks `crate::wire::DragRole::draggable` — the gesture's own arming moment. Dispatched before the leaf's own ordinary click/selection semantics (if any) also fire through `crate::engine::EventSink::dispatch` for the same `MouseDown` — the two are independent effects of one press, not alternatives.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"drag_started"` | yes |  |
| `source` | `WidgetId` | yes |  |
| `source_detail` | `HitDetail` | yes |  |

<a id="wire-_draggesturewireeventdropped"></a>
#### `_DragGestureWireEventDropped`

The gesture ended (`MouseUp`) while hovering a valid drop target — the host applies whatever mutation `source`/`source_detail` dropped onto `target`/`target_detail` stands for (e.g. reordering a `List`'s own backing collection from `source_detail`'s row index to `target_detail`'s). A `MouseUp` with no drop target under the pointer ends the gesture silently instead — no `Dropped` is dispatched at all.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"dropped"` | yes |  |
| `source` | `WidgetId` | yes |  |
| `source_detail` | `HitDetail` | yes |  |
| `target` | `WidgetId` | yes |  |
| `target_detail` | `HitDetail` | yes |  |

<a id="wire-_drawoparc"></a>
#### `_DrawOpArc`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `center` | `Point` | yes |  |
| `color` | `Color` | yes |  |
| `end_angle` | `number` | yes |  |
| `radius` | `number` | yes |  |
| `start_angle` | `number` | yes |  |
| `stroke_width` | `number` | yes |  |

<a id="wire-_drawopbezier"></a>
#### `_DrawOpBezier`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `control1` | `Point` | yes |  |
| `control2` | `Point` | yes |  |
| `from` | `Point` | yes |  |
| `stroke_width` | `number` | yes |  |
| `to` | `Point` | yes |  |

<a id="wire-_drawopdashedline"></a>
#### `_DrawOpDashedLine`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `dash_len` | `number` | yes |  |
| `from` | `Point` | yes |  |
| `gap_len` | `number` | yes |  |
| `stroke_width` | `number` | yes |  |
| `to` | `Point` | yes |  |

<a id="wire-_drawopdict"></a>
#### `_DrawOpDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Rect` | `_DrawOpRect` | Fill an axis-aligned rect with a solid colour. |
| `RoundedRect` | `_DrawOpRoundedRect` | Fill a rect with rounded corners. |
| `Line` | `_DrawOpLine` | Stroke a single line segment. |
| `Path` | `_DrawOpPath` | Stroke a polyline through `points`, in order. |
| `TextRun` | `_DrawOpTextRun` | Paint `text` at `rect`'s top-left corner in `color`, using the surface's current font. |
| `Image` | `_DrawOpImage` | Paint an image into `rect`, honouring `image.fit`. |
| `PushClip` | `_DrawOpPushClip` | Push an axis-aligned clip rect. |
| `Polygon` | `_DrawOpPolygon` | Fill a closed polygon through `points`, in order, using the even-odd rule (the same rule `polygon_fill_spans` implements). |
| `Ellipse` | `_DrawOpEllipse` | Fill an axis-aligned ellipse centred on `center`. |
| `Arc` | `_DrawOpArc` | Stroke a circular arc centred on `center`, from `start_angle` to `end_angle` (radians, `0.0` pointing along `+x`, increasing clockwise in screen coordinates — same convention as `std::f32::consts::TAU`'s usual `cos`/`sin` sweep). |
| `Bezier` | `_DrawOpBezier` | Stroke a cubic Bézier curve from `from` to `to`, shaped by control points `control1`/`control2` — the same four-point contract as SVG's `C` path command. |
| `DashedLine` | `_DrawOpDashedLine` | Stroke a single line segment as alternating `dash_len`-long drawn segments and `gap_len`-long gaps, starting with a drawn segment at `from`. |
| `StrokeRect` | `_DrawOpStrokeRect` | Stroke the outline of a rect without filling it — the outline twin of `DrawOp::Rect`, matching `crate::paint_surface::PaintSurface::surface_stroke_rect`'s own inset-stroke convention on every pixel backend. |
| `StyledTextRun` | `_DrawOpStyledTextRun` | `DrawOp::TextRun` with an explicit font role, italic flag, and (currently inert — see module doc) font size. |

<a id="wire-_drawopellipse"></a>
#### `_DrawOpEllipse`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `center` | `Point` | yes |  |
| `color` | `Color` | yes |  |
| `radius_x` | `number` | yes |  |
| `radius_y` | `number` | yes |  |

<a id="wire-_drawopimage"></a>
#### `_DrawOpImage`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `image` | `Image` | yes |  |
| `rect` | `Rect` | yes |  |

<a id="wire-_drawopline"></a>
#### `_DrawOpLine`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `from` | `Point` | yes |  |
| `stroke_width` | `number` | yes |  |
| `to` | `Point` | yes |  |

<a id="wire-_drawoppath"></a>
#### `_DrawOpPath`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `closed` | `boolean` | yes |  |
| `color` | `Color` | yes |  |
| `points` | `Point[]` | yes |  |
| `stroke_width` | `number` | yes |  |

<a id="wire-_drawoppolygon"></a>
#### `_DrawOpPolygon`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `points` | `Point[]` | yes |  |

<a id="wire-_drawoppushclip"></a>
#### `_DrawOpPushClip`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `rect` | `Rect` | yes |  |

<a id="wire-_drawoprect"></a>
#### `_DrawOpRect`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `rect` | `Rect` | yes |  |

<a id="wire-_drawoproundedrect"></a>
#### `_DrawOpRoundedRect`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `radius` | `number` | yes |  |
| `rect` | `Rect` | yes |  |

<a id="wire-_drawopstrokerect"></a>
#### `_DrawOpStrokeRect`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `rect` | `Rect` | yes |  |
| `stroke_width` | `number` | yes |  |

<a id="wire-_drawopstyledtextrun"></a>
#### `_DrawOpStyledTextRun`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `italic` | `boolean` | yes |  |
| `rect` | `Rect` | yes |  |
| `role` | `CanvasFontRole` | yes |  |
| `size` | `number \| null` | no | Per-op font size override. |
| `text` | `string` | yes |  |

<a id="wire-_drawoptextrun"></a>
#### `_DrawOpTextRun`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `rect` | `Rect` | yes |  |
| `text` | `string` | yes |  |

<a id="wire-_dualmodepaletteactiondict"></a>
#### `_DualModePaletteActionDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `ItemConfirmed` | `_DualModePaletteActionItemConfirmed` | In List mode: the user confirmed the highlighted item. |
| `TextConfirmed` | `_DualModePaletteActionTextConfirmed` | In Input mode: the user pressed `Enter` to confirm the free-text value. |
| `QueryChanged` | `_DualModePaletteActionQueryChanged` | The query text changed. |
| `ModeToggled` | `_DualModePaletteActionModeToggled` | The active mode toggled between `List` and `Input`. |
| `SelectionChanged` | `_DualModePaletteActionSelectionChanged` | The highlight moved to item `idx`. |

<a id="wire-_dualmodepaletteactionitemconfirmed"></a>
#### `_DualModePaletteActionItemConfirmed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_dualmodepaletteactionmodetoggled"></a>
#### `_DualModePaletteActionModeToggled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `new_mode` | `PaletteMode` | yes |  |

<a id="wire-_dualmodepaletteactionquerychanged"></a>
#### `_DualModePaletteActionQueryChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `string` | yes |  |

<a id="wire-_dualmodepaletteactionselectionchanged"></a>
#### `_DualModePaletteActionSelectionChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_dualmodepaletteactiontextconfirmed"></a>
#### `_DualModePaletteActionTextConfirmed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `string` | yes |  |

<a id="wire-_editoreventpointer"></a>
#### `_EditorEventPointer`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `hit` | `EditorHit` | yes |  |
| `kind` | `PointerKind` | yes |  |
| `mods` | `Modifiers` | yes |  |

<a id="wire-_editoreventscrollbar"></a>
#### `_EditorEventScrollbar`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `axis` | `ScrollAxis` | yes |  |
| `fraction` | `number` | yes |  |
| `kind` | `PointerKind` | yes |  |
| `mods` | `Modifiers` | yes |  |

<a id="wire-_editoreventwheel"></a>
#### `_EditorEventWheel`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `dx` | `number` | yes |  |
| `dy` | `number` | yes |  |
| `mods` | `Modifiers` | yes |  |

<a id="wire-_editorhitbufferpos"></a>
#### `_EditorHitBufferPos`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `line` | `number` | yes |  |

<a id="wire-_editorhitdict"></a>
#### `_EditorHitDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `BufferPos` | `_EditorHitBufferPos` | Click landed on a text position. |
| `Gutter` | `_EditorHitGutter` | Click landed in the gutter region. |

<a id="wire-_editorhitgutter"></a>
#### `_EditorHitGutter`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `gutter_col` | `number` | yes |  |
| `line_idx` | `number` | yes |  |

<a id="wire-_fieldkindbuttonrow"></a>
#### `_FieldKindButtonRow`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `buttons` | `ButtonRowItem[]` | yes |  |

<a id="wire-_fieldkindcolorpicker"></a>
#### `_FieldKindColorPicker`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `Color` | yes |  |

<a id="wire-_fieldkinddict"></a>
#### `_FieldKindDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Toggle` | `_FieldKindToggle` | Boolean toggle. |
| `TextInput` | `_FieldKindTextInput` | Single-line text input. |
| `ReadOnly` | `_FieldKindReadOnly` | Read-only display of a value computed elsewhere. |
| `Slider` | `_FieldKindSlider` | Numeric slider bounded by `min`..=`max` with increment `step`. |
| `ColorPicker` | `_FieldKindColorPicker` | Colour picker. |
| `Dropdown` | `_FieldKindDropdown` | Single-choice selection from a fixed list. |
| `TextArea` | `_FieldKindTextArea` | Multi-line text editing area. |
| `PasswordInput` | `_FieldKindPasswordInput` | Masked single-line text input. |
| `SegmentedControl` | `_FieldKindSegmentedControl` | Horizontal row of N options where exactly one is selected. |
| `ToggleGroup` | `_FieldKindToggleGroup` | Horizontal row of named boolean toggles. |
| `ButtonRow` | `_FieldKindButtonRow` | Horizontal row of action buttons. |
| `Toolbar` | `Toolbar` | A full `crate::Toolbar` strip embedded inside the form as a single field row. |
| `DatePicker` | `DatePicker` | A full `crate::primitives::date_picker::DatePicker` embedded as a single field row. |
| `ComboBox` | `ComboBox` | A searchable / type-to-filter `crate::primitives::combo_box::ComboBox` embedded as a single field row: a text input backed by a fixed list of options, where typing filters the list instead of requiring an exact match. |
| `NumberInput` | `NumberInput` | A bounded `crate::primitives::number_input::NumberInput` embedded as a single field row: a text area showing the current value (or in-progress draft) plus stacked increment/decrement stepper buttons. |
| `RadioGroup` | `RadioGroup` | A `crate::primitives::radio_group::RadioGroup` embedded as a field whose row spans one sub-row per option (unlike every other `FieldKind`, which stays a single row — see `crate::primitives::layout_metrics::form_field_measure`'s doc), painted through the embedded group's own `crate::primitives::radio_group::RadioGroup::layout` and backend rasteriser rather than a second paint path. |

<a id="wire-_fieldkinddropdown"></a>
#### `_FieldKindDropdown`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `options` | `StyledText[]` | yes |  |
| `selected_idx` | `number` | yes |  |

<a id="wire-_fieldkindpasswordinput"></a>
#### `_FieldKindPasswordInput`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number \| null` | no |  |
| `mask_char` | `string` | no |  |
| `placeholder` | `string` | no |  |
| `value` | `string` | yes |  |

<a id="wire-_fieldkindreadonly"></a>
#### `_FieldKindReadOnly`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `StyledText` | yes |  |

<a id="wire-_fieldkindsegmentedcontrol"></a>
#### `_FieldKindSegmentedControl`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `options` | `string[]` | yes |  |
| `selected_idx` | `number` | yes |  |

<a id="wire-_fieldkindslider"></a>
#### `_FieldKindSlider`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `max` | `number` | yes |  |
| `min` | `number` | yes |  |
| `step` | `number` | no |  |
| `value` | `number` | yes |  |

<a id="wire-_fieldkindtextarea"></a>
#### `_FieldKindTextArea`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number \| null` | no |  |
| `placeholder` | `string` | no |  |
| `value` | `string` | yes |  |
| `visible_rows` | `number` | no |  |

<a id="wire-_fieldkindtextinput"></a>
#### `_FieldKindTextInput`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number \| null` | no |  |
| `placeholder` | `string` | no |  |
| `selection_anchor` | `number \| null` | no |  |
| `value` | `string` | yes |  |

<a id="wire-_fieldkindtoggle"></a>
#### `_FieldKindToggle`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `boolean` | yes |  |

<a id="wire-_fieldkindtogglegroup"></a>
#### `_FieldKindToggleGroup`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `toggles` | `ToggleGroupItem[]` | yes |  |

<a id="wire-_findreplaceclicktargetdict"></a>
#### `_FindReplaceClickTargetDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `FindInput` | `number` | Click in the find input field at the given char offset. |
| `ReplaceInput` | `number` | Click in the replace input field at the given char offset. |

<a id="wire-_findreplaceeventdict"></a>
#### `_FindReplaceEventDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `FieldEdited` | `_FindReplaceEventFieldEdited` | A field's text, caret or selection changed, or focus moved to it (a click in it, Tab). |
| `Toggled` | `_FindReplaceEventToggled` | A switch was flipped; `on` is its new value. |

<a id="wire-_findreplaceeventfieldedited"></a>
#### `_FindReplaceEventFieldEdited`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number` | yes |  |
| `field` | `FindReplaceField` | yes |  |
| `sel_anchor` | `number \| null` | no |  |
| `text` | `string` | yes |  |

<a id="wire-_findreplaceeventtoggled"></a>
#### `_FindReplaceEventToggled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `on` | `boolean` | yes |  |
| `toggle` | `FindReplaceToggle` | yes |  |

<a id="wire-_formeventbuttonclicked"></a>
#### `_FormEventButtonClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_formeventcolorchanged"></a>
#### `_FormEventColorChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `Color` | yes |  |

<a id="wire-_formeventcomboboxcancelled"></a>
#### `_FormEventComboBoxCancelled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_formeventcomboboxcommitted"></a>
#### `_FormEventComboBoxCommitted`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `string` | yes |  |

<a id="wire-_formeventcomboboxhighlightchanged"></a>
#### `_FormEventComboBoxHighlightChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `index` | `number` | yes |  |

<a id="wire-_formeventcomboboxtextchanged"></a>
#### `_FormEventComboBoxTextChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `string` | yes |  |

<a id="wire-_formeventdatechanged"></a>
#### `_FormEventDateChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `CalendarDate` | yes |  |

<a id="wire-_formeventdatefocusmoved"></a>
#### `_FormEventDateFocusMoved`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `CalendarDate` | yes |  |

<a id="wire-_formeventdateopenchanged"></a>
#### `_FormEventDateOpenChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `open` | `boolean` | yes |  |

<a id="wire-_formeventdropdownchanged"></a>
#### `_FormEventDropdownChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `selected_idx` | `number` | yes |  |

<a id="wire-_formeventfocuschanged"></a>
#### `_FormEventFocusChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_formeventkeypressed"></a>
#### `_FormEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_formeventnumbercancelled"></a>
#### `_FormEventNumberCancelled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_formeventnumberchanged"></a>
#### `_FormEventNumberChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `number` | yes |  |

<a id="wire-_formeventnumbercommitted"></a>
#### `_FormEventNumberCommitted`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `number` | yes |  |

<a id="wire-_formeventnumberdraftchanged"></a>
#### `_FormEventNumberDraftChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `draft` | `string` | yes |  |
| `id` | `WidgetId` | yes |  |

<a id="wire-_formeventradiochanged"></a>
#### `_FormEventRadioChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `selected` | `number` | yes |  |

<a id="wire-_formeventsegmentedcontrolchanged"></a>
#### `_FormEventSegmentedControlChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `selected_idx` | `number` | yes |  |

<a id="wire-_formeventsliderchanged"></a>
#### `_FormEventSliderChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `number` | yes |  |

<a id="wire-_formeventtextinputchanged"></a>
#### `_FormEventTextInputChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `string` | yes |  |

<a id="wire-_formeventtextinputcommitted"></a>
#### `_FormEventTextInputCommitted`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `string` | yes |  |

<a id="wire-_formeventtogglechanged"></a>
#### `_FormEventToggleChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `boolean` | yes |  |

<a id="wire-_formeventtoolbarbuttonclicked"></a>
#### `_FormEventToolbarButtonClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `button_id` | `WidgetId` | yes |  |
| `field_id` | `WidgetId` | yes |  |

<a id="wire-_headerhitdict"></a>
#### `_HeaderHitDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Action` | `string` | Right-aligned action button. |

<a id="wire-_hitdetailcanvasregion"></a>
#### `_HitDetailCanvasRegion`

Landed on a declared region of a script-defined widget — `region` is that region's own `WidgetId`, resolved through `crate::scripted::ScriptedExtra::scripted_hit_test` ahead of `Self::ExtraBody`'s coarser "landed somewhere on the widget" answer. A point in no region still falls back to `ExtraBody`. A `"canvas"` leaf resolves its regions through `Canvas`'s own `Interactive` instead.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"canvas_region"` | yes |  |
| `region` | `WidgetId` | yes |  |

<a id="wire-_hitdetaildatatablefooter"></a>
#### `_HitDetailDataTableFooter`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"data_table_footer"` | yes |  |

<a id="wire-_hitdetaildatatableheader"></a>
#### `_HitDetailDataTableHeader`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `kind` | `"data_table_header"` | yes |  |

<a id="wire-_hitdetaildatatableheaderdivider"></a>
#### `_HitDetailDataTableHeaderDivider`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `kind` | `"data_table_header_divider"` | yes |  |

<a id="wire-_hitdetaildatatablerow"></a>
#### `_HitDetailDataTableRow`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"data_table_row"` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_hitdetailextrabody"></a>
#### `_HitDetailExtraBody`

Landed on an additive primitive's interactive region (currently `quadraui::Form`). Carries no sub-region detail yet — just enough for the binding's own event handler to set `widget` and attach this as `hit`, so it sees the click reached the primitive. Per-kind sub-region detail (e.g. which combo-box row) is a later, separate extension of this variant.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"extra_body"` | yes |  |

<a id="wire-_hitdetaillistitem"></a>
#### `_HitDetailListItem`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |
| `kind` | `"list_item"` | yes |  |

<a id="wire-_hitdetaillisttitle"></a>
#### `_HitDetailListTitle`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"list_title"` | yes |  |

<a id="wire-_hitdetailscrollbody"></a>
#### `_HitDetailScrollBody`

Landed inside a `crate::layout::LayoutNode::Scroll`'s viewport but on no more specific descendant hit — e.g. empty space between stacked content items. Carries no sub-region detail, mirroring `Self::ExtraBody`; the binding's own wheel routing uses this to resolve which `Scroll` a wheel event targets.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"scroll_body"` | yes |  |

<a id="wire-_hitdetailscrollscrollbarthumb"></a>
#### `_HitDetailScrollScrollbarThumb`

Landed on a `Scroll`'s own scrollbar thumb — mirrors `Self::TreeScrollbarThumb`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"scroll_scrollbar_thumb"` | yes |  |

<a id="wire-_hitdetailscrollscrollbartrackafter"></a>
#### `_HitDetailScrollScrollbarTrackAfter`

Landed after a `Scroll`'s own scrollbar thumb (page-down/right zone) — mirrors `Self::TreeScrollbarTrackAfter`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"scroll_scrollbar_track_after"` | yes |  |

<a id="wire-_hitdetailscrollscrollbartrackbefore"></a>
#### `_HitDetailScrollScrollbarTrackBefore`

Landed before a `Scroll`'s own scrollbar thumb (page-up/left zone) — mirrors `Self::TreeScrollbarTrackBefore`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"scroll_scrollbar_track_before"` | yes |  |

<a id="wire-_hitdetailsplitdivider"></a>
#### `_HitDetailSplitDivider`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"split_divider"` | yes |  |

<a id="wire-_hitdetailsplitfirstpane"></a>
#### `_HitDetailSplitFirstPane`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"split_first_pane"` | yes |  |

<a id="wire-_hitdetailsplitsecondpane"></a>
#### `_HitDetailSplitSecondPane`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"split_second_pane"` | yes |  |

<a id="wire-_hitdetailsplittreedivider"></a>
#### `_HitDetailSplitTreeDivider`

Landed on a `quadraui::SplitTree` divider — `split_index` is the pre-order index of the `SplitTree::Split` node being resized (see `quadraui::SplitTreeDivider::split_index`). Arms a drag in the binding's own event handler, mirroring `SplitDivider`'s `crate::drag::ActiveDrag` handling.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"split_tree_divider"` | yes |  |
| `split_index` | `number` | yes |  |

<a id="wire-_hitdetailtabbarrightsegment"></a>
#### `_HitDetailTabBarRightSegment`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tab_bar_right_segment"` | yes |  |

<a id="wire-_hitdetailtabbarscrollleft"></a>
#### `_HitDetailTabBarScrollLeft`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tab_bar_scroll_left"` | yes |  |

<a id="wire-_hitdetailtabbarscrollright"></a>
#### `_HitDetailTabBarScrollRight`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tab_bar_scroll_right"` | yes |  |

<a id="wire-_hitdetailtabbartab"></a>
#### `_HitDetailTabBarTab`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tab_bar_tab"` | yes |  |
| `tab` | `number` | yes |  |

<a id="wire-_hitdetailtabbartabclose"></a>
#### `_HitDetailTabBarTabClose`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tab_bar_tab_close"` | yes |  |
| `tab` | `number` | yes |  |

<a id="wire-_hitdetailtextdisplayline"></a>
#### `_HitDetailTextDisplayLine`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"text_display_line"` | yes |  |
| `line` | `number` | yes |  |

<a id="wire-_hitdetailtextdisplayscrollbarthumb"></a>
#### `_HitDetailTextDisplayScrollbarThumb`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"text_display_scrollbar_thumb"` | yes |  |

<a id="wire-_hitdetailtextdisplayscrollbartrackafter"></a>
#### `_HitDetailTextDisplayScrollbarTrackAfter`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"text_display_scrollbar_track_after"` | yes |  |

<a id="wire-_hitdetailtextdisplayscrollbartrackbefore"></a>
#### `_HitDetailTextDisplayScrollbarTrackBefore`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"text_display_scrollbar_track_before"` | yes |  |

<a id="wire-_hitdetailtreechevron"></a>
#### `_HitDetailTreeChevron`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_chevron"` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_hitdetailtreerow"></a>
#### `_HitDetailTreeRow`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_row"` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_hitdetailtreescrollbarthumb"></a>
#### `_HitDetailTreeScrollbarThumb`

Landed on a tree's vertical scrollbar thumb. Arms a drag in the binding's own event handler (mirroring `SplitDivider`'s `ActiveDrag`); never reaches `semantic_event_for_hit`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_scrollbar_thumb"` | yes |  |

<a id="wire-_hitdetailtreescrollbartrackafter"></a>
#### `_HitDetailTreeScrollbarTrackAfter`

Landed after the thumb on a tree's vertical scrollbar track (page-down zone). Handled directly in the binding's own event handler, same as `TreeScrollbarTrackBefore`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_scrollbar_track_after"` | yes |  |

<a id="wire-_hitdetailtreescrollbartrackbefore"></a>
#### `_HitDetailTreeScrollbarTrackBefore`

Landed before the thumb on a tree's vertical scrollbar track (page-up zone). Handled directly in the binding's own event handler, which replaces the raw click with `UiEvent::ScrollOffsetChanged` — never reaches `semantic_event_for_hit`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_scrollbar_track_before"` | yes |  |

<a id="wire-_hitdetailwidgetbody"></a>
#### `_HitDetailWidgetBody`

Landed on a `crate::layout::LeafDescriptor::Widget`'s whole painted area — the `Widget` counterpart of `Self::ExtraBody`. Carries no sub-region detail, same reason: a `DynWidget` has no `ExtraPaint::hit_test`-style sub-region test to resolve one finer than "landed somewhere on the widget" against. Routed through `DynWidget::handle` (not `semantic_event_for_hit`, which has no arm for this detail and falls through to `None`) by `crate::engine::InteractionEngine::handle`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"widget_body"` | yes |  |

<a id="wire-_icon1"></a>
#### `_Icon1`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `codicon` | `string` | yes |  |
| `color` | `Color \| null` | no |  |
| `fallback` | `string` | yes |  |

<a id="wire-_icon2"></a>
#### `_Icon2`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color \| null` | no |  |
| `fallback` | `string` | yes |  |
| `glyph` | `string` | yes |  |

<a id="wire-_keybindingdict"></a>
#### `_KeyBindingDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Literal` | `string` | App-specific binding. |

<a id="wire-_listvieweventitemactivated"></a>
#### `_ListViewEventItemActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_listvieweventitemdoubleclicked"></a>
#### `_ListViewEventItemDoubleClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_listvieweventkeypressed"></a>
#### `_ListViewEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_listvieweventselectionchanged"></a>
#### `_ListViewEventSelectionChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_listvieweventselectionset"></a>
#### `_ListViewEventSelectionSet`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `indices` | `number[]` | yes |  |

<a id="wire-_messagelisteventscrolled"></a>
#### `_MessageListEventScrolled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `at_bottom` | `boolean` | yes |  |
| `scroll_top` | `number` | yes |  |

<a id="wire-_minimapeventseek"></a>
#### `_MinimapEventSeek`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `fraction` | `number` | yes |  |

<a id="wire-_minimapsizingdict"></a>
#### `_MinimapSizingDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `FixedPitch` | `number` | Tile rows top-down at exactly this pitch (in `bounds`'s own coordinate units), regardless of `row_count`. |
| `RepeatToFill` | `number` | Tile rows top-down at a whole-number multiple of `base_pitch` (in `bounds`'s own coordinate units), chosen so the rows fill as much of `bounds.height` as a whole multiple allows. |
| `VsCodeParity` | `_MinimapSizingVsCodeParity` | VS Code-parity minimap **width** policy — orthogonal to `Fill` / `FixedPitch`, which size row *pitch* (vertical). |

<a id="wire-_minimapsizingvscodeparity"></a>
#### `_MinimapSizingVsCodeParity`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `fraction` | `number` | yes |  |
| `max` | `number` | yes |  |
| `min` | `number` | yes |  |
| `target_cols` | `number` | yes |  |

<a id="wire-_mousebuttondict"></a>
#### `_MouseButtonDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Other` | `number` | Backend-specific button index. |

<a id="wire-_multisectionvieweventauxactivated"></a>
#### `_MultiSectionViewEventAuxActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `hit` | `AuxHit` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_multisectionvieweventauxinputchanged"></a>
#### `_MultiSectionViewEventAuxInputChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `caret` | `number` | yes |  |
| `section` | `number` | yes |  |
| `text` | `string` | yes |  |

<a id="wire-_multisectionvieweventauxinputsubmitted"></a>
#### `_MultiSectionViewEventAuxInputSubmitted`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `section` | `number` | yes |  |
| `text` | `string` | yes |  |

<a id="wire-_multisectionvieweventcollapsechanged"></a>
#### `_MultiSectionViewEventCollapseChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `collapsed` | `boolean` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_multisectionvieweventheaderactivated"></a>
#### `_MultiSectionViewEventHeaderActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `hit` | `HeaderHit` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_multisectionvieweventscrollchanged"></a>
#### `_MultiSectionViewEventScrollChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `offset` | `number` | yes |  |
| `section` | `number \| null` | no |  |

<a id="wire-_multisectionvieweventsectionresized"></a>
#### `_MultiSectionViewEventSectionResized`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `above` | `number` | yes |  |
| `above_size` | `number` | yes |  |
| `below` | `number` | yes |  |
| `below_size` | `number` | yes |  |

<a id="wire-_namedkeydict"></a>
#### `_NamedKeyDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `F` | `number` | Function key 1-24. |

<a id="wire-_paletteeventdict"></a>
#### `_PaletteEventDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `QueryChanged` | `_PaletteEventQueryChanged` | The query text changed (user typed, pasted, or deleted). |
| `SelectionChanged` | `_PaletteEventSelectionChanged` | Keyboard / mouse moved selection to a different row. |
| `ItemConfirmed` | `_PaletteEventItemConfirmed` | User confirmed the highlighted row (Enter or double-click). |
| `ExpandToggled` | `_PaletteEventExpandToggled` | User toggled a tree item's expand/collapse state. |
| `KeyPressed` | `_PaletteEventKeyPressed` | A key was pressed while the palette had focus and the primitive did not consume it. |

<a id="wire-_paletteeventexpandtoggled"></a>
#### `_PaletteEventExpandToggled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `expanded` | `boolean` | yes |  |
| `idx` | `number` | yes |  |

<a id="wire-_paletteeventitemconfirmed"></a>
#### `_PaletteEventItemConfirmed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_paletteeventkeypressed"></a>
#### `_PaletteEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_paletteeventquerychanged"></a>
#### `_PaletteEventQueryChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `string` | yes |  |

<a id="wire-_paletteeventselectionchanged"></a>
#### `_PaletteEventSelectionChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_pathpickeractionconfirmed"></a>
#### `_PathPickerActionConfirmed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `string` | yes | The resolved, absolute path. |

<a id="wire-_pathpickeractiondict"></a>
#### `_PathPickerActionDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Confirmed` | `_PathPickerActionConfirmed` | The user confirmed a path — an existing entry for the folder picker and `FilePickerMode::Open`, or `root.join(query)` for `FilePickerMode::Save`. |
| `QueryChanged` | `_PathPickerActionQueryChanged` | The query text changed: the host sets `PathPickerState::query` and refilters. |
| `SelectionChanged` | `_PathPickerActionSelectionChanged` | The highlight moved to row `idx` of `PathPickerState::filtered`. |
| `Navigated` | `_PathPickerActionNavigated` | The picker asked to browse `root` instead — the parent directory, or a directory row the user confirmed. |

<a id="wire-_pathpickeractionnavigated"></a>
#### `_PathPickerActionNavigated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `root` | `string` | yes | The new browsing root. |

<a id="wire-_pathpickeractionquerychanged"></a>
#### `_PathPickerActionQueryChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `query` | `string` | yes | The new query. |

<a id="wire-_pathpickeractionselectionchanged"></a>
#### `_PathPickerActionSelectionChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes | The newly highlighted row. |

<a id="wire-_pipelineeventkeypressed"></a>
#### `_PipelineEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_pipelineeventstageaction"></a>
#### `_PipelineEventStageAction`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |

<a id="wire-_pipelineeventstagefocused"></a>
#### `_PipelineEventStageFocused`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |

<a id="wire-_pipelineeventstageselected"></a>
#### `_PipelineEventStageSelected`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |

<a id="wire-_pointerkinddict"></a>
#### `_PointerKindDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Down` | `MouseButton` | A button went down. |
| `Up` | `MouseButton` | A button was released. |
| `DoubleClick` | `MouseButton` | A double-click resolved on this button. |

<a id="wire-_scrollvieweventscrolled"></a>
#### `_ScrollViewEventScrolled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `offset` | `number` | yes |  |

<a id="wire-_sectionsizedict"></a>
#### `_SectionSizeDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Fixed` | `number` | Exact size in main-axis units (cells / pixels). |
| `Percent` | `number` | Share of the *original* container, `0.0..=1.0`. |
| `Weight` | `number` | Proportional weight (CSS `flex` semantics). |

<a id="wire-_shelleventactivitycursormoved"></a>
#### `_ShellEventActivityCursorMoved`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |

<a id="wire-_shelleventbottomitemclicked"></a>
#### `_ShellEventBottomItemClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_shelleventbottompanelresized"></a>
#### `_ShellEventBottomPanelResized`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `new_height` | `number` | yes |  |

<a id="wire-_shelleventdict"></a>
#### `_ShellEventDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `PanelChanged` | `_ShellEventPanelChanged` |  |
| `SidebarResized` | `_ShellEventSidebarResized` |  |
| `BottomPanelResized` | `_ShellEventBottomPanelResized` |  |
| `BottomItemClicked` | `_ShellEventBottomItemClicked` |  |
| `ActivityCursorMoved` | `_ShellEventActivityCursorMoved` | The activity bar's keyboard cursor moved to `index` in the logical item sequence (top panels, then bottom items). |

<a id="wire-_shelleventpanelchanged"></a>
#### `_ShellEventPanelChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `panel_id` | `WidgetId` | yes |  |

<a id="wire-_shelleventsidebarresized"></a>
#### `_ShellEventSidebarResized`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `new_width` | `number` | yes |  |

<a id="wire-_sidebaractionactivesectionchanged"></a>
#### `_SidebarActionActiveSectionChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `section` | `number \| null` | no |  |

<a id="wire-_sidebaractioncollapsechanged"></a>
#### `_SidebarActionCollapseChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `collapsed` | `boolean` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_sidebaractioncontextmenurequested"></a>
#### `_SidebarActionContextMenuRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |
| `position` | `Point` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_sidebaractioneditcancelled"></a>
#### `_SidebarActionEditCancelled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_sidebaractioneditchanged"></a>
#### `_SidebarActionEditChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number` | yes |  |
| `path` | `number[]` | yes |  |
| `section` | `number` | yes |  |
| `selection_anchor` | `number \| null` | no |  |
| `text` | `string` | yes |  |

<a id="wire-_sidebaractioneditconfirmed"></a>
#### `_SidebarActionEditConfirmed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `new_text` | `string` | yes |  |
| `path` | `number[]` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_sidebaractionform"></a>
#### `_SidebarActionForm`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `event` | `FormEvent` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_sidebaractionheaderactivated"></a>
#### `_SidebarActionHeaderActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `section` | `number` | yes |  |

<a id="wire-_sidebaractionpanelscrolled"></a>
#### `_SidebarActionPanelScrolled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `offset` | `number` | yes |  |

<a id="wire-_sidebaractionrowactivated"></a>
#### `_SidebarActionRowActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_sidebaractionrowselected"></a>
#### `_SidebarActionRowSelected`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_sidebaractionrowtoggleexpand"></a>
#### `_SidebarActionRowToggleExpand`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_sidebaractionscrollchanged"></a>
#### `_SidebarActionScrollChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `offset` | `number` | yes |  |
| `section` | `number` | yes |  |

<a id="wire-_statusbareventkeypressed"></a>
#### `_StatusBarEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_statusbareventsegmentclicked"></a>
#### `_StatusBarEventSegmentClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_tabbareventbuttonclicked"></a>
#### `_TabBarEventButtonClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_tabbareventfocusmoved"></a>
#### `_TabBarEventFocusMoved`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |

<a id="wire-_tabbareventkeypressed"></a>
#### `_TabBarEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_tabbareventtabactivated"></a>
#### `_TabBarEventTabActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |

<a id="wire-_tabbareventtabclosed"></a>
#### `_TabBarEventTabClosed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |

<a id="wire-_tabdropinstructiondict"></a>
#### `_TabDropInstructionDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Reorder` | `_TabDropInstructionReorder` | Reorder the tab within its own pane. |
| `MoveToPane` | `_TabDropInstructionMoveToPane` | Move the tab into a different pane (merge). |
| `SplitToNewPane` | `_TabDropInstructionSplitToNewPane` | Split the target pane, creating a new pane adjacent to it that holds the dragged tab. |

<a id="wire-_tabdropinstructionmovetopane"></a>
#### `_TabDropInstructionMoveToPane`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `from_idx` | `number` | yes | Position the tab held in the source pane before the move. |
| `from_pane_idx` | `number` | yes |  |
| `insert_idx` | `number` | yes | Position in the target pane's tab list where the tab should land. |
| `source_now_empty` | `boolean` | yes | `true` when removing the tab leaves the source pane with zero tabs — the host should collapse it (or not; `resolve_tab_drop` never decides that, since collapsing means renumbering the host's own pane indices, which only the host can do correctly). |
| `to_pane_idx` | `number` | yes |  |

<a id="wire-_tabdropinstructionreorder"></a>
#### `_TabDropInstructionReorder`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `from_idx` | `number` | yes | Position before the move. |
| `pane_idx` | `number` | yes |  |
| `to_idx` | `number` | yes | Position after the move. |

<a id="wire-_tabdropinstructionsplittonewpane"></a>
#### `_TabDropInstructionSplitToNewPane`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `edge` | `DropEdge` | yes | Which edge the tab was dropped onto. |
| `from_idx` | `number` | yes | Position the tab held in the source pane before the move. |
| `from_pane_idx` | `number` | yes |  |
| `source_now_empty` | `boolean` | yes | Same meaning as `TabDropInstruction::MoveToPane::source_now_empty`. |
| `split_direction` | `SplitDirection` | yes | `Left`/`Right` edges split horizontally; `Top`/`Bottom` split vertically — pre-resolved here so the host doesn't have to duplicate this mapping. |
| `target_pane_idx` | `number` | yes | The pane whose edge was targeted. |

<a id="wire-_tabgroupeventdividerresized"></a>
#### `_TabGroupEventDividerResized`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `divider_idx` | `number` | yes |  |
| `ratio` | `number` | yes |  |

<a id="wire-_tabgroupeventnewtabrequested"></a>
#### `_TabGroupEventNewTabRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `pane_idx` | `number` | yes |  |

<a id="wire-_tabgroupeventpaneadded"></a>
#### `_TabGroupEventPaneAdded`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `pane_idx` | `number` | yes |  |

<a id="wire-_tabgroupeventpanecollapsed"></a>
#### `_TabGroupEventPaneCollapsed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `pane_idx` | `number` | yes | Index the pane had *before* collapse. |

<a id="wire-_tabgroupeventpanefocused"></a>
#### `_TabGroupEventPaneFocused`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `pane_idx` | `number` | yes |  |

<a id="wire-_tabgroupeventtabactivated"></a>
#### `_TabGroupEventTabActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `pane_idx` | `number` | yes |  |
| `tab_id` | `string` | yes |  |

<a id="wire-_tabgroupeventtabclosed"></a>
#### `_TabGroupEventTabClosed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `pane_idx` | `number` | yes |  |
| `tab_id` | `string` | yes |  |

<a id="wire-_tabgroupeventtabdropped"></a>
#### `_TabGroupEventTabDropped`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `instruction` | `TabDropInstruction` | yes | Where it goes. |
| `tab_id` | `string` | yes | Id of the dropped tab. |

<a id="wire-_tabgroupeventtabmovedtopane"></a>
#### `_TabGroupEventTabMovedToPane`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `from_pane_idx` | `number` | yes |  |
| `insert_idx` | `number` | yes | Position in the target pane's tab list where the tab landed. |
| `tab_id` | `string` | yes |  |
| `to_pane_idx` | `number` | yes |  |

<a id="wire-_tabgroupeventtabreordered"></a>
#### `_TabGroupEventTabReordered`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `from_idx` | `number` | yes | Position before the move. |
| `pane_idx` | `number` | yes |  |
| `tab_id` | `string` | yes | Id of the tab that moved. |
| `to_idx` | `number` | yes | Position after the move. |

<a id="wire-_tabgroupeventtabsplittonewpane"></a>
#### `_TabGroupEventTabSplitToNewPane`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `edge` | `DropEdge` | yes | Which edge the tab was dropped onto. |
| `from_pane_idx` | `number` | yes |  |
| `new_pane_idx` | `number` | yes | Final index of the newly created pane (post-collapse). |
| `tab_id` | `string` | yes |  |
| `target_pane_idx` | `number` | yes | The pane whose edge was targeted (original index, pre-collapse). |

<a id="wire-_tabicon1"></a>
#### `_TabIcon1`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `codicon` | `string` | yes |  |
| `color` | `Color` | yes |  |

<a id="wire-_tabicon2"></a>
#### `_TabIcon2`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `color` | `Color` | yes |  |
| `glyph` | `string` | yes |  |

<a id="wire-_terminaleventdict"></a>
#### `_TerminalEventDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `KeyPressed` | `_TerminalEventKeyPressed` | A key was pressed with the terminal focused, for a host that reports keys per terminal rather than as plain `crate::UiEvent::KeyPressed`. |
| `SelectStart` | `_TerminalEventSelectStart` | User started selecting at `(row, col)` (0-based, content area). |
| `SelectExtend` | `_TerminalEventSelectExtend` | User dragged the selection to a new endpoint. |
| `Scroll` | `_TerminalEventScroll` | Mouse wheel scroll: positive = scroll content downward (toward live), negative = scroll backward into history. |

<a id="wire-_terminaleventkeypressed"></a>
#### `_TerminalEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_terminaleventscroll"></a>
#### `_TerminalEventScroll`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `delta` | `number` | yes |  |

<a id="wire-_terminaleventselectextend"></a>
#### `_TerminalEventSelectExtend`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_terminaleventselectstart"></a>
#### `_TerminalEventSelectStart`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_textdisplayeventautoscrolltoggled"></a>
#### `_TextDisplayEventAutoScrollToggled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `enabled` | `boolean` | yes |  |

<a id="wire-_textdisplayeventcopied"></a>
#### `_TextDisplayEventCopied`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |

<a id="wire-_textdisplayeventkeypressed"></a>
#### `_TextDisplayEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_textdisplayeventscrolled"></a>
#### `_TextDisplayEventScrolled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `new_offset` | `number` | yes |  |

<a id="wire-_toolbarbuttonaction"></a>
#### `_ToolbarButtonAction`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `enabled` | `boolean` | no | When false the button renders dimmed and clicks aren't dispatched. |
| `icon` | `string \| null` | no | Optional leading icon glyph (e.g. |
| `id` | `WidgetId` | yes |  |
| `is_active` | `boolean` | no | When true the button has a pressed / toggled visual (e.g. |
| `key_hint` | `string \| null` | no | Optional parenthesised key hint shown after the label (e.g. |
| `label` | `string` | yes | Primary label text (e.g. |
| `spec` | `ButtonSpec \| null` | no | Optional `crate::ButtonSpec` override, carrying the B3a vocabulary (variant intent, icon placement/gap, a richer tooltip) this variant's own legacy fields above can't express. |
| `tooltip` | `string` | no | Optional hover tooltip. |

<a id="wire-_toolbarbuttondict"></a>
#### `_ToolbarButtonDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Action` | `_ToolbarButtonAction` | A clickable action button. |
| `Label` | `_ToolbarButtonLabel` | A non-clickable label segment (e.g. |
| `Group` | `_ToolbarButtonGroup` | A bordered cluster of buttons — back/forward sharing one pill, or a view-mode segmented control where exactly one child is on. |
| `Field` | `_ToolbarButtonField` | An embedded `crate::primitives::text_field::TextField` — the Finder-style trailing-edge search box. |
| `MenuButton` | `_ToolbarButtonMenuButton` | A pull-down button — "Group by ▾", "Action ▾" — painted as an `Self::Action`-shaped pill plus a trailing `▾` affordance. |

<a id="wire-_toolbarbuttonfield"></a>
#### `_ToolbarButtonField`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `field` | `TextField` | yes |  |
| `id` | `WidgetId` | yes |  |
| `width` | `ToolbarItemWidth` | yes |  |

<a id="wire-_toolbarbuttongroup"></a>
#### `_ToolbarButtonGroup`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `exclusive` | `boolean` | no | When true, exactly one child is `is_active` and a click on any child reports `ToolbarEvent::GroupSelected` instead of `ToolbarEvent::ButtonClicked` — see `Toolbar::click`. |
| `id` | `WidgetId` | yes |  |
| `items` | `ToolbarButton[]` | yes |  |
| `label` | `string \| null` | no | Optional accessibility name for the group as a whole — a11y role `group` (`exclusive: false`) or `radiogroup` (`exclusive: true`); see `a11y::tree`'s `Toolbar` impl. |

<a id="wire-_toolbarbuttonlabel"></a>
#### `_ToolbarButtonLabel`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `fg` | `Color \| null` | no |  |
| `text` | `string` | yes |  |

<a id="wire-_toolbarbuttonmenubutton"></a>
#### `_ToolbarButtonMenuButton`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `icon` | `string \| null` | no |  |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |
| `menu` | `ContextMenuItem[]` | yes |  |

<a id="wire-_toolbareventbuttonclicked"></a>
#### `_ToolbarEventButtonClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `is_active` | `boolean` | yes |  |

<a id="wire-_toolbareventfieldchanged"></a>
#### `_ToolbarEventFieldChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `string` | yes |  |

<a id="wire-_toolbareventfieldcleared"></a>
#### `_ToolbarEventFieldCleared`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_toolbareventfieldcommitted"></a>
#### `_ToolbarEventFieldCommitted`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `string` | yes |  |

<a id="wire-_toolbareventgroupselected"></a>
#### `_ToolbarEventGroupSelected`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `group` | `WidgetId` | yes |  |
| `id` | `WidgetId` | yes |  |

<a id="wire-_toolbareventmenurequested"></a>
#### `_ToolbarEventMenuRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_toolbareventoverflowrequested"></a>
#### `_ToolbarEventOverflowRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-_toolbaritemwidthflex"></a>
#### `_ToolbarItemWidthFlex`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `max` | `number` | yes |  |
| `min` | `number` | yes |  |

<a id="wire-_treeeventcontextmenurequested"></a>
#### `_TreeEventContextMenuRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |
| `position` | `Point` | yes |  |

<a id="wire-_treeeventkeypressed"></a>
#### `_TreeEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_treeeventrowactivated"></a>
#### `_TreeEventRowActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |

<a id="wire-_treeeventrowclicked"></a>
#### `_TreeEventRowClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `modifiers` | `Modifiers` | yes |  |
| `path` | `number[]` | yes |  |

<a id="wire-_treeeventrowdoubleclicked"></a>
#### `_TreeEventRowDoubleClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |

<a id="wire-_treeeventrowrenamecancelled"></a>
#### `_TreeEventRowRenameCancelled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |

<a id="wire-_treeeventrowrenamechanged"></a>
#### `_TreeEventRowRenameChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cursor` | `number` | yes |  |
| `path` | `number[]` | yes |  |
| `selection_anchor` | `number \| null` | no |  |
| `text` | `string` | yes |  |

<a id="wire-_treeeventrowrenamecommitted"></a>
#### `_TreeEventRowRenameCommitted`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |
| `text` | `string` | yes |  |

<a id="wire-_treeeventrowtoggleexpand"></a>
#### `_TreeEventRowToggleExpand`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |

<a id="wire-_treeeventscrollchanged"></a>
#### `_TreeEventScrollChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `scroll_offset` | `number` | yes |  |

<a id="wire-_treeeventselectionchanged"></a>
#### `_TreeEventSelectionChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `number[]` | yes |  |

<a id="wire-_treeeventselectionset"></a>
#### `_TreeEventSelectionSet`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `paths` | `number[][]` | yes |  |

<a id="wire-_treetablecontrollerwireeventchildrenrequested"></a>
#### `_TreeTableControllerWireEventChildrenRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |

<a id="wire-_treetablecontrollerwireeventrowactivated"></a>
#### `_TreeTableControllerWireEventRowActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |

<a id="wire-_treetablecontrollerwireeventrowselected"></a>
#### `_TreeTableControllerWireEventRowSelected`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |

<a id="wire-_treetablecontrollerwireeventsortchanged"></a>
#### `_TreeTableControllerWireEventSortChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `ascending` | `boolean` | yes |  |
| `col` | `number` | yes |  |

<a id="wire-_treetablecontrollerwireeventtoggleexpand"></a>
#### `_TreeTableControllerWireEventToggleExpand`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `expand` | `boolean` | yes |  |
| `id` | `string` | yes |  |

<a id="wire-_treetableeventchildrenrequested"></a>
#### `_TreeTableEventChildrenRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |
| `path` | `number[]` | yes |  |

<a id="wire-_treetableeventcolumnresized"></a>
#### `_TreeTableEventColumnResized`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `width` | `number` | yes |  |

<a id="wire-_treetableeventcontextmenurequested"></a>
#### `_TreeTableEventContextMenuRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |
| `path` | `number[]` | yes |  |
| `position` | `Point` | yes |  |

<a id="wire-_treetableeventrowactivated"></a>
#### `_TreeTableEventRowActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |
| `path` | `number[]` | yes |  |

<a id="wire-_treetableeventrowdoubleclicked"></a>
#### `_TreeTableEventRowDoubleClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |
| `path` | `number[]` | yes |  |

<a id="wire-_treetableeventrowselected"></a>
#### `_TreeTableEventRowSelected`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |
| `path` | `number[]` | yes |  |

<a id="wire-_treetableeventscroll"></a>
#### `_TreeTableEventScroll`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `delta` | `number` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_treetableeventscrollchanged"></a>
#### `_TreeTableEventScrollChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `scroll_offset` | `number` | yes |  |

<a id="wire-_treetableeventselectionset"></a>
#### `_TreeTableEventSelectionSet`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `indices` | `number[]` | yes |  |
| `paths` | `number[][]` | yes |  |

<a id="wire-_treetableeventsortchanged"></a>
#### `_TreeTableEventSortChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `ascending` | `boolean` | yes |  |
| `col` | `number` | yes |  |

<a id="wire-_treetableeventtoggleexpanded"></a>
#### `_TreeTableEventToggleExpanded`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `expand` | `boolean` | yes |  |
| `path` | `number[]` | yes |  |

<a id="wire-_uieventbuttonactivated"></a>
#### `_UiEventButtonActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `widget` | `WidgetId` | yes |  |

<a id="wire-_uieventcommandcenteractivated"></a>
#### `_UiEventCommandCenterActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `target` | `CommandCenterTarget` | yes |  |
| `widget` | `WidgetId` | yes |  |

<a id="wire-_uieventdict"></a>
#### `_UiEventDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Accelerator` | `[AcceleratorId, Modifiers]` | A declared accelerator fired. |
| `GlobalShortcut` | `GlobalShortcutId` | The OS delivered a global shortcut — a key combination registered through `PlatformServices::register_global_shortcut` that fires regardless of whether this app's window has focus. |
| `KeyPressed` | `_UiEventKeyPressed` | A raw key was pressed. |
| `CharTyped` | `string` | An IME **committed** the composed text for one character. |
| `MouseDown` | `_UiEventMouseDown` |  |
| `MouseUp` | `_UiEventMouseUp` |  |
| `MouseMoved` | `_UiEventMouseMoved` |  |
| `MouseEntered` | `_UiEventMouseEntered` | Optional capability — hover state is a real, likely-future desktop need (tooltip auto-show, hover highlighting) that has no substitute mechanism. |
| `MouseLeft` | `_UiEventMouseLeft` | See `Self::MouseEntered` — same optional-capability status, same rationale, always emitted/not-emitted as a pair. |
| `DoubleClick` | `_UiEventDoubleClick` |  |
| `Scroll` | `_UiEventScroll` | Scroll-wheel event. |
| `WindowResized` | `_UiEventWindowResized` |  |
| `WindowFocused` | `boolean` |  |
| `DpiChanged` | `number` | Optional capability — Win emits this from `WM_DPICHANGED`; GTK observes the `DrawingArea`'s `notify::scale-factor` signal and macOS observes `NSWindowDidChangeBackingPropertiesNotification`, both firing on a live runtime DPI change (monitor move, external monitor plug/unplug) as well as at startup; TUI is always `scale == 1.0`, so it never applies there. |
| `WindowStateChanged` | `_UiEventWindowStateChanged` | Optional capability — one of the window's tri-state chrome flags (maximize/restore, fullscreen enter/exit, minimize/restore) changed: a window-manager keyboard shortcut, a title-bar double-click GTK/macOS handle natively, `Cmd+Ctrl+F`, or the app's own `Backend::toggle_window_maximize` / `WindowControl::set_fullscreen` call. |
| `FilesDropped` | `_UiEventFilesDropped` | Optional capability — emitted by GTK (`gtk::DropTarget` on the `DrawingArea`), macOS (`NSDraggingDestination` on `QuadraView`), and Windows (`WM_DROPFILES`) when the user drops OS files onto the window. |
| `ClipboardPaste` | `string` |  |
| `OpenRequested` | `_UiEventOpenRequested` | The OS asked this app to open one or more URLs and/or files. |
| `TextCopied` | `string` | Text was copied to the clipboard by the TUI runner's built-in text-selection mechanism (click-drag → Ctrl-C). |
| `FocusChanged` | `WidgetId \| null` | Keyboard focus moved (or was cleared). |
| `SystemThemeChanged` | `SystemTheme` | The OS-level light/dark/accent/high-contrast preference changed while the app was running — the live counterpart to `PlatformServices::system_theme`'s poll-on-demand query. |
| `PowerStateChanged` | `_UiEventPowerStateChanged` | A power-state transition occurred: the system is suspending or resumed, switched between battery and AC, or the screen locked or unlocked. |
| `ScrollOffsetChanged` | `_UiEventScrollOffsetChanged` | A scrollbar drag or click resolved to a new offset. |
| `ScrollbarMoved` | `_UiEventScrollbarMoved` | A standalone `crate::Scrollbar` widget resolved a gesture — thumb drag, track page, or wheel — to a new thumb position, expressed as `fraction` of the thumb's available travel (`0.0` = start of the track, `1.0` = end). |
| `SwitchToggled` | `_UiEventSwitchToggled` | A standalone `crate::Switch` flipped, via `crate::Switch::handle_key` (`Enter`/`Space` while focused) or `crate::Switch::click` (a left click, regardless of focus). |
| `RadioGroupSelectionChanged` | `_UiEventRadioGroupSelectionChanged` | A standalone `crate::RadioGroup`'s selection changed, via `crate::RadioGroup::handle_key` (arrow-key cursor movement) or `crate::RadioGroup::select` (a click on a specific option). |
| `ButtonActivated` | `_UiEventButtonActivated` | A standalone `crate::Button` activated, via `crate::Button::handle_key` (`Enter`/`Space` while focused) or `crate::Button::click` (a left click, regardless of focus). |
| `CommandCenterActivated` | `_UiEventCommandCenterActivated` | A `crate::CommandCenter`'s back arrow, forward arrow or search box was activated — a left press on that part, or an assistive-technology `Click` (which activates the search box). |
| `TextSelectionChanged` | `_UiEventTextSelectionChanged` | A mouse text-selection drag extended or finalised its range. |
| `SplitDividerDragged` | `_UiEventSplitDividerDragged` | A `crate::SplitTree` — or `crate::Split` — divider drag moved. |
| `MenuActivated` | `WidgetId` | A menu item was activated via a native menu installer (`Backend::install_menu_bar`). |
| `ContextMenuItemActivated` | `WidgetId` | A right-click context menu — native (via `Backend::show_context_menu`) or in-window painted (via `crate::compose::ContextMenuController`) — had one of its items activated. |
| `MessageDialogResolved` | `WidgetId` | An in-canvas message/alert/confirm `crate::Dialog` opened via `crate::compose::MessageDialog` was resolved — the user activated one of its buttons by click, Enter, or Escape resolving to the cancel/default button. |
| `ToastActionClicked` | `WidgetId` | An action button on an in-canvas `crate::Toast` (painted via a binding's `show_toast`-style surface, on the same anchored-overlay slot `MessageDialogResolved` uses) was clicked. |
| `CompletionItemActivated` | `[WidgetId, number]` | An item in an in-canvas `crate::Completions` popup was activated by a click or Enter. |
| `RichTextLinkClicked` | `[WidgetId, string]` | A clickable link inside an in-canvas `crate::RichTextPopup` was clicked. |
| `TrayClicked` | `_UiEventTrayClicked` | The user clicked the app's tray/status-bar icon (`crate::backend::TrayService::set_icon`) with no menu currently intercepting the click. |
| `NotificationActivated` | `_UiEventNotificationActivated` | The user activated a system notification sent through `crate::backend::PlatformServices::send_notification` — either the notification body itself (`action: None`) or one of the action buttons registered on it via `crate::backend::Notification::with_action` (`action: Some(id)`, the same `WidgetId` passed to that call). |
| `Tree` | `[WidgetId, TreeEvent]` |  |
| `List` | `[WidgetId, ListViewEvent]` |  |
| `Form` | `[WidgetId, FormEvent]` |  |
| `Palette` | `[WidgetId, PaletteEvent]` |  |
| `TabBar` | `[WidgetId, TabBarEvent]` |  |
| `StatusBar` | `[WidgetId, StatusBarEvent]` |  |
| `ActivityBar` | `[WidgetId, ActivityBarEvent]` |  |
| `Terminal` | `[WidgetId, TerminalEvent]` |  |
| `TextDisplay` | `[WidgetId, TextDisplayEvent]` |  |
| `Chart` | `[WidgetId, ChartEvent]` |  |
| `DataTable` | `[WidgetId, DataTableEvent]` |  |
| `TreeTable` | `[WidgetId, TreeTableEvent]` | A `crate::TreeTable` chevron/row/header/column interaction — see `TreeTableEvent`'s own variants, in particular `TreeTableEvent::ToggleExpanded` for the branch-expand chevron click `DataTableEvent` has no counterpart for. |
| `CellGrid` | `[WidgetId, CellGridEvent]` | A `crate::CellGrid` cell was clicked, moving the cursor to its absolute `(row, col)` — see `CellGridEvent::CursorMoved`. |
| `MultiSectionView` | `[WidgetId, MultiSectionViewEvent]` | A `crate::MultiSectionView` header, divider, scrollbar or aux input was used, or one of its section bodies reported an event — see `crate::MultiSectionViewEvent`. |
| `Chat` | `[WidgetId, ChatEvent]` | A `crate::Chat` was submitted, cancelled or clicked, or one of its controlled fields changed — see `crate::ChatEvent`. |
| `Sidebar` | `[WidgetId, SidebarAction]` | A `crate::Sidebar` section, row or form field was used, or one of its controlled fields changed — see `crate::SidebarAction`. |
| `TabGroup` | `[WidgetId, TabGroupEvent]` | A `crate::TabGroup` tab, divider or pane was used — see `crate::TabGroupEvent`. |
| `ScrollView` | `[WidgetId, ScrollViewEvent]` | A `crate::ScrollView` resolved a wheel, key or scrollbar gesture to a new offset — see `crate::ScrollViewEvent`. |
| `Editor` | `[WidgetId, EditorEvent]` | A pointer or wheel gesture on a `crate::Editor`, resolved against its layout — see `crate::EditorEvent`. |
| `Minimap` | `[WidgetId, MinimapEvent]` | A click or drag on a `crate::Minimap` — see `crate::MinimapEvent`. |
| `Board` | `[WidgetId, BoardAction]` | A key, click or context-menu request on a `crate::BoardModel` — see `crate::BoardAction`. |
| `PipelineView` | `[WidgetId, PipelineEvent]` | A stage of a `crate::PipelineView` was focused, selected or activated — see `crate::PipelineEvent`. |
| `DiffView` | `[WidgetId, DiffViewEvent]` | A `crate::DiffView` scrolled or moved focus between its panes — see `crate::DiffViewEvent`. |
| `MessageList` | `[WidgetId, MessageListEvent]` | A `crate::MessageList` scrolled — see `crate::MessageListEvent`. |
| `FindReplace` | `[WidgetId, FindReplaceEvent]` | A `crate::FindReplacePanel` field was edited, a switch flipped, or a match/replace/close action requested — see `crate::FindReplaceEvent`. |
| `HelpOverlay` | `[WidgetId, HelpOverlayAction]` | A `crate::compose::HelpOverlay` cheatsheet asked to open or close; the host writes it onto the overlay's `open`. |
| `PathPicker` | `[WidgetId, PathPickerAction]` | A `crate::compose::FilePicker` or `crate::compose::FolderPicker` confirmed a path, was dismissed, or asked the host to apply a browsing change. |
| `DualModePalette` | `[WidgetId, DualModePaletteAction]` | A `crate::compose::DualModePalette` confirmed an item or text, was dismissed, or asked the host to apply a query, mode or highlight change. |
| `AppMenu` | `[WidgetId, AppMenuEvent]` | A `crate::compose::AppMenu` opened, closed or moved within its cascade; the host applies the new view onto the menu. |
| `ShellView` | `[WidgetId, ShellEvent]` | An app shell (`crate::ShellView`) resolved a gesture on its own chrome — a panel switch, a sidebar or bottom-panel resize or hide, a bottom activity item click. |
| `BottomPanel` | `[WidgetId, BottomPanelEvent]` | A `crate::BottomPanel`'s tab strip activated or closed a tab, or toggled the panel maximised. |
| `Workspace` | `[WidgetId, WorkspaceEvent]` | A `crate::Workspace` strip asked to activate, close or promote a document. |
| `Toolbar` | `[WidgetId, ToolbarEvent]` | A `crate::Toolbar` button was clicked (mouse) or activated via Enter/Space while keyboard-focused — the id is the toolbar's own, matching every other chrome-bar variant above (`StatusBar`, `ActivityBar`); the clicked/activated button's own id and new `is_active` state are carried in the `ToolbarEvent` payload. |
| `Canvas` | `[WidgetId, CanvasEvent]` | A `crate::Canvas`'s declared `crate::CanvasRegion` was clicked, double-clicked, or gained/lost pointer hover — the id is the canvas's own, matching every other primitive-specific variant above; which region (if any) is carried in the `CanvasEvent` payload. |
| `BackendNative` | `BackendNativeEvent` | Backend-specific event the crate couldn't normalise. |
| `User` | `UserPayload` | An app-defined value delivered from a background thread through `crate::Backend::waker`. |

<a id="wire-_uieventdoubleclick"></a>
#### `_UiEventDoubleClick`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `position` | `Point` | yes |  |
| `widget` | `WidgetId \| null` | no |  |

<a id="wire-_uieventfilesdropped"></a>
#### `_UiEventFilesDropped`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `paths` | `string[]` | yes |  |
| `position` | `Point` | yes |  |

<a id="wire-_uieventkeypressed"></a>
#### `_UiEventKeyPressed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `Key` | yes |  |
| `modifiers` | `Modifiers` | yes |  |
| `repeat` | `boolean` | yes |  |

<a id="wire-_uieventmousedown"></a>
#### `_UiEventMouseDown`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `button` | `MouseButton` | yes |  |
| `modifiers` | `Modifiers` | yes |  |
| `position` | `Point` | yes |  |
| `widget` | `WidgetId \| null` | no |  |

<a id="wire-_uieventmouseentered"></a>
#### `_UiEventMouseEntered`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `widget` | `WidgetId` | yes |  |

<a id="wire-_uieventmouseleft"></a>
#### `_UiEventMouseLeft`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `widget` | `WidgetId` | yes |  |

<a id="wire-_uieventmousemoved"></a>
#### `_UiEventMouseMoved`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `buttons` | `ButtonMask` | yes |  |
| `position` | `Point` | yes |  |

<a id="wire-_uieventmouseup"></a>
#### `_UiEventMouseUp`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `button` | `MouseButton` | yes |  |
| `position` | `Point` | yes |  |
| `widget` | `WidgetId \| null` | no |  |

<a id="wire-_uieventnotificationactivated"></a>
#### `_UiEventNotificationActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `action` | `WidgetId \| null` | no |  |
| `tag` | `string \| null` | no |  |

<a id="wire-_uieventopenrequested"></a>
#### `_UiEventOpenRequested`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `files` | `string[]` | yes |  |
| `urls` | `string[]` | yes |  |

<a id="wire-_uieventpowerstatechanged"></a>
#### `_UiEventPowerStateChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `PowerEventKind` | yes |  |

<a id="wire-_uieventradiogroupselectionchanged"></a>
#### `_UiEventRadioGroupSelectionChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |
| `widget` | `WidgetId` | yes |  |

<a id="wire-_uieventscroll"></a>
#### `_UiEventScroll`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `delta` | `ScrollDelta` | yes |  |
| `position` | `Point` | yes |  |
| `widget` | `WidgetId \| null` | no |  |

<a id="wire-_uieventscrolloffsetchanged"></a>
#### `_UiEventScrollOffsetChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `new_offset` | `number` | yes |  |
| `widget` | `WidgetId` | yes |  |

<a id="wire-_uieventscrollbarmoved"></a>
#### `_UiEventScrollbarMoved`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `fraction` | `number` | yes |  |
| `widget` | `WidgetId` | yes |  |

<a id="wire-_uieventsplitdividerdragged"></a>
#### `_UiEventSplitDividerDragged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `new_ratio` | `number` | yes |  |
| `split_index` | `number` | yes |  |
| `tree` | `WidgetId` | yes |  |

<a id="wire-_uieventswitchtoggled"></a>
#### `_UiEventSwitchToggled`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `boolean` | yes |  |
| `widget` | `WidgetId` | yes |  |

<a id="wire-_uieventtextselectionchanged"></a>
#### `_UiEventTextSelectionChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `anchor` | `Point` | yes |  |
| `focus` | `Point` | yes |  |
| `region` | `WidgetId` | yes |  |

<a id="wire-_uieventtrayclicked"></a>
#### `_UiEventTrayClicked`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `button` | `MouseButton` | yes |  |

<a id="wire-_uieventwindowresized"></a>
#### `_UiEventWindowResized`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `viewport` | `Viewport` | yes |  |

<a id="wire-_uieventwindowstatechanged"></a>
#### `_UiEventWindowStateChanged`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `fullscreen` | `boolean` | yes |  |
| `maximized` | `boolean` | yes |  |
| `minimized` | `boolean` | yes |  |

<a id="wire-_workspaceeventactivated"></a>
#### `_WorkspaceEventActivated`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes | The now-active document's opaque id. |
| `index` | `number` | yes | Its index in the strip. |

<a id="wire-_workspaceeventclosed"></a>
#### `_WorkspaceEventClosed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes | The removed document's opaque id. |
| `index` | `number` | yes | The index it occupied *before* removal. |

<a id="wire-_workspaceeventopened"></a>
#### `_WorkspaceEventOpened`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes | The document's opaque id. |
| `index` | `number` | yes | Where it landed in the strip. |

<a id="wire-_workspaceeventpromoted"></a>
#### `_WorkspaceEventPromoted`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes | The promoted document's opaque id. |

<a id="wire-_workspaceeventreordered"></a>
#### `_WorkspaceEventReordered`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `number` | yes | Its index before the move. |
| `id` | `string` | yes | The moved document's opaque id. |
| `to` | `number` | yes | Its index after the move. |

<!-- END GENERATED: scripts/gen_wire_types.py -->
