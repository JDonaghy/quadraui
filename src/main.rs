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
    apply_action, build_list, build_picker, build_status_bar, picker_bounds, picker_current_index,
    Action, AppState, Focus,
};
use quadraui::{Color, ListView, StyledText};

const APP_ID: &str = "io.github.jdonaghy.kubeui-gtk";
const UI_FONT: &str = "Monospace 11";
const PICKER_FONT: &str = "Monospace 12";

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
    // Picker (modal) takes precedence.
    if let Some(picker) = state.picker.as_ref() {
        let pb = picker_bounds(picker, viewport, metrics.char_w, metrics.line_h);
        let xf = x as f32;
        let yf = y as f32;
        let inside = xf >= pb.x && xf < pb.x + pb.width && yf >= pb.y && yf < pb.y + pb.height;
        if !inside {
            return vec![Action::PickerCancel];
        }
        // Title overlays row 0; items live on rows 1..h-1 of line height.
        let inner_y = (yf - pb.y) as f64;
        if inner_y > metrics.line_h as f64 && inner_y < pb.height as f64 - metrics.line_h as f64 {
            let row = ((inner_y - metrics.line_h as f64) / metrics.line_h as f64) as usize;
            return vec![Action::PickerSelectVisible(row)];
        }
        return vec![];
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
    draw_yaml(
        cr,
        &layout,
        list_w,
        0.0,
        w - list_w,
        body_h,
        line_h,
        state.yaml_for_selected(),
        state.yaml_scroll,
        state.focus == Focus::Yaml,
    );

    // ── Status bar ─────────────────────────────────────────────
    let bar = build_status_bar(state);
    quadraui::gtk::draw_status_bar(cr, &layout, 0.0, h - line_h, w, line_h, &bar, &theme());

    // ── Picker overlay (optional) ──────────────────────────────
    if let Some(picker) = state.picker.as_ref() {
        let viewport = quadraui::Rect::new(0.0, 0.0, w as f32, h as f32);
        let pb = picker_bounds(picker, viewport, metrics.char_w, metrics.line_h);
        let current = picker_current_index(state, picker.purpose);
        let view = build_picker(picker, current);
        draw_picker(
            cr,
            &layout,
            pb.x as f64,
            pb.y as f64,
            pb.width as f64,
            pb.height as f64,
            line_h,
            &view,
        );
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
    // Pane background (slightly darker than window bg).
    set_color(cr, Color::rgb(22, 25, 35));
    cr.rectangle(x, y, w, h);
    let _ = cr.fill();

    // Title row.
    let mut row_y = y;
    if let Some(title) = list.title.as_ref() {
        draw_styled_text(cr, layout, x + 6.0, row_y, title, Color::rgb(160, 200, 240));
        row_y += line_h;
    }

    // Items.
    for (i, item) in list.items.iter().enumerate() {
        if row_y + line_h > y + h {
            break;
        }
        let is_sel = i == list.selected_idx;
        if is_sel {
            set_color(cr, Color::rgb(50, 60, 90));
            cr.rectangle(x, row_y, w, line_h);
            let _ = cr.fill();
        }
        draw_styled_text(
            cr,
            layout,
            x + 6.0,
            row_y,
            &item.text,
            Color::rgb(220, 220, 220),
        );
        // Right-aligned detail.
        if let Some(detail) = item.detail.as_ref() {
            let detail_w = measure_styled(layout, detail);
            let dx = (x + w) - detail_w - 8.0;
            draw_styled_text(cr, layout, dx, row_y, detail, Color::rgb(180, 180, 180));
        }
        row_y += line_h;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_yaml(
    cr: &Cairo,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_h: f64,
    yaml: &str,
    scroll: usize,
    has_focus: bool,
) {
    // Pane background.
    set_color(cr, Color::rgb(16, 18, 24));
    cr.rectangle(x, y, w, h);
    let _ = cr.fill();

    // Title row.
    let title_color = if has_focus {
        Color::rgb(255, 220, 140)
    } else {
        Color::rgb(140, 200, 240)
    };
    set_color(cr, title_color);
    layout.set_text(if has_focus { " YAML  ◀ j/k" } else { " YAML" });
    cr.move_to(x + 6.0, y);
    pangocairo::functions::show_layout(cr, layout);

    // YAML body — naive key:value heuristic.
    let mut row_y = y + line_h;
    for line in yaml.lines().skip(scroll) {
        if row_y + line_h > y + h {
            break;
        }
        let trimmed = line.trim_start();
        if let Some(colon) = trimmed.find(':') {
            let indent = line.len() - trimmed.len();
            let key = &line[..indent + colon];
            let value = &line[indent + colon..];
            // Key in blue.
            set_color(cr, Color::rgb(140, 200, 240));
            layout.set_text(key);
            cr.move_to(x + 6.0, row_y);
            pangocairo::functions::show_layout(cr, layout);
            let (key_w, _) = layout.pixel_size();
            // Value in default fg.
            set_color(cr, Color::rgb(200, 200, 200));
            layout.set_text(value);
            cr.move_to(x + 6.0 + key_w as f64, row_y);
            pangocairo::functions::show_layout(cr, layout);
        } else {
            set_color(cr, Color::rgb(200, 200, 200));
            layout.set_text(line);
            cr.move_to(x + 6.0, row_y);
            pangocairo::functions::show_layout(cr, layout);
        }
        row_y += line_h;
    }
}

/// Theme used for the public quadraui rasterisers. kubeui's palette is
/// hardcoded; the only field consumed today is the fallback fill colour
/// `quadraui::gtk::draw_status_bar` uses when its bar has no segments.
fn theme() -> quadraui::Theme {
    quadraui::Theme {
        background: Color::rgb(40, 40, 60),
        foreground: Color::rgb(220, 220, 220),
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_picker(
    cr: &Cairo,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    line_h: f64,
    list: &ListView,
) {
    // Backdrop (modal-ish — slight inset shadow effect via darker bg).
    set_color(cr, Color::rgb(28, 32, 44));
    cr.rectangle(x, y, w, h);
    let _ = cr.fill();

    // Border (single line).
    set_color(cr, Color::rgb(120, 160, 200));
    cr.set_line_width(1.0);
    cr.rectangle(x + 0.5, y + 0.5, w - 1.0, h - 1.0);
    let _ = cr.stroke();

    // Title overlay on top border.
    if let Some(title) = list.title.as_ref() {
        if let Some(span) = title.spans.first() {
            // Background under the title text so it cuts the border.
            layout.set_text(&format!(" {} ", span.text));
            let (tw, _) = layout.pixel_size();
            set_color(cr, Color::rgb(28, 32, 44));
            cr.rectangle(x + 8.0, y - 1.0, tw as f64, line_h);
            let _ = cr.fill();
            set_color(cr, Color::rgb(120, 160, 200));
            cr.move_to(x + 8.0, y);
            // Picker title slightly larger for visual hierarchy.
            let bigger = pango::FontDescription::from_string(PICKER_FONT);
            layout.set_font_description(Some(&bigger));
            layout.set_text(&format!(" {} ", span.text));
            pangocairo::functions::show_layout(cr, layout);
            // Restore default font.
            let normal = pango::FontDescription::from_string(UI_FONT);
            layout.set_font_description(Some(&normal));
        }
    }

    // Items.
    let mut row_y = y + line_h;
    for (i, item) in list.items.iter().enumerate() {
        if row_y + line_h > y + h - line_h {
            break;
        }
        let is_sel = i == list.selected_idx;
        if is_sel {
            set_color(cr, Color::rgb(60, 80, 120));
            cr.rectangle(x + 1.0, row_y, w - 2.0, line_h);
            let _ = cr.fill();
        }
        if let Some(span) = item.text.spans.first() {
            set_color(cr, Color::rgb(220, 220, 220));
            layout.set_text(&span.text);
            cr.move_to(x + 12.0, row_y);
            pangocairo::functions::show_layout(cr, layout);
        }
        row_y += line_h;
    }
}

// ─── Pango helpers ──────────────────────────────────────────────────────────

fn measure_styled(layout: &pango::Layout, st: &StyledText) -> f64 {
    let total: String = st.spans.iter().map(|s| s.text.as_str()).collect();
    layout.set_text(&total);
    layout.pixel_size().0 as f64
}

fn draw_styled_text(
    cr: &Cairo,
    layout: &pango::Layout,
    x: f64,
    y: f64,
    st: &StyledText,
    default_fg: Color,
) {
    let mut cx = x;
    for span in &st.spans {
        let fg = span.fg.unwrap_or(default_fg);
        set_color(cr, fg);
        layout.set_text(&span.text);
        cr.move_to(cx, y);
        pangocairo::functions::show_layout(cr, layout);
        cx += layout.pixel_size().0 as f64;
    }
}
