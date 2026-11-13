# Bindings wire format (v1)

A `quadraui-py`/`quadraui-node` render callback returns, and a `quadraui-py`/
`quadraui-node` event callback receives, plain `dict`/object values -- JSON-
shaped data, not Rust types. This page is the reference for that shape: the
layout tree a render callback returns, the leaf descriptors it can embed, the
two shapes a `UiEvent` can take on the wire, the controlled/uncontrolled
resolution rule that applies to several descriptor fields, and the
`wire_version` compatibility policy.

The per-kind field tables below (*Generated reference*) are rendered
straight from `quadraui-binding-core/src/wire_schema.rs` by
`scripts/gen_wire_types.py`'s Markdown renderer -- the same Rust-side schema
that produces `quadraui-py/python/quadraui/wire.pyi` and
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
| `"split"` | `LayoutSplitNode` | Two children (`first`/`second`) either side of a draggable divider. |
| `"tabs"` | `LayoutTabsNode` | A tab strip, each tab's `content` itself a `LayoutNode`. |
| `"panel"` | `LayoutPanelNode` | Deprecated chrome wrapper around one child -- prefer `"sidebar_panel"`. |
| `"sidebar_panel"` | `LayoutSidebarPanelNode` | Chrome (toolbar, accent) wrapper around one child. |
| `"split_tree"` | `LayoutSplitTreeNode` | A recursive tree of splits (`SplitTreeRecursiveNode`), for more than two panes. |
| `"leaf"` | `LayoutLeafNode` | A primitive widget -- see *Leaf kinds* below. |

A `"row"`/`"column"`/`"split"`/`"tabs"`/`"panel"`/`"sidebar_panel"`/
`"split_tree"` node also accepts the sizing keys `"size"`, `"min"`, `"max"`
when it's itself a `"row"`/`"column"` child, and the chrome keys `"padding"`,
`"margin"`, `"gap"` where that node kind supports them -- see each node's own
field table in the generated reference for exactly which it accepts.

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

## `wire_version` policy

Every top-level `RenderOutput` (any `LayoutNode` variant, or
`RenderFrameDict`) may carry an optional `"wire_version"` integer key --
never on a nested child node. `quadraui-binding-core::wire_schema::WIRE_VERSION`
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

## Generated reference

<!-- BEGIN GENERATED: scripts/gen_wire_types.py (wire_schema.rs reference) -->

<!-- toc -->

