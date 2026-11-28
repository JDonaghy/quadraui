//! The retained layout-tree descriptor model a binding's render callback
//! decodes into: `LayoutNode` and its `Row`/`Column`/`Grid`/`Leaf`/`Split`/
//! `Tabs`/`SidebarPanel`/`SplitTree` shapes, each paired with the
//! `quadraui` descriptor(s) it carries. A binding owns decoding its own
//! wire format (a Python `dict`, a JS object, ...) into these types; this
//! module owns what the decoded shape looks like and how it paints
//! ([`crate::render::paint_layout`]).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use quadraui::{
    Align, CellOverflow, Color, DataTable, Justify, ListView, Rect, ScrollAxis, SectionSize,
    SidebarPanel, Spacing, Split, SplitTree, Style, TabBar, TextDisplay, Theme, TreeView,
};

use quadraui::layout::{grid_cell_rects, GridTrack};
use quadraui::primitive::DynWidget;

use crate::extra_paint::ExtraPaint;
use crate::DecodeError;

/// One node of a render callback's layout tree: a `Row`/`Column` container
/// holding child nodes, a `Leaf` wrapping one id-addressable primitive
/// descriptor, a `Split` holding exactly two named child nodes divided by a
/// draggable ratio, a `Tabs` holding a tab strip plus one content subtree
/// per tab (of which only the selected one paints), a `SidebarPanel`
/// holding one chrome descriptor plus a single content
/// subtree, a `SplitTree` holding an arbitrarily nested divider tree of
/// content subtrees, a `Scroll` holding one content subtree painted at
/// its own natural size and offset inside a clipped viewport, a `Grid`
/// holding one child subtree per cell of a two-dimensional column/row
/// track grid, or a `Styled` wrapper applying a [`StyleOverride`] to one
/// wrapped subtree.
pub enum LayoutNode {
    Row(LayoutContainer),
    Column(LayoutContainer),
    Grid(LayoutGrid),
    Leaf(LayoutLeaf),
    Split(LayoutSplit),
    Tabs(LayoutTabs),
    SidebarPanel(LayoutSidebarPanel),
    SplitTree(LayoutSplitTree),
    Scroll(LayoutScroll),
    Styled(LayoutStyled),
}

impl LayoutNode {
    /// Peels every nested [`LayoutNode::Styled`] wrapper, returning the
    /// first node underneath that isn't one. A `"style"` key is purely
    /// cosmetic — it never changes what a node structurally *is* — so
    /// every structural match against a node's own kind (a scroll's
    /// content being a `Row`/`Column` vs. an atomic leaf, a direct scroll
    /// child being a `Leaf`, a leaf's descriptor kind) must match against
    /// this, not the raw (possibly `Styled`) node, or a purely cosmetic
    /// `"style"` sibling key would silently change that structural
    /// decision — see `crate::render::scroll_content_slots`,
    /// `natural_main_length`, `is_offset_windowed_leaf`,
    /// `offset_windowed_leaf_scrolled_to_offset`, and
    /// `crate::host::resolve_layout_node`'s own `LayoutNode::Scroll` arm,
    /// each of which calls this (or [`Self::unstyled_mut`]) rather than
    /// matching the node directly. The override(s) peeled off here still
    /// apply to the subtree's own *paint* — see
    /// `crate::render::push_styled_scope`, which brackets
    /// `Backend::set_theme`/`set_style` around the same nested layers this
    /// unwraps, so a caller that needs the structural node AND the correct
    /// paint state calls both.
    pub fn unstyled(&self) -> &LayoutNode {
        let mut node = self;
        while let LayoutNode::Styled(styled) = node {
            node = &styled.node;
        }
        node
    }

    /// [`Self::unstyled`]'s mutable counterpart, for a resolve pass that
    /// needs to mutate the real node underneath any `Styled` wrapper(s)
    /// (`crate::host::resolve_layout_node`'s `LayoutNode::Scroll` arm,
    /// which writes a resolved cursor/scroll position back onto a content
    /// child).
    pub fn unstyled_mut(&mut self) -> &mut LayoutNode {
        let mut node = self;
        while let LayoutNode::Styled(styled) = node {
            node = &mut styled.node;
        }
        node
    }
}

/// A [`LayoutNode::Styled`]'s decoded shape: a [`StyleOverride`] paired
/// with the single wrapped subtree it applies to. Every node kind's own
/// `"style"` wire sibling key decodes to this wrapper around whatever node
/// the rest of that same dict already decoded to — see
/// `crate::wire::layout_node_from_value`'s own wrapping, applied uniformly
/// after the row/column/split/tabs/sidebar_panel/split_tree/scroll/
/// leaf dispatch, so every node kind gets the same `"style"` key for free
/// rather than each needing its own decode of it.
///
/// `style` is boxed for the same reason `node` already is:
/// [`StyleOverride`] covers every [`quadraui::Theme`]/[`quadraui::Style`]
/// token, so storing it inline here would make this (and therefore
/// [`LayoutNode::Styled`], and every other node kind that nests a
/// `LayoutNode` by value) the dominant size of the whole [`LayoutNode`]
/// enum. Every read site reaches straight through the box via `Deref`
/// (`styled.style.bg`, `styled.style.apply_theme(..)`), so only a
/// construction site (`LayoutStyled { style: Box::new(..), .. }`) ever
/// needs to know it's boxed.
pub struct LayoutStyled {
    pub style: Box<StyleOverride>,
    pub node: Box<LayoutNode>,
}

