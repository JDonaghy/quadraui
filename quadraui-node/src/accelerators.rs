//! Accelerators & global shortcuts -- thin napi-rs leaves straight over
//! [`Backend::register_accelerator`]/[`PlatformServices::register_global_shortcut`],
//! the napi-rs analog of `quadraui-py`'s own accelerator methods on
//! `QuiApp`.
//!
//! Every method resolves against `self.driver`, this app's one headless
//! `TuiBackend` -- there is no `run_native`/`run_tui` analog on this
//! crate's `QuiApp` for a global shortcut to reach instead. Every in-tree
//! `PlatformServices` override inherits
//! [`quadraui::PlatformServices::register_global_shortcut`]'s
//! `Unsupported` default today, so `registerGlobalShortcut`'s success path
//! has no automated coverage here and needs manual verification once a
//! backend implements a real OS hotkey call.

use super::*;
use quadraui::{Accelerator, AcceleratorId, GlobalShortcutId};

#[napi]
impl QuiApp {
    /// Registers `descriptor` (decoded as an [`Accelerator`]) so a matching
    /// native key event fires `UiEvent::Accelerator(id, modifiers)` instead
    /// of a plain `KeyPressed` -- [`Backend::register_accelerator`].
    /// Re-registering the same `descriptor.id` replaces its previous
    /// binding/scope/label wholesale. `descriptor` requires all four keys
    /// -- `id`, `binding`, `scope`, and `label` (`null` for no display
    /// label; omitting the key raises `InvalidDescriptorError` rather than
    /// defaulting). Only `{"scope": "Global"}` actually fires today; a
    /// non-`Global` scope is accepted and then silently never fires.
    /// Takes effect immediately on this app's own live backend, and is
    /// also replayed by [`BindingHost::setup`] onto any later backend.
    #[napi(js_name = "registerAccelerator")]
    pub fn register_accelerator(&mut self, env: Env, descriptor: Unknown) -> Result<()> {
        self.check_not_dispatching()?;
        let acc: Accelerator = decode_via_json("registerAccelerator", descriptor)?;
        self.guarded(env, |app| {
            let (backend, app_logic) = app.driver.parts_mut();
            backend.register_accelerator(&acc);
            app_logic.host.accelerators.insert(acc.id.clone(), acc);
            Ok(())
        })
    }

    /// Removes a previously registered accelerator --
    /// [`Backend::unregister_accelerator`]. A no-op if `id` was never
    /// registered, or was already unregistered.
    #[napi(js_name = "unregisterAccelerator")]
    pub fn unregister_accelerator(&mut self, env: Env, id: String) -> Result<()> {
        self.guarded(env, |app| {
            let id = AcceleratorId::new(id);
            let (backend, app_logic) = app.driver.parts_mut();
            backend.unregister_accelerator(&id);
            app_logic.host.accelerators.remove(&id);
            Ok(())
        })
    }

    /// Registers `descriptor` (decoded as an [`Accelerator`]) as an
    /// OS-wide global shortcut --
    /// [`quadraui::PlatformServices::register_global_shortcut`] -- fired as
    /// `UiEvent::GlobalShortcut(id)` even while this app isn't focused.
    /// `descriptor.scope` is ignored: a global shortcut is OS-wide by
    /// construction. Returns an opaque handle to pass back to
    /// [`Self::unregister_global_shortcut`].
    #[napi(js_name = "registerGlobalShortcut")]
    pub fn register_global_shortcut(
        &mut self,
        env: Env,
        descriptor: Unknown,
    ) -> Result<Unknown<'static>> {
        self.check_not_dispatching()?;
        let acc: Accelerator = decode_via_json("registerGlobalShortcut", descriptor)?;
        self.guarded(env, |app| {
            let id = app
                .driver
                .backend()
                .services()
                .register_global_shortcut(acc)
                .map_err(|err| internal_error(format!("registerGlobalShortcut: {err:?}")))?;
            encode_via_json(env, "registerGlobalShortcut", &id)
        })
    }

    /// Releases a shortcut returned by [`Self::register_global_shortcut`]
    /// -- [`quadraui::PlatformServices::unregister_global_shortcut`].
    #[napi(js_name = "unregisterGlobalShortcut")]
    pub fn unregister_global_shortcut(&mut self, env: Env, id: Unknown) -> Result<()> {
        self.check_not_dispatching()?;
        let id: GlobalShortcutId = decode_via_json("unregisterGlobalShortcut", id)?;
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .unregister_global_shortcut(id)
                .map_err(|err| internal_error(format!("unregisterGlobalShortcut: {err:?}")))
        })
    }

    /// Releases every currently registered global shortcut --
    /// [`quadraui::PlatformServices::unregister_all_global_shortcuts`].
    #[napi(js_name = "unregisterAllGlobalShortcuts")]
    pub fn unregister_all_global_shortcuts(&mut self, env: Env) -> Result<()> {
        self.guarded(env, |app| {
            app.driver
                .backend()
                .services()
                .unregister_all_global_shortcuts()
                .map_err(|err| internal_error(format!("unregisterAllGlobalShortcuts: {err:?}")))
        })
    }
}
