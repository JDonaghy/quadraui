# Linux wheels with a native GTK backend

How `pip install quadraui` gives Linux users a working `run_native()` (a GTK 4
window) while keeping `import quadraui` and `run_tui()` working on hosts with
no GTK at all. Tracking issue: #3081 (epic #3080).

## Outcome

| Host | Wheel pip picks | `run_native()` |
|---|---|---|
| glibc >= 2.39 with GTK >= 4.10 (Ubuntu 24.04+, Fedora 40+, Debian 13+, Arch) | `manylinux_2_39` | opens a GTK window |
| glibc >= 2.39, no GTK 4 (servers, minimal containers) | `manylinux_2_39` | raises `UnsupportedError` naming `libgtk-4.so.1` |
| glibc 2.17 to 2.38 (Ubuntu 22.04, Debian 12, RHEL 9) | `manylinux_2_17` (TUI-only, as today) | raises `UnsupportedError` explaining the glibc floor |

`import quadraui`, the headless driver and `run_tui()` behave the same in
every row.

Ubuntu 22.04 (GTK 4.6) and Debian 12 (GTK 4.8) do not get a GTK window. That
limit comes from GTK, not from packaging: `quadraui`'s `gtk4` dependency enables
the `v4_10` feature because `quadraui/src/gtk/services.rs` calls
`GtkFileDialog` and `GtkAlertDialog`, both new in GTK 4.10. A binary that
calls those functions cannot load against an older `libgtk-4.so.1`. See
*Reaching GTK 4.6 hosts* below for what it would take.

## The constraint

A gtk-rs build links the GTK stack as ordinary `DT_NEEDED` entries. Measured
on precision (Linux Mint 22.3, which is Ubuntu 24.04-based: glibc 2.39,
GTK 4.14.5), maturin 1.16.0, auditwheel 6.8.2:

```
$ readelf -d _quadraui.abi3.so | grep NEEDED          # built without `gtk`
 libgcc_s.so.1  libm.so.6  libc.so.6  ld-linux-x86-64.so.2

$ readelf -d _quadraui.abi3.so | grep NEEDED          # built with `gtk`
 libpangoft2-1.0.so.0  libpango-1.0.so.0  libgobject-2.0.so.0
 libglib-2.0.so.0  libfontconfig.so.1  libpangocairo-1.0.so.0
 libcairo.so.2  libgtk-4.so.1  libgdk_pixbuf-2.0.so.0  libgio-2.0.so.0
 libgcc_s.so.1  libm.so.6  libc.so.6  ld-linux-x86-64.so.2
```

Two consequences:

1. **A single GTK-linked extension cannot import without GTK.** CPython
   loads extension modules with `dlopen(..., RTLD_NOW)`
   (`sys.getdlopenflags() == os.RTLD_NOW` on precision), so a missing
   `libgtk-4.so.1`, or an older one that lacks a symbol the module uses, fails
   the whole `import`. That would break `run_tui()` users on every headless
   host.
2. **auditwheel vendors whatever is not on its allow-list.** `auditwheel
   show` on the GTK build reports the platform tag as plain `linux_x86_64`
   and lists the whole transitive stack (libthai, libdatrie, libjpeg,
   libtiff, libxkbcommon, libstdc++, libmount, libselinux, ...). A default
   `auditwheel repair` would copy all of that into the wheel. That would be
   large, and a vendored GTK cannot match the host's display server, theme,
   input methods or fonts.

## Options considered

**A. Two extension modules in one wheel (chosen).** The wheel carries the
existing GTK-free `_quadraui` plus `_quadraui_gtk`: the same crate built with
the `gtk-ext` feature (`gtk`, with the module renamed so it exports
`PyInit__quadraui_gtk`). `python/quadraui/_ext.py` imports `_quadraui_gtk`
first and falls back to `_quadraui` on `ImportError`. Each `.so` statically
links its own copy of `quadraui`, `quadraui-binding-core` and PyO3, so the
two must never meet in one process: a `QuiApp` from one is not an instance
of the other's `QuiApp`, and their exception classes are distinct. The loader
therefore loads exactly one of them and registers it as `quadraui._quadraui`
in `sys.modules`. Every `from ._quadraui import ...` in the package, and
every `from quadraui import _quadraui` in tests and user code, then reaches
the same module object. No state ever has to cross between the two builds.

**B. Load GTK at runtime with `dlopen`.** gtk-rs's `-sys` crates link by
name through `system-deps`/pkg-config, and every generated binding is an
`extern "C"` declaration that resolves at load time. Making GTK lazy would
mean replacing that linkage for gtk4, gdk4, gsk4, pango, pangocairo, cairo,
glib, gobject, gio and gdk-pixbuf with a hand-maintained function-pointer
table, or patching the generated `-sys` crates. That is a fork of gtk-rs, so
it is impractical.

