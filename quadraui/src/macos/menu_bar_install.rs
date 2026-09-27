//! NSMenu installer for `Backend::install_menu_bar` on macOS.
//!
//! Walks a `MenuBar` + its nested `ContextMenuItem` submenus into a
//! native `NSMenu` / `NSMenuItem` hierarchy and assigns it to
//! `NSApp.mainMenu`. A standard macOS app menu (`<AppName>` → About,
//! Hide, Quit, …) is auto-prepended so unbundled CLI hosts get the
//! native shape with no app-side work.
//!
//! Action selectors fire on a Rust-side Obj-C subclass
//! (`QuadraMenuTarget`) that holds a clone of the backend's event
//! queue and a `tag → WidgetId` map. When the user clicks a menu
//! item, the selector reads the sender's `tag`, looks up the
//! `WidgetId`, and pushes `UiEvent::MenuActivated(id)` onto the
//! queue. Standard app-menu items (Hide, Quit, etc.) use stock
//! AppKit selectors instead and don't route through our target.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

// objc2 0.6 (#796) removed `declare_class!` in favour of `define_class!`
// (superclass/mutability move from a `ClassType` impl block onto struct
// attributes), renamed `DeclaredClass` to `DefinedClass`, and deprecated
// `msg_send_id!` in favour of `msg_send!` (which now performs the same
// `Retained` conversion itself — see that macro's own doc).
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSControlStateValueOff, NSControlStateValueOn, NSEventModifierFlags, NSMenu,
    NSMenuItem,
};
use objc2_foundation::{MainThreadMarker, NSProcessInfo, NSString};

use crate::accelerator::{parse_key_binding, Accelerator, KeyBinding, ParsedBinding};
use crate::event::UiEvent;
use crate::primitives::context_menu::ContextMenuItem;
use crate::primitives::menu_bar::{MenuBar, MenuBarItem};
use crate::types::WidgetId;

/// Which install path created the target — determines which
/// [`UiEvent`] variant the action selector pushes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MenuKind {
    /// Installed via `Backend::install_menu_bar` (system menu bar at
    /// the top of the screen). Action → `UiEvent::MenuActivated`.
    Bar,
    /// Shown via `Backend::show_context_menu` (right-click pop-up).
    /// Action → `UiEvent::ContextMenuItemActivated`.
    Context,
}

/// Per-instance state for [`QuadraMenuTarget`]. Holds the tag-to-ID
/// map used to identify which item was clicked, the event-queue
/// handle to push activations onto, and the install kind so the
/// action selector knows which `UiEvent` variant to dispatch.
pub(crate) struct QuadraMenuTargetIvars {
    /// `NSMenuItem.tag` → `WidgetId`. Tags are auto-assigned starting
    /// at 1 (0 is reserved as "no tag" by AppKit). One entry per leaf
    /// item; submenu container items have no tag.
    tag_to_id: RefCell<HashMap<isize, WidgetId>>,
    /// Clone of the backend's event queue. The action selector pushes
    /// activations here.
    events: Rc<RefCell<VecDeque<UiEvent>>>,
    /// Selects which `UiEvent` variant the action selector pushes.
    kind: MenuKind,
}

define_class!(
    /// Obj-C target for action selectors emitted by quadraui-installed
    /// `NSMenuItem`s. One instance per `install_menu_bar` call; held
    /// alive by [`crate::macos::MacBackend`].
    //
    // SAFETY:
    // - QuadraMenuTarget is created and used exclusively on the main
    //   thread via `MainThreadMarker`. AppKit dispatches all action
    //   selectors on the main thread, so the non-Send `Rc` is safe.
    // - The class doesn't implement Drop; its ivars hold owned
    //   `RefCell` / `Rc` smart pointers that drop cleanly when the
    //   class instance is finalized by the Obj-C runtime.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = QuadraMenuTargetIvars]
    pub(crate) struct QuadraMenuTarget;

    unsafe impl NSObjectProtocol for QuadraMenuTarget {}

    impl QuadraMenuTarget {
        /// Action selector wired to every quadraui-installed
        /// `NSMenuItem`. Reads the sender's tag, looks up the
        /// associated `WidgetId`, pushes
        /// `UiEvent::MenuActivated(id)` onto the backend queue, and
        /// triggers `setNeedsDisplay` on the key window's content
        /// view so `drawRect:` fires — the paint closure drains the
        /// queue and dispatches the event through `AppLogic::handle`
        /// before painting, mirroring the TUI/GTK poll-and-dispatch
        /// shape.
        #[unsafe(method(quadraMenuAction:))]
        fn quadra_menu_action(&self, sender: &NSMenuItem) {
            // `tag`/`setNeedsDisplay:` are safe methods as of
            // objc2-app-kit 0.3 (#796 bump).
            let tag = sender.tag();
            let id = self.ivars().tag_to_id.borrow().get(&tag).cloned();
            if let Some(id) = id {
                let event = match self.ivars().kind {
                    MenuKind::Bar => UiEvent::MenuActivated(id),
                    MenuKind::Context => UiEvent::ContextMenuItemActivated(id),
                };
                self.ivars().events.borrow_mut().push_back(event);
                let mtm = MainThreadMarker::from(self);
                let app = NSApplication::sharedApplication(mtm);
                if let Some(window) = app.keyWindow() {
                    if let Some(view) = window.contentView() {
                        view.setNeedsDisplay(true);
                    }
                }
            }
        }
    }
);

