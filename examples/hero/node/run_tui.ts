// Runs the hero counter (`counter.tsx`) live in this terminal: click the
// buttons, `q` quits. Compiles the JSX in memory the same way
// `run_headless.ts` does -- see that file's header.
//
//     node examples/hero/node/run_tui.ts     (from the repo root)

const { loadHero } = require("./load_jsx.ts");
loadHero(true);
