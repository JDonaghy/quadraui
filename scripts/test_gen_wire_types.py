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
    """`main(["--check"])`'s exit code against a throwaway set of output
    paths and a small monkeypatched [`gwt.DEFS`] fixture -- never against
    the real committed `wire.pyi`/`wire.d.ts`/`docs/bindings-wire-format.md`,
    and never through [`gwt.load_defs_from_rust`]'s own `cargo run` (that's
    `scripts/check_wire_types.sh`'s job)."""

    def setUp(self) -> None:
        self._py_out, self._ts_out, self._stub_out, self._doc_out = (
            gwt.PY_OUT,
            gwt.TS_OUT,
            gwt.STUB_OUT,
            gwt.DOC_OUT,
        )
        self._defs = gwt.DEFS
        gwt.DEFS = [gwt.ObjDef("Foo", [gwt.Field("x", gwt.INT)])]

    def tearDown(self) -> None:
        gwt.PY_OUT, gwt.TS_OUT, gwt.STUB_OUT, gwt.DOC_OUT = (
            self._py_out,
            self._ts_out,
            self._stub_out,
            self._doc_out,
        )
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
            gwt.DOC_OUT = Path(tmp) / "bindings-wire-format.md"
            gwt.DOC_OUT.write_text(f"{gwt.DOC_GENERATED_BEGIN}\n{gwt.DOC_GENERATED_END}\n")
            self.assertEqual(gwt.main([]), 0)
            self.assertEqual(gwt.main(["--check"]), 0)

    def test_check_fails_when_committed_output_is_stale(self) -> None:
        import tempfile

        with tempfile.TemporaryDirectory(dir=gwt.REPO_ROOT) as tmp:
            gwt.PY_OUT = Path(tmp) / "wire.pyi"
            gwt.TS_OUT = Path(tmp) / "wire.d.ts"
            gwt.STUB_OUT = Path(tmp) / "_quadraui.pyi"
            gwt.DOC_OUT = Path(tmp) / "bindings-wire-format.md"
            gwt.PY_OUT.write_text("stale\n")
            gwt.TS_OUT.write_text("stale\n")
            gwt.STUB_OUT.write_text("stale\n")
            gwt.DOC_OUT.write_text(
                f"{gwt.DOC_GENERATED_BEGIN}\nstale\n{gwt.DOC_GENERATED_END}\n"
            )
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
        parsed = gwt.parse_pymethods(src, check_floor=False)
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
        parsed = gwt.parse_pymethods(src, check_floor=False)
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

    def test_parses_create_exception_hierarchy_and_aliases_pyexception(self) -> None:
        src = """
create_exception!(_quadraui, QuadrauiError, PyException, "doc");

create_exception!(
    _quadraui,
    InvalidDescriptorError,
    QuadrauiError,
    "doc"
);
"""
        self.assertEqual(
            gwt.parse_create_exceptions(src),
            [("QuadrauiError", "Exception"), ("InvalidDescriptorError", "QuadrauiError")],
        )

    def test_parse_exception_codes_reads_the_get_type_code_table(self) -> None:
        src = """
    for (ty, code) in [
        (py.get_type::<MissingFieldError>(), "missing_field"),
        (py.get_type::<UnknownVariantError>(), "unknown_variant"),
    ] {
        ty.setattr("code", code)?;
    }
"""
        self.assertEqual(
            gwt.parse_exception_codes(src),
            {"MissingFieldError": "missing_field", "UnknownVariantError": "unknown_variant"},
        )

    def test_pymethods_attribute_between_pymethods_and_impl_is_skipped(self) -> None:
        src = """
#[pymethods]
#[cfg(feature = "gtk")]
impl Foo {
    fn bar(&self) {}
}
"""
        parsed = gwt.parse_pymethods(src, check_floor=False)
        self.assertEqual(list(parsed.keys()), ["Foo"])
        self.assertEqual(parsed["Foo"][0].name, "bar")

    def test_floor_check_raises_when_an_expected_class_has_no_methods(self) -> None:
        src = """
#[pymethods]
impl QuiApp {
    fn bar(&self) {}
}
"""
        with self.assertRaises(ValueError):
            gwt.parse_pymethods(src, check_floor=True)


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

    def test_is_param_widens_vec_to_whatever_pyo3_actually_accepts(self) -> None:
        # A return value is always a concrete `list`/`bytes` once
        # pythonized -- `is_param=False` (the default) reflects that.
        self.assertEqual(gwt.rust_type_to_py("Vec<String>"), "List[str]")
        self.assertEqual(gwt.rust_type_to_py("Vec<u8>"), "bytes")
        # A parameter extracts from any sequence (`Vec<T>`) or from
        # `bytes`/`bytearray`/`list[int]` alike (`Vec<u8>`) -- `is_param=True`
        # renders the wider shape PyO3 actually accepts at the call site.
        self.assertEqual(gwt.rust_type_to_py("Vec<String>", is_param=True), "Sequence[str]")
        self.assertEqual(
            gwt.rust_type_to_py("Vec<u8>", is_param=True), "Union[bytes, bytearray, Sequence[int]]"
        )
        self.assertEqual(
            gwt.rust_type_to_py("Option<Vec<(String, Vec<String>)>>", is_param=True),
            "Optional[Sequence[Tuple[str, Sequence[str]]]]",
        )

    def test_optional_any_collapses_to_bare_any(self) -> None:
        self.assertEqual(gwt.rust_type_to_py("Option<Py<PyAny>>"), "Any")

    def test_default_literal_mapping(self) -> None:
        self.assertEqual(gwt._default_literal("None"), "None")
        self.assertEqual(gwt._default_literal("true"), "True")
        self.assertEqual(gwt._default_literal("false"), "False")
        self.assertEqual(gwt._default_literal("Vec::new()"), "...")
        self.assertEqual(gwt._default_literal("1"), "1")
        self.assertEqual(gwt._default_literal('"chrome"'), '"chrome"')
        with self.assertRaises(ValueError):
            gwt._default_literal("SomeConstant::VALUE")


