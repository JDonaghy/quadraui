// A TypeScript consumer of `@quadraui/node/app`, type-checked by
// `../package_exports.test.ts` under `tsc --strict --module node16`. The
// `@ts-expect-error` lines fail the check if `App` or `Button` ever lose
// their declared types (an `any` accepts every call, so the expected error
// would go missing).
import { App, Button } from "@quadraui/node/app";
import type { Renderable } from "@quadraui/node/app";
import { harness } from "@quadraui/node/testing";
import { QuadrauiError } from "@quadraui/node";

let clicks = 0;
const view = (): Renderable => Button("Go", { id: "go", onClick: () => clicks++ });
const app = new App(20, 3, { view });
const h = harness(app);
h.clickText("Go");
const count: number = clicks;
const isError: boolean = new QuadrauiError("x") instanceof Error;

// @ts-expect-error -- `App` takes (width, height, options).
new App("wide", 3, { view });
// @ts-expect-error -- `Button`'s label is a string.
Button(42);

export { count, isError };
