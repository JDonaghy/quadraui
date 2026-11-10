//! Dumps [`quadraui_binding_core::wire_schema::schema`] to stdout as JSON --
//! `scripts/gen_wire_types.py` shells out to this binary instead of
//! hand-transcribing the wire schema into Python, so the generated
//! `wire.pyi`/`wire.d.ts` are produced from the Rust schema (and the
//! [`quadraui_binding_core::wire_schema::coverage`] drift guards alongside
//! it) rather than a copy of it.
//!
//! Usage: `cargo run --quiet -p quadraui-binding-core --bin export_wire_schema`

fn main() {
    let defs = quadraui_binding_core::wire_schema::schema();
    let json = serde_json::to_string(&defs).expect("wire schema must serialize to JSON");
    println!("{json}");
}
