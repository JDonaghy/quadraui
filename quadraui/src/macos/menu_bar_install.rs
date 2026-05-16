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

use objc2::declare_class;
use objc2::msg_send_id;
use objc2::mutability;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, Sel};
use objc2::sel;
use objc2::{ClassType, DeclaredClass};
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

/// Per-instance state for [`QuadraMenuTarget`]. Holds the tag-to-ID
/// map used to identify which item was clicked and the event-queue
/// handle to push activations onto.
pub(crate) struct QuadraMenuTargetIvars {
    /// `NSMenuItem.tag` → `WidgetId`. Tags are auto-assigned starting
    /// at 1 (0 is reserved as "no tag" by AppKit). One entry per leaf
    /// item; submenu container items have no tag.
    tag_to_id: RefCell<HashMap<isize, WidgetId>>,
    /// Clone of the backend's event queue. The action selector pushes
    /// `UiEvent::MenuActivated` here when a tagged item is clicked.
    events: Rc<RefCell<VecDeque<UiEvent>>>,
}

declare_class!(
    /// Obj-C target for action selectors emitted by quadraui-installed
    /// `NSMenuItem`s. One instance per `install_menu_bar` call; held
    /// alive by [`crate::macos::MacBackend`].
    pub(crate) struct QuadraMenuTarget;

    // SAFETY:
    // - QuadraMenuTarget is created and used exclusively on the main
    //   thread via `MainThreadMarker`. AppKit dispatches all action
    //   selectors on the main thread, so the non-Send `Rc` is safe.
    // - The class doesn't implement Drop; its ivars hold owned
    //   `RefCell` / `Rc` smart pointers that drop cleanly when the
    //   class instance is finalized by the Obj-C runtime.
    unsafe impl ClassType for QuadraMenuTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "QuadraMenuTarget";
    }

    impl DeclaredClass for QuadraMenuTarget {
        type Ivars = QuadraMenuTargetIvars;
    }

    unsafe impl NSObjectProtocol for QuadraMenuTarget {}

    unsafe impl QuadraMenuTarget {
        /// Action selector wired to every quadraui-installed
        /// `NSMenuItem`. Reads the sender's tag, looks up the
        /// associated `WidgetId`, pushes
        /// `UiEvent::MenuActivated(id)` onto the backend queue, and
        /// triggers `setNeedsDisplay` on the key window's content
        /// view so `drawRect:` fires — the paint closure drains the
        /// queue and dispatches the event through `AppLogic::handle`
        /// before painting, mirroring the TUI/GTK poll-and-dispatch
        /// shape.
        #[method(quadraMenuAction:)]
        fn quadra_menu_action(&self, sender: &NSMenuItem) {
            let tag = unsafe { sender.tag() };
            let id = self.ivars().tag_to_id.borrow().get(&tag).cloned();
            if let Some(id) = id {
                self.ivars()
                    .events
                    .borrow_mut()
                    .push_back(UiEvent::MenuActivated(id));
                let mtm = MainThreadMarker::from(self);
                let app = NSApplication::sharedApplication(mtm);
                if let Some(window) = app.keyWindow() {
                    if let Some(view) = window.contentView() {
                        unsafe { view.setNeedsDisplay(true) };
                    }
                }
            }
        }
    }
);

impl QuadraMenuTarget {
    fn new(mtm: MainThreadMarker, events: Rc<RefCell<VecDeque<UiEvent>>>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(QuadraMenuTargetIvars {
            tag_to_id: RefCell::new(HashMap::new()),
            events,
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Test-only accessor — exposes the tag map so unit tests can
    /// verify the right WidgetIds were registered, and simulate a
    /// selector dispatch without actually invoking AppKit.
    #[cfg(test)]
    pub(crate) fn simulate_activation(&self, tag: isize) {
        let id = self.ivars().tag_to_id.borrow().get(&tag).cloned();
        if let Some(id) = id {
            self.ivars()
                .events
                .borrow_mut()
                .push_back(UiEvent::MenuActivated(id));
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
    let target = QuadraMenuTarget::new(mtm, events);

    // Build the root NSMenu. Title is unused for the main menu bar but
    // assigned for diagnostics in Accessibility Inspector.
    let main_menu: Retained<NSMenu> = unsafe {
        msg_send_id![
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

    let ns_app = NSApplication::sharedApplication(mtm);
    ns_app.setMainMenu(Some(&main_menu));

    target
}

/// Standard app-menu prefix. Uses native AppKit selectors so apps
/// don't supply these — they Just Work for hide / quit etc.
fn append_app_menu(mtm: MainThreadMarker, main_menu: &NSMenu, app_name: &str) {
    // Container item; title is ignored (AppKit uses the process name).
    let app_item: Retained<NSMenuItem> = NSMenuItem::new(mtm);
    main_menu.addItem(&app_item);

    let app_menu: Retained<NSMenu> = unsafe {
        msg_send_id![
            mtm.alloc::<NSMenu>(),
            initWithTitle: &*NSString::from_str(app_name),
        ]
    };
    app_item.setSubmenu(Some(&app_menu));

    let cmd = NSEventModifierFlags::NSEventModifierFlagCommand;
    let cmd_opt = NSEventModifierFlags::NSEventModifierFlagCommand
        | NSEventModifierFlags::NSEventModifierFlagOption;

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
        msg_send_id![
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
    unsafe { container.setTitle(&NSString::from_str(&title)) };
    unsafe { container.setEnabled(!top.disabled) };
    main_menu.addItem(&container);

    let submenu: Retained<NSMenu> = unsafe {
        msg_send_id![
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
    unsafe { ns_item.setTitle(&NSString::from_str(&title)) };
    unsafe { ns_item.setEnabled(!item.disabled) };
    if let Some(true) = item.checked {
        unsafe { ns_item.setState(NSControlStateValueOn) };
    } else if let Some(false) = item.checked {
        unsafe { ns_item.setState(NSControlStateValueOff) };
    }

    if let Some(nested) = item.submenu.as_ref() {
        // Submenu container — no action on this item; the child items
        // carry the actions.
        let child_menu: Retained<NSMenu> = unsafe {
            msg_send_id![
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
        unsafe { ns_item.setTag(tag) };
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
                unsafe { ns_item.setKeyEquivalent(&NSString::from_str(&key)) };
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
        mask |= NSEventModifierFlags::NSEventModifierFlagControl;
    }
    if p.modifiers.shift {
        mask |= NSEventModifierFlags::NSEventModifierFlagShift;
    }
    if p.modifiers.alt {
        mask |= NSEventModifierFlags::NSEventModifierFlagOption;
    }
    if p.modifiers.cmd {
        mask |= NSEventModifierFlags::NSEventModifierFlagCommand;
    }
    Some((key, mask))
}

fn cmd() -> NSEventModifierFlags {
    NSEventModifierFlags::NSEventModifierFlagCommand
}

fn shift() -> NSEventModifierFlags {
    NSEventModifierFlags::NSEventModifierFlagShift
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
        assert_eq!(mask, NSEventModifierFlags::NSEventModifierFlagCommand);
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
            NSEventModifierFlags::NSEventModifierFlagCommand
                | NSEventModifierFlags::NSEventModifierFlagShift,
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
        assert!(mask.contains(NSEventModifierFlags::NSEventModifierFlagControl));
        assert!(mask.contains(NSEventModifierFlags::NSEventModifierFlagShift));
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
}
