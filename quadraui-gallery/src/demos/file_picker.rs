//! `File Picker` demo — adapted from
//! `quadraui/examples/common/file_picker_app.rs` and
//! `folder_picker_app.rs`.
//!
//! Sidebar name is "File Picker" (the sidebar's demo-name column is too
//! narrow for the fuller "File & Folder Picker"), but its three variants
//! cover `FilePicker`'s two modes *plus* `FolderPicker`
//! — the in-app pickers every backend (including TUI) gets for free, no
//! native file-dialog support required. All three are rooted at a small
//! synthetic directory tree this demo creates for itself (see
//! [`build_demo_tree`]), so the listing is a fixed set of names on every
//! machine and every checkout — it neither depends on
//! `CARGO_MANIFEST_DIR` resolving at run time nor drifts as future demos
//! are added to this crate's own source tree.

use quadraui::DrawsPrimitives;
use std::path::PathBuf;

use quadraui::compose::PickerKind;
use quadraui::{
    Backend, BackendCaps, Color, FilePicker, FilePickerMode, FolderPicker, Handled, PathPicker,
    PathPickerAction, Reaction, Rect, StatusBar, StatusBarSegment, Style, UiEvent, WidgetId,
    PALETTE_CHROME_ROWS,
};
use tempfile::TempDir;

use crate::demo::{extract_region, Demo};

const SOURCE: &str = include_str!("file_picker.rs");

/// `Style`'s chrome-metric `token`, floored at the chrome line height on
/// a pixel backend — never shrunk below it, so a chrome font taller than
/// the design token isn't clipped — or the plain chrome line height on
/// TUI's fixed cell grid, where every `Style` chrome metric is a
/// documented no-op. Mirrors `ShellView`'s own token-vs-line-height carve
/// (`compose::shell_layout::compute_layout`'s `header_h`/`status_h`).
fn chrome_strip_height(backend: &dyn Backend, token: impl Fn(&Style) -> f32) -> f32 {
    let lh = backend.chrome_line_height();
    if lh <= 1.0 {
        backend.snap_height(lh)
    } else {
        backend.snap_height(lh.max(token(&backend.style())))
    }
}

// gallery:begin
/// Materialise a small, fixed directory tree for the pickers to list.
///
/// Deliberately **not** this crate's own `src/demos/` (that directory's
/// contents change every time a future demo is ported, so the picker's
/// listing — and any capture-mode snapshot of it — would silently churn
/// on unrelated PRs) and **not** a path derived from
/// `env!("CARGO_MANIFEST_DIR")` (a build-machine path baked in at
/// compile time; `CLAUDE.md`'s `cargo xwin test` flow runs the compiled
/// `.exe` on a separate Windows host where that path doesn't resolve).
///
/// The returned [`TempDir`]'s own path is unique per run, but the file
/// *names* inside it are fixed, so the listing itself is deterministic.
/// Callers must keep the `TempDir` alive for as long as the picker
/// needs its contents — dropping it deletes the directory.
fn build_demo_tree() -> TempDir {
    let dir = tempfile::tempdir().expect("create a temp dir for the file picker demo");
    let root = dir.path();
    std::fs::write(root.join("README.md"), b"# quadraui gallery demo tree\n")
        .expect("write README.md");
    std::fs::write(root.join("main.rs"), b"fn main() {}\n").expect("write main.rs");
    std::fs::create_dir(root.join("src")).expect("create src/");
    std::fs::write(root.join("src").join("lib.rs"), b"// lib\n").expect("write src/lib.rs");
    std::fs::create_dir(root.join("assets")).expect("create assets/");
    std::fs::write(root.join("assets").join("logo.svg"), b"<svg/>\n")
        .expect("write assets/logo.svg");
    dir
}

pub struct FilePickerDemo {
    // Kept alive for `FilePickerDemo`'s whole lifetime: the pickers
    // below hold only the `PathBuf` this resolves to, not the `TempDir`
    // itself, and dropping it would delete the tree out from under them.
    root_dir: TempDir,
    open_picker: FilePicker,
    save_picker: FilePicker,
    folder_picker: FolderPicker,
    confirmed: Option<String>,
}

impl FilePickerDemo {
    pub fn new() -> Self {
        let root_dir = build_demo_tree();
        let root: PathBuf = root_dir.path().to_path_buf();
        Self {
            root_dir,
            open_picker: FilePicker::new(FilePickerMode::Open, root.clone(), vec![])
                .with_id("gallery:file-picker:open"),
            save_picker: FilePicker::new(FilePickerMode::Save, root.clone(), vec![])
                .with_id("gallery:file-picker:save")
                .with_initial_filename("untitled.rs"),
            folder_picker: FolderPicker::rooted_at(root, vec![])
                .with_id("gallery:file-picker:folder"),
            confirmed: None,
        }
    }

    fn popup_rect(area: Rect, backend: &dyn Backend) -> Rect {
        let w = (area.width * 0.8).max(40.0);
        let h = (area.height * 0.8).max(12.0 * backend.line_height());
        let x = area.x + (area.width - w) / 2.0;
        let y = area.y + (area.height - h) / 2.0;
        Rect::new(x, y, w, h)
    }

