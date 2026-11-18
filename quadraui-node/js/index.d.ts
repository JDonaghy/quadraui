// The published type surface for `./index.ts`'s runtime facade, maintained
// by hand rather than generated from it. `index.ts` deliberately spells
// every one of its exports as a plain `exports.X = X` assignment rather
// than TypeScript `export` syntax (see that file's own header doc for why
// -- Node's native TypeScript support treats a top-level `export`/`import`
// as the file-format signal that flips a `"type": "commonjs"` `.ts` file
// to ESM, which turns the bare `exports`/`require` this file's many direct
// consumers (`js/app/runtime.ts`, every example, every test) rely on into
// a hard `ReferenceError`). That same property means `tsc --declaration`
// run against `index.ts` can't see most of what it exports either: nothing
// not reachable from one of its own `export type` aliases survives
// emission, which is every *value* export (`QuiApp`, `QuadrauiError` and
// its subclasses, `pump`, `run`, ...).
//
// This file is `index.ts`'s sibling declaration instead -- the same shape
// a hand-authored `.d.ts` gives any other CommonJS module whose runtime
// shape isn't mechanically derivable from its own syntax. It is never
// executed (a `.d.ts` is inert to Node either way), so it is free to use
// ordinary `export` syntax throughout, and deliberately repeats the small
// set of plain interfaces `index.ts` itself already spells with `export
// type`/`export interface` (`WidgetState`, `PumpHandle`, `RunnableApp`,
// `RunHandle`) rather than importing them back from `"./index"`: resolving
// a bare specifier (no extension) against this directory prefers
// `index.ts` over this file whenever both are part of the same TypeScript
// program, so an import here naming `"./index"` would resolve right back
// to `index.ts` -- fine for every in-repo consumer (which either
// `require()`s `index.ts` untyped or `import type`s only those same
// already-`export`ed interfaces directly off it), but circular and
// needless for this file's own, separate purpose: being copied next to
// the compiled `dist/index.js` this package's `main`/`types` point a
// published consumer at (see `package.json`'s `build:dist` script) -- the
// one place this file's own declarations are actually the ones read.
//
// Reconciles with the native, napi-generated `../index.d.ts` the same way
// `index.ts` reconciles with `../index.js` at runtime: `QuiApp` extends
// `native.QuiApp` to inherit its full generated method list unchanged,
// and `QuiShellApp`/`QuiAppWaker` are named straight off it. Keep this
// file's shape in sync with `index.ts`'s own `exports.*` assignments by
// hand whenever one changes.

import type * as native from "../index";

/** Base class every subclass below extends -- see `index.ts`'s own doc. */
export declare class QuadrauiError extends Error {
  constructor(message: string);
}

/**
 * A descriptor/event shape this binding's decode adapter rejected.
 *
 * `path` is the dotted path into the layout tree the underlying
 * `DecodeError::path` named (e.g. `"render › split#body › panel#chrome"`)
 * -- only present when this instance came from an actual `DecodeError`;
 * `undefined` for a plain `InvalidDescriptorError` raised directly for a
 * non-`DecodeError` failure.
 */
export declare class InvalidDescriptorError extends QuadrauiError {
  readonly path?: string;
}

export declare class MissingFieldError extends InvalidDescriptorError {
  readonly code: "missing_field";
}

export declare class UnknownVariantError extends InvalidDescriptorError {
  readonly code: "unknown_variant";
}

export declare class TypeMismatchError extends InvalidDescriptorError {
  readonly code: "type_mismatch";
}

export declare class UnknownKeyError extends InvalidDescriptorError {
  readonly code: "unknown_key";
}

export declare class VersionMismatchError extends InvalidDescriptorError {
  readonly code: "version_mismatch";
}

export declare class DecodeFailedError extends InvalidDescriptorError {
  readonly code: "borrow";
}
/** A fixed slot or additive-primitive kind that already holds a descriptor. */
export declare class SlotOccupiedError extends QuadrauiError {}
/** A slot id that is not one of the documented `SLOT_*` constants. */
export declare class UnknownSlotError extends QuadrauiError {}
/** A zero width/height, or a byte buffer whose length doesn't match the width/height it encodes. */
export declare class InvalidDimensionsError extends QuadrauiError {}
/** A `pushKeyEvent` code point that is not a valid Unicode scalar value. */
export declare class InvalidCharError extends QuadrauiError {}
/** A Rust panic caught at this app's `guarded` boundary. */
export declare class PanicError extends QuadrauiError {}
/** Raised by every method on an app an earlier caught panic poisoned. */
export declare class PoisonedError extends QuadrauiError {}
/** A failure internal to this binding rather than caused by caller input. */
export declare class InternalError extends QuadrauiError {}
/** A `role` string `measureText` does not recognize. */
export declare class InvalidFontRoleError extends QuadrauiError {}
/** A `RenderCtx`'s `measureText`/`lineHeight`/`charWidth` was called after the render call that handed it out already returned. */
export declare class StaleRenderCtxError extends QuadrauiError {}
/** A method called re-entrantly from inside this same app's own registered event/tick/render callback. */
export declare class ReentrantCallError extends QuadrauiError {}
/** A `QuiApp` method called while `startTui`/`startNative` is live. */
export declare class LiveRunnerActiveError extends QuadrauiError {}
/** `step`/`run` called after a live `startTui`/`startNative` session this app once held has already ended. */
export declare class LiveRunnerExitedError extends QuadrauiError {}
/** `startNative` called on a binary built with none of the `gtk`/`macos`/`win` features. */
export declare class NoNativeBackendError extends QuadrauiError {}
/** `startNative` on the `gtk` feature found no display to open a window on. */
export declare class NoDisplayError extends QuadrauiError {}
/** `startNative` on the `gtk` feature was given an `appId` that is not a valid `GApplication` id. */
export declare class InvalidAppIdError extends QuadrauiError {}
/** A `quadraui_binding_core::services` call this backend has no implementation for at all. */
export declare class UnsupportedError extends QuadrauiError {}

