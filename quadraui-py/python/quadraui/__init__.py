"""Python bindings over quadraui's TUI driver, headless or real.

``QuiApp`` is a cell-grid app: register a ``render`` callback with
``set_render(callback)`` that returns a layout tree of dict-shaped
descriptors (button, text display, list, tree, data table, tab bar,
``progress``/``spinner``/``image``/..., or a ``"row"``/``"column"``
container nesting any number of them), then either read the rendered frame
back headlessly with ``screen()`` / ``screen_contains()``, or call
``run_tui()`` to hand the same descriptors to a real terminal -- raw mode,
alternate screen, live redraw on real keyboard/resize events -- until a
registered handler raises to quit.

The older fixed-slot ``insert_button``/``insert_text_display``/
``insert_list``/``insert_tree``/``insert_data_table``/``insert_tab_bar``,
the additive ``insert_extra(kind, descriptor)``/``clear_extra(kind)`` (and
its typed ``insert_progress``/``insert_spinner``/``insert_image``
convenience wrappers), and ``slot_clear`` still work but are **deprecated**:
each one raises a ``DeprecationWarning`` naming the inline layout leaf that
replaces it, and is scheduled for removal in a future breaking release.

Every error ``QuiApp`` raises itself (as opposed to one a registered
Python callback raised, which passes through unchanged) is a
``QuadrauiError`` subclass: ``InvalidDescriptorError``, ``SlotOccupiedError``,
``UnknownSlotError``, ``InvalidDimensionsError``, ``InvalidCharError``,
``PanicError``, ``PoisonedError``, ``InternalError``,
``NativeAlreadyRunError``, ``NoDisplayError``, ``NotOnMainThreadError``,
``InvalidAppIdError``, ``InvalidFontRoleError``, and ``UnsupportedError``.
A descriptor-decode failure (``quadraui_binding_core::DecodeError``) raises
one of ``InvalidDescriptorError``'s own subclasses -- ``MissingFieldError``,
``UnknownVariantError``, ``TypeMismatchError``, ``UnknownKeyError``,
``VersionMismatchError``, or ``DecodeFailedError``, chosen by the decoder's
own variant, each with a ``code`` class attribute and a message naming the
full path into the layout tree the problem was found at.

``QuiApp``'s platform-services methods -- ``check_for_update``/
``download_and_stage``/``apply_on_quit`` (auto-update), ``begin_power_save_block``/
``end_power_save_block`` (the sleep/suspend inhibitor), ``set_login_item``/
``login_item_enabled`` (launch at login), ``set_as_default_protocol_client``/
``is_default_protocol_client``/``remove_as_default_protocol_client`` (OS
protocol registration), ``begin_drag_out`` (OS-level drag-out of this app's
window), and ``add_recent_document``/``clear_recent_documents``/
``set_dock_menu``/``set_jump_list`` (recent docs, the Dock icon menu, and the
Windows jump list) -- each raise ``UnsupportedError`` on a backend with no
implementation (the headless ``TuiBackend`` implements none of them) rather
than silently no-opping. The void-returning ones among them (every one
except ``check_for_update``, ``download_and_stage``,
``begin_power_save_block``, ``login_item_enabled``, and
``is_default_protocol_client``, which each return a value a caller needs
right away) have a matching method on the ``QuiAppActions`` handle
``QuiApp.actions()`` returns, for use from inside a registered handler.

``quadraui.app_path(kind)`` resolves a standard, per-app filesystem location
(``"user_data"``, ``"app_config"``, ``"app_cache"``, ``"logs"``, ``"temp"``,
``"downloads"``, ``"documents"``, ``"home"``) with no live ``QuiApp`` needed;
``quadraui.set_app_identifier(identifier)`` sets the process-global
identifier the app-scoped kinds resolve under.
``quadraui.install_crash_handler(cfg)`` installs a process-global panic hook
that writes a crash record on an uncaught panic, from a ``cfg`` dict whose
keys (``"app_version"``, ``"upload_endpoint"``, ``"metadata"``,
``"log_dir"``) are all optional.

``QuiApp.viewport()``, ``line_height()``, ``char_width()``,
``list_char_width()``, ``chrome_line_height()``, ``scrollbar_reserve()``,
``scales_text_rows()``, ``snap_height(h)``, ``measure_text(text, role)``,
and ``backend_caps()`` expose the headless backend's own layout metrics and
declared capabilities, so an app can size and scroll content against real
measurements instead of a hardcoded guess.

``QuiApp.waker()`` returns a ``QuiAppWaker`` -- unlike ``QuiApp`` itself,
which is pinned to the thread that created it, this handle is safe to pass
to a background thread so it can call ``post(value)``/``wake()`` to deliver
a value back into the app's event stream as a ``{"User": value}`` event
and wake whichever loop currently owns it, without exiting. Its
``post_after(delay_ms, value)``/``wake_after(delay_ms)`` schedule the same
delivery after a delay, from any thread, as a lightweight stand-in for a
timer.

``QuiApp.arm_wake_notify(callback)``/``disarm_wake_notify()`` register (or
clear) a no-argument callable fired every time any ``QuiAppWaker`` handle
this app has ever handed out delivers a ``post``/``wake``/``post_after``/
``wake_after`` -- the low-level hook ``quadraui.asyncio.run(app)`` (an
alias for the ``quadraui.aio`` module -- see its own module doc) arms to
step promptly from a running asyncio loop instead of only on a fixed poll
interval. Most callers reach for ``quadraui.asyncio.run``/
``await app.run_async()`` directly rather than this pair.

``run_native()`` dispatches a ``"WindowClose"`` event to a registered
handler when the window manager's close button (or Alt-F4, or the red
traffic-light button on macOS) asks to close the window, same as any other
event -- and unlike every other event, its default is to let the close
proceed: a handler that returns ``None`` (no explicit ``return``) or
``False`` does not block it, same as having no handler registered at all.
Only an explicit ``return True`` vetoes the close and leaves the window
open, e.g. to show an "unsaved changes?" prompt first. A terminal opened
via ``run_tui()`` has no close button to dispatch this for; quitting there
is entirely up to whatever keystroke a registered handler chooses to raise
on.

``QuiApp.run_shell(width, height, config, ...)`` builds a ``QuiShellApp``
instead: a headless VS Code-style app frame (activity bar + sidebar +
optional bottom-panel tab strip) around Python-registered content and
event handlers, driven by the same ``step``/``run`` discipline as ``QuiApp``
itself. ``config`` is a ``dict`` (``"title"``, ``"panels"``, ``"position"``,
``"bottom_panel"``, ...); sidebar show/hide/resize and bottom-panel tab
switching are handled by the shell chrome itself and reported back via
``set_on_shell_event``/``set_on_bottom_panel_event``.

``QuiApp.theme()``/``set_theme(overrides)`` and ``style()``/``set_style(overrides)``
read and partially override the backend's colour palette and non-colour
geometry tokens; ``overrides`` only needs to name the fields being changed
-- everything else keeps its current value. ``set_nerd_fonts(enabled)``/
``nerd_fonts_enabled()``, ``set_editor_font(family, size_pt)``,
``set_ui_font(font_desc)``, ``register_font_from_memory(bytes)``,
``set_nerd_font_fallback(family)``, ``has_font_family(family)``, and
``set_caret_shape(shape)`` configure fonts and the hardware caret. All of
these take effect immediately and are replayed onto a real backend a later
``run_tui``/``run_native`` call constructs. A handler can react to a
``SystemThemeChanged`` event by recording it and calling ``set_theme``
once ``step``/``run`` returns (not from inside the handler itself -- the
same restriction as mutating any other ``QuiApp`` state mid-callback).
"""