    fn status_bar(&self) -> StatusBar {
        StatusBar {
            id: WidgetId::new("gallery:file-picker:status"),
            left_segments: vec![StatusBarSegment {
                text: format!(
                    " {} ",
                    self.confirmed
                        .as_deref()
                        .unwrap_or("navigate, Enter to confirm")
                ),
                fg: Color::rgb(220, 220, 220),
                bg: Color::rgb(40, 60, 100),
                bold: false,
                action_id: None,
            }],
            right_segments: vec![],
        }
    }

    fn visible_rows(popup_rect: Rect, backend: &dyn Backend) -> usize {
        let lh = backend.line_height();
        let rows = if lh > 0.0 {
            (popup_rect.height / lh) as usize
        } else {
            24
        };
        rows.saturating_sub(PALETTE_CHROME_ROWS)
    }

    /// Resolve `event` through `picker`'s own input handling, apply a
    /// browsing change, and record a confirmation (as `verb: path`) or a
    /// dismissal.
    fn drive<K: PickerKind>(
        picker: &mut PathPicker<K>,
        confirmed: &mut Option<String>,
        verb: &str,
        event: &UiEvent,
        visible_rows: usize,
    ) -> Reaction {
        match picker.handle_event(event, visible_rows) {
            Handled::Event(PathPickerAction::Confirmed { path }) => {
                *confirmed = Some(format!("{verb}: {}", path.display()));
                Reaction::Redraw
            }
            Handled::Event(PathPickerAction::Cancelled) => {
                *confirmed = Some("Dismissed".into());
                Reaction::Redraw
            }
            Handled::Event(change) => {
                picker.apply(&change, visible_rows);
                Reaction::Redraw
            }
            Handled::Consumed => Reaction::Redraw,
            Handled::Ignored => Reaction::Continue,
        }
    }
}

impl Default for FilePickerDemo {
    fn default() -> Self {
        Self::new()
    }
}

impl Demo for FilePickerDemo {
    fn name(&self) -> &'static str {
        // Not "File & Folder Picker": the sidebar's demo-name column is
        // narrow enough (~20 cells) that the longer title clips, hiding
        // the final letter rather than revealing the folder variant.
        // The "Choose folder" entry in `variants()` is what actually
        // surfaces it, once this row is selected.
        "File Picker"
    }

    fn group(&self) -> &'static str {
        "Content"
    }

    fn variants(&self) -> &'static [&'static str] {
        &["Open file", "Save file", "Choose folder"]
    }

    fn render(&self, variant: usize, backend: &mut dyn Backend, area: Rect) {
        let bar_h = chrome_strip_height(backend, |s| s.status_bar_height);
        let bar_rect = Rect::new(area.x, area.y + area.height - bar_h, area.width, bar_h);
        let _ = backend.draw_dyn(bar_rect, &self.status_bar(), &());

        let popup_rect = Self::popup_rect(area, backend);
        let _ = match variant {
            0 => backend.draw_dyn(popup_rect, &self.open_picker, &()),
            1 => backend.draw_dyn(popup_rect, &self.save_picker, &()),
            _ => backend.draw_dyn(popup_rect, &self.folder_picker, &()),
        };
    }

    fn handle(
        &mut self,
        variant: usize,
        event: &UiEvent,
        backend: &mut dyn Backend,
        area: Rect,
    ) -> Reaction {
        let popup_rect = Self::popup_rect(area, backend);
        let visible_rows = Self::visible_rows(popup_rect, backend);
        let confirmed = &mut self.confirmed;
        match variant {
            0 => Self::drive(
                &mut self.open_picker,
                confirmed,
                "Opened",
                event,
                visible_rows,
            ),
            1 => Self::drive(
                &mut self.save_picker,
                confirmed,
                "Saved",
                event,
                visible_rows,
            ),
            _ => Self::drive(
                &mut self.folder_picker,
                confirmed,
                "Chose folder",
                event,
                visible_rows,
            ),
        }
    }

    fn source(&self) -> &'static str {
        extract_region(SOURCE)
    }

    fn data(&self, variant: usize) -> serde_json::Value {
        let mode = match variant {
            0 => "open",
            1 => "save",
            _ => "folder",
        };
        serde_json::json!({
            "mode": mode,
            "root": self.root_dir.path().display().to_string(),
            "confirmed": self.confirmed,
        })
    }

    fn caps_note(&self, _variant: usize, _caps: &BackendCaps) -> Option<String> {
        // Both pickers are in-app pickers (quadraui's own
        // palette-rendered list, not a native OS dialog), so they work
        // identically everywhere — there's no `BackendCaps` flag to
        // check and no gap to report.
        None
    }
}
// gallery:end

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_tree_contains_the_fixed_synthetic_entries() {
        let dir = build_demo_tree();
        let root = dir.path();
        assert!(root.join("README.md").is_file());
        assert!(root.join("main.rs").is_file());
        assert!(root.join("src").join("lib.rs").is_file());
        assert!(root.join("assets").join("logo.svg").is_file());
    }

    #[test]
    fn save_picker_starts_with_the_initial_filename() {
        let demo = FilePickerDemo::new();
        assert_eq!(demo.save_picker.mode(), FilePickerMode::Save);
        assert_eq!(demo.save_picker.paths.query, "untitled.rs");
    }
}
