// Black-box coverage for `@quadraui/node`'s own public surface: every
// `QuadrauiError` subclass this binding can raise must be importable from
// the top-level module, not just reachable as a private implementation
// detail of `js/index.ts` -- a `test_public_api.py`-style check (see
// `quadraui-py/tests/test_public_api.py`), adapted to this binding's own
// shape: napi-rs has no custom-exception-class registration for this test
// to introspect the way `quadraui-py`'s `dir(_native)` does, so the
// canonical list is `ERROR_CLASSES` (see `js/index.ts`'s own doc on it)
// rather than something discovered off the native addon.
//
// Run against a built addon, e.g.:
//
//     npm run build:debug
//     npm test
const { test } = require("node:test");
const assert = require("node:assert/strict");

const quadraui = require("../js/index.ts");

test("every class in ERROR_CLASSES is exported from the module under its own name", () => {
  const missing: string[] = [];
  for (const [name, ctor] of Object.entries(quadraui.ERROR_CLASSES)) {
    if (quadraui[name] !== ctor) {
      missing.push(name);
    }
  }
  assert.deepEqual(missing, []);
});

test("every class in ERROR_CLASSES extends QuadrauiError, which extends Error", () => {
  assert.ok(quadraui.QuadrauiError.prototype instanceof Error);
  for (const [name, ctor] of Object.entries(quadraui.ERROR_CLASSES) as [string, new (message: string) => Error][]) {
    const instance = new ctor(`${name} probe`);
    assert.ok(instance instanceof quadraui.QuadrauiError, `${name} does not extend QuadrauiError`);
    assert.ok(instance instanceof Error, `${name} does not extend Error`);
    assert.equal(instance.name, name, `${name}'s own .name does not match its own class name`);
    assert.equal(instance.message, `${name} probe`);
  }
});

test("ERROR_CLASSES is keyed by each class's own name, with no stray or missing entries", () => {
  for (const [key, ctor] of Object.entries(quadraui.ERROR_CLASSES)) {
    assert.equal((ctor as { name: string }).name, key);
  }
});

test("ERROR_CLASSES is frozen -- a consumer cannot mutate the canonical registry", () => {
  assert.ok(Object.isFrozen(quadraui.ERROR_CLASSES));
});

test("toQuadrauiError is exported and is a function", () => {
  assert.equal(typeof quadraui.toQuadrauiError, "function");
});

test("toQuadrauiError converts a tagged plain Error into the matching subclass, preserving message and stack", () => {
  const tagged = new Error("SlotOccupiedError: slot 0 already holds a descriptor");
  const converted = quadraui.toQuadrauiError(tagged) as Error;
  assert.ok(converted instanceof quadraui.SlotOccupiedError);
  assert.ok(converted instanceof quadraui.QuadrauiError);
  assert.equal(converted.message, tagged.message);
  assert.equal(converted.stack, tagged.stack);
});

test("toQuadrauiError passes through an Error with no recognized tag unchanged", () => {
  const plain = new Error("boom");
  assert.equal(quadraui.toQuadrauiError(plain), plain);
});

test("toQuadrauiError passes through a non-Error value unchanged", () => {
  assert.equal(quadraui.toQuadrauiError("boom"), "boom");
});

test("a real native failure is importable and catchable by its specific subclass name", () => {
  // `pushKeyEvent`'s own `char::from_u32` check is this binding's
  // `InvalidCharError`; this is the form every example/docstring implies
  // works: `catch (err) { if (err instanceof InvalidCharError) ... }`.
  const { QuiApp, InvalidCharError, QuadrauiError } = quadraui;
  const app = new QuiApp(5, 5);
  assert.throws(
    () => app.pushKeyEvent(0x110000),
    (err: unknown) => {
      assert.ok(err instanceof InvalidCharError);
      assert.ok(err instanceof QuadrauiError);
      assert.match((err as Error).message, /InvalidCharError/);
      return true;
    },
  );
});

test("QuiApp's own constructor raises a catchable InvalidDimensionsError", () => {
  // The constructor is the one call every other wrapped method reaches
  // through an instance's prototype, but a `new` call never does -- see
  // `js/index.ts`'s own doc on why `QuiApp` wraps its constructor
  // separately.
  const { QuiApp, InvalidDimensionsError } = quadraui;
  assert.throws(() => new QuiApp(0, 4), (err: unknown) => err instanceof InvalidDimensionsError);
});
