// Black-box coverage for `QuiApp.widgetState(id)` -- Node's read access to
// the uncontrolled-by-default `Tree`/`List`/`DataTable` cursor + scroll,
// `Split` ratio, `TabBar` active tab, and `Tree` expanded-set store
// `quadraui-binding-core::WidgetState` holds. Ports the same acceptance
// `quadraui-py`'s own `widget_state` test is held to: a 500-row tree with
// no `scroll_offset`/`selected_path` fields scrolls to keep an
// arrow-key-moved cursor visible, a split ratio persists after a divider
// drag, and supplying `scroll_offset` explicitly pins the scroll.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

function treeRows(count: number) {
  const rows = [];
  for (let i = 0; i < count; i++) {
    rows.push({
      path: [i],
      indent: 0,
      icon: null,
      text: { spans: [{ text: `row ${i}`, fg: null, bg: null }] },
      badge: null,
      is_expanded: null,
    });
  }
  return rows;
}

// `bordered: true` reserves the 1-cell margin `TreeView::layout` needs so
// the generic focus-ring overlay (painted once this tree is the focused
// tab stop, as every test below makes it) never overwrites a row of real
// content -- see `TreeStyle::bordered`'s own doc.
const TREE_STYLE = {
  indent: 2,
  show_chevrons: true,
  chevron_expanded: "v",
  chevron_collapsed: ">",
  bordered: true,
};

test("a 500-row tree with no scroll/selection fields scrolls to keep the cursor visible", () => {
  const app = new QuiApp(24, 8);
  // `QuiApp`'s interaction engine (hit routing, focus, nav keys) only runs
  // for a dispatched event once a handler is registered -- see
  // `NodeAppLogic::handle`. A no-op handler is enough; the assertions below
  // read the navigation's effect back through `widgetState`, not the event
  // stream.
  app.setHandler(() => {});
  app.setRender(() => ({
    tree: {
      id: "fm:tree",
      rows: treeRows(500),
      selection_mode: "Single",
      selected_path: null,
      style: TREE_STYLE,
    },
  }));
  // Force the first paint so the tree's hit-test layout is on the hit
  // map before any interaction below -- `pushEvent`/`run` only ever
  // re-render on a handler-returned `Reaction::Redraw`, never on their
  // own.
  app.screen();

  // A click inside row 0's content cell -- one in from the bordered
  // frame's top-left corner -- focuses the tree (click-to-focus, same as
  // `data_table_interactions.test.ts`'s list case) so the arrow keys below
  // route to it.
  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1, y: 1 }, modifiers: {} },
  });
  app.run(1);

  for (let i = 0; i < 40; i++) {
    app.pushEvent({
      KeyPressed: { key: { Named: "Down" }, modifiers: {}, repeat: false },
    });
  }
  app.run(40);

  // 40 Downs starting from no selection land on row index 39 (the first
  // Down selects row 0, each of the other 39 moves by one).
  const state = app.widgetState("fm:tree");
  assert.ok(state, "the tree must have resolved widget state after navigation");
  assert.deepEqual(state.selectedPath, [39]);
  assert.ok(
    state.scrollOffset > 0,
    `500 rows must have scrolled the uncontrolled view: ${JSON.stringify(state)}`,
  );
  assert.ok(app.screenContains("row 39"), "row 39 (the moved cursor) must be visible on screen");
  assert.ok(!app.screenContains("row 0"), "row 0 must have scrolled out of view");
});

test("supplying scroll_offset explicitly pins the tree's scroll", () => {
  const app = new QuiApp(24, 8);
  app.setHandler(() => {});
  app.setRender(() => ({
    tree: {
      id: "fm:tree",
      rows: treeRows(500),
      selection_mode: "Single",
      selected_path: null,
      scroll_offset: 50,
      style: TREE_STYLE,
    },
  }));
  app.screen();

  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1, y: 1 }, modifiers: {} },
  });
  app.run(1);

  for (let i = 0; i < 10; i++) {
    app.pushEvent({
      KeyPressed: { key: { Named: "Down" }, modifiers: {}, repeat: false },
    });
  }
  app.run(10);

  const state = app.widgetState("fm:tree");
  assert.ok(state, "the tree must have resolved widget state");
  assert.equal(
    state.scrollOffset,
    50,
    "a controlled scroll_offset must stay pinned at the descriptor's own value, unmoved by navigation",
  );
  assert.ok(app.screenContains("row 50"), "the pinned scroll window must start at row 50");
});

