import { App } from "@quadraui/node/app";

class Counter {
  count = 0;

  view() {
    return (
      <column>
        <text id="count">Count: {this.count}</text>
        <row size="content">
          <button id="dec" label="-" onClick={() => this.count--} />
          <button id="inc" label="+" onClick={() => this.count++} />
        </row>
      </column>
    );
  }
}

if (require.main === module) {
  const counter = new Counter();
  new App({ view: () => counter.view() }).runTui(); // q quits; or .runNative("Counter"): same Counter
}
module.exports = { Counter };
