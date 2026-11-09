//! Pure application state — no rendering, no event types.
//!
//! [`crate::KubeApp`] owns a single [`AppState`] instance and mutates
//! it directly from `quadraui::AppLogic::handle`. Builders in
//! [`crate::view`] turn the state into `quadraui` primitives that
//! [`crate::KubeApp::render`] draws through the generic
//! `quadraui::Backend` trait, so the same code runs on every backend.

use quadraui::{
    fuzzy_score, Backend, Key, Modifiers, NamedKey, Palette, PaletteHit, PaletteItem, PaletteMode,
    Point, Rect, StyledSpan, StyledText, WidgetId,
};

use crate::k8s::ResourceItem;

/// Top-level app state. Cloning is cheap-ish (Vec<ResourceItem> can
/// be large but is rarely cloned outside tests). Mutation happens
/// through `quadraui::AppLogic::handle`; nothing in the rest of the
/// crate holds a long-lived borrow.
pub struct AppState {
    /// Kubeconfig context name (informational only — kube uses the
    /// default context until we add a switcher).
    pub context: String,
    /// Namespaces fetched from the cluster on startup. Falls back to
    /// `["default"]` if the list call fails so the UI is still usable.
    pub namespaces: Vec<String>,
    pub current_ns: usize,
    /// Resource kind currently displayed.
    pub kind: ResourceKind,
    pub resources: Vec<ResourceItem>,
    pub selected: usize,
    pub yaml_scroll: usize,
    /// Which pane consumes j/k. Tab cycles.
    pub focus: Focus,
    /// Modal namespace/kind picker. `Some` while open.
    pub picker: Option<PickerOverlay>,
    /// Last status / error message — surfaced in the bottom bar.
    pub status: String,
}

impl AppState {
    pub fn new(context: String, namespaces: Vec<String>) -> Self {
        let namespaces = if namespaces.is_empty() {
            vec!["default".to_string()]
        } else {
            namespaces
        };
        let current_ns = namespaces.iter().position(|n| n == "default").unwrap_or(0);
        Self {
            context,
            namespaces,
            current_ns,
            kind: ResourceKind::Pods,
            resources: Vec::new(),
            selected: 0,
            yaml_scroll: 0,
            focus: Focus::Resources,
            picker: None,
            status: "Press r to load.".to_string(),
        }
    }

    pub fn current_namespace(&self) -> &str {
        &self.namespaces[self.current_ns]
    }

    pub fn yaml_for_selected(&self) -> &str {
        self.resources
            .get(self.selected)
            .map(|r| r.yaml.as_str())
            .unwrap_or("")
    }
}

/// Kubernetes resource types the dashboard knows how to list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResourceKind {
    Pods,
    Deployments,
    Services,
    ConfigMaps,
    Secrets,
}

impl ResourceKind {
    pub const ALL: &'static [ResourceKind] = &[
        ResourceKind::Pods,
        ResourceKind::Deployments,
        ResourceKind::Services,
        ResourceKind::ConfigMaps,
        ResourceKind::Secrets,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            ResourceKind::Pods => "Pods",
            ResourceKind::Deployments => "Deployments",
            ResourceKind::Services => "Services",
            ResourceKind::ConfigMaps => "ConfigMaps",
            ResourceKind::Secrets => "Secrets",
        }
    }
}

/// Which pane consumes j/k input. Tab cycles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Resources,
    Yaml,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickerPurpose {
    Namespace,
    ResourceKind,
}

/// What happened after [`PickerOverlay::handle_key`] or
/// [`PickerOverlay::handle_mouse`] processed an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerOutcome {
    /// The user confirmed a row. `orig` is the index into `source`.
    Confirmed { orig: usize },
    /// The picker was dismissed (`Escape`, or a click outside the
    /// popup).
    Cancelled,
    /// Internal state changed (query, selection, or scroll) —
    /// caller should redraw.
    Changed,
    /// The event wasn't relevant to the picker.
    Ignored,
}

