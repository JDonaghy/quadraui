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
`_ty_from_json`/`_def_from_json`'s JSON decode, and `main`'s `--check` exit
codes. Every test monkeypatches `gwt.DEFS` directly (or never calls
`main`/`get_defs` at all) rather than exercising `load_defs_from_rust`'s own
`cargo run` -- that round-trip, and the schema's actual content, is
`scripts/check_wire_types.sh`'s job, run separately so this file stays a
fast, toolchain-independent unit suite.
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


class KeywordCollidingFieldTests(unittest.TestCase):
    """A Rust field name that collides with a Python keyword (`from` on
    `DrawOp::Line`/`Bezier`/`DashedLine`) can't be a class-body `TypedDict`
    attribute (`class X: from: Y` is a `SyntaxError`) -- `render_python`
    falls back to the functional `TypedDict("X", {...})` form for any
    `ObjDef` with such a field, and keeps using the ordinary class form for
    one without."""

    def test_keyword_field_renders_as_functional_typed_dict(self) -> None:
        obj = gwt.ObjDef(
            "_Sample3",
            [
                gwt.Field("from", gwt.STR, doc="where it came from"),
                gwt.Field("to", gwt.STR, optional=True),
            ],
        )
        text = gwt.render_python([obj])
        self.assertIn('_Sample3 = TypedDict(', text)
        self.assertIn('"_Sample3"', text)
        # The field's own doc still shows up as a trailing comment, the
        # same as the class-based renderer's per-field comment.
        self.assertIn('"from": str,  # where it came from', text)
        self.assertIn('"to": NotRequired[str],', text)
        self.assertNotIn("class _Sample3", text)

    def test_non_keyword_field_still_renders_as_a_class(self) -> None:
        obj = gwt.ObjDef("_Sample4", [gwt.Field("source", gwt.STR)])
        text = gwt.render_python([obj])
        self.assertIn("class _Sample4(TypedDict):", text)
        self.assertNotIn("_Sample4 = TypedDict(", text)


class DefJsonDecodeTests(unittest.TestCase):
    """[`gwt._ty_from_json`]/[`gwt._def_from_json`] -- the decode half of
    [`gwt.load_defs_from_rust`], exercised directly against hand-built JSON
    rather than a real `cargo run` (that round-trip is
    `scripts/check_wire_types.sh`'s job, not this renderer-only suite's)."""

    def test_ty_from_json_round_trips_every_shape(self) -> None:
        self.assertEqual(gwt._ty_from_json({"kind": "prim", "name": "STR"}), gwt.STR)
        self.assertEqual(gwt._ty_from_json({"kind": "ref", "name": "Foo"}), gwt.REF("Foo"))
        self.assertEqual(
            gwt._ty_from_json({"kind": "nullable", "inner": {"kind": "prim", "name": "INT"}}),
            gwt.NULLABLE(gwt.INT),
        )
        self.assertEqual(
            gwt._ty_from_json({"kind": "list", "inner": {"kind": "prim", "name": "BOOL"}}),
            gwt.LIST(gwt.BOOL),
        )
        self.assertEqual(
            gwt._ty_from_json(
                {
                    "kind": "dict",
                    "key": {"kind": "prim", "name": "STR"},
                    "value": {"kind": "prim", "name": "ANY"},
                }
            ),
            gwt.DICTT(gwt.STR, gwt.ANY),
        )
        self.assertEqual(
            gwt._ty_from_json(
                {"kind": "tuple", "items": [{"kind": "prim", "name": "STR"}, {"kind": "prim", "name": "INT"}]}
            ),
            gwt.TUP(gwt.STR, gwt.INT),
        )
        self.assertEqual(gwt._ty_from_json({"kind": "lit", "values": ["A", "B"]}), gwt.LIT("A", "B"))
        self.assertEqual(
            gwt._ty_from_json(
                {"kind": "union", "items": [{"kind": "prim", "name": "STR"}, {"kind": "prim", "name": "INT"}]}
            ),
            gwt.UNIONT(gwt.STR, gwt.INT),
        )

    def test_def_from_json_decodes_obj_and_alias(self) -> None:
        obj = gwt._def_from_json(
            {
                "kind": "obj",
                "name": "Foo",
                "fields": [{"name": "x", "ty": {"kind": "prim", "name": "INT"}, "optional": True, "doc": "d"}],
                "doc": "Foo's own doc.",
                "discriminated": False,
                "common": [],
            }
        )
        assert isinstance(obj, gwt.ObjDef)
        self.assertEqual(obj.name, "Foo")
        self.assertEqual(obj.fields, [gwt.Field("x", gwt.INT, optional=True, doc="d")])
        self.assertEqual(obj.doc, "Foo's own doc.")

        alias = gwt._def_from_json(
            {"kind": "alias", "name": "Bar", "ty": {"kind": "prim", "name": "STR"}, "doc": ""}
        )
        assert isinstance(alias, gwt.AliasDef)
        self.assertEqual(alias.name, "Bar")
        self.assertEqual(alias.ty, gwt.STR)

    def test_def_from_json_rejects_unknown_kind(self) -> None:
        with self.assertRaises(ValueError):
            gwt._def_from_json({"kind": "bogus", "name": "Foo"})


