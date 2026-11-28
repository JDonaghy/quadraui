//! The view tree a [`super::View`] returns: [`Node`]s built with
//! [`column`]/[`row`] containers and leaf constructors ([`text`],
//! [`button`], [`list`], [`form`], [`status_bar`], or any widget through
//! [`widget`]).
//!
//! A node is plain data. Building one paints nothing and routes nothing;
//! [`super::Run`] lays the tree out once per frame through the core stack
//! solver ([`crate::layout::stack::stack_rects`]) and keeps the result for
//! paint and routing.
//!
//! Every leaf wraps one real widget descriptor. Its event handlers
//! (`on_click`, `on_select`, …) turn the semantic [`UiEvent`]s that widget
//! emits into the app's own message type `M`.
//!
//! # Sizes
//!
//! A child's main-axis size is a [`SectionSize`] (equal share by default,
//! like the bindings' `Column`/`Row`), clamped by an optional `min`/`max`.
//! `fixed`, `min`, `max` and the container spacing (`gap`, `padding`) are
//! surface-native pixels, the same values the bindings' wire format takes:
//! a pixel backend uses them as they are, and TUI rounds them up to whole
//! cells against a nominal 8×16 cell (see [`crate::Spacing`]).

use std::any::Any;

use crate::event::{Rect, UiEvent};
use crate::layout::stack::{
    quantise_horizontal, quantise_vertical, stack_rects, Align, EdgeInsets, Justify, Spacing,
    StackChild, StackStyle,
};
use crate::layout::NaturalSize;
use crate::primitive::{DynWidget, LayoutCtx, Size};
use crate::primitives::button::Button;
use crate::primitives::form::{FieldKind, Form, FormEvent, FormField};
use crate::primitives::list::{ListItem, ListView, ListViewEvent};
use crate::primitives::multi_section_view::SectionSize;
use crate::primitives::status_bar::{StatusBar, StatusBarSegment};
use crate::primitives::text_display::{TextDisplay, TextDisplayLine};
use crate::types::{StyledText, WidgetId};

/// A widget a [`Node`] can hold: any [`DynWidget`] that can be cloned and
/// re-addressed. Every built-in widget with an `id` field implements it;
/// a custom widget implements [`Leaf::set_id`] by assigning its own id.
pub trait Leaf: DynWidget + Clone {
    /// Sets the id the widget is addressed by (routing, focus, events).
    fn set_id(&mut self, id: WidgetId);
}

macro_rules! leaf_with_id_field {
    ($($ty:ty),* $(,)?) => {
        $(impl Leaf for $ty {
            fn set_id(&mut self, id: WidgetId) {
                self.id = id;
            }
        })*
    };
}

leaf_with_id_field!(
    crate::ActivityBar,
    crate::Button,
    crate::Canvas,
    crate::CellGrid,
    crate::Chart,
    crate::Checkbox,
    crate::ComboBox,
    crate::DatePicker,
    crate::DiffView,
    crate::Form,
    crate::ListView,
    crate::Markdown,
    crate::MessageList,
    crate::NumberInput,
    crate::PasswordInput,
    crate::PipelineView,
    crate::RadioGroup,
    crate::Select,
    crate::SidebarPanel,
    crate::Slider,
    crate::Split,
    crate::StatusBar,
    crate::Switch,
    crate::TabBar,
    crate::TextArea,
    crate::TextDisplay,
    crate::TextField,
    crate::TextInput,
    crate::Toolbar,
);

// These widgets implement `Interactive` only in a build with a backend.
#[cfg(any(
    feature = "gtk",
    feature = "tui",
    feature = "win",
    all(feature = "macos", target_os = "macos")
))]
leaf_with_id_field!(
    crate::DataTable,
    crate::IconGrid,
    crate::MultiSectionView,
    crate::TreeView,
);

