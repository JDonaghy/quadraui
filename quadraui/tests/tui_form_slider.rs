#![cfg(feature = "tui")]
//! `FieldKind::Slider` on the TUI form rasteriser: the track `draw_form`
//! paints between the `[`/`]` brackets occupies exactly the cells
//! `tui_form_layout` reports as the field's single `item_bounds` entry,
//! which is the rect `FormController`'s click routing maps a pointer x
//! across.

use quadraui::tui::{draw_form, tui_form_layout};
use quadraui::{ControlWidth, FieldKind, Form, FormField, StyledText, Theme, WidgetId};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

fn cell_char(buf: &Buffer, x: u16, y: u16) -> char {
    buf[(x, y)].symbol().chars().next().unwrap_or(' ')
}

fn field(id: &str, label_text: &str, kind: FieldKind) -> FormField {
    FormField {
        id: WidgetId::new(id),
        label: StyledText::plain(label_text),
        kind,
        disabled: false,
        validation: None,
        hint: StyledText::plain(""),
        label_position: None,
        label_align: None,
        label_width: None,
        tab_index: None,
        control_width: None,
        min_control_width: None,
        row_align: None,
    }
}

fn make_slider_form(label_text: &str, value: f32) -> Form {
    Form {
        id: WidgetId::new("settings"),
        fields: vec![field(
            "font-size",
            label_text,
            FieldKind::Slider {
                value,
                min: 8.0,
                max: 32.0,
                step: 1.0,
            },
        )],
        focused_field: None,
        scroll_offset: 0,
        has_focus: false,
        label_position: Default::default(),
        label_align: Default::default(),
        label_width: None,
        control_width: Default::default(),
        min_control_width: None,
        spacing: Default::default(),
    }
}

/// The track `draw_field_slider` paints must occupy exactly the cells
/// the layout reports as the field's single `item_bounds` entry —
/// that rect is what `FormController`'s click routing maps a pointer
/// x across, so a drift between the two would hand a user a value
/// that doesn't match the fill under their cursor. Asserted for a
/// *labeled* field, whose track sits past the label column: mapping
/// across the whole row instead would put `min` under the label.
#[test]
fn slider_track_cells_match_the_item_bounds_click_routing_reads() {
    let f = make_slider_form("Font size", 20.0);
    let area = Rect::new(0, 0, 44, 1);
    let mut buf = Buffer::empty(area);
    draw_form(&mut buf, area, &f, &Theme::default());

    let layout = tui_form_layout(&f, area);
    let vis = &layout.visible_fields[0];
    let (item_id, track) = vis.item_bounds.first().expect("Slider has one item");
    assert_eq!(
        *item_id,
        WidgetId::new("font-size"),
        "the item carries the field's own id, so a click on it resolves to the field"
    );

    let row: String = (0..44).map(|x| cell_char(&buf, x, 0)).collect();
    let open = row.find('[').expect("track is bracketed");
    let close = row.find(']').expect("track is bracketed");
    assert_eq!(
        open as f32 + 1.0,
        track.x,
        "the track's first cell is one past the `[` bracket; row: {row:?}"
    );
    assert_eq!(
        (close - open - 1) as f32,
        track.width,
        "the track spans exactly the cells between the brackets; row: {row:?}"
    );
    assert!(
        track.x > "Font size".len() as f32,
        "a labeled slider's track must start past its label column; row: {row:?}"
    );

    // The fill's own extent is the same fraction of that rect a
    // click at the pointer would resolve to: value 20 of [8, 32] is
    // exactly half way.
    let filled = row.chars().filter(|c| *c == '=').count();
    assert_eq!(
        filled as f32,
        (track.width / 2.0).round(),
        "half-way value must fill half the track; row: {row:?}"
    );
}

/// A `ControlWidth::Fill` slider stretches its track to the row's
/// remaining width instead of keeping the natural 12 cells, and the
/// reported `item_bounds` grows with it — so click routing follows
/// the wider painted track rather than a fixed-width one.
#[test]
fn a_fill_width_sliders_track_and_item_bounds_grow_together() {
    let mut natural = make_slider_form("Font size", 20.0);
    let mut filled = make_slider_form("Font size", 20.0);
    filled.control_width = ControlWidth::Fill;
    natural.control_width = ControlWidth::Content;

    let area = Rect::new(0, 0, 60, 1);
    let natural_track = tui_form_layout(&natural, area).visible_fields[0]
        .item_bounds
        .first()
        .expect("Slider has one item")
        .1;
    let filled_track = tui_form_layout(&filled, area).visible_fields[0]
        .item_bounds
        .first()
        .expect("Slider has one item")
        .1;
    assert_eq!(natural_track.width, 12.0, "the natural track is 12 cells");
    assert!(
        filled_track.width > natural_track.width,
        "Fill must stretch the track: {} vs {}",
        filled_track.width,
        natural_track.width
    );

    let mut buf = Buffer::empty(area);
    draw_form(&mut buf, area, &filled, &Theme::default());
    let row: String = (0..60).map(|x| cell_char(&buf, x, 0)).collect();
    let open = row.find('[').expect("track is bracketed");
    let close = row.find(']').expect("track is bracketed");
    assert_eq!(
        (close - open - 1) as f32,
        filled_track.width,
        "the stretched track's painted cells must match its item rect; row: {row:?}"
    );
}