class CheckExitCodeTests(unittest.TestCase):
    """`main(["--check"])`'s exit code against a throwaway pair of output
    paths and a small monkeypatched [`gwt.DEFS`] fixture -- never against
    the real committed `wire.pyi`/`wire.d.ts`, and never through
    [`gwt.load_defs_from_rust`]'s own `cargo run` (that's
    `scripts/check_wire_types.sh`'s job)."""

    def setUp(self) -> None:
        self._py_out, self._ts_out, self._stub_out = gwt.PY_OUT, gwt.TS_OUT, gwt.STUB_OUT
        self._defs = gwt.DEFS
        gwt.DEFS = [gwt.ObjDef("Foo", [gwt.Field("x", gwt.INT)])]

    def tearDown(self) -> None:
        gwt.PY_OUT, gwt.TS_OUT, gwt.STUB_OUT = self._py_out, self._ts_out, self._stub_out
        gwt.DEFS = self._defs

    def test_check_passes_once_outputs_match_generation(self) -> None:
        import tempfile

        # Under `REPO_ROOT` -- `main`'s own `print`s call `Path.relative_to`
        # against it, which raises on a path outside it (e.g. the system
        # `$TMPDIR`).
        with tempfile.TemporaryDirectory(dir=gwt.REPO_ROOT) as tmp:
            gwt.PY_OUT = Path(tmp) / "wire.pyi"
            gwt.TS_OUT = Path(tmp) / "wire.d.ts"
            gwt.STUB_OUT = Path(tmp) / "_quadraui.pyi"
            self.assertEqual(gwt.main([]), 0)
            self.assertEqual(gwt.main(["--check"]), 0)

    def test_check_fails_when_committed_output_is_stale(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory(dir=gwt.REPO_ROOT) as tmp:
            gwt.PY_OUT = Path(tmp) / "wire.pyi"
            gwt.TS_OUT = Path(tmp) / "wire.d.ts"
            gwt.STUB_OUT = Path(tmp) / "_quadraui.pyi"
            gwt.PY_OUT.write_text("stale\n")
            gwt.TS_OUT.write_text("stale\n")
            gwt.STUB_OUT.write_text("stale\n")
            self.assertEqual(gwt.main(["--check"]), 1)


class PymethodsParserTests(unittest.TestCase):
    """[`gwt.parse_pymethods`]/[`gwt.parse_pyfunctions`] and the
    Rust-type-to-`.pyi` mapping they feed, against small inline `lib.rs`-
    shaped snippets rather than the real, 10k-line `quadraui-py/src/lib.rs`
    -- that file's own drift is covered by `gen_wire_types.py --check`
    (exercised above against throwaway output paths) and
    `scripts/check_wire_types.sh`'s `mypy`/`pyright` runs against its real
    generated output, not by this fast, toolchain-independent unit suite."""

    def test_parses_doc_attrs_defaults_and_skips_python_token(self) -> None:
        src = """
#[pymethods]
impl Foo {
    /// Some doc comment, ignored by this parser entirely.
    #[pyo3(signature = (needle, limit=10))]
    fn find(&mut self, py: Python<'_>, needle: &str, limit: u32) -> PyResult<Option<String>> {
        Ok(None)
    }
}
"""
        parsed = gwt.parse_pymethods(src)
        self.assertEqual(list(parsed.keys()), ["Foo"])
        [meth] = parsed["Foo"]
        self.assertEqual(meth.name, "find")
        self.assertEqual([p.name for p in meth.params], ["self", "py", "needle", "limit"])
        self.assertEqual(meth.params[3].default, "10")
        self.assertEqual(meth.return_type, "PyResult<Option<String>>")
        # `py: Python<'_>` is PyO3's own implicit GIL token, never a Python-
        # visible argument -- dropped at render time, not at parse time.
        rendered = "\n".join(gwt._render_method("Foo", meth))
        self.assertIn("def find(self, needle: str, limit: int = 10) -> Optional[str]: ...", rendered)
        self.assertNotIn("py", rendered)

    def test_new_and_staticmethod_attributes_are_recognized(self) -> None:
        src = """
#[pymethods]
impl Foo {
    #[new]
    fn new(width: u16) -> PyResult<Self> {
        unimplemented!()
    }

    #[staticmethod]
    fn make(width: u16) -> PyResult<Foo> {
        unimplemented!()
    }
}
"""
        parsed = gwt.parse_pymethods(src)
        new_meth, make_meth = parsed["Foo"]
        self.assertTrue(new_meth.is_new)
        self.assertFalse(make_meth.is_new)
        self.assertTrue(make_meth.is_static)
        self.assertFalse(new_meth.is_static)

    def test_parse_pyfunctions_finds_module_level_functions(self) -> None:
        src = """
/// Doc comment.
#[pyfunction]
fn app_path(kind: &str) -> PyResult<String> {
    unimplemented!()
}
"""
        [fn] = gwt.parse_pyfunctions(src)
        self.assertEqual(fn.name, "app_path")
        self.assertEqual([p.rust_type for p in fn.params], ["&str"])
        self.assertEqual(fn.return_type, "PyResult<String>")


class RustTypeToPyTests(unittest.TestCase):
    def test_primitives_and_containers(self) -> None:
        self.assertEqual(gwt.rust_type_to_py("u16"), "int")
        self.assertEqual(gwt.rust_type_to_py("f32"), "float")
        self.assertEqual(gwt.rust_type_to_py("bool"), "bool")
        self.assertEqual(gwt.rust_type_to_py("&str"), "str")
        self.assertEqual(gwt.rust_type_to_py("()"), "None")
        self.assertEqual(gwt.rust_type_to_py("Option<u64>"), "Optional[int]")
        self.assertEqual(gwt.rust_type_to_py("PyResult<bool>"), "bool")
        self.assertEqual(gwt.rust_type_to_py("Vec<String>"), "List[str]")
        self.assertEqual(gwt.rust_type_to_py("Vec<u8>"), "bytes")
        self.assertEqual(
            gwt.rust_type_to_py("Option<(u32, u32, Vec<u8>)>"),
            "Optional[Tuple[int, int, bytes]]",
        )
        self.assertEqual(gwt.rust_type_to_py("&Bound<'_, PyDict>"), "Mapping[str, Any]")
        self.assertEqual(gwt.rust_type_to_py("&Bound<'_, PyList>"), "Sequence[Any]")
        self.assertEqual(gwt.rust_type_to_py("&'static str"), "str")
        self.assertEqual(gwt.rust_type_to_py("QuiAppWaker"), "QuiAppWaker")

    def test_unmapped_type_raises_instead_of_falling_back_to_any(self) -> None:
        with self.assertRaises(ValueError):
            gwt.rust_type_to_py("SomeNeverBeforeSeenType")

    def test_default_literal_mapping(self) -> None:
        self.assertEqual(gwt._default_literal("None"), "None")
        self.assertEqual(gwt._default_literal("true"), "True")
        self.assertEqual(gwt._default_literal("false"), "False")
        self.assertEqual(gwt._default_literal("Vec::new()"), "[]")
        self.assertEqual(gwt._default_literal("1"), "1")
        self.assertEqual(gwt._default_literal('"chrome"'), '"chrome"')
        with self.assertRaises(ValueError):
            gwt._default_literal("SomeConstant::VALUE")


class RenderPyiStubTests(unittest.TestCase):
    def test_renders_a_class_a_free_function_and_the_exception_hierarchy(self) -> None:
        src = """
#[pymethods]
impl QuiAppWaker {
    fn post(&self, value: Py<PyAny>) {}
}

#[pyfunction]
fn app_path(kind: &str) -> PyResult<String> {
    unimplemented!()
}
"""
        text = gwt.render_pyi_stub(src)
        self.assertIn("class QuadrauiError(Exception): ...", text)
        self.assertIn("class QuiAppWaker:", text)
        self.assertIn("def post(self, value: Any) -> None: ...", text)
        self.assertIn("def app_path(kind: str) -> str: ...", text)

    def test_callable_named_params_render_as_callable_not_any(self) -> None:
        src = """
#[pymethods]
impl QuiApp {
    fn set_handler(&mut self, handler: Py<PyAny>) {}
}
"""
        text = gwt.render_pyi_stub(src)
        self.assertIn("def set_handler(self, handler: Callable[..., Any]) -> None: ...", text)


if __name__ == "__main__":
    unittest.main()
