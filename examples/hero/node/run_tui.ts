// Runs a JSX app from this checkout as the entry module: the hero counter
// (`counter.tsx`) by default, or the `.tsx`/`.ts` file you name. The file is
// compiled in memory the same way `run_headless.ts` does, and
// `@quadraui/node/*` resolves to this checkout -- see `load_jsx.ts`.
//
//     node examples/hero/node/run_tui.ts                  (the counter; q quits)
//     node examples/hero/node/run_tui.ts path/to/app.tsx  (your own app)
//
// Run it from the repo root, in a real terminal. Node >= 22.18 runs `.ts`
// directly; 22.6 to 22.17 need --experimental-strip-types.

const path = require("node:path");
const { load, loadHero } = require("./load_jsx.ts");

const file = process.argv[2];
if (file) load(path.resolve(file), true);
else loadHero(true);
