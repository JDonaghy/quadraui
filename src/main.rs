//! kubeui-gtk — GTK4 Kubernetes dashboard.
//!
//! Counterpart to the TUI binary. Both shells share `kubeui-core` for
//! state, k8s client, view-builders, and the action reducer; what
//! differs here is the rasteriser (Cairo + Pango instead of a
//! ratatui buffer) and the event source (GTK signals instead of
//! crossterm events).
//!
//! A new feature lives in `kubeui-core::Action` + `apply_action`;
//! both backends pick it up the moment they bind input to it.

use std::cell::RefCell;
use std::rc::Rc;

use gtk4::cairo::Context as Cairo;
use gtk4::gdk;
use gtk4::glib;
use gtk4::pango;
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow, DrawingArea, EventControllerKey, GestureClick};

use kubeui_core::{
    apply_action, build_list, build_picker_menu, build_status_bar, decode_picker_hit_id,
    picker_anchor, picker_current_index, picker_menu_width, Action, AppState, Focus,
};
use quadraui::{Color, ContextMenuHit, ContextMenuItemMeasure, ListView};

const APP_ID: &str = "io.github.jdonaghy.kubeui-gtk";
const UI_FONT: &str = "Monospace 11";

fn main() -> anyhow::Result<()> {
    kubeui_core::install_crypto_provider()?;
    let rt = Rc::new(tokio::runtime::Runtime::new()?);

    // Bootstrap k8s state on the main thread before we hand off to
    // GTK — keeps the first frame already populated with the right
    // namespace list. Same pattern as the TUI binary's main().
    let context = rt
        .block_on(kubeui_core::current_context_name())
        .unwrap_or_else(|_| "<unknown>".to_string());
    let (namespaces, ns_status) = match rt.block_on(kubeui_core::list_namespaces()) {
        Ok(ns) => (ns, String::new()),
        Err(e) => (Vec::new(), format!("Namespace list failed: {e}")),
    };
    let ns_count = namespaces.len();

    let mut state = AppState::new(context, namespaces);
    state.status = if ns_count > 0 {
        format!("Found {ns_count} namespaces. Press r to load.")
    } else {
        ns_status
    };
    let state = Rc::new(RefCell::new(state));

    let app = Application::builder().application_id(APP_ID).build();
    {
        let state = state.clone();
        let rt = rt.clone();
        app.connect_activate(move |app| build_ui(app, state.clone(), rt.clone()));
    }
    // GTK Application::run takes args; we ignore the kubeui-gtk CLI
    // for now (no flags yet).
    let exit = app.run_with_args::<&str>(&[]);
    if exit != gtk4::glib::ExitCode::SUCCESS {
        return Err(anyhow::anyhow!("GTK exited with status {:?}", exit));
    }
    Ok(())
}