/// Modal namespace/kind picker.
///
/// `quadraui::DualModePaletteController` can't back this: its `List`
/// mode reserves `j`/`k` for vim-style selection *before* the
/// generic "append to query" arm ever sees them, so a fuzzy filter
/// over names that contain `j`/`k` (every `kube-*` namespace, the
/// canonical case this picker exists for) can never receive those
/// characters as text. Its `Input` mode has the opposite problem — it
/// accepts every character into the query, but hides the item list
/// entirely. Neither mode is "a plain search box over a filtered
/// list", so this type owns its own query/selection/scroll state
/// instead, and renders through the same `quadraui::Palette`
/// primitive and `Backend::draw_palette`/`Backend::palette_layout`
/// the controller uses internally — it just drives the state machine
/// itself rather than through the controller's key dispatch.
///
/// `filtered[row]` is the `source` index that displayed row `row`
/// currently maps to; it's recomputed by [`Self::refilter`] against
/// [`fuzzy_score`] every time the query changes, so `source`'s
/// original index survives filtering.
pub struct PickerOverlay {
    pub purpose: PickerPurpose,
    /// Palette title (e.g. `"Namespace"`).
    title: String,
    /// Full, unfiltered candidate list (namespace names or resource
    /// kind labels) in original order.
    source: Vec<String>,
    /// Index into `source` of the item the app currently has active —
    /// marked with a `●` prefix so the user can see what they're
    /// changing away from.
    current_orig: Option<usize>,
    /// `filtered[row]` is the `source` index that row `row` currently
    /// displays. Rebuilt by [`Self::refilter`].
    filtered: Vec<usize>,
    /// Rendered items for `filtered`, rebuilt alongside it.
    items: Vec<PaletteItem>,
    /// Search query text. Editing only ever appends/removes at the
    /// end (no mid-string cursor) — a search box, not a full text
    /// field.
    query: String,
    /// Index into `filtered`/`items` of the highlighted row.
    selected: usize,
    /// First visible row index (scroll offset into `items`).
    scroll_top: usize,
}

impl PickerOverlay {
    pub fn new(
        purpose: PickerPurpose,
        title: impl Into<String>,
        source: Vec<String>,
        current_orig: Option<usize>,
    ) -> Self {
        let filtered: Vec<usize> = (0..source.len()).collect();
        let items = build_items(&source, &filtered, current_orig);
        // Pre-select the currently active row so the user can press
        // Enter immediately to close without changing anything.
        let selected = current_orig
            .and_then(|cur| filtered.iter().position(|&i| i == cur))
            .unwrap_or(0);
        Self {
            purpose,
            title: title.into(),
            source,
            current_orig,
            filtered,
            items,
            query: String::new(),
            selected,
            scroll_top: 0,
        }
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Original `source` index the currently highlighted row maps
    /// to, if any (empty filter result → `None`).
    pub fn selected_orig_index(&self) -> Option<usize> {
        self.filtered.get(self.selected).copied()
    }

    /// Build the `Palette` descriptor for the current state. Public
    /// so `KubeApp::render` can paint it directly through
    /// `Backend::draw_palette` without going through a controller.
    pub fn build_palette(&self) -> Palette {
        Palette {
            id: WidgetId::new("kubeui_picker"),
            title: self.title.clone(),
            query: self.query.clone(),
            query_cursor: self.query.len(),
            items: self.items.clone(),
            selected_idx: self.selected,
            scroll_offset: self.scroll_top,
            total_count: self.source.len(),
            has_focus: true,
            show_query: true,
            create_label: None,
            preview: None,
            mode: PaletteMode::List,
        }
    }

    /// Clamp `scroll_top` so `selected` is visible within
    /// `visible_rows` rows. `visible_rows` is always a real,
    /// render-derived count (never a sentinel), so this never risks
    /// the `scroll_top + visible_rows` overflow a `usize::MAX`
    /// placeholder would invite.
    pub fn sync_scroll(&mut self, visible_rows: usize) {
        if visible_rows == 0 {
            return;
        }
        if self.selected < self.scroll_top {
            self.scroll_top = self.selected;
        }
        if self.selected >= self.scroll_top + visible_rows {
            self.scroll_top = self.selected + 1 - visible_rows;
        }
    }

    /// Drive the state machine with a keyboard event. `visible_rows`
    /// is the number of list rows that fit inside the palette chrome
    /// (see `quadraui::PALETTE_CHROME_ROWS`).
    pub fn handle_key(
        &mut self,
        key: &Key,
        modifiers: &Modifiers,
        visible_rows: usize,
    ) -> PickerOutcome {
        // `cmd` is treated the same as `ctrl` — every backend delivers
        // a Cmd-modified letter as a plain `Key::Char` with the
        // modifier set, not a dedicated key variant.
        let modified = modifiers.ctrl || modifiers.cmd;

        match key {
            Key::Named(NamedKey::Escape) => PickerOutcome::Cancelled,

            Key::Named(NamedKey::Enter) => match self.selected_orig_index() {
                Some(orig) => PickerOutcome::Confirmed { orig },
                None => PickerOutcome::Ignored,
            },

            Key::Named(NamedKey::Up) => {
                self.move_selection(-1, visible_rows);
                PickerOutcome::Changed
            }
            Key::Named(NamedKey::Down) => {
                self.move_selection(1, visible_rows);
                PickerOutcome::Changed
            }

            Key::Named(NamedKey::Backspace) if !modified => {
                if self.query.pop().is_some() {
                    self.refilter();
                    self.sync_scroll(visible_rows);
                    PickerOutcome::Changed
                } else {
                    PickerOutcome::Ignored
                }
            }

            // Printable characters — including `j`/`k` — always
            // append to the query. There is no vim-nav shortcut here;
            // arrow keys cover selection.
            Key::Char(c) if !modified => {
                self.query.push(*c);
                self.refilter();
                self.sync_scroll(visible_rows);
                PickerOutcome::Changed
            }

            _ => PickerOutcome::Ignored,
        }
    }

    /// Drive the state machine with a left-click. `position` is
    /// absolute (surface-native); `rect` is the same popup rect
    /// `KubeApp::render` painted this frame. A click outside `rect`
    /// dismisses the picker (mirrors clicking off any modal); a click
    /// on a row selects it (matches arrow-key selection — confirming
    /// still requires `Enter`).
    pub fn handle_mouse(
        &mut self,
        position: Point,
        rect: Rect,
        backend: &dyn Backend,
        visible_rows: usize,
    ) -> PickerOutcome {
        let inside = position.x >= rect.x
            && position.x < rect.x + rect.width
            && position.y >= rect.y
            && position.y < rect.y + rect.height;
        if !inside {
            return PickerOutcome::Cancelled;
        }

        let palette = self.build_palette();
        let layout = backend.palette_layout(rect, &palette);
        match layout.hit_test(position.x - rect.x, position.y - rect.y) {
            PaletteHit::Item(idx) if idx < self.items.len() => {
                self.selected = idx;
                self.sync_scroll(visible_rows);
                PickerOutcome::Changed
            }
            _ => PickerOutcome::Ignored,
        }
    }

    fn move_selection(&mut self, delta: isize, visible_rows: usize) {
        if self.filtered.is_empty() {
            return;
        }
        let len = self.filtered.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1);
        self.selected = next as usize;
        self.sync_scroll(visible_rows);
    }

