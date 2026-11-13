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
    scroll: { id: "scroll:main", axis: "vertical", content: { column: buttons(30) } },
  }));

  assert.ok(app.screenContains("Item00"));
  assert.ok(app.screenContains("Item09"));
  assert.ok(
    !app.screenContains("Item15"),
    "content past the viewport must not paint until the scroll actually moves",
  );
});

test("scroll wheel notch on the scrollbar dispatches and persists the new offset", () => {
  const app = new QuiApp(40, 10);
  app.setRender(() => ({
    scroll: { id: "scroll:main", axis: "vertical", content: { column: buttons(30) } },
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
});

test("scroll PageDown on a focused child scrolls the whole container", () => {
  const app = new QuiApp(40, 10);
  app.setRender(() => ({
    scroll: { id: "scroll:main", axis: "vertical", content: { column: buttons(30) } },
  }));
  app.screen();

  const received: unknown[] = [];
  app.setHandler((event: unknown) => received.push(event));
  app.pushEvent({ MouseDown: { button: "Left", position: { x: 1.0, y: 1.0 }, modifiers: {} } });
  app.run(1);
  received.length = 0;
  app.pushEvent({ KeyPressed: { key: { Named: "PageDown" }, modifiers: {}, repeat: false } });
  app.run(1);

  assert.deepEqual(received, [
    { ScrollOffsetChanged: { widget: "scroll:main", new_offset: 10 } },
  ]);
  assert.ok(app.screenContains("Item10"));
  assert.ok(!app.screenContains("Item00"));
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