/// A scoped subset of the backend-wide [`quadraui::Theme`] colour tokens
/// and [`quadraui::Style`] metric tokens a `"style"` wire key may override
/// for one [`LayoutNode::Styled`] subtree — merged onto whichever theme/
/// style were already in effect above that node (see
/// [`crate::render::paint_layout`]'s `LayoutNode::Styled` arm, which
/// brackets `Backend::set_theme`/`set_style` around the wrapped subtree's
/// own paint and restores the outer values once it returns — the same
/// "vary the theme per-pane" contract [`quadraui::Backend::set_theme`]'s
/// own doc already documents, just driven by the wire format instead of a
/// host's own render code). Every field left `None` (the default,
/// [`StyleOverride::is_empty`]) falls through to whatever the parent
/// already set, so a nested override only ever narrows what it names —
/// never resets a field an ancestor already overrode.
///
/// A node reaches this shape two ways, which merge into one
/// [`LayoutNode::Styled`] wrapper: its own inline `"style"` dict, and a
/// `"class"` key naming one or more registered [`StyleSets`] entries.
/// Inline wins — see [`StyleSets`]'s own precedence doc.
///
/// # Token mapping
///
/// | Wire key | Resolved onto | Read by (examples) |
/// |---|---|---|
/// | `bg` | [`quadraui::Theme::background`] | plain `Row`/`Column`/`Split`/`Tabs`/`SplitTree` areas; `Button`'s `Icon` variant; a non-`bordered` `List`/`Tree` |
/// | `fg` | [`quadraui::Theme::foreground`] | generic chrome text (`TabBar`'s dirty-tab glyph, `List`/`Tree`/`DataTable` body/header text) |
/// | `border` | [`quadraui::Theme::border_fg`] | bordered-surface border glyphs (`List`/`Dialog`) |
/// | `accent` | [`quadraui::Theme::accent_fg`] | focus/active-state accents (`Form` toggle, slider fill, `TabBar`'s active-tab underline) |
/// | `surface_bg` | [`quadraui::Theme::surface_bg`] | `Button`'s `Secondary`/`Danger`/`Warning` variants and disabled state, a `bordered` `List`, `Dialog`/`Toast`/`Palette`/`Toolbar`/`Float` chrome |
/// | `accent_bg` | [`quadraui::Theme::accent_bg`] | `Button`'s `Primary` variant's solid fill, `Switch`/`Progress`'s active fill |
/// | `padding` | [`quadraui::Style::padding`] | inner inset shared by `Button`/`Toast`/`Tooltip`/`ContextMenu`/`Palette`/`Dialog` |
/// | `corner_radius` | [`quadraui::Style::corner_radius`] | rounded-rect radius shared by every rounded overlay |
/// | `border_width` | [`quadraui::Style::border_width`] | border stroke width on a pixel backend (no-op on TUI, same as the token itself) |
///
/// A generic `bg` override does **not** change a `Button`'s fill on any
/// variant but `Icon` — `Button`'s own TUI/GTK painters read `surface_bg`
/// (`Secondary`/`Danger`/`Warning`/disabled) or `accent_bg` (`Primary`),
/// never `background`, so `{"style": {"bg": "#…"}}` on a `"button"` leaf is
/// a silent no-op. Use `surface_bg`/`accent_bg` instead — see the table
/// above for which variant reads which. The same split applies to every
/// other bordered/filled surface (`List`, `Dialog`, `Toast`, `Palette`,
/// `Switch`, `Progress`, `Toolbar`, `FindReplace`, `Float`): `bg` only ever
/// reaches `background`.
///
/// `bg`/`fg`/`border`/`accent`/`surface_bg`/`accent_bg` decode from the
/// same shapes [`quadraui::Color`]'s own `Deserialize` accepts (an
/// `{"r","g","b","a"}` object or `[r, g, b[, a]]` array), plus a
/// `"#rrggbb"`/`"#rrggbbaa"` hex string convenience — see
/// `crate::wire::style_override_from_value`.
///
/// # Every `Theme`/`Style` token is covered
///
/// Beyond the six semantic aliases the table above documents, every other
/// [`quadraui::Theme`] colour field and every [`quadraui::Style`] metric
/// field is overridable too, under the same name the struct it comes from
/// already uses (`"tab_bar_bg"`, `"selected_bg"`, `"row_height"`,
/// `"scrollbar_width"`, …) — [`style_override_tokens!`] is the single
/// source of truth both this struct and its impls below expand from, and
/// `crate::wire::style`'s decode and `crate::wire::layout::schema`'s `StyleValue`
/// schema expand the exact same table through their own callback. Adding a
/// field to `Theme`/`Style` and wiring it up here is one appended line to
/// that one table, not a four-way hand-sync.
///
/// [`StyleOverride::font_family`]/[`StyleOverride::font_size`]/
/// [`StyleOverride::font_weight`] and [`StyleOverride::hover`]/
/// [`StyleOverride::pressed`]/[`StyleOverride::focus`] are the exception:
/// they carry no matching `Theme`/`Style` field, so they are hand-written
/// fields on the struct this table expands into rather than table rows —
/// see [`StyleOverride`]'s own doc for why.
///
/// # Metric overrides feed layout
///
/// `padding` changes `crate::render::natural_size`'s measurement of a
/// `Content`/`Align`-sized `Button` leaf, not just how it paints —
/// `natural_size` brackets its own recursion into a
/// `Styled` node with the same push/pop this struct's override already
/// gets at paint time, so `Backend::style()` reports the merged value
/// while a wrapped leaf is being measured. Every other metric token
/// (`corner_radius`, `border_width`, and the rest) still only ever
/// changes how a subtree paints — no primitive measures them today, the
/// same way `Theme`'s colours never affect geometry.
///
/// # Generated from one table
///
/// `style_override_tokens!` lists every colour field (`$struct_field =>
/// $theme_field`, same identifier on both sides except the six aliases
/// above) and every metric field (`$field: $ty => $setter => $decode`,
/// `$setter` being the [`quadraui::Style`] `with_*` builder
/// [`StyleOverride::apply_style`] calls and `$decode` the
/// `crate::wire::value` reader `crate::wire::style::style_override_fields`
/// calls) exactly once, then hands that table to a callback macro — the
/// same X-macro shape `crate::extra_paint::extra_primitives` already uses
/// for its own `"kind" => Type` dispatch table. `define_style_override`
/// below is this struct's own callback.
macro_rules! style_override_tokens {
    ($callback:ident $(, $extra:tt)*) => {
        $callback! {
            $($extra)*
            colors: [
                bg => background,
                fg => foreground,
                border => border_fg,
                accent => accent_fg,
                surface_bg => surface_bg,
                accent_bg => accent_bg,
                tab_bar_bg => tab_bar_bg,
                tab_active_bg => tab_active_bg,
                tab_active_fg => tab_active_fg,
                tab_inactive_fg => tab_inactive_fg,
                tab_preview_active_fg => tab_preview_active_fg,
                tab_preview_inactive_fg => tab_preview_inactive_fg,
                separator => separator,
                surface_fg => surface_fg,
                selected_bg => selected_bg,
                inactive_selected_bg => inactive_selected_bg,
                title_fg => title_fg,
                header_bg => header_bg,
                header_fg => header_fg,
                muted_fg => muted_fg,
                error_fg => error_fg,
                warning_fg => warning_fg,
                query_fg => query_fg,
                match_fg => match_fg,
                hover_bg => hover_bg,
                hover_fg => hover_fg,
                hover_border => hover_border,
                input_bg => input_bg,
                inactive_fg => inactive_fg,
                selection_bg => selection_bg,
                link_fg => link_fg,
                completion_bg => completion_bg,
                completion_fg => completion_fg,
                completion_border => completion_border,
                completion_selected_bg => completion_selected_bg,
                scrollbar_track => scrollbar_track,
                scrollbar_thumb => scrollbar_thumb,
                editor_active_background => editor_active_background,
                cursorline_bg => cursorline_bg,
                dap_stopped_bg => dap_stopped_bg,
                colorcolumn_bg => colorcolumn_bg,
                diff_added_bg => diff_added_bg,
                diff_removed_bg => diff_removed_bg,
                diff_padding_bg => diff_padding_bg,
                line_number_fg => line_number_fg,
                line_number_active_fg => line_number_active_fg,
                diagnostic_error => diagnostic_error,
                diagnostic_warning => diagnostic_warning,
                diagnostic_info => diagnostic_info,
                diagnostic_hint => diagnostic_hint,
                git_added => git_added,
                git_modified => git_modified,
                git_deleted => git_deleted,
                lightbulb => lightbulb,
                spell_error => spell_error,
                cursor => cursor,
                selection => selection,
                yank_highlight_bg => yank_highlight_bg,
                bracket_match_bg => bracket_match_bg,
                indent_guide_fg => indent_guide_fg,
                indent_guide_active_fg => indent_guide_active_fg,
                annotation_fg => annotation_fg,
                ghost_text_fg => ghost_text_fg,
                command_line_bg => command_line_bg,
                command_line_fg => command_line_fg,
                board_selected_card_bg => board_selected_card_bg,
                board_col_header_bg => board_col_header_bg,
                badge_running => badge_running,
                badge_passed => badge_passed,
                badge_warning => badge_warning,
                badge_blocked => badge_blocked,
                card_hint_bg => card_hint_bg,
                card_hint_fg => card_hint_fg,
            ],
            theme_floats: [
                cursor_normal_alpha => cursor_normal_alpha,
                selection_alpha => selection_alpha,
                yank_highlight_alpha => yank_highlight_alpha,
            ],
            metrics: [
                padding: f32 => with_padding => opt_f32,
                corner_radius: f32 => with_corner_radius => opt_f32,
                border_width: f32 => with_border_width => opt_f32,
                focus_ring_width: f32 => with_focus_ring_width => opt_f32,
                button_radius: f32 => with_button_radius => opt_f32,
                shadow_elevation: u8 => with_shadow_elevation => opt_u8,
                control_height: f32 => with_control_height => opt_f32,
                backdrop_opacity: f32 => with_backdrop_opacity => opt_f32,
                tab_height: f32 => with_tab_height => opt_f32,
                status_bar_height: f32 => with_status_bar_height => opt_f32,
                activity_bar_width: f32 => with_activity_bar_width => opt_f32,
                row_height: f32 => with_row_height => opt_f32,
                toolbar_height: f32 => with_toolbar_height => opt_f32,
                scrollbar_width: f32 => with_scrollbar_width => opt_f32,
                row_padding_x: f32 => with_row_padding_x => opt_f32,
                tab_padding_x: f32 => with_tab_padding_x => opt_f32,
                tooltip_delay_ms: u64 => with_tooltip_delay_ms => opt_u64,
                tooltip_auto_dismiss_ms: u64 => with_tooltip_auto_dismiss_ms => opt_u64,
                status_bar_cell_gap: f32 => with_status_bar_cell_gap => opt_f32,
                status_bar_cell_overflow: CellOverflow
                    => with_status_bar_cell_overflow => opt_cell_overflow,
                list_cell_overflow: CellOverflow
                    => with_list_cell_overflow => opt_cell_overflow,
            ],
        }
    };
}
pub(crate) use style_override_tokens;

/// A per-node font-weight override — mirrors the one weight distinction
/// every pixel backend's native font API already exposes through a
/// plain bold flag (e.g. `measure_text_styled(text, bold)`), rather than
/// modelling the full CSS `100`–`900` numeric scale no backend resolves
/// today. A future backend that wants finer weights grows this enum
/// additively; nothing here forecloses that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontWeight {
    Normal,
    Bold,
}

