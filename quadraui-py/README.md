# quadraui

Python bindings for [quadraui](https://github.com/JDonaghy/quadraui), a
cross-platform Rust UI toolkit. This package lets a Python program build
`quadraui` descriptors and drive them headlessly — no Rust toolchain
required:

```bash
pip install quadraui
```

```python
from quadraui import QuiApp
```

Today this binding ships the TUI-feature surface of `quadraui` only (see the
*Linux wheel* note below); the native GTK/macOS/Windows backends are the next
step on the ladder toward a pure-Python cross-platform UI.

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
