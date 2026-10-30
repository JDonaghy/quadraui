//! Accessibility-tree snapshots for controls and application chrome.
//!
//! Every test here drives the backend-independent seam
//! (`quadraui::testing::a11y::A11yScenario`): push the same descriptors a
//! backend would paint, then assert the resulting role/state tree as text.
//! The snapshots are the contract an assistive technology sees, so they
//! spell out roles, labels and states rather than probing single fields.

use appshell_demo::AppShellDemo;
use quadraui::compose::app_shell::AppShell;
use quadraui::testing::a11y::A11yScenario;
use quadraui::{
    ActivityBar, ActivityItem, Color, CommandCenter, CommandLine, ContextMenu, ContextMenuItem,
    ContextMenuItemMeasure, Dialog, DialogButton, DialogMeasure, FieldKind, Form, FormField, Icon,
    ProgressBar, Rect, Scrollbar, Split, SplitDirection, StatusBar, StatusBarSegment, StyledText,
    Surface, TabBar, TabBarSegment, TabItem, Toast, ToastCorner, ToastOverlay, ToastSeverity,
    ToolbarItemMeasure, ValidationState, WidgetId,
};

const VIEWPORT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    width: 80.0,
    height: 24.0,
};

fn rect(y: f32, height: f32) -> Rect {
    Rect::new(0.0, y, 80.0, height)
}

// --------------------------------------------------------------- tab bar

fn tab(label: &str, active: bool) -> TabItem {
    TabItem {
        label: label.to_string(),
        is_active: active,
        is_dirty: false,
        is_preview: false,
        is_closable: true,
    }
}

#[test]
fn tab_bar_exposes_tabs_as_a_tab_list() {
    let mut dirty = tab("main.rs", false);
    dirty.is_dirty = true;
    let bar = TabBar {
        id: WidgetId::new("tabs"),
        tabs: vec![tab("lib.rs", true), dirty],
        scroll_offset: 0,
        right_segments: vec![TabBarSegment {
            text: "split".to_string(),
            width_cells: 6,
            id: Some(WidgetId::new("tabs:split")),
            is_active: false,
        }],
        active_accent: None,
        show_tab_close: true,
        compact: false,
    };
    let snapshot = A11yScenario::new()
        .with_surface(Surface::TabBar {
            rect: rect(0.0, 1.0),
            bar,
            hovered_close: None,
        })
        .snapshot();

    snapshot.assert_matches(
        "\
window \"quadraui\" [focused]
  tab_list \"Tab bar\" [size=2]
    tab \"lib.rs\" [selected pos=1/2]
      button \"Close lib.rs\"
    tab \"main.rs\" [value=\"modified\" pos=2/2]
      button \"Close main.rs\"
    button \"split\"",
    );
}

// ---------------------------------------------------------- activity bar

fn activity(id: &str, tooltip: &str, active: bool) -> ActivityItem {
    ActivityItem {
        id: WidgetId::new(id),
        icon: Icon::new("", "E"),
        tooltip: tooltip.to_string(),
        is_active: active,
        is_keyboard_selected: false,
    }
}

#[test]
fn activity_bar_items_are_tabs_in_a_toolbar() {
    let bar = ActivityBar {
        id: WidgetId::new("activity"),
        top_items: vec![
            activity("activity:explorer", "Explorer", true),
            activity("activity:search", "Search", false),
        ],
        bottom_items: vec![activity("activity:settings", "Settings", false)],
        active_accent: None,
        selection_bg: None,
        is_keyboard_focused: false,
    };
    let snapshot = A11yScenario::new()
        .with_surface(Surface::ActivityBar {
            rect: Rect::new(0.0, 0.0, 4.0, 24.0),
            bar,
            hovered: None,
        })
        .snapshot();

    snapshot.assert_matches(
        "\
window \"quadraui\" [focused]
  toolbar \"Activity bar\" [size=3]
    tab \"Explorer\" [selected pos=1/3]
    tab \"Search\" [pos=2/3]
    tab \"Settings\" [pos=3/3]",
    );
}

// ------------------------------------------------------------ status bar

fn segment(text: &str, action: Option<&str>) -> StatusBarSegment {
    StatusBarSegment {
        text: text.to_string(),
        fg: Color::rgb(220, 220, 220),
        bg: Color::rgb(30, 30, 30),
        bold: false,
        action_id: action.map(WidgetId::new),
    }
}

