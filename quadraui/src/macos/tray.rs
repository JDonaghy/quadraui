//! Tray / status-bar icon control for macOS (issue #953).
//!
//! `NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength)`
//! backs `Backend::tray`'s [`crate::backend::TrayService`] surface (see
//! `impl TrayService for MacBackend` in `super::backend`, which delegates
//! every method to the free functions here). The status item is created
//! lazily, on the first [`set_icon`] call — see [`MacTrayState::ensure_item`]'s
//! doc for why there is no "not constructed yet" state to report `None`
//! for the way `Backend::window`/`NSWindow` genuinely has one.
//!
//! ## Menu vs. plain click (issue #953's "reuse `ContextMenu`" design note)
//!
//! [`set_menu`] reuses [`super::menu_bar_install::build_ns_menu`] — the
//! same `ContextMenu` → `NSMenu` conversion `Backend::show_context_menu`
//! uses — and attaches it via `NSStatusItem::setMenu`. AppKit's own
//! behaviour once a menu is attached that way: **any** click on the
//! status item (left or right) opens the menu directly, and the
//! button's own target/action (the `UiEvent::TrayClicked` producer
//! below) does not fire for that click at all. So once [`set_menu`] has
//! been called with a non-empty [`ContextMenu`], `TrayClicked` stops
//! firing and `UiEvent::ContextMenuItemActivated` (selection only — see
//! below) takes over, mirroring a right-click context menu's own event
//! vocabulary. This is a native AppKit trade-off (the same shape ships
//! in Electron/Qt tray implementations), not a quadraui gap.
//!
//! `UiEvent::ContextMenuDismissed` deliberately does **not** fire for a
//! tray menu, unlike [`super::menu_bar_install::show_context_menu`]:
//! that function calls `popUpMenuPositioningItem:atLocation:inView:`
//! itself and can push `ContextMenuDismissed` the instant the call
//! returns, but `NSStatusItem::setMenu` hands the whole click-track-close
//! cycle to AppKit with no synchronous call of ours to hang a "closed
//! now" push off. Wiring `NSMenuDelegate::menuDidClose:` to recover that
//! signal is a real, nameable follow-up (it would need `QuadraMenuTarget`
//! to grow a second protocol conformance shared with the menu-bar/
//! context-menu paths, which don't need it today) rather than something
//! faked here.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSEventMask, NSEventType, NSImage, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSData, NSString};

use crate::backend::{BackendError, ServiceResult};
use crate::event::{MouseButton, UiEvent};
use crate::primitives::context_menu::ContextMenu;
use crate::primitives::image::ImageSource;

use super::menu_bar_install::{build_ns_menu, MenuKind, QuadraMenuTarget};

/// Per-[`super::MacBackend`] tray state — the `NSStatusItem` plus the
/// two Obj-C targets that keep its action selectors alive. Both targets
/// must be retained for as long as the item shows a live click/menu
/// surface, mirroring `MacBackend::menu_target`'s "retain or the
/// selector's target dangles" rule.
#[derive(Default)]
pub(crate) struct MacTrayState {
    status_item: Option<Retained<NSStatusItem>>,
    /// Target for the plain-click action selector — see
    /// [`QuadraTrayTarget`]. `None` until [`MacTrayState::ensure_item`]
    /// first runs (and stays populated afterward even if
    /// [`set_menu`] later supersedes it for click purposes — AppKit
    /// still calls into it for one more click if the menu is detached
    /// again via an empty [`ContextMenu`]).
    click_target: Option<Retained<QuadraTrayTarget>>,
    /// Target for the attached menu's item-activation selectors, set by
    /// [`set_menu`]. `None` until a non-empty menu has been attached at
    /// least once.
    menu_target: Option<Retained<QuadraMenuTarget>>,
}