impl QuadraMenuTarget {
    /// `pub(crate)`: reused directly by `super::tray`'s `TrayService::set_menu`
    /// (issue #953), which builds its own `QuadraMenuTarget` (with
    /// `MenuKind::Context`, same as `show_context_menu`) so tray-menu
    /// activations push `UiEvent::ContextMenuItemActivated` through the
    /// identical `WidgetId` path.
    pub(crate) fn new(
        mtm: MainThreadMarker,
        events: Rc<RefCell<VecDeque<UiEvent>>>,
        kind: MenuKind,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraMenuTargetIvars {
            tag_to_id: RefCell::new(HashMap::new()),
            events,
            kind,
        });
        unsafe { msg_send![super(this), init] }
    }

    /// Test-only accessor — simulates a selector dispatch without
    /// invoking AppKit. Pushes the same `UiEvent` variant the action
    /// selector would have pushed.
    #[cfg(test)]
    pub(crate) fn simulate_activation(&self, tag: isize) {
        let id = self.ivars().tag_to_id.borrow().get(&tag).cloned();
        if let Some(id) = id {
            let event = match self.ivars().kind {
                MenuKind::Bar => UiEvent::MenuActivated(id),
                MenuKind::Context => UiEvent::ContextMenuItemActivated(id),
            };
            self.ivars().events.borrow_mut().push_back(event);
        }
    }

    #[cfg(test)]
    pub(crate) fn registered_id(&self, tag: isize) -> Option<WidgetId> {
        self.ivars().tag_to_id.borrow().get(&tag).cloned()
    }
}

/// Install `bar` as the system menu bar.
///
/// Returns the [`QuadraMenuTarget`] — the backend MUST retain this so
/// the action selector's target stays alive while the menu is in use.
/// Dropping it before the menu unloads would dangle the target
/// pointer on every installed `NSMenuItem`.
///
/// The standard macOS app menu (`<AppName>` → About / Hide / Quit
/// etc.) is automatically prepended to the left of `bar.items` and
/// uses AppKit's stock selectors (`hide:`, `terminate:`, …) — no
/// app-side wiring needed.
pub(crate) fn install_menu_bar(
    mtm: MainThreadMarker,
    bar: &MenuBar,
    events: Rc<RefCell<VecDeque<UiEvent>>>,
) -> Retained<QuadraMenuTarget> {
    let target = QuadraMenuTarget::new(mtm, events, MenuKind::Bar);

    // Build the root NSMenu. Title is unused for the main menu bar but
    // assigned for diagnostics in Accessibility Inspector.
    let main_menu: Retained<NSMenu> = unsafe {
        msg_send![
            mtm.alloc::<NSMenu>(),
            initWithTitle: &*NSString::from_str(""),
        ]
    };

    // App-menu prefix uses stock AppKit selectors (target = nil → first
    // responder chain).
    let app_name = NSProcessInfo::processInfo().processName().to_string();
    append_app_menu(mtm, &main_menu, &app_name);

    let mut next_tag: isize = 1;

    for top in &bar.items {
        append_top_level_menu(mtm, &main_menu, &target, top, &mut next_tag);
    }

    // #1177: the standard "Enter/Exit Full Screen" item (and its
    // Ctrl+Cmd+F key equivalent) isn't inserted automatically for a
    // programmatically-built main menu the way it is for a nib-authored
    // one — see `append_full_screen_item`'s doc.
    append_full_screen_item(mtm, &main_menu);

    let ns_app = NSApplication::sharedApplication(mtm);
    ns_app.setMainMenu(Some(&main_menu));

    target
}