fn build_ui(app: &Application, state: Rc<RefCell<AppState>>, rt: Rc<tokio::runtime::Runtime>) {
    let drawing_area = DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .can_focus(true)
        .focusable(true)
        .build();

    // ── Draw callback ───────────────────────────────────────────────────────
    {
        let state = state.clone();
        drawing_area.set_draw_func(move |da, cr, w, h| {
            paint(cr, w as f64, h as f64, &state.borrow(), da);
        });
    }

    // ── Key controller ──────────────────────────────────────────────────────
    let key = EventControllerKey::new();
    {
        let state = state.clone();
        let rt = rt.clone();
        let da = drawing_area.clone();
        key.connect_key_pressed(move |_ctrl, key, _code, _modifier| {
            // Borrow once to read picker state, drop, then apply each
            // action with a fresh mutable borrow. apply_action may
            // call rt.block_on which can re-enter the GTK main loop;
            // holding state borrowed across that is unsafe.
            let actions = {
                let s = state.borrow();
                key_to_actions(&s, key)
            };
            let mut redraw = false;
            for a in actions {
                let mut s = state.borrow_mut();
                apply_action(&mut s, a, &rt);
                redraw = true;
                if s.should_quit {
                    drop(s);
                    if let Some(window) = da.root().and_downcast::<gtk4::Window>() {
                        window.close();
                    }
                    return glib::Propagation::Stop;
                }
            }
            if redraw {
                da.queue_draw();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }

    // ── Mouse controller ────────────────────────────────────────────────────
    let click = GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    {
        let state = state.clone();
        let rt = rt.clone();
        let da = drawing_area.clone();
        click.connect_pressed(move |_, _n_press, x, y| {
            let actions = {
                let s = state.borrow();
                let widget = da.upcast_ref::<gtk4::Widget>();
                let viewport =
                    quadraui::Rect::new(0.0, 0.0, widget.width() as f32, widget.height() as f32);
                let metrics = font_metrics(&da);
                click_to_actions(&s, viewport, metrics, x, y)
            };
            for a in actions {
                let mut s = state.borrow_mut();
                apply_action(&mut s, a, &rt);
                if s.should_quit {
                    drop(s);
                    if let Some(window) = da.root().and_downcast::<gtk4::Window>() {
                        window.close();
                    }
                    return;
                }
            }
            da.queue_draw();
        });
    }
    drawing_area.add_controller(click);

    let window = ApplicationWindow::builder()
        .application(app)
        .title("kubeui — GTK")
        .default_width(1100)
        .default_height(700)
        .child(&drawing_area)
        .build();
    window.add_controller(key);
    drawing_area.grab_focus();
    window.present();
}

// ─── Event translation ──────────────────────────────────────────────────────

fn key_to_actions(state: &AppState, key: gdk::Key) -> Vec<Action> {
    if state.picker.is_some() {
        return match key {
            gdk::Key::Escape => vec![Action::PickerCancel],
            gdk::Key::Return | gdk::Key::KP_Enter => vec![Action::PickerCommit],
            gdk::Key::Down => vec![Action::PickerMoveDown],
            gdk::Key::Up => vec![Action::PickerMoveUp],
            gdk::Key::BackSpace => vec![Action::PickerBackspace],
            other => match other.to_unicode() {
                Some(ch) if !ch.is_control() => vec![Action::PickerInput(ch)],
                _ => vec![],
            },
        };
    }
    match key {
        gdk::Key::q | gdk::Key::Escape => vec![Action::Quit],
        gdk::Key::r => vec![Action::Refresh],
        gdk::Key::n => vec![Action::OpenNamespacePicker],
        // Capital K (Shift+k). Lowercase k is "move up".
        gdk::Key::K => vec![Action::OpenKindPicker],
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => vec![Action::ToggleFocus],
        gdk::Key::j | gdk::Key::Down => vec![Action::MoveDown],
        gdk::Key::k | gdk::Key::Up => vec![Action::MoveUp],
        gdk::Key::Page_Down => vec![Action::YamlPageDown],
        gdk::Key::Page_Up => vec![Action::YamlPageUp],
        _ => vec![],
    }
}

fn click_to_actions(
    state: &AppState,
    viewport: quadraui::Rect,
    metrics: FontMetrics,
    x: f64,
    y: f64,
) -> Vec<Action> {
    // Picker (dropdown) takes precedence. Hit-test the live menu layout
    // so click resolution stays in lock-step with paint.
    if let Some(picker) = state.picker.as_ref() {
        let Some(anchor) = picker_anchor(state, viewport, metrics.char_w, metrics.line_h) else {
            return vec![Action::PickerCancel];
        };
        let menu = build_picker_menu(picker, picker_current_index(state, picker.purpose));
        let menu_w = picker_menu_width(picker, viewport, metrics.char_w);
        let menu_layout = menu.layout_at(anchor, viewport, menu_w, |_| {
            ContextMenuItemMeasure::new(metrics.line_h)
        });
        match menu_layout.hit_test(x as f32, y as f32) {
            ContextMenuHit::Item(id) => {
                if let Some(orig) = decode_picker_hit_id(id.as_str()) {
                    let visible = picker.visible_indices();
                    if let Some(visible_idx) = visible.iter().position(|&o| o == orig) {
                        return vec![Action::PickerSelectVisible(visible_idx)];
                    }
                }
                return vec![];
            }
            ContextMenuHit::Inert => return vec![],
            ContextMenuHit::Empty => return vec![Action::PickerCancel],
        }
    }

    // Status bar lives on the last `line_h` row.
    let status_top = viewport.height - metrics.line_h;
    if y as f32 >= status_top {
        let bar = build_status_bar(state);
        // `resolve_click` works in cells; convert pixel x → cell col.
        let col = (x / metrics.char_w as f64) as u16;
        let cols = (viewport.width / metrics.char_w) as usize;
        if let Some(id) = bar.resolve_click(col, cols) {
            return vec![Action::StatusBarSegmentClicked(id)];
        }
    }
    vec![]
}

// ─── Cairo rasterisation ────────────────────────────────────────────────────

#[derive(Clone, Copy)]
struct FontMetrics {
    char_w: f32,
    line_h: f32,
}

fn font_metrics(da: &DrawingArea) -> FontMetrics {
    let pango_ctx = da.pango_context();
    let desc = pango::FontDescription::from_string(UI_FONT);
    let m = pango_ctx.metrics(Some(&desc), None);
    let char_w = m.approximate_char_width() as f32 / pango::SCALE as f32;
    let line_h = (m.ascent() + m.descent()) as f32 / pango::SCALE as f32;
    FontMetrics { char_w, line_h }
}

fn set_color(cr: &Cairo, c: Color) {
    cr.set_source_rgb(c.r as f64 / 255.0, c.g as f64 / 255.0, c.b as f64 / 255.0);
}

fn paint(cr: &Cairo, w: f64, h: f64, state: &AppState, da: &DrawingArea) {
    // Background.
    set_color(cr, Color::rgb(20, 22, 30));
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();

    let metrics = font_metrics(da);
    let pango_ctx = da.pango_context();
    let layout = pango::Layout::new(&pango_ctx);
    let desc = pango::FontDescription::from_string(UI_FONT);
    layout.set_font_description(Some(&desc));

    let line_h = metrics.line_h as f64;
    let body_h = (h - line_h).max(0.0);
    let list_w = (w * 0.4).max(200.0).min(w);

    // ── Resource list pane ──────────────────────────────────────
    let list = build_list(state);
    draw_list(cr, &layout, 0.0, 0.0, list_w, body_h, line_h, &list);

    // ── YAML pane ──────────────────────────────────────────────
    draw_yaml(cr, &layout, list_w, 0.0, w - list_w, body_h, line_h, state);

    // ── Status bar ─────────────────────────────────────────────
    let bar = build_status_bar(state);
    quadraui::gtk::draw_status_bar(cr, &layout, 0.0, h - line_h, w, line_h, &bar, &theme());

    // ── Picker overlay (optional) ──────────────────────────────
    if let Some(picker) = state.picker.as_ref() {
        let viewport = quadraui::Rect::new(0.0, 0.0, w as f32, h as f32);
        if let Some(anchor) = picker_anchor(state, viewport, metrics.char_w, metrics.line_h) {
            let current = picker_current_index(state, picker.purpose);
            let menu = build_picker_menu(picker, current);
            let menu_w = picker_menu_width(picker, viewport, metrics.char_w);
            let menu_layout = menu.layout_at(anchor, viewport, menu_w, |_| {
                ContextMenuItemMeasure::new(metrics.line_h)
            });
            quadraui::gtk::draw_context_menu(cr, &layout, &menu, &menu_layout, line_h, &theme());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_list(
    cr: &Cairo,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_h: f64,
    list: &ListView,
) {
    quadraui::gtk::draw_list(cr, layout, x, y, w, h, list, &theme(), line_h, false);
}

/// Draw the YAML pane: bespoke title row + delegated `TextDisplay`
/// body. Title stays in the binary because it depends on focus state
/// and shouldn't scroll with the body.
#[allow(clippy::too_many_arguments)]
fn draw_yaml(
    cr: &Cairo,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_h: f64,
    state: &AppState,
) {
    let pane_bg = Color::rgb(16, 18, 24);
    let has_focus = state.focus == Focus::Yaml;

    // Title row: bespoke (focus-dependent string, doesn't scroll).
    set_color(cr, pane_bg);
    cr.rectangle(x, y, w, line_h);
    let _ = cr.fill();
    let title_color = if has_focus {
        Color::rgb(255, 220, 140)
    } else {
        Color::rgb(140, 200, 240)
    };
    set_color(cr, title_color);
    layout.set_text(if has_focus { " YAML  ◀ j/k" } else { " YAML" });
    cr.move_to(x + 6.0, y);
    pangocairo::functions::show_layout(cr, layout);

    // Body: delegated.
    let display = kubeui_core::build_yaml_view(state);
    let yaml_theme = quadraui::Theme {
        background: pane_bg,
        ..theme()
    };
    quadraui::gtk::draw_text_display(
        cr,
        layout,
        x + 6.0,
        y + line_h,
        w - 6.0,
        h - line_h,
        &display,
        &yaml_theme,
        line_h,
    );
}

/// Theme used for the public quadraui rasterisers. kubeui's palette is
/// hardcoded; this maps the relevant subset to `quadraui::Theme` fields
/// so the public `draw_*` rasterisers paint with kubeui's colours.
fn theme() -> quadraui::Theme {
    quadraui::Theme {
        // List/pane background.
        background: Color::rgb(22, 25, 35),
        foreground: Color::rgb(220, 220, 220),
        surface_fg: Color::rgb(220, 220, 220),
        // Selected row in the resource list.
        selected_bg: Color::rgb(50, 60, 90),
        // Right-aligned detail / dim text.
        muted_fg: Color::rgb(180, 180, 180),
        // Header (kubeui list title row); no flat strip in current UI but
        // align with the muted blue title that the legacy renderer used.
        header_bg: Color::rgb(22, 25, 35),
        header_fg: Color::rgb(160, 200, 240),
        ..quadraui::Theme::default()
    }
}