impl MacTrayState {
    /// The backing `NSStatusItem`, creating it (and wiring the plain-click
    /// target) on first call. Every [`TrayService`][crate::backend::TrayService]
    /// method below calls this before touching the item, so calling
    /// [`set_tooltip`] or [`set_menu`] before [`set_icon`] still works —
    /// the icon is simply blank until a later `set_icon` call, matching
    /// `TrayService::set_icon`'s own doc ("the first successful call is
    /// what makes the icon appear at all").
    fn ensure_item(
        &mut self,
        mtm: MainThreadMarker,
        events: &Rc<RefCell<VecDeque<UiEvent>>>,
    ) -> Retained<NSStatusItem> {
        if let Some(item) = &self.status_item {
            return item.clone();
        }
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = item.button(mtm) {
            let target = QuadraTrayTarget::new(mtm, events.clone());
            // SAFETY: `target` is retained in `self.click_target` below
            // for the tray's lifetime, so the button's weak `target`
            // property never dangles — same contract
            // `menu_bar_install::append_menu_item` documents for
            // `QuadraMenuTarget`.
            unsafe { button.setAction(Some(sel!(quadraTrayAction:))) };
            let target_obj: &AnyObject = unsafe { &*(&*target as *const _ as *const AnyObject) };
            unsafe { button.setTarget(Some(target_obj)) };
            // A plain `NSButton`/`NSControl` only sends its action on a
            // left-button click by default — widen the mask so a
            // right-click (no menu attached yet) also produces
            // `UiEvent::TrayClicked` rather than being swallowed.
            button.sendActionOn(NSEventMask::LeftMouseUp | NSEventMask::RightMouseUp);
            self.click_target = Some(target);
        }
        self.status_item = Some(item.clone());
        item
    }
}

/// Obj-C target for the tray status item's plain-click action selector
/// (issue #953) — the `TrayService` analogue of
/// [`super::menu_bar_install::QuadraMenuTarget`]. Reads which mouse
/// button triggered the click off `NSApplication::currentEvent` (the
/// button control itself carries no button identity — only "the
/// configured action fired") and pushes `UiEvent::TrayClicked`.
struct QuadraTrayTargetIvars {
    events: Rc<RefCell<VecDeque<UiEvent>>>,
}

define_class!(
    // SAFETY: same posture as `QuadraMenuTarget` — created and used
    // exclusively on the main thread via `MainThreadMarker` (AppKit
    // dispatches every action selector on the main thread), no `Drop`
    // impl, ivars are owned `RefCell`/`Rc` smart pointers that drop
    // cleanly when the Obj-C runtime finalizes the instance.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = QuadraTrayTargetIvars]
    struct QuadraTrayTarget;

    unsafe impl NSObjectProtocol for QuadraTrayTarget {}

    impl QuadraTrayTarget {
        /// Action selector wired to the tray button in
        /// [`MacTrayState::ensure_item`]. Pushes
        /// `UiEvent::TrayClicked { button }` and — mirroring
        /// `QuadraMenuTarget::quadra_menu_action`'s redraw kick —
        /// triggers `setNeedsDisplay` on the key window's content view
        /// so the queued event actually gets drained this run loop turn
        /// rather than waiting for the next unrelated paint.
        #[unsafe(method(quadraTrayAction:))]
        fn quadra_tray_action(&self, _sender: &AnyObject) {
            let mtm = MainThreadMarker::from(self);
            let button = mouse_button_from_current_event(mtm);
            self.ivars()
                .events
                .borrow_mut()
                .push_back(UiEvent::TrayClicked { button });
            let app = NSApplication::sharedApplication(mtm);
            if let Some(window) = app.keyWindow() {
                if let Some(view) = window.contentView() {
                    view.setNeedsDisplay(true);
                }
            }
        }
    }
);

impl QuadraTrayTarget {
    fn new(mtm: MainThreadMarker, events: Rc<RefCell<VecDeque<UiEvent>>>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraTrayTargetIvars { events });
        unsafe { msg_send![super(this), init] }
    }
}

/// Which mouse button triggered the currently-dispatching action
/// selector, read off `NSApplication::currentEvent`. `Left` is the
/// fallback for anything not recognizably `Right`/`Other` — also the
/// answer when there is no current event at all (e.g. a
/// programmatically-triggered action) — matching
/// `QuadraMenuTarget`-adjacent code's "absence degrades to the
/// least-surprising default" posture rather than panicking.
fn mouse_button_from_current_event(mtm: MainThreadMarker) -> MouseButton {
    let app = NSApplication::sharedApplication(mtm);
    match app.currentEvent().map(|event| event.r#type()) {
        Some(NSEventType::RightMouseUp) => MouseButton::Right,
        Some(NSEventType::OtherMouseUp) => MouseButton::Middle,
        _ => MouseButton::Left,
    }
}