/// Install the standard macOS "Enter Full Screen" menu item (#1177).
///
/// Unlike a nib/storyboard-authored main menu, AppKit does **not**
/// auto-insert this item for a menu built entirely in code via `NSMenu`
/// — see Apple's Technical Q&A QA1830 ("Adding a Full Screen Menu Item
/// Without Using a Storyboard or Nib File"). Without it, the only way
/// into native full screen is a plain click on the green traffic
/// light, and — per #1177 — there is then no keyboard shortcut and no
/// menu item to get back out.
///
/// Placed in whichever of the app's own top-level "View" or "Window"
/// menus already exists (matching where Terminal.app / VS Code / most
/// native apps put it); if `bar` has neither, a dedicated top-level
/// "View" menu is created to hold just this one item. Either way the
/// item's action is `toggleFullScreen:` with a `nil` target, so it
/// dispatches along the standard responder chain to `NSWindow` — same
/// selector [`super::backend::MacBackend::set_fullscreen`] calls
/// directly, so the menu, the Ctrl+Cmd+F key equivalent, and any
/// programmatic call all converge on the identical AppKit transition.
///
/// The title stays `"Enter Full Screen"`; AppKit's own
/// `NSWindow`-provided `validateMenuItem:` flips it to `"Exit Full
/// Screen"` while the key window is full screen and back again on
/// exit, purely because the action is `toggleFullScreen:` — no
/// per-frame updating needed on our side.
fn append_full_screen_item(mtm: MainThreadMarker, main_menu: &NSMenu) {
    let target_menu = find_top_level_menu(main_menu, &["View", "Window"]).unwrap_or_else(|| {
        let view_item: Retained<NSMenuItem> = NSMenuItem::new(mtm);
        view_item.setTitle(&NSString::from_str("View"));
        main_menu.addItem(&view_item);
        let view_menu: Retained<NSMenu> = unsafe {
            msg_send![
                mtm.alloc::<NSMenu>(),
                initWithTitle: &*NSString::from_str("View"),
            ]
        };
        view_item.setSubmenu(Some(&view_menu));
        view_menu
    });

    if target_menu.numberOfItems() > 0 {
        target_menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    add_stock_item(
        mtm,
        &target_menu,
        "Enter Full Screen",
        sel!(toggleFullScreen:),
        "f",
        NSEventModifierFlags::Command | NSEventModifierFlags::Control,
    );
}

/// Find the first top-level menu in `main_menu` whose title matches
/// (case-insensitively) one of `titles`, in `titles`' priority order.
fn find_top_level_menu(main_menu: &NSMenu, titles: &[&str]) -> Option<Retained<NSMenu>> {
    for title in titles {
        for item in main_menu.itemArray().iter() {
            let Some(submenu) = item.submenu() else {
                continue;
            };
            if submenu.title().to_string().eq_ignore_ascii_case(title) {
                return Some(submenu);
            }
        }
    }
    None
}

/// Standard app-menu prefix. Uses native AppKit selectors so apps
/// don't supply these — they Just Work for hide / quit etc.
fn append_app_menu(mtm: MainThreadMarker, main_menu: &NSMenu, app_name: &str) {
    // Container item; title is ignored (AppKit uses the process name).
    let app_item: Retained<NSMenuItem> = NSMenuItem::new(mtm);
    main_menu.addItem(&app_item);

    let app_menu: Retained<NSMenu> = unsafe {
        msg_send![
            mtm.alloc::<NSMenu>(),
            initWithTitle: &*NSString::from_str(app_name),
        ]
    };
    app_item.setSubmenu(Some(&app_menu));

    let cmd = NSEventModifierFlags::Command;
    let cmd_opt = NSEventModifierFlags::Command | NSEventModifierFlags::Option;

    add_stock_item(
        mtm,
        &app_menu,
        &format!("About {app_name}"),
        sel!(orderFrontStandardAboutPanel:),
        "",
        NSEventModifierFlags(0),
    );
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_stock_item(
        mtm,
        &app_menu,
        &format!("Hide {app_name}"),
        sel!(hide:),
        "h",
        cmd,
    );
    add_stock_item(
        mtm,
        &app_menu,
        "Hide Others",
        sel!(hideOtherApplications:),
        "h",
        cmd_opt,
    );
    add_stock_item(
        mtm,
        &app_menu,
        "Show All",
        sel!(unhideAllApplications:),
        "",
        NSEventModifierFlags(0),
    );
    app_menu.addItem(&NSMenuItem::separatorItem(mtm));
    add_stock_item(
        mtm,
        &app_menu,
        &format!("Quit {app_name}"),
        sel!(terminate:),
        "q",
        cmd,
    );
}

fn add_stock_item(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    title: &str,
    action: Sel,
    key_equivalent: &str,
    modifiers: NSEventModifierFlags,
) {
    let title_ns = NSString::from_str(title);
    let key_ns = NSString::from_str(key_equivalent);
    let item: Retained<NSMenuItem> = unsafe {
        msg_send![
            mtm.alloc::<NSMenuItem>(),
            initWithTitle: &*title_ns,
            action: Some(action),
            keyEquivalent: &*key_ns,
        ]
    };
    item.setKeyEquivalentModifierMask(modifiers);
    menu.addItem(&item);
}

/// Append a top-level menu (File / Edit / View / …) to the main menu
/// bar. Walks `top.submenu` recursively for nested dropdowns.
fn append_top_level_menu(
    mtm: MainThreadMarker,
    main_menu: &NSMenu,
    target: &QuadraMenuTarget,
    top: &MenuBarItem,
    next_tag: &mut isize,
) {
    let title = strip_mnemonic(&top.label);
    let container: Retained<NSMenuItem> = NSMenuItem::new(mtm);
    // `setTitle:`/`setEnabled:` are safe methods as of objc2-app-kit 0.3
    // (#796 bump).
    container.setTitle(&NSString::from_str(&title));
    container.setEnabled(!top.disabled);
    main_menu.addItem(&container);

    let submenu: Retained<NSMenu> = unsafe {
        msg_send![
            mtm.alloc::<NSMenu>(),
            initWithTitle: &*NSString::from_str(&title),
        ]
    };
    container.setSubmenu(Some(&submenu));

    if let Some(items) = top.submenu.as_ref() {
        for item in items {
            append_menu_item(mtm, &submenu, target, item, next_tag);
        }
    }
}

/// Append one `ContextMenuItem` (and any nested submenu) to `menu`.
fn append_menu_item(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    target: &QuadraMenuTarget,
    item: &ContextMenuItem,
    next_tag: &mut isize,
) {
    if item.is_separator() {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        return;
    }

    let title: String = item.label.spans.iter().map(|s| s.text.as_str()).collect();
    let ns_item: Retained<NSMenuItem> = NSMenuItem::new(mtm);
    // `setTitle:`/`setEnabled:`/`setState:` are safe methods as of
    // objc2-app-kit 0.3 (#796 bump).
    ns_item.setTitle(&NSString::from_str(&title));
    ns_item.setEnabled(!item.disabled);
    if let Some(true) = item.checked {
        ns_item.setState(NSControlStateValueOn);
    } else if let Some(false) = item.checked {
        ns_item.setState(NSControlStateValueOff);
    }

    if let Some(nested) = item.submenu.as_ref() {
        // Submenu container — no action on this item; the child items
        // carry the actions.
        let child_menu: Retained<NSMenu> = unsafe {
            msg_send![
                mtm.alloc::<NSMenu>(),
                initWithTitle: &*NSString::from_str(&title),
            ]
        };
        for child in nested {
            append_menu_item(mtm, &child_menu, target, child, next_tag);
        }
        ns_item.setSubmenu(Some(&child_menu));
        menu.addItem(&ns_item);
        return;
    }

    // Leaf item: wire the action selector with a unique tag → WidgetId
    // mapping so the dispatch can look up the activated id.
    if let Some(ref id) = item.id {
        let tag = *next_tag;
        *next_tag += 1;
        // `setTag:`/`setKeyEquivalent:` are safe methods as of
        // objc2-app-kit 0.3 (#796 bump); `setAction:`/`setTarget:` still
        // require `unsafe` (the target pointer's lifetime/type isn't
        // statically verified).
        ns_item.setTag(tag);
        target
            .ivars()
            .tag_to_id
            .borrow_mut()
            .insert(tag, id.clone());

        unsafe { ns_item.setAction(Some(sel!(quadraMenuAction:))) };
        // SAFETY: target outlives the menu — MacBackend retains it for
        // the install lifetime. The cast from &QuadraMenuTarget to
        // &AnyObject is a no-op upcast through the Obj-C class chain.
        let target_obj: &AnyObject = unsafe { &*(target as *const _ as *const AnyObject) };
        unsafe { ns_item.setTarget(Some(target_obj)) };

        if let Some(ref acc) = item.key_equivalent {
            if let Some((key, mods)) = accelerator_to_ns(acc) {
                ns_item.setKeyEquivalent(&NSString::from_str(&key));
                ns_item.setKeyEquivalentModifierMask(mods);
            }
        }
    }

    menu.addItem(&ns_item);
}

/// Translate `&File` → `File` (strip the Windows-style `&` mnemonic
/// prefix that's meaningless on macOS).
fn strip_mnemonic(label: &str) -> String {
    label.replace('&', "")
}

/// Convert an [`Accelerator`] to a `(keyEquivalent, modifierMask)`
/// pair for NSMenuItem. Returns `None` for bindings that can't be
/// expressed as a single character with modifiers (rare — chord
/// shortcuts aren't menu-bindable on macOS).
fn accelerator_to_ns(acc: &Accelerator) -> Option<(String, NSEventModifierFlags)> {
    match &acc.binding {
        // Universal bindings: rendered with ⌘ as the modifier on macOS.
        KeyBinding::Save => Some(("s".into(), cmd())),
        KeyBinding::Open => Some(("o".into(), cmd())),
        KeyBinding::New => Some(("n".into(), cmd())),
        KeyBinding::Close => Some(("w".into(), cmd())),
        KeyBinding::Copy => Some(("c".into(), cmd())),
        KeyBinding::Cut => Some(("x".into(), cmd())),
        KeyBinding::Paste => Some(("v".into(), cmd())),
        KeyBinding::Undo => Some(("z".into(), cmd())),
        KeyBinding::Redo => Some(("z".into(), cmd() | shift())),
        KeyBinding::SelectAll => Some(("a".into(), cmd())),
        KeyBinding::Find => Some(("f".into(), cmd())),
        KeyBinding::Replace => Some(("h".into(), cmd())),
        KeyBinding::Quit => Some(("q".into(), cmd())),
        KeyBinding::Literal(s) => parse_key_binding(s).and_then(parsed_to_ns),
    }
}

fn parsed_to_ns(p: ParsedBinding) -> Option<(String, NSEventModifierFlags)> {
    // Map normalised key (lowercase letter / TitleCase name) to the
    // NSMenuItem `keyEquivalent` string. Only the simplest cases are
    // supported here — named keys like `Enter` / `F5` need different
    // string spellings that AppKit understands (e.g. `"\r"` for Return).
    // Apps using exotic literals can fall back to detail strings.
    let key = if p.key.chars().count() == 1 {
        p.key
    } else {
        return None;
    };
    let mut mask = NSEventModifierFlags(0);
    if p.modifiers.ctrl {
        mask |= NSEventModifierFlags::Control;
    }
    if p.modifiers.shift {
        mask |= NSEventModifierFlags::Shift;
    }
    if p.modifiers.alt {
        mask |= NSEventModifierFlags::Option;
    }
    if p.modifiers.cmd {
        mask |= NSEventModifierFlags::Command;
    }
    Some((key, mask))
}

fn cmd() -> NSEventModifierFlags {
    NSEventModifierFlags::Command
}

fn shift() -> NSEventModifierFlags {
    NSEventModifierFlags::Shift
}

/// Build a standalone `NSMenu` from `items`, wiring `target`'s
/// `quadraMenuAction:` selector on every leaf item — the exact
/// leaf-wiring loop [`show_context_menu`] runs inline below, factored out
/// so [`super::tray`]'s `TrayService::set_menu` (issue #953) can reuse
/// the identical `ContextMenu` → `NSMenu` conversion for a tray icon's
/// attached menu, rather than a second hand-rolled walk of
/// `ContextMenuItem`. `target` should be a [`QuadraMenuTarget`] built
/// with [`MenuKind::Context`] so activations push
/// `UiEvent::ContextMenuItemActivated`, the same variant a right-click
/// context menu produces.
pub(crate) fn build_ns_menu(
    mtm: MainThreadMarker,
    items: &[ContextMenuItem],
    target: &QuadraMenuTarget,
) -> Retained<NSMenu> {
    let ns_menu: Retained<NSMenu> = unsafe {
        msg_send![
            mtm.alloc::<NSMenu>(),
            initWithTitle: &*NSString::from_str(""),
        ]
    };
    let mut next_tag: isize = 1;
    for item in items {
        append_menu_item(mtm, &ns_menu, target, item, &mut next_tag);
    }
    ns_menu
}

/// Show `menu` as a native right-click context menu at view-local
/// `(x, y)`. Blocks on AppKit's modal pop-up event loop until the
/// user picks an item or dismisses.
///
/// During the modal loop the action selector pushes
/// `UiEvent::ContextMenuItemActivated(id)` onto the events queue on
/// activation. After the pop-up dismisses (with or without selection)
/// this function pushes `UiEvent::ContextMenuDismissed` so apps that
/// track open-menu state can clear it.
pub(crate) fn show_context_menu(
    mtm: MainThreadMarker,
    menu: &crate::primitives::context_menu::ContextMenu,
    anchor_x: f64,
    anchor_y: f64,
    events: Rc<RefCell<VecDeque<UiEvent>>>,
) {
    let target = QuadraMenuTarget::new(mtm, events.clone(), MenuKind::Context);
    let ns_menu: Retained<NSMenu> = unsafe {
        msg_send![
            mtm.alloc::<NSMenu>(),
            initWithTitle: &*NSString::from_str(""),
        ]
    };

    let mut next_tag: isize = 1;
    for item in &menu.items {
        append_menu_item(mtm, &ns_menu, &target, item, &mut next_tag);
    }

    // Pop up positioned at `anchor` in the key window's content view
    // coordinate space (top-left origin, matching QuadraView's
    // `isFlipped = true`). If no key window is available (e.g. tests
    // running headless), the pop-up is suppressed and we only push
    // `Dismissed`.
    let ns_app = NSApplication::sharedApplication(mtm);
    if let Some(window) = ns_app.keyWindow() {
        if let Some(view) = window.contentView() {
            let location = objc2_foundation::NSPoint::new(anchor_x, anchor_y);
            // `popUpMenuPositioningItem:atLocation:inView:` is a safe
            // method as of objc2-app-kit 0.3 (#796 bump).
            ns_menu.popUpMenuPositioningItem_atLocation_inView(None, location, Some(&view));
        }
    }

    // Always push Dismissed — apps that only care about activation
    // ignore this. `target` drops here, after `ns_menu`, so the
    // menu items' target back-references release cleanly.
    events.borrow_mut().push_back(UiEvent::ContextMenuDismissed);
    let _ = target;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accelerator::{AcceleratorId, AcceleratorScope};
    use crate::types::{StyledText, WidgetId};

    fn events() -> Rc<RefCell<VecDeque<UiEvent>>> {
        Rc::new(RefCell::new(VecDeque::new()))
    }

    fn item(id: &str, label: &str) -> ContextMenuItem {
        ContextMenuItem {
            id: Some(WidgetId::new(id)),
            label: StyledText::plain(label),
            ..Default::default()
        }
    }

    #[test]
    fn accelerator_save_maps_to_cmd_s() {
        let acc = Accelerator {
            id: AcceleratorId::new("editor.save"),
            binding: KeyBinding::Save,
            scope: AcceleratorScope::Global,
            label: None,
        };
        let (key, mask) = accelerator_to_ns(&acc).expect("Save maps");
        assert_eq!(key, "s");
        assert_eq!(mask, NSEventModifierFlags::Command);
    }

    #[test]
    fn accelerator_redo_maps_to_cmd_shift_z() {
        let acc = Accelerator {
            id: AcceleratorId::new("editor.redo"),
            binding: KeyBinding::Redo,
            scope: AcceleratorScope::Global,
            label: None,
        };
        let (key, mask) = accelerator_to_ns(&acc).expect("Redo maps");
        assert_eq!(key, "z");
        assert_eq!(
            mask,
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        );
    }

    #[test]
    fn accelerator_literal_ctrl_shift_t_maps() {
        let acc = Accelerator {
            id: AcceleratorId::new("editor.reopen-tab"),
            binding: KeyBinding::Literal("Ctrl+Shift+T".into()),
            scope: AcceleratorScope::Global,
            label: None,
        };
        let (key, mask) = accelerator_to_ns(&acc).expect("Literal maps");
        assert_eq!(key, "t");
        assert!(mask.contains(NSEventModifierFlags::Control));
        assert!(mask.contains(NSEventModifierFlags::Shift));
    }

    #[test]
    fn install_then_simulate_activation_pushes_event() {
        let Some(mtm) = MainThreadMarker::new() else {
            // Test only runs on the main thread (CI runner default).
            return;
        };
        let events = events();
        let bar = MenuBar {
            id: WidgetId::new("menubar"),
            items: vec![MenuBarItem {
                id: WidgetId::new("file"),
                label: "&File".into(),
                disabled: false,
                submenu: Some(vec![item("file.open", "Open…"), item("file.save", "Save")]),
            }],
            open_item: None,
            focused_item: None,
        };
        let target = install_menu_bar(mtm, &bar, events.clone());

        // The two leaf items got tags 1 and 2 in declaration order.
        assert_eq!(target.registered_id(1), Some(WidgetId::new("file.open")));
        assert_eq!(target.registered_id(2), Some(WidgetId::new("file.save")));

        // Simulate "Save" being activated.
        target.simulate_activation(2);
        let queued: Vec<_> = events.borrow().iter().cloned().collect();
        assert_eq!(
            queued,
            vec![UiEvent::MenuActivated(WidgetId::new("file.save"))]
        );
    }

    #[test]
    fn separator_in_submenu_does_not_consume_a_tag() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let bar = MenuBar {
            id: WidgetId::new("menubar"),
            items: vec![MenuBarItem {
                id: WidgetId::new("edit"),
                label: "&Edit".into(),
                disabled: false,
                submenu: Some(vec![
                    item("edit.cut", "Cut"),
                    ContextMenuItem::default(), // separator
                    item("edit.copy", "Copy"),
                ]),
            }],
            open_item: None,
            focused_item: None,
        };
        let target = install_menu_bar(mtm, &bar, events());
        // Tags should be 1 (cut) and 2 (copy); separator skipped.
        assert_eq!(target.registered_id(1), Some(WidgetId::new("edit.cut")));
        assert_eq!(target.registered_id(2), Some(WidgetId::new("edit.copy")));
        assert_eq!(target.registered_id(3), None);
    }

    #[test]
    fn nested_submenu_walks_recursively() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let bar = MenuBar {
            id: WidgetId::new("menubar"),
            items: vec![MenuBarItem {
                id: WidgetId::new("view"),
                label: "View".into(),
                disabled: false,
                submenu: Some(vec![ContextMenuItem {
                    id: Some(WidgetId::new("appearance")),
                    label: StyledText::plain("Appearance"),
                    submenu: Some(vec![item("appearance.zoom_in", "Zoom In")]),
                    ..Default::default()
                }]),
            }],
            open_item: None,
            focused_item: None,
        };
        let target = install_menu_bar(mtm, &bar, events());
        // Only the leaf "Zoom In" gets a tag — the "Appearance"
        // container is a submenu-bearing item, not actionable.
        assert_eq!(
            target.registered_id(1),
            Some(WidgetId::new("appearance.zoom_in")),
        );
    }

    // ── #1177 full-screen menu item / Ctrl+Cmd+F ─────────────────

    /// Find `title`'s top-level submenu in `NSApp.mainMenu()`, the same
    /// way a user visually scanning the menu bar would.
    fn find_installed_top_level_menu(
        mtm: MainThreadMarker,
        title: &str,
    ) -> Option<Retained<NSMenu>> {
        let main_menu = NSApplication::sharedApplication(mtm).mainMenu()?;
        find_top_level_menu(&main_menu, &[title])
    }

    /// Locate the leaf item within `menu` whose action is
    /// `toggleFullScreen:` — there should be exactly one after
    /// `install_menu_bar`, regardless of which top-level menu it ended
    /// up in.
    fn find_full_screen_item(menu: &NSMenu) -> Option<Retained<NSMenuItem>> {
        menu.itemArray()
            .iter()
            .find(|item| item.action() == Some(sel!(toggleFullScreen:)))
    }

    /// #1177: a `MenuBar` with neither a "View" nor a "Window" top-level
    /// menu still ends up with a native "Enter Full Screen" item —
    /// `install_menu_bar` must synthesize a "View" menu to hold it
    /// rather than silently dropping the standard shortcut the way the
    /// pre-fix code did.
    #[test]
    fn full_screen_item_creates_view_menu_when_none_exists() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let bar = MenuBar {
            id: WidgetId::new("menubar"),
            items: vec![MenuBarItem {
                id: WidgetId::new("file"),
                label: "&File".into(),
                disabled: false,
                submenu: Some(vec![item("file.open", "Open…")]),
            }],
            open_item: None,
            focused_item: None,
        };
        let _target = install_menu_bar(mtm, &bar, events());

        let view_menu =
            find_installed_top_level_menu(mtm, "View").expect("a View menu must be synthesized");
        let fs_item =
            find_full_screen_item(&view_menu).expect("Enter Full Screen item must be present");

        assert_eq!(fs_item.title().to_string(), "Enter Full Screen");
        assert_eq!(fs_item.keyEquivalent().to_string(), "f");
        assert_eq!(
            fs_item.keyEquivalentModifierMask(),
            NSEventModifierFlags::Command | NSEventModifierFlags::Control,
            "Ctrl+Cmd+F is the standard AppKit full-screen shortcut",
        );
    }

    /// #1177: when the app's own `MenuBar` already has a top-level
    /// "View" menu, the full-screen item is appended into *that* menu
    /// (after a separator) instead of creating a redundant second
    /// top-level "View" — matching where Terminal.app / VS Code / most
    /// native apps put the item, and not duplicating the menu bar.
    #[test]
    fn full_screen_item_reuses_existing_view_menu() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let bar = MenuBar {
            id: WidgetId::new("menubar"),
            items: vec![MenuBarItem {
                id: WidgetId::new("view"),
                label: "View".into(),
                disabled: false,
                submenu: Some(vec![item("view.zoom_in", "Zoom In")]),
            }],
            open_item: None,
            focused_item: None,
        };
        let _target = install_menu_bar(mtm, &bar, events());

        let main_menu = NSApplication::sharedApplication(mtm)
            .mainMenu()
            .expect("main menu installed");
        let view_menus: Vec<_> = main_menu
            .itemArray()
            .iter()
            .filter(|item| {
                item.submenu()
                    .is_some_and(|s| s.title().to_string().eq_ignore_ascii_case("view"))
            })
            .collect();
        assert_eq!(
            view_menus.len(),
            1,
            "must not create a second top-level View menu when one already exists",
        );

        let view_menu = view_menus[0].submenu().unwrap();
        find_full_screen_item(&view_menu)
            .expect("Enter Full Screen item must land in the app's existing View menu");
        // The app's own item is still there too — the fix appends, it
        // doesn't replace.
        assert!(
            view_menu
                .itemArray()
                .iter()
                .any(|i| i.title().to_string() == "Zoom In"),
            "the app's own View-menu item must survive alongside the injected full-screen item",
        );
    }

    // ── #185 native context menu ─────────────────────────────────

    fn context_menu(items: Vec<ContextMenuItem>) -> crate::primitives::context_menu::ContextMenu {
        crate::primitives::context_menu::ContextMenu {
            id: WidgetId::new("ctx"),
            items,
            selected_idx: 0,
            bg: None,
            placement: crate::primitives::context_menu::ContextMenuPlacement::default(),
        }
    }

    /// Build a context-menu target the same way `show_context_menu`
    /// does internally, without invoking AppKit's popup. Lets us
    /// round-trip the action selector + dispatch logic in unit tests.
    fn build_context_target(
        mtm: MainThreadMarker,
        menu: &crate::primitives::context_menu::ContextMenu,
        ev: Rc<RefCell<VecDeque<UiEvent>>>,
    ) -> Retained<QuadraMenuTarget> {
        let target = QuadraMenuTarget::new(mtm, ev, MenuKind::Context);
        let ns_menu: Retained<NSMenu> = unsafe {
            msg_send![
                mtm.alloc::<NSMenu>(),
                initWithTitle: &*NSString::from_str(""),
            ]
        };
        let mut next_tag: isize = 1;
        for item in &menu.items {
            append_menu_item(mtm, &ns_menu, &target, item, &mut next_tag);
        }
        target
    }

    #[test]
    fn context_menu_activation_pushes_context_variant() {
        // Same selector machinery as install_menu_bar, but with kind
        // == Context. The action selector must push
        // `ContextMenuItemActivated`, NOT `MenuActivated`, so apps
        // can route the two through different handlers.
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let ev = events();
        let menu = context_menu(vec![item("ctx.copy", "Copy"), item("ctx.paste", "Paste")]);
        let target = build_context_target(mtm, &menu, ev.clone());

        target.simulate_activation(1);
        target.simulate_activation(2);

        let queued: Vec<_> = ev.borrow().iter().cloned().collect();
        assert_eq!(
            queued,
            vec![
                UiEvent::ContextMenuItemActivated(WidgetId::new("ctx.copy")),
                UiEvent::ContextMenuItemActivated(WidgetId::new("ctx.paste")),
            ],
        );
    }

    #[test]
    fn context_menu_separator_does_not_consume_a_tag() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let menu = context_menu(vec![
            item("ctx.cut", "Cut"),
            ContextMenuItem::default(), // separator
            item("ctx.copy", "Copy"),
        ]);
        let target = build_context_target(mtm, &menu, events());
        assert_eq!(target.registered_id(1), Some(WidgetId::new("ctx.cut")));
        assert_eq!(target.registered_id(2), Some(WidgetId::new("ctx.copy")));
        assert_eq!(target.registered_id(3), None);
    }

    #[test]
    fn menu_bar_and_context_target_share_tag_numbering_independently() {
        // Each target has its own tag space — installing a menu bar
        // and showing a context menu later must not collide.
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let bar = MenuBar {
            id: WidgetId::new("menubar"),
            items: vec![MenuBarItem {
                id: WidgetId::new("file"),
                label: "File".into(),
                disabled: false,
                submenu: Some(vec![item("file.save", "Save")]),
            }],
            open_item: None,
            focused_item: None,
        };
        let menu = context_menu(vec![item("ctx.copy", "Copy")]);
        let ev = events();

        let bar_target = install_menu_bar(mtm, &bar, ev.clone());
        let ctx_target = build_context_target(mtm, &menu, ev.clone());

        // Both used tag 1, but each maps to its own WidgetId.
        assert_eq!(
            bar_target.registered_id(1),
            Some(WidgetId::new("file.save")),
        );
        assert_eq!(ctx_target.registered_id(1), Some(WidgetId::new("ctx.copy")),);

        bar_target.simulate_activation(1);
        ctx_target.simulate_activation(1);

        let queued: Vec<_> = ev.borrow().iter().cloned().collect();
        assert_eq!(
            queued,
            vec![
                UiEvent::MenuActivated(WidgetId::new("file.save")),
                UiEvent::ContextMenuItemActivated(WidgetId::new("ctx.copy")),
            ],
        );
    }
}
