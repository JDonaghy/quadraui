// Black-box coverage for the `"scroll"` `LayoutNode` variant through the
// published JS surface -- the Node counterpart of
// `quadraui-py/tests/test_layout_containers.py`'s own `scroll` tests.
//
// An uncontrolled scroll position actually moving is driven by two
// separate code paths: `InteractionEngine::handle`'s own wheel/key
// translation writes the new offset into `WidgetState`, and
// `resolve_layout_node` has to read it back before the next
// `paintLayout` -- so a test asserting only the dispatched event, or
// only the resulting screen, would each miss half of the round trip.
//
// Run with `npm run build:debug` (or `npm run build`) then `npm test`.
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");
const { Scroll, TextDisplay } = require("../js/app/index.ts");

function buttons(count: number) {
  return Array.from({ length: count }, (_, i) => ({
    id: `btn:${i}`,
    button: { id: `btn:${i}`, label: `Item${String(i).padStart(2, "0")}` },
  }));
}

test("scroll renders its content subtree and clips whatever overflows it", () => {
  const app = new QuiApp(40, 10);
  app.setRender(() => ({
    scroll: { id: "scroll:main", axis: "vertical", content: { column: buttons(40) } },
  }));

  assert.ok(app.screenContains("Item00"));
  assert.ok(app.screenContains("Item09"));
  assert.ok(
    !app.screenContains("Item10"),
    "content past the viewport must not paint until the scroll actually moves",
  );
});

test("scroll wheel notch on the scrollbar dispatches and persists the new offset", () => {
  const app = new QuiApp(40, 10);
  app.setRender(() => ({
    scroll: { id: "scroll:main", axis: "vertical", content: { column: buttons(40) } },
  }));
  // Force the first paint so the scrollbar's own track is on the hit map
  // before the wheel notch below.
  app.screen();

  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));
  // x=39 lands on the vertical scrollbar's own track/thumb (the
  // rightmost column the viewport reserves once content overflows it),
  // resolving through `HitDetail::ScrollScrollbarThumb` directly rather
  // than needing a child leaf's own id to look up its enclosing scroll.
  // `ScrollView::handleWheel`'s own convention: a positive `delta.y` is
  // forward/down.
  app.pushEvent({ Scroll: { delta: { x: 0.0, y: 3.0 }, position: { x: 39.0, y: 1.0 } } });
  app.run(1);

  assert.deepEqual(received, [{ ScrollOffsetChanged: { widget: "scroll:main", new_offset: 3 } }]);
  assert.ok(
    !app.screenContains("Item00"),
    "the resolved offset must reach the next paint, not stay pinned at 0",
  );
  assert.ok(app.screenContains("Item04"));

  // The hit map itself must have moved with the paint, not just the
  // pixels: clicking row 0 -- Item00's old screen position -- now hits
  // "btn:3", the child the scroll brought to the top, proving the click
  // router (not only the repainted text) is offset-aware end to end.
  received.length = 0;
  app.pushEvent({ MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} } });
  app.run(1);
  const mouseDowns = (received as any[])
    .filter((event) => "MouseDown" in event)
    .map((event) => event.MouseDown);
  assert.ok(mouseDowns.length > 0, JSON.stringify(received));
  assert.equal(mouseDowns[0].widget, "btn:3", JSON.stringify(received));
});

test("scroll PageDown on a focused child scrolls the whole container", () => {
  const app = new QuiApp(40, 10);
  app.setRender(() => ({
    scroll: { id: "scroll:main", axis: "vertical", content: { column: buttons(40) } },
  }));
  app.screen();

  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));
  // Row 5, not row 1 -- far enough from the row-0 click below (more than
  // `DoubleClickDetector`'s 1.5-cell radius) that the two don't fold
  // into a single `DoubleClick`, which would make that second click's
  // own `MouseDown` assertion vacuous.
  app.pushEvent({ MouseDown: { button: "Left", position: { x: 1.0, y: 5.0 }, modifiers: {} } });
  app.run(1);
  received.length = 0;
  app.pushEvent({ KeyPressed: { key: { Named: "PageDown" }, modifiers: {}, repeat: false } });
  app.run(1);

  assert.deepEqual(received, [
    { ScrollOffsetChanged: { widget: "scroll:main", new_offset: 10 } },
  ]);
  assert.ok(app.screenContains("Item10"));
  assert.ok(!app.screenContains("Item00"));

  // Clicking the viewport's top row now resolves to "btn:10" -- the
  // button PageDown brought there -- not "btn:0", proving the hit map
  // scrolled along with the paint.
  received.length = 0;
  app.pushEvent({ MouseDown: { button: "Left", position: { x: 1.0, y: 0.0 }, modifiers: {} } });
  app.run(1);
  const mouseDowns = (received as any[])
    .filter((event) => "MouseDown" in event)
    .map((event) => event.MouseDown);
  assert.ok(mouseDowns.length > 0, JSON.stringify(received));
  assert.equal(mouseDowns[0].widget, "btn:10", JSON.stringify(received));
});

test("scroll with a controlled offset pins it from the very first paint", () => {
  // The Node binding resolves `Scroll` through
  // `quadraui_binding_core::host::resolve_layout_node` directly, which
  // computes `max_offset` from this frame's own measured content/
  // viewport rather than a previous hit, so a controlled offset is
  // pinned correctly from the first render already -- this is the Node
  // counterpart of `quadraui-py`'s own first-paint regression test,
  // proving both bindings agree on the documented controlled-scroll
  // behaviour.
  const app = new QuiApp(40, 10);
  app.setRender(() => ({
    scroll: {
      id: "scroll:main",
      axis: "vertical",
      offset: 15.0,
      content: { column: buttons(40) },
    },
  }));

  assert.ok(
    !app.screenContains("Item00"),
    "a controlled offset must pin the very first paint, not clamp to 0 for lack of a prior hit",
  );
  assert.ok(app.screenContains("Item15"));
});

test("Scroll node helper builds the expected wire shape and omits defaults", () => {
  const node = Scroll(TextDisplay([]));
  assert.equal(node.kind, "scroll");
  assert.equal(node.id, null);
  const built = node.build("scroll:1", { content: { text_display: { id: "x", lines: [] } } });
  assert.deepEqual(built, {
    scroll: { id: "scroll:1", content: { text_display: { id: "x", lines: [] } } },
  });

  const pinned = Scroll(TextDisplay([]), {
    id: "scroll:pinned",
    axis: "horizontal",
    offset: 4.0,
    padding: 2,
    margin: 1.0,
  });
  assert.equal(pinned.id, "scroll:pinned");
  const builtPinned = pinned.build("scroll:pinned", { content: { ignored: true } });
  assert.deepEqual(builtPinned, {
    scroll: {
      id: "scroll:pinned",
      axis: "horizontal",
      offset: 4.0,
      content: { ignored: true },
    },
    padding: 2,
    margin: 1.0,
  });
});