/// The object-safe half of [`Leaf`] a [`Node`] stores.
pub(crate) trait ErasedLeaf: DynWidget {
    fn clone_dyn(&self) -> Box<dyn DynWidget>;
    fn set_id_dyn(&mut self, id: WidgetId);
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<W: Leaf> ErasedLeaf for W {
    fn clone_dyn(&self) -> Box<dyn DynWidget> {
        Box::new(self.clone())
    }
    fn set_id_dyn(&mut self, id: WidgetId) {
        self.set_id(id);
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Maps a semantic event to a message, given the id of the leaf it is
/// attached to.
pub(crate) type Handler<M> = Box<dyn Fn(WidgetId, &UiEvent) -> Option<M>>;

/// One node of a view tree: a leaf wrapping a widget, or a
/// [`column`]/[`row`] container of child nodes, plus the size it asks its
/// parent container for.
pub struct Node<M> {
    pub(crate) kind: Kind<M>,
    pub(crate) sizing: StackChild,
}

pub(crate) enum Kind<M> {
    Leaf {
        widget: Box<dyn ErasedLeaf>,
        /// Whether the app named this leaf with [`Node::id`]; an unnamed
        /// leaf gets a path id when the tree is laid out.
        named: bool,
        handlers: Vec<Handler<M>>,
    },
    Stack {
        horizontal: bool,
        children: Vec<Node<M>>,
        style: StackStyle,
        spacing: Spacing,
    },
}

/// A vertical stack of `children`, each given an equal share of the height
/// unless it asks for its own size: `column([text("Hi"), button("OK")])`.
pub fn column<M>(children: impl IntoIterator<Item = Node<M>>) -> Node<M> {
    stack(false, children)
}

/// A horizontal stack of `children`: [`column`] along the width.
pub fn row<M>(children: impl IntoIterator<Item = Node<M>>) -> Node<M> {
    stack(true, children)
}

fn stack<M>(horizontal: bool, children: impl IntoIterator<Item = Node<M>>) -> Node<M> {
    Node::from_kind(Kind::Stack {
        horizontal,
        children: children.into_iter().collect(),
        style: StackStyle::default(),
        spacing: Spacing::default(),
    })
}

/// Any widget as a leaf, addressed by the id it already carries.
pub fn widget<M, W: Leaf>(widget: W) -> Node<M> {
    Node::from_kind(Kind::Leaf {
        widget: Box::new(widget),
        named: true,
        handlers: Vec::new(),
    })
}

fn unnamed<M, W: Leaf>(widget: W) -> Node<M> {
    let mut node = self::widget(widget);
    if let Kind::Leaf { named, .. } = &mut node.kind {
        *named = false;
    }
    node
}

/// Plain text (a [`TextDisplay`]): one unstyled line per `\n`-separated
/// line of `text`. It is sized to its content, so a label in a [`column`]
/// takes one row, as Python's `Text` and Node's `<text>` do;
/// [`Node::weight`] gives it an equal share instead.
pub fn text<M>(text: impl Into<String>) -> Node<M> {
    let mut display = TextDisplay::new(WidgetId::new(""));
    display.lines = text
        .into()
        .split('\n')
        .map(TextDisplayLine::plain)
        .collect();
    unnamed(display).content()
}

/// A push button labelled `label`. [`Node::on_click`] gives it a message.
pub fn button<M>(label: impl Into<String>) -> Node<M> {
    unnamed(Button::new(WidgetId::new(""), label))
}

/// A list with one plain-text row per item, its first row selected.
/// [`Node::selected`] moves the selection and [`Node::on_select`] reports
/// a new one.
pub fn list<M>(items: impl IntoIterator<Item = impl Into<String>>) -> Node<M> {
    let items = items.into_iter().map(|item| ListItem {
        text: StyledText::plain(item),
        icon: None,
        detail: None,
        decoration: Default::default(),
    });
    unnamed(ListView {
        id: WidgetId::new(""),
        title: None,
        items: items.collect(),
        selected_idx: 0,
        scroll_offset: 0,
        has_focus: false,
        bordered: false,
        h_scroll: 0,
        max_content_width: None,
        show_v_scrollbar: false,
        striped: false,
    })
}

/// A status bar with `left` and `right` segment groups.
pub fn status_bar<M>(
    left: impl IntoIterator<Item = StatusBarSegment>,
    right: impl IntoIterator<Item = StatusBarSegment>,
) -> Node<M> {
    unnamed(StatusBar {
        id: WidgetId::new(""),
        left_segments: left.into_iter().collect(),
        right_segments: right.into_iter().collect(),
    })
}

/// A form of `fields`. Each [`Field`]'s handlers see the events its own
/// field reports.
///
/// Leave the form's focused field unset and [`super::Run`] keeps it, the
/// way the bindings' engine does: it follows `FormEvent::FocusChanged` and
/// clears once keyboard focus leaves the form.
pub fn form<M: 'static>(fields: impl IntoIterator<Item = Field<M>>) -> Node<M> {
    let (fields, handlers): (Vec<FormField>, Vec<Vec<FieldHandler<M>>>) = fields
        .into_iter()
        .map(|field| (field.field, field.handlers))
        .unzip();
    let form = Form {
        id: WidgetId::new(""),
        fields,
        focused_field: None,
        scroll_offset: 0,
        has_focus: false,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    };
    let handlers: Vec<FieldHandler<M>> = handlers.into_iter().flatten().collect();
    unnamed(form).on(move |id, event| match event {
        UiEvent::Form(form, event) if *form == id => handlers.iter().find_map(|h| h(event)),
        _ => None,
    })
}

type FieldHandler<M> = Box<dyn Fn(&FormEvent) -> Option<M>>;

/// One [`form`] field and the messages its events map to.
pub struct Field<M> {
    field: FormField,
    handlers: Vec<FieldHandler<M>>,
}

/// A single-line text input field holding `value`, addressed by `id`.
pub fn text_field<M>(id: &str, value: impl Into<String>) -> Field<M> {
    Field::new(FormField {
        id: WidgetId::new(id),
        label: StyledText::default(),
        kind: FieldKind::TextInput {
            value: value.into(),
            placeholder: String::new(),
            cursor: None,
            selection_anchor: None,
        },
        hint: StyledText::default(),
        disabled: false,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    })
}

impl<M> Field<M> {
    /// Any [`FormField`], for a kind [`text_field`] does not cover.
    pub fn new(field: FormField) -> Self {
        Self {
            field,
            handlers: Vec::new(),
        }
    }

    /// The label shown beside the field.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.field.label = StyledText::plain(label);
        self
    }

    /// Maps any [`FormEvent`] this field reports to a message.
    pub fn on(mut self, handler: impl Fn(&FormEvent) -> Option<M> + 'static) -> Self {
        let id = self.field.id;
        self.handlers.push(Box::new(move |event| {
            (form_event_field(event) == Some(id))
                .then(|| handler(event))
                .flatten()
        }));
        self
    }

    /// The message for each edit, given the field's new text.
    pub fn on_change(self, message: impl Fn(String) -> M + 'static) -> Self {
        self.on(move |event| match event {
            FormEvent::TextInputChanged { value, .. } => Some(message(value.clone())),
            _ => None,
        })
    }

    /// The message for Enter in the field, given its text.
    pub fn on_submit(self, message: impl Fn(String) -> M + 'static) -> Self {
        self.on(move |event| match event {
            FormEvent::TextInputCommitted { value, .. } => Some(message(value.clone())),
            _ => None,
        })
    }
}

/// The field a [`FormEvent`] is about, when it names one.
fn form_event_field(event: &FormEvent) -> Option<WidgetId> {
    match event {
        FormEvent::ToggleChanged { id, .. }
        | FormEvent::TextInputChanged { id, .. }
        | FormEvent::TextInputCommitted { id, .. }
        | FormEvent::SliderChanged { id, .. }
        | FormEvent::ColorChanged { id, .. }
        | FormEvent::DropdownChanged { id, .. }
        | FormEvent::SegmentedControlChanged { id, .. }
        | FormEvent::DateChanged { id, .. } => Some(*id),
        _ => None,
    }
}

impl<M> Node<M> {
    fn from_kind(kind: Kind<M>) -> Self {
        Self {
            kind,
            sizing: StackChild::default(),
        }
    }