#[test]
fn status_bar_segments_split_into_buttons_and_labels() {
    let bar = StatusBar {
        id: WidgetId::new("status"),
        left_segments: vec![segment("main", Some("status:branch")), segment("", None)],
        right_segments: vec![segment("Ln 1, Col 1", None)],
    };
    let snapshot = A11yScenario::new()
        .with_surface(Surface::StatusBar {
            rect: rect(23.0, 1.0),
            bar,
            hovered: None,
            pressed: None,
        })
        .snapshot();

    snapshot.assert_matches(
        "\
window \"quadraui\" [focused]
  status \"Status bar\"
    button \"main\"
    label \"Ln 1, Col 1\"",
    );
}

// --------------------------------------------- command centre + command line

#[test]
fn command_center_and_command_line_expose_their_controls() {
    let snapshot = A11yScenario::new()
        .with_surface(Surface::CommandCenter {
            rect: rect(0.0, 1.0),
            cc: CommandCenter {
                id: WidgetId::new("cc"),
                back_enabled: false,
                forward_enabled: true,
                search_label: "Search files".to_string(),
            },
        })
        .with_surface(Surface::CommandLine {
            rect: rect(23.0, 1.0),
            cmd: CommandLine {
                id: WidgetId::new("cmdline"),
                text: ":w".to_string(),
                cursor_offset: Some(2),
                right_align: false,
            },
        })
        .snapshot();

    snapshot.assert_matches(
        "\
window \"quadraui\" [focused]
  toolbar \"Command center\"
    button \"Back\" [disabled]
    button \"Forward\"
    text_input \"Search files\"
  text_input \"Command line\" [value=\":w\"]",
    );
}

// ------------------------------------------------- menu bar + context menu

fn menu_item(id: &str, label: &str) -> ContextMenuItem {
    ContextMenuItem {
        id: Some(WidgetId::new(id)),
        label: StyledText::plain(label),
        ..ContextMenuItem::default()
    }
}

#[test]
fn context_menu_exposes_items_checks_and_separators() {
    let mut toggle = menu_item("menu:wrap", "Word Wrap");
    toggle.checked = Some(true);
    let mut disabled = menu_item("menu:paste", "Paste");
    disabled.disabled = true;
    let menu = ContextMenu {
        id: WidgetId::new("menu"),
        items: vec![
            menu_item("menu:copy", "Copy"),
            ContextMenuItem::default(),
            toggle,
            disabled,
        ],
        selected_idx: 2,
        bg: None,
        placement: Default::default(),
    };
    let layout = menu.layout(2.0, 2.0, VIEWPORT, 20.0, |_| {
        ContextMenuItemMeasure::new(1.0)
    });
    let snapshot = A11yScenario::new()
        .with_surface(Surface::ContextMenu { menu, layout })
        .snapshot();

    snapshot.assert_matches(
        "\
window \"quadraui\" [focused]
  menu \"Context menu\" [size=4]
    menu_item \"Copy\" [pos=1/4]
    presentation \"Separator\"
    menu_item \"Word Wrap\" [checked pos=3/4]
    menu_item \"Paste\" [disabled pos=4/4]",
    );
}

// ------------------------------------------------------------------- form

fn field(id: &str, label: &str, kind: FieldKind) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain(label),
        kind,
        hint: StyledText::plain(""),
        disabled: false,
        validation: None,
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
    }
}

#[test]
fn form_fields_carry_role_state_and_a_live_validation_alert() {
    let mut email = field(
        "form:email",
        "Email",
        FieldKind::TextInput {
            value: "not-an-email".to_string(),
            placeholder: String::new(),
            cursor: Some(0),
            selection_anchor: None,
        },
    );
    email.validation = Some(ValidationState::Error("Enter a valid address".to_string()));
    let form = Form {
        id: WidgetId::new("form"),
        fields: vec![
            email,
            field("form:news", "Newsletter", FieldKind::Toggle { value: true }),
        ],
        focused_field: None,
        scroll_offset: 0,
        has_focus: true,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
    };
    let snapshot = A11yScenario::new()
        .with_surface(Surface::Form {
            rect: rect(0.0, 6.0),
            form,
        })
        .snapshot();

    snapshot.assert_matches(
        "\
window \"quadraui\" [focused]
  form \"Form\" [size=2]
    text_input \"Email\" [value=\"not-an-email\" pos=1/2]
      alert \"Error: Enter a valid address\" [live=polite]
    check_box \"Newsletter\" [checked pos=2/2]",
    );
}