**C. A separate distribution or extra (`pip install quadraui[gtk]`).** An
extra cannot swap the extension module inside `quadraui` itself, so this
would need a second distribution (`quadraui-gtk`) shipping its own complete
copy of the extension, plus the same loader logic as A. It produces the same
bytes on disk as A, adds a second package to publish and version-lock, and
leaves `pip install quadraui` without a window. A does the same job inside
the one package users already install.

## How the wheel is built

`.github/workflows/quadraui-py-wheels.yml`, job `linux-gtk`, once per
architecture: natively on `ubuntu-24.04` (x86_64) and `ubuntu-24.04-arm`
(aarch64), each with `libgtk-4-dev`:

1. `cargo build --release -p quadraui-py --lib --features pyo3/extension-module,gtk-ext`,
   then copy `target/release/libquadraui_py.so` to
   `quadraui-py/python/quadraui/_quadraui_gtk.abi3.so`.
2. `maturin build --release --auditwheel skip --features pyo3/extension-module`
   builds the GTK-free `_quadraui` and packs the prebuilt `_quadraui_gtk`
   alongside it, through `[tool.maturin] include` in `pyproject.toml`.
   maturin leaves a prebuilt `.so` under `python-source` out of the wheel
   unless it is listed there.
3. `auditwheel repair --plat manylinux_2_39_<arch> --exclude <lib> ...` for
   the ten system libraries in the `DT_NEEDED` list above, plus
   `libcairo-gobject.so.2`, which the aarch64 build also needs. With those
   excluded, nothing is vendored, so the wheel has no `quadraui.libs/`
   directory, and auditwheel accepts the `manylinux_2_39` tag.
4. A layout check fails the job if anything was vendored, if either `.so`
   is missing, if `_quadraui_gtk` does not need `libgtk-4.so.1`, or if
   `_quadraui` needs GLib. That catches a new GTK-side dependency that
   needs adding to the exclude list.

The `build` job's `manylinux: auto` wheel stays TUI-only, with a glibc 2.17
floor; its `ubuntu-24.04-arm` leg builds the aarch64 one. Every wheel is
uploaded as a `quadraui-py-wheel-*` artifact, which `quadraui-py-release.yml`'s publish step already collects.
For the same version and platform, pip ranks newer manylinux tags first
(`packaging.tags.sys_tags()` on a glibc 2.39 host starts at
`manylinux_2_39`), so a host that can use the GTK wheel gets it, and older
hosts get the TUI wheel instead of failing to install.

### Why not a manylinux image

`manylinux_2_28` (AlmaLinux 8) has no GTK 4 packages. `manylinux_2_34`
(AlmaLinux 9) ships GTK 4.6, older than the 4.10 that `gtk4-sys`'s `v4_10`
feature asks pkg-config for. A build there would need GTK built from source
in the container, or system-deps overrides that skip the version check and
link against a library missing the 4.10 symbols. Either way, the result
would still only load on GTK >= 4.10 hosts, and every mainstream distro with
GTK >= 4.10 already has glibc >= 2.39. Building on Ubuntu 24.04 gives the
same reach with a stock toolchain. The glibc 2.39 floor costs nothing that
GTK 4.10 had not already ruled out.

## Runtime behaviour

`python/quadraui/_ext.py`:

- loads `_quadraui_gtk` if present and loadable; otherwise `_quadraui`.
  A `ModuleNotFoundError` for `_quadraui_gtk` itself (macOS, Windows, the
  TUI-only Linux wheel, a `maturin develop` checkout) moves on quietly. Any
  other `ImportError` (a missing `.so`, an undefined symbol) is recorded.
- on Linux, when the loaded module has no native backend, sets
  `native_unavailable_reason`. `quadraui/__init__.py` then installs a
  `QuiApp.run_native` that raises `UnsupportedError` with that reason. The
  message includes the loader's own error and the install command for apt,
  dnf and pacman. The loader names only the first `DT_NEEDED` entry it cannot
  resolve. On a stock `ubuntu:24.04` container that is
  `libpangoft2-1.0.so.0`, not GTK, as CI showed. So `_ext` also tries to
  open `libgtk-4.so.1` with `ctypes`, and if that fails the message says
  `(libgtk-4.so.1 is not installed)`.
- `QUADRAUI_DISABLE_GTK=1` skips the GTK variant, for processes that never
  open a window. Loading GTK adds about 9 to 12 ms to `import quadraui`
  (warm cache, precision), against about 1 ms for the GTK-free module.

