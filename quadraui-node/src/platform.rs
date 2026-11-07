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

    // ─── Platform services: update, power-save, login item, protocol,
    // crash reporter, drag-out, recent docs/dock menu/jump list ──────────
    //
    // Every method below is argument conversion + one
    // `quadraui_binding_core::services` call + [`service_error_to_napi`] --
    // see that module's own doc for why the actual service-building logic
    // (parsing `kind` strings, collapsing `BackendError`, minting an opaque
    // handle) lives there rather than here. Every one of these raises
    // `UnsupportedError` on the headless `TuiBackend`, which implements
    // none of them -- see `quadraui::tui::services`'s module doc for which
    // `PlatformServices` methods it does implement. `applyOnQuit`,
    // `endPowerSaveBlock`, `setLoginItem`, `setAsDefaultProtocolClient`,
    // `removeAsDefaultProtocolClient`, `beginDragOut`,
    // `addRecentDocument`, `clearRecentDocuments`, `setDockMenu`, and
    // `setJumpList` have no return value a caller needs synchronously, so
    // each has a [`QuiAppActions`] mirror (via [`QuiApp::actions`]) for use
    // from inside a registered handler, the same handler restriction
    // [`QuiAppActions`]'s own doc describes. `checkForUpdate`,
    // `downloadAndStage`, `beginPowerSaveBlock`, `loginItemEnabled`, and
    // `isDefaultProtocolClient` each return a value a caller needs right
    // away, so they have no `QuiAppActions` mirror.

    /// Queries `feedUrl` for a newer version, as a `{version, notes, url,
    /// signature}` object (or `null` when nothing newer is published) --
    /// [`quadraui_binding_core::update_check_for_update`].
    #[napi(js_name = "checkForUpdate")]
    pub fn check_for_update(&mut self, env: Env, feed_url: String) -> Result<Unknown<'static>> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            let info = update_check_for_update(app.driver.backend().services(), &feed_url)
                .map_err(|err| service_error_to_napi("checkForUpdate", err))?;
            encode_via_json(env, "checkForUpdate", &info)
        })
    }

    /// Downloads and stages `info` (the object [`Self::check_for_update`]
    /// returned), returning an opaque handle for a later
    /// [`Self::apply_on_quit`] call --
    /// [`quadraui_binding_core::update_download_and_stage`].
    #[napi(js_name = "downloadAndStage")]
    pub fn download_and_stage(&mut self, env: Env, info: Unknown) -> Result<i64> {
        let info: UpdateInfoWire = decode_via_json("downloadAndStage", info)?;
        self.check_not_live()?;
        self.guarded(env, |app| {
            update_download_and_stage(app.driver.backend().services(), info)
                .map(|handle| handle as i64)
                .map_err(|err| service_error_to_napi("downloadAndStage", err))
        })
    }

    /// Installs the update staged under `handle` --
    /// [`quadraui_binding_core::update_apply_on_quit`]. `handle` is
    /// consumed: a second call with the same value raises
    /// `InvalidDescriptorError`.
    ///
    /// Call [`QuiAppActions::apply_on_quit`] (via [`QuiApp::actions`]) from
    /// a handler instead.
    #[napi(js_name = "applyOnQuit")]
    pub fn apply_on_quit(&mut self, env: Env, handle: i64) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            update_apply_on_quit(app.driver.backend().services(), handle as u64)
                .map_err(|err| service_error_to_napi("applyOnQuit", err))
        })
    }

    /// Starts a sleep/suspend inhibitor of the kind named by `kind`
    /// (`"prevent_display_sleep"` or `"prevent_app_suspension"`), returning
    /// an opaque handle for a later [`Self::end_power_save_block`] call --
    /// [`quadraui_binding_core::begin_power_save_block`].
    #[napi(js_name = "beginPowerSaveBlock")]
    pub fn begin_power_save_block(&mut self, env: Env, kind: String) -> Result<i64> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            begin_power_save_block(app.driver.backend().services(), &kind)
                .map(|handle| handle as i64)
                .map_err(|err| service_error_to_napi("beginPowerSaveBlock", err))
        })
    }

    /// Ends the sleep/suspend inhibitor started under `handle` --
    /// [`quadraui_binding_core::end_power_save_block`]. `handle` is
    /// consumed, the same one-shot story [`Self::apply_on_quit`]'s doc
    /// describes.
    ///
    /// Call [`QuiAppActions::end_power_save_block`] (via
    /// [`QuiApp::actions`]) from a handler instead.
    #[napi(js_name = "endPowerSaveBlock")]
    pub fn end_power_save_block(&mut self, env: Env, handle: i64) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            end_power_save_block(app.driver.backend().services(), handle as u64)
                .map_err(|err| service_error_to_napi("endPowerSaveBlock", err))
        })
    }

    /// Registers (`enabled: true`) or unregisters (`enabled: false`) this
    /// app to launch at login --
    /// [`quadraui_binding_core::set_login_item`]. `openHidden`/`args` are
    /// ignored when `enabled` is `false`.
    ///
    /// Call [`QuiAppActions::set_login_item`] (via [`QuiApp::actions`])
    /// from a handler instead.
    #[napi(js_name = "setLoginItem")]
    pub fn set_login_item(
        &mut self,
        env: Env,
        enabled: bool,
        open_hidden: Option<bool>,
        args: Option<Vec<String>>,
    ) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            set_login_item(
                app.driver.backend().services(),
                enabled,
                open_hidden.unwrap_or(false),
                args.unwrap_or_default(),
            )
            .map_err(|err| service_error_to_napi("setLoginItem", err))
        })
    }

    /// Whether this app is currently registered to launch at login --
    /// [`quadraui_binding_core::login_item_enabled`].
    #[napi(js_name = "loginItemEnabled")]
    pub fn login_item_enabled(&mut self, env: Env) -> Result<bool> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            login_item_enabled(app.driver.backend().services())
                .map_err(|err| service_error_to_napi("loginItemEnabled", err))
        })
    }

    /// Registers this app as the OS-level default handler for `scheme` --
    /// [`quadraui_binding_core::set_as_default_protocol_client`].
    ///
    /// Call [`QuiAppActions::set_as_default_protocol_client`] (via
    /// [`QuiApp::actions`]) from a handler instead.
    #[napi(js_name = "setAsDefaultProtocolClient")]
    pub fn set_as_default_protocol_client(&mut self, env: Env, scheme: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            set_as_default_protocol_client(app.driver.backend().services(), &scheme)
                .map_err(|err| service_error_to_napi("setAsDefaultProtocolClient", err))
        })
    }

    /// Whether this app is currently the OS-level default handler for
    /// `scheme` -- [`quadraui_binding_core::is_default_protocol_client`].
    #[napi(js_name = "isDefaultProtocolClient")]
    pub fn is_default_protocol_client(&mut self, env: Env, scheme: String) -> Result<bool> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            is_default_protocol_client(app.driver.backend().services(), &scheme)
                .map_err(|err| service_error_to_napi("isDefaultProtocolClient", err))
        })
    }

    /// Unregisters this app as the OS-level default handler for `scheme` --
    /// [`quadraui_binding_core::remove_as_default_protocol_client`].
    ///
    /// Call [`QuiAppActions::remove_as_default_protocol_client`] (via
    /// [`QuiApp::actions`]) from a handler instead.
    #[napi(js_name = "removeAsDefaultProtocolClient")]
    pub fn remove_as_default_protocol_client(&mut self, env: Env, scheme: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            remove_as_default_protocol_client(app.driver.backend().services(), &scheme)
                .map_err(|err| service_error_to_napi("removeAsDefaultProtocolClient", err))
        })
    }

    /// Starts an OS-level drag of `files`/`text` out of this app's window
    /// -- [`quadraui_binding_core::begin_drag_out`]. At least one of
    /// `files`/`text` must be given.
    ///
    /// Call [`QuiAppActions::begin_drag_out`] (via [`QuiApp::actions`])
    /// from a handler instead.
    #[napi(js_name = "beginDragOut")]
    pub fn begin_drag_out(
        &mut self,
        env: Env,
        files: Option<Vec<String>>,
        text: Option<String>,
    ) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            begin_drag_out(
                app.driver.backend().services(),
                files.unwrap_or_default(),
                text,
            )
            .map_err(|err| service_error_to_napi("beginDragOut", err))
        })
    }

    /// Adds `path` to the OS "recent documents" list --
    /// [`quadraui_binding_core::add_recent_document`].
    ///
    /// Call [`QuiAppActions::add_recent_document`] (via
    /// [`QuiApp::actions`]) from a handler instead.
    #[napi(js_name = "addRecentDocument")]
    pub fn add_recent_document(&mut self, env: Env, path: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            add_recent_document(app.driver.backend().services(), &path)
                .map_err(|err| service_error_to_napi("addRecentDocument", err))
        })
    }

    /// Clears the OS "recent documents" list --
    /// [`quadraui_binding_core::clear_recent_documents`].
    ///
    /// Call [`QuiAppActions::clear_recent_documents`] (via
    /// [`QuiApp::actions`]) from a handler instead.
    #[napi(js_name = "clearRecentDocuments")]
    pub fn clear_recent_documents(&mut self, env: Env) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            clear_recent_documents(app.driver.backend().services())
                .map_err(|err| service_error_to_napi("clearRecentDocuments", err))
        })
    }

    /// Attaches (or replaces) this app's Dock icon menu from `items` (an
    /// array of context-menu-item objects, the same shape
    /// [`Self::set_tray_menu`] takes) --
    /// [`quadraui_binding_core::set_dock_menu`].
    ///
    /// Call [`QuiAppActions::set_dock_menu`] (via [`QuiApp::actions`])
    /// from a handler instead.
    #[napi(js_name = "setDockMenu")]
    pub fn set_dock_menu(&mut self, env: Env, items: Unknown) -> Result<()> {
        let items = to_descriptor_value(items)
            .map_err(|detail| invalid_descriptor_error(format!("setDockMenu: {detail}")))?;
        self.check_not_live()?;
        self.guarded(env, |app| {
            set_dock_menu(app.driver.backend().services(), items)
                .map_err(|err| service_error_to_napi("setDockMenu", err))
        })
    }

    /// Sets this app's Windows taskbar jump list from `items` (an array of
    /// `{title, program, args, description}` objects) --
    /// [`quadraui_binding_core::set_jump_list`].
    ///
    /// Call [`QuiAppActions::set_jump_list`] (via [`QuiApp::actions`])
    /// from a handler instead.
    #[napi(js_name = "setJumpList")]
    pub fn set_jump_list(&mut self, env: Env, items: Unknown) -> Result<()> {
        let items = to_descriptor_value(items)
            .map_err(|detail| invalid_descriptor_error(format!("setJumpList: {detail}")))?;
        self.check_not_live()?;
        self.guarded(env, |app| {
            set_jump_list(app.driver.backend().services(), items)
                .map_err(|err| service_error_to_napi("setJumpList", err))
        })
    }

    // ─── Window document state, tray & pointer cursor ───────────────────
    //
    // Every method below resolves `WindowControl`/`TrayService` off this
    // `QuiApp`'s own headless `TuiBackend` -- never a `startNative` window
    // -- via [`with_window`]/[`with_tray`]'s `Backend::window`/
    // `Backend::tray` fold. `TuiBackend` reports
    // `Err(BackendError::Unsupported)` -- surfaced here as `InternalError`
    // -- for every method below, and `set_cursor` always returns `false`
    // (no native pointer concept) -- see `quadraui::backend`'s
    // `WindowControl`/`TrayService`/`Backend::set_cursor` docs for why
    // those gaps are structural, not "not implemented yet". A
    // `startNative` window built with the `gtk`/`macos`/`win` feature
    // backs every one of these for real; call the matching method on
    // [`QuiAppActions`] (via [`QuiApp::actions`]) from inside a registered
    // handler/tick/render callback instead of these, for the same
    // re-entrant-borrow reason [`QuiAppActions`]'s own doc describes.

    /// Marks the window's document as having unsaved changes (`true`) or
    /// clean (`false`) -- [`quadraui::WindowControl::set_document_edited`].
    /// Raises `InternalError` on every in-tree backend except a macOS
    /// `startNative` window, which has no reachable equivalent from this
    /// headless app.
    #[napi(js_name = "setDocumentEdited")]
    pub fn set_document_edited(&mut self, env: Env, edited: bool) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            with_window(app.driver.parts_mut().0, |w| w.set_document_edited(edited))
                .map_err(|err| internal_error(format!("setDocumentEdited: {err:?}")))
        })
    }

    /// Associates the window with the file at `path` (or clears the
    /// association when `null`) --
    /// [`quadraui::WindowControl::set_represented_filename`]. Same
    /// macOS-only story as [`Self::set_document_edited`].
    #[napi(js_name = "setRepresentedFilename")]
    pub fn set_represented_filename(&mut self, env: Env, path: Option<String>) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            with_window(app.driver.parts_mut().0, |w| {
                w.set_represented_filename(path.as_deref().map(Path::new))
            })
            .map_err(|err| internal_error(format!("setRepresentedFilename: {err:?}")))
        })
    }

    /// Sets (or replaces) the tray icon's image --
    /// [`quadraui::backend::TrayService::set_icon`]. `icon` is an
    /// [`ImageSource`] object, e.g. `{Path: "/path/to.png"}` or
    /// `{Bytes: [...]}` -- the same externally-tagged shape
    /// `quadraui::ImageSource`'s derived `Deserialize` accepts. Raises
    /// `InternalError` on every in-tree backend except a `gtk`/`macos`/
    /// `win` `startNative` window: TUI has no tray/status-bar concept at
    /// all ([`quadraui::Backend::tray`] stays `None` there permanently).
    #[napi(js_name = "setTrayIcon")]
    pub fn set_tray_icon(&mut self, env: Env, icon: Unknown) -> Result<()> {
        let icon: ImageSource = decode_direct(env, "setTrayIcon", icon)?;
        self.check_not_live()?;
        self.guarded(env, |app| {
            with_tray(app.driver.parts_mut().0, |t| t.set_icon(icon))
                .map_err(|err| internal_error(format!("setTrayIcon: {err:?}")))
        })
    }

    /// Sets the tray icon's hover tooltip text --
    /// [`quadraui::backend::TrayService::set_tooltip`]. Same
    /// tray-less-on-TUI story as [`Self::set_tray_icon`].
    #[napi(js_name = "setTrayTooltip")]
    pub fn set_tray_tooltip(&mut self, env: Env, tooltip: String) -> Result<()> {
        self.check_not_live()?;
        self.guarded(env, |app| {
            with_tray(app.driver.parts_mut().0, |t| t.set_tooltip(&tooltip))
                .map_err(|err| internal_error(format!("setTrayTooltip: {err:?}")))
        })
    }

    /// Attaches (or replaces) the tray icon's right-click/click menu --
    /// [`quadraui::backend::TrayService::set_menu`]. `items` decodes the
    /// same way [`QuiApp::show_context_menu`]'s own `items` argument does
    /// -- an array of [`ContextMenuItem`] objects; an activated entry
    /// fires `UiEvent::ContextMenuItemActivated` the same way a
    /// right-click context menu's does. Same tray-less-on-TUI story as
    /// [`Self::set_tray_icon`].
    #[napi(js_name = "setTrayMenu")]
    pub fn set_tray_menu(&mut self, env: Env, items: Unknown) -> Result<()> {
        let items: Vec<ContextMenuItem> = decode_direct(env, "setTrayMenu", items)?;
        self.check_not_live()?;
        self.guarded(env, |app| {
            let menu = menu_for_surface(WidgetId::new("qui-tray-menu"), items);
            with_tray(app.driver.parts_mut().0, |t| t.set_menu(&menu))
                .map_err(|err| internal_error(format!("setTrayMenu: {err:?}")))
        })
    }

    /// Hints the OS mouse-pointer glyph -- [`Backend::set_cursor`].
    /// `shape` is one of `"default"`, `"resize"` (requires `edge`),
    /// `"text"`, `"pointer"`, `"grab"`, `"grabbing"`, `"crosshair"`,
    /// `"wait"`, `"not_allowed"`, `"col_resize"`, `"row_resize"`; `edge`
    /// (only meaningful for `"resize"`) is one of `"north"`, `"south"`,
    /// `"east"`, `"west"`, `"north_east"`, `"north_west"`, `"south_east"`,
    /// `"south_west"`. Returns whether the pointer glyph was actually
    /// applied -- `false` on the headless `TuiBackend`, which has no
    /// native pointer concept at all, same as [`Backend::set_cursor`]'s
    /// own "no window yet" case on a windowed backend.
    #[napi(js_name = "setCursor")]
    pub fn set_cursor(&mut self, env: Env, shape: String, edge: Option<String>) -> Result<bool> {
        let shape: PointerShape = pointer_shape(&shape, edge.as_deref())
            .map_err(|err| invalid_descriptor_error(format!("setCursor: {err}")))?;
        self.check_not_live()?;
        self.guarded(env, |app| Ok(app.driver.parts_mut().0.set_cursor(shape)))
    }
}