class RenderPyiStubTests(unittest.TestCase):
    def test_renders_a_class_a_free_function_and_the_exception_hierarchy(self) -> None:
        src = """
create_exception!(
    _quadraui,
    QuadrauiError,
    PyException,
    "Base class for every exception this binding raises itself."
);

#[pymethods]
impl QuiAppWaker {
    fn post(&self, value: Py<PyAny>) {}
}

#[pyfunction]
fn app_path(kind: &str) -> PyResult<String> {
    unimplemented!()
}
"""
        text = gwt.render_pyi_stub(src, check_floor=False)
        self.assertIn("class QuadrauiError(Exception): ...", text)
        self.assertIn("class QuiAppWaker:", text)
        self.assertIn("def post(self, value: Any) -> None: ...", text)
        self.assertIn("def app_path(kind: str) -> str: ...", text)

    def test_exception_with_a_code_table_entry_gets_a_code_str_attribute(self) -> None:
        src = """
create_exception!(
    _quadraui,
    QuadrauiError,
    PyException,
    "Base class for every exception this binding raises itself."
);

create_exception!(
    _quadraui,
    InvalidDescriptorError,
    QuadrauiError,
    "doc"
);

create_exception!(
    _quadraui,
    MissingFieldError,
    InvalidDescriptorError,
    "doc"
);

#[pymodule]
fn _quadraui(m: &Bound<'_, PyModule>) -> PyResult<()> {
    for (ty, code) in [
        (py.get_type::<MissingFieldError>(), "missing_field"),
    ] {
        ty.setattr("code", code)?;
    }
    Ok(())
}
"""
        text = gwt.render_pyi_stub(src, check_floor=False)
        self.assertIn("class QuadrauiError(Exception): ...", text)
        self.assertIn("class InvalidDescriptorError(QuadrauiError): ...", text)
        self.assertIn("class MissingFieldError(InvalidDescriptorError):\n    code: str", text)

    def test_callable_named_params_render_as_callable_not_any(self) -> None:
        src = """
#[pymethods]
impl QuiApp {
    fn set_handler(&mut self, handler: Py<PyAny>) {}
}
"""
        text = gwt.render_pyi_stub(src, check_floor=False)
        self.assertIn("def set_handler(self, handler: Callable[..., Any]) -> None: ...", text)