/// Decode `source` into an `NSImage` using AppKit's own image importers
/// (PNG/JPEG/TIFF/...) rather than routing through
/// [`super::image`]'s `CGImageSource`-based decoder: a status-item icon
/// only ever needs an `NSImage` handed straight to
/// `NSStatusBarButton::setImage`, so going through `CGImage` first would
/// buy nothing but an extra conversion step. `None` on any decode
/// failure (missing file, corrupt bytes, unrecognised format) — same
/// collapse-to-`None`/`Unsupported` posture `super::image::decode_image`
/// documents for the in-canvas `Image` primitive.
fn decode_ns_image(source: &ImageSource) -> Option<Retained<NSImage>> {
    match source {
        ImageSource::Bytes(bytes) => {
            let data = NSData::with_bytes(bytes);
            NSImage::initWithData(NSImage::alloc(), &data)
        }
        ImageSource::Path(path) => {
            let path_str = path.to_str()?;
            NSImage::initWithContentsOfFile(NSImage::alloc(), &NSString::from_str(path_str))
        }
    }
}

/// [`crate::backend::TrayService::set_icon`]'s macOS implementation —
/// see `impl TrayService for MacBackend` in `super::backend`.
pub(crate) fn set_icon(
    state: &mut MacTrayState,
    events: Rc<RefCell<VecDeque<UiEvent>>>,
    icon: ImageSource,
) -> ServiceResult<()> {
    let mtm = MainThreadMarker::new().ok_or(BackendError::Unsupported)?;
    let image = decode_ns_image(&icon).ok_or_else(|| BackendError::PlatformFailure {
        context: "NSImage failed to decode the tray icon source".to_string(),
    })?;
    let item = state.ensure_item(mtm, &events);
    if let Some(button) = item.button(mtm) {
        button.setImage(Some(&image));
    }
    Ok(())
}

/// [`crate::backend::TrayService::set_tooltip`]'s macOS implementation.
pub(crate) fn set_tooltip(
    state: &mut MacTrayState,
    events: Rc<RefCell<VecDeque<UiEvent>>>,
    tooltip: &str,
) -> ServiceResult<()> {
    let mtm = MainThreadMarker::new().ok_or(BackendError::Unsupported)?;
    let item = state.ensure_item(mtm, &events);
    if let Some(button) = item.button(mtm) {
        button.setToolTip(Some(&NSString::from_str(tooltip)));
    }
    Ok(())
}

/// [`crate::backend::TrayService::set_menu`]'s macOS implementation. An
/// empty `menu.items` detaches any previously-attached menu (see
/// `TrayService::set_menu`'s doc on why that's not the same as never
/// calling this method) — click behaviour then reverts to
/// `UiEvent::TrayClicked` via [`MacTrayState::click_target`].
pub(crate) fn set_menu(
    state: &mut MacTrayState,
    events: Rc<RefCell<VecDeque<UiEvent>>>,
    menu: &ContextMenu,
) -> ServiceResult<()> {
    let mtm = MainThreadMarker::new().ok_or(BackendError::Unsupported)?;
    let item = state.ensure_item(mtm, &events);
    if menu.items.is_empty() {
        item.setMenu(None);
        state.menu_target = None;
        return Ok(());
    }
    let target = QuadraMenuTarget::new(mtm, events, MenuKind::Context);
    let ns_menu = build_ns_menu(mtm, &menu.items, &target);
    item.setMenu(Some(&ns_menu));
    state.menu_target = Some(target);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events() -> Rc<RefCell<VecDeque<UiEvent>>> {
        Rc::new(RefCell::new(VecDeque::new()))
    }

    /// Rust's default test harness spawns a fresh OS thread per `#[test]`
    /// fn, so every method here — which all gate on `MainThreadMarker::new()`
    /// — must degrade to `Err(BackendError::Unsupported)` rather than
    /// panic. Same RED-verify shape
    /// `services::tests::show_message_dialog_off_main_thread_returns_none_not_panic`
    /// already pins for `MacPlatformServices::show_message_dialog`.
    #[test]
    fn set_icon_off_main_thread_returns_unsupported_not_panic() {
        let mut state = MacTrayState::default();
        let result = set_icon(&mut state, events(), ImageSource::Bytes(Vec::new()));
        assert_eq!(result, Err(BackendError::Unsupported));
    }

    #[test]
    fn set_tooltip_off_main_thread_returns_unsupported_not_panic() {
        let mut state = MacTrayState::default();
        let result = set_tooltip(&mut state, events(), "hello");
        assert_eq!(result, Err(BackendError::Unsupported));
    }

    #[test]
    fn set_menu_off_main_thread_returns_unsupported_not_panic() {
        let mut state = MacTrayState::default();
        let menu = ContextMenu {
            id: crate::types::WidgetId::new("tray-menu"),
            items: Vec::new(),
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
        };
        let result = set_menu(&mut state, events(), &menu);
        assert_eq!(result, Err(BackendError::Unsupported));
    }
}
