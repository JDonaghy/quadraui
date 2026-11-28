// Black-box coverage for the virtualised-collection seam a real Node app
// sees: a `data_table` leaf that sends only a 50-row slice of a
// 100,000-row collection (its `items_window` field) scrolls by absolute
// row index and asks the app for the rows a move brought into view, as a
// `{"RowsRequested": {id, start, count}}` event delivered to the same
// registered handler every `UiEvent` reaches.
//
// Nothing in the app computes that request: the table itself does, from
// its own window and the scroll the move resolved to, and it never
// re-asks for a slice the app already supplied. Mirrors
// `quadraui-py/tests/test_windowed_rows_requested.py`.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const { QuiApp } = require("../js/index.ts");

const TOTAL = 100_000;
const WINDOW_LEN = 50;
const TAB = { KeyPressed: { key: { Named: "Tab" }, modifiers: {}, repeat: false } };
const END = { KeyPressed: { key: { Named: "End" }, modifiers: {}, repeat: false } };

function cell(text: string) {
  return { spans: [{ text, fg: null, bg: null }] };
}

// A `QuiApp` whose render callback sends only `state.start`'s own slice of
// a `TOTAL`-row table, declaring the real total in `items_window`.
function windowedApp(state: { start: number }) {
  const app = new QuiApp(24, 8);
  app.setRender(() => {
    const start = state.start;
    const length = Math.min(WINDOW_LEN, TOTAL - start);
    const rows = [];
    for (let i = 0; i < length; i++) {
      rows.push({ cells: [cell(`row-${start + i}`)] });
    }
    return {
      column: [
        {
          id: "table",
          data_table: {
            id: "table",
            columns: [{ title: "Row" }],
            rows,
            items_window: { start, total: TOTAL },
          },
        },
      ],
    };
  });
  // Force the first paint so the table is on the hit map before any
  // interaction below.
  app.screen();
  return app;
}

test("scrolling past the loaded window asks the app for more rows", () => {
  const state = { start: 0 };
  const app = windowedApp(state);
  const received: any[] = [];
  app.setHandler((event: any) => {
    received.push(event);
  });
  // Focus the table, then jump to the end of the whole 100,000-row
  // collection -- far outside the 50 rows the app has loaded.
  app.pushEvent(TAB);
  app.pushEvent(END);
  app.run(2);

  const requests = received.filter((event) => "RowsRequested" in event);
  assert.equal(requests.length, 1, `expected exactly one RowsRequested: ${JSON.stringify(received)}`);
  const request = requests[0].RowsRequested;
  assert.equal(request.id, "table");
  assert.ok(request.start + request.count >= TOTAL, JSON.stringify(request));
  // Bounded: a 100,000-row collection is never requested whole.
  assert.ok(request.count < WINDOW_LEN * 10, JSON.stringify(request));
});

test("answering the request paints the requested rows", () => {
  const state = { start: 0 };
  const app = windowedApp(state);
  app.setHandler((event: any) => {
    const request = event.RowsRequested;
    if (request !== undefined) {
      // Exactly what a real app does with one: load that slice and
      // repaint.
      state.start = request.start;
      return true;
    }
    return undefined;
  });
  app.pushEvent(TAB);
  app.pushEvent(END);
  app.run(2);

  // The app answered with the tail of the collection, so the rows that
  // paint are the ones around the absolute offset the table is scrolled
  // to -- not row 0's slice, and not blank placeholders.
  assert.ok(state.start > TOTAL - WINDOW_LEN * 10, JSON.stringify(state));
  const offset = app.widgetState("table")?.scrollOffset;
  assert.ok(offset !== undefined && offset > TOTAL - WINDOW_LEN * 10, String(offset));
  // The leftmost column starts under the table's own border, so the row
  // label's first character is clipped out of the painted cell.
  const screen = app.screen();
  assert.ok(screen.includes(`ow-${offset}`), screen);
});

test("a covered window is never re-requested", () => {
  const state = { start: 0 };
  const app = windowedApp(state);
  const received: any[] = [];
  app.setHandler((event: any) => {
    const request = event.RowsRequested;
    if (request !== undefined) {
      received.push(request);
      state.start = request.start;
      return true;
    }
    return undefined;
  });
  app.pushEvent(TAB);
  app.pushEvent(END);
  app.run(2);
  // The same key again, with the answered window now covering the
  // viewport: nothing left to ask for.
  app.pushEvent(END);
  app.run(1);

  assert.equal(received.length, 1, JSON.stringify(received));
});
