const { App, keyName } = require("@quadraui/node/app");

class Counter {
  count = 0;

  view() {
    return (
      <column>
        <textDisplay id="count" lines={[{ spans: [{ text: `Count: ${this.count}`, fg: null, bg: null }] }]} />
        <row>
          <button id="dec" label="-" onClick={() => this.count--} />
          <button id="inc" label="+" onClick={() => this.count++} />
        </row>
      </column>
    );
  }

  onKey(event: unknown, quit: () => void) {
    if (keyName(event) === "q") quit();
  }
}

if (require.main === module) {
  const counter = new Counter();
  const app = new App(30, 5, { view: () => counter.view(), onEvent: (e: unknown) => counter.onKey(e, quit) });
  const live = app.runTui(); // or app.runNative("Counter"): same Counter
  // Ending a live session needs both calls, deferred out of the handler (examples/hero/README.md, "Gaps").
  const quit = () => setImmediate(() => (live.stop(), app.stopLive()));
}
module.exports = { Counter };