/// Which interaction state a leaf currently has, for
/// [`StyleOverride::resolve`] to pick the matching state-variant branch
/// (`hover`/`pressed`/`focus`) of a `"style"` override. A binding's own
/// hit-test/focus tracking decides which states currently apply to a
/// given node; this module only resolves the override once told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InteractionState {
    Hovered,
    Pressed,
    Focused,
}

/// [`style_override_tokens!`]'s callback for [`StyleOverride`] itself:
/// the struct (one `Option<Color>` field per colour token, one
/// `Option<$ty>` per metric token) plus every impl this module's own
/// callers need — [`StyleOverride::is_empty`], [`StyleOverride::apply_theme`],
/// [`StyleOverride::apply_style`] and [`StyleOverride::merged_over`].
macro_rules! define_style_override {
    (
        colors: [ $( $cfield:ident => $tfield:ident ),* $(,)? ],
        theme_floats: [ $( $ffield:ident => $tffield:ident ),* $(,)? ],
        metrics: [ $( $mfield:ident : $mty:ty => $setter:ident => $decode:ident ),* $(,)? ],
    ) => {
        /// A scoped subset of the backend-wide [`quadraui::Theme`] colour
        /// tokens and [`quadraui::Style`] metric tokens a `"style"` wire
        /// key may override for one [`LayoutNode::Styled`] subtree —
        /// merged onto whichever theme/style were already in effect above
        /// that node. See this module's own doc comment right above
        /// [`style_override_tokens!`] for the full token list and the
        /// token-mapping table for the six semantic aliases.
        ///
        /// # Font and interaction-state tokens
        ///
        /// [`Self::font_family`]/[`Self::font_size`]/[`Self::font_weight`]
        /// and [`Self::hover`]/[`Self::pressed`]/[`Self::focus`] are
        /// **not** generated from [`style_override_tokens!`]'s table —
        /// unlike every other field above, none of them maps onto an
        /// existing [`quadraui::Theme`]/[`quadraui::Style`] field, so
        /// [`Self::apply_theme`]/[`Self::apply_style`] never touch them.
        /// They are read straight off the resolved override instead: a
        /// paint path calls [`Self::resolve`] with the node's current
        /// [`InteractionState`]s to fold the matching branch over the
        /// base tokens, then reads `font_family`/`font_size`/
        /// `font_weight` off the result directly.
        ///
        /// `font_family` being an owned `String` (rather than
        /// `Option<Color>`/`Option<f32>`'s plain-data shapes) is why this
        /// struct is `Clone`, not `Copy`, unlike most small token
        /// structs in this crate — [`StyleSets::get`] clones a boxed
        /// `Defined` entry instead of copying it out.
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct StyleOverride {
            $(pub $cfield: Option<Color>,)*
            $(pub $ffield: Option<f32>,)*
            $(pub $mfield: Option<$mty>,)*
            /// Per-node font family override (e.g. `"Fira Code"`) — see
            /// this struct's own "Font and interaction-state tokens"
            /// doc for why it isn't merged by [`Self::apply_theme`]/
            /// [`Self::apply_style`].
            pub font_family: Option<String>,
            /// Per-node font size override, in surface-native pixels.
            pub font_size: Option<f32>,
            /// Per-node font weight override.
            pub font_weight: Option<FontWeight>,
            /// Override applied when the node is hovered, merged over
            /// this override's own base tokens by [`Self::resolve`].
            /// Boxed for the same reason [`LayoutStyled::style`] is —
            /// inlining a full [`StyleOverride`] three times over would
            /// make this the dominant size of every value that embeds
            /// one.
            pub hover: Option<Box<StyleOverride>>,
            /// Override applied when the node is pressed. See
            /// [`Self::hover`].
            pub pressed: Option<Box<StyleOverride>>,
            /// Override applied when the node has keyboard focus. See
            /// [`Self::hover`].
            pub focus: Option<Box<StyleOverride>>,
        }

        impl StyleOverride {
            /// `true` when every field is `None` — an override that
            /// changes nothing, equivalent to no `"style"` key at all.
            /// `crate::wire::style_override_from_value` folds an
            /// all-empty `{"style": {}}` down to `None` via this check,
            /// rather than threading a no-op [`LayoutNode::Styled`]
            /// wrapper through the tree.
            pub fn is_empty(&self) -> bool {
                *self == Self::default()
            }

            /// Applies this override onto `theme`, leaving every field
            /// this override didn't name unchanged. Every [`Theme`] field
            /// is named by either the colour or `theme_floats` group, so
            /// this struct literal is already exhaustive and needs no
            /// `..theme` base.
            pub fn apply_theme(&self, theme: Theme) -> Theme {
                Theme {
                    $($tfield: self.$cfield.unwrap_or(theme.$tfield),)*
                    $($tffield: self.$ffield.unwrap_or(theme.$tffield),)*
                }
            }

            /// [`Self::apply_theme`]'s [`quadraui::Style`] counterpart.
            /// `Style` is `#[non_exhaustive]`, so this goes through its
            /// own `with_*` builder methods rather than struct-update
            /// syntax.
            pub fn apply_style(&self, style: Style) -> Style {
                let mut style = style;
                $(
                    if let Some(v) = self.$mfield {
                        style = style.$setter(v);
                    }
                )*
                style
            }

            /// Merges `self` *over* `base`: every token `self` names
            /// wins, every token it leaves `None` falls through to
            /// whatever `base` named (or stays `None` if neither did).
            /// The merge a `"class"` list and a node's own inline
            /// `"style"` key resolve through — see [`StyleSets::resolve`]
            /// for the full precedence chain.
            ///
            /// `hover`/`pressed`/`focus` merge recursively: when both
            /// `self` and `base` name the same state branch, that
            /// branch's own tokens merge the same way (`self`'s branch
            /// over `base`'s), rather than one branch replacing the
            /// other wholesale — so a `"class"` list's `hover` tokens
            /// and a node's own inline `"style"`'s `hover` tokens
            /// combine instead of the inline one silently discarding
            /// the class's.
            pub fn merged_over(&self, base: StyleOverride) -> StyleOverride {
                StyleOverride {
                    $($cfield: self.$cfield.or(base.$cfield),)*
                    $($ffield: self.$ffield.or(base.$ffield),)*
                    $($mfield: self.$mfield.or(base.$mfield),)*
                    font_family: self.font_family.clone().or(base.font_family),
                    font_size: self.font_size.or(base.font_size),
                    font_weight: self.font_weight.or(base.font_weight),
                    hover: merge_state_branch(self.hover.as_deref(), base.hover.as_deref()),
                    pressed: merge_state_branch(self.pressed.as_deref(), base.pressed.as_deref()),
                    focus: merge_state_branch(self.focus.as_deref(), base.focus.as_deref()),
                }
            }

            /// Resolves this override for the leaf's current interaction
            /// `states`: picks the matching state-variant branch and
            /// merges it *over* `self`'s own base tokens via
            /// [`Self::merged_over`], so a state branch only ever narrows
            /// what it names — a token the branch leaves unset still
            /// falls through to the base override, exactly like a nested
            /// `"class"`/`"style"` merge.
            ///
            /// `pressed` takes precedence over `hovered`, which takes
            /// precedence over `focused`, when more than one applies at
            /// once — a node that is both hovered and pressed (ordinary
            /// mouse-down-while-hovering) reads as pressed, matching every
            /// `Button`-family primitive's own resting/hover/pressed
            /// precedence. Ranking falls through to the next-highest
            /// active state when a higher-ranked one has no branch of its
            /// own set: a style that only defines `hover` keeps its hover
            /// tint once the mouse goes down, rather than losing it the
            /// instant `pressed` (unset) would otherwise take over. An
            /// empty `states`, or no branch set for any state in
            /// `states`, returns `self` unchanged.
            ///
            /// The returned override still carries its own
            /// `hover`/`pressed`/`focus` fields (unlike `font_family`/
            /// `font_size`/`font_weight` and every colour/metric token,
            /// which the chosen branch already folded in) — calling this
            /// method again on the result for a *different* state layers
            /// that state's branch on top of whichever one already got
            /// applied, rather than resolving fresh against the original
            /// base. Call this once per node per frame, on the
            /// not-yet-resolved override, not on a previous call's result.
            pub fn resolve(&self, states: &[InteractionState]) -> StyleOverride {
                let resolved = self.clone();
                if states.contains(&InteractionState::Pressed) {
                    if let Some(over) = self.pressed.as_deref() {
                        return over.merged_over(resolved);
                    }
                }
                if states.contains(&InteractionState::Hovered) {
                    if let Some(over) = self.hover.as_deref() {
                        return over.merged_over(resolved);
                    }
                }
                if states.contains(&InteractionState::Focused) {
                    if let Some(over) = self.focus.as_deref() {
                        return over.merged_over(resolved);
                    }
                }
                resolved
            }
        }
    };
}
style_override_tokens!(define_style_override);

