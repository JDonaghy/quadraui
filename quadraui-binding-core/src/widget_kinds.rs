//! The kinds a layout leaf or a flat render dict decodes into a generic
//! [`crate::layout::LeafDescriptor::Widget`], and the runtime registration
//! a binding (or any crate linking this one) adds its own kinds through.

use std::rc::Rc;

use quadraui::primitive::DynWidget;

/// The [`quadraui::widget_kinds!`] kinds this crate decodes into a generic
/// [`crate::layout::LeafDescriptor::Widget`]: every static kind except the
/// ones a layout leaf still carries in a fixed [`crate::layout::LeafDescriptor`]
/// slot (`"text_display"`, `"list"`, `"tree"`, `"data_table"`) or as a
/// container node (`"sidebar_panel"`). [`decode_widget_leaf`] decodes each
/// through [`quadraui::decode_widget`], and each is painted, hit-tested and
/// routed through the widget's own `Primitive`/`Interactive` contract, its
/// `Interactive::State` kept across frames by
/// [`crate::engine::InteractionEngine`].
pub const WIDGET_KINDS: [&str; 43] = [
    "form",
    "menu_bar",
    "tree_table",
    "tree_table_controller",
    "text_input",
    "text_field",
    "password",
    "textarea",
    "toolbar",
    "number_input",
    "combo_box",
    "date_picker",
    "progress",
    "spinner",
    "scrollbar",
    "command_line",
    "message_list",
    "multi_section_view",
    "find_replace",
    "icon_grid",
    "cell_grid",
    "switch",
    "radio_group",
    "button",
    "command_center",
    "terminal",
    "status_bar",
    "activity_bar",
    "palette",
    "context_menu",
    "editor",
    "minimap",
    "image",
    "chart",
    "canvas",
    "scroll_view",
    "board",
    "pipeline_view",
    "diff_view",
    "checkbox",
    "slider",
    "select",
    "markdown",
];

/// Every kind a layout leaf or a flat render dict decodes into a generic
/// [`crate::layout::LeafDescriptor::Widget`]: [`WIDGET_KINDS`], then every
/// kind registered at runtime whose name is not a reserved node key (see
/// [`crate::wire::is_reserved_node_key`]). A binding (or any crate linking
/// this one) that registers a kind through [`register_widget_kind`] gets it
/// decoded, painted and routed with no table edit here.
pub fn widget_leaf_kinds() -> Vec<&'static str> {
    crate::scripted::ensure_scripted_widget_kind();
    let mut kinds = WIDGET_KINDS.to_vec();
    kinds.extend(
        quadraui::widget_kinds::runtime_widget_kinds()
            .into_iter()
            .filter(|kind| {
                !crate::wire::is_reserved_node_key(kind) && !WIDGET_KINDS.contains(kind)
            }),
    );
    kinds
}

/// Registers a widget kind a binding defines at runtime, so a layout leaf
/// `{"id": .., kind: {..}}` decodes through `decode` into a generic
/// [`crate::layout::LeafDescriptor::Widget`]. Fails with
/// [`quadraui::widget_kinds::KindError::Duplicate`] when `kind` is a
/// reserved node key ([`crate::wire::is_reserved_node_key`]) as well as
/// when [`quadraui::register_widget_kind`] itself refuses it, so a name
/// that layout decoding would shadow is rejected rather than silently
/// never used.
pub fn register_widget_kind(
    kind: &'static str,
    decode: quadraui::widget_kinds::DecodeFn,
) -> Result<(), quadraui::widget_kinds::KindError> {
    reject_reserved_kind(kind)?;
    quadraui::register_widget_kind(kind, decode)
}

/// [`register_widget_kind`] plus the JSON Schema of the kind's payload,
/// which [`crate::wire::schema::wire_schema`] lists alongside the static kinds.
pub fn register_widget_kind_with_schema(
    kind: &'static str,
    decode: quadraui::widget_kinds::DecodeFn,
    schema: quadraui::widget_kinds::SchemaFn,
) -> Result<(), quadraui::widget_kinds::KindError> {
    reject_reserved_kind(kind)?;
    quadraui::register_widget_kind_with_schema(kind, decode, schema)
}

fn reject_reserved_kind(kind: &str) -> Result<(), quadraui::widget_kinds::KindError> {
    crate::scripted::ensure_scripted_widget_kind();
    if crate::wire::is_reserved_node_key(kind) {
        return Err(quadraui::widget_kinds::KindError::Duplicate(
            kind.to_string(),
        ));
    }
    Ok(())
}