    // ── Leaf ────────────────────────────────────────────────────────────

    /// Names this leaf: the id its events carry, focus and widget state
    /// are kept under, and tests find it by. A leaf left unnamed gets an
    /// id from its position in the tree. No effect on a container.
    pub fn id(mut self, id: &str) -> Self {
        if let Kind::Leaf { widget, named, .. } = &mut self.kind {
            widget.set_id_dyn(WidgetId::new(id));
            *named = true;
        }
        self
    }

    /// Maps any event to a message, given this leaf's id: the general
    /// form of [`Self::on_click`]/[`Self::on_select`]. No effect on a
    /// container.
    pub fn on(mut self, handler: impl Fn(WidgetId, &UiEvent) -> Option<M> + 'static) -> Self {
        if let Kind::Leaf { handlers, .. } = &mut self.kind {
            handlers.push(Box::new(handler));
        }
        self
    }

    /// The message for activating this button (click, Enter or Space).
    pub fn on_click(self, message: M) -> Self
    where
        M: Clone + 'static,
    {
        self.on(move |id, event| match event {
            UiEvent::ButtonActivated { widget } if *widget == id => Some(message.clone()),
            _ => None,
        })
    }

    /// The message for a new selection in this list, given its index.
    pub fn on_select(self, message: impl Fn(usize) -> M + 'static) -> Self {
        self.on(move |id, event| match event {
            UiEvent::List(list, ListViewEvent::SelectionChanged { idx }) if *list == id => {
                Some(message(*idx))
            }
            _ => None,
        })
    }