/// [`StyleOverride::merged_over`]'s helper for one `hover`/`pressed`/
/// `focus` branch: merges `over` onto `under` when both are set, keeps
/// whichever one is set when only one is, and stays `None` when neither
/// is — the same "narrow, never reset" merge [`StyleOverride::merged_over`]
/// itself documents, applied one level down.
fn merge_state_branch(
    over: Option<&StyleOverride>,
    under: Option<&StyleOverride>,
) -> Option<Box<StyleOverride>> {
    match (over, under) {
        (Some(o), Some(u)) => Some(Box::new(o.merged_over(u.clone()))),
        (Some(o), None) => Some(Box::new(o.clone())),
        (None, Some(u)) => Some(Box::new(u.clone())),
        (None, None) => None,
    }
}

/// One [`BUILTIN_STYLE_SETS`] entry: a name paired with the `Theme`-deriving
/// function it resolves through.
type BuiltinStyleSet = (&'static str, fn(&Theme) -> StyleOverride);

/// Each built-in style-set name paired with the `Theme`-deriving function
/// [`StyleSets::new`] registers it under — the one source both `new()`
/// and [`BUILTIN_STYLE_SET_NAMES`] read from, so a future fifth built-in
/// (or a renamed one) can't update one and forget the other.
const BUILTIN_STYLE_SETS: [BuiltinStyleSet; 4] = [
    ("primary", builtin_primary),
    ("secondary", builtin_secondary),
    ("danger", builtin_danger),
    ("subtle", builtin_subtle),
];

/// The names [`BUILTIN_STYLE_SETS`] registers, in that table's own order —
/// **not** the sorted order [`StyleSets::names`] reports them in. Each
/// maps onto existing `Theme` tokens (and, through them, onto an existing
/// `ButtonVariant`'s own resting fill) rather than onto fixed literal
/// colours, so a built-in set follows an app's own `set_theme` palette —
/// see [`StyleSets`]'s own doc table.
pub const BUILTIN_STYLE_SET_NAMES: [&str; 4] = [
    BUILTIN_STYLE_SETS[0].0,
    BUILTIN_STYLE_SETS[1].0,
    BUILTIN_STYLE_SETS[2].0,
    BUILTIN_STYLE_SETS[3].0,
];

/// How one named style set produces the [`StyleOverride`] a `"class"`
/// reference to it resolves to. `Defined` boxes its payload for the same
/// size reason [`LayoutStyled::style`] does — `StyleOverride` covers every
/// `Theme`/`Style` token, so storing it inline would make this the
/// dominant size of every [`StyleSets`] entry; [`StyleSets::get`] reads
/// the boxed value back out with a clone (`StyleOverride` is `Clone`,
/// not `Copy` — see its own doc comment for why).
#[derive(Clone)]
enum StyleSetDef {
    /// A set a host defined through its own `define_style` surface: fixed
    /// tokens, independent of the active [`Theme`].
    Defined(Box<StyleOverride>),
    /// One of [`BUILTIN_STYLE_SET_NAMES`], derived from whichever `Theme`
    /// is active when it resolves.
    Builtin(fn(&Theme) -> StyleOverride),
}

/// The named style sets a layout node's `"class"` key refers to — the
/// CSS-class analogue of this wire format's own per-node `"style"` key.
/// A host registers a set under a name through its binding's own
/// `define_style(name, overrides)` surface; any node in any later frame's
/// tree then carries `{"class": "<name>"}` (or a list of names) to pull in
/// that set's tokens, instead of restating them inline on every node that
/// wants them.
///
/// # Precedence
///
/// [`Self::resolve`] merges the named sets left to right (a later name in
/// the list wins a token an earlier one also named), and
/// `crate::wire::layout_node_from_value` then merges the node's own inline
/// `"style"` dict over that result — so inline always wins, exactly like a
/// CSS `style=` attribute beating a `class=` rule. A name this registry
/// has never heard of contributes nothing and is **not** an error, the
/// same data-format-v1 unknown-key policy an unrecognised sibling key on a
/// node dict already gets.
///
/// # Built-in sets
///
/// [`Self::new`] starts with four sets, each derived from the live
/// [`Theme`] rather than from literal colours:
///
/// | Name | Tokens | Intent |
/// |---|---|---|
/// | `primary` | `accent_bg`/`surface_bg` = [`Theme::accent_bg`], `accent` = [`Theme::accent_fg`] | [`quadraui::ButtonVariant::Primary`]'s own resting fill, extended to every filled surface in the subtree |
/// | `secondary` | `accent_bg`/`surface_bg` = [`Theme::surface_bg`], `fg` = [`Theme::surface_fg`] | [`quadraui::ButtonVariant::Secondary`]'s own resting fill |
/// | `danger` | `accent_bg`/`surface_bg` = [`Theme::button_danger_bg`], `border`/`accent` = [`Theme::error_fg`] | [`quadraui::ButtonVariant::Danger`]'s own resting fill |
/// | `subtle` | `fg`/`border` = [`Theme::muted_fg`], `accent_bg`/`surface_bg` = [`Theme::background`] | de-emphasised chrome: muted text, no raised surface |
///
/// Each sets *both* `accent_bg` and `surface_bg` because which of the two
/// a `Button` reads depends on its variant ([`Theme::accent_bg`] for
/// `Primary`, [`Theme::surface_bg`] for `Secondary`/`Icon`) — naming only
/// one would make `{"class": "danger"}` land on some buttons and silently
/// no-op on others. See [`StyleOverride`]'s own token table for which
/// other primitives read which.
///
/// [`Self::define`] replaces a built-in under the same name, so an app
/// that wants its own `primary` simply defines one.
///
/// # Which theme a built-in resolves against
///
/// A built-in resolves against the theme passed to [`Self::resolve`] —
/// the backend's own app-wide [`Theme`] at the start of the frame, since
/// `"class"` is resolved during the render callback's decode. An ancestor
/// node's `"style"` key that changes `surface_bg` for its subtree does
/// *not* re-derive a descendant's `{"class": "secondary"}` against that
/// narrowed theme.
#[derive(Clone)]
pub struct StyleSets {
    sets: HashMap<String, StyleSetDef>,
}

impl Default for StyleSets {
    fn default() -> Self {
        Self::new()
    }
}

impl StyleSets {
    /// A registry holding exactly the [`BUILTIN_STYLE_SETS`] sets.
    pub fn new() -> Self {
        let mut sets: HashMap<String, StyleSetDef> = HashMap::new();
        for (name, derive) in BUILTIN_STYLE_SETS {
            sets.insert(name.to_owned(), StyleSetDef::Builtin(derive));
        }
        Self { sets }
    }

    /// Registers `overrides` under `name`, replacing whatever set (built-in
    /// or previously defined) that name already held. Redefining a name
    /// takes effect on the next frame: nothing caches a resolved override,
    /// so every `"class"` reference in the next tree re-resolves through
    /// this registry.
    pub fn define(&mut self, name: impl Into<String>, overrides: StyleOverride) {
        self.sets
            .insert(name.into(), StyleSetDef::Defined(Box::new(overrides)));
    }

    /// Every registered set name, sorted so a caller enumerating them gets
    /// a stable order out of the underlying [`HashMap`].
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.sets.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    /// One set's own [`StyleOverride`], resolved against `theme` if it is a
    /// built-in. `None` for a name this registry has never heard of.
    pub fn get(&self, name: &str, theme: &Theme) -> Option<StyleOverride> {
        self.sets.get(name).map(|def| match def {
            StyleSetDef::Defined(overrides) => (**overrides).clone(),
            StyleSetDef::Builtin(derive) => derive(theme),
        })
    }

    /// Merges every set `names` refers to into one [`StyleOverride`], left
    /// to right — a later name wins any token an earlier one also named.
    /// A name this registry doesn't hold contributes nothing (see this
    /// type's own doc for why that is not an error).
    pub fn resolve(&self, names: &[String], theme: &Theme) -> StyleOverride {
        let mut merged = StyleOverride::default();
        for name in names {
            if let Some(overrides) = self.get(name, theme) {
                merged = overrides.merged_over(merged);
            }
        }
        merged
    }
}

/// The `primary` built-in — see [`StyleSets`]'s own table.
fn builtin_primary(theme: &Theme) -> StyleOverride {
    StyleOverride {
        accent_bg: Some(theme.accent_bg),
        surface_bg: Some(theme.accent_bg),
        accent: Some(theme.accent_fg),
        ..StyleOverride::default()
    }
}

/// The `secondary` built-in — see [`StyleSets`]'s own table.
fn builtin_secondary(theme: &Theme) -> StyleOverride {
    StyleOverride {
        accent_bg: Some(theme.surface_bg),
        surface_bg: Some(theme.surface_bg),
        fg: Some(theme.surface_fg),
        ..StyleOverride::default()
    }
}

/// The `danger` built-in — see [`StyleSets`]'s own table.
fn builtin_danger(theme: &Theme) -> StyleOverride {
    let fill = theme.button_danger_bg();
    StyleOverride {
        accent_bg: Some(fill),
        surface_bg: Some(fill),
        border: Some(theme.error_fg),
        accent: Some(theme.error_fg),
        ..StyleOverride::default()
    }
}

/// The `subtle` built-in — see [`StyleSets`]'s own table.
fn builtin_subtle(theme: &Theme) -> StyleOverride {
    StyleOverride {
        fg: Some(theme.muted_fg),
        border: Some(theme.muted_fg),
        accent_bg: Some(theme.background),
        surface_bg: Some(theme.background),
        ..StyleOverride::default()
    }
}

/// A [`LayoutNode::Row`]/[`LayoutNode::Column`]'s decoded shape: its child
/// slots ([`LayoutChild`], each pairing one child node with its own
/// main-axis size/min/max) paired with the [`Spacing`] (`padding`/`margin`/
/// `gap`), the main-axis [`Justify`]/cross-axis [`Align`], and `wrap` the
/// render callback's `"row"`/`"column"` dict declared alongside them — see
/// `crate::wire::layout_node_from_value`'s decode for the wire shape.
/// `spacing`/`justify`/`align` each default to no inset/no gap/`Start`/
/// `Stretch`, `wrap` defaults to `false`, and every child's own size
/// defaults to [`SectionSize::EqualShare`] with no min/max, so an existing
/// tree with none of these keys paints the same shares as before they
/// existed: every child gets the same share of the full container, the way
/// [`quadraui::layout::stack::split_rects_with_gap`] (this container's own
/// predecessor) always divided it — down to floating-point tolerance, not
/// necessarily the identical bit pattern, since the two now reach that
/// share by different arithmetic (see
/// `crate::render::layout_container_rects`).
///
/// `wrap: true` flows children onto a new line, stacked along the cross
/// axis, once the next child would overflow the container's main-axis
/// extent — CSS flexbox's `flex-wrap: wrap`, restricted to a single
/// direction (no `wrap-reverse`). See
/// `quadraui::layout::stack::stack_rects` for the multi-line
/// resolution; `justify`/`align` still apply, each scoped to the line a
/// child landed on rather than the container's full extent.
pub struct LayoutContainer {
    pub children: Vec<LayoutChild>,
    pub spacing: Spacing,
    pub justify: Justify,
    pub align: Align,
    pub wrap: bool,
}

impl LayoutContainer {
    /// Builds a container whose every child uses the default
    /// [`LayoutChild::new`] sizing (`EqualShare`, no min/max) and whose
    /// `justify`/`align`/`wrap` are all their defaults — the equal-share-only
    /// shape every `"row"`/`"column"` node had before per-child sizing
    /// existed. Convenient for a call site that only ever deals in bare
    /// [`LayoutNode`]s (most of this crate's own tests, and any decode
    /// path that hasn't been taught the new per-child keys).
    pub fn new(children: Vec<LayoutNode>, spacing: Spacing) -> Self {
        Self {
            children: children.into_iter().map(LayoutChild::new).collect(),
            spacing,
            justify: Justify::default(),
            align: Align::default(),
            wrap: false,
        }
    }
}

/// One [`LayoutContainer`] child slot: the child [`LayoutNode`] itself,
/// paired with the main-axis [`SectionSize`] strategy it resolves under
/// (default [`SectionSize::EqualShare`], the only strategy that existed
/// before per-child sizing), and an optional `min`/`max` clamp in the same
/// portable surface-native-pixel unit [`Spacing`]'s fields use (quantised
/// to a whole native unit at paint time, same as `padding`/`margin`/`gap`
/// — see `crate::render::layout_container_rects`).
pub struct LayoutChild {
    pub node: LayoutNode,
    pub size: SectionSize,
    pub min: Option<f32>,
    pub max: Option<f32>,
}

impl LayoutChild {
    /// A child with the pre-existing default sizing: `EqualShare`, no
    /// min/max.
    pub fn new(node: LayoutNode) -> Self {
        Self {
            node,
            size: SectionSize::default(),
            min: None,
            max: None,
        }
    }
}

/// A [`LayoutNode::Grid`]'s decoded shape: two independent track lists —
/// `columns` along the horizontal axis, `rows` along the vertical one —
/// plus one child subtree per cell, in row-major order (left to right
/// within a row, then top to bottom), and this node's own
/// `padding`/`margin` inset.
///
/// Geometry is [`quadraui::layout::resolve_grid_tracks`]'s, never
/// reimplemented here: both axes resolve through the same
/// `Fixed`/`Content` → `Percent` → `Weight` pass order a `Row`/`Column`
/// child's [`SectionSize`] resolves under, so a two-dimensional dashboard
/// is one node instead of a hand-nested row-of-columns whose cells only
/// line up by accident. There is no `gap` key (a grid's own tracks carry
/// no gutter — a caller wanting one adds `padding` to the cell's own
/// child, or a `Fixed` spacer track), and no id: a grid holds no
/// interaction state of its own, exactly like `Row`/`Column`.
///
/// `children` never exceeds `columns.len() * rows.len()` — a binding's
/// decode rejects an over-full grid rather than silently dropping the
/// overflow. Fewer children than cells is fine: the trailing cells paint
/// nothing.
pub struct LayoutGrid {
    pub columns: Vec<GridTrack>,
    pub rows: Vec<GridTrack>,
    pub children: Vec<LayoutNode>,
    pub spacing: Spacing,
}

impl LayoutGrid {
    /// This grid's cell rects inside `area`, flattened row-major so the
    /// result zips straight against [`Self::children`] — resolved by
    /// [`quadraui::layout::grid_cell_rects`] against the painting
    /// backend's `char_width`/`line_height`. [`crate::render::paint_layout`]
    /// and [`crate::host::resolve_layout_node`] both call this, so the rect
    /// a nested leaf is resolved against is the rect it is painted into.
    pub fn cell_rects(&self, area: Rect, char_width: f32, line_height: f32) -> Vec<Rect> {
        grid_cell_rects(
            &self.columns,
            &self.rows,
            &self.spacing,
            area,
            char_width,
            line_height,
        )
    }
}

/// A [`LayoutNode::Split`]'s decoded shape: the `quadraui::Split` descriptor
/// (its own `id`, `direction`, `ratio`, pane minimums) paired with the two
/// named child nodes it divides. `first`/`second` are full [`LayoutNode`]s,
/// not leaf-only slots, so a pane may itself hold a nested row/column/split.
/// `spacing` is this node's own `padding`/`margin`/`gap` — `gap` is the
/// space between `first` and `second`, inserted after `Split::first_min`/
/// `second_min` are evaluated (see `crate::render::paint_layout`'s
/// `LayoutNode::Split` arm), so a minimum applies to the pane after the
/// gap is already carved out of the available space.
pub struct LayoutSplit {
    pub split: Split,
    pub first: Box<LayoutNode>,
    pub second: Box<LayoutNode>,
    pub spacing: Spacing,
}

/// A [`LayoutNode::Tabs`]'s decoded shape: a `quadraui::TabBar` descriptor
/// built from each tab's title (with `selected`'s item marked active) for
/// painting the strip, paired with `entries` holding the full content
/// subtree behind each tab. A tab group is a `Split` whose `first`/`second`
/// are each a `Tabs` — no separate "group" concept. `spacing` is this
/// node's own `padding`/`margin` (its `gap` is always `0.0` — a `"tabs"`
/// node decodes `"gap"` as an error instead, see
/// `crate::wire::spacing_from_value`), inset around the whole node
/// (strip and content both) before the strip/content split.
pub struct LayoutTabs {
    pub tab_bar: TabBar,
    pub entries: Vec<LayoutTabEntry>,
    pub selected: usize,
    pub spacing: Spacing,
    /// Index of the keyboard-focused tab, decoded from the `"tabs"`
    /// dict's optional `"focused_index"` key. Threaded into a
    /// [`quadraui::TabChrome`] rather than onto `tab_bar` directly — see
    /// that field's doc for why `TabBar` itself stays a plain field.
    pub focused_index: Option<usize>,
}

/// One tab inside a [`LayoutTabs`]: the id that addresses it, and the full
/// [`LayoutNode`] subtree painted beneath the strip while it is selected.
pub struct LayoutTabEntry {
    pub id: String,
    pub content: Box<LayoutNode>,
}

/// A [`LayoutNode::SidebarPanel`]'s decoded shape: the `quadraui::SidebarPanel`
/// chrome descriptor (optional toolbar) paired with the single content
/// subtree painted beneath the toolbar slot.
pub struct LayoutSidebarPanel {
    pub sidebar_panel: SidebarPanel,
    pub content: Box<LayoutNode>,
}

/// A [`LayoutNode::Scroll`]'s decoded shape: a generic scrollable region
/// hosting one content subtree, built on [`quadraui::ScrollView`] for
/// offset clamping, visible-range, and scrollbar geometry — never
/// reimplemented here, see that primitive's own module doc. `content`'s
/// natural size along `axis` becomes `ScrollView::content_len`;
/// [`crate::render::paint_layout`]/[`crate::host::resolve_layout_node`] lay
/// it out at that natural size, offset by the resolved scroll position,
/// and clip to the viewport via `Backend::push_clip`/`Backend::pop_clip`.
/// `id` is a bare `String`, not a `WidgetId`, matching [`LayoutSplitTree::id`]/
/// [`LayoutLeaf::id`]'s own convention — wrapped at each use site instead.
pub struct LayoutScroll {
    pub id: String,
    pub axis: ScrollAxis,
    /// Controlled scroll offset from the wire — `None` means uncontrolled
    /// (the engine owns it, resolved in [`crate::host::resolve_layout_node`]
    /// against [`crate::widget_state::WidgetState`] the same way
    /// [`crate::host::resolve_split_ratio`] resolves a `Split`'s divider).
    /// Unlike `Split::ratio` (a bare `f32` with no "unset" sentinel), this
    /// field is already `Option<f32>`, so `WidgetState::resolve`'s own
    /// controlled/uncontrolled policy applies directly — no baseline latch
    /// needed.
    pub offset: Option<f32>,
    pub content: Box<LayoutNode>,
    /// Inset around the viewport before scroll content is measured/painted
    /// — this node's own `padding`/`margin` (no `gap`: there's only one
    /// content subtree).
    pub spacing: Spacing,
}

/// A [`LayoutNode::SplitTree`]'s decoded shape: the `quadraui::SplitTree`
/// geometry tree (every leaf position holding a synthetic, internal-only
/// `WidgetId`) paired with `leaves`, the matching content subtree for each
/// leaf position in the same pre-order [`SplitTree::leaf_ids`] would
/// produce. [`crate::render::paint_layout`]'s `LayoutNode::SplitTree` arm zips
/// `SplitTreeLayout::leaves`' resolved rects back against `leaves` by
/// position — the synthetic ids in `tree` are never user-facing and never
/// checked for uniqueness; `id` is the node's own user-supplied id instead.
pub struct LayoutSplitTree {
    pub id: String,
    pub tree: SplitTree,
    pub leaves: Vec<LayoutNode>,
}

/// One id-addressable primitive inside a layout tree, decoded from a
/// `{"id": ..., "<kind>": <descriptor>}` dict. `id` is unique across the
/// whole tree ([`validate_unique_leaf_ids`] checks this, called by
/// [`crate::wire::decode_render_output`] after decoding a tree), so a kind
/// may appear more than once (e.g. two `data_table`s side by side) with
/// each instance still addressable on its own.
pub struct LayoutLeaf {
    pub id: String,
    pub descriptor: LeafDescriptor,
    /// Optional pixel-precise scroll position lifted out of this leaf's
    /// raw `"list"`/`"tree"`/`"data_table"` descriptor dict by
    /// [`crate::scroll_px_from_value`] — `None` unless the dict set a
    /// numeric `"scroll_px"` key, and always `None` for a `descriptor`
    /// that isn't one of those three kinds. [`crate::render::paint_leaf`]
    /// passes it in the paint state the leaf is painted with; a dict that
    /// never sets the key paints exactly as before.
    pub scroll_px: Option<f32>,
}

/// The decoded descriptor carried by a [`LayoutLeaf`] — one variant per
/// primitive kind a layout tree leaf may wrap.
pub enum LeafDescriptor {
    TextDisplay(TextDisplay),
    List(ListView),
    Tree(TreeView),
    DataTable(DataTable),
    TabBar(TabBar),
    /// Any additive primitive kind registered in a binding's own
    /// `EXTRA_PRIMITIVES` decode table, or a `"scripted_widget"` leaf
    /// spliced in from `crate::scripted::register_scripted_widget`'s own
    /// registry (see that function's doc). Unlike the five variants above,
    /// neither ever adds a new `LeafDescriptor` variant — both route
    /// through this one.
    Extra(Rc<dyn ExtraPaint>),
    /// A kind registered in [`crate::extra_paint::WIDGET_KINDS`], decoded
    /// through [`quadraui::decode_widget`] into a type-erased
    /// [`quadraui::primitive::DynWidget`] rather than an [`ExtraPaint`] —
    /// the generic seam a binding kind joins when its `quadraui::Primitive`
    /// has no bespoke paint/hit-test/event wiring of its own to write
    /// here, only the decode a [`WIDGET_KINDS`] entry adds (both as a
    /// layout-tree leaf, this variant, and as a flat
    /// [`crate::render::RenderFrame::widgets`] slot, which carries the
    /// same decoded `DynWidget` without going through `LeafDescriptor` at
    /// all). Painted via [`quadraui::DrawsPrimitives::draw_dyn`]
    /// (see [`crate::render::paint_leaf`]'s `Widget` arm), hit-tested as a
    /// whole area (see [`crate::hit::PaintedHit::Widget`]), and routed to
    /// [`DynWidget::handle`] on a click (see
    /// [`crate::engine::InteractionEngine::handle`]'s `HitDetail::WidgetBody`
    /// arm). `kind` is the matched [`WIDGET_KINDS`] string, kept for
    /// diagnostics — nothing in this generic path downcasts on it, unlike
    /// [`Self::Extra`]'s own kind-specific arms.
    ///
    /// [`WIDGET_KINDS`]: crate::extra_paint::WIDGET_KINDS
    Widget {
        kind: &'static str,
        widget: Rc<dyn DynWidget>,
    },
}

/// The descriptor keys a layout tree leaf dict may carry — the five names
/// [`crate::render::RenderFrame`]'s own flat-slot `Deserialize` impl uses
/// for its slots, each mapping to its own [`LeafDescriptor`] variant, plus
/// `"scripted_widget"` (see [`LeafDescriptor::Extra`]'s own doc), which
/// maps to [`LeafDescriptor::Extra`] like every
/// [`crate::extra_paint::EXTRA_PRIMITIVE_KINDS`] kind does — unlike those,
/// though, `"scripted_widget"` can't be decoded through that same
/// `extra_primitives!` table (no `Deserialize` impl exists, or ever will,
/// for a live script callback object), so it is listed here instead,
/// handled by its own arm in `crate::wire::leaf::layout_leaf_from_value`'s
/// per-key loop.
pub const LEAF_KIND_KEYS: [&str; 6] = [
    "text_display",
    "list",
    "tree",
    "data_table",
    "tab_bar",
    "scripted_widget",
];

/// Builds the [`DecodeError::Borrow`] [`validate_unique_leaf_ids`]'s walk
/// raises on the first repeated id, naming both the offending `kind` (the
/// node currently being visited -- the second, colliding occurrence of
/// `id`, not whichever earlier node first claimed its namespace) and `id`
/// itself.
fn duplicate_leaf_id_error(what: &str, kind: &str, id: &str) -> DecodeError {
    DecodeError::borrow(
        format!("{what} › {kind}#{id}"),
        format!("duplicate leaf id {id:?}"),
    )
}

/// Walks `node` collecting every leaf and split `id` into one namespace,
/// returning a [`DecodeError`] on the first repeat. A unique id per
/// leaf/split lets host code address one instance unambiguously even when a
/// kind appears more than once, and keeps a split's id from silently
/// shadowing a leaf's. `what` labels the error message the same way every
/// other decoder in this crate does — [`crate::wire::decode_render_output`]
/// passes its own caller-supplied label through unchanged.
///
/// `what` accumulates every ancestor's `kind#id` segment on the way down
/// (the same ` › `-joined shape every other decoder in this crate builds),
/// so a collision five levels into a tree names the whole subtree it was
/// found in, not just the colliding leaf's bare `kind#id` hung directly off
/// the tree's root label.
pub fn validate_unique_leaf_ids(what: &str, node: &LayoutNode) -> Result<(), DecodeError> {
    fn walk(what: &str, node: &LayoutNode, seen: &mut HashSet<String>) -> Result<(), DecodeError> {
        match node {
            LayoutNode::Row(container) | LayoutNode::Column(container) => {
                for (index, child) in container.children.iter().enumerate() {
                    let context = format!("{what} › children[{index}]");
                    walk(&context, &child.node, seen)?;
                }
                Ok(())
            }
            LayoutNode::Grid(grid) => {
                for (index, child) in grid.children.iter().enumerate() {
                    let context = format!("{what} › cells[{index}]");
                    walk(&context, child, seen)?;
                }
                Ok(())
            }
            LayoutNode::Leaf(leaf) => {
                if !seen.insert(leaf.id.clone()) {
                    return Err(duplicate_leaf_id_error(what, "leaf", &leaf.id));
                }
                Ok(())
            }
            LayoutNode::Split(split) => {
                let id = split.split.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(duplicate_leaf_id_error(what, "split", &id));
                }
                let context = format!("{what} › split#{id}");
                walk(&context, &split.first, seen)?;
                walk(&context, &split.second, seen)
            }
            LayoutNode::Tabs(tabs) => {
                let id = tabs.tab_bar.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(duplicate_leaf_id_error(what, "tabs", &id));
                }
                let context = format!("{what} › tabs#{id}");
                for entry in &tabs.entries {
                    if !seen.insert(entry.id.clone()) {
                        return Err(duplicate_leaf_id_error(&context, "tab", &entry.id));
                    }
                    let entry_context = format!("{context} › tab#{}", entry.id);
                    walk(&entry_context, &entry.content, seen)?;
                }
                Ok(())
            }
            LayoutNode::SidebarPanel(sidebar_node) => {
                let id = sidebar_node.sidebar_panel.id.as_str().to_owned();
                if !seen.insert(id.clone()) {
                    return Err(duplicate_leaf_id_error(what, "sidebar_panel", &id));
                }
                let context = format!("{what} › sidebar_panel#{id}");
                walk(&context, &sidebar_node.content, seen)
            }
            LayoutNode::SplitTree(split_tree) => {
                if !seen.insert(split_tree.id.clone()) {
                    return Err(duplicate_leaf_id_error(what, "split_tree", &split_tree.id));
                }
                let context = format!("{what} › split_tree#{}", split_tree.id);
                for leaf in &split_tree.leaves {
                    walk(&context, leaf, seen)?;
                }
                Ok(())
            }
            LayoutNode::Scroll(scroll_node) => {
                if !seen.insert(scroll_node.id.clone()) {
                    return Err(duplicate_leaf_id_error(what, "scroll", &scroll_node.id));
                }
                let context = format!("{what} › scroll#{}", scroll_node.id);
                walk(&context, &scroll_node.content, seen)
            }
            // A `"style"` wrapper carries no id of its own -- it's purely
            // a paint-time overlay around whatever node it wraps, so this
            // walk passes straight through to it.
            LayoutNode::Styled(styled) => walk(what, &styled.node, seen),
        }
    }
    let mut seen = HashSet::new();
    walk(what, node, &mut seen)
}