test("a split ratio persists after a drag and widgetState reports the dragged value", () => {
  const app = new QuiApp(40, 10);
  app.setHandler(() => {});
  app.setRender(() => ({
    split: {
      id: "split:main",
      orientation: "horizontal",
      ratio: 0.5,
      first: {
        id: "text:left",
        text_display: {
          id: "text:left",
          lines: [{ spans: [{ text: "L", fg: null, bg: null }] }],
        },
      },
      second: {
        id: "text:right",
        text_display: {
          id: "text:right",
          lines: [{ spans: [{ text: "R", fg: null, bg: null }] }],
        },
      },
    },
  }));

  const before = app.screen().split("\n");
  const dividerCol = before[0].indexOf("│");
  assert.ok(dividerCol > 0, "the split must paint a divider on the first row");
  assert.equal(
    app.widgetState("split:main")?.splitRatio,
    0.5,
    "the first render resolves the descriptor's own starting ratio",
  );

  app.pushEvent({
    MouseDown: { button: "Left", position: { x: dividerCol, y: 0 }, modifiers: {} },
  });
  app.pushEvent({
    MouseMoved: {
      position: { x: dividerCol + 10, y: 0 },
      buttons: { left: true, right: false, middle: false },
    },
  });
  app.run(2);

  const dragged = app.widgetState("split:main");
  assert.ok(dragged, "the drag must persist a widget-state entry for the split");
  assert.notEqual(
    dragged.splitRatio,
    0.5,
    "the drag must have actually moved the ratio away from its starting 0.5",
  );

  // The render callback's own `split.ratio` is still `0.5` every frame --
  // the dragged ratio must survive regardless, same acceptance
  // `quadraui-binding-core`'s own
  // `dragging_a_split_divider_persists_the_ratio_into_the_widget_state_store`
  // covers at the Rust level.
  const after = app.screen().split("\n");
  const newDividerCol = after[0].indexOf("│");
  assert.notEqual(newDividerCol, dividerCol, "the painted divider must have actually moved");

  const resolved = app.widgetState("split:main");
  assert.ok(resolved, "the split must still have resolved widget state");
  assert.ok(
    Math.abs(resolved.splitRatio! - dragged.splitRatio!) < 0.01,
    "a further render with the same unchanged app descriptor must keep reporting the dragged ratio",
  );
});

// A render-callback layout tree is free to give a `Tree`/`List`/
// `DataTable`/`TabBar` leaf's own descriptor a different `id` than the
// `LayoutLeaf` tree-address `id` wrapping it -- `quadraui-node/tests/
// callbacks.test.ts` already does exactly this for a `text_display` leaf
// (`{ id: "left", text_display: { id: "text:left", ... } }`). Navigation
// and `widgetState` must both key off the outer (`LayoutLeaf`) id, the
// same id `paint_leaf`'s own `PaintedHit` and the engine's nav hooks use
// -- not the descriptor's own embedded id, which this test deliberately
// sets to something else.
test("a tree leaf whose outer id differs from its own descriptor id still resolves navigation under the outer id", () => {
  const app = new QuiApp(24, 8);
  app.setHandler(() => {});
  app.setRender(() => ({
    row: [
      {
        id: "fm:tree-leaf",
        tree: {
          id: "fm:tree-inner",
          rows: treeRows(500),
          selection_mode: "Single",
          selected_path: null,
          style: TREE_STYLE,
        },
      },
    ],
  }));
  // `QuiApp`'s constructor renders once before `setRender` is even
  // called, so that first frame's hit map is empty -- an explicit render
  // here (same as `split:main`'s own test above) repaints with the real
  // callback now registered, so the click below actually has something
  // to land on.
  app.screen();

  app.pushEvent({
    MouseDown: { button: "Left", position: { x: 1, y: 1 }, modifiers: {} },
  });
  app.run(1);

  for (let i = 0; i < 40; i++) {
    app.pushEvent({
      KeyPressed: { key: { Named: "Down" }, modifiers: {}, repeat: false },
    });
  }
  app.run(40);

  const state = app.widgetState("fm:tree-leaf");
  assert.ok(
    state,
    "the leaf's own tree-address id must have resolved widget state after navigation",
  );
  assert.deepEqual(state.selectedPath, [39]);
  assert.ok(
    state.scrollOffset! > 0,
    `500 rows must have scrolled the uncontrolled view: ${JSON.stringify(state)}`,
  );
  assert.ok(app.screenContains("row 39"), "row 39 (the moved cursor) must be visible on screen");
  assert.ok(!app.screenContains("row 0"), "row 0 must have scrolled out of view");

  assert.equal(
    app.widgetState("fm:tree-inner"),
    null,
    "the descriptor's own embedded id must never accumulate widget state of its own",
  );

  // One more nav key, and one more render, past the block above -- proves
  // the leaf id's entry survives `WidgetState::gc()` rather than being
  // swept the very next frame after the nav key that wrote it.
  app.pushEvent({
    KeyPressed: { key: { Named: "Down" }, modifiers: {}, repeat: false },
  });
  app.run(1);
  const afterAnotherFrame = app.widgetState("fm:tree-leaf");
  assert.ok(
    afterAnotherFrame,
    "the leaf's widget state must survive a further render instead of being GC'd",
  );
  assert.deepEqual(afterAnotherFrame.selectedPath, [40]);
});
