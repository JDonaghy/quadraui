//! `WinBackend` — Direct2D + DirectWrite implementation of [`Backend`].
//!
//! Every method is a `todo!()` stub. Implement against Direct2D /
//! DirectWrite (via `windows-rs`) and the compiler tells you when
//! you're done.
//!
//! # Implementation notes
//!
//! - **Render target**: `ID2D1HwndRenderTarget` or
//!   `ID2D1DeviceContext` for the main window. Offscreen:
//!   `ID2D1BitmapRenderTarget` for headless tests (#24).
//! - **Text**: `IDWriteTextFormat` + `IDWriteTextLayout` for
//!   measurement and rendering. Store `line_height` and `char_width`
//!   from `IDWriteFontMetrics` (same role as GTK's
//!   `current_line_height` / `current_char_width`).
//! - **Frame scope**: `BeginDraw()` / `EndDraw()` bracket each frame.
//!   Unlike GTK, the render target is available outside the frame
//!   scope for measurement — `_layout()` methods can use
//!   `IDWriteTextLayout` directly.
//! - **DPI**: use `GetDpiForWindow()` and scale coordinates
//!   accordingly. `Viewport::scale` carries the DPI ratio.
//! - **Events**: translate `WM_LBUTTONDOWN` → `UiEvent::MouseDown`,
//!   `WM_KEYDOWN` → `UiEvent::KeyPressed`, etc. See `win::run` and
//!   issue #20 (event translation).

use std::collections::HashMap;
use std::time::Duration;

use crate::backend::{Backend, EditorPaintResult, PlatformServices};
use crate::event::{Rect, UiEvent, Viewport};
use crate::modal_stack::ModalStack;
use crate::primitives::activity_bar::ActivityBarRowHit;
use crate::primitives::command_center::{CommandCenter, CommandCenterLayout};
use crate::primitives::completions::{Completions, CompletionsLayout};
use crate::primitives::context_menu::{ContextMenu, ContextMenuLayout};
use crate::primitives::dialog::{Dialog, DialogLayout};
use crate::primitives::editor::Editor;
use crate::primitives::find_replace::FindReplacePanel;
use crate::primitives::form::{Form, FormLayout};
use crate::primitives::menu_bar::{MenuBar, MenuBarLayout};
use crate::primitives::message_list::MessageList;
use crate::primitives::multi_section_view::{MultiSectionView, MultiSectionViewLayout};
use crate::primitives::panel::{Panel, PanelLayout};
use crate::primitives::progress::{ProgressBar, ProgressBarLayout};
use crate::primitives::rich_text_popup::{RichTextPopup, RichTextPopupLayout};
use crate::primitives::scrollbar::Scrollbar;
use crate::primitives::spinner::{Spinner, SpinnerLayout};
use crate::primitives::split::{Split, SplitLayout};
use crate::primitives::status_bar::StatusBarHitRegion;
use crate::primitives::tab_bar::TabBarHits;
use crate::primitives::text_display::TextDisplayLayout;
use crate::primitives::toast::{ToastStack, ToastStackLayout};
use crate::primitives::tooltip::{Tooltip, TooltipLayout};
use crate::primitives::tree::TreeViewLayout;
use crate::types::WidgetId;
use crate::{
    Accelerator, AcceleratorId, ActivityBar, ListView, Palette, StatusBar, TabBar, Terminal,
    TextDisplay, TreeView,
};

use super::services::WinPlatformServices;

pub struct WinBackend {
    viewport: Viewport,
    modal_stack: ModalStack,
    accelerators: HashMap<AcceleratorId, Accelerator>,
    services: WinPlatformServices,
    current_line_height: f32,
    current_char_width: f32,
    // TODO: add Direct2D / DirectWrite handles here:
    // render_target: ID2D1HwndRenderTarget,
    // text_format: IDWriteTextFormat,
    // dpi_scale: f32,
}

