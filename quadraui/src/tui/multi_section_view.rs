//! TUI entry points for [`crate::MultiSectionView`]: paint it through the
//! widget's own cell painter over a [`ratatui::buffer::Buffer`], and
//! resolve the chrome layout that paint uses for hit-testing.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect as TuiRect;

use super::BufferCellSurface;
use crate::event::Rect;
use crate::primitive::{LayoutCtx, Primitive};
use crate::primitives::multi_section_view::{
    cell_msv_layout, MultiSectionView, MultiSectionViewLayout,
};
use crate::theme::Theme;

fn q_rect(area: TuiRect) -> Rect {
    Rect::new(
        area.x as f32,
        area.y as f32,
        area.width as f32,
        area.height as f32,
    )
}

/// Paint `view` into `area` on `buf`, each section body through its own
/// widget; `nerd_fonts_enabled` reaches the tree and list bodies.
pub fn draw_multi_section_view(
    buf: &mut Buffer,
    area: TuiRect,
    view: &MultiSectionView,
    theme: &Theme,
    nerd_fonts_enabled: bool,
) {
    let ctx = LayoutCtx::new(1.0, 1.0, nerd_fonts_enabled);
    let layout = Primitive::layout(view, &ctx, q_rect(area));
    let style = crate::Style::default();
    view.paint_cells(
        &layout,
        &mut BufferCellSurface::new(buf),
        &crate::PaintEnv::new(theme, &style),
    );
}

/// The chrome layout [`draw_multi_section_view`] paints `view` with in
/// `area`, for hit-testing what it painted.
pub fn tui_msv_layout(view: &MultiSectionView, area: TuiRect) -> MultiSectionViewLayout {
    cell_msv_layout(view, q_rect(area))
}