class OverridesResolveTests(unittest.TestCase):
    """[`gwt._check_overrides_resolve`] -- a `PARAM_OVERRIDES`/
    `RETURN_OVERRIDES` key that doesn't name a parsed method/parameter
    (e.g. a stale entry left behind by a Rust-side rename) must fail
    generation loudly instead of silently reverting to the generic
    mapping."""

    def setUp(self) -> None:
        self._param_overrides = dict(gwt.PARAM_OVERRIDES)
        self._return_overrides = dict(gwt.RETURN_OVERRIDES)

    def tearDown(self) -> None:
        gwt.PARAM_OVERRIDES.clear()
        gwt.PARAM_OVERRIDES.update(self._param_overrides)
        gwt.RETURN_OVERRIDES.clear()
        gwt.RETURN_OVERRIDES.update(self._return_overrides)

    def test_stale_param_override_key_raises(self) -> None:
        gwt.PARAM_OVERRIDES.clear()
        gwt.RETURN_OVERRIDES.clear()
        gwt.PARAM_OVERRIDES[("QuiApp", "set_handler", "no_such_param")] = "Any"
        with self.assertRaises(ValueError):
            gwt._check_overrides_resolve({"QuiApp": []}, [])

    def test_stale_return_override_key_raises(self) -> None:
        gwt.PARAM_OVERRIDES.clear()
        gwt.RETURN_OVERRIDES.clear()
        gwt.RETURN_OVERRIDES[("QuiApp", "no_such_method")] = "Any"
        with self.assertRaises(ValueError):
            gwt._check_overrides_resolve({"QuiApp": []}, [])

    def test_matching_override_keys_do_not_raise(self) -> None:
        gwt.PARAM_OVERRIDES.clear()
        gwt.RETURN_OVERRIDES.clear()
        gwt.PARAM_OVERRIDES[("QuiApp", "set_handler", "value")] = "Any"
        gwt.RETURN_OVERRIDES[("QuiApp", "theme")] = "Dict[str, Any]"
        meth = gwt.RustMethod(
            name="set_handler",
            params=[gwt.RustParam(name="value", rust_type="Py<PyAny>")],
            return_type=None,
        )
        theme_meth = gwt.RustMethod(name="theme", params=[], return_type="Py<PyDict>")
        gwt._check_overrides_resolve({"QuiApp": [meth, theme_meth]}, [])


