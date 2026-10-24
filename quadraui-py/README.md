# quadraui

Python bindings for [quadraui](https://github.com/JDonaghy/quadraui), a
cross-platform Rust UI toolkit. This package lets a Python program build
`quadraui` descriptors and drive them headlessly, or call `run_tui()` to take
over a real terminal with them — no Rust toolchain required:

```bash
pip install quadraui
```

```python
from quadraui import QuiApp
```

`run_native()` opens a real AppKit window on macOS, a real
Direct2D/DirectWrite window on Windows, or a real GTK window on Linux, built
from the same descriptors `run_tui()` reads headlessly or over a terminal.
The prebuilt macOS and Windows wheels on PyPI ship `run_native()` built in —
`pip install quadraui` is enough, no Rust toolchain needed. The prebuilt
Linux wheel is TUI-only (see the *Wheels* section below for why); call
`hasattr(QuiApp, "run_native")` to check at runtime, or use `run_tui()`,
which every platform's wheel supports unchanged.

Full docs, the primitive reference, and worked examples live on the project
site: <https://jdonaghy.github.io/quadraui/>.

## Wheels

Prebuilt `abi3` wheels (CPython >= 3.8) are published for Linux, macOS, and
Windows, each with the native-window feature it can ship as a clean,
self-contained wheel:

- **macOS** — built with the `macos` feature (Core Graphics/AppKit via
  `objc2`), which links only against frameworks every Mac already has.
  `run_native()` works out of the box.
- **Windows** — built with the `win` feature (Direct2D/DirectWrite), which
  links only against system DLLs every Windows install already has.
  `run_native()` works out of the box.
- **Linux** — built with neither `gtk` feature nor any other native
  feature, so it stays TUI-only. `gtk` needs a real `libgtk-4` (plus its own
  transitive GLib/Pango/Cairo stack) at both link and run time, and GTK is
  not in the handful of C libraries `auditwheel`/manylinux knows how to
  vendor into a portable wheel — there is no manylinux-compatible way to
  ship it today. A Linux user who wants `run_native()` installs system GTK
  development packages and builds from source with `maturin develop
  --features gtk`; `run_tui()` needs none of that and works on the
  published wheel as-is.

## License

Dual-licensed under MIT or Apache-2.0, at your option — see
[`LICENSE-MIT`](https://github.com/JDonaghy/quadraui/blob/main/LICENSE-MIT) and
[`LICENSE-APACHE`](https://github.com/JDonaghy/quadraui/blob/main/LICENSE-APACHE)
in the repository root.