#[cfg(test)]
mod tests {
    use quadraui::Button;

    use super::*;

    #[test]
    fn layout_child_new_defaults_to_equal_share_with_no_min_max() {
        let child = LayoutChild::new(LayoutNode::Row(LayoutContainer::new(
            Vec::new(),
            Spacing::default(),
        )));
        assert_eq!(child.size, SectionSize::EqualShare);
        assert_eq!(child.min, None);
        assert_eq!(child.max, None);
    }

    #[test]
    fn style_override_default_is_empty() {
        assert!(StyleOverride::default().is_empty());
    }

    #[test]
    fn style_override_with_any_field_set_is_not_empty() {
        let over = StyleOverride {
            bg: Some(quadraui::Color::rgb(1, 2, 3)),
            ..StyleOverride::default()
        };
        assert!(!over.is_empty());
    }

    #[test]
    fn style_override_apply_theme_only_touches_named_fields() {
        let base = Theme::default();
        let over = StyleOverride {
            bg: Some(quadraui::Color::rgb(10, 20, 30)),
            ..StyleOverride::default()
        };
        let derived = over.apply_theme(base);
        assert_eq!(derived.background, quadraui::Color::rgb(10, 20, 30));
        // Every other field passes through unchanged.
        assert_eq!(derived.foreground, base.foreground);
        assert_eq!(derived.border_fg, base.border_fg);
        assert_eq!(derived.accent_fg, base.accent_fg);
    }