// ----------------------------------------------------------------- dialog

#[test]
fn an_open_dialog_is_modal_and_hides_the_rest_of_the_tree() {
    let dialog = Dialog {
        id: WidgetId::new("dialog:confirm"),
        title: StyledText::plain("Unsaved changes"),
        body: vec![StyledText::plain("Save before closing?")],
        buttons: vec![
            DialogButton {
                id: WidgetId::new("dialog:save"),
                label: "Save".to_string(),
                is_default: true,
                is_cancel: false,
                tint: None,
            },
            DialogButton {
                id: WidgetId::new("dialog:cancel"),
                label: "Cancel".to_string(),
                is_default: false,
                is_cancel: true,
                tint: None,
            },
        ],
        severity: None,
        vertical_buttons: false,
        table: None,
        input: None,
    };
    let measure = DialogMeasure {
        width: 40.0,
        title_height: 1.0,
        body_height: 1.0,
        table_height: 0.0,
        input_height: 0.0,
        button_row_height: 1.0,
        button_width: 8.0,
        button_gap: 1.0,
        padding: 1.0,
    };
    let layout = dialog.layout(VIEWPORT, measure, |_| ToolbarItemMeasure::new(8.0));
    let status = StatusBar {
        id: WidgetId::new("status"),
        left_segments: vec![segment("main", None)],
        right_segments: vec![],
    };
    let snapshot = A11yScenario::new()
        .with_surface(Surface::StatusBar {
            rect: rect(23.0, 1.0),
            bar: status,
            hovered: None,
            pressed: None,
        })
        .with_surface(Surface::Dialog { dialog, layout })
        .with_modal(WidgetId::new("dialog:confirm"))
        .snapshot();

    snapshot.assert_matches(
        "\
window \"quadraui\" [focused]
  status \"Status bar\" [hidden]
    label \"main\"
  dialog \"Unsaved changes\" [modal]
    label \"Save before closing?\"
    button \"Save\" [value=\"default\" pos=1/2]
    button \"Cancel\" [value=\"cancel\" pos=2/2]",
    );
}

// ------------------------------------------------------------------ toast

#[test]
fn toasts_are_polite_live_regions() {
    let stack = ToastOverlay {
        id: WidgetId::new("toasts"),
        corner: ToastCorner::BottomRight,
        toasts: vec![Toast {
            id: WidgetId::new("toast:saved"),
            title: "File saved".to_string(),
            body: "lib.rs written to disk".to_string(),
            severity: ToastSeverity::Success,
            actions: vec![],
            accent: None,
        }],
        focus: None,
    };
    let snapshot = A11yScenario::new()
        .with_surface(Surface::Toast {
            rect: rect(18.0, 5.0),
            stack,
        })
        .snapshot();

    snapshot.assert_matches(
        "\
window \"quadraui\" [focused]
  status \"Notifications\" [size=1 live=polite]
    alert \"File saved\" [value=\"success: lib.rs written to disk\" pos=1/1 live=polite]
      button \"Dismiss\"",
    );
}

// --------------------------------------------------- presentational pieces

#[test]
fn purely_visual_parts_are_presentational_not_omitted() {
    let snapshot = A11yScenario::new()
        .with_surface(Surface::Progress {
            rect: rect(0.0, 1.0),
            bar: ProgressBar {
                id: WidgetId::new("progress"),
                label: "Indexing".to_string(),
                value: Some(0.5),
                frame_idx: 0,
                cancellable: true,
                accent: None,
            },
        })
        .with_surface(Surface::Split {
            rect: rect(1.0, 10.0),
            split: Split {
                id: WidgetId::new("split"),
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first_min: 4.0,
                second_min: 4.0,
            },
        })
        .with_surface(Surface::Scrollbar {
            rect: Rect::new(79.0, 1.0, 1.0, 10.0),
            sb: Scrollbar::vertical(
                WidgetId::new("scroll"),
                Rect::new(79.0, 1.0, 1.0, 10.0),
                0.0,
                20.0,
                10.0,
                1.0,
            ),
        })
        .snapshot();

    snapshot.assert_matches(
        "\
window \"quadraui\" [focused]
  progress \"Indexing\" [range=0.5/0-1]
    presentation \"Progress track\"
    button \"Cancel\"
  group \"Split\" [value=\"horizontal\" range=0.5/0-1]
    presentation \"Split divider\"
  scroll_bar \"Vertical scrollbar\" [range=0/0-1]
    presentation \"Scrollbar thumb\"",
    );
}