A side effect: `hasattr(QuiApp, "run_native")` is now true on every Linux
build. Callers that want to fall back should catch `UnsupportedError`.

Cost: the `manylinux_2_39` wheel is about 12.6 MB, against about 6.1 MB for the
TUI-only wheel, because it carries the extension twice.

## Evidence from the spike (precision)

- Both builds produced; `DT_NEEDED` as above; `PyInit__quadraui_gtk` is the
  only `PyInit_*` symbol exported by the `gtk-ext` build.
- `auditwheel repair --plat manylinux_2_39_x86_64 --exclude ...` on the
  two-module wheel: tagged `cp38-abi3-manylinux_2_39_x86_64`, both `.so`
  inside, no `quadraui.libs/`.
- Installed into a clean venv: `quadraui._quadraui` is
  `quadraui._quadraui_gtk` and `_NATIVE_BACKEND == "gtk"`. The full
  `quadraui-py/tests` suite passes against the installed wheel under
  `xvfb-run` (654 passed), and `test_run_native.py` opens real GTK windows.
- `quadraui-py/ci/linux_wheel_smoke.py gtk` under Xvfb: `run_native`
  painted 4 frames and returned on `QUADRAUI_GTK_SMOKE_MS`.
- Fallback path: in a second venv, `patchelf --replace-needed libgtk-4.so.1
  libgtk-4.so.999` on the installed `_quadraui_gtk`. This makes the dynamic
  loader fail exactly as it does on a host without the library; it is not a
  stand-in for a GTK-less distro, which CI covers below. `import quadraui`
  succeeded on `_quadraui`, the headless render and `run_tui` checks passed,
  and `run_native` raised `UnsupportedError: run_native needs the system GTK 4
  libraries, but the GTK build of quadraui could not be loaded:
  libgtk-4.so.999: cannot open shared object file: ...`.
- TUI-only wheel (no `_quadraui_gtk` inside): `run_native` raises the
  glibc-floor message; everything else unchanged.

## CI proof in clean containers

Job `linux-smoke` installs from a directory holding **both** Linux wheels, so
pip's tag choice is tested too. It runs `quadraui-py/ci/linux_wheel_smoke.py`:

| Container | Packages | Mode | Asserts |
|---|---|---|---|
| `ubuntu:24.04` | python3, libgtk-4-1, Xvfb, dbus | `gtk` | GTK wheel picked, `_quadraui_gtk` loaded, headless render, `run_tui` reaches the terminal runner, `run_native` paints at least one frame |
| `fedora:42` | python3, gtk4, Xvfb, dbus | `gtk` | same |
| `ubuntu:24.04` | python3 only | `missing-gtk` | GTK wheel picked, fell back to `_quadraui`, TUI works, `run_native` error names `libgtk-4.so.1` |
| `debian:12` | python3 only | `tui-only` | TUI wheel picked (glibc 2.36), TUI works, `run_native` error names the glibc floor |

The aarch64 rows run on `ubuntu-24.04-arm` against the aarch64 wheels:
`ubuntu:24.04` in `gtk` and `missing-gtk` modes, and `debian:12` in
`tui-only` mode.

`GSK_RENDERER=cairo` is set because containers have no GPU device. Fedora 40
is past end of life, so its package mirrors are unreliable in CI; Fedora 42
stands in for it. Fedora 40's own glibc (2.39) and GTK (4.14) match Ubuntu
24.04's, which is tested directly.

## Developer notes

- `maturin develop --features gtk` still builds a single `_quadraui` with
  GTK, as before; the loader finds no `_quadraui_gtk` and uses it.
- A stale `python/quadraui/_quadraui_gtk.abi3.so` left in a checkout from
  a local wheel build is gitignored, but `_ext.py` prefers it over a fresh
  `maturin develop` build. Delete it when switching back.

## Follow-ups

- **Reaching GTK 4.6 hosts (Ubuntu 22.04, Debian 12).** This would mean
  dropping the `v4_10` feature from `quadraui`'s `gtk4` dependency. The 4.10
  dialogs would then be resolved at runtime (`gtk::check_version` plus
  `GtkFileChooserNative`/`GtkMessageDialog` fallbacks), which touches
  `quadraui/src/gtk/services.rs` and is outside this packaging change. With
  that done, the GTK variant could be built in `manylinux_2_34` (AlmaLinux 9,
  GTK 4.6) and the floor drops to glibc 2.34.
- **Node bindings** are out of scope; the same two-module approach would
  apply to a napi `.node` file.