    #[test]
    fn style_override_apply_theme_covers_the_alpha_fields() {
        let base = Theme::default();
        let over = StyleOverride {
            selection_alpha: Some(0.9),
            ..StyleOverride::default()
        };
        let derived = over.apply_theme(base);
        assert_eq!(derived.selection_alpha, 0.9);
        // Every other alpha field, and every colour field, passes through
        // unchanged.
        assert_eq!(derived.cursor_normal_alpha, base.cursor_normal_alpha);
        assert_eq!(derived.yank_highlight_alpha, base.yank_highlight_alpha);
        assert_eq!(derived.background, base.background);
    }

    #[test]
    fn style_override_apply_style_only_touches_named_fields() {
        let base = Style::default();
        let over = StyleOverride {
            corner_radius: Some(99.0),
            ..StyleOverride::default()
        };
        let derived = over.apply_style(base);
        assert_eq!(derived.corner_radius, 99.0);
        assert_eq!(derived.padding, base.padding);
        assert_eq!(derived.border_width, base.border_width);
    }

    /// Font tokens are a plain part of the override, round-tripping
    /// through construction just like any colour/metric field —
    /// `apply_theme`/`apply_style` never touch them (no matching
    /// `Theme`/`Style` field exists), so they are only observable by
    /// reading them straight off the override.
    #[test]
    fn style_override_carries_font_tokens() {
        let over = StyleOverride {
            font_family: Some("Fira Code".to_string()),
            font_size: Some(14.0),
            font_weight: Some(FontWeight::Bold),
            ..StyleOverride::default()
        };
        assert_eq!(over.font_family, Some("Fira Code".to_string()));
        assert_eq!(over.font_size, Some(14.0));
        assert_eq!(over.font_weight, Some(FontWeight::Bold));
        assert!(!over.is_empty());
    }

