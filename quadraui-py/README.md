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

`run_native()` opens a real GTK window on Linux, a real AppKit window on
macOS, or a real Direct2D/DirectWrite window on Windows, built from the same
descriptors `run_tui()` reads headlessly or over a terminal — the
`gtk`/`macos`/`win` Cargo features that back it are all source-available
today. The prebuilt wheels on PyPI are still TUI-only (see the *Linux wheel*
note below); building with `maturin develop --features gtk`, `--features
macos`, or `--features win` is required for `run_native()` until a
native-enabled wheel ships.

Full docs, the primitive reference, and worked examples live on the project
site: <https://jdonaghy.github.io/quadraui/>.

## Wheels

Prebuilt `abi3` wheels (CPython >= 3.8) are published for Linux, macOS, and
Windows. Linux has no manylinux-compatible system GTK to bundle, so every
platform's wheel is TUI-only by construction — there is no separate
"GTK-less Linux variant", because that is the only variant on any platform
today.

## License

Dual-licensed under MIT or Apache-2.0, at your option — see
[`LICENSE-MIT`](https://github.com/JDonaghy/quadraui/blob/main/LICENSE-MIT) and
[`LICENSE-APACHE`](https://github.com/JDonaghy/quadraui/blob/main/LICENSE-APACHE)
in the repository root.
