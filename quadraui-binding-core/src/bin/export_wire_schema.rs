//! Prints [`quadraui_binding_core::wire::schema::wire_schema`] to stdout:
//! one JSON Schema document with `$defs` for every wire type, a `kinds`
//! map from each leaf widget kind to its descriptor and a `roots` map of
//! the whole-message shapes. `scripts/gen_wire_types.py` reads it to
//! generate `wire.pyi`, `wire.d.ts` and the reference section of
//! `docs/bindings-wire-format.md`.
//!
//! Usage: `cargo run --quiet -p quadraui-binding-core --features schema --bin export_wire_schema`

fn main() {
    let doc = quadraui_binding_core::wire::schema::wire_schema();
    let json = serde_json::to_string_pretty(&doc).expect("the wire schema serializes to JSON");
    println!("{json}");
}