    /// Edits the leaf's widget descriptor in place when it is a `W`: any
    /// field the constructor does not take. No effect otherwise.
    pub fn with<W: 'static>(mut self, edit: impl FnOnce(&mut W)) -> Self {
        if let Kind::Leaf { widget, .. } = &mut self.kind {
            if let Some(widget) = widget.as_any_mut().downcast_mut::<W>() {
                edit(widget);
            }
        }
        self
    }

    /// Selects row `idx` of this [`list`].
    pub fn selected(self, idx: usize) -> Self {
        self.with(|list: &mut ListView| list.selected_idx = idx)
    }

    // ── Size within the parent container ────────────────────────────────

    /// The main-axis size this node asks its parent container for.
    pub fn size(mut self, size: SectionSize) -> Self {
        self.sizing.size = size;
        self
    }

    /// Exactly `px` surface-native pixels along the parent's main axis.
    pub fn fixed(self, px: u16) -> Self {
        self.size(SectionSize::Fixed(px))
    }

    /// Its own natural (measured) size along the parent's main axis.
    pub fn content(self) -> Self {
        self.size(SectionSize::Content)
    }

    /// A `weight` share of the space the fixed, content and percent
    /// siblings leave; equal shares are weight 1.
    pub fn weight(self, weight: f32) -> Self {
        self.size(SectionSize::Weight(weight))
    }

    /// `fraction` (0.0 to 1.0) of the parent's main axis.
    pub fn percent(self, fraction: f32) -> Self {
        self.size(SectionSize::Percent(fraction))
    }

    /// Never smaller than `px` surface-native pixels along the main axis.
    pub fn min(mut self, px: f32) -> Self {
        self.sizing.min = Some(px);
        self
    }

    /// Never larger than `px` surface-native pixels along the main axis.
    pub fn max(mut self, px: f32) -> Self {
        self.sizing.max = Some(px);
        self
    }

    // ── Container options ───────────────────────────────────────────────

    fn stack_mut(&mut self) -> Option<(&mut StackStyle, &mut Spacing)> {
        match &mut self.kind {
            Kind::Stack { style, spacing, .. } => Some((style, spacing)),
            Kind::Leaf { .. } => None,
        }
    }

    /// `px` surface-native pixels between adjacent children. No effect
    /// on a leaf.
    pub fn gap(mut self, px: f32) -> Self {
        if let Some((style, _)) = self.stack_mut() {
            style.gap = px;
        }
        self
    }

    /// `px` surface-native pixels of space inside each edge. No effect on
    /// a leaf.
    pub fn padding(mut self, px: f32) -> Self {
        if let Some((_, spacing)) = self.stack_mut() {
            spacing.padding = EdgeInsets::uniform(px);
        }
        self
    }

