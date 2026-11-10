#!/usr/bin/env python3
"""Tests for `scripts/gen_wire_types.py`'s rendering layer.

Stdlib-only (`unittest`), matching `scripts/test_release.py`'s own rationale
for avoiding a pytest dependency. Run with:

    python3 scripts/test_gen_wire_types.py

The committed `.pyi`/`.d.ts` staleness check (`gen_wire_types.py --check`)
is self-consistent with whatever the renderer produces, so a bug in the
renderer itself can't be seen from the committed output alone. This file
exercises the renderer directly against small throwaway schemas instead:
`_ts_wrap` parenthesization, the `flat()`-built discriminated union's
`TypedDict` (Python) vs. per-variant-union (TypeScript) divergence
(including a `common` field folded into every TypeScript member),
`_insert_before` ordering, and `main`'s `--check` exit codes.
"""
from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gen_wire_types as gwt  # noqa: E402


class TsWrapTests(unittest.TestCase):
    def test_bare_type_is_unwrapped(self) -> None:
        self.assertEqual(gwt._ts_wrap(gwt.STR), "string")

    def test_union_is_parenthesized_before_composing(self) -> None:
        unionized = gwt.UNIONT(gwt.STR, gwt.INT)
        self.assertEqual(gwt._ts_wrap(unionized), "(string | number)")
        # `LIST` composes the wrapped form -- an unwrapped union here would
        # silently change `(A | B)[]` into `A | B[]`.
        self.assertEqual(gwt.LIST(unionized).ts, "(string | number)[]")

    def test_nullable_wraps_its_inner_union(self) -> None:
        inner = gwt.UNIONT(gwt.STR, gwt.INT)
        self.assertEqual(gwt.NULLABLE(inner).ts, "(string | number) | null")


class DiscriminatedRenderingTests(unittest.TestCase):
    """`flat()`'s `TypedDict` (Python) vs. union-of-single-key-objects
    (TypeScript) split, plus a `common` field that rides alongside every
    tag -- see `ObjDef.common`'s own doc for why the two targets need
    different treatment."""

    def setUp(self) -> None:
        self.obj = gwt.flat(
            "_Sample",
            [("Foo", gwt.STR), ("Bar", gwt.INT)],
            common=[gwt.Field("hit", gwt.STR, optional=True)],
        )

    def test_python_renders_one_typed_dict_with_every_field_not_required(self) -> None:
        text = gwt.render_python([self.obj])
        self.assertIn("class _Sample(TypedDict):", text)
        self.assertIn("Foo: NotRequired[str]", text)
        self.assertIn("Bar: NotRequired[int]", text)
        self.assertIn("hit: NotRequired[str]", text)

    def test_typescript_renders_a_union_with_hit_on_every_member(self) -> None:
        text = gwt.render_typescript([self.obj])
        self.assertIn(
            "export type _Sample = "
            "{ Foo: string; hit?: string; } | { Bar: number; hit?: string; };",
            text,
        )

    def test_typescript_without_common_omits_the_trailing_field(self) -> None:
        obj = gwt.flat("_NoCommon", [("Foo", gwt.STR)])
        text = gwt.render_typescript([obj])
        self.assertIn("export type _NoCommon = { Foo: string; };", text)


class InsertBeforeTests(unittest.TestCase):
    def test_inserts_immediately_before_the_named_def(self) -> None:
        defs: list = [
            gwt.ObjDef("A", []),
            gwt.ObjDef("B", []),
            gwt.ObjDef("C", []),
        ]
        saved, gwt.DEFS = gwt.DEFS, defs
        try:
            gwt._insert_before("B", gwt.ObjDef("Inserted", []))
        finally:
            defs, gwt.DEFS = gwt.DEFS, saved
        self.assertEqual([d.name for d in defs], ["A", "Inserted", "B", "C"])

    def test_raises_when_the_named_def_is_missing(self) -> None:
        defs: list = [gwt.ObjDef("A", [])]
        saved, gwt.DEFS = gwt.DEFS, defs
        try:
            with self.assertRaises(StopIteration):
                gwt._insert_before("Missing", gwt.ObjDef("Inserted", []))
        finally:
            gwt.DEFS = saved


class CheckExitCodeTests(unittest.TestCase):
    """`main(["--check"])`'s exit code against a throwaway pair of output
    paths, monkeypatched in and restored -- never against the real
    committed `wire.pyi`/`wire.d.ts`."""

    def setUp(self) -> None:
        self._py_out, self._ts_out = gwt.PY_OUT, gwt.TS_OUT

    def tearDown(self) -> None:
        gwt.PY_OUT, gwt.TS_OUT = self._py_out, self._ts_out

    def test_check_passes_once_outputs_match_generation(self) -> None:
        import tempfile

        # Under `REPO_ROOT` -- `main`'s own `print`s call `Path.relative_to`
        # against it, which raises on a path outside it (e.g. the system
        # `$TMPDIR`).
        with tempfile.TemporaryDirectory(dir=gwt.REPO_ROOT) as tmp:
            gwt.PY_OUT = Path(tmp) / "wire.pyi"
            gwt.TS_OUT = Path(tmp) / "wire.d.ts"
            self.assertEqual(gwt.main([]), 0)
            self.assertEqual(gwt.main(["--check"]), 0)

    def test_check_fails_when_committed_output_is_stale(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory(dir=gwt.REPO_ROOT) as tmp:
            gwt.PY_OUT = Path(tmp) / "wire.pyi"
            gwt.TS_OUT = Path(tmp) / "wire.d.ts"
            gwt.PY_OUT.write_text("stale\n")
            gwt.TS_OUT.write_text("stale\n")
            self.assertEqual(gwt.main(["--check"]), 1)


if __name__ == "__main__":
    unittest.main()