    /// [`StyleOverride::resolve`] with no active state returns the base
    /// override unchanged.
    #[test]
    fn resolve_with_no_states_returns_base_unchanged() {
        let base = StyleOverride {
            bg: Some(quadraui::Color::rgb(10, 10, 10)),
            ..StyleOverride::default()
        };
        assert_eq!(base.resolve(&[]), base);
    }

    /// A `hover` branch's tokens win over the base's once the leaf is
    /// hovered; a token the branch doesn't name still falls through to
    /// the base.
    #[test]
    fn resolve_hovered_applies_the_hover_branch_over_the_base() {
        let base = StyleOverride {
            bg: Some(quadraui::Color::rgb(10, 10, 10)),
            fg: Some(quadraui::Color::rgb(20, 20, 20)),
            hover: Some(Box::new(StyleOverride {
                bg: Some(quadraui::Color::rgb(99, 99, 99)),
                ..StyleOverride::default()
            })),
            ..StyleOverride::default()
        };
        let resolved = base.resolve(&[InteractionState::Hovered]);
        assert_eq!(resolved.bg, Some(quadraui::Color::rgb(99, 99, 99)));
        // `fg` wasn't named by the hover branch, so the base's own value
        // falls through.
        assert_eq!(resolved.fg, Some(quadraui::Color::rgb(20, 20, 20)));
    }

    /// With no matching branch set, resolving for a state the override
    /// never named returns the base unchanged — a style that only
    /// defines `hover` has nothing special to say about `focus`.
    #[test]
    fn resolve_falls_through_to_base_when_the_matching_branch_is_unset() {
        let base = StyleOverride {
            bg: Some(quadraui::Color::rgb(10, 10, 10)),
            hover: Some(Box::new(StyleOverride {
                bg: Some(quadraui::Color::rgb(99, 99, 99)),
                ..StyleOverride::default()
            })),
            ..StyleOverride::default()
        };
        assert_eq!(base.resolve(&[InteractionState::Focused]), base);
    }