from . import aio as asyncio

from ._quadraui import (
    DecodeFailedError,
    InternalError,
    InvalidAppIdError,
    InvalidCharError,
    InvalidDescriptorError,
    InvalidDimensionsError,
    InvalidFontRoleError,
    MissingFieldError,
    NativeAlreadyRunError,
    NoDisplayError,
    NotOnMainThreadError,
    PanicError,
    PoisonedError,
    QuadrauiError,
    QuiApp,
    QuiAppWaker,
    QuiShellApp,
    SlotOccupiedError,
    TypeMismatchError,
    UnknownKeyError,
    UnknownSlotError,
    UnknownVariantError,
    UnsupportedError,
    VersionMismatchError,
    app_path,
    install_crash_handler,
    set_app_identifier,
)

__all__ = [
    "QuiApp",
    "QuiAppWaker",
    "QuiShellApp",
    "QuadrauiError",
    "InvalidDescriptorError",
    "MissingFieldError",
    "UnknownVariantError",
    "TypeMismatchError",
    "UnknownKeyError",
    "VersionMismatchError",
    "DecodeFailedError",
    "SlotOccupiedError",
    "UnknownSlotError",
    "InvalidDimensionsError",
    "InvalidCharError",
    "PanicError",
    "PoisonedError",
    "InternalError",
    "NativeAlreadyRunError",
    "NoDisplayError",
    "NotOnMainThreadError",
    "InvalidAppIdError",
    "InvalidFontRoleError",
    "UnsupportedError",
    "app_path",
    "set_app_identifier",
    "install_crash_handler",
]