/// Decodes one [`WIDGET_KINDS`] leaf dict body into a type-erased
/// [`DynWidget`], routed through `diagnostics`, so an unrecognized key inside `value` is a hard
/// `unknown_key` error under `strict` and a one-shot warning under the
/// default policy rather than being silently ignored.
pub(crate) fn decode_widget_leaf(
    diagnostics: &crate::wire::DecodeDiagnostics,
    what: &str,
    kind: &str,
    value: &serde_json::Value,
) -> Result<Rc<dyn DynWidget>, crate::DecodeError> {
    crate::wire::decode_widget_checked(diagnostics, what, kind, value).map(Rc::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hit::PaintedHit;
    use crate::layout::{LayoutLeaf, LeafDescriptor};
    use quadraui::tui::testing::TuiDriver;
    use quadraui::StyledText;
    use quadraui::{AppLogic, FormEvent, Reaction, Toolbar, ToolbarButton, TreeTable, UiEvent};
    use quadraui::{Backend, Point, Rect, WidgetId};
    use std::cell::RefCell;

    /// Records every event an [`EventSink`] receives, in order, for a
    /// chrome-leaf test to assert against without any host-language
    /// callback in scope.
    #[derive(Default)]
    struct RecordingSink {
        dispatched: Vec<UiEvent>,
    }

    impl crate::engine::EventSink for RecordingSink {
        fn dispatch(
            &mut self,
            event: &UiEvent,
            _hit_detail: Option<&crate::hit::HitDetail>,
        ) -> Reaction {
            self.dispatched.push(event.clone());
            Reaction::Continue
        }
    }

    /// A `Switch` placed as a `LeafDescriptor::Widget` paints through
    /// [`quadraui::DrawsPrimitives::draw_dyn`] and a `MouseDown` landing
    /// on it round-trips through
    /// [`crate::engine::InteractionEngine::handle`]'s `HitDetail::WidgetBody`
    /// arm into [`quadraui::primitive::DynWidget::handle`], which — unlike
    /// [`IconGrid`]'s own no-op — resolves [`Switch::click`] and comes back
    /// out as a real `UiEvent::SwitchToggled`, substituted for the raw
    /// `MouseDown` entirely (see [`InteractionEngine::dispatch_dyn_widget_click`]'s
    /// own doc for why a resolved event clears the attached hit detail).
    #[test]
    fn switch_leaf_click_toggles_through_the_dyn_widget_seam() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
        use quadraui::{Modifiers, MouseButton, Switch};

        struct SwitchLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for SwitchLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "sw:1".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "switch",
                            widget: Rc::new(Switch::new(WidgetId::new("sw:1"))),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                self.engine.set_hits(hits);
            }

            fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = SwitchLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        let switch_area = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Widget { id, area, .. } if *id == WidgetId::new("sw:1") => {
                        Some(*area)
                    }
                    _ => None,
                })
                .expect("switch leaf should have painted a Widget hit")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(switch_area.x, switch_area.y),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::SwitchToggled { widget, value: true }
                    if *widget == WidgetId::new("sw:1")
            )),
            "expected a SwitchToggled event for the inline switch leaf, got {dispatched:?}"
        );
    }

    /// A `Toolbar` placed directly as a `LayoutNode::Leaf` is a generic
    /// widget leaf: a `MouseDown` on its button round-trips through
    /// [`crate::engine::InteractionEngine::handle`] into the toolbar's own
    /// `Interactive::handle`, which reports
    /// `UiEvent::Toolbar(.., ToolbarEvent::ButtonClicked)`.
    #[test]
    fn toolbar_leaf_click_emits_button_clicked_through_the_interaction_engine() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
        use quadraui::{Modifiers, MouseButton, ToolbarEvent};

        struct ToolbarLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for ToolbarLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let toolbar = Toolbar::new(WidgetId::new("toolbar:main")).with_buttons(vec![
                    ToolbarButton::Action {
                        id: WidgetId::new("toolbar:save"),
                        label: "Save".into(),
                        icon: None,
                        key_hint: None,
                        enabled: true,
                        is_active: false,
                        tooltip: String::new(),
                        spec: None,
                    },
                ]);
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "toolbar:main".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "toolbar",
                            widget: Rc::new(toolbar),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                crate::host::render_overlays(&self.engine, backend, hits);
            }

            fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = ToolbarLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        let button_point = toolbar_button_points(5)
            .into_iter()
            .next()
            .expect("toolbar leaf should have painted its button");

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: button_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Toolbar(id, ToolbarEvent::ButtonClicked { id: button_id, .. })
                    if *id == WidgetId::new("toolbar:main")
                        && *button_id == WidgetId::new("toolbar:save")
            )),
            "expected a ButtonClicked event for the inline toolbar leaf, got {dispatched:?}"
        );
    }

    /// The first painted point of each distinct `"toolbar:main"` button
    /// on row 0, scanning `width` columns of the painted widget store.
    fn toolbar_button_points(width: usize) -> Vec<Point> {
        let mut seen = Vec::new();
        let mut points = Vec::new();
        for x in 0..width {
            let point = Point::new(x as f32, 0.0);
            if let Some((id, Some(sub))) = quadraui::primitive::painted_hit(point.x, point.y) {
                if id == WidgetId::new("toolbar:main") && !seen.contains(&sub) {
                    seen.push(sub);
                    points.push(point);
                }
            }
        }
        points
    }

    /// A toolbar leaf paints through `draw_dyn`, so it is recorded in the
    /// painted widget store under its own id with one sub-part per button,
    /// and [`crate::engine::InteractionEngine::handle`] feeds a pointer move
    /// over a button into the ambient pointer state, asking for a redraw
    /// when the hovered button changes — also for a host that never runs
    /// the move through a runner of its own.
    #[test]
    fn toolbar_extra_is_hit_tested_by_the_ambient_pointer_state() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;

        struct ToolbarLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for ToolbarLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let save = |id: &str| ToolbarButton::Action {
                    id: WidgetId::new(id),
                    label: "Save".into(),
                    icon: None,
                    key_hint: None,
                    enabled: true,
                    is_active: false,
                    tooltip: String::new(),
                    spec: None,
                };
                let toolbar = Toolbar::new(WidgetId::new("toolbar:main"))
                    .with_buttons(vec![save("toolbar:save"), save("toolbar:keep")]);
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "toolbar:main".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "toolbar",
                            widget: Rc::new(toolbar),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                crate::host::render_overlays(&self.engine, backend, hits);
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = ToolbarLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 30, 5);
        quadraui::interaction::reset_ambient_interaction();

        let button_points = toolbar_button_points(30);
        assert_eq!(button_points.len(), 2);

        let painted = |p: Point| quadraui::primitive::painted_hit(p.x, p.y);
        let first = painted(button_points[0]).expect("first button is in the painted store");
        let second = painted(button_points[1]).expect("second button is in the painted store");
        assert_eq!(first.0, WidgetId::new("toolbar:main"));
        assert!(first.1.is_some() && second.1.is_some() && first.1 != second.1);

        let mut move_to = |p: Point| {
            let (backend, app) = driver.parts_mut();
            let sink = &mut *app.sink.borrow_mut();
            app.engine.handle(
                UiEvent::MouseMoved {
                    position: p,
                    buttons: Default::default(),
                },
                backend,
                sink,
            )
        };
        assert_eq!(move_to(button_points[0]), Reaction::Redraw);
        assert_eq!(
            quadraui::interaction::ambient_interaction().hovered_target(),
            Some(first)
        );
        assert_eq!(move_to(button_points[1]), Reaction::Redraw);
        assert_eq!(
            quadraui::interaction::ambient_interaction().hovered_target(),
            Some(second)
        );
    }

    /// An `IconGrid` placed as a `LeafDescriptor::Widget` paints through
    /// [`quadraui::DrawsPrimitives::draw_dyn`] and records a
    /// [`PaintedHit::Widget`], proving the generic seam carries a click
    /// decode-to-paint-to-hit-test-to-dispatch end to end: a `MouseDown`
    /// landing on the painted area resolves [`crate::hit::HitDetail::WidgetBody`] (not some
    /// fallback "unrecognised hit" shape), and
    /// [`crate::engine::InteractionEngine::handle`] routes it into
    /// [`quadraui::primitive::DynWidget::handle`] rather than skipping
    /// dispatch outright — observable here because
    /// [`IconGrid::handle`][quadraui::primitive::DynWidget::handle] always
    /// returns [`quadraui::Handled::Ignored`] (`IconGrid` has no
    /// `Interactive` override), so the dispatch falls through to the same
    /// raw `MouseDown` [`RecordingSink::dispatch`] would have seen without
    /// this seam — but now carrying `Some(&HitDetail::WidgetBody)`, which
    /// only happens once [`InteractionEngine::handle`] has actually walked
    /// the `dispatch_dyn_widget_click` branch rather than treating the hit
    /// as unrecognised.
    #[test]
    fn icon_grid_leaf_click_round_trips_through_the_dyn_widget_seam() {
        use crate::engine::InteractionEngine;
        use crate::hit::HitDetail;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
        use quadraui::{IconGrid, IconGridItem, Modifiers, MouseButton};

        /// Unlike [`RecordingSink`], also keeps the [`HitDetail`] each
        /// dispatch carried, so the test can assert the click resolved to
        /// [`HitDetail::WidgetBody`] specifically, not merely that some
        /// event arrived.
        #[derive(Default)]
        struct DetailRecordingSink {
            dispatched: Vec<(UiEvent, Option<HitDetail>)>,
        }

        impl crate::engine::EventSink for DetailRecordingSink {
            fn dispatch(&mut self, event: &UiEvent, hit_detail: Option<&HitDetail>) -> Reaction {
                self.dispatched.push((event.clone(), hit_detail.cloned()));
                Reaction::Continue
            }
        }

        struct IconGridLeafApp {
            engine: InteractionEngine,
            sink: RefCell<DetailRecordingSink>,
        }

        impl AppLogic for IconGridLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let grid = IconGrid {
                    id: WidgetId::new("grid:1"),
                    items: vec![IconGridItem {
                        id: WidgetId::new("grid:1:item-0"),
                        icon: quadraui::Icon::new("\u{f15b}", "F"),
                        image: None,
                        label: StyledText::plain("README.md"),
                        detail: None,
                        decoration: Default::default(),
                    }],
                    selected_idx: 0,
                    has_focus: true,
                    scroll_offset: 0,
                    tile_size: None,
                };
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "grid:1".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "icon_grid",
                            widget: Rc::new(grid),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                self.engine.set_hits(hits);
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = IconGridLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(DetailRecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);

        let grid_area = {
            let hits = driver.app().engine.hit_map();
            hits.iter()
                .find_map(|hit| match hit {
                    PaintedHit::Widget { id, area, .. } if *id == WidgetId::new("grid:1") => {
                        Some(*area)
                    }
                    _ => None,
                })
                .expect("icon_grid leaf should have painted a Widget hit")
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: Point::new(grid_area.x, grid_area.y),
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|(event, hit_detail)| matches!(
                (event, hit_detail),
                (UiEvent::MouseDown { .. }, Some(HitDetail::WidgetBody))
            )),
            "expected the icon_grid click to resolve HitDetail::WidgetBody \
             through the InteractionEngine, got {dispatched:?}"
        );
    }

    /// A `Markdown` leaf renders headings, bold/italic, an inline code
    /// span and a list through the shared `render_markdown_to_styled`
    /// adapter — asserted both on the raw markdown syntax having been
    /// *consumed* (every marker absent from the painted screen text) and
    /// on a `fg` distinct from plain body text for the code span, so a
    /// bypass that dumped `source` to the screen verbatim — which still
    /// contains every one of those substrings — fails this test. This
    /// proves the same renderer `push_turn_markdown` uses is reused rather
    /// than a second parser. A `MouseDown` on the link's painted row
    /// round-trips through the generic widget seam into
    /// [`quadraui::Markdown`]'s own `Interactive::handle` and out as
    /// `UiEvent::RichTextLinkClicked` — the same event `RichTextPopup`'s own
    /// link click already dispatches.
    #[test]
    fn markdown_leaf_renders_formatting_and_link_click_emits_rich_text_link_clicked() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
        use quadraui::{Modifiers, MouseButton};

        struct MarkdownLeafApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for MarkdownLeafApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let markdown = quadraui::Markdown::new(
                    WidgetId::new("md:1"),
                    "# Title\n**bold** *italic* `code`\n- item\n[docs](https://example.com/docs)",
                );
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "md:1".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "markdown",
                            widget: Rc::new(markdown),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                self.engine.set_hits(hits);
            }

            fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = MarkdownLeafApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 40, 10);
        driver.render();

        assert!(driver.screen_contains("Title"), "heading text missing");
        assert!(driver.screen_contains("bold"), "bold text missing");
        assert!(driver.screen_contains("italic"), "italic text missing");
        assert!(driver.screen_contains("code"), "code span text missing");
        assert!(driver.screen_contains("item"), "list item text missing");
        assert!(driver.screen_contains("docs"), "link text missing");

        // Every raw markdown marker must be gone from the painted screen
        // -- a renderer bypass that printed `source` verbatim would still
        // pass every `screen_contains` assertion above, since all of those
        // substrings also appear in the raw source text.
        assert!(
            !driver.screen_contains("# Title"),
            "heading marker should be stripped, not painted literally"
        );
        assert!(
            !driver.screen_contains("**bold**"),
            "bold marker should be stripped, not painted literally"
        );
        assert!(
            !driver.screen_contains("*italic*"),
            "italic marker should be stripped, not painted literally"
        );
        assert!(
            !driver.screen_contains("`code`"),
            "code-span marker should be stripped, not painted literally"
        );
        assert!(
            !driver.screen_contains("[docs]"),
            "link syntax should be stripped, not painted literally"
        );
        assert!(
            driver.screen_contains("\u{2022}"),
            "list item should paint a bullet marker"
        );

        // The adapter's own styling -- not just marker removal -- reaches
        // the screen: the code span's `fg` differs from plain body text's
        // (`Theme::accent_fg`, not the default foreground). `TextDisplay`'s
        // TUI rasteriser (`tui::text_display::draw_text_display`) paints
        // only `span.fg`/`span.bg`, not `span.bold`/`span.italic` (a
        // pre-existing TUI limitation of the primitive this leaf paints
        // through, not something this leaf can fix on its own), so bold/
        // italic have no distinct on-screen signal to assert beyond the
        // marker-stripping above.
        let (item_x, item_y) = driver
            .find("item")
            .expect("plain list text should be painted");
        let item_style = driver
            .style_at(item_x as u16, item_y as u16)
            .expect("style at plain list text");
        let (code_x, code_y) = driver.find("code").expect("code span should be painted");
        let code_style = driver
            .style_at(code_x as u16, code_y as u16)
            .expect("style at code span");
        assert_ne!(
            code_style.fg, item_style.fg,
            "code span should be coloured differently from plain body text"
        );

        let link_point = {
            let (x, y) = driver.find("docs").expect("the link line is painted");
            Point::new(x, y)
        };

        driver.dispatch(UiEvent::MouseDown {
            widget: None,
            button: MouseButton::Left,
            position: link_point,
            modifiers: Modifiers::default(),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::RichTextLinkClicked(id, url)
                    if *id == WidgetId::new("md:1") && url == "https://example.com/docs"
            )),
            "expected a RichTextLinkClicked event for the inline markdown leaf, got {dispatched:?}"
        );
    }

    /// A wheel notch over a `Markdown` leaf's body must emit
    /// `UiEvent::TextDisplay(id, TextDisplayEvent::Scrolled { .. })` — the
    /// same scroll a plain `"text_display"` leaf's own wheel reports, since
    /// the composed display handles it — since the host
    /// owns `Markdown::scroll_offset` as a controlled component and has no
    /// other way to learn a scroll gesture happened. The document has more
    /// lines than the 10-row viewport fits, so there is something to
    /// scroll.
    #[test]
    fn markdown_leaf_wheel_scroll_emits_text_display_scrolled() {
        use crate::engine::InteractionEngine;
        use crate::layout::{LayoutContainer, LayoutNode};
        use crate::render::paint_layout;
        use quadraui::Spacing;
        use quadraui::{ScrollDelta, TextDisplayEvent};

        struct MarkdownScrollApp {
            engine: InteractionEngine,
            sink: RefCell<RecordingSink>,
        }

        impl AppLogic for MarkdownScrollApp {
            type AreaId = ();

            fn render(&self, backend: &mut dyn Backend, _area: ()) {
                let source = (0..20)
                    .map(|i| format!("line {i}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let markdown = quadraui::Markdown::new(WidgetId::new("md:scroll"), source);
                let tree = LayoutNode::Row(LayoutContainer::new(
                    vec![LayoutNode::Leaf(LayoutLeaf {
                        id: "md:scroll".into(),
                        descriptor: LeafDescriptor::Widget {
                            kind: "markdown",
                            widget: Rc::new(markdown),
                        },
                        scroll_px: None,
                    })],
                    Spacing::default(),
                ));
                let viewport = backend.viewport();
                let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
                let mut hits = Vec::new();
                paint_layout(backend, area, &tree, &mut hits);
                self.engine.set_hits(hits);
            }

            fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
                self.engine.tab_stops()
            }

            fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
                self.engine
                    .handle(event, backend, &mut *self.sink.borrow_mut())
            }
        }

        let app = MarkdownScrollApp {
            engine: InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 40, 10);
        driver.render();

        driver.dispatch(UiEvent::Scroll {
            widget: None,
            delta: ScrollDelta::new(0.0, -1.0),
            position: Point::new(1.0, 1.0),
        });

        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::TextDisplay(id, TextDisplayEvent::Scrolled { new_offset: 1 })
                    if *id == WidgetId::new("md:scroll")
            )),
            "expected a TextDisplay(Scrolled {{ new_offset: 1 }}) event for a wheel notch \
             over the markdown leaf, got {dispatched:?}"
        );
    }

    /// A reusable one-leaf [`AppLogic`] for the standalone
    /// `"checkbox"`/`"slider"`/`"select"`/`"password"`/`"textarea"` leaf
    /// tests below — the same shape
    /// [`switch_leaf_click_toggles_through_the_interaction_engine`] builds
    /// by hand, generalised over any leaf descriptor so each leaf's own test
    /// only needs to build the descriptor and assert on the result.
    struct SingleLeafApp {
        id: WidgetId,
        leaf: Box<dyn Fn() -> LeafDescriptor>,
        engine: crate::engine::InteractionEngine,
        sink: RefCell<RecordingSink>,
    }

    impl AppLogic for SingleLeafApp {
        type AreaId = ();

        fn render(&self, backend: &mut dyn Backend, _area: ()) {
            use crate::layout::{LayoutContainer, LayoutNode};
            use crate::render::paint_layout;
            use quadraui::Spacing;

            let tree = LayoutNode::Row(LayoutContainer::new(
                vec![LayoutNode::Leaf(LayoutLeaf {
                    id: self.id.as_str().to_string(),
                    descriptor: (self.leaf)(),
                    scroll_px: None,
                })],
                Spacing::default(),
            ));
            let viewport = backend.viewport();
            let area = Rect::new(0.0, 0.0, viewport.width, viewport.height);
            let mut hits = Vec::new();
            paint_layout(backend, area, &tree, &mut hits);
            self.engine.set_hits(hits);
        }

        fn extra_tab_stops(&self, _area: ()) -> Vec<(WidgetId, Rect)> {
            self.engine.tab_stops()
        }

        fn handle(&mut self, event: UiEvent, backend: &mut dyn Backend) -> Reaction {
            self.engine
                .handle(event, backend, &mut *self.sink.borrow_mut())
        }
    }

    /// Renders a [`WIDGET_KINDS`] leaf of `kind` as the sole leaf of a
    /// `20x5` driver and returns it already focused (via a leading `Tab`,
    /// the same mechanism
    /// [`tab_focuses_a_toolbar_and_enter_activates_its_focused_button`]
    /// (`crate::engine`'s own tests) uses) — every leaf below is its
    /// layout tree's only tab stop, so one `Tab` always lands focus on it.
    fn single_widget_driver(
        id: WidgetId,
        kind: &'static str,
        widget: Rc<dyn DynWidget>,
    ) -> TuiDriver<SingleLeafApp> {
        leaf_driver(
            id,
            Box::new(move || LeafDescriptor::Widget {
                kind,
                widget: widget.clone(),
            }),
        )
    }

    fn leaf_driver(
        id: WidgetId,
        leaf: Box<dyn Fn() -> LeafDescriptor>,
    ) -> TuiDriver<SingleLeafApp> {
        let app = SingleLeafApp {
            id,
            leaf,
            engine: crate::engine::InteractionEngine::new(),
            sink: RefCell::new(RecordingSink::default()),
        };
        let mut driver = TuiDriver::new(app, 20, 5);
        driver.press_named(quadraui::NamedKey::Tab);
        driver.app().sink.borrow_mut().dispatched.clear();
        driver
    }

    /// The absolute area the leaf `id` painted into this frame, read back
    /// from the engine's own hit map the same way
    /// [`switch_leaf_click_toggles_through_the_interaction_engine`] does.
    fn leaf_area(driver: &TuiDriver<SingleLeafApp>, id: WidgetId) -> Rect {
        driver
            .app()
            .engine
            .hit_map()
            .iter()
            .find_map(|hit| match hit {
                PaintedHit::Widget {
                    id: hit_id, area, ..
                } if *hit_id == id => Some(*area),
                _ => None,
            })
            .expect("leaf should have painted a Widget hit")
    }

    /// Render shows the unchecked glyph; a click toggles it (mouse); Space
    /// toggles it again (keyboard) — both routed through the generic
    /// widget seam into [`quadraui::Checkbox`]'s own `Interactive::handle`,
    /// both reaching the handler as [`FormEvent::ToggleChanged`]. The
    /// `[ ]`/`[x]` glyph is the form's own `FieldKind::Toggle` painter.
    #[test]
    fn checkbox_leaf_renders_clicks_and_keys_toggle_through_the_interaction_engine() {
        let id = WidgetId::new("cb:1");
        let checkbox = || -> Rc<dyn DynWidget> {
            let mut checkbox = quadraui::Checkbox::new(id);
            checkbox.focused = true;
            Rc::new(checkbox)
        };

        let mut driver = single_widget_driver(id, "checkbox", checkbox());
        assert!(driver.screen_contains("[ ]"), "{}", driver.screen());

        let area = leaf_area(&driver, id);
        driver.click(area.x, area.y);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::ToggleChanged { value: true, .. })
                    if *ev_id == id
            )),
            "click should emit ToggleChanged{{value: true}}: {dispatched:?}"
        );

        // A fresh driver, same starting `value: false` — the engine only
        // ever sees the last-painted snapshot (the host echoes the new
        // value back on its next render, which this single-frame test
        // never does), so Space is exercised against the same starting
        // state the click above was, not the click's own outcome.
        let mut driver = single_widget_driver(id, "checkbox", checkbox());
        driver.type_char(' ');
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::ToggleChanged { value: true, .. })
                    if *ev_id == id
            )),
            "Space should emit ToggleChanged{{value: true}}: {dispatched:?}"
        );
    }

    /// Right/Up (keyboard) steps the value up by `step`; a click (mouse)
    /// maps the click's x-position along the painted track to a value —
    /// reaching the handler as [`FormEvent::SliderChanged`] either way.
    #[test]
    fn slider_leaf_renders_clicks_and_keys_step_through_the_interaction_engine() {
        let id = WidgetId::new("sl:1");
        let mut slider = quadraui::Slider::new(id, 0.0, 0.0, 10.0);
        slider.focused = true;
        let mut driver = single_widget_driver(id, "slider", Rc::new(slider));

        // The last track cell, so the mapped fraction is close to (but
        // short of) `1.0` — proving the click landed where the pointer
        // was, not just "somewhere on the body".
        let (close_x, close_y) = driver.find("]").expect("the track's closing bracket");
        driver.click(close_x - 1.0, close_y);
        {
            let dispatched = &driver.app().sink.borrow().dispatched;
            assert!(
                dispatched.iter().any(|event| matches!(
                    event,
                    UiEvent::Form(ev_id, FormEvent::SliderChanged { value, .. })
                        if *ev_id == id && *value > 9.0 && *value < 10.0
                )),
                "a click near the track's right edge should emit SliderChanged{{value: ~9.5}}: {dispatched:?}"
            );
        }
        driver.app().sink.borrow_mut().dispatched.clear();

        driver.press_named(quadraui::NamedKey::Right);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::SliderChanged { value, .. })
                    if *ev_id == id && (*value - 1.0).abs() < f32::EPSILON
            )),
            "Right should emit SliderChanged{{value: 1.0}}: {dispatched:?}"
        );
    }

    /// An unrecognised key inside a `"slider"` descriptor body -- a
    /// generic-widget leaf, reached through [`decode_widget_leaf`] rather
    /// than the six fixed leaf kinds -- is a hard [`crate::DecodeError::UnknownKey`] under
    /// `strict`, naming the key, the dict it was found on, and the keys
    /// `Slider` actually accepts, the same policy a layout-tree node's own
    /// sibling keys already get.
    #[test]
    fn slider_leaf_body_rejects_an_unknown_key_under_strict() {
        let value = serde_json::json!({
            "id": "sl:1",
            "value": 0.0,
            "min": 0.0,
            "max": 10.0,
            "stpe": 1.0,
        });
        let diagnostics = crate::wire::DecodeDiagnostics::strict();
        let error = match decode_widget_leaf(&diagnostics, "test", "slider", &value) {
            Ok(_) => panic!("a typo'd \"stpe\" key must be rejected under strict"),
            Err(err) => err,
        };
        assert_eq!(error.code(), "unknown_key");
        assert!(
            error.to_string().contains("\"stpe\""),
            "{}",
            error.to_string()
        );
    }

    /// The same typo'd key [`slider_leaf_body_rejects_an_unknown_key_under_strict`]
    /// rejects under `strict` only warns once, through the registered hook,
    /// under the default (warn) policy -- the decode still succeeds despite
    /// the unrecognised key.
    #[test]
    fn slider_leaf_body_warns_once_under_default_policy() {
        let value = serde_json::json!({
            "id": "sl:1",
            "value": 0.0,
            "min": 0.0,
            "max": 10.0,
            "stpe": 1.0,
        });
        let diagnostics = crate::wire::DecodeDiagnostics::warn();
        if let Err(err) = decode_widget_leaf(&diagnostics, "test", "slider", &value) {
            panic!("warn mode still decodes despite the unrecognised key: {err}");
        }
        assert_eq!(
            diagnostics.warned_keys(),
            std::collections::HashSet::from(["stpe".to_string()])
        );
    }

    /// Down (keyboard) and a click (mouse) both move to the next option,
    /// committing immediately — reaching the handler as
    /// [`FormEvent::DropdownChanged`], mirroring `FieldKind::Dropdown`'s
    /// own "no separate open step" contract.
    #[test]
    fn select_leaf_renders_clicks_and_keys_move_through_the_interaction_engine() {
        let id = WidgetId::new("se:1");
        let select = || -> Rc<dyn DynWidget> {
            let mut select = quadraui::Select::new(
                id,
                vec!["One".to_string(), "Two".to_string(), "Three".to_string()],
            );
            select.focused = true;
            Rc::new(select)
        };

        let mut driver = single_widget_driver(id, "select", select());
        assert!(driver.screen_contains("One"), "{}", driver.screen());

        let area = leaf_area(&driver, id);
        driver.click(area.x, area.y);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::DropdownChanged { selected_idx: 1, .. })
                    if *ev_id == id
            )),
            "click should emit DropdownChanged{{selected_idx: 1}}: {dispatched:?}"
        );

        // A fresh driver at the same starting `selected_idx: 0` — see the
        // identical comment in the checkbox test above for why this
        // doesn't chain off the click's own outcome.
        let mut driver = single_widget_driver(id, "select", select());
        driver.press_named(quadraui::NamedKey::Down);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::DropdownChanged { selected_idx: 1, .. })
                    if *ev_id == id
            )),
            "Down should emit DropdownChanged{{selected_idx: 1}}: {dispatched:?}"
        );
    }

    /// Typing a character is the real interactive path for a standalone
    /// password leaf — reaching the handler as
    /// [`FormEvent::TextInputChanged`] through the generic widget leaf's
    /// own `Interactive::handle`. The render assertion checks the masked
    /// glyph appears and the plaintext never does.
    #[test]
    fn password_leaf_renders_masked_and_typing_reaches_the_interaction_engine() {
        let id = WidgetId::new("pw:1");
        let mut password = quadraui::PasswordInput::new(id);
        password.value = "hi".to_string();
        password.has_focus = true;
        password.mask_char = '*';
        let mut driver = single_widget_driver(id, "password", Rc::new(password));
        assert!(driver.screen_contains("**"), "{}", driver.screen());
        assert!(!driver.screen_contains("hi"), "{}", driver.screen());

        driver.type_char('!');
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hi!"
            )),
            "typing '!' should emit TextInputChanged{{value: \"hi!\"}}: {dispatched:?}"
        );
    }

    /// An IME-committed character (`CharTyped`) or a clipboard paste
    /// (`ClipboardPaste`) both reach a focused standalone password leaf,
    /// and a newline embedded in the pasted text is dropped rather than
    /// splitting the one-line value.
    #[test]
    fn password_leaf_reaches_char_typed_and_clipboard_paste_and_strips_newlines() {
        let id = WidgetId::new("pw:2");
        let password = || -> Rc<dyn DynWidget> {
            let mut password = quadraui::PasswordInput::new(id);
            password.value = "hi".to_string();
            password.has_focus = true;
            Rc::new(password)
        };

        let mut driver = single_widget_driver(id, "password", password());
        driver.dispatch(UiEvent::CharTyped('!'));
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hi!"
            )),
            "CharTyped('!') should emit TextInputChanged{{value: \"hi!\"}}: {dispatched:?}"
        );

        let mut driver = single_widget_driver(id, "password", password());
        driver.dispatch(UiEvent::ClipboardPaste("yo\nthere".to_string()));
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hiyothere"
            )),
            "pasting \"yo\\nthere\" should strip the newline: {dispatched:?}"
        );
    }

    /// Enter inserts a newline in a standalone textarea leaf (not a
    /// commit, unlike a password leaf), reaching the handler as
    /// [`FormEvent::TextInputChanged`] with the embedded `\n`.
    #[test]
    fn text_area_leaf_renders_and_enter_inserts_a_newline_through_the_interaction_engine() {
        let id = WidgetId::new("ta:1");
        let mut area = quadraui::TextArea::new(id);
        area.value = "hi".to_string();
        area.has_focus = true;
        let mut driver = single_widget_driver(id, "textarea", Rc::new(area));
        assert!(driver.screen_contains("hi"), "{}", driver.screen());

        driver.press_named(quadraui::NamedKey::Enter);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hi\n"
            )),
            "Enter should emit TextInputChanged{{value: \"hi\\n\"}}: {dispatched:?}"
        );
    }

    /// A clipboard paste reaches a focused standalone textarea leaf with
    /// its newline kept verbatim: a textarea has no single-line invariant
    /// to protect.
    #[test]
    fn text_area_leaf_reaches_clipboard_paste_with_embedded_newline() {
        let id = WidgetId::new("ta:2");
        let mut area = quadraui::TextArea::new(id);
        area.value = "hi".to_string();
        area.has_focus = true;
        let mut driver = single_widget_driver(id, "textarea", Rc::new(area));

        driver.dispatch(UiEvent::ClipboardPaste(" there\nfriend".to_string()));
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                    if *ev_id == id && value == "hi there\nfriend"
            )),
            "pasting \" there\\nfriend\" should keep its newline: {dispatched:?}"
        );
    }

    /// A focused `"text_input"` leaf edits through the shared text core:
    /// Ctrl+Backspace deletes the word before the caret, Ctrl+Z undoes it
    /// and Ctrl+Shift+Z redoes it, with the caret the widget's own state
    /// walked to.
    #[test]
    fn text_input_leaf_word_delete_undo_and_redo_run_through_the_core_state() {
        use std::cell::RefCell as Cell;
        let id = WidgetId::new("ti:1");
        // The app's own copy of the value, echoed back into every render.
        let value = Rc::new(Cell::new("foo bar".to_string()));
        let rendered = Rc::clone(&value);
        let mut driver = leaf_driver(
            id,
            Box::new(move || LeafDescriptor::Widget {
                kind: "text_input",
                widget: Rc::new(
                    quadraui::TextInput::new(id)
                        .with_lines(vec![rendered.borrow().clone()])
                        .with_cursor_col(7),
                ),
            }),
        );
        let ctrl = |shift| quadraui::Modifiers {
            ctrl: true,
            shift,
            ..Default::default()
        };
        let mut press = |key: quadraui::Key, modifiers| -> Option<String> {
            driver.app().sink.borrow_mut().dispatched.clear();
            driver.dispatch(UiEvent::KeyPressed {
                key,
                modifiers,
                repeat: false,
            });
            let changed =
                driver
                    .app()
                    .sink
                    .borrow()
                    .dispatched
                    .iter()
                    .find_map(|event| match event {
                        UiEvent::Form(ev_id, FormEvent::TextInputChanged { value, .. })
                            if *ev_id == id =>
                        {
                            Some(value.clone())
                        }
                        _ => None,
                    });
            if let Some(changed) = &changed {
                *value.borrow_mut() = changed.clone();
                driver.render();
            }
            changed
        };
        assert_eq!(
            press(
                quadraui::Key::Named(quadraui::NamedKey::Backspace),
                ctrl(false)
            )
            .as_deref(),
            Some("foo ")
        );
        assert_eq!(
            press(quadraui::Key::Char('z'), ctrl(false)).as_deref(),
            Some("foo bar")
        );
        assert_eq!(
            press(quadraui::Key::Char('z'), ctrl(true)).as_deref(),
            Some("foo ")
        );
    }

    /// A `"tree_table"` widget leaf paints a branch row's label and chevron
    /// glyph, and a click on the chevron resolves through the table's own
    /// `Interactive::handle` to
    /// [`quadraui::UiEvent::TreeTable`]`(`[`quadraui::TreeTableEvent::ToggleExpanded`]`)`.
    #[test]
    fn tree_table_extra_renders_label_and_chevron_and_passes_through_a_click() {
        let id = WidgetId::new("tt:files");
        let table = quadraui::DataTable {
            id,
            columns: vec![quadraui::Column {
                title: "Name".to_string(),
                width: Default::default(),
                align: Default::default(),
            }],
            rows: vec![
                quadraui::DataRow {
                    cells: vec![quadraui::StyledText::plain("src")],
                    decoration: Default::default(),
                },
                quadraui::DataRow {
                    cells: vec![quadraui::StyledText::plain("main.rs")],
                    decoration: Default::default(),
                },
            ],
            selected_idx: None,
            scroll_offset: 0,
            sort: None,
            has_focus: true,
            show_scrollbar: false,
            min_total_width: None,
            h_scroll: 0.0,
            column_overrides: vec![],
            footer: None,
            bordered: false,
            striped: false,
            glyphs: None,
        };
        let tree_table = TreeTable::new(
            table,
            vec![
                quadraui::TreeTableNode::new(vec![0], 0).with_expanded(true),
                quadraui::TreeTableNode::new(vec![0, 0], 1),
            ],
        );

        let mut driver = single_widget_driver(id, "tree_table", Rc::new(tree_table));
        assert!(driver.screen_contains("src"), "{}", driver.screen());
        assert!(driver.screen_contains("main.rs"), "{}", driver.screen());
        assert!(
            driver.screen_contains("▾"),
            "the expanded branch row's chevron glyph must reach the screen: {}",
            driver.screen()
        );

        let area = leaf_area(&driver, id);
        // Row 0 is the header; "src" (the only expanded branch, indent 0)
        // paints its chevron at column 0 of row 1 — see
        // `tree_table::paint_cells::tests::branch_row_paints_a_chevron_and_the_label_still_appears`.
        driver.click(area.x, area.y + 1.0);
        let dispatched = &driver.app().sink.borrow().dispatched;
        assert!(
            dispatched.iter().any(|event| matches!(
                event,
                UiEvent::TreeTable(
                    ev_id,
                    quadraui::TreeTableEvent::ToggleExpanded { path, expand: false }
                ) if *ev_id == id && path.as_slice() == [0u16]
            )),
            "a click on the chevron should resolve to TreeTableEvent::ToggleExpanded: \
                 {dispatched:?}"
        );
    }
}
