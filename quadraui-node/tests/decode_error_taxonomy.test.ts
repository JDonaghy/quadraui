// Black-box coverage for `quadraui_binding_core::DecodeError`'s
// per-variant exception taxonomy: `MissingFieldError`, `UnknownVariantError`,
// `TypeMismatchError`, `UnknownKeyError`, `VersionMismatchError` each
// extend `InvalidDescriptorError` (so an existing `instanceof
// InvalidDescriptorError`/`/InvalidDescriptorError/` check keeps matching
// unchanged), carry a `code` property mirroring
// `quadraui_binding_core::DecodeError::code`, and raise with a message
// naming the full path into the layout tree at which the problem was
// found -- not just a bare field name.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test

const { test } = require("node:test");
const assert = require("node:assert/strict");

const {
  InvalidDescriptorError,
  MissingFieldError,
  QuiApp,
  TypeMismatchError,
  UnknownKeyError,
  UnknownVariantError,
  VersionMismatchError,
} = require("../js/index.ts");

function button(id: string, label: string) {
  return { id, button: { id, label } };
}

test("a missing field names the split's own path", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({
    split: {
      id: "frm",
      orientation: "horizontal",
      // "ratio" deliberately omitted.
      first: button("a", "A"),
      second: button("b", "B"),
    },
  }));
  assert.throws(() => app.screen(), (err: unknown) => {
    assert.ok(err instanceof MissingFieldError);
    assert.ok(err instanceof InvalidDescriptorError);
    assert.equal((err as { code: string }).code, "missing_field");
    assert.match((err as Error).message, /split#frm/);
    return true;
  });
});

test("an unknown orientation names the bad value and the split's path", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({
    split: {
      id: "frm",
      orientation: "diagonal",
      ratio: 0.5,
      first: button("a", "A"),
      second: button("b", "B"),
    },
  }));
  assert.throws(() => app.screen(), (err: unknown) => {
    assert.ok(err instanceof UnknownVariantError);
    assert.equal((err as { code: string }).code, "unknown_variant");
    assert.match((err as Error).message, /split#frm/);
    assert.match((err as Error).message, /diagonal/);
    return true;
  });
});

test("a wrongly typed row raises TypeMismatchError", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({ row: "not a list" }));
  assert.throws(() => app.screen(), (err: unknown) => {
    assert.ok(err instanceof TypeMismatchError);
    assert.equal((err as { code: string }).code, "type_mismatch");
    return true;
  });
});

test("an unknown padding key raises UnknownKeyError naming it", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({ row: [button("a", "A")], padding: { lef: 4 } }));
  assert.throws(() => app.screen(), (err: unknown) => {
    assert.ok(err instanceof UnknownKeyError);
    assert.equal((err as { code: string }).code, "unknown_key");
    assert.match((err as Error).message, /lef/);
    return true;
  });
});

test("a wire_version newer than this host understands raises VersionMismatchError", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({
    wire_version: 999,
    button: { id: "b1", label: "Go" },
  }));
  assert.throws(() => app.screen(), (err: unknown) => {
    assert.ok(err instanceof VersionMismatchError);
    assert.equal((err as { code: string }).code, "version_mismatch");
    assert.match((err as Error).message, /999/);
    return true;
  });
});

test("a deeply nested decode error names every ancestor on the way down", () => {
  const app = new QuiApp(20, 5);
  app.setRender(() => ({
    tabs: {
      id: "frm",
      tabs: [
        {
          id: "one",
          title: "One",
          content: {
            panel: {
              id: "chrome",
              content: {
                split: {
                  id: "inner",
                  orientation: "horizontal",
                  first: button("a", "A"),
                  second: button("b", "B"),
                },
              },
            },
          },
        },
      ],
    },
  }));
  assert.throws(() => app.screen(), (err: unknown) => {
    assert.ok(err instanceof MissingFieldError);
    const message = (err as Error).message;
    assert.match(message, /tabs#frm/);
    assert.match(message, /panel#chrome/);
    assert.match(message, /split#inner/);
    return true;
  });
});