// ------------------------------------------------- whole-window app shell

#[path = "../examples/common/appshell_demo.rs"]
mod appshell_demo;

/// The `appshell_demo` example's shell, built from its own `ShellConfig`
/// the way `build_shell_adapter` builds it at runtime.
fn demo_shell() -> AppShell {
    let config = AppShellDemo::config();
    let mut shell = AppShell::new(config.panels, config.default_sidebar_width)
        .with_bottom_items(config.bottom_items)
        .with_min_width(config.min_sidebar_width)
        .with_max_width(config.max_sidebar_width)
        .with_activity_bar_width(config.activity_bar_width)
        .with_position(config.position);
    for (id, icon) in config.panel_icons {
        shell = shell.with_panel_icon(id, icon);
    }
    if config.has_command_line {
        shell = shell.with_command_line();
    }
    if config.has_status_bar {
        shell = shell.with_status_bar();
    }
    if config.status_bar_under_main_column {
        shell = shell.with_status_bar_under_main_column();
    }
    shell.set_title_bar_visible(config.has_title_bar);
    shell
}

/// The sidebar header `AppShell::render` paints above the active panel's
/// content. Colours come from the backend theme and are irrelevant to the
/// accessibility tree, so any palette will do here.
fn sidebar_header(shell: &AppShell) -> StatusBar {
    let title = shell
        .active_panel()
        .map(|panel| panel.title.clone())
        .unwrap_or_default();
    StatusBar {
        id: WidgetId::new("app-shell:sidebar-header"),
        left_segments: vec![StatusBarSegment {
            text: format!(" {title} "),
            fg: Color::rgb(220, 220, 220),
            bg: Color::rgb(40, 40, 40),
            bold: true,
            action_id: None,
        }],
        right_segments: vec![],
    }
}

/// The whole-window shape an assistive technology sees for the AppShell
/// demo: the shell's own chrome (activity bar, sidebar header) plus every
/// content bar the app paints, in paint order.
#[test]
fn appshell_demo_whole_window_tree() {
    let shell = demo_shell();
    let layout = shell.layout(VIEWPORT, 1.0);
    let app = AppShellDemo::new();

    let mut scenario = A11yScenario::new().with_root_label("AppShell Demo");
    scenario.push(Surface::ActivityBar {
        rect: layout.activity_bar_bounds,
        bar: shell.build_activity_bar(),
        hovered: None,
    });
    scenario.push(Surface::StatusBar {
        rect: layout.sidebar_header_bounds.expect("sidebar header"),
        bar: sidebar_header(&shell),
        hovered: None,
        pressed: None,
    });
    for (rect, bar) in app.content_bars(&layout, 1.0) {
        scenario.push(Surface::StatusBar {
            rect,
            bar,
            hovered: None,
            pressed: None,
        });
    }

    scenario.snapshot().assert_matches(
        "\
window \"AppShell Demo\" [focused]
  toolbar \"Activity bar\" [size=4]
    tab \"Explorer\" [selected pos=1/4]
    tab \"Search\" [pos=2/4]
    tab \"Source Control\" [pos=3/4]
    tab \"Settings\" [pos=4/4]
  status \"Status bar\"
    label \"EXPLORER\"
  status \"Status bar\"
    label \"(sidebar content here)\"
  status \"Status bar\"
    label \"Tab=focus bar | click icons | drag divider | p=jump to Source Control | Ctrl+B=toggle sidebar | t=toggle title bar | q=quit\"
  status \"Status bar\"
    label \"STATUS BAR (main-column width)\"
    label \"x=25 w=55\"
  status \"Status bar\"
    label \"COMMAND LINE\"",
    );
}
