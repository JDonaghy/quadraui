//! kubeui-gtk — GTK4 Kubernetes dashboard.
//!
//! Counterpart to the TUI binary. Both shells share `kubeui-core` for
//! state, k8s client, view-builders, theme, click resolution, and the
//! action reducer; what differs here is the rasteriser (Cairo + Pango
//! instead of a ratatui buffer) and the event source (GTK signals
//! instead of crossterm events).
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
    apply_action, bootstrap_state, build_list, build_picker_menu, build_status_bar,
    build_yaml_view, picker_anchor, picker_current_index, picker_menu_width, resolve_click, theme,
    Action, AppState,
};
use quadraui::{Color, ContextMenuItemMeasure};

const APP_ID: &str = "io.github.jdonaghy.kubeui-gtk";
const UI_FONT: &str = "Monospace 11";

fn main() -> anyhow::Result<()> {
    kubeui_core::install_crypto_provider()?;
    let rt = Rc::new(tokio::runtime::Runtime::new()?);
    // Bootstrap on the main thread before handing off to GTK so the
    // first frame is already populated. Same pattern as the TUI binary.
    let state = Rc::new(RefCell::new(bootstrap_state(&rt)));

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
                resolve_click(
                    &s,
                    viewport,
                    x as f32,
                    y as f32,
                    metrics.char_w,
                    metrics.line_h,
                )
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

fn paint(cr: &Cairo, w: f64, h: f64, state: &AppState, da: &DrawingArea) {
    // Background.
    let bg = theme().background;
    cr.set_source_rgb(
        bg.r as f64 / 255.0,
        bg.g as f64 / 255.0,
        bg.b as f64 / 255.0,
    );
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
    quadraui::gtk::draw_list(
        cr,
        &layout,
        0.0,
        0.0,
        list_w,
        body_h,
        &list,
        &theme(),
        line_h,
        false,
    );

    // ── YAML pane ──────────────────────────────────────────────
    let yaml = build_yaml_view(state);
    let yaml_theme = quadraui::Theme {
        background: Color::rgb(16, 18, 24),
        ..theme()
    };
    quadraui::gtk::draw_text_display(
        cr,
        &layout,
        list_w,
        0.0,
        w - list_w,
        body_h,
        &yaml,
        &yaml_theme,
        line_h,
    );

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
