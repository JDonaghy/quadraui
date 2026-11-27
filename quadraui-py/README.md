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
The prebuilt wheels on PyPI ship `run_native()` built in — `pip install
quadraui` is enough, no Rust toolchain needed. On Linux it needs the system
GTK 4 (4.10 or newer) and glibc 2.39 or newer (Ubuntu 24.04, Fedora 40,
Debian 13 and later); see the *Wheels* section below. Where those are
missing, `import quadraui` and `run_tui()` still work and `run_native()`
raises `UnsupportedError` explaining what is missing, so an app can catch
that and fall back to `run_tui()`.

Full docs, the primitive reference, and worked examples live on the project
site: <https://jdonaghy.github.io/quadraui/>.

**Building an app with a coding agent?** The package ships its own guide,
`AGENTS.md`. It covers how to structure a `quadraui.app` app, test it with
the harness, use hot reload, and which gotchas to avoid. It sits next to the
installed module, so it always matches your installed version:
`python -c "import quadraui, os; print(os.path.join(os.path.dirname(quadraui.__file__), 'AGENTS.md'))"`.
[Read it on GitHub](https://github.com/JDonaghy/quadraui/blob/main/quadraui-py/python/quadraui/AGENTS.md).

## Building and testing an app

`quadraui.app` is a thin layer over `QuiApp`: a `view()` returning typed
node trees, `on_*` callbacks on those nodes instead of one app-wide event
switch, and `App.enable_hot_reload(module)` to pick up edits to your view
code while the app runs, keeping the state you built up.

`quadraui.testing` is the harness you write that app's tests against — it
queries the frame the app actually rendered and synthesises the gestures a
user performs, so a test never spells out a wire event or a coordinate:

```python
def test_opening_a_folder(quadraui_harness):      # fixture ships with the package
    h = quadraui_harness(MyApp(width=80, height=30).app)

    h.find(role="tree").row("alpha").double_click()
    h.press("Enter")

    assert h.find(role="data_table").row_texts == ["a1.txt", "a2.txt"]
    h.assert_text("alpha")
```

## Version compatibility

This package's minor version always equals the `quadraui` Rust crate's own
minor version; the patch component moves independently in either direction
(see
[`BINDING_VERSION_POLICY.md`](https://github.com/JDonaghy/quadraui/blob/main/quadraui/docs/decisions/BINDING_VERSION_POLICY.md)).
A released `quadraui` version is always `pip install`-able as the matching
`quadraui-py` minor:

| `quadraui` (Rust crate) | `quadraui` (this PyPI package) |
|---|---|
| 0.*Y*.x | 0.*Y*.x — any patch |

For example, both `quadraui` crate `0.10.1` and `0.10.3` pair with any
`quadraui-py` `0.10.x` release; a crate `0.11.0` needs a `quadraui-py`
`0.11.x` release, not `0.10.x`.

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
- **Linux** — two wheels, and pip picks the newest one the host's glibc
  allows:
  - `manylinux_2_39` (glibc 2.39+: Ubuntu 24.04, Fedora 40, Debian 13 and
    later) carries two builds of the extension: one with the `gtk` feature,
    linking the system GTK 4 libraries, and one without. `import quadraui`
    loads the GTK build when `libgtk-4.so.1` (4.10 or newer) is installed
    and the GTK-free build otherwise, so `run_native()` opens a GTK window
    with GTK present and raises `UnsupportedError` naming the missing
    library without it. Install GTK with `sudo apt install libgtk-4-1`
    (Ubuntu/Debian) or `sudo dnf install gtk4` (Fedora). Set
    `QUADRAUI_DISABLE_GTK=1` to skip loading GTK in a process that never
    opens a window.
  - `manylinux_2_17` (older glibc, e.g. Ubuntu 22.04, Debian 12) is
    TUI-only: those distributions ship GTK older than 4.10, which the GTK
    backend needs. `run_native()` raises `UnsupportedError` there.

## License

Dual-licensed under MIT or Apache-2.0, at your option — see
[`LICENSE-MIT`](https://github.com/JDonaghy/quadraui/blob/main/LICENSE-MIT) and
[`LICENSE-APACHE`](https://github.com/JDonaghy/quadraui/blob/main/LICENSE-APACHE)
in the repository root.