/// The standard, per-app filesystem location named by `kind` (one of
/// `"user_data"`, `"app_config"`, `"app_cache"`, `"logs"`, `"temp"`,
/// `"downloads"`, `"documents"`, `"home"`), as a path string --
/// [`quadraui_binding_core::resolve_app_path`]. A module-level function,
/// like that one: no live [`QuiApp`] is needed. Every app-scoped kind
/// (everything but `"temp"`/`"downloads"`/`"documents"`/`"home"`) is
/// resolved relative to whatever identifier [`set_app_identifier`] last
/// set, and raises `UnsupportedError` until one has been.
///
/// `#[cfg_attr(test, allow(dead_code))]`: napi-rs's own module-export
/// registration for a free `#[napi]` function (unlike a `#[napi] impl`
/// method, which hangs off a type `cargo test`'s own `#[cfg(test)] mod
/// tests` already constructs) only exists for a real cdylib load, not the
/// plain `rlib` `cargo test` compiles -- so this function is genuinely
/// unreferenced from Rust in that configuration, with its only caller
/// being JS, which this crate's own `node --test` suite
/// (`quadraui-node/tests/platform_parity.test.ts`) exercises against the
/// real compiled addon instead.
#[cfg_attr(test, allow(dead_code))]
#[napi(js_name = "appPath")]
pub fn app_path(kind: String) -> Result<String> {
    resolve_app_path(&kind)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|err| service_error_to_napi("appPath", err))
}