    /// Recompute the filtered item list from `query` via
    /// [`fuzzy_score`], resetting selection and scroll — a query edit
    /// always starts the user back at the top match.
    ///
    /// An empty query shows every candidate, unranked, in original
    /// order — the fast path `fuzzy_score`'s own doc recommends for
    /// "nothing typed yet" (scoring would just report everything as a
    /// trivial match, so skip the sort instead of paying for one that
    /// changes nothing).
    fn refilter(&mut self) {
        let (filtered, positions): (Vec<usize>, Vec<Vec<usize>>) = if self.query.is_empty() {
            (
                (0..self.source.len()).collect(),
                vec![Vec::new(); self.source.len()],
            )
        } else {
            // RFC1123 names are ASCII, so lowercasing both sides for
            // matching keeps byte offsets into the lowercased string
            // valid as offsets into the original `source[orig]` too.
            let q = self.query.to_lowercase();
            let mut scored: Vec<(i32, usize, Vec<usize>)> = self
                .source
                .iter()
                .enumerate()
                .filter_map(|(i, s)| {
                    fuzzy_score(&s.to_lowercase(), &q).map(|(score, pos)| (score, i, pos))
                })
                .collect();
            scored.sort_by_key(|&(score, _, _)| std::cmp::Reverse(score));
            let filtered = scored.iter().map(|&(_, i, _)| i).collect();
            let positions = scored.into_iter().map(|(_, _, p)| p).collect();
            (filtered, positions)
        };
        self.items =
            build_items_with_positions(&self.source, &filtered, &positions, self.current_orig);
        self.filtered = filtered;
        self.selected = 0;
        self.scroll_top = 0;
    }
}