/** Every subclass above, keyed by both its own name and the tag its native `Error.message` prefix carries. */
export declare const ERROR_CLASSES: Readonly<Record<string, new (message: string) => QuadrauiError>>;

/** Converts a value caught from a native call into the {@link QuadrauiError} subclass its message is tagged with. */
export declare function toQuadrauiError(err: unknown): unknown;

/**
 * The resolved widget-state shape `QuiApp.widgetState(id)` returns -- see
 * `index.ts`'s own doc comment for field-by-field detail.
 */
export interface WidgetState {
  selectedPath: number[] | null;
  selectedIndex: number | null;
  scrollOffset: number | null;
  splitRatio: number | null;
  activeTab: number | null;
  expanded: number[][] | null;
}

/**
 * A headless quadraui app, driven entirely from JavaScript/TypeScript --
 * see `index.ts`'s own doc comment for the full description. Extends the
 * native, napi-generated `QuiApp` to inherit every other method unchanged;
 * only the constructor (wraps a zero-width/zero-height native throw in
 * {@link toQuadrauiError}) and `widgetState` (narrows its return type) are
 * overridden here.
 */
export declare class QuiApp extends native.QuiApp {
  constructor(width: number, height: number);
  widgetState(id: string): WidgetState | null;
}

/** The standard, per-app filesystem location named by `kind`. */
export declare function appPath(kind: string): string;
/** Sets the app identifier scoping {@link appPath}'s app-scoped kinds. */
export declare function setAppIdentifier(identifier: string): void;
/** Installs a crash handler that writes a crash record on an uncaught panic. */
export declare function installCrashHandler(cfg: unknown): void;

/**
 * The {@link RenderCtx} for whichever `render`/`view` callback is
 * currently executing on this thread, or `null` outside of one -- see
 * `index.ts`'s own doc comment for the full contract.
 */
export declare function getCurrentCtx(): RenderCtx | null;

/**
 * Registers a script-defined widget's four callbacks against `id` -- see
 * `index.ts`'s own doc comment for the full contract.
 * `quadraui-node/js/app/widget.ts`'s `defineWidget` is the ergonomic,
 * typed way to reach this.
 */
export declare function registerScriptedWidget(
  id: string,
  measure: (packed: unknown) => unknown,
  paint: (packed: unknown) => unknown,
  onEvent: (packed: unknown) => unknown,
  a11y: (packed: unknown) => unknown,
  focusable: boolean,
): void;

/**
 * Render-time capabilities a script `render`/`view` callback needs from
 * the live backend painting this frame -- real text measurement
 * (`measureText`/`lineHeight`/`charWidth`), cell-grid-vs-pixel surface
 * facts (`cellGrid`/`scale`), and read-only theme/style token lookup by
 * name (`themeToken`/`styleToken`) -- see the native binding's own doc
 * comment (`quadraui-node/src/lib.rs`'s `RenderCtx`) for the full method
 * list.
 */
export type RenderCtx = InstanceType<typeof native.RenderCtx>;

/**
 * A headless `ShellApp`/`ShellConfig`-backed app -- see `index.ts`'s own
 * doc comment (and the native binding's own `QuiShellApp`) for the full
 * method list.
 */
export type QuiShellApp = InstanceType<typeof native.QuiShellApp>;

/** A thread-safe handle, returned by `QuiApp.waker()` -- see `index.ts`'s own doc comment. */
export type QuiAppWaker = InstanceType<typeof native.QuiAppWaker>;

/**
 * A `QuiApp.actions()` handle -- the sanctioned way to open a dialog/
 * toast/context menu/... from inside a render/event/tick callback, where
 * the matching `QuiApp` method itself would raise `ReentrantCallError`;
 * see `index.ts`'s own doc comment (`REENTRANT_RESTRICTED_METHODS`) for
 * the full list of methods this mirrors, and `app/runtime.ts`'s `App.confirm`/
 * `App.alert` for the typed-app-layer wrapper built on `showMessageDialog`.
 */
export type QuiAppActions = InstanceType<typeof native.QuiAppActions>;

/** A handle returned by `pump()`: stops the `setInterval` loop it started. */
export interface PumpHandle {
  stop(): void;
}

/** Drives `app.step()` repeatedly on a fixed timer -- see `index.ts`'s own doc comment for the full contract. */
export declare function pump(
  app: { step(): boolean; stopLive?(): boolean },
  intervalMs?: number,
): PumpHandle;

/** The type of {@link pump}. */
export type PumpFn = typeof pump;

/** An `app` {@link run} can drive -- see `index.ts`'s own doc comment. */
export interface RunnableApp {
  step(): boolean;
  stopLive?(): boolean;
  armWakeNotify(callback: () => void): void;
  disarmWakeNotify(): void;
}

/** A handle returned by {@link run}: stops the wake-driven loop it started. */
export interface RunHandle {
  stop(): void;
}

/** Drives `app.step()` wake-driven instead of on a fixed interval -- see `index.ts`'s own doc comment for the full contract. */
export declare function run(app: RunnableApp, fallbackIntervalMs?: number): RunHandle;

/** The type of {@link run}. */
export type RunFn = typeof run;

export type * from "./wire";