impl WinBackend {
    pub fn new() -> Self {
        Self {
            viewport: Viewport::new(0.0, 0.0, 1.0),
            modal_stack: ModalStack::new(),
            accelerators: HashMap::new(),
            services: WinPlatformServices::new(),
            current_line_height: 16.0,
            current_char_width: 8.0,
        }
    }
}

impl Default for WinBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for WinBackend {
    // ─── Frame + viewport ─────────────────────────────────────────────

    fn viewport(&self) -> Viewport {
        self.viewport
    }

    fn begin_frame(&mut self, viewport: Viewport) {
        self.viewport = viewport;
        todo!("ID2D1RenderTarget::BeginDraw()")
    }

    fn end_frame(&mut self) {
        todo!("ID2D1RenderTarget::EndDraw()")
    }

    // ─── Events + keybindings ─────────────────────────────────────────

    fn poll_events(&mut self) -> Vec<UiEvent> {
        todo!("PeekMessage loop → translate WM_* → UiEvent")
    }

    fn wait_events(&mut self, _timeout: Duration) -> Vec<UiEvent> {
        todo!("MsgWaitForMultipleObjects + GetMessage → UiEvent")
    }

    fn register_accelerator(&mut self, acc: &Accelerator) {
        self.accelerators.insert(acc.id.clone(), acc.clone());
    }

    fn unregister_accelerator(&mut self, id: &AcceleratorId) {
        self.accelerators.remove(id);
    }

    // ─── Modal-overlay tracking ───────────────────────────────────────

    fn modal_stack_mut(&mut self) -> &mut ModalStack {
        &mut self.modal_stack
    }

    // ─── Platform services ────────────────────────────────────────────

    fn services(&self) -> &dyn PlatformServices {
        &self.services
    }

    // ─── Measurement ──────────────────────────────────────────────────

    fn line_height(&self) -> f32 {
        self.current_line_height
    }

    // ─── Drawing ──────────────────────────────────────────────────────

    fn draw_tree(&mut self, _rect: Rect, _tree: &TreeView) {
        todo!("Direct2D tree rasteriser")
    }

    fn draw_list(&mut self, _rect: Rect, _list: &ListView) {
        todo!("Direct2D list rasteriser")
    }

    fn draw_form(&mut self, _rect: Rect, _form: &Form) {
        todo!("Direct2D form rasteriser")
    }

    fn draw_palette(&mut self, _rect: Rect, _palette: &Palette) {
        todo!("Direct2D palette rasteriser")
    }

    fn draw_status_bar(&mut self, _rect: Rect, _bar: &StatusBar) -> Vec<StatusBarHitRegion> {
        todo!("Direct2D status bar rasteriser")
    }

    fn draw_tab_bar(
        &mut self,
        _rect: Rect,
        _bar: &TabBar,
        _hovered_close_tab: Option<usize>,
    ) -> TabBarHits {
        todo!("Direct2D tab bar rasteriser")
    }

    fn draw_activity_bar(
        &mut self,
        _rect: Rect,
        _bar: &ActivityBar,
        _hovered_idx: Option<usize>,
    ) -> Vec<ActivityBarRowHit> {
        todo!("Direct2D activity bar rasteriser")
    }

    fn draw_terminal(&mut self, _rect: Rect, _term: &Terminal) {
        todo!("Direct2D terminal cell grid rasteriser")
    }

    fn draw_text_display(&mut self, _rect: Rect, _td: &TextDisplay) {
        todo!("Direct2D text display rasteriser")
    }

    fn text_display_layout(&self, _rect: Rect, _td: &TextDisplay) -> TextDisplayLayout {
        todo!("DirectWrite text display layout")
    }

    fn draw_tooltip(&mut self, _tooltip: &Tooltip, _layout: &TooltipLayout) {
        todo!("Direct2D tooltip rasteriser")
    }

    fn draw_context_menu(
        &mut self,
        _menu: &ContextMenu,
        _layout: &ContextMenuLayout,
    ) -> Vec<(Rect, WidgetId)> {
        todo!("Direct2D context menu rasteriser")
    }