fn build_items(
    source: &[String],
    filtered: &[usize],
    current_orig: Option<usize>,
) -> Vec<PaletteItem> {
    let empty = vec![Vec::new(); filtered.len()];
    build_items_with_positions(source, filtered, &empty, current_orig)
}

/// Build the `PaletteItem` list for `filtered` rows. `positions[row]`
/// is forwarded to `PaletteItem::match_positions` so backends can
/// highlight why each row matched; the row mapping to `current_orig`
/// (if any) gets a `●` marker prefix.
fn build_items_with_positions(
    source: &[String],
    filtered: &[usize],
    positions: &[Vec<usize>],
    current_orig: Option<usize>,
) -> Vec<PaletteItem> {
    filtered
        .iter()
        .enumerate()
        .map(|(row, &orig)| {
            let is_current = current_orig == Some(orig);
            let prefix = if is_current { "● " } else { "  " };
            let text = format!("{prefix}{}", source[orig]);
            // Match positions are byte offsets into `source[orig]`;
            // shift them by the prefix's byte length so they still
            // land on the right characters in the prefixed text.
            let shift = prefix.len();
            let match_positions = positions
                .get(row)
                .map(|p| p.iter().map(|&b| b + shift).collect())
                .unwrap_or_default();
            PaletteItem {
                text: StyledText {
                    spans: vec![StyledSpan::plain(text)],
                },
                detail: None,
                icon: None,
                match_positions,
                depth: 0,
                expandable: false,
                expanded: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn namespaces() -> Vec<String> {
        vec![
            "default".into(),
            "kube-system".into(),
            "kube-public".into(),
            "monitoring".into(),
        ]
    }

    const ROWS: usize = 10;

    fn type_char(picker: &mut PickerOverlay, ch: char) {
        let outcome = picker.handle_key(&Key::Char(ch), &Modifiers::default(), ROWS);
        assert_eq!(outcome, PickerOutcome::Changed);
    }

    /// Reproduces the issue's own acceptance example: typing the
    /// literal characters `k`, `b`, `s` — including the two vim-nav
    /// letters `DualModePaletteController`'s List mode would have
    /// swallowed as selection moves — must land in the query text
    /// verbatim and fuzzy-match `kube-system`.
    #[test]
    fn fuzzy_query_matches_subsequence_including_j_and_k() {
        let mut picker =
            PickerOverlay::new(PickerPurpose::Namespace, "Namespace", namespaces(), Some(0));
        for ch in "kbs".chars() {
            type_char(&mut picker, ch);
        }
        assert_eq!(picker.query(), "kbs");
        let top = picker.selected_orig_index().expect("a match for 'kbs'");
        assert_eq!(picker.source[top], "kube-system");
    }

    #[test]
    fn empty_query_preserves_original_order() {
        let picker = PickerOverlay::new(PickerPurpose::Namespace, "Namespace", namespaces(), None);
        assert_eq!(picker.filtered, vec![0, 1, 2, 3]);
    }

    #[test]
    fn construction_preselects_current_item() {
        let picker =
            PickerOverlay::new(PickerPurpose::Namespace, "Namespace", namespaces(), Some(2));
        assert_eq!(picker.selected_orig_index(), Some(2));
    }

    #[test]
    fn up_down_arrows_move_selection_without_touching_query() {
        let mut picker =
            PickerOverlay::new(PickerPurpose::Namespace, "Namespace", namespaces(), Some(0));
        let outcome = picker.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default(), ROWS);
        assert_eq!(outcome, PickerOutcome::Changed);
        assert_eq!(picker.selected(), 1);
        assert_eq!(picker.query(), "");
    }

    #[test]
    fn enter_confirms_the_selected_row() {
        let mut picker =
            PickerOverlay::new(PickerPurpose::Namespace, "Namespace", namespaces(), Some(0));
        picker.handle_key(&Key::Named(NamedKey::Down), &Modifiers::default(), ROWS);
        let outcome = picker.handle_key(&Key::Named(NamedKey::Enter), &Modifiers::default(), ROWS);
        assert_eq!(outcome, PickerOutcome::Confirmed { orig: 1 });
    }

    #[test]
    fn escape_cancels() {
        let mut picker =
            PickerOverlay::new(PickerPurpose::Namespace, "Namespace", namespaces(), Some(0));
        let outcome = picker.handle_key(&Key::Named(NamedKey::Escape), &Modifiers::default(), ROWS);
        assert_eq!(outcome, PickerOutcome::Cancelled);
    }
}
