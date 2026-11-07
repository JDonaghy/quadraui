//! Platform services -- file dialogs, notifications, URL/path opening,
//! trash, beep, system theme, platform identity -- thin napi-rs leaves
//! straight over [`quadraui::PlatformServices`], the napi-rs analog of
//! `quadraui-py`'s own platform-services methods on `QuiApp`.
//!
//! Every method resolves [`quadraui::PlatformServices`] off
//! `self.driver.backend()`, this app's own headless `TuiBackend`, and so
//! calls [`QuiApp::check_not_live`] first: once `startTui`/`startNative`
//! is running, that backend is the dormant pre-live one, not the live
//! session's own.
//! `TuiPlatformServices` (`quadraui::tui::services`) is a genuine,
//! non-stub implementation for every method here -- see that module's doc
//! for exactly what each one does on a terminal.
//!
//! `openPath`/`revealInFileManager`/`moveToTrash`/`restoreFromTrash`/
//! `sendNotification` each has a deferred-queue twin on [`QuiAppActions`]
//! for use from inside a registered handler/tick callback (which cannot
//! call back into `QuiApp` itself -- see [`QuiApp::check_not_dispatching`]).
//! The methods below are the direct, top-level-script-only versions.

use super::*;
use quadraui::FileDialogOptions;
use std::path::Path;

#[napi]
impl QuiApp {
    /// Queues scripted input for the next nested dialog/picker loop a
    /// `showFileOpenDialog`/`showFileSaveDialog`/`showFolderDialog` call
    /// drives -- [`quadraui::tui::testing::TuiDriver::queue_dialog_events`].
    /// `events` decodes the same way `pushEvent`'s single object does, one
    /// [`UiEvent`] per array entry. With nothing queued, the very first
    /// event the nested loop synthesizes is Escape, so an un-driven dialog
    /// call resolves to `null` (cancelled) rather than hanging.
    #[napi(js_name = "queueDialogEvents")]
    pub fn queue_dialog_events(&mut self, env: Env, events: Unknown) -> Result<()> {
        self.check_not_dispatching()?;
        self.check_not_live()?;
        let events: Vec<UiEvent> = decode_via_json("queueDialogEvents", events)?;
        self.guarded(env, |app| {
            app.driver.queue_dialog_events(events);
            Ok(())
        })
    }