    /// A style that only defines `hover` keeps its hover tint once the
    /// mouse goes down — `pressed` outranks `hovered` only when `pressed`
    /// itself has a branch set; with none set, resolution falls through
    /// to the next-ranked active state instead of losing the tint
    /// entirely.
    #[test]
    fn resolve_falls_through_to_hover_when_pressed_has_no_branch_of_its_own() {
        let base = StyleOverride {
            bg: Some(quadraui::Color::rgb(0, 0, 0)),
            hover: Some(Box::new(StyleOverride {
                bg: Some(quadraui::Color::rgb(10, 10, 10)),
                ..StyleOverride::default()
            })),
            ..StyleOverride::default()
        };
        let resolved = base.resolve(&[InteractionState::Pressed, InteractionState::Hovered]);
        assert_eq!(resolved.bg, Some(quadraui::Color::rgb(10, 10, 10)));
    }

    /// `pressed` outranks `hovered`, which outranks `focused`, when more
    /// than one state is active at once — the ordinary
    /// mouse-down-while-hovering case must read as pressed, not hover.
    #[test]
    fn resolve_prefers_pressed_over_hovered_over_focused() {
        let base = StyleOverride {
            bg: Some(quadraui::Color::rgb(0, 0, 0)),
            hover: Some(Box::new(StyleOverride {
                bg: Some(quadraui::Color::rgb(10, 10, 10)),
                ..StyleOverride::default()
            })),
            pressed: Some(Box::new(StyleOverride {
                bg: Some(quadraui::Color::rgb(20, 20, 20)),
                ..StyleOverride::default()
            })),
            focus: Some(Box::new(StyleOverride {
                bg: Some(quadraui::Color::rgb(30, 30, 30)),
                ..StyleOverride::default()
            })),
            ..StyleOverride::default()
        };
        let resolved = base.resolve(&[
            InteractionState::Focused,
            InteractionState::Hovered,
            InteractionState::Pressed,
        ]);
        assert_eq!(resolved.bg, Some(quadraui::Color::rgb(20, 20, 20)));

        let resolved = base.resolve(&[InteractionState::Focused, InteractionState::Hovered]);
        assert_eq!(resolved.bg, Some(quadraui::Color::rgb(10, 10, 10)));
    }

    /// `merged_over`'s `hover`/`pressed`/`focus` branches merge
    /// recursively: a `"class"` list's `hover` tokens and a node's own
    /// inline `"style"`'s `hover` tokens combine rather than one
    /// replacing the other wholesale.
    #[test]
    fn merged_over_combines_hover_branches_instead_of_replacing() {
        let class_set = StyleOverride {
            hover: Some(Box::new(StyleOverride {
                bg: Some(quadraui::Color::rgb(1, 1, 1)),
                fg: Some(quadraui::Color::rgb(2, 2, 2)),
                ..StyleOverride::default()
            })),
            ..StyleOverride::default()
        };
        let inline = StyleOverride {
            hover: Some(Box::new(StyleOverride {
                bg: Some(quadraui::Color::rgb(9, 9, 9)),
                ..StyleOverride::default()
            })),
            ..StyleOverride::default()
        };
        let merged = inline.merged_over(class_set);
        let hover = merged.hover.expect("hover branch must survive the merge");
        // Inline's own `bg` wins...
        assert_eq!(hover.bg, Some(quadraui::Color::rgb(9, 9, 9)));
        // ...but the class's `fg` (which inline's hover branch never
        // named) still comes through.
        assert_eq!(hover.fg, Some(quadraui::Color::rgb(2, 2, 2)));
    }

    /// A registered style set round-trips its own `hover` branch through
    /// `StyleSets::define`/`get`, and that branch survives
    /// `StyleSets::resolve`'s own merge — a `"class"` reference carries
    /// state variants exactly like a node's inline `"style"` key does.
    #[test]
    fn named_style_set_round_trips_its_hover_branch() {
        let mut sets = StyleSets::new();
        sets.define(
            "card",
            StyleOverride {
                bg: Some(quadraui::Color::rgb(5, 5, 5)),
                hover: Some(Box::new(StyleOverride {
                    bg: Some(quadraui::Color::rgb(50, 50, 50)),
                    ..StyleOverride::default()
                })),
                ..StyleOverride::default()
            },
        );
        let theme = Theme::default();

        let got = sets.get("card", &theme).expect("just-defined set exists");
        let hover = got.hover.expect("hover branch must round-trip through get");
        assert_eq!(hover.bg, Some(quadraui::Color::rgb(50, 50, 50)));

        let resolved = sets.resolve(&["card".to_string()], &theme);
        let hover = resolved
            .hover
            .as_deref()
            .expect("hover branch must survive resolve's merge");
        assert_eq!(hover.bg, Some(quadraui::Color::rgb(50, 50, 50)));
        // The resolved set's own hover-state tokens apply through
        // `StyleOverride::resolve` the same way any other override's do.
        assert_eq!(
            resolved.resolve(&[InteractionState::Hovered]).bg,
            Some(quadraui::Color::rgb(50, 50, 50))
        );
    }

    #[test]
    fn validate_unique_leaf_ids_passes_through_a_styled_wrapper() {
        let node = LayoutNode::Styled(LayoutStyled {
            style: Box::new(StyleOverride {
                bg: Some(quadraui::Color::rgb(1, 1, 1)),
                ..StyleOverride::default()
            }),
            node: Box::new(LayoutNode::Leaf(LayoutLeaf {
                id: "a".to_string(),
                descriptor: LeafDescriptor::Widget {
                    kind: "button",
                    widget: Rc::new(Button::new(quadraui::WidgetId::new("a"), "A")),
                },
                scroll_px: None,
            })),
        });
        assert!(validate_unique_leaf_ids("test", &node).is_ok());
    }

    /// A repeated leaf id is a [`DecodeError::Borrow`] naming the offending
    /// node's own path, not just the bare id string -- `"test ›
    /// children[1] › leaf#a"`, not just `"a"`, so a binding can tell which
    /// of two leaves sharing an id is the second (colliding) one.
    #[test]
    fn validate_unique_leaf_ids_names_the_colliding_nodes_path() {
        let leaf = |id: &str| {
            LayoutNode::Leaf(LayoutLeaf {
                id: id.to_string(),
                descriptor: LeafDescriptor::Widget {
                    kind: "button",
                    widget: Rc::new(Button::new(quadraui::WidgetId::new(id), id)),
                },
                scroll_px: None,
            })
        };
        let node = LayoutNode::Row(LayoutContainer {
            children: vec![LayoutChild::new(leaf("a")), LayoutChild::new(leaf("a"))],
            spacing: Spacing::default(),
            justify: Justify::default(),
            align: Align::default(),
            wrap: false,
        });
        let error = validate_unique_leaf_ids("test", &node).unwrap_err();
        assert_eq!(error.code(), "borrow");
        assert_eq!(error.path(), "test › children[1] › leaf#a");
        assert!(
            error.to_string().contains("duplicate leaf id \"a\""),
            "{error}"
        );
    }

    /// The same collision, but found several levels deep inside a
    /// `split`/`sidebar_panel` nesting -- every ancestor's own `kind#id`
    /// accumulates into the error's path, so it names the whole subtree
    /// the collision was found in, not just the colliding leaf hung
    /// directly off the tree's root label.
    #[test]
    fn validate_unique_leaf_ids_accumulates_every_ancestors_path() {
        let leaf = |id: &str| {
            LayoutNode::Leaf(LayoutLeaf {
                id: id.to_string(),
                descriptor: LeafDescriptor::Widget {
                    kind: "button",
                    widget: Rc::new(Button::new(quadraui::WidgetId::new(id), id)),
                },
                scroll_px: None,
            })
        };
        let panel = LayoutNode::SidebarPanel(LayoutSidebarPanel {
            sidebar_panel: quadraui::SidebarPanel {
                id: quadraui::WidgetId::new("chrome"),
                toolbar: None,
                toolbar_height: None,
            },
            content: Box::new(leaf("dup")),
        });
        let split = LayoutNode::Split(LayoutSplit {
            split: quadraui::Split {
                id: quadraui::WidgetId::new("body"),
                direction: quadraui::SplitDirection::Horizontal,
                ratio: 0.5,
                first_min: 0.0,
                second_min: 0.0,
            },
            first: Box::new(leaf("dup")),
            second: Box::new(panel),
            spacing: Spacing::default(),
        });
        let error = validate_unique_leaf_ids("render", &split).unwrap_err();
        assert_eq!(error.code(), "borrow");
        assert_eq!(
            error.path(),
            "render › split#body › sidebar_panel#chrome › leaf#dup"
        );
    }
}
