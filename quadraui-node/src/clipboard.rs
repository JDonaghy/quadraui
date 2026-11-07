//! System clipboard access -- the napi-rs analog of `quadraui-py`'s own
//! clipboard methods on `QuiApp`.
//!
//! Every method below reads/writes through `self.driver.backend()`'s own
//! clipboard, deliberately *not* gated by [`QuiApp::check_not_live`]: the
//! clipboard is one `arboard::Clipboard` handle tied to this app's own
//! headless `TuiBackend` for its whole lifetime, so it keeps working the
//! same way whether or not `startTui`/`startNative` has moved this app's
//! [`NodeAppLogic`] into a live session -- mirroring `quadraui-py`'s own
//! `clipboard_write_text`, which keeps a separate, lazily constructed
//! `TuiBackend` alive for the clipboard alone so a long-lived
//! `arboard::Clipboard` handle survives a `run_native`/`run_tui` call
//! moving the main `PyAppLogic` into a different backend. This is more
//! surprising for a `startNative` session than a `startTui` one: a real
//! `GtkRunner`/`MacRunner`/`WinRunner` window has its own native
//! clipboard integration (GTK's own selection/clipboard objects, AppKit's
//! `NSPasteboard`, Win32's clipboard APIs) that these methods never touch
//! -- they always resolve against the dormant headless `TuiBackend`'s
//! own clipboard instead, regardless of what the live native window's
//! clipboard holds.

use super::*;

#[napi]
impl QuiApp {
    /// Writes `text` to the system clipboard, via the same
    /// `quadraui::tui::services::TuiClipboard` fallback chain (arboard, then
    /// OSC 52, then a native CLI tool) a real terminal backend uses
    /// internally.
    #[napi(js_name = "clipboardWriteText")]
    pub fn clipboard_write_text(&mut self, env: Env, text: String) -> Result<()> {
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .clipboard()
                .write_text(&text);
            Ok(())
        })
    }

    /// Reads the system clipboard's current plain-text contents, or `None`
    /// on an empty / non-text clipboard, or a platform error reading it.
    #[napi(js_name = "clipboardReadText")]
    pub fn clipboard_read_text(&mut self, env: Env) -> Result<Option<String>> {
        self.guarded(env, |app| {
            Ok(app.driver.backend().services().clipboard().read_text())
        })
    }

    /// Reads the X11/Wayland PRIMARY selection -- the platform convention
    /// behind middle-click paste, distinct from `clipboardReadText`'s
    /// CLIPBOARD selection. `None` on every backend but GTK on Linux/BSD.
    #[napi(js_name = "clipboardReadPrimarySelection")]
    pub fn clipboard_read_primary_selection(&mut self, env: Env) -> Result<Option<String>> {
        self.guarded(env, |app| {
            Ok(app
                .driver
                .backend()
                .services()
                .clipboard()
                .read_primary_selection())
        })
    }

    /// Reads the current clipboard contents as `[width, height, pixels]`,
    /// where `pixels` is a straight (non-premultiplied) RGBA8 buffer,
    /// row-major top-to-bottom left-to-right, exactly `width * height * 4`
    /// entries. `None` collapses an empty/non-image clipboard and a backend
    /// with no image clipboard channel at all (always true on TUI) into one
    /// result -- mirrors `quadraui-py`'s own `clipboard_read_image` doc for
    /// why.
    #[napi(js_name = "clipboardReadImage")]
    pub fn clipboard_read_image(&mut self, env: Env) -> Result<Option<(u32, u32, Vec<u8>)>> {
        self.guarded(env, |app| {
            Ok(app
                .driver
                .backend()
                .services()
                .clipboard()
                .read_image()
                .ok()
                .map(|img| (img.width, img.height, img.pixels)))
        })
    }

    /// Writes `pixels` (straight RGBA8, row-major top-to-bottom
    /// left-to-right) to the system clipboard as an image. Raises if
    /// `pixels` isn't exactly `width * height * 4` entries long, or if the
    /// backend itself rejects the write -- including `Unsupported` on TUI,
    /// which has no image clipboard channel.
    #[napi(js_name = "clipboardWriteImage")]
    pub fn clipboard_write_image(
        &mut self,
        env: Env,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<()> {
        self.check_not_dispatching()?;
        // `checked_mul` rather than a plain `*`: an adversarial
        // `width`/`height` overflows `usize` on a 32-bit target and wraps
        // silently in a release build, which would let a too-short `pixels`
        // slip past the length check below instead of being rejected here.
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| {
                invalid_dimensions_error(format!(
                    "clipboardWriteImage: width * height * 4 overflows (width={width}, height={height})"
                ))
            })?;
        if pixels.len() != expected {
            return Err(invalid_dimensions_error(format!(
                "clipboardWriteImage: pixels must be exactly width * height * 4 entries \
                 ({expected}), got {}",
                pixels.len()
            )));
        }
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .clipboard()
                .write_image(&RgbaImage {
                    width,
                    height,
                    pixels,
                })
                .map_err(|err| internal_error(format!("clipboardWriteImage: {err:?}")))
        })
    }

    /// Writes `html` to the system clipboard, with `alt_text` as the
    /// plain-text fallback a paste target that only understands text falls
    /// back to. Raises on failure, including `Unsupported` on TUI, which
    /// has no HTML clipboard channel.
    #[napi(js_name = "clipboardWriteHtml")]
    pub fn clipboard_write_html(&mut self, env: Env, html: String, alt_text: String) -> Result<()> {
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .clipboard()
                .write_html(&html, &alt_text)
                .map_err(|err| internal_error(format!("clipboardWriteHtml: {err:?}")))
        })
    }

    /// Reads the list of file paths currently on the clipboard (a
    /// Finder/Explorer copy), as a list of their string representations.
    /// `None` collapses an empty list and a backend with no file-list
    /// clipboard channel at all (TUI) into one result -- mirrors
    /// `clipboardReadImage`'s own fold for the same reason.
    #[napi(js_name = "clipboardReadFileList")]
    pub fn clipboard_read_file_list(&mut self, env: Env) -> Result<Option<Vec<String>>> {
        self.guarded(env, |app| {
            Ok(app
                .driver
                .backend()
                .services()
                .clipboard()
                .read_file_list()
                .ok()
                .map(|paths| {
                    paths
                        .into_iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect()
                }))
        })
    }
}