    /// Opens a file-open picker --
    /// [`quadraui::PlatformServices::show_file_open_dialog`]. On the
    /// headless `TuiBackend` this drives
    /// [`quadraui::compose::FilePickerController`] through a nested
    /// draw-and-read loop fed by [`Self::queue_dialog_events`] (or Escape
    /// immediately, with nothing queued). Returns the confirmed path, or
    /// `null` on cancel. `filters` is an array of `[label, [ext, ...]]`
    /// pairs, e.g. `[["Rust", ["rs"]]]`.
    #[napi(js_name = "showFileOpenDialog")]
    pub fn show_file_open_dialog(
        &mut self,
        env: Env,
        title: Option<String>,
        initial_dir: Option<String>,
        filters: Option<Vec<(String, Vec<String>)>>,
    ) -> Result<Option<String>> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let opts = FileDialogOptions {
                title,
                initial_dir: initial_dir.map(PathBuf::from),
                initial_filename: None,
                filters: filters.unwrap_or_default(),
            };
            Ok(app
                .driver
                .backend()
                .services()
                .show_file_open_dialog(opts)
                .map(|p| p.display().to_string()))
        })
    }

    /// Opens a file-save picker --
    /// [`quadraui::PlatformServices::show_file_save_dialog`]. Same
    /// nested-loop/`queueDialogEvents` story as
    /// [`Self::show_file_open_dialog`]; `initial_filename` seeds the
    /// picker's query field.
    #[napi(js_name = "showFileSaveDialog")]
    pub fn show_file_save_dialog(
        &mut self,
        env: Env,
        title: Option<String>,
        initial_dir: Option<String>,
        initial_filename: Option<String>,
        filters: Option<Vec<(String, Vec<String>)>>,
    ) -> Result<Option<String>> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let opts = FileDialogOptions {
                title,
                initial_dir: initial_dir.map(PathBuf::from),
                initial_filename,
                filters: filters.unwrap_or_default(),
            };
            Ok(app
                .driver
                .backend()
                .services()
                .show_file_save_dialog(opts)
                .map(|p| p.display().to_string()))
        })
    }

    /// Opens a directory picker --
    /// [`quadraui::PlatformServices::show_folder_open_dialog`]. Same
    /// nested-loop/`queueDialogEvents` story as
    /// [`Self::show_file_open_dialog`], but drives
    /// [`quadraui::compose::FolderPickerController`] instead of
    /// `FilePickerController`.
    #[napi(js_name = "showFolderDialog")]
    pub fn show_folder_dialog(
        &mut self,
        env: Env,
        title: Option<String>,
        initial_dir: Option<String>,
    ) -> Result<Option<String>> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let opts = FileDialogOptions {
                title,
                initial_dir: initial_dir.map(PathBuf::from),
                initial_filename: None,
                filters: Vec::new(),
            };
            Ok(app
                .driver
                .backend()
                .services()
                .show_folder_open_dialog(opts)
                .map(|p| p.display().to_string()))
        })
    }

    /// Dispatches a system notification --
    /// [`quadraui::PlatformServices::send_notification`]. On the headless
    /// `TuiBackend` this is a genuine best-effort degrade (an OSC 9/777
    /// escape sequence plus a native `osascript`/`notify-send` spawn where
    /// available), not a no-op. Fire-and-forget: there is no return value
    /// to assert on, and no error this binding can observe.
    #[napi(js_name = "sendNotification")]
    pub fn send_notification(
        &mut self,
        env: Env,
        title: String,
        body: String,
        urgent: Option<bool>,
        silent: Option<bool>,
        tag: Option<String>,
    ) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let mut n = Notification::new(title, body).with_silent(silent.unwrap_or(false));
            if let Some(tag) = tag {
                n = n.with_tag(tag);
            }
            n.urgent = urgent.unwrap_or(false);
            app.driver.backend().services().send_notification(n);
            Ok(())
        })
    }

    /// Opens `url` in the platform's default browser --
    /// [`quadraui::PlatformServices::open_url_result`] (the fallible twin
    /// of `open_url`, used here so a genuine failure raises instead of
    /// being silently swallowed). On the headless `TuiBackend` this shells
    /// out to `xdg-open`/`open`/`cmd /c start`, falling back to an OSC 8
    /// hyperlink. Raises only when neither leg reached anything.
    #[napi(js_name = "openUrl")]
    pub fn open_url(&mut self, env: Env, url: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .open_url_result(&url)
                .map_err(|err| internal_error(format!("openUrl: {err:?}")))
        })
    }

    /// Reveals `path` in the platform's file manager with it selected --
    /// [`quadraui::PlatformServices::reveal_in_file_manager`]. On the
    /// headless `TuiBackend` this reaches the desktop session underneath
    /// the terminal, falling back to opening the parent directory. Raises
    /// when no such session is reachable.
    ///
    /// Calling this from inside the registered event handler or render
    /// callback raises `ReentrantCallError` -- call
    /// [`QuiAppActions::reveal_in_file_manager`] (via `QuiApp::actions()`)
    /// from a handler instead.
    #[napi(js_name = "revealInFileManager")]
    pub fn reveal_in_file_manager(&mut self, env: Env, path: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .reveal_in_file_manager(Path::new(&path))
                .map_err(|err| internal_error(format!("revealInFileManager: {err:?}")))
        })
    }

    /// Opens `path` with the platform's default handler for its type --
    /// [`quadraui::PlatformServices::open_path`]. Distinct from
    /// [`Self::open_url`]: this takes a filesystem path, not a URL. Same
    /// handler restriction as [`Self::reveal_in_file_manager`] -- call
    /// [`QuiAppActions::open_path`] from a handler instead.
    #[napi(js_name = "openPath")]
    pub fn open_path(&mut self, env: Env, path: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .open_path(Path::new(&path))
                .map_err(|err| internal_error(format!("openPath: {err:?}")))
        })
    }

    /// Moves `path` to the platform trash/recycle bin --
    /// [`quadraui::PlatformServices::move_to_trash`]. Genuinely full
    /// support on the headless `TuiBackend`, not a degrade. Same handler
    /// restriction as [`Self::reveal_in_file_manager`] -- call
    /// [`QuiAppActions::move_to_trash`] from a handler instead.
    #[napi(js_name = "moveToTrash")]
    pub fn move_to_trash(&mut self, env: Env, path: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .move_to_trash(Path::new(&path))
                .map_err(|err| internal_error(format!("moveToTrash: {err:?}")))
        })
    }

    /// Restores `path` from the platform trash/recycle bin back to its
    /// original location -- [`quadraui::PlatformServices::restore_from_trash`],
    /// the inverse of [`Self::move_to_trash`]. Raises both on a real
    /// filesystem failure and when no trashed item matches `path`, or when
    /// built for macOS, where no programmatic trash listing/restore API
    /// exists. Same handler restriction as [`Self::reveal_in_file_manager`]
    /// -- call [`QuiAppActions::restore_from_trash`] from a handler
    /// instead.
    #[napi(js_name = "restoreFromTrash")]
    pub fn restore_from_trash(&mut self, env: Env, path: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .restore_from_trash(Path::new(&path))
                .map_err(|err| internal_error(format!("restoreFromTrash: {err:?}")))
        })
    }

    /// Rings the terminal bell -- [`quadraui::PlatformServices::beep`].
    /// Writes BEL (`\x07`) to both stdout and (Unix) `/dev/tty`; harmless
    /// but not itself observable from a driver test -- a terminal bell is
    /// this method's own "manual verification" story.
    #[napi]
    pub fn beep(&mut self, env: Env) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .beep()
                .map_err(|err| internal_error(format!("beep: {err:?}")))
        })
    }

    /// The OS-level light/dark preference, accent colour, and
    /// high-contrast setting, as a `{dark, accent, high_contrast,
    /// reduce_motion}` object -- [`quadraui::PlatformServices::system_theme`].
    /// On the headless `TuiBackend` this is an environment-only heuristic
    /// (`$COLORFGBG`) -- `accent` is always `null` and
    /// `high_contrast`/`reduce_motion` are always `false` there. Raises
    /// when `$COLORFGBG` is unset or unparseable.
    #[napi(js_name = "systemTheme")]
    pub fn system_theme(&mut self, env: Env) -> Result<Unknown<'static>> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let theme = app
                .driver
                .backend()
                .services()
                .system_theme()
                .map_err(|err| internal_error(format!("systemTheme: {err:?}")))?;
            encode_via_json(env, "systemTheme", &theme)
        })
    }

    /// This app's platform identifier --
    /// [`quadraui::PlatformServices::platform_name`]. Always `"tui"` here:
    /// every method on this page resolves services off this `QuiApp`'s own
    /// headless `TuiBackend`.
    #[napi(js_name = "platformName")]
    pub fn platform_name(&mut self, env: Env) -> Result<&'static str> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            Ok(app.driver.backend().services().platform_name())
        })
    }
}