- [`WidgetId`](#wire-widgetid)
- [`TreePath`](#wire-treepath)
- [`AcceleratorId`](#wire-acceleratorid)
- [`GlobalShortcutId`](#wire-globalshortcutid)
- [`Color`](#wire-color)
- [`StyledSpan`](#wire-styledspan)
- [`StyledText`](#wire-styledtext)
- [`IconGlyph`](#wire-iconglyph)
- [`IconCodicon`](#wire-iconcodicon)
- [`Icon`](#wire-icon)
- [`Badge`](#wire-badge)
- [`Decoration`](#wire-decoration)
- [`SelectionMode`](#wire-selectionmode)
- [`TreeStyle`](#wire-treestyle)
- [`Modifiers`](#wire-modifiers)
- [`Point`](#wire-point)
- [`ScrollDelta`](#wire-scrolldelta)
- [`Rect`](#wire-rect)
- [`ButtonMask`](#wire-buttonmask)
- [`Viewport`](#wire-viewport)
- [`SystemTheme`](#wire-systemtheme)
- [`PowerEventKind`](#wire-powereventkind)
- [`_NamedKeyF`](#wire-_namedkeyf)
- [`NamedKey`](#wire-namedkey)
- [`Key`](#wire-key)
- [`_MouseButtonOther`](#wire-_mousebuttonother)
- [`MouseButton`](#wire-mousebutton)
- [`BackendNativeEvent`](#wire-backendnativeevent)
- [`StatusBarSegment`](#wire-statusbarsegment)
- [`StatusBarDescriptor`](#wire-statusbardescriptor)
- [`ButtonVariant`](#wire-buttonvariant)
- [`ButtonIconPosition`](#wire-buttoniconposition)
- [`ButtonDescriptor`](#wire-buttondescriptor)
- [`ButtonSpecDescriptor`](#wire-buttonspecdescriptor)
- [`TextDisplayLineDescriptor`](#wire-textdisplaylinedescriptor)
- [`TextDisplayDescriptor`](#wire-textdisplaydescriptor)
- [`ListItemDescriptor`](#wire-listitemdescriptor)
- [`ListViewDescriptor`](#wire-listviewdescriptor)
- [`TreeRowEditStateDescriptor`](#wire-treeroweditstatedescriptor)
- [`TreeRowDescriptor`](#wire-treerowdescriptor)
- [`TreeViewDescriptor`](#wire-treeviewdescriptor)
- [`ColumnAlign`](#wire-columnalign)
- [`SortDirection`](#wire-sortdirection)
- [`_ColumnWidthContent`](#wire-_columnwidthcontent)
- [`ColumnWidth`](#wire-columnwidth)
- [`ColumnDescriptor`](#wire-columndescriptor)
- [`DataRowDescriptor`](#wire-datarowdescriptor)
- [`DataTableDescriptor`](#wire-datatabledescriptor)
- [`TabItemDescriptor`](#wire-tabitemdescriptor)
- [`TabBarSegmentDescriptor`](#wire-tabbarsegmentdescriptor)
- [`TabBarDescriptor`](#wire-tabbardescriptor)
- [`_ToolbarButtonAction`](#wire-_toolbarbuttonaction)
- [`_ToolbarButtonActionWrapper`](#wire-_toolbarbuttonactionwrapper)
- [`_ToolbarButtonLabel`](#wire-_toolbarbuttonlabel)
- [`_ToolbarButtonLabelWrapper`](#wire-_toolbarbuttonlabelwrapper)
- [`ToolbarButtonDescriptor`](#wire-toolbarbuttondescriptor)
- [`ToolbarDescriptor`](#wire-toolbardescriptor)
- [`PanelActionDescriptor`](#wire-panelactiondescriptor)
- [`PaddingAxis`](#wire-paddingaxis)
- [`PaddingSides`](#wire-paddingsides)
- [`PaddingValue`](#wire-paddingvalue)
- [`LayoutRowNode`](#wire-layoutrownode)
- [`LayoutColumnNode`](#wire-layoutcolumnnode)
- [`LayoutSplitPayload`](#wire-layoutsplitpayload)
- [`LayoutSplitNode`](#wire-layoutsplitnode)
- [`LayoutTabEntryNode`](#wire-layouttabentrynode)
- [`LayoutTabsPayload`](#wire-layouttabspayload)
- [`LayoutTabsNode`](#wire-layouttabsnode)
- [`LayoutPanelPayload`](#wire-layoutpanelpayload)
- [`LayoutPanelNode`](#wire-layoutpanelnode)
- [`LayoutSidebarPanelPayload`](#wire-layoutsidebarpanelpayload)
- [`LayoutSidebarPanelNode`](#wire-layoutsidebarpanelnode)
- [`LayoutScrollPayload`](#wire-layoutscrollpayload)
- [`LayoutScrollNode`](#wire-layoutscrollnode)
- [`_SplitTreeLeaf`](#wire-_splittreeleaf)
- [`SplitTreeSplitPayload`](#wire-splittreesplitpayload)
- [`_SplitTreeSplitWrapper`](#wire-_splittreesplitwrapper)
- [`SplitTreeRecursiveNode`](#wire-splittreerecursivenode)
- [`LayoutSplitTreePayload`](#wire-layoutsplittreepayload)
- [`LayoutSplitTreeNode`](#wire-layoutsplittreenode)
- [`ProgressBarDescriptor`](#wire-progressbardescriptor)
- [`SpinnerDescriptor`](#wire-spinnerdescriptor)
- [`_ImageSourceBytes`](#wire-_imagesourcebytes)
- [`_ImageSourcePath`](#wire-_imagesourcepath)
- [`ImageSource`](#wire-imagesource)
- [`ImageFit`](#wire-imagefit)
- [`ImageDescriptor`](#wire-imagedescriptor)
- [`TextInputDescriptor`](#wire-textinputdescriptor)
- [`NumberInputDescriptor`](#wire-numberinputdescriptor)
- [`ComboBoxDescriptor`](#wire-comboboxdescriptor)
- [`SwitchDescriptor`](#wire-switchdescriptor)
- [`ScrollAxis`](#wire-scrollaxis)
- [`ScrollbarDescriptor`](#wire-scrollbardescriptor)
- [`ScrollViewDescriptor`](#wire-scrollviewdescriptor)
- [`CommandLineDescriptor`](#wire-commandlinedescriptor)
- [`CommandCenterDescriptor`](#wire-commandcenterdescriptor)
- [`RadioOptionDescriptor`](#wire-radiooptiondescriptor)
- [`RadioGroupDescriptor`](#wire-radiogroupdescriptor)
- [`MessageRowDescriptor`](#wire-messagerowdescriptor)
- [`MessageListDescriptor`](#wire-messagelistdescriptor)
- [`Weekday`](#wire-weekday)
- [`DateFormat`](#wire-dateformat)
- [`DateLocale`](#wire-datelocale)
- [`TimeOfDay`](#wire-timeofday)
- [`DatePickerDescriptor`](#wire-datepickerdescriptor)
- [`ChartKind`](#wire-chartkind)
- [`SeriesDescriptor`](#wire-seriesdescriptor)
- [`ChartDescriptor`](#wire-chartdescriptor)
- [`StageStatus`](#wire-stagestatus)
- [`PipelineStageDescriptor`](#wire-pipelinestagedescriptor)
- [`PipelineViewDescriptor`](#wire-pipelineviewdescriptor)
- [`BadgeStatus`](#wire-badgestatus)
- [`CardBadgeDescriptor`](#wire-cardbadgedescriptor)
- [`BoardCardDescriptor`](#wire-boardcarddescriptor)
- [`BoardColumnDescriptor`](#wire-boardcolumndescriptor)
- [`BoardModelDescriptor`](#wire-boardmodeldescriptor)
- [`MinimapLineDescriptor`](#wire-minimaplinedescriptor)
- [`MinimapSpanDescriptor`](#wire-minimapspandescriptor)
- [`MinimapDescriptor`](#wire-minimapdescriptor)
- [`ActivityItemDescriptor`](#wire-activityitemdescriptor)
- [`ActivityBarDescriptor`](#wire-activitybardescriptor)
- [`PaletteMode`](#wire-palettemode)
- [`PaletteItemDescriptor`](#wire-paletteitemdescriptor)
- [`PalettePreviewDescriptor`](#wire-palettepreviewdescriptor)
- [`PaletteDescriptor`](#wire-palettedescriptor)
- [`TerminalCursorShape`](#wire-terminalcursorshape)
- [`TerminalCellDescriptor`](#wire-terminalcelldescriptor)
- [`TerminalScrollbarDescriptor`](#wire-terminalscrollbardescriptor)
- [`TerminalDescriptor`](#wire-terminaldescriptor)
- [`CanvasFontRole`](#wire-canvasfontrole)
- [`_DrawOpRect`](#wire-_drawoprect)
- [`_DrawOpRoundedRect`](#wire-_drawoproundedrect)
- [`_DrawOpLine`](#wire-_drawopline)
- [`_DrawOpPath`](#wire-_drawoppath)
- [`_DrawOpTextRun`](#wire-_drawoptextrun)
- [`_DrawOpImage`](#wire-_drawopimage)
- [`_DrawOpPushClip`](#wire-_drawoppushclip)
- [`_DrawOpPolygon`](#wire-_drawoppolygon)
- [`_DrawOpEllipse`](#wire-_drawopellipse)
- [`_DrawOpArc`](#wire-_drawoparc)
- [`_DrawOpBezier`](#wire-_drawopbezier)
- [`_DrawOpDashedLine`](#wire-_drawopdashedline)
- [`_DrawOpStrokeRect`](#wire-_drawopstrokerect)
- [`_DrawOpStyledTextRun`](#wire-_drawopstyledtextrun)
- [`_DrawOpDict`](#wire-_drawopdict)
- [`DrawOp`](#wire-drawop)
- [`CanvasDescriptor`](#wire-canvasdescriptor)
- [`DiagnosticSeverity`](#wire-diagnosticseverity)
- [`GitLineStatus`](#wire-gitlinestatus)
- [`EditorDiffLine`](#wire-editordiffline)
- [`CursorShape`](#wire-cursorshape)
- [`SelectionKind`](#wire-selectionkind)
- [`CursorPos`](#wire-cursorpos)
- [`EditorCursor`](#wire-editorcursor)
- [`EditorSelection`](#wire-editorselection)
- [`EditorStyle`](#wire-editorstyle)
- [`EditorStyledSpan`](#wire-editorstyledspan)
- [`DiagnosticMark`](#wire-diagnosticmark)
- [`SpellMark`](#wire-spellmark)
- [`EditorLine`](#wire-editorline)
- [`EditorDescriptor`](#wire-editordescriptor)
- [`_FrClickFindInput`](#wire-_frclickfindinput)
- [`_FrClickReplaceInput`](#wire-_frclickreplaceinput)
- [`FindReplaceClickTarget`](#wire-findreplaceclicktarget)
- [`FrHitRegion`](#wire-frhitregion)
- [`FindReplacePanelDescriptor`](#wire-findreplacepaneldescriptor)
- [`_ControlWidthFixed`](#wire-_controlwidthfixed)
- [`ControlWidth`](#wire-controlwidth)
- [`LabelPosition`](#wire-labelposition)
- [`LabelAlign`](#wire-labelalign)
- [`RowAlign`](#wire-rowalign)
- [`FormSpacing`](#wire-formspacing)
- [`_ValidationStateError`](#wire-_validationstateerror)
- [`_ValidationStateWarning`](#wire-_validationstatewarning)
- [`ValidationState`](#wire-validationstate)
- [`ToggleGroupItem`](#wire-togglegroupitem)
- [`ButtonRowItem`](#wire-buttonrowitem)
- [`_FieldKindTextInput`](#wire-_fieldkindtextinput)
- [`_FieldKindReadOnly`](#wire-_fieldkindreadonly)
- [`_FieldKindSlider`](#wire-_fieldkindslider)
- [`_FieldKindColorPicker`](#wire-_fieldkindcolorpicker)
- [`_FieldKindDropdown`](#wire-_fieldkinddropdown)
- [`_FieldKindTextArea`](#wire-_fieldkindtextarea)
- [`_FieldKindPasswordInput`](#wire-_fieldkindpasswordinput)
- [`_FieldKindSegmentedControl`](#wire-_fieldkindsegmentedcontrol)
- [`_FieldKindToggleGroup`](#wire-_fieldkindtogglegroup)
- [`_FieldKindButtonRow`](#wire-_fieldkindbuttonrow)
- [`_FieldKindToolbar`](#wire-_fieldkindtoolbar)
- [`_FieldKindDatePicker`](#wire-_fieldkinddatepicker)
- [`_FieldKindComboBox`](#wire-_fieldkindcombobox)
- [`_FieldKindNumberInput`](#wire-_fieldkindnumberinput)
- [`_FieldKindRadioGroup`](#wire-_fieldkindradiogroup)
- [`_FieldKindToggle`](#wire-_fieldkindtoggle)
- [`_FieldKindDict`](#wire-_fieldkinddict)
- [`FieldKind`](#wire-fieldkind)
- [`FormFieldDescriptor`](#wire-formfielddescriptor)
- [`FormDescriptor`](#wire-formdescriptor)
- [`MsvAxis`](#wire-msvaxis)
- [`ScrollMode`](#wire-scrollmode)
- [`_SectionSizeFixed`](#wire-_sectionsizefixed)
- [`_SectionSizePercent`](#wire-_sectionsizepercent)
- [`_SectionSizeWeight`](#wire-_sectionsizeweight)
- [`SectionSize`](#wire-sectionsize)
- [`HeaderAction`](#wire-headeraction)
- [`SectionHeader`](#wire-sectionheader)
- [`EmptyBody`](#wire-emptybody)
- [`InlineInput`](#wire-inlineinput)
- [`_SectionAuxInput`](#wire-_sectionauxinput)
- [`_SectionAuxToolbar`](#wire-_sectionauxtoolbar)
- [`_SectionAuxSearch`](#wire-_sectionauxsearch)
- [`_SectionAuxCustom`](#wire-_sectionauxcustom)
- [`SectionAux`](#wire-sectionaux)
- [`_SectionBodyTree`](#wire-_sectionbodytree)
- [`_SectionBodyList`](#wire-_sectionbodylist)
- [`_SectionBodyForm`](#wire-_sectionbodyform)
- [`_SectionBodyTerminal`](#wire-_sectionbodyterminal)
- [`_SectionBodyMessageList`](#wire-_sectionbodymessagelist)
- [`_SectionBodyText`](#wire-_sectionbodytext)
- [`_SectionBodyChart`](#wire-_sectionbodychart)
- [`_SectionBodyEmpty`](#wire-_sectionbodyempty)
- [`_SectionBodyCustom`](#wire-_sectionbodycustom)
- [`SectionBody`](#wire-sectionbody)
- [`Section`](#wire-section)
- [`MultiSectionViewDescriptor`](#wire-multisectionviewdescriptor)
- [`_KeyBindingLiteral`](#wire-_keybindingliteral)
- [`KeyBinding`](#wire-keybinding)
- [`_AcceleratorScopeWidget`](#wire-_acceleratorscopewidget)
- [`_AcceleratorScopeMode`](#wire-_acceleratorscopemode)
- [`AcceleratorScope`](#wire-acceleratorscope)
- [`Accelerator`](#wire-accelerator)
- [`ContextMenuItem`](#wire-contextmenuitem)
- [`MenuBarItemDescriptor`](#wire-menubaritemdescriptor)
- [`MenuBarDescriptor`](#wire-menubardescriptor)
- [`DiffRowKind`](#wire-diffrowkind)
- [`DiffRow`](#wire-diffrow)
- [`DiffHunk`](#wire-diffhunk)
- [`DiffMode`](#wire-diffmode)
- [`DiffEditability`](#wire-diffeditability)
- [`DiffPane`](#wire-diffpane)
- [`DiffViewDescriptor`](#wire-diffviewdescriptor)
- [`LayoutLeafNode`](#wire-layoutleafnode)
- [`LayoutNode`](#wire-layoutnode)
- [`RenderFrameDict`](#wire-renderframedict)
- [`RenderOutput`](#wire-renderoutput)
- [`PanelDefinitionDescriptor`](#wire-paneldefinitiondescriptor)
- [`BottomPanelTabDescriptor`](#wire-bottompaneltabdescriptor)
- [`BottomPanelConfigDescriptor`](#wire-bottompanelconfigdescriptor)
- [`ShellConfigDescriptor`](#wire-shellconfigdescriptor)
- [`_KeyPressedStrPayload`](#wire-_keypressedstrpayload)
- [`_TreePathPayload`](#wire-_treepathpayload)
- [`_TreeRowClickedPayload`](#wire-_treerowclickedpayload)
- [`_TreeSelectionSetPayload`](#wire-_treeselectionsetpayload)
- [`_TreeRowRenameChangedPayload`](#wire-_treerowrenamechangedpayload)
- [`_TreeRowRenameCommittedPayload`](#wire-_treerowrenamecommittedpayload)
- [`TreeEvent`](#wire-treeevent)
- [`_IdxPayload`](#wire-_idxpayload)
- [`_IndicesPayload`](#wire-_indicespayload)
- [`ListViewEvent`](#wire-listviewevent)
- [`_ColPayload`](#wire-_colpayload)
- [`_DataTableScrollPayload`](#wire-_datatablescrollpayload)
- [`_ColumnResizedPayload`](#wire-_columnresizedpayload)
- [`_DataTableSelectionSetPayload`](#wire-_datatableselectionsetpayload)
- [`DataTableEvent`](#wire-datatableevent)
- [`_IndexPayload`](#wire-_indexpayload)
- [`_IdPayload`](#wire-_idpayload)
- [`TabBarEvent`](#wire-tabbarevent)
- [`StatusBarEvent`](#wire-statusbarevent)
- [`_NewOffsetPayload`](#wire-_newoffsetpayload)
- [`_EnabledPayload`](#wire-_enabledpayload)
- [`_TextPayload`](#wire-_textpayload)
- [`TextDisplayEvent`](#wire-textdisplayevent)
- [`ActivityBarEvent`](#wire-activitybarevent)
- [`_RowColPayload`](#wire-_rowcolpayload)
- [`_IntDeltaPayload`](#wire-_intdeltapayload)
- [`_TerminalEventDict`](#wire-_terminaleventdict)
- [`TerminalEvent`](#wire-terminalevent)
- [`_DataPointClickedPayload`](#wire-_datapointclickedpayload)
- [`_LegendClickedPayload`](#wire-_legendclickedpayload)
- [`ChartEvent`](#wire-chartevent)
- [`_ToolbarButtonClickedPayload`](#wire-_toolbarbuttonclickedpayload)
- [`ToolbarEvent`](#wire-toolbarevent)
- [`CalendarDate`](#wire-calendardate)
- [`_IdValueBoolPayload`](#wire-_idvalueboolpayload)
- [`_IdValueStrPayload`](#wire-_idvaluestrpayload)
- [`_IdValueFloatPayload`](#wire-_idvaluefloatpayload)
- [`_IdValueColorPayload`](#wire-_idvaluecolorpayload)
- [`_IdSelectedIdxPayload`](#wire-_idselectedidxpayload)
- [`_IdValueDatePayload`](#wire-_idvaluedatepayload)
- [`_FormToolbarButtonClickedPayload`](#wire-_formtoolbarbuttonclickedpayload)
- [`_IdValueF64Payload`](#wire-_idvaluef64payload)
- [`_IdDraftPayload`](#wire-_iddraftpayload)
- [`_IdIndexPayload`](#wire-_idindexpayload)
- [`_IdSelectedPayload`](#wire-_idselectedpayload)
- [`FormEvent`](#wire-formevent)
- [`_QueryChangedPayload`](#wire-_querychangedpayload)
- [`_SelectionChangedIdxPayload`](#wire-_selectionchangedidxpayload)
- [`_ItemConfirmedPayload`](#wire-_itemconfirmedpayload)
- [`_ExpandToggledPayload`](#wire-_expandtoggledpayload)
- [`PaletteEvent`](#wire-paletteevent)
- [`_HitDetailButtonBody`](#wire-_hitdetailbuttonbody)
- [`_HitDetailTreeRow`](#wire-_hitdetailtreerow)
- [`_HitDetailTreeChevron`](#wire-_hitdetailtreechevron)
- [`_HitDetailListTitle`](#wire-_hitdetaillisttitle)
- [`_HitDetailListItem`](#wire-_hitdetaillistitem)
- [`_HitDetailDataTableHeader`](#wire-_hitdetaildatatableheader)
- [`_HitDetailDataTableHeaderDivider`](#wire-_hitdetaildatatableheaderdivider)
- [`_HitDetailDataTableRow`](#wire-_hitdetaildatatablerow)
- [`_HitDetailDataTableFooter`](#wire-_hitdetaildatatablefooter)
- [`_HitDetailTabBarTab`](#wire-_hitdetailtabbartab)
- [`_HitDetailTabBarTabClose`](#wire-_hitdetailtabbartabclose)
- [`_HitDetailTabBarScrollLeft`](#wire-_hitdetailtabbarscrollleft)
- [`_HitDetailTabBarScrollRight`](#wire-_hitdetailtabbarscrollright)
- [`_HitDetailTabBarRightSegment`](#wire-_hitdetailtabbarrightsegment)
- [`_HitDetailTextDisplayLine`](#wire-_hitdetailtextdisplayline)
- [`_HitDetailTextDisplayScrollbarThumb`](#wire-_hitdetailtextdisplayscrollbarthumb)
- [`_HitDetailTextDisplayScrollbarTrackBefore`](#wire-_hitdetailtextdisplayscrollbartrackbefore)
- [`_HitDetailTextDisplayScrollbarTrackAfter`](#wire-_hitdetailtextdisplayscrollbartrackafter)
- [`_HitDetailSplitFirstPane`](#wire-_hitdetailsplitfirstpane)
- [`_HitDetailSplitSecondPane`](#wire-_hitdetailsplitsecondpane)
- [`_HitDetailSplitDivider`](#wire-_hitdetailsplitdivider)
- [`_HitDetailSplitTreeDivider`](#wire-_hitdetailsplittreedivider)
- [`_HitDetailPanelAction`](#wire-_hitdetailpanelaction)
- [`_HitDetailPanelTitleBar`](#wire-_hitdetailpaneltitlebar)
- [`_HitDetailSidebarPanelToolbarButton`](#wire-_hitdetailsidebarpaneltoolbarbutton)
- [`_HitDetailTreeScrollbarTrackBefore`](#wire-_hitdetailtreescrollbartrackbefore)
- [`_HitDetailTreeScrollbarThumb`](#wire-_hitdetailtreescrollbarthumb)
- [`_HitDetailTreeScrollbarTrackAfter`](#wire-_hitdetailtreescrollbartrackafter)
- [`_HitDetailExtraBody`](#wire-_hitdetailextrabody)
- [`_HitDetailExtraChrome`](#wire-_hitdetailextrachrome)
- [`_HitDetailScrollBody`](#wire-_hitdetailscrollbody)
- [`_HitDetailScrollScrollbarTrackBefore`](#wire-_hitdetailscrollscrollbartrackbefore)
- [`_HitDetailScrollScrollbarThumb`](#wire-_hitdetailscrollscrollbarthumb)
- [`_HitDetailScrollScrollbarTrackAfter`](#wire-_hitdetailscrollscrollbartrackafter)
- [`HitDetail`](#wire-hitdetail)
- [`_UiKeyPressedPayload`](#wire-_uikeypressedpayload)
- [`_MouseDownPayload`](#wire-_mousedownpayload)
- [`_MouseUpPayload`](#wire-_mouseuppayload)
- [`_MouseMovedPayload`](#wire-_mousemovedpayload)
- [`_WidgetPayload`](#wire-_widgetpayload)
- [`_DoubleClickPayload`](#wire-_doubleclickpayload)
- [`_UiScrollPayload`](#wire-_uiscrollpayload)
- [`_WindowResizedPayload`](#wire-_windowresizedpayload)
- [`_WindowStateChangedPayload`](#wire-_windowstatechangedpayload)
- [`_FilesDroppedPayload`](#wire-_filesdroppedpayload)
- [`_OpenRequestedPayload`](#wire-_openrequestedpayload)
- [`_PowerStateChangedPayload`](#wire-_powerstatechangedpayload)
- [`_ScrollOffsetChangedPayload`](#wire-_scrolloffsetchangedpayload)
- [`_TextSelectionChangedPayload`](#wire-_textselectionchangedpayload)
- [`_SplitDividerDraggedPayload`](#wire-_splitdividerdraggedpayload)
- [`_TrayClickedPayload`](#wire-_trayclickedpayload)
- [`_NotificationActivatedPayload`](#wire-_notificationactivatedpayload)
- [`_UiEventDict`](#wire-_uieventdict)
- [`UiEvent`](#wire-uievent)

<!-- /toc -->

<a id="wire-widgetid"></a>
#### `WidgetId`

A widget's own string label.

Alias for `string`.

<a id="wire-treepath"></a>
#### `TreePath`

Route from the tree root, as child indices.

Alias for `number[]`.

<a id="wire-acceleratorid"></a>
#### `AcceleratorId`

Alias for `string`.

<a id="wire-globalshortcutid"></a>
#### `GlobalShortcutId`

Alias for `number`.

<a id="wire-color"></a>
#### `Color`

8-bit RGBA.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `r` | `number` | yes |  |
| `g` | `number` | yes |  |
| `b` | `number` | yes |  |
| `a` | `number` | yes |  |

<a id="wire-styledspan"></a>
#### `StyledSpan`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |
| `fg` | `Color \| null` | yes |  |
| `bg` | `Color \| null` | yes |  |
| `bold` | `boolean` | no |  |
| `italic` | `boolean` | no |  |
| `underline` | `boolean` | no |  |

<a id="wire-styledtext"></a>
#### `StyledText`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `spans` | `StyledSpan[]` | yes |  |

<a id="wire-iconglyph"></a>
#### `IconGlyph`

The glyph form of `Icon` -- also the only shape `Icon::serialize` ever emits.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `glyph` | `string` | yes | Nerd Font glyph, painted verbatim. |
| `fallback` | `string` | yes | ASCII/basic-Unicode fallback. |
| `color` | `Color \| null` | no |  |

<a id="wire-iconcodicon"></a>
#### `IconCodicon`

The codicon form of `Icon` -- lets an app pass a bundled icon name instead of hand-encoding its glyph codepoint.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `codicon` | `string` | yes | Bundled codicon name. |
| `fallback` | `string` | yes | ASCII/basic-Unicode fallback. |
| `color` | `Color \| null` | no |  |

<a id="wire-icon"></a>
#### `Icon`

An icon reference: either `{"glyph", "fallback", "color"?}` or `{"codicon", "fallback", "color"?}` -- see `Icon`'s own doc in `quadraui/src/types.rs`.

Alias for `IconGlyph | IconCodicon`.

<a id="wire-badge"></a>
#### `Badge`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |
| `fg` | `Color \| null` | yes |  |
| `bg` | `Color \| null` | yes |  |

<a id="wire-decoration"></a>
#### `Decoration`

Alias for `"Normal" | "Header" | "Muted" | "Error" | "Warning" | "Modified"`.

<a id="wire-selectionmode"></a>
#### `SelectionMode`

Alias for `"None" | "Single" | "Multi"`.

<a id="wire-treestyle"></a>
#### `TreeStyle`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `indent` | `number` | yes |  |
| `show_chevrons` | `boolean` | yes |  |
| `chevron_expanded` | `string` | yes |  |
| `chevron_collapsed` | `string` | yes |  |
| `row_height` | `number \| null` | no |  |
| `bordered` | `boolean` | no |  |

<a id="wire-modifiers"></a>
#### `Modifiers`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `shift` | `boolean` | no |  |
| `ctrl` | `boolean` | no |  |
| `alt` | `boolean` | no |  |
| `cmd` | `boolean` | no |  |

<a id="wire-point"></a>
#### `Point`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `x` | `number` | yes |  |
| `y` | `number` | yes |  |

<a id="wire-scrolldelta"></a>
#### `ScrollDelta`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `x` | `number` | yes |  |
| `y` | `number` | yes |  |

<a id="wire-rect"></a>
#### `Rect`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `x` | `number` | yes |  |
| `y` | `number` | yes |  |
| `width` | `number` | yes |  |
| `height` | `number` | yes |  |

<a id="wire-buttonmask"></a>
#### `ButtonMask`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `left` | `boolean` | no |  |
| `right` | `boolean` | no |  |
| `middle` | `boolean` | no |  |

<a id="wire-viewport"></a>
#### `Viewport`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `width` | `number` | yes |  |
| `height` | `number` | yes |  |
| `scale` | `number` | yes |  |

<a id="wire-systemtheme"></a>
#### `SystemTheme`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `dark` | `boolean` | yes |  |
| `accent` | `Color \| null` | yes |  |
| `high_contrast` | `boolean` | yes |  |
| `reduce_motion` | `boolean` | no |  |

<a id="wire-powereventkind"></a>
#### `PowerEventKind`

Alias for `"Suspend" | "Resume" | "OnBattery" | "OnAc" | "LockScreen" | "UnlockScreen"`.

<a id="wire-_namedkeyf"></a>
#### `_NamedKeyF`

Function key 1-24.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `F` | `number` | yes |  |

<a id="wire-namedkey"></a>
#### `NamedKey`

Alias for `("Escape" | "Tab" | "BackTab" | "Enter" | "Backspace" | "Delete" | "Insert" | "Home" | "End" | "PageUp" | "PageDown" | "Up" | "Down" | "Left" | "Right" | "CapsLock" | "NumLock" | "ScrollLock" | "Menu") | _NamedKeyF`.

<a id="wire-key"></a>
#### `Key`

A raw keyboard key: `{"Char": "a"}` or `{"Named": "Up"}`/`{"Named": {"F": 2}}`.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Char` | `string` |  |
| `Named` | `NamedKey` |  |

<a id="wire-_mousebuttonother"></a>
#### `_MouseButtonOther`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Other` | `number` | yes |  |

<a id="wire-mousebutton"></a>
#### `MouseButton`

Alias for `("Left" | "Right" | "Middle" | "X1" | "X2") | _MouseButtonOther`.

<a id="wire-backendnativeevent"></a>
#### `BackendNativeEvent`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `backend` | `string` | yes |  |
| `kind` | `string` | yes |  |
| `payload` | `string` | yes |  |

<a id="wire-statusbarsegment"></a>
#### `StatusBarSegment`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |
| `fg` | `Color` | yes |  |
| `bg` | `Color` | yes |  |
| `bold` | `boolean` | no |  |
| `action_id` | `WidgetId \| null` | no |  |

<a id="wire-statusbardescriptor"></a>
#### `StatusBarDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `left_segments` | `StatusBarSegment[]` | yes |  |
| `right_segments` | `StatusBarSegment[]` | yes |  |

<a id="wire-buttonvariant"></a>
#### `ButtonVariant`

Alias for `"Primary" | "Secondary" | "Icon" | "Danger" | "Warning"`.

<a id="wire-buttoniconposition"></a>
#### `ButtonIconPosition`

Alias for `"Leading" | "Trailing"`.

<a id="wire-buttondescriptor"></a>
#### `ButtonDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |
| `variant` | `ButtonVariant` | no |  |
| `disabled` | `boolean` | no |  |
| `focused` | `boolean` | no |  |
| `tooltip` | `string \| null` | no |  |
| `icon` | `Icon \| null` | no |  |
| `icon_position` | `ButtonIconPosition` | no |  |
| `icon_gap` | `number` | no |  |

<a id="wire-buttonspecdescriptor"></a>
#### `ButtonSpecDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `label` | `string` | yes |  |
| `variant` | `ButtonVariant` | no |  |
| `icon` | `Icon \| null` | no |  |
| `icon_position` | `ButtonIconPosition` | no |  |
| `icon_gap` | `number` | no |  |
| `tooltip` | `string \| null` | no |  |
| `disabled` | `boolean` | no |  |

<a id="wire-textdisplaylinedescriptor"></a>
#### `TextDisplayLineDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `spans` | `StyledSpan[]` | yes |  |
| `decoration` | `Decoration` | no |  |
| `timestamp` | `string \| null` | no |  |

<a id="wire-textdisplaydescriptor"></a>
#### `TextDisplayDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `lines` | `TextDisplayLineDescriptor[]` | yes |  |
| `scroll_offset` | `number` | no |  |
| `auto_scroll` | `boolean` | no |  |
| `max_lines` | `number` | no |  |
| `has_focus` | `boolean` | no |  |
| `title` | `StyledText \| null` | no |  |
| `show_scrollbar` | `boolean` | no |  |

<a id="wire-listitemdescriptor"></a>
#### `ListItemDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `StyledText` | yes |  |
| `icon` | `Icon \| null` | no |  |
| `detail` | `StyledText \| null` | no |  |
| `decoration` | `Decoration` | no |  |

<a id="wire-listviewdescriptor"></a>
#### `ListViewDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `title` | `StyledText \| null` | no |  |
| `items` | `ListItemDescriptor[]` | yes |  |
| `selected_idx` | `number` | yes |  |
| `scroll_offset` | `number` | no |  |
| `has_focus` | `boolean` | no |  |
| `bordered` | `boolean` | no |  |
| `h_scroll` | `number` | no |  |
| `max_content_width` | `number \| null` | no |  |
| `show_v_scrollbar` | `boolean` | no |  |
| `scroll_px` | `number` | no | binding-core extension key, lifted out by `scroll_px_from_value` -- not a `ListView` field. |

<a id="wire-treeroweditstatedescriptor"></a>
#### `TreeRowEditStateDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |
| `cursor` | `number` | yes |  |
| `selection_anchor` | `number \| null` | yes |  |
| `placeholder` | `string \| null` | no |  |

<a id="wire-treerowdescriptor"></a>
#### `TreeRowDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `TreePath` | yes |  |
| `indent` | `number` | yes |  |
| `icon` | `Icon \| null` | yes |  |
| `text` | `StyledText` | yes |  |
| `badge` | `Badge \| null` | yes |  |
| `is_expanded` | `boolean \| null` | yes |  |
| `decoration` | `Decoration` | no |  |
| `edit` | `TreeRowEditStateDescriptor \| null` | no |  |

<a id="wire-treeviewdescriptor"></a>
#### `TreeViewDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `rows` | `TreeRowDescriptor[]` | yes |  |
| `selection_mode` | `SelectionMode` | yes |  |
| `selected_path` | `TreePath \| null` | yes |  |
| `scroll_offset` | `number` | no |  |
| `style` | `TreeStyle` | yes |  |
| `has_focus` | `boolean` | no |  |
| `scroll_px` | `number` | no |  |

<a id="wire-columnalign"></a>
#### `ColumnAlign`

Alias for `"Left" | "Center" | "Right"`.

<a id="wire-sortdirection"></a>
#### `SortDirection`

Alias for `"Ascending" | "Descending"`.

<a id="wire-_columnwidthcontent"></a>
#### `_ColumnWidthContent`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `min` | `number` | yes |  |
| `max` | `number` | yes |  |

<a id="wire-columnwidth"></a>
#### `ColumnWidth`

Column sizing strategy, e.g. `{"Flex": 2.0}` -- the deprecated `"Fixed"` variant is omitted here; use `FixedCells`/`FixedPx` instead.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `FixedCells` | `number` |  |
| `FixedPx` | `number` |  |
| `Flex` | `number` |  |
| `Content` | `_ColumnWidthContent` |  |

<a id="wire-columndescriptor"></a>
#### `ColumnDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `title` | `string` | yes |  |
| `width` | `ColumnWidth` | no |  |
| `align` | `ColumnAlign` | no |  |

<a id="wire-datarowdescriptor"></a>
#### `DataRowDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `cells` | `StyledText[]` | yes |  |
| `decoration` | `Decoration` | no |  |

<a id="wire-datatabledescriptor"></a>
#### `DataTableDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `columns` | `ColumnDescriptor[]` | yes |  |
| `rows` | `DataRowDescriptor[]` | yes |  |
| `selected_idx` | `number \| null` | no |  |
| `scroll_offset` | `number` | no |  |
| `sort` | `[number, SortDirection] \| null` | no |  |
| `has_focus` | `boolean` | no |  |
| `show_scrollbar` | `boolean` | no |  |
| `min_total_width` | `number \| null` | no |  |
| `h_scroll` | `number` | no |  |
| `column_overrides` | `(number \| null)[]` | no |  |
| `footer` | `DataRowDescriptor \| null` | no |  |
| `scroll_px` | `number` | no |  |

<a id="wire-tabitemdescriptor"></a>
#### `TabItemDescriptor`

The raw `tab_bar` leaf's own tab shape -- distinct from a `"tabs"` layout node's tab entry (`LayoutTabEntryNode`), which uses `"title"`/`"closable"` instead of `"label"`/`"is_closable"`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `label` | `string` | yes |  |
| `is_active` | `boolean` | no |  |
| `is_dirty` | `boolean` | no |  |
| `is_preview` | `boolean` | no |  |
| `is_closable` | `boolean` | no |  |

<a id="wire-tabbarsegmentdescriptor"></a>
#### `TabBarSegmentDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |
| `width_cells` | `number` | yes |  |
| `id` | `WidgetId \| null` | no |  |
| `is_active` | `boolean` | no |  |

<a id="wire-tabbardescriptor"></a>
#### `TabBarDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `tabs` | `TabItemDescriptor[]` | yes |  |
| `scroll_offset` | `number` | no |  |
| `right_segments` | `TabBarSegmentDescriptor[]` | no |  |
| `active_accent` | `Color \| null` | no |  |
| `show_tab_close` | `boolean` | no |  |
| `compact` | `boolean` | no |  |

<a id="wire-_toolbarbuttonaction"></a>
#### `_ToolbarButtonAction`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |
| `icon` | `string \| null` | no |  |
| `key_hint` | `string \| null` | no |  |
| `enabled` | `boolean` | no |  |
| `is_active` | `boolean` | no |  |
| `tooltip` | `string` | no |  |
| `spec` | `ButtonSpecDescriptor \| null` | no |  |

<a id="wire-_toolbarbuttonactionwrapper"></a>
#### `_ToolbarButtonActionWrapper`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Action` | `_ToolbarButtonAction` | yes |  |

<a id="wire-_toolbarbuttonlabel"></a>
#### `_ToolbarButtonLabel`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |
| `fg` | `Color \| null` | no |  |

<a id="wire-_toolbarbuttonlabelwrapper"></a>
#### `_ToolbarButtonLabelWrapper`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Label` | `_ToolbarButtonLabel` | yes |  |

<a id="wire-toolbarbuttondescriptor"></a>
#### `ToolbarButtonDescriptor`

One `Toolbar::buttons` entry: `"Separator"`/`"Spacer"` (bare strings), `{"Action": {...}}`, or `{"Label": {...}}`.

Alias for `("Separator" | "Spacer") | _ToolbarButtonActionWrapper | _ToolbarButtonLabelWrapper`.

<a id="wire-toolbardescriptor"></a>
#### `ToolbarDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `buttons` | `ToolbarButtonDescriptor[]` | yes |  |
| `bg` | `Color \| null` | no |  |
| `focused_index` | `number \| null` | no |  |

<a id="wire-panelactiondescriptor"></a>
#### `PanelActionDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `icon` | `string` | yes |  |
| `tooltip` | `string` | no |  |
| `is_active` | `boolean` | no |  |

<a id="wire-paddingaxis"></a>
#### `PaddingAxis`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `x` | `number` | no |  |
| `y` | `number` | no |  |

<a id="wire-paddingsides"></a>
#### `PaddingSides`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `top` | `number` | no |  |
| `right` | `number` | no |  |
| `bottom` | `number` | no |  |
| `left` | `number` | no |  |

<a id="wire-paddingvalue"></a>
#### `PaddingValue`

A `"padding"` spacing value: one number (every side), `{"x", "y"}`, or `{"top", "right", "bottom", "left"}` -- never both shapes at once.

Alias for `number | PaddingAxis | PaddingSides`.

<a id="wire-layoutrownode"></a>
#### `LayoutRowNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `row` | `LayoutNode[]` | yes |  |
| `padding` | `PaddingValue` | no |  |
| `margin` | `number` | no |  |
| `gap` | `number` | no |  |
| `justify` | `"start" \| "center" \| "end" \| "space_between"` | no |  |
| `align` | `"start" \| "center" \| "end" \| "stretch"` | no |  |
| `wire_version` | `number` | no |  |
| `size` | `SectionSize` | no |  |
| `min` | `number` | no |  |
| `max` | `number` | no |  |

<a id="wire-layoutcolumnnode"></a>
#### `LayoutColumnNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `column` | `LayoutNode[]` | yes |  |
| `padding` | `PaddingValue` | no |  |
| `margin` | `number` | no |  |
| `gap` | `number` | no |  |
| `justify` | `"start" \| "center" \| "end" \| "space_between"` | no |  |
| `align` | `"start" \| "center" \| "end" \| "stretch"` | no |  |
| `wire_version` | `number` | no |  |
| `size` | `SectionSize` | no |  |
| `min` | `number` | no |  |
| `max` | `number` | no |  |

<a id="wire-layoutsplitpayload"></a>
#### `LayoutSplitPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `orientation` | `"horizontal" \| "vertical"` | yes |  |
| `ratio` | `number` | yes |  |
| `first_min` | `number` | no |  |
| `second_min` | `number` | no |  |
| `first` | `LayoutNode` | yes |  |
| `second` | `LayoutNode` | yes |  |

<a id="wire-layoutsplitnode"></a>
#### `LayoutSplitNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `split` | `LayoutSplitPayload` | yes |  |
| `padding` | `PaddingValue` | no |  |
| `margin` | `number` | no |  |
| `gap` | `number` | no |  |
| `wire_version` | `number` | no |  |
| `size` | `SectionSize` | no |  |
| `min` | `number` | no |  |
| `max` | `number` | no |  |

<a id="wire-layouttabentrynode"></a>
#### `LayoutTabEntryNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `title` | `string` | yes |  |
| `content` | `LayoutNode` | yes |  |
| `closable` | `boolean` | no |  |
| `is_dirty` | `boolean` | no |  |
| `is_preview` | `boolean` | no |  |

<a id="wire-layouttabspayload"></a>
#### `LayoutTabsPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `tabs` | `LayoutTabEntryNode[]` | yes |  |
| `selected` | `number` | no |  |
| `closable` | `boolean` | no |  |
| `scroll_offset` | `number` | no |  |
| `focused_index` | `number \| null` | no |  |
| `right_segments` | `TabBarSegmentDescriptor[]` | no |  |
| `active_accent` | `Color \| null` | no |  |
| `compact` | `boolean` | no |  |

<a id="wire-layouttabsnode"></a>
#### `LayoutTabsNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `tabs` | `LayoutTabsPayload` | yes |  |
| `padding` | `PaddingValue` | no |  |
| `margin` | `number` | no |  |
| `wire_version` | `number` | no |  |
| `size` | `SectionSize` | no |  |
| `min` | `number` | no |  |
| `max` | `number` | no |  |

<a id="wire-layoutpanelpayload"></a>
#### `LayoutPanelPayload`

The deprecated `panel` node's own chrome dict -- prefer `LayoutSidebarPanelNode`.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `title` | `StyledText \| null` | no |  |
| `actions` | `PanelActionDescriptor[]` | no |  |
| `accent` | `Color \| null` | no |  |
| `collapsed` | `boolean` | no |  |
| `content` | `LayoutNode` | yes |  |

<a id="wire-layoutpanelnode"></a>
#### `LayoutPanelNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `panel` | `LayoutPanelPayload` | yes |  |
| `padding` | `PaddingValue` | no |  |
| `margin` | `number` | no |  |
| `wire_version` | `number` | no |  |
| `size` | `SectionSize` | no |  |
| `min` | `number` | no |  |
| `max` | `number` | no |  |

<a id="wire-layoutsidebarpanelpayload"></a>
#### `LayoutSidebarPanelPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `toolbar` | `ToolbarDescriptor \| null` | no |  |
| `toolbar_height` | `number \| null` | no |  |
| `content` | `LayoutNode` | yes |  |

<a id="wire-layoutsidebarpanelnode"></a>
#### `LayoutSidebarPanelNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `sidebar_panel` | `LayoutSidebarPanelPayload` | yes |  |
| `wire_version` | `number` | no |  |
| `size` | `SectionSize` | no |  |
| `min` | `number` | no |  |
| `max` | `number` | no |  |

<a id="wire-layoutscrollpayload"></a>
#### `LayoutScrollPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `axis` | `"vertical" \| "horizontal"` | no |  |
| `offset` | `number` | no |  |
| `content` | `LayoutNode` | yes |  |

<a id="wire-layoutscrollnode"></a>
#### `LayoutScrollNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `scroll` | `LayoutScrollPayload` | yes |  |
| `padding` | `PaddingValue` | no |  |
| `margin` | `number` | no |  |
| `wire_version` | `number` | no |  |
| `size` | `SectionSize` | no |  |
| `min` | `number` | no |  |
| `max` | `number` | no |  |

<a id="wire-_splittreeleaf"></a>
#### `_SplitTreeLeaf`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `leaf` | `LayoutNode` | yes |  |

<a id="wire-splittreesplitpayload"></a>
#### `SplitTreeSplitPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `orientation` | `"horizontal" \| "vertical"` | yes |  |
| `ratio` | `number` | yes |  |
| `first` | `SplitTreeRecursiveNode` | yes |  |
| `second` | `SplitTreeRecursiveNode` | yes |  |

<a id="wire-_splittreesplitwrapper"></a>
#### `_SplitTreeSplitWrapper`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `split` | `SplitTreeSplitPayload` | yes |  |

<a id="wire-splittreerecursivenode"></a>
#### `SplitTreeRecursiveNode`

Alias for `_SplitTreeLeaf | _SplitTreeSplitWrapper`.

<a id="wire-layoutsplittreepayload"></a>
#### `LayoutSplitTreePayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `root` | `SplitTreeRecursiveNode` | yes |  |

<a id="wire-layoutsplittreenode"></a>
#### `LayoutSplitTreeNode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `split_tree` | `LayoutSplitTreePayload` | yes |  |
| `wire_version` | `number` | no |  |
| `size` | `SectionSize` | no |  |
| `min` | `number` | no |  |
| `max` | `number` | no |  |

<a id="wire-progressbardescriptor"></a>
#### `ProgressBarDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | no |  |
| `value` | `number \| null` | no |  |
| `frame_idx` | `number` | no |  |
| `cancellable` | `boolean` | no |  |
| `accent` | `Color \| null` | no |  |

<a id="wire-spinnerdescriptor"></a>
#### `SpinnerDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | no |  |
| `frame_idx` | `number` | no |  |
| `accent` | `Color \| null` | no |  |

<a id="wire-_imagesourcebytes"></a>
#### `_ImageSourceBytes`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Bytes` | `number[]` | yes |  |

<a id="wire-_imagesourcepath"></a>
#### `_ImageSourcePath`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Path` | `string` | yes |  |

<a id="wire-imagesource"></a>
#### `ImageSource`

`{"Bytes": [u8, ...]}` or `{"Path": "..."}` -- a `PathBuf` serializes as a plain string.

Alias for `_ImageSourceBytes | _ImageSourcePath`.

<a id="wire-imagefit"></a>
#### `ImageFit`

Alias for `"Contain" | "Cover" | "Fill" | "None"`.

<a id="wire-imagedescriptor"></a>
#### `ImageDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `source` | `ImageSource` | yes |  |
| `intrinsic_size` | `[number, number] \| null` | yes |  |
| `fit` | `ImageFit` | yes |  |
| `fallback_text` | `string` | yes |  |

<a id="wire-textinputdescriptor"></a>
#### `TextInputDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `lines` | `string[]` | yes |  |
| `cursor_line` | `number` | no |  |
| `cursor_col` | `number` | no |  |
| `placeholder` | `string \| null` | no |  |
| `scroll_offset` | `number` | no |  |
| `scroll_col` | `number` | no |  |
| `has_focus` | `boolean` | no |  |

<a id="wire-numberinputdescriptor"></a>
#### `NumberInputDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `number` | no |  |
| `min` | `number` | yes |  |
| `max` | `number` | yes |  |
| `step` | `number` | no |  |
| `precision` | `number` | no |  |
| `draft` | `string \| null` | no |  |
| `disabled` | `boolean` | no |  |
| `focused` | `boolean` | no |  |

<a id="wire-comboboxdescriptor"></a>
#### `ComboBoxDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `options` | `string[]` | yes |  |
| `text` | `string` | no |  |
| `committed` | `string` | no |  |
| `open` | `boolean` | no |  |
| `highlighted` | `number \| null` | no |  |
| `disabled` | `boolean` | no |  |
| `focused` | `boolean` | no |  |

<a id="wire-switchdescriptor"></a>
#### `SwitchDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `boolean` | no |  |
| `disabled` | `boolean` | no |  |
| `focused` | `boolean` | no |  |

<a id="wire-scrollaxis"></a>
#### `ScrollAxis`

Alias for `"Vertical" | "Horizontal"`.

<a id="wire-scrollbardescriptor"></a>
#### `ScrollbarDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `axis` | `ScrollAxis` | yes |  |
| `track` | `Rect` | yes |  |
| `thumb_start` | `number` | yes |  |
| `thumb_len` | `number` | yes |  |
| `hovered` | `boolean` | no |  |
| `dragging` | `boolean` | no |  |

<a id="wire-scrollviewdescriptor"></a>
#### `ScrollViewDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `axis` | `ScrollAxis` | no |  |
| `viewport` | `Rect` | yes |  |
| `content_len` | `number` | yes |  |
| `scroll_offset` | `number` | no |  |
| `focused` | `boolean` | no |  |

<a id="wire-commandlinedescriptor"></a>
#### `CommandLineDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `text` | `string` | yes |  |
| `cursor_offset` | `number \| null` | no |  |
| `right_align` | `boolean` | no |  |

<a id="wire-commandcenterdescriptor"></a>
#### `CommandCenterDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `back_enabled` | `boolean` | yes |  |
| `forward_enabled` | `boolean` | yes |  |
| `search_label` | `string` | no |  |

<a id="wire-radiooptiondescriptor"></a>
#### `RadioOptionDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |
| `disabled` | `boolean` | no |  |

<a id="wire-radiogroupdescriptor"></a>
#### `RadioGroupDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `options` | `RadioOptionDescriptor[]` | yes |  |
| `selected` | `number \| null` | no |  |
| `focused` | `boolean` | no |  |

<a id="wire-messagerowdescriptor"></a>
#### `MessageRowDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |
| `fg` | `Color` | yes |  |
| `indent` | `number` | no |  |
| `spans` | `StyledSpan[]` | no |  |
| `scale` | `number` | no |  |

<a id="wire-messagelistdescriptor"></a>
#### `MessageListDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `rows` | `MessageRowDescriptor[]` | yes |  |
| `scroll_top` | `number` | no |  |

<a id="wire-weekday"></a>
#### `Weekday`

Alias for `"Sunday" | "Monday" | "Tuesday" | "Wednesday" | "Thursday" | "Friday" | "Saturday"`.

<a id="wire-dateformat"></a>
#### `DateFormat`

Alias for `"Iso8601" | "UsSlash" | "EuSlash"`.

<a id="wire-datelocale"></a>
#### `DateLocale`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `first_weekday` | `Weekday` | yes |  |
| `format` | `DateFormat` | yes |  |

<a id="wire-timeofday"></a>
#### `TimeOfDay`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `hour` | `number` | yes |  |
| `minute` | `number` | yes |  |

<a id="wire-datepickerdescriptor"></a>
#### `DatePickerDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `visible_month` | `CalendarDate` | yes |  |
| `selected` | `CalendarDate \| null` | no |  |
| `focused` | `CalendarDate` | yes |  |
| `locale` | `DateLocale` | yes |  |
| `time` | `TimeOfDay \| null` | no |  |
| `has_focus` | `boolean` | no |  |

<a id="wire-chartkind"></a>
#### `ChartKind`

Alias for `"Sparkline" | "Line" | "Bar" | "BarGrouped"`.

<a id="wire-seriesdescriptor"></a>
#### `SeriesDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `label` | `string` | yes |  |
| `data` | `number[]` | yes |  |
| `color` | `Color \| null` | no |  |
| `fill` | `boolean` | no |  |

<a id="wire-chartdescriptor"></a>
#### `ChartDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `kind` | `ChartKind` | yes |  |
| `series` | `SeriesDescriptor[]` | yes |  |
| `x_label` | `string \| null` | no |  |
| `y_label` | `string \| null` | no |  |
| `y_range` | `[number, number] \| null` | no |  |
| `x_range` | `[number, number] \| null` | no |  |
| `show_legend` | `boolean` | no |  |
| `y_ticks` | `number \| null` | no |  |
| `x_ticks` | `number \| null` | no |  |
| `show_grid` | `boolean` | no |  |

<a id="wire-stagestatus"></a>
#### `StageStatus`

Alias for `"Pending" | "Active" | "Done" | "Failed" | "Skipped" | "Stale"`.

<a id="wire-pipelinestagedescriptor"></a>
#### `PipelineStageDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `label` | `string` | yes |  |
| `status` | `StageStatus` | yes |  |
| `action` | `string \| null` | no |  |

<a id="wire-pipelineviewdescriptor"></a>
#### `PipelineViewDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `stages` | `PipelineStageDescriptor[]` | yes |  |
| `focused_stage` | `number \| null` | no |  |

<a id="wire-badgestatus"></a>
#### `BadgeStatus`

Alias for `"Pending" | "Running" | "Passed" | "Warning" | "Blocked"`.

<a id="wire-cardbadgedescriptor"></a>
#### `CardBadgeDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `label` | `string` | yes |  |
| `status` | `BadgeStatus` | yes |  |

<a id="wire-boardcarddescriptor"></a>
#### `BoardCardDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `title` | `string` | yes |  |
| `labels` | `string[]` | no |  |
| `badges` | `CardBadgeDescriptor[]` | no |  |
| `hint` | `string \| null` | no |  |

<a id="wire-boardcolumndescriptor"></a>
#### `BoardColumnDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `title` | `string` | yes |  |
| `cards` | `BoardCardDescriptor[]` | yes |  |
| `scroll_offset` | `number` | no |  |

<a id="wire-boardmodeldescriptor"></a>
#### `BoardModelDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `columns` | `BoardColumnDescriptor[]` | yes |  |
| `selected_card_id` | `WidgetId \| null` | no |  |
| `col_scroll_offset` | `number` | no |  |

<a id="wire-minimaplinedescriptor"></a>
#### `MinimapLineDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |
| `line_idx` | `number` | yes |  |

<a id="wire-minimapspandescriptor"></a>
#### `MinimapSpanDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `line_idx` | `number` | yes |  |
| `start_col` | `number` | yes |  |
| `end_col` | `number` | yes |  |
| `color` | `Color` | yes |  |

<a id="wire-minimapdescriptor"></a>
#### `MinimapDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `lines` | `MinimapLineDescriptor[]` | yes |  |
| `syntax_spans` | `MinimapSpanDescriptor[]` | yes |  |
| `visible_row_start` | `number` | yes |  |
| `visible_row_count` | `number` | yes |  |
| `total_buffer_lines` | `number` | yes |  |

<a id="wire-activityitemdescriptor"></a>
#### `ActivityItemDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `icon` | `Icon` | yes |  |
| `tooltip` | `string` | no |  |
| `is_active` | `boolean` | no |  |
| `is_keyboard_selected` | `boolean` | no |  |

<a id="wire-activitybardescriptor"></a>
#### `ActivityBarDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `top_items` | `ActivityItemDescriptor[]` | yes |  |
| `bottom_items` | `ActivityItemDescriptor[]` | no |  |
| `active_accent` | `Color \| null` | no |  |
| `selection_bg` | `Color \| null` | no |  |
| `is_keyboard_focused` | `boolean` | no |  |

<a id="wire-palettemode"></a>
#### `PaletteMode`

Alias for `"List" | "Input"`.

<a id="wire-paletteitemdescriptor"></a>
#### `PaletteItemDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `StyledText` | yes |  |
| `detail` | `StyledText \| null` | no |  |
| `icon` | `Icon \| null` | no |  |
| `match_positions` | `number[]` | no |  |
| `depth` | `number` | no |  |
| `expandable` | `boolean` | no |  |
| `expanded` | `boolean` | no |  |

<a id="wire-palettepreviewdescriptor"></a>
#### `PalettePreviewDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `lines` | `StyledText[]` | yes |  |
| `title` | `string \| null` | no |  |
| `scroll_offset` | `number` | no |  |
| `highlight_line` | `number \| null` | no |  |

<a id="wire-palettedescriptor"></a>
#### `PaletteDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `title` | `string` | yes |  |
| `query` | `string` | yes |  |
| `query_cursor` | `number` | no |  |
| `items` | `PaletteItemDescriptor[]` | yes |  |
| `selected_idx` | `number` | yes |  |
| `scroll_offset` | `number` | no |  |
| `total_count` | `number` | no |  |
| `has_focus` | `boolean` | no |  |
| `show_query` | `boolean` | no |  |
| `create_label` | `string \| null` | no |  |
| `preview` | `PalettePreviewDescriptor \| null` | no |  |
| `mode` | `PaletteMode` | no |  |

<a id="wire-terminalcursorshape"></a>
#### `TerminalCursorShape`

Alias for `"Block" | "Underline" | "Bar"`.

<a id="wire-terminalcelldescriptor"></a>
#### `TerminalCellDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |
| `fg` | `Color` | yes |  |
| `bg` | `Color` | yes |  |
| `bold` | `boolean` | no |  |
| `italic` | `boolean` | no |  |
| `underline` | `boolean` | no |  |
| `dim` | `boolean` | no |  |
| `selected` | `boolean` | no |  |
| `is_cursor` | `boolean` | no |  |
| `cursor_shape` | `TerminalCursorShape` | no |  |
| `cursor_blinking` | `boolean` | no |  |
| `is_find_match` | `boolean` | no |  |
| `is_find_active` | `boolean` | no |  |

<a id="wire-terminalscrollbardescriptor"></a>
#### `TerminalScrollbarDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `total_lines` | `number` | yes |  |
| `visible_lines` | `number` | yes |  |
| `scroll_offset` | `number` | yes |  |
| `inverted` | `boolean` | no |  |
| `width` | `number \| null` | no |  |

<a id="wire-terminaldescriptor"></a>
#### `TerminalDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `cells` | `TerminalCellDescriptor[][]` | yes |  |
| `scrollbar` | `TerminalScrollbarDescriptor \| null` | no |  |

<a id="wire-canvasfontrole"></a>
#### `CanvasFontRole`

Alias for `"Chrome" | "Editor"`.

<a id="wire-_drawoprect"></a>
#### `_DrawOpRect`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `rect` | `Rect` | yes |  |
| `color` | `Color` | yes |  |

<a id="wire-_drawoproundedrect"></a>
#### `_DrawOpRoundedRect`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `rect` | `Rect` | yes |  |
| `radius` | `number` | yes |  |
| `color` | `Color` | yes |  |

<a id="wire-_drawopline"></a>
#### `_DrawOpLine`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `Point` | yes |  |
| `to` | `Point` | yes |  |
| `color` | `Color` | yes |  |
| `stroke_width` | `number` | yes |  |

<a id="wire-_drawoppath"></a>
#### `_DrawOpPath`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `points` | `Point[]` | yes |  |
| `color` | `Color` | yes |  |
| `stroke_width` | `number` | yes |  |
| `closed` | `boolean` | yes |  |

<a id="wire-_drawoptextrun"></a>
#### `_DrawOpTextRun`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `rect` | `Rect` | yes |  |
| `text` | `string` | yes |  |
| `color` | `Color` | yes |  |

<a id="wire-_drawopimage"></a>
#### `_DrawOpImage`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `rect` | `Rect` | yes |  |
| `image` | `ImageDescriptor` | yes |  |

<a id="wire-_drawoppushclip"></a>
#### `_DrawOpPushClip`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `rect` | `Rect` | yes |  |

<a id="wire-_drawoppolygon"></a>
#### `_DrawOpPolygon`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `points` | `Point[]` | yes |  |
| `color` | `Color` | yes |  |

<a id="wire-_drawopellipse"></a>
#### `_DrawOpEllipse`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `center` | `Point` | yes |  |
| `radius_x` | `number` | yes |  |
| `radius_y` | `number` | yes |  |
| `color` | `Color` | yes |  |

<a id="wire-_drawoparc"></a>
#### `_DrawOpArc`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `center` | `Point` | yes |  |
| `radius` | `number` | yes |  |
| `start_angle` | `number` | yes |  |
| `end_angle` | `number` | yes |  |
| `color` | `Color` | yes |  |
| `stroke_width` | `number` | yes |  |

<a id="wire-_drawopbezier"></a>
#### `_DrawOpBezier`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `Point` | yes |  |
| `control1` | `Point` | yes |  |
| `control2` | `Point` | yes |  |
| `to` | `Point` | yes |  |
| `color` | `Color` | yes |  |
| `stroke_width` | `number` | yes |  |

<a id="wire-_drawopdashedline"></a>
#### `_DrawOpDashedLine`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `from` | `Point` | yes |  |
| `to` | `Point` | yes |  |
| `color` | `Color` | yes |  |
| `stroke_width` | `number` | yes |  |
| `dash_len` | `number` | yes |  |
| `gap_len` | `number` | yes |  |

<a id="wire-_drawopstrokerect"></a>
#### `_DrawOpStrokeRect`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `rect` | `Rect` | yes |  |
| `color` | `Color` | yes |  |
| `stroke_width` | `number` | yes |  |

<a id="wire-_drawopstyledtextrun"></a>
#### `_DrawOpStyledTextRun`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `rect` | `Rect` | yes |  |
| `text` | `string` | yes |  |
| `color` | `Color` | yes |  |
| `role` | `CanvasFontRole` | yes |  |
| `italic` | `boolean` | yes |  |
| `size` | `number \| null` | no |  |

<a id="wire-_drawopdict"></a>
#### `_DrawOpDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Rect` | `_DrawOpRect` |  |
| `RoundedRect` | `_DrawOpRoundedRect` |  |
| `Line` | `_DrawOpLine` |  |
| `Path` | `_DrawOpPath` |  |
| `TextRun` | `_DrawOpTextRun` |  |
| `Image` | `_DrawOpImage` |  |
| `PushClip` | `_DrawOpPushClip` |  |
| `Polygon` | `_DrawOpPolygon` |  |
| `Ellipse` | `_DrawOpEllipse` |  |
| `Arc` | `_DrawOpArc` |  |
| `Bezier` | `_DrawOpBezier` |  |
| `DashedLine` | `_DrawOpDashedLine` |  |
| `StrokeRect` | `_DrawOpStrokeRect` |  |
| `StyledTextRun` | `_DrawOpStyledTextRun` |  |

<a id="wire-drawop"></a>
#### `DrawOp`

A single `Canvas` paint instruction -- `"PopClip"` (bare string, no payload) or `{"<Variant>": {...}}`.

Alias for `"PopClip" | _DrawOpDict`.

<a id="wire-canvasdescriptor"></a>
#### `CanvasDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `ops` | `DrawOp[]` | yes |  |

<a id="wire-diagnosticseverity"></a>
#### `DiagnosticSeverity`

Alias for `"Error" | "Warning" | "Information" | "Hint"`.

<a id="wire-gitlinestatus"></a>
#### `GitLineStatus`

Alias for `"Added" | "Modified" | "Deleted"`.

<a id="wire-editordiffline"></a>
#### `EditorDiffLine`

Alias for `"Same" | "Added" | "Removed" | "Padding"`.

<a id="wire-cursorshape"></a>
#### `CursorShape`

Alias for `"Block" | "Bar" | "Underline"`.

<a id="wire-selectionkind"></a>
#### `SelectionKind`

Alias for `"Char" | "Line" | "Block"`.

<a id="wire-cursorpos"></a>
#### `CursorPos`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `view_line` | `number` | yes |  |
| `col` | `number` | yes |  |

<a id="wire-editorcursor"></a>
#### `EditorCursor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `pos` | `CursorPos` | yes |  |
| `shape` | `CursorShape` | yes |  |

<a id="wire-editorselection"></a>
#### `EditorSelection`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `SelectionKind` | yes |  |
| `start_line` | `number` | yes |  |
| `start_col` | `number` | yes |  |
| `end_line` | `number` | yes |  |
| `end_col` | `number` | yes |  |

<a id="wire-editorstyle"></a>
#### `EditorStyle`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `fg` | `Color` | yes |  |
| `bg` | `Color \| null` | yes |  |
| `bold` | `boolean` | no |  |
| `italic` | `boolean` | no |  |
| `font_scale` | `number` | yes |  |

<a id="wire-editorstyledspan"></a>
#### `EditorStyledSpan`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `start_byte` | `number` | yes |  |
| `end_byte` | `number` | yes |  |
| `style` | `EditorStyle` | yes |  |

<a id="wire-diagnosticmark"></a>
#### `DiagnosticMark`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `start_col` | `number` | yes |  |
| `end_col` | `number` | yes |  |
| `severity` | `DiagnosticSeverity` | yes |  |
| `message` | `string` | yes |  |

<a id="wire-spellmark"></a>
#### `SpellMark`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `start_col` | `number` | yes |  |
| `end_col` | `number` | yes |  |

<a id="wire-editorline"></a>
#### `EditorLine`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `raw_text` | `string` | yes |  |
| `gutter_text` | `string` | yes |  |
| `spans` | `EditorStyledSpan[]` | yes |  |
| `line_idx` | `number` | yes |  |
| `is_current_line` | `boolean` | no |  |
| `is_fold_header` | `boolean` | no |  |
| `folded_line_count` | `number` | no |  |
| `git_diff` | `GitLineStatus \| null` | no |  |
| `diff_status` | `EditorDiffLine \| null` | no |  |
| `diagnostics` | `DiagnosticMark[]` | no |  |
| `spell_errors` | `SpellMark[]` | no |  |
| `is_breakpoint` | `boolean` | no |  |
| `is_conditional_bp` | `boolean` | no |  |
| `is_dap_current` | `boolean` | no |  |
| `is_wrap_continuation` | `boolean` | no |  |
| `segment_col_offset` | `number` | no |  |
| `annotation` | `string \| null` | no |  |
| `ghost_suffix` | `string \| null` | no |  |
| `is_ghost_continuation` | `boolean` | no |  |
| `indent_guides` | `number[]` | no |  |
| `colorcolumns` | `number[]` | no |  |

<a id="wire-editordescriptor"></a>
#### `EditorDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `rect` | `Rect` | yes |  |
| `lines` | `EditorLine[]` | yes |  |
| `cursor` | `EditorCursor \| null` | no |  |
| `extra_cursors` | `CursorPos[]` | no |  |
| `selection` | `EditorSelection \| null` | no |  |
| `extra_selections` | `EditorSelection[]` | no |  |
| `yank_highlight` | `EditorSelection \| null` | no |  |
| `scroll_top` | `number` | yes |  |
| `scroll_left` | `number` | no |  |
| `total_lines` | `number` | yes |  |
| `max_col` | `number` | no |  |
| `gutter_char_width` | `number` | no |  |
| `is_active` | `boolean` | yes |  |
| `show_active_bg` | `boolean` | yes |  |
| `has_git_diff` | `boolean` | yes |  |
| `has_breakpoints` | `boolean` | yes |  |
| `diagnostic_gutter` | `Record<string, DiagnosticSeverity>` | no |  |
| `code_action_lines` | `number[]` | no |  |
| `bracket_match_positions` | `[number, number][]` | no |  |
| `active_indent_col` | `number \| null` | no |  |
| `tabstop` | `number` | yes |  |
| `cursorline` | `boolean` | yes |  |
| `lightbulb_glyph` | `string` | no |  |

<a id="wire-_frclickfindinput"></a>
#### `_FrClickFindInput`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `FindInput` | `number` | yes |  |

<a id="wire-_frclickreplaceinput"></a>
#### `_FrClickReplaceInput`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `ReplaceInput` | `number` | yes |  |

<a id="wire-findreplaceclicktarget"></a>
#### `FindReplaceClickTarget`

Alias for `("Chevron" | "ToggleCase" | "ToggleWholeWord" | "ToggleRegex" | "PrevMatch" | "NextMatch" | "ToggleInSelection" | "Close" | "TogglePreserveCase" | "ReplaceCurrent" | "ReplaceAll") | _FrClickFindInput | _FrClickReplaceInput`.

<a id="wire-frhitregion"></a>
#### `FrHitRegion`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `row` | `number` | yes |  |
| `width` | `number` | yes |  |

<a id="wire-findreplacepaneldescriptor"></a>
#### `FindReplacePanelDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `query` | `string` | yes |  |
| `replacement` | `string` | no |  |
| `show_replace` | `boolean` | no |  |
| `focus` | `number` | no |  |
| `cursor` | `number` | no |  |
| `sel_anchor` | `number \| null` | no |  |
| `match_info` | `string` | no |  |
| `case_sensitive` | `boolean` | no |  |
| `whole_word` | `boolean` | no |  |
| `use_regex` | `boolean` | no |  |
| `preserve_case` | `boolean` | no |  |
| `in_selection` | `boolean` | no |  |
| `group_bounds` | `Rect` | yes |  |
| `panel_width` | `number` | no |  |
| `replace_one_glyph` | `string` | no |  |
| `replace_all_glyph` | `string` | no |  |
| `hit_regions` | `[FrHitRegion, FindReplaceClickTarget][]` | no |  |

<a id="wire-_controlwidthfixed"></a>
#### `_ControlWidthFixed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Fixed` | `number` | yes |  |

<a id="wire-controlwidth"></a>
#### `ControlWidth`

Alias for `("Fill" | "Content") | _ControlWidthFixed`.

<a id="wire-labelposition"></a>
#### `LabelPosition`

Alias for `"Left" | "Top" | "Inline" | "Hidden"`.

<a id="wire-labelalign"></a>
#### `LabelAlign`

Alias for `"Start" | "Center" | "End"`.

<a id="wire-rowalign"></a>
#### `RowAlign`

Alias for `"Start" | "Center" | "End" | "Stretch"`.

<a id="wire-formspacing"></a>
#### `FormSpacing`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `padding` | `number` | no |  |
| `item_gap` | `number \| null` | no |  |
| `row_align` | `RowAlign` | no |  |

<a id="wire-_validationstateerror"></a>
#### `_ValidationStateError`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Error` | `string` | yes |  |

<a id="wire-_validationstatewarning"></a>
#### `_ValidationStateWarning`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Warning` | `string` | yes |  |

<a id="wire-validationstate"></a>
#### `ValidationState`

`{"Error": "..."}` or `{"Warning": "..."}`.

Alias for `_ValidationStateError | _ValidationStateWarning`.

<a id="wire-togglegroupitem"></a>
#### `ToggleGroupItem`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |
| `value` | `boolean` | yes |  |

<a id="wire-buttonrowitem"></a>
#### `ButtonRowItem`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |
| `disabled` | `boolean` | no |  |
| `icon` | `Icon \| null` | no |  |
| `spec` | `ButtonSpecDescriptor \| null` | no |  |

<a id="wire-_fieldkindtextinput"></a>
#### `_FieldKindTextInput`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `string` | yes |  |
| `placeholder` | `string` | no |  |
| `cursor` | `number \| null` | no |  |
| `selection_anchor` | `number \| null` | no |  |

<a id="wire-_fieldkindreadonly"></a>
#### `_FieldKindReadOnly`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `StyledText` | yes |  |

<a id="wire-_fieldkindslider"></a>
#### `_FieldKindSlider`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `number` | yes |  |
| `min` | `number` | yes |  |
| `max` | `number` | yes |  |
| `step` | `number` | no |  |

<a id="wire-_fieldkindcolorpicker"></a>
#### `_FieldKindColorPicker`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `Color` | yes |  |

<a id="wire-_fieldkinddropdown"></a>
#### `_FieldKindDropdown`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `options` | `StyledText[]` | yes |  |
| `selected_idx` | `number` | yes |  |

<a id="wire-_fieldkindtextarea"></a>
#### `_FieldKindTextArea`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `string` | yes |  |
| `placeholder` | `string` | no |  |
| `cursor` | `number \| null` | no |  |
| `visible_rows` | `number` | no |  |

<a id="wire-_fieldkindpasswordinput"></a>
#### `_FieldKindPasswordInput`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `string` | yes |  |
| `placeholder` | `string` | no |  |
| `cursor` | `number \| null` | no |  |
| `mask_char` | `string` | no |  |

<a id="wire-_fieldkindsegmentedcontrol"></a>
#### `_FieldKindSegmentedControl`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `options` | `string[]` | yes |  |
| `selected_idx` | `number` | yes |  |

<a id="wire-_fieldkindtogglegroup"></a>
#### `_FieldKindToggleGroup`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `toggles` | `ToggleGroupItem[]` | yes |  |

<a id="wire-_fieldkindbuttonrow"></a>
#### `_FieldKindButtonRow`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `buttons` | `ButtonRowItem[]` | yes |  |

<a id="wire-_fieldkindtoolbar"></a>
#### `_FieldKindToolbar`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Toolbar` | `ToolbarDescriptor` | yes |  |

<a id="wire-_fieldkinddatepicker"></a>
#### `_FieldKindDatePicker`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `DatePicker` | `DatePickerDescriptor` | yes |  |

<a id="wire-_fieldkindcombobox"></a>
#### `_FieldKindComboBox`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `ComboBox` | `ComboBoxDescriptor` | yes |  |

<a id="wire-_fieldkindnumberinput"></a>
#### `_FieldKindNumberInput`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `NumberInput` | `NumberInputDescriptor` | yes |  |

<a id="wire-_fieldkindradiogroup"></a>
#### `_FieldKindRadioGroup`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `RadioGroup` | `RadioGroupDescriptor` | yes |  |

<a id="wire-_fieldkindtoggle"></a>
#### `_FieldKindToggle`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `boolean` | yes |  |

<a id="wire-_fieldkinddict"></a>
#### `_FieldKindDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Toggle` | `_FieldKindToggle` |  |
| `TextInput` | `_FieldKindTextInput` |  |
| `ReadOnly` | `_FieldKindReadOnly` |  |
| `Slider` | `_FieldKindSlider` |  |
| `ColorPicker` | `_FieldKindColorPicker` |  |
| `Dropdown` | `_FieldKindDropdown` |  |
| `TextArea` | `_FieldKindTextArea` |  |
| `PasswordInput` | `_FieldKindPasswordInput` |  |
| `SegmentedControl` | `_FieldKindSegmentedControl` |  |
| `ToggleGroup` | `_FieldKindToggleGroup` |  |
| `ButtonRow` | `_FieldKindButtonRow` |  |

<a id="wire-fieldkind"></a>
#### `FieldKind`

A `FormField`'s kind -- `"Label"`/`"Button"` (bare strings, no payload), a tagged variant from `_FieldKindDict`, or one of the embedded-primitive wrappers (`{"Toolbar": ...}`, `{"DatePicker": ...}`, `{"ComboBox": ...}`, `{"NumberInput": ...}`, `{"RadioGroup": ...}`).

Alias for `("Label" | "Button") | _FieldKindDict | _FieldKindToolbar | _FieldKindDatePicker | _FieldKindComboBox | _FieldKindNumberInput | _FieldKindRadioGroup`.

<a id="wire-formfielddescriptor"></a>
#### `FormFieldDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `StyledText` | yes |  |
| `kind` | `FieldKind` | yes |  |
| `hint` | `StyledText` | no |  |
| `disabled` | `boolean` | no |  |
| `validation` | `ValidationState \| null` | no |  |
| `label_position` | `LabelPosition \| null` | no |  |
| `label_align` | `LabelAlign \| null` | no |  |
| `label_width` | `number \| null` | no |  |
| `tab_index` | `number \| null` | no |  |
| `control_width` | `ControlWidth \| null` | no |  |
| `min_control_width` | `number \| null` | no |  |
| `row_align` | `RowAlign \| null` | no |  |

<a id="wire-formdescriptor"></a>
#### `FormDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `fields` | `FormFieldDescriptor[]` | yes |  |
| `focused_field` | `WidgetId \| null` | yes |  |
| `scroll_offset` | `number` | no |  |
| `has_focus` | `boolean` | no |  |
| `label_position` | `LabelPosition` | no |  |
| `label_align` | `LabelAlign` | no |  |
| `label_width` | `number \| null` | no |  |
| `control_width` | `ControlWidth` | no |  |
| `min_control_width` | `number \| null` | no |  |
| `spacing` | `FormSpacing` | no |  |

<a id="wire-msvaxis"></a>
#### `MsvAxis`

Alias for `"Vertical" | "Horizontal"`.

<a id="wire-scrollmode"></a>
#### `ScrollMode`

Alias for `"PerSection" | "WholePanel"`.

<a id="wire-_sectionsizefixed"></a>
#### `_SectionSizeFixed`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Fixed` | `number` | yes |  |

<a id="wire-_sectionsizepercent"></a>
#### `_SectionSizePercent`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Percent` | `number` | yes |  |

<a id="wire-_sectionsizeweight"></a>
#### `_SectionSizeWeight`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Weight` | `number` | yes |  |

<a id="wire-sectionsize"></a>
#### `SectionSize`

Alias for `("Content" | "EqualShare") | _SectionSizeFixed | _SectionSizePercent | _SectionSizeWeight`.

<a id="wire-headeraction"></a>
#### `HeaderAction`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `icon` | `Icon` | yes |  |
| `tooltip` | `string \| null` | yes |  |
| `enabled` | `boolean` | no |  |

<a id="wire-sectionheader"></a>
#### `SectionHeader`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `icon` | `Icon \| null` | yes |  |
| `title` | `StyledText` | yes |  |
| `badge` | `StyledText \| null` | yes |  |
| `actions` | `HeaderAction[]` | no |  |
| `show_chevron` | `boolean` | no |  |

<a id="wire-emptybody"></a>
#### `EmptyBody`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `icon` | `Icon \| null` | yes |  |
| `text` | `StyledText` | yes |  |
| `hint` | `StyledText \| null` | yes |  |
| `action` | `HeaderAction \| null` | yes |  |

<a id="wire-inlineinput"></a>
#### `InlineInput`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `text` | `string` | yes |  |
| `caret` | `number` | no |  |
| `placeholder` | `string \| null` | yes |  |
| `has_focus` | `boolean` | no |  |

<a id="wire-_sectionauxinput"></a>
#### `_SectionAuxInput`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Input` | `InlineInput` | yes |  |

<a id="wire-_sectionauxtoolbar"></a>
#### `_SectionAuxToolbar`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Toolbar` | `HeaderAction[]` | yes |  |

<a id="wire-_sectionauxsearch"></a>
#### `_SectionAuxSearch`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Search` | `InlineInput` | yes |  |

<a id="wire-_sectionauxcustom"></a>
#### `_SectionAuxCustom`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Custom` | `WidgetId` | yes |  |

<a id="wire-sectionaux"></a>
#### `SectionAux`

Alias for `_SectionAuxInput | _SectionAuxToolbar | _SectionAuxSearch | _SectionAuxCustom`.

<a id="wire-_sectionbodytree"></a>
#### `_SectionBodyTree`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Tree` | `TreeViewDescriptor` | yes |  |

<a id="wire-_sectionbodylist"></a>
#### `_SectionBodyList`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `List` | `ListViewDescriptor` | yes |  |

<a id="wire-_sectionbodyform"></a>
#### `_SectionBodyForm`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Form` | `FormDescriptor` | yes |  |

<a id="wire-_sectionbodyterminal"></a>
#### `_SectionBodyTerminal`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Terminal` | `TerminalDescriptor` | yes |  |

<a id="wire-_sectionbodymessagelist"></a>
#### `_SectionBodyMessageList`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `MessageList` | `MessageListDescriptor` | yes |  |

<a id="wire-_sectionbodytext"></a>
#### `_SectionBodyText`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Text` | `StyledText[]` | yes |  |

<a id="wire-_sectionbodychart"></a>
#### `_SectionBodyChart`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Chart` | `ChartDescriptor` | yes |  |

<a id="wire-_sectionbodyempty"></a>
#### `_SectionBodyEmpty`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Empty` | `EmptyBody` | yes |  |

<a id="wire-_sectionbodycustom"></a>
#### `_SectionBodyCustom`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Custom` | `WidgetId` | yes |  |

<a id="wire-sectionbody"></a>
#### `SectionBody`

Alias for `_SectionBodyTree | _SectionBodyList | _SectionBodyForm | _SectionBodyTerminal | _SectionBodyMessageList | _SectionBodyText | _SectionBodyChart | _SectionBodyEmpty | _SectionBodyCustom`.

<a id="wire-section"></a>
#### `Section`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `header` | `SectionHeader` | yes |  |
| `body` | `SectionBody` | yes |  |
| `aux` | `SectionAux \| null` | no |  |
| `size` | `SectionSize` | yes |  |
| `collapsed` | `boolean` | no |  |
| `min_size` | `number \| null` | no |  |
| `max_size` | `number \| null` | no |  |

<a id="wire-multisectionviewdescriptor"></a>
#### `MultiSectionViewDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `sections` | `Section[]` | yes |  |
| `active_section` | `number \| null` | no |  |
| `axis` | `MsvAxis` | yes |  |
| `allow_resize` | `boolean` | no |  |
| `allow_collapse` | `boolean` | no |  |
| `scroll_mode` | `ScrollMode` | yes |  |
| `has_focus` | `boolean` | no |  |
| `panel_scroll` | `number` | no |  |

<a id="wire-_keybindingliteral"></a>
#### `_KeyBindingLiteral`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Literal` | `string` | yes |  |

<a id="wire-keybinding"></a>
#### `KeyBinding`

A universal accelerator name (bare string) or `{"Literal": "..."}` for an app-specific binding.

Alias for `("Save" | "Open" | "New" | "Close" | "Copy" | "Cut" | "Paste" | "Undo" | "Redo" | "SelectAll" | "Find" | "Replace" | "Quit") | _KeyBindingLiteral`.

<a id="wire-_acceleratorscopewidget"></a>
#### `_AcceleratorScopeWidget`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Widget` | `WidgetId` | yes |  |

<a id="wire-_acceleratorscopemode"></a>
#### `_AcceleratorScopeMode`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `Mode` | `string` | yes |  |

<a id="wire-acceleratorscope"></a>
#### `AcceleratorScope`

Alias for `"Global" | _AcceleratorScopeWidget | _AcceleratorScopeMode`.

<a id="wire-accelerator"></a>
#### `Accelerator`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `AcceleratorId` | yes |  |
| `binding` | `KeyBinding` | yes |  |
| `scope` | `AcceleratorScope` | yes |  |
| `label` | `string \| null` | yes |  |

<a id="wire-contextmenuitem"></a>
#### `ContextMenuItem`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId \| null` | no |  |
| `label` | `StyledText` | yes |  |
| `detail` | `StyledText \| null` | no |  |
| `disabled` | `boolean` | no |  |
| `key_equivalent` | `Accelerator \| null` | no |  |
| `checked` | `boolean \| null` | no |  |
| `icon` | `Icon \| null` | no |  |
| `submenu` | `ContextMenuItem[] \| null` | no |  |

<a id="wire-menubaritemdescriptor"></a>
#### `MenuBarItemDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `label` | `string` | yes |  |
| `disabled` | `boolean` | no |  |
| `submenu` | `ContextMenuItem[] \| null` | no |  |

<a id="wire-menubardescriptor"></a>
#### `MenuBarDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `items` | `MenuBarItemDescriptor[]` | yes |  |
| `open_item` | `number \| null` | no |  |
| `focused_item` | `number \| null` | no |  |

<a id="wire-diffrowkind"></a>
#### `DiffRowKind`

Alias for `"Same" | "Changed" | "Removed" | "Added"`.

<a id="wire-diffrow"></a>
#### `DiffRow`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `left` | `string \| null` | yes |  |
| `right` | `string \| null` | yes |  |
| `kind` | `DiffRowKind` | yes |  |

<a id="wire-diffhunk"></a>
#### `DiffHunk`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `left_start` | `number` | yes |  |
| `right_start` | `number` | yes |  |
| `rows` | `DiffRow[]` | yes |  |

<a id="wire-diffmode"></a>
#### `DiffMode`

Alias for `"SideBySide" | "Unified"`.

<a id="wire-diffeditability"></a>
#### `DiffEditability`

Alias for `"ReadOnly" | "RightEditable"`.

<a id="wire-diffpane"></a>
#### `DiffPane`

Alias for `"Left" | "Right"`.

<a id="wire-diffviewdescriptor"></a>
#### `DiffViewDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `left` | `string` | yes |  |
| `right` | `string` | yes |  |
| `left_label` | `string \| null` | yes |  |
| `right_label` | `string \| null` | yes |  |
| `hunks` | `DiffHunk[]` | yes |  |
| `mode` | `DiffMode` | yes |  |
| `editability` | `DiffEditability` | yes |  |
| `scroll_offset` | `number` | yes |  |
| `focused_pane` | `DiffPane` | yes |  |
| `has_focus` | `boolean` | yes |  |

<a id="wire-layoutleafnode"></a>
#### `LayoutLeafNode`

One `{"id", "<kind>": <descriptor>}` leaf -- exactly one kind key is present. Every registered kind (the six core kinds, `status_bar`, and every `EXTRA_PRIMITIVE_KINDS` additive kind) is fully typed.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `wire_version` | `number` | no |  |
| `button` | `ButtonDescriptor` | no |  |
| `text_display` | `TextDisplayDescriptor` | no |  |
| `list` | `ListViewDescriptor` | no |  |
| `tree` | `TreeViewDescriptor` | no |  |
| `data_table` | `DataTableDescriptor` | no |  |
| `tab_bar` | `TabBarDescriptor` | no |  |
| `status_bar` | `StatusBarDescriptor` | no |  |
| `progress` | `ProgressBarDescriptor` | no |  |
| `spinner` | `SpinnerDescriptor` | no |  |
| `image` | `ImageDescriptor` | no |  |
| `form` | `FormDescriptor` | no |  |
| `text_input` | `TextInputDescriptor` | no |  |
| `number_input` | `NumberInputDescriptor` | no |  |
| `combo_box` | `ComboBoxDescriptor` | no |  |
| `date_picker` | `DatePickerDescriptor` | no |  |
| `radio_group` | `RadioGroupDescriptor` | no |  |
| `switch` | `SwitchDescriptor` | no |  |
| `chart` | `ChartDescriptor` | no |  |
| `message_list` | `MessageListDescriptor` | no |  |
| `board` | `BoardModelDescriptor` | no |  |
| `pipeline_view` | `PipelineViewDescriptor` | no |  |
| `minimap` | `MinimapDescriptor` | no |  |
| `multi_section_view` | `MultiSectionViewDescriptor` | no |  |
| `diff_view` | `DiffViewDescriptor` | no |  |
| `activity_bar` | `ActivityBarDescriptor` | no |  |
| `toolbar` | `ToolbarDescriptor` | no |  |
| `menu_bar` | `MenuBarDescriptor` | no |  |
| `command_line` | `CommandLineDescriptor` | no |  |
| `command_center` | `CommandCenterDescriptor` | no |  |
| `palette` | `PaletteDescriptor` | no |  |
| `scrollbar` | `ScrollbarDescriptor` | no |  |
| `editor` | `EditorDescriptor` | no |  |
| `terminal` | `TerminalDescriptor` | no |  |
| `canvas` | `CanvasDescriptor` | no |  |
| `scroll_view` | `ScrollViewDescriptor` | no |  |
| `find_replace` | `FindReplacePanelDescriptor` | no |  |
| `size` | `SectionSize` | no |  |
| `min` | `number` | no |  |
| `max` | `number` | no |  |

<a id="wire-layoutnode"></a>
#### `LayoutNode`

One node of a render callback's returned layout tree.

Alias for `LayoutRowNode | LayoutColumnNode | LayoutSplitNode | LayoutTabsNode | LayoutPanelNode | LayoutSidebarPanelNode | LayoutSplitTreeNode | LayoutScrollNode | LayoutLeafNode`.

<a id="wire-renderframedict"></a>
#### `RenderFrameDict`

The pre-layout-tree flat per-slot shape a render callback may still return instead of a `LayoutNode` tree. Every registered kind is fully typed.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `wire_version` | `number` | no |  |
| `button` | `ButtonDescriptor \| null` | no |  |
| `text_display` | `TextDisplayDescriptor \| null` | no |  |
| `list` | `ListViewDescriptor \| null` | no |  |
| `tree` | `TreeViewDescriptor \| null` | no |  |
| `data_table` | `DataTableDescriptor \| null` | no |  |
| `tab_bar` | `TabBarDescriptor \| null` | no |  |
| `status_bar` | `StatusBarDescriptor \| null` | no |  |
| `progress` | `ProgressBarDescriptor \| null` | no |  |
| `spinner` | `SpinnerDescriptor \| null` | no |  |
| `image` | `ImageDescriptor \| null` | no |  |
| `form` | `FormDescriptor \| null` | no |  |
| `text_input` | `TextInputDescriptor \| null` | no |  |
| `number_input` | `NumberInputDescriptor \| null` | no |  |
| `combo_box` | `ComboBoxDescriptor \| null` | no |  |
| `date_picker` | `DatePickerDescriptor \| null` | no |  |
| `radio_group` | `RadioGroupDescriptor \| null` | no |  |
| `switch` | `SwitchDescriptor \| null` | no |  |
| `chart` | `ChartDescriptor \| null` | no |  |
| `message_list` | `MessageListDescriptor \| null` | no |  |
| `board` | `BoardModelDescriptor \| null` | no |  |
| `pipeline_view` | `PipelineViewDescriptor \| null` | no |  |
| `minimap` | `MinimapDescriptor \| null` | no |  |
| `multi_section_view` | `MultiSectionViewDescriptor \| null` | no |  |
| `diff_view` | `DiffViewDescriptor \| null` | no |  |
| `activity_bar` | `ActivityBarDescriptor \| null` | no |  |
| `toolbar` | `ToolbarDescriptor \| null` | no |  |
| `menu_bar` | `MenuBarDescriptor \| null` | no |  |
| `command_line` | `CommandLineDescriptor \| null` | no |  |
| `command_center` | `CommandCenterDescriptor \| null` | no |  |
| `palette` | `PaletteDescriptor \| null` | no |  |
| `scrollbar` | `ScrollbarDescriptor \| null` | no |  |
| `editor` | `EditorDescriptor \| null` | no |  |
| `terminal` | `TerminalDescriptor \| null` | no |  |
| `canvas` | `CanvasDescriptor \| null` | no |  |
| `scroll_view` | `ScrollViewDescriptor \| null` | no |  |
| `find_replace` | `FindReplacePanelDescriptor \| null` | no |  |

<a id="wire-renderoutput"></a>
#### `RenderOutput`

A render callback's return value: a layout tree or the flat shape.

Alias for `LayoutNode | RenderFrameDict`.

<a id="wire-paneldefinitiondescriptor"></a>
#### `PanelDefinitionDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `icon` | `string` | no |  |
| `tooltip` | `string` | no |  |
| `title` | `string` | no |  |

<a id="wire-bottompaneltabdescriptor"></a>
#### `BottomPanelTabDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `string` | yes |  |
| `label` | `string` | no |  |
| `closable` | `boolean` | no |  |
| `badge` | `string \| null` | no |  |

<a id="wire-bottompanelconfigdescriptor"></a>
#### `BottomPanelConfigDescriptor`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `tabs` | `BottomPanelTabDescriptor[]` | no |  |
| `active_tab_id` | `string` | no |  |
| `maximised` | `boolean` | no |  |
| `height_fraction` | `number` | no |  |

<a id="wire-shellconfigdescriptor"></a>
#### `ShellConfigDescriptor`

`run_shell`'s `config` dict.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `title` | `string` | yes |  |
| `panels` | `PanelDefinitionDescriptor[]` | no |  |
| `bottom_items` | `PanelDefinitionDescriptor[]` | no |  |
| `position` | `"left" \| "right"` | no |  |
| `sidebar_visible` | `boolean` | no |  |
| `default_sidebar_width` | `number` | no |  |
| `min_sidebar_width` | `number` | no |  |
| `max_sidebar_width` | `number` | no |  |
| `activity_bar_width` | `number` | no |  |
| `has_title_bar` | `boolean` | no |  |
| `title_bar_height_lh` | `number` | no |  |
| `has_status_bar` | `boolean` | no |  |
| `has_command_line` | `boolean` | no |  |
| `has_bottom_panel` | `boolean` | no |  |
| `bottom_panel_height_lh` | `number` | no |  |
| `bottom_panel` | `BottomPanelConfigDescriptor` | no |  |

<a id="wire-_keypressedstrpayload"></a>
#### `_KeyPressedStrPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `string` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_treepathpayload"></a>
#### `_TreePathPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `TreePath` | yes |  |

<a id="wire-_treerowclickedpayload"></a>
#### `_TreeRowClickedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `TreePath` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_treeselectionsetpayload"></a>
#### `_TreeSelectionSetPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `paths` | `TreePath[]` | yes |  |

<a id="wire-_treerowrenamechangedpayload"></a>
#### `_TreeRowRenameChangedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `TreePath` | yes |  |
| `text` | `string` | yes |  |
| `cursor` | `number` | yes |  |
| `selection_anchor` | `number \| null` | yes |  |

<a id="wire-_treerowrenamecommittedpayload"></a>
#### `_TreeRowRenameCommittedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `path` | `TreePath` | yes |  |
| `text` | `string` | yes |  |

<a id="wire-treeevent"></a>
#### `TreeEvent`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `RowClicked` | `_TreeRowClickedPayload` |  |
| `RowDoubleClicked` | `_TreePathPayload` |  |
| `RowToggleExpand` | `_TreePathPayload` |  |
| `SelectionChanged` | `_TreePathPayload` |  |
| `KeyPressed` | `_KeyPressedStrPayload` |  |
| `SelectionSet` | `_TreeSelectionSetPayload` |  |
| `RowRenameChanged` | `_TreeRowRenameChangedPayload` |  |
| `RowRenameCommitted` | `_TreeRowRenameCommittedPayload` |  |
| `RowRenameCancelled` | `_TreePathPayload` |  |

<a id="wire-_idxpayload"></a>
#### `_IdxPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_indicespayload"></a>
#### `_IndicesPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `indices` | `number[]` | yes |  |

<a id="wire-listviewevent"></a>
#### `ListViewEvent`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `SelectionChanged` | `_IdxPayload` |  |
| `ItemActivated` | `_IdxPayload` |  |
| `ItemDoubleClicked` | `_IdxPayload` |  |
| `KeyPressed` | `_KeyPressedStrPayload` |  |
| `SelectionSet` | `_IndicesPayload` |  |

<a id="wire-_colpayload"></a>
#### `_ColPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |

<a id="wire-_datatablescrollpayload"></a>
#### `_DataTableScrollPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `delta` | `number` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_columnresizedpayload"></a>
#### `_ColumnResizedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `col` | `number` | yes |  |
| `width` | `number` | yes |  |

<a id="wire-_datatableselectionsetpayload"></a>
#### `_DataTableSelectionSetPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `indices` | `number[]` | yes |  |

<a id="wire-datatableevent"></a>
#### `DataTableEvent`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `HeaderClicked` | `_ColPayload` |  |
| `RowActivated` | `_IdxPayload` |  |
| `RowSelected` | `_IdxPayload` |  |
| `RowDoubleClicked` | `_IdxPayload` |  |
| `Scroll` | `_DataTableScrollPayload` |  |
| `ColumnResized` | `_ColumnResizedPayload` |  |
| `SelectionSet` | `_DataTableSelectionSetPayload` |  |

<a id="wire-_indexpayload"></a>
#### `_IndexPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `index` | `number` | yes |  |

<a id="wire-_idpayload"></a>
#### `_IdPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |

<a id="wire-tabbarevent"></a>
#### `TabBarEvent`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `TabActivated` | `_IndexPayload` |  |
| `TabClosed` | `_IndexPayload` |  |
| `ButtonClicked` | `_IdPayload` |  |
| `KeyPressed` | `_KeyPressedStrPayload` |  |
| `FocusMoved` | `_IndexPayload` |  |

<a id="wire-statusbarevent"></a>
#### `StatusBarEvent`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `SegmentClicked` | `_IdPayload` |  |
| `KeyPressed` | `_KeyPressedStrPayload` |  |

<a id="wire-_newoffsetpayload"></a>
#### `_NewOffsetPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `new_offset` | `number` | yes |  |

<a id="wire-_enabledpayload"></a>
#### `_EnabledPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `enabled` | `boolean` | yes |  |

<a id="wire-_textpayload"></a>
#### `_TextPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `text` | `string` | yes |  |

<a id="wire-textdisplayevent"></a>
#### `TextDisplayEvent`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Scrolled` | `_NewOffsetPayload` |  |
| `AutoScrollToggled` | `_EnabledPayload` |  |
| `Copied` | `_TextPayload` |  |
| `KeyPressed` | `_KeyPressedStrPayload` |  |

<a id="wire-activitybarevent"></a>
#### `ActivityBarEvent`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `ItemClicked` | `_IdPayload` |  |
| `KeyPressed` | `_KeyPressedStrPayload` |  |

<a id="wire-_rowcolpayload"></a>
#### `_RowColPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `row` | `number` | yes |  |
| `col` | `number` | yes |  |

<a id="wire-_intdeltapayload"></a>
#### `_IntDeltaPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `delta` | `number` | yes |  |

<a id="wire-_terminaleventdict"></a>
#### `_TerminalEventDict`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `KeyPressed` | `_KeyPressedStrPayload` |  |
| `SelectStart` | `_RowColPayload` |  |
| `SelectExtend` | `_RowColPayload` |  |
| `Scroll` | `_IntDeltaPayload` |  |

<a id="wire-terminalevent"></a>
#### `TerminalEvent`

Mouse release has no payload -- bare `"SelectEnd"`, unlike every other variant.

Alias for `"SelectEnd" | _TerminalEventDict`.

<a id="wire-_datapointclickedpayload"></a>
#### `_DataPointClickedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `series_idx` | `number` | yes |  |
| `data_idx` | `number` | yes |  |

<a id="wire-_legendclickedpayload"></a>
#### `_LegendClickedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `series_idx` | `number` | yes |  |

<a id="wire-chartevent"></a>
#### `ChartEvent`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Clicked` | `_IdPayload` |  |
| `DataPointClicked` | `_DataPointClickedPayload` |  |
| `LegendClicked` | `_LegendClickedPayload` |  |

<a id="wire-_toolbarbuttonclickedpayload"></a>
#### `_ToolbarButtonClickedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `is_active` | `boolean` | yes |  |

<a id="wire-toolbarevent"></a>
#### `ToolbarEvent`

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `ButtonClicked` | `_ToolbarButtonClickedPayload` |  |

<a id="wire-calendardate"></a>
#### `CalendarDate`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `year` | `number` | yes |  |
| `month` | `number` | yes |  |
| `day` | `number` | yes |  |

<a id="wire-_idvalueboolpayload"></a>
#### `_IdValueBoolPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `boolean` | yes |  |

<a id="wire-_idvaluestrpayload"></a>
#### `_IdValueStrPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `string` | yes |  |

<a id="wire-_idvaluefloatpayload"></a>
#### `_IdValueFloatPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `number` | yes |  |

<a id="wire-_idvaluecolorpayload"></a>
#### `_IdValueColorPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `Color` | yes |  |

<a id="wire-_idselectedidxpayload"></a>
#### `_IdSelectedIdxPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `selected_idx` | `number` | yes |  |

<a id="wire-_idvaluedatepayload"></a>
#### `_IdValueDatePayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `CalendarDate` | yes |  |

<a id="wire-_formtoolbarbuttonclickedpayload"></a>
#### `_FormToolbarButtonClickedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `field_id` | `WidgetId` | yes |  |
| `button_id` | `WidgetId` | yes |  |

<a id="wire-_idvaluef64payload"></a>
#### `_IdValueF64Payload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `value` | `number` | yes |  |

<a id="wire-_iddraftpayload"></a>
#### `_IdDraftPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `draft` | `string` | yes |  |

<a id="wire-_idindexpayload"></a>
#### `_IdIndexPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `index` | `number` | yes |  |

<a id="wire-_idselectedpayload"></a>
#### `_IdSelectedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `id` | `WidgetId` | yes |  |
| `selected` | `number` | yes |  |

<a id="wire-formevent"></a>
#### `FormEvent`

Events a `Form` emits back to the app -- see `quadraui/src/primitives/form.rs`'s `FormEvent`.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `ToggleChanged` | `_IdValueBoolPayload` |  |
| `TextInputChanged` | `_IdValueStrPayload` |  |
| `TextInputCommitted` | `_IdValueStrPayload` |  |
| `SliderChanged` | `_IdValueFloatPayload` |  |
| `ColorChanged` | `_IdValueColorPayload` |  |
| `DropdownChanged` | `_IdSelectedIdxPayload` |  |
| `SegmentedControlChanged` | `_IdSelectedIdxPayload` |  |
| `DateChanged` | `_IdValueDatePayload` |  |
| `FocusChanged` | `_IdPayload` |  |
| `ButtonClicked` | `_IdPayload` |  |
| `ToolbarButtonClicked` | `_FormToolbarButtonClickedPayload` |  |
| `KeyPressed` | `_KeyPressedStrPayload` |  |
| `ComboBoxTextChanged` | `_IdValueStrPayload` |  |
| `ComboBoxHighlightChanged` | `_IdIndexPayload` |  |
| `ComboBoxCommitted` | `_IdValueStrPayload` |  |
| `ComboBoxCancelled` | `_IdPayload` |  |
| `NumberChanged` | `_IdValueF64Payload` |  |
| `NumberDraftChanged` | `_IdDraftPayload` |  |
| `NumberCommitted` | `_IdValueF64Payload` |  |
| `NumberCancelled` | `_IdPayload` |  |
| `RadioChanged` | `_IdSelectedPayload` |  |

<a id="wire-_querychangedpayload"></a>
#### `_QueryChangedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `value` | `string` | yes |  |

<a id="wire-_selectionchangedidxpayload"></a>
#### `_SelectionChangedIdxPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_itemconfirmedpayload"></a>
#### `_ItemConfirmedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |

<a id="wire-_expandtoggledpayload"></a>
#### `_ExpandToggledPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `idx` | `number` | yes |  |
| `expanded` | `boolean` | yes |  |

<a id="wire-paletteevent"></a>
#### `PaletteEvent`

Events a `Palette` emits back to the app -- see `quadraui/src/primitives/palette.rs`'s `PaletteEvent`. `Closed` has no payload -- bare `"Closed"`, like `TerminalEvent`'s `"SelectEnd"`.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `QueryChanged` | `_QueryChangedPayload` |  |
| `SelectionChanged` | `_SelectionChangedIdxPayload` |  |
| `ItemConfirmed` | `_ItemConfirmedPayload` |  |
| `ExpandToggled` | `_ExpandToggledPayload` |  |
| `KeyPressed` | `_KeyPressedStrPayload` |  |

<a id="wire-_hitdetailbuttonbody"></a>
#### `_HitDetailButtonBody`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"button_body"` | yes |  |

<a id="wire-_hitdetailtreerow"></a>
#### `_HitDetailTreeRow`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_row"` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_hitdetailtreechevron"></a>
#### `_HitDetailTreeChevron`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_chevron"` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_hitdetaillisttitle"></a>
#### `_HitDetailListTitle`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"list_title"` | yes |  |

<a id="wire-_hitdetaillistitem"></a>
#### `_HitDetailListItem`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"list_item"` | yes |  |
| `index` | `number` | yes |  |

<a id="wire-_hitdetaildatatableheader"></a>
#### `_HitDetailDataTableHeader`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"data_table_header"` | yes |  |
| `col` | `number` | yes |  |

<a id="wire-_hitdetaildatatableheaderdivider"></a>
#### `_HitDetailDataTableHeaderDivider`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"data_table_header_divider"` | yes |  |
| `col` | `number` | yes |  |

<a id="wire-_hitdetaildatatablerow"></a>
#### `_HitDetailDataTableRow`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"data_table_row"` | yes |  |
| `row` | `number` | yes |  |

<a id="wire-_hitdetaildatatablefooter"></a>
#### `_HitDetailDataTableFooter`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"data_table_footer"` | yes |  |

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

<a id="wire-_hitdetailtabbarrightsegment"></a>
#### `_HitDetailTabBarRightSegment`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tab_bar_right_segment"` | yes |  |

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

<a id="wire-_hitdetailtextdisplayscrollbartrackbefore"></a>
#### `_HitDetailTextDisplayScrollbarTrackBefore`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"text_display_scrollbar_track_before"` | yes |  |

<a id="wire-_hitdetailtextdisplayscrollbartrackafter"></a>
#### `_HitDetailTextDisplayScrollbarTrackAfter`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"text_display_scrollbar_track_after"` | yes |  |

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

<a id="wire-_hitdetailsplitdivider"></a>
#### `_HitDetailSplitDivider`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"split_divider"` | yes |  |

<a id="wire-_hitdetailsplittreedivider"></a>
#### `_HitDetailSplitTreeDivider`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"split_tree_divider"` | yes |  |
| `split_index` | `number` | yes |  |

<a id="wire-_hitdetailpanelaction"></a>
#### `_HitDetailPanelAction`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"panel_action"` | yes |  |

<a id="wire-_hitdetailpaneltitlebar"></a>
#### `_HitDetailPanelTitleBar`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"panel_title_bar"` | yes |  |

<a id="wire-_hitdetailsidebarpaneltoolbarbutton"></a>
#### `_HitDetailSidebarPanelToolbarButton`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"sidebar_panel_toolbar_button"` | yes |  |
| `bar` | `WidgetId` | yes |  |

<a id="wire-_hitdetailtreescrollbartrackbefore"></a>
#### `_HitDetailTreeScrollbarTrackBefore`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_scrollbar_track_before"` | yes |  |

<a id="wire-_hitdetailtreescrollbarthumb"></a>
#### `_HitDetailTreeScrollbarThumb`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_scrollbar_thumb"` | yes |  |

<a id="wire-_hitdetailtreescrollbartrackafter"></a>
#### `_HitDetailTreeScrollbarTrackAfter`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"tree_scrollbar_track_after"` | yes |  |

<a id="wire-_hitdetailextrabody"></a>
#### `_HitDetailExtraBody`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"extra_body"` | yes |  |

<a id="wire-_hitdetailextrachrome"></a>
#### `_HitDetailExtraChrome`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"extra_chrome"` | yes |  |
| `bar` | `WidgetId` | yes |  |

<a id="wire-_hitdetailscrollbody"></a>
#### `_HitDetailScrollBody`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"scroll_body"` | yes |  |

<a id="wire-_hitdetailscrollscrollbartrackbefore"></a>
#### `_HitDetailScrollScrollbarTrackBefore`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"scroll_scrollbar_track_before"` | yes |  |

<a id="wire-_hitdetailscrollscrollbarthumb"></a>
#### `_HitDetailScrollScrollbarThumb`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"scroll_scrollbar_thumb"` | yes |  |

<a id="wire-_hitdetailscrollscrollbartrackafter"></a>
#### `_HitDetailScrollScrollbarTrackAfter`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `"scroll_scrollbar_track_after"` | yes |  |

<a id="wire-hitdetail"></a>
#### `HitDetail`

The sub-region of a primitive a pointer event landed on -- see `HitDetail` in `quadraui-binding-core/src/hit.rs`. Narrow on `.kind`.

Alias for `_HitDetailButtonBody | _HitDetailTreeRow | _HitDetailTreeChevron | _HitDetailListTitle | _HitDetailListItem | _HitDetailDataTableHeader | _HitDetailDataTableHeaderDivider | _HitDetailDataTableRow | _HitDetailDataTableFooter | _HitDetailTabBarTab | _HitDetailTabBarTabClose | _HitDetailTabBarScrollLeft | _HitDetailTabBarScrollRight | _HitDetailTabBarRightSegment | _HitDetailTextDisplayLine | _HitDetailTextDisplayScrollbarThumb | _HitDetailTextDisplayScrollbarTrackBefore | _HitDetailTextDisplayScrollbarTrackAfter | _HitDetailSplitFirstPane | _HitDetailSplitSecondPane | _HitDetailSplitDivider | _HitDetailSplitTreeDivider | _HitDetailPanelAction | _HitDetailPanelTitleBar | _HitDetailSidebarPanelToolbarButton | _HitDetailTreeScrollbarTrackBefore | _HitDetailTreeScrollbarThumb | _HitDetailTreeScrollbarTrackAfter | _HitDetailExtraBody | _HitDetailExtraChrome | _HitDetailScrollBody | _HitDetailScrollScrollbarTrackBefore | _HitDetailScrollScrollbarThumb | _HitDetailScrollScrollbarTrackAfter`.

<a id="wire-_uikeypressedpayload"></a>
#### `_UiKeyPressedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `key` | `Key` | yes |  |
| `modifiers` | `Modifiers` | yes |  |
| `repeat` | `boolean` | yes |  |

<a id="wire-_mousedownpayload"></a>
#### `_MouseDownPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `widget` | `WidgetId \| null` | yes |  |
| `button` | `MouseButton` | yes |  |
| `position` | `Point` | yes |  |
| `modifiers` | `Modifiers` | yes |  |

<a id="wire-_mouseuppayload"></a>
#### `_MouseUpPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `widget` | `WidgetId \| null` | yes |  |
| `button` | `MouseButton` | yes |  |
| `position` | `Point` | yes |  |

<a id="wire-_mousemovedpayload"></a>
#### `_MouseMovedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `position` | `Point` | yes |  |
| `buttons` | `ButtonMask` | yes |  |

<a id="wire-_widgetpayload"></a>
#### `_WidgetPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `widget` | `WidgetId` | yes |  |

<a id="wire-_doubleclickpayload"></a>
#### `_DoubleClickPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `widget` | `WidgetId \| null` | yes |  |
| `position` | `Point` | yes |  |

<a id="wire-_uiscrollpayload"></a>
#### `_UiScrollPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `widget` | `WidgetId \| null` | yes |  |
| `delta` | `ScrollDelta` | yes |  |
| `position` | `Point` | yes |  |

<a id="wire-_windowresizedpayload"></a>
#### `_WindowResizedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `viewport` | `Viewport` | yes |  |

<a id="wire-_windowstatechangedpayload"></a>
#### `_WindowStateChangedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `maximized` | `boolean` | yes |  |
| `fullscreen` | `boolean` | yes |  |
| `minimized` | `boolean` | yes |  |

<a id="wire-_filesdroppedpayload"></a>
#### `_FilesDroppedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `paths` | `string[]` | yes |  |
| `position` | `Point` | yes |  |

<a id="wire-_openrequestedpayload"></a>
#### `_OpenRequestedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `urls` | `string[]` | yes |  |
| `files` | `string[]` | yes |  |

<a id="wire-_powerstatechangedpayload"></a>
#### `_PowerStateChangedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `kind` | `PowerEventKind` | yes |  |

<a id="wire-_scrolloffsetchangedpayload"></a>
#### `_ScrollOffsetChangedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `widget` | `WidgetId` | yes |  |
| `new_offset` | `number` | yes |  |

<a id="wire-_textselectionchangedpayload"></a>
#### `_TextSelectionChangedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `region` | `WidgetId` | yes |  |
| `anchor` | `Point` | yes |  |
| `focus` | `Point` | yes |  |

<a id="wire-_splitdividerdraggedpayload"></a>
#### `_SplitDividerDraggedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `tree` | `WidgetId` | yes |  |
| `split_index` | `number` | yes |  |
| `new_ratio` | `number` | yes |  |

<a id="wire-_trayclickedpayload"></a>
#### `_TrayClickedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `button` | `MouseButton` | yes |  |

<a id="wire-_notificationactivatedpayload"></a>
#### `_NotificationActivatedPayload`

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `tag` | `string \| null` | yes |  |
| `action` | `WidgetId \| null` | yes |  |

<a id="wire-_uieventdict"></a>
#### `_UiEventDict`

Everything a user (or platform) can do that an app might care about -- see `quadraui/src/event.rs`'s `UiEvent`.

Exactly one of the tags below is present on the wire (a discriminated union):

| Tag | Payload type | Description |
| --- | --- | --- |
| `Accelerator` | `[AcceleratorId, Modifiers]` |  |
| `GlobalShortcut` | `GlobalShortcutId` |  |
| `KeyPressed` | `_UiKeyPressedPayload` |  |
| `CharTyped` | `string` |  |
| `MouseDown` | `_MouseDownPayload` |  |
| `MouseUp` | `_MouseUpPayload` |  |
| `MouseMoved` | `_MouseMovedPayload` |  |
| `MouseEntered` | `_WidgetPayload` |  |
| `MouseLeft` | `_WidgetPayload` |  |
| `DoubleClick` | `_DoubleClickPayload` |  |
| `Scroll` | `_UiScrollPayload` |  |
| `WindowResized` | `_WindowResizedPayload` |  |
| `WindowFocused` | `boolean` |  |
| `DpiChanged` | `number` |  |
| `WindowStateChanged` | `_WindowStateChangedPayload` |  |
| `FilesDropped` | `_FilesDroppedPayload` |  |
| `ClipboardPaste` | `string` |  |
| `OpenRequested` | `_OpenRequestedPayload` |  |
| `TextCopied` | `string` |  |
| `FocusChanged` | `WidgetId \| null` |  |
| `SystemThemeChanged` | `SystemTheme` |  |
| `PowerStateChanged` | `_PowerStateChangedPayload` |  |
| `ScrollOffsetChanged` | `_ScrollOffsetChangedPayload` |  |
| `TextSelectionChanged` | `_TextSelectionChangedPayload` |  |
| `SplitDividerDragged` | `_SplitDividerDraggedPayload` |  |
| `MenuActivated` | `WidgetId` |  |
| `ContextMenuItemActivated` | `WidgetId` |  |
| `MessageDialogResolved` | `WidgetId` |  |
| `ToastActionClicked` | `WidgetId` |  |
| `CompletionItemActivated` | `[WidgetId, number]` |  |
| `RichTextLinkClicked` | `[WidgetId, string]` |  |
| `TrayClicked` | `_TrayClickedPayload` |  |
| `NotificationActivated` | `_NotificationActivatedPayload` |  |
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
| `Toolbar` | `[WidgetId, ToolbarEvent]` |  |
| `BackendNative` | `BackendNativeEvent` |  |
| `User` | `unknown` |  |

Present alongside whichever tag above is set:

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `hit` | `HitDetail` | no | The hit-tested sub-region a pointer landed on, when the binding attached one (never present on `User`). |

<a id="wire-uievent"></a>
#### `UiEvent`

`isinstance(event, str)` (Python) / `typeof event === "string"` (TS) separates the three no-payload variants from the rest before narrowing on a key.

Alias for `("WindowClose" | "DisplaysChanged" | "ContextMenuDismissed") | _UiEventDict`.

<!-- END GENERATED: scripts/gen_wire_types.py -->