/// Sets the app identifier scoping [`app_path`]'s app-scoped kinds --
/// [`quadraui_binding_core::set_app_identifier`]. A module-level function:
/// the identifier is process-global, not per-[`QuiApp`]. Same
/// `#[cfg_attr(test, allow(dead_code))]` story as [`app_path`]'s own doc.
#[cfg_attr(test, allow(dead_code))]
#[napi(js_name = "setAppIdentifier")]
pub fn set_app_identifier_js(identifier: String) {
    set_app_identifier(&identifier);
}

/// Installs a crash handler that writes a `quadraui::crash::CrashRecord`
/// on an uncaught panic, decoding `cfg` (a `{appVersion, uploadEndpoint,
/// metadata, logDir}` object -- in the underlying Rust shape's own
/// `snake_case` field names, since this decodes straight to
/// [`serde_json::Value`] with no `camelCase` remap -- every key optional)
/// into a `quadraui::crash::CrashReporterConfig` --
/// [`quadraui_binding_core::install_crash_handler`]. A module-level
/// function, like that one: no live [`QuiApp`] is needed, since this
/// installs a process-global Rust panic hook. Same
/// `#[cfg_attr(test, allow(dead_code))]` story as [`app_path`]'s own doc.
#[cfg_attr(test, allow(dead_code))]
#[napi(js_name = "installCrashHandler")]
pub fn install_crash_handler_js(cfg: Unknown) -> Result<()> {
    let cfg = to_descriptor_value(cfg)
        .map_err(|detail| invalid_descriptor_error(format!("installCrashHandler: {detail}")))?;
    install_crash_handler(cfg).map_err(|err| service_error_to_napi("installCrashHandler", err))
}