class MarkdownRenderingTests(unittest.TestCase):
    """[`gwt.render_markdown_reference`] against small throwaway schemas --
    an ordinary struct, a `flat()`-built discriminated union (with and
    without a `common` field), and an alias -- mirroring
    `DiscriminatedRenderingTests`'/`KeywordCollidingFieldTests`' own
    fixture-based shape above, for the Markdown target instead of the
    Python/TypeScript ones."""

    def test_ordinary_obj_renders_a_field_table(self) -> None:
        obj = gwt.ObjDef(
            "Sample",
            [
                gwt.Field("id", gwt.STR, doc="the widget id"),
                gwt.Field("count", gwt.INT, optional=True),
            ],
            doc="Sample's own doc.",
        )
        text = gwt.render_markdown_reference([obj])
        self.assertIn('<a id="wire-sample"></a>', text)
        self.assertIn("#### `Sample`", text)
        self.assertIn("Sample's own doc.", text)
        self.assertIn("| Field | Type | Required | Description |", text)
        self.assertIn("| `id` | `string` | yes | the widget id |", text)
        self.assertIn("| `count` | `number` | no |  |", text)
        # Table of contents links to the same anchor the heading defines.
        self.assertIn("[`Sample`](#wire-sample)", text)

    def test_obj_with_no_fields_renders_a_placeholder(self) -> None:
        obj = gwt.ObjDef("Empty", [])
        text = gwt.render_markdown_reference([obj])
        self.assertIn("_(no fields)_", text)
        self.assertNotIn("| Field | Type |", text)

    def test_discriminated_union_renders_a_tag_table_plus_common_fields(self) -> None:
        obj = gwt.flat(
            "_Sample",
            [("Foo", gwt.STR), ("Bar", gwt.INT)],
            common=[gwt.Field("hit", gwt.STR, optional=True, doc="hit region")],
        )
        text = gwt.render_markdown_reference([obj])
        self.assertIn(
            "Exactly one of the tags below is present on the wire "
            "(a discriminated union):",
            text,
        )
        self.assertIn("| Tag | Payload type | Description |", text)
        self.assertIn("| `Foo` | `string` |  |", text)
        self.assertIn("| `Bar` | `number` |  |", text)
        self.assertIn("Present alongside whichever tag above is set:", text)
        self.assertIn("| `hit` | `string` | no | hit region |", text)

    def test_discriminated_union_without_common_omits_that_section(self) -> None:
        obj = gwt.flat("_NoCommon", [("Foo", gwt.STR)])
        text = gwt.render_markdown_reference([obj])
        self.assertNotIn("Present alongside whichever tag above is set:", text)

    def test_alias_renders_its_target_type(self) -> None:
        alias = gwt.AliasDef("WidgetId", gwt.STR, doc="A widget's own label.")
        text = gwt.render_markdown_reference([alias])
        self.assertIn("A widget's own label.", text)
        self.assertIn("Alias for `string`.", text)

    def test_pipe_in_a_union_type_is_escaped_so_it_cannot_break_the_table(self) -> None:
        obj = gwt.ObjDef("Sample2", [gwt.Field("x", gwt.NULLABLE(gwt.STR))])
        text = gwt.render_markdown_reference([obj])
        self.assertIn("| `x` | `string \\| null` | yes |  |", text)

    def test_doc_with_embedded_newline_is_collapsed_to_one_line(self) -> None:
        obj = gwt.ObjDef("Sample3", [gwt.Field("x", gwt.STR, doc="line one\nline two")])
        text = gwt.render_markdown_reference([obj])
        self.assertIn("| `x` | `string` | yes | line one line two |", text)


class RenderDocPageTests(unittest.TestCase):
    """[`gwt.render_doc_page`]'s splice between `DOC_GENERATED_BEGIN`/
    `DOC_GENERATED_END` -- hand-written prose on either side of the markers
    must survive untouched, and a page missing either marker must raise
    rather than silently dropping its prose."""

    def test_splices_generated_content_between_markers(self) -> None:
        template = (
            "# Prose before\n\n"
            f"{gwt.DOC_GENERATED_BEGIN}\nstale content\n{gwt.DOC_GENERATED_END}\n\n"
            "# Prose after\n"
        )
        obj = gwt.ObjDef("Sample", [gwt.Field("x", gwt.STR)])
        text = gwt.render_doc_page(template, [obj])
        self.assertIn("# Prose before", text)
        self.assertIn("# Prose after", text)
        self.assertIn("#### `Sample`", text)
        self.assertNotIn("stale content", text)
        # Re-splicing the already-generated page is idempotent.
        self.assertEqual(text, gwt.render_doc_page(text, [obj]))

    def test_missing_begin_marker_raises(self) -> None:
        template = f"# Prose\n\nno begin marker here\n{gwt.DOC_GENERATED_END}\n"
        with self.assertRaises(ValueError):
            gwt.render_doc_page(template, [])

    def test_missing_end_marker_raises(self) -> None:
        template = f"# Prose\n\n{gwt.DOC_GENERATED_BEGIN}\nno end marker here\n"
        with self.assertRaises(ValueError):
            gwt.render_doc_page(template, [])


if __name__ == "__main__":
    unittest.main()
