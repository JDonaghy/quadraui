# Diplomat spike — not attempted (#1640)

This directory is reserved for the Diplomat-generated half of the #1640
spike (`src/`, `python/`, `ts/` scaffolds only). The hand-rolled
`extern "C"` + cbindgen half (`spikes/ffi-cbindgen/`) was built and
evaluated first; there was not enough remaining budget in this work
session to also stand up a working Diplomat skeleton against the same
descriptor-store + callback-vtable + run/step/wake shape.

See the PR/issue discussion for the recommendation drawn from the
cbindgen-side evidence alone, and for what a follow-up session building
this half would need to do: define the same three surfaces
(`insert`/`update`/`remove`, one callback in a vtable, `run`/`step`/`wake`)
in Diplomat's `#[diplomat::bridge]` dialect, generate C + Python
(nanobind) + TypeScript from it, and write the matching Python client
for a side-by-side comparison with `spikes/ffi-cbindgen/python/client.py`.