    fn draw_dialog(&mut self, _dialog: &Dialog, _layout: &DialogLayout) -> Vec<Rect> {
        todo!("Direct2D dialog rasteriser")
    }

    fn draw_multi_section_view(&mut self, _rect: Rect, _view: &MultiSectionView) {
        todo!("Direct2D MSV rasteriser")
    }

    fn msv_layout(&self, _rect: Rect, _view: &MultiSectionView) -> MultiSectionViewLayout {
        todo!("DirectWrite MSV layout")
    }

    fn tree_layout(&self, _rect: Rect, _tree: &TreeView) -> TreeViewLayout {
        todo!("DirectWrite tree layout")
    }

    fn form_layout(&self, _rect: Rect, _form: &Form) -> FormLayout {
        todo!("DirectWrite form layout")
    }

    fn draw_editor(&mut self, _rect: Rect, _editor: &Editor) -> EditorPaintResult {
        todo!("Direct2D editor rasteriser")
    }

    fn draw_message_list(&mut self, _rect: Rect, _list: &MessageList) {
        todo!("Direct2D message list rasteriser")
    }

    fn draw_rich_text_popup(&mut self, _popup: &RichTextPopup, _layout: &RichTextPopupLayout) {
        todo!("Direct2D rich text popup rasteriser")
    }

    fn draw_find_replace(&mut self, _rect: Rect, _panel: &FindReplacePanel) {
        todo!("Direct2D find/replace rasteriser")
    }

    fn draw_completions(&mut self, _completions: &Completions, _layout: &CompletionsLayout) {
        todo!("Direct2D completions rasteriser")
    }

    fn draw_scrollbar(&mut self, _rect: Rect, _scrollbar: &Scrollbar) {
        todo!("Direct2D scrollbar rasteriser")
    }

    fn draw_menu_bar(&mut self, _rect: Rect, _bar: &MenuBar) -> MenuBarLayout {
        todo!("Direct2D menu bar rasteriser")
    }

    fn menu_bar_layout(&self, _rect: Rect, _bar: &MenuBar) -> MenuBarLayout {
        todo!("DirectWrite menu bar layout")
    }

    fn draw_split(&mut self, _rect: Rect, _split: &Split) -> SplitLayout {
        todo!("Direct2D split rasteriser")
    }

    fn split_layout(&self, _rect: Rect, _split: &Split) -> SplitLayout {
        todo!("DirectWrite split layout")
    }

    fn draw_panel(&mut self, _rect: Rect, _panel: &Panel) -> PanelLayout {
        todo!("Direct2D panel rasteriser")
    }

    fn panel_layout(&self, _rect: Rect, _panel: &Panel) -> PanelLayout {
        todo!("DirectWrite panel layout")
    }

    fn draw_toast_stack(&mut self, _rect: Rect, _stack: &ToastStack) -> ToastStackLayout {
        todo!("Direct2D toast stack rasteriser")
    }

    fn toast_stack_layout(&self, _rect: Rect, _stack: &ToastStack) -> ToastStackLayout {
        todo!("DirectWrite toast stack layout")
    }

    fn draw_progress(&mut self, _rect: Rect, _bar: &ProgressBar) -> ProgressBarLayout {
        todo!("Direct2D progress bar rasteriser")
    }

    fn progress_layout(&self, _rect: Rect, _bar: &ProgressBar) -> ProgressBarLayout {
        todo!("DirectWrite progress layout")
    }

    fn draw_spinner(&mut self, _rect: Rect, _spinner: &Spinner) -> SpinnerLayout {
        todo!("Direct2D spinner rasteriser")
    }

    fn spinner_layout(&self, _rect: Rect, _spinner: &Spinner) -> SpinnerLayout {
        todo!("DirectWrite spinner layout")
    }

    fn draw_command_center(&mut self, _rect: Rect, _cc: &CommandCenter) -> CommandCenterLayout {
        todo!("Direct2D command center rasteriser")
    }

    fn command_center_layout(&self, _rect: Rect, _cc: &CommandCenter) -> CommandCenterLayout {
        todo!("DirectWrite command center layout")
    }
}
