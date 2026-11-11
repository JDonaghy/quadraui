// Black-box coverage for the callback bridge (`setHandler`/`setTickHandler`/
// `setRender`), the re-entrancy guard, and the `actions()` deferred-action
// queue -- ports the shape of `quadraui-py/src/lib.rs`'s
// `reaction_from_return_treats_only_explicit_false_as_exit`, its
// `native_app_logic_window_close_handler_*` cases, and
// `handler_queues_show_message_dialog_via_actions_handle_without_reentrant_borrow`,
// plus a new case this crate's own `QuiApp::check_not_dispatching` needs
// that PyO3's runtime borrow guard already covers for `quadraui-py`: a
// handler calling back into `QuiApp` mid-dispatch must raise, not corrupt
// state.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

test("setHandler is called with the converted event payload", () => {
  const app = new QuiApp(10, 5);
  const received: unknown[] = [];
  app.setHandler((event: unknown) => {
    received.push(event);
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.step();
  assert.equal(received.length, 1);
});

test("a handler returning only an explicit false exits; true/undefined/non-boolean keep running", () => {
  const app = new QuiApp(10, 5);
  let nextReturn: unknown = undefined;
  app.setHandler(() => nextReturn);

  nextReturn = true;
  app.pushKeyEvent("a".codePointAt(0));
  assert.equal(app.step(), true, "an explicit true must keep running");

  nextReturn = undefined;
  app.pushKeyEvent("a".codePointAt(0));
  assert.equal(app.step(), true, "no explicit return must keep running");

  nextReturn = 0;
  app.pushKeyEvent("a".codePointAt(0));
  assert.equal(app.step(), true, "a falsy non-boolean must keep running");

  nextReturn = false;
  app.pushKeyEvent("a".codePointAt(0));
  assert.equal(app.step(), false, "only an explicit false must request exit");
});

test("WindowClose with no handler registered keeps running, same as any other unhandled event", () => {
  const app = new QuiApp(10, 5);
  app.pushEvent("WindowClose");
  assert.equal(app.step(), true);
});

test("WindowClose: a handler returning true vetoes the close and stays open", () => {
  const app = new QuiApp(10, 5);
  app.setHandler(() => true);
  app.pushEvent("WindowClose");
  assert.equal(app.step(), true);
});

test("WindowClose: a handler returning false confirms the close and exits", () => {
  const app = new QuiApp(10, 5);
  app.setHandler(() => false);
  app.pushEvent("WindowClose");
  assert.equal(app.step(), false);
});

test("WindowClose: a handler with no explicit opinion (no return) confirms the close by default", () => {
  const app = new QuiApp(10, 5);
  app.setHandler(() => {
    /* no return */
  });
  app.pushEvent("WindowClose");
  assert.equal(app.step(), false);
});

test("setTickHandler is invoked whenever a step has no event queued", () => {
  const app = new QuiApp(10, 5);
  let ticks = 0;
  app.setTickHandler(() => {
    ticks += 1;
  });
  app.step();
  app.step();
  assert.equal(ticks, 2);
});

test("a tick handler returning false exits without raising", () => {
  const app = new QuiApp(10, 5);
  app.setTickHandler(() => false);
  assert.equal(app.step(), false);
});

test("setRender overrides the fixed slots for every frame", () => {
  const app = new QuiApp(40, 5);
  app.setRender(() => ({
    button: { id: "demo:ok", label: "From render" },
  }));
  assert.ok(app.screenContains("From render"));
});

test("setRender honours a row's padding/gap spacing keys", () => {
  // `"padding"`/`"gap"` are siblings of the `"row"` key in the same node
  // dict. The value is a surface-native-pixel measurement, exactly like
  // `Style.padding` -- on a cell-grid backend (TUI) it converts into whole
  // cells via `ceil(px / nominal_cell_px)` (an 8px-wide, 16px-tall nominal
  // cell), not read as that many cells outright. `padding: 12` therefore
  // insets the leaf by `ceil(12 / 8) == 2` columns and `ceil(12 / 16) ==
  // 1` row from the top-left corner.
  const app = new QuiApp(20, 6);
  app.setRender(() => ({
    row: [{ id: "left", text_display: { id: "text:left", lines: [{ spans: [{ text: "L", fg: null, bg: null }] }] } }],
    padding: 12,
  }));
  const lines = app.screen().split("\n");
  assert.equal(lines[0].trim(), "", "the row above the padding inset stays blank");
  assert.equal(lines[1].slice(0, 2), "  ", "padding columns stay blank");
  assert.equal(lines[1].trim(), "L");
});

test("setRender honours a panel's padding/margin spacing keys", () => {
  // A `"panel"` node's `"padding"`/`"margin"` keys are siblings of the
  // `"panel"` key, honoured by the same shared layout pass a `"row"`'s own
  // spacing keys use -- the whole panel (chrome and content both) is
  // inset before the chrome paints. `padding: 12` insets a panel with no
  // title bar by `ceil(12 / 8) == 2` columns and `ceil(12 / 16) == 1` row.
  const app = new QuiApp(10, 5);
  app.setRender(() => ({
    panel: {
      id: "panel:1",
      content: { id: "text:body", text_display: { id: "text:body", lines: [{ spans: [{ text: "X", fg: null, bg: null }] }] } },
    },
    padding: 12,
  }));
  const lines = app.screen().split("\n");
  assert.equal(lines[0].trim(), "", "no padding row stays blank");
  assert.equal(lines[1].slice(0, 2), "  ", "padding columns stay blank");
  assert.equal(lines[1].trim(), "X");
});

test("setRender rejects a gap key on a panel node", () => {
  // `"gap"` has no meaning on a `"panel"` node -- there's no second
  // sibling to space it from -- so it's a decode error.
  const app = new QuiApp(10, 5);
  app.setRender(() => ({
    panel: {
      id: "panel:1",
      content: { id: "text:body", text_display: { id: "text:body", lines: [] } },
    },
    gap: 8,
  }));
  assert.throws(() => app.screen(), /InvalidDescriptorError/);
});

test("setRender rejects a gap key on a tabs node", () => {
  // `"gap"` has no meaning on a `"tabs"` node either.
  const app = new QuiApp(10, 8);
  app.setRender(() => ({
    tabs: { id: "tabbar:1", tabs: [] },
    gap: 8,
  }));
  assert.throws(() => app.screen(), /InvalidDescriptorError/);
});

test("setRender accepts an optional focused_index on a tabs node", () => {
  // "focused_index" marks a tab keyboard-focused (reverse video on
  // TUI; see `tui::tab_bar`'s own test for the pixel-level treatment).
  // Passing it must not raise and must not change which tab's content is
  // shown -- that's still "selected"'s job.
  const app = new QuiApp(10, 8);
  app.setRender(() => ({
    tabs: {
      id: "tabbar:1",
      selected: 0,
      focused_index: 1,
      tabs: [
        {
          id: "one",
          title: "One",
          content: {
            id: "text:one",
            text_display: {
              id: "text:one",
              lines: [{ spans: [{ text: "O", fg: null, bg: null }] }],
            },
          },
        },
        {
          id: "two",
          title: "Two",
          content: {
            id: "text:two",
            text_display: {
              id: "text:two",
              lines: [{ spans: [{ text: "T", fg: null, bg: null }] }],
            },
          },
        },
      ],
    },
  }));
  const lines = app.screen().split("\n");
  assert.match(lines[0], /One/);
  assert.match(lines[0], /Two/);
  // Excludes the strip row (`lines[0]`) — `"One"`'s own `O` would
  // trivially satisfy a screen-wide `/O/`, without proving the content
  // pane (tab 0's selected text, not tab 1's keyboard-focused one) ever
  // rendered. The Python twin
  // (`test_tabs_accepts_an_optional_focused_index`) asserts the same way.
  assert.match(lines.slice(1).join("\n"), /O/);
});

test("setRender rejects an out-of-range focused_index on a tabs node", () => {
  // Same out-of-range convention "selected" already uses.
  const app = new QuiApp(10, 8);
  app.setRender(() => ({
    tabs: {
      id: "tabbar:1",
      focused_index: 5,
      tabs: [
        {
          id: "one",
          title: "One",
          content: {
            id: "text:one",
            text_display: {
              id: "text:one",
              lines: [{ spans: [{ text: "O", fg: null, bg: null }] }],
            },
          },
        },
      ],
    },
  }));
  assert.throws(() => app.screen(), /InvalidDescriptorError/);
});

test("setRender honours a split's padding/margin/gap spacing keys", () => {
  // A `"split"` node's `"padding"`/`"margin"`/`"gap"` keys are siblings of
  // the `"split"` key -- `"gap"` is the one spacing key with a real
  // meaning here: extra blank space carved out between the divider and
  // the second pane, subtracted from the available width before
  // `first_min`/`second_min` are evaluated. `padding: 8` insets by one
  // column/row each (`ceil(8/8) == 1`, `ceil(8/16) == 1`); `gap: 16` then
  // inserts `ceil(16/8) == 2` blank columns after the 1-column divider.
  const app = new QuiApp(20, 4);
  app.setRender(() => ({
    split: {
      id: "split:1",
      orientation: "horizontal",
      ratio: 0.5,
      first: { id: "text:left", text_display: { id: "text:left", lines: [{ spans: [{ text: "L", fg: null, bg: null }] }] } },
      second: { id: "text:right", text_display: { id: "text:right", lines: [{ spans: [{ text: "R", fg: null, bg: null }] }] } },
    },
    padding: 8,
    gap: 16,
  }));
  const lines = app.screen().split("\n");
  assert.equal(lines[0].trim(), "", "no padding row stays blank");
  const contentLine = lines[1];
  assert.equal(contentLine[1], "L", "first pane starts right after the padding column");
  const dividerCol = contentLine.indexOf("│");
  const rightCol = contentLine.indexOf("R");
  assert.equal(
    rightCol - dividerCol,
    1 + 2,
    "second pane starts 1 divider column plus the 2-column gap past the divider",
  );
});

test("setRender honours first_min as a portable minimum pane size", () => {
  // `first_min`/`second_min` are in the same portable surface-native-pixel
  // unit `"padding"`/`"margin"`/`"gap"` use, not a backend-native one.
  // `80` quantises to `ceil(80 / 8) == 10` TUI columns.
  const divider = (firstMin: number) => {
    const app = new QuiApp(40, 10);
    app.setRender(() => ({
      split: {
        id: "split:1",
        orientation: "horizontal",
        ratio: 0.02,
        first_min: firstMin,
        first: { id: "text:left", text_display: { id: "text:left", lines: [{ spans: [{ text: "L", fg: null, bg: null }] }] } },
        second: { id: "text:right", text_display: { id: "text:right", lines: [{ spans: [{ text: "R", fg: null, bg: null }] }] } },
      },
    }));
    return app.screen().split("\n")[0].indexOf("│");
  };
  assert.ok(divider(0) < 5);
  assert.ok(divider(80) >= 10);
});

test("setRender: a row child's size/min/max keys build a sidebar(fixed) + content(weight) + inspector(percent) layout", () => {
  // The acceptance scenario: a sidebar (`"Fixed"`), a content pane
  // (`"Weight"`), and an inspector (`"Percent"`) sharing one row. `size`
  // is a sibling of whatever key names a child's own node kind -- here
  // that's the leaf dict's own `"id"`/`"text_display"` pair. `Fixed(80)`
  // quantises to `ceil(80 / 8) == 10` TUI columns; `Percent(0.2)` of the
  // 40-column screen is 8 columns; the only `Weight` child absorbs the
  // entire 22-column remainder.
  const app = new QuiApp(40, 1);
  const leaf = (id: string, text: string) => ({
    id,
    text_display: { id, lines: [{ spans: [{ text, fg: null, bg: null }] }] },
  });
  app.setRender(() => ({
    row: [
      { ...leaf("text:sidebar", "S"), size: { Fixed: 80 } },
      { ...leaf("text:content", "C"), size: { Weight: 1.0 } },
      { ...leaf("text:inspector", "I"), size: { Percent: 0.2 } },
    ],
  }));
  const line = app.screen().split("\n")[0];
  assert.equal(line.indexOf("S"), 0);
  assert.equal(line.indexOf("C"), 10);
  assert.equal(line.indexOf("I"), 32);
});

test("setRender: a row's justify/align keys are siblings of the row key", () => {
  // Two 80px (10-column) `Fixed` children in a 40-column row leave 20
  // columns of leftover; `"justify": "space_between"` puts all of it
  // between them rather than after the last one (the `"start"` default).
  const app = new QuiApp(40, 1);
  const leaf = (id: string, text: string) => ({
    id,
    text_display: { id, lines: [{ spans: [{ text, fg: null, bg: null }] }] },
  });
  app.setRender(() => ({
    row: [
      { ...leaf("text:left", "L"), size: { Fixed: 80 } },
      { ...leaf("text:right", "R"), size: { Fixed: 80 } },
    ],
    justify: "space_between",
  }));
  const line = app.screen().split("\n")[0];
  assert.equal(line.indexOf("L"), 0);
  assert.equal(line.indexOf("R"), 30);
});

test("setRender rejects an unknown justify value", () => {
  const app = new QuiApp(20, 3);
  app.setRender(() => ({
    row: [{ id: "a", text_display: { id: "a", lines: [] } }],
    justify: "spread",
  }));
  assert.throws(() => app.screen(), /InvalidDescriptorError/);
});

test("a handler calling back into QuiApp (e.g. setRender) mid-dispatch raises instead of corrupting state", () => {
  const app = new QuiApp(20, 5);
  let reentrantCallThrew = false;
  app.setHandler(() => {
    try {
      app.setRender(() => ({ button: { id: "demo:ok", label: "OK" } }));
    } catch {
      reentrantCallThrew = true;
    }
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.step();
  assert.ok(reentrantCallThrew, "a reentrant call into QuiApp must raise, not succeed");
  assert.ok(!app.screenContains("OK"), "the reentrant setRender call must not have applied");
});

test("actions().showMessageDialog queues a dialog a handler cannot open directly mid-dispatch", () => {
  const app = new QuiApp(40, 20);
  const actions = app.actions();
  app.setHandler(() => {
    actions.showMessageDialog("Title", "Body", [{ id: "ok", label: "OK" }], null);
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.step();
  assert.ok(app.screenContains("Title"), "the deferred dialog must be painted once step applies the queue");
});

test("actions() raises if called re-entrantly from inside a handler", () => {
  const app = new QuiApp(10, 5);
  let threw = false;
  app.setHandler(() => {
    try {
      app.actions();
    } catch {
      threw = true;
    }
  });
  app.pushKeyEvent("a".codePointAt(0));
  app.step();
  assert.ok(threw, "calling actions() mid-dispatch must raise");
});