    /// Where leftover main-axis space goes. No effect on a leaf.
    pub fn justify(mut self, justify: Justify) -> Self {
        if let Some((style, _)) = self.stack_mut() {
            style.justify = justify;
        }
        self
    }

    /// How children are sized and placed across the main axis. No effect
    /// on a leaf.
    pub fn align(mut self, align: Align) -> Self {
        if let Some((style, _)) = self.stack_mut() {
            style.align = align;
        }
        self
    }
}

// ── Layout ──────────────────────────────────────────────────────────────

/// The backend metrics a layout pass needs, read once per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metrics {
    pub area: Rect,
    pub char_width: f32,
    pub line_height: f32,
    pub nerd_fonts: bool,
}

impl Metrics {
    fn ctx(&self) -> LayoutCtx<'static> {
        LayoutCtx::new(self.line_height, self.char_width, self.nerd_fonts)
    }
}

/// A leaf placed by a layout pass: its final rect, widget and handlers.
pub(crate) struct Placed<M> {
    pub rect: Rect,
    pub widget: Box<dyn ErasedLeaf>,
    pub handlers: Vec<Handler<M>>,
}

impl<M> Node<M> {
    /// Lays this tree out in `metrics.area` and returns its leaves in
    /// paint order. An unnamed leaf is named by its child-index path from
    /// the root (`"view.0.1"`).
    pub(crate) fn place(self, metrics: &Metrics) -> Vec<Placed<M>> {
        let mut out = Vec::new();
        self.place_in(metrics.area, metrics, &mut String::from("view"), &mut out);
        out
    }

    fn place_in(self, area: Rect, m: &Metrics, path: &mut String, out: &mut Vec<Placed<M>>) {
        match self.kind {
            Kind::Leaf {
                mut widget,
                named,
                handlers,
            } => {
                if !named {
                    widget.set_id_dyn(WidgetId::new(path.as_str()));
                }
                out.push(Placed {
                    rect: area,
                    widget,
                    handlers,
                });
            }
            Kind::Stack {
                horizontal,
                children,
                style,
                spacing,
            } => {
                let area = spacing.inset(area, m.char_width, m.line_height);
                let sizing: Vec<StackChild> = children.iter().map(|c| c.sizing).collect();
                let ctx = m.ctx();
                let avail = Size::new(area.width, area.height);
                let rects = stack_rects(
                    area,
                    &sizing,
                    style,
                    horizontal,
                    m.char_width,
                    m.line_height,
                    &mut |i| children[i].natural(&ctx, m, avail),
                );
                for (i, (child, rect)) in children.into_iter().zip(rects).enumerate() {
                    let len = path.len();
                    path.push('.');
                    path.push_str(&i.to_string());
                    child.place_in(rect, m, path, out);
                    path.truncate(len);
                }
            }
        }
    }

    /// This node's natural size within `avail` (its parent container's
    /// area): a leaf's own measurement, or a container's children end to
    /// end along its axis (plus the gaps between them) and its largest
    /// child across it, plus the container's padding and margin.
    fn natural(&self, ctx: &LayoutCtx<'_>, m: &Metrics, avail: Size) -> NaturalSize {
        match &self.kind {
            Kind::Leaf { widget, .. } => widget.measure(ctx, avail),
            Kind::Stack {
                horizontal,
                children,
                style,
                spacing,
            } => {
                let gap = if *horizontal {
                    quantise_horizontal(style.gap, m.char_width)
                } else {
                    quantise_vertical(style.gap, m.line_height)
                };
                let content = NaturalSize::stacked(
                    children.iter().map(|child| child.natural(ctx, m, avail)),
                    *horizontal,
                    gap,
                );
                spacing.outset(content, m.char_width, m.line_height)
            }
        }
    }
}

impl<M> std::fmt::Debug for Node<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            Kind::Leaf { widget, .. } => f
                .debug_struct("Leaf")
                .field("id", &widget.id())
                .field("sizing", &self.sizing)
                .finish(),
            Kind::Stack {
                horizontal,
                children,
                ..
            } => f
                .debug_struct(if *horizontal { "Row" } else { "Column" })
                .field("children", children)
                .field("sizing", &self.sizing)
                .finish(),
        }
    }
}
