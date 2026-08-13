//! Conformance scenario **schema v0** (quadraui#491, audit §6.4).
//!
//! A scenario is a JSON file under
//! `quadraui/tests/conformance/scenarios/<area>/<name>.scn.json`. It names
//! a fixture, a logical viewport, an optional list of required backend
//! capabilities, and an ordered list of steps. The runner
//! (`super::runner`) replays those steps against every registered backend
//! driver.
//!
//! ## The one structural invariant: no coordinates
//!
//! **There is no numeric coordinate field anywhere in this schema.** No
//! step carries an `x`, a `y`, a `row`, a `col`, or a pixel offset — every
//! act step names *painted text* (`click_text`, `drag_text`, `scroll_at`)
//! and every geometric assertion names *two painted things* and asks how
//! they relate (`assert_left_of`, `assert_above`, `assert_inside`). A
//! hardcoded coordinate is therefore not "discouraged by review", it is
//! **unrepresentable**: `serde` rejects any unknown step key outright, so
//! `{"click_at": {"x": 12, "y": 3}}` fails to deserialise. That is the
//! whole point — TUI cells and GTK pixels are different units, so a literal
//! in a shared body would silently be wrong on one side.
//!
//! The four numbers that *do* appear are all unit-free counts, not
//! positions: `tier` (a conformance tier, C0–C4), `viewport.cols`/`rows`
//! (logical cells, scaled per backend by [`LogicalViewport`]),
//! `scroll_at.lines` (a wheel-notch count in `line_height` multiples), and
//! `assert_count.count` (a cardinality).
//!
//! ## Adding a step kind
//!
//! Add a variant to [`Step`] and a match arm in
//! [`super::runner::run_scenario`]. Adding a *scenario* needs no Rust at
//! all — drop a `.scn.json` file in `scenarios/<area>/`.

use serde::Deserialize;

use quadraui::testing::{Anchor, LogicalViewport};
use quadraui::NamedKey;

/// Backend-neutral viewport declaration. Deserialises straight into
/// [`LogicalViewport`], which each backend interprets in its own units
/// (TUI: cells; GTK: `cols × char_width` / `rows × line_height` pixels).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ViewportSpec {
    pub cols: u32,
    pub rows: u32,
}

impl From<ViewportSpec> for LogicalViewport {
    fn from(v: ViewportSpec) -> Self {
        LogicalViewport::new(v.cols, v.rows)
    }
}

/// Where within a located text run a click lands. Mirrors
/// [`quadraui::testing::Anchor`] with a serde-friendly spelling.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum AnchorSpec {
    #[default]
    Center,
    LeftEdge,
    RightEdge,
}

impl From<AnchorSpec> for Anchor {
    fn from(a: AnchorSpec) -> Self {
        match a {
            AnchorSpec::Center => Anchor::Center,
            AnchorSpec::LeftEdge => Anchor::LeftEdge,
            AnchorSpec::RightEdge => Anchor::RightEdge,
        }
    }
}

/// One scenario step.
///
/// Serde's default **externally tagged** enum representation is exactly the
/// audit's "every step is one key" rule: `{"press": "Right"}`,
/// `{"drag_text": {"from": "alpha", "to": "gamma"}}`,
/// `{"assert_exited": true}`. Unknown keys are a hard deserialisation
/// error, which is what makes the no-coordinates invariant structural
/// rather than advisory.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Step {
    // ── Act ────────────────────────────────────────────────────────────
    /// Press a named (non-printable) key: `{"press": "Right"}`,
    /// `{"press": "F6"}`. See [`parse_named_key`].
    Press(String),
    /// Type one character with no modifiers: `{"type_char": "q"}`.
    TypeChar(char),
    /// Type each character of a string in turn: `{"type_text": "feat"}`.
    TypeText(String),
    /// Press a character key with Ctrl held: `{"ctrl_char": "c"}`.
    CtrlChar(char),
    /// Click the centre of the first painted run containing this text.
    ClickText(String),
    /// Click a specific anchor within a painted run:
    /// `{"click_text_at": {"text": "Name", "anchor": "right_edge"}}`.
    /// `anchor` is optional and defaults to `center` (same as `click_text`)
    /// when omitted — this is what makes `AnchorSpec`'s `#[default]`
    /// meaningful rather than dead code.
    ClickTextAt {
        text: String,
        #[serde(default)]
        anchor: AnchorSpec,
    },
    /// Drag from one painted run's centre to another's:
    /// `{"drag_text": {"from": "alpha", "to": "gamma"}}`.
    DragText { from: String, to: String },
    /// Scroll `lines` notches with the pointer over a painted run:
    /// `{"scroll_at": {"target": "row 1", "lines": -3}}`. Positive scrolls
    /// up, matching [`quadraui::ScrollDelta`].
    ScrollAt { target: String, lines: i32 },

    // ── Assert ─────────────────────────────────────────────────────────
    /// Some painted text run contains this needle.
    AssertScreenHas(String),
    /// No painted text run contains this needle.
    AssertAbsent(String),
    /// Exactly `count` painted runs contain `text`.
    AssertCount { text: String, count: usize },
    /// `a`'s painted bounds end at or before `b`'s begin, horizontally.
    AssertLeftOf { a: String, b: String },
    /// `a`'s painted bounds end at or before `b`'s begin, vertically.
    AssertAbove { a: String, b: String },
    /// `a`'s painted bounds lie entirely within the registered zone
    /// `zone` (a [`quadraui::WidgetId`] string).
    AssertInside { a: String, zone: String },
    /// The app has (or has not) returned `Reaction::Exit`.
    AssertExited(bool),

    // ── Document ───────────────────────────────────────────────────────
    /// Free-text commentary. Never executed; printed on failure so the
    /// surrounding steps read as prose in the report.
    Note(String),
}

/// A whole scenario file.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// Stable dotted id, e.g. `pipeline.click_advances_stage`. Must match
    /// the file stem so a failure in the matrix table is greppable.
    pub id: String,
    /// Fixture name, resolved through [`super::fixtures`].
    pub fixture: String,
    /// Conformance tier (see `docs/TESTING.md` → *Conformance tiers*).
    pub tier: u8,
    pub viewport: ViewportSpec,
    /// Backend capabilities this scenario needs. A backend that does not
    /// declare all of them **skips** the scenario, with the missing
    /// capability named in the matrix — silence is impossible.
    #[serde(default)]
    pub requires: Vec<String>,
    pub steps: Vec<Step>,
}

impl Scenario {
    /// Parse a scenario from JSON source, tagging errors with `origin`
    /// (normally the file path) so a typo in one file names that file.
    pub fn from_json(origin: &str, src: &str) -> Result<Self, String> {
        serde_json::from_str::<Scenario>(src).map_err(|e| format!("{origin}: {e}"))
    }
}

/// Map a scenario's `press` spelling to a [`NamedKey`].
///
/// Deliberately a small explicit table rather than `serde`'s derived
/// representation for `NamedKey`: scenario files should read like the key
/// caps a human presses (`"Esc"`, `"F6"`), not like a Rust enum literal
/// (`{"F": 6}`).
pub fn parse_named_key(s: &str) -> Result<NamedKey, String> {
    let key = match s {
        "Escape" | "Esc" => NamedKey::Escape,
        "Tab" => NamedKey::Tab,
        "BackTab" | "ShiftTab" => NamedKey::BackTab,
        "Enter" | "Return" => NamedKey::Enter,
        "Backspace" => NamedKey::Backspace,
        "Delete" | "Del" => NamedKey::Delete,
        "Insert" => NamedKey::Insert,
        "Home" => NamedKey::Home,
        "End" => NamedKey::End,
        "PageUp" => NamedKey::PageUp,
        "PageDown" => NamedKey::PageDown,
        "Up" => NamedKey::Up,
        "Down" => NamedKey::Down,
        "Left" => NamedKey::Left,
        "Right" => NamedKey::Right,
        "CapsLock" => NamedKey::CapsLock,
        "NumLock" => NamedKey::NumLock,
        "ScrollLock" => NamedKey::ScrollLock,
        "Menu" => NamedKey::Menu,
        other => {
            let n = other
                .strip_prefix('F')
                .and_then(|d| d.parse::<u8>().ok())
                .filter(|n| (1..=24).contains(n))
                .ok_or_else(|| format!("unknown key name {other:?} in a `press` step"))?;
            NamedKey::F(n)
        }
    };
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_audit_worked_example_verbatim() {
        // §6.5 example (1), byte-for-byte from docs/SMELL_AUDIT_2026-07.md.
        let src = r#"{
            "id": "pipeline.click_advances_stage",
            "fixture": "pipeline_app",
            "tier": 1,
            "viewport": { "cols": 100, "rows": 30 },
            "steps": [
                { "assert_screen_has": "stage 1" },
                { "press": "Right" },
                { "click_text": "Go" },
                { "assert_screen_has": "stage 3" },
                { "type_char": "q" },
                { "assert_exited": true }
            ]
        }"#;
        let s = Scenario::from_json("inline", src).expect("audit example must parse");
        assert_eq!(s.id, "pipeline.click_advances_stage");
        assert_eq!(s.fixture, "pipeline_app");
        assert_eq!(s.tier, 1);
        assert_eq!(
            s.viewport,
            ViewportSpec {
                cols: 100,
                rows: 30
            }
        );
        assert!(s.requires.is_empty(), "`requires` defaults to empty");
        assert_eq!(s.steps.len(), 6);
        assert_eq!(s.steps[1], Step::Press("Right".into()));
        assert_eq!(s.steps[4], Step::TypeChar('q'));
        assert_eq!(s.steps[5], Step::AssertExited(true));
    }

    #[test]
    fn parses_struct_variant_steps_and_requires() {
        let src = r#"{
            "id": "panel.drag_select_copy",
            "fixture": "panel_app",
            "tier": 1,
            "viewport": { "cols": 100, "rows": 30 },
            "requires": ["text_selection"],
            "steps": [
                { "note": "drag across two lines, then copy" },
                { "drag_text": { "from": "alpha", "to": "gamma" } },
                { "ctrl_char": "c" },
                { "assert_left_of": { "a": "EXPLORER", "b": "main" } },
                { "assert_above": { "a": "EXPLORER", "b": "status" } },
                { "assert_inside": { "a": "content", "zone": "app-shell:sidebar" } },
                { "assert_count": { "text": "row", "count": 3 } },
                { "scroll_at": { "target": "row", "lines": -3 } },
                { "click_text_at": { "text": "Name", "anchor": "right_edge" } }
            ]
        }"#;
        let s = Scenario::from_json("inline", src).expect("must parse");
        assert_eq!(s.requires, vec!["text_selection".to_string()]);
        assert_eq!(
            s.steps[1],
            Step::DragText {
                from: "alpha".into(),
                to: "gamma".into()
            }
        );
        assert_eq!(
            s.steps[8],
            Step::ClickTextAt {
                text: "Name".into(),
                anchor: AnchorSpec::RightEdge
            }
        );
    }

    /// `click_text_at`'s `anchor` key is optional; omitting it falls back
    /// to `AnchorSpec::Center` (`#[default]`) — the same target
    /// `click_text` always hits. This is what makes the `Default` derive
    /// on `AnchorSpec` load-bearing rather than dead code.
    #[test]
    fn click_text_at_anchor_defaults_to_center_when_omitted() {
        let src = r#"{
            "id": "x",
            "fixture": "panel_app",
            "tier": 1,
            "viewport": { "cols": 100, "rows": 30 },
            "steps": [
                { "click_text_at": { "text": "Name" } }
            ]
        }"#;
        let s = Scenario::from_json("inline", src).expect("anchor-less click_text_at must parse");
        assert_eq!(
            s.steps[0],
            Step::ClickTextAt {
                text: "Name".into(),
                anchor: AnchorSpec::Center
            }
        );
    }

    /// The headline invariant of schema v0: a coordinate literal cannot be
    /// written down. This is the executable form of "hardcoded coordinates
    /// are structurally impossible".
    #[test]
    fn numeric_coordinate_steps_are_unrepresentable() {
        for coord_step in [
            r#"{ "click_at": { "x": 12, "y": 3 } }"#,
            r#"{ "click": [12, 3] }"#,
            r#"{ "drag": { "x0": 1, "y0": 2, "x1": 3, "y1": 4 } }"#,
            r#"{ "click_text": "Go", "x": 12 }"#,
            r#"{ "assert_at": { "text": "Go", "row": 4 } }"#,
        ] {
            let src = format!(
                r#"{{ "id": "x", "fixture": "pipeline_app", "tier": 1,
                      "viewport": {{ "cols": 10, "rows": 10 }},
                      "steps": [ {coord_step} ] }}"#
            );
            assert!(
                Scenario::from_json("inline", &src).is_err(),
                "coordinate-bearing step must not deserialise: {coord_step}"
            );
        }
    }

    #[test]
    fn unknown_top_level_keys_are_rejected() {
        let src = r#"{
            "id": "x", "fixture": "pipeline_app", "tier": 1,
            "viewport": { "cols": 10, "rows": 10 },
            "origin": { "x": 0, "y": 0 },
            "steps": []
        }"#;
        assert!(Scenario::from_json("inline", src).is_err());
    }

    #[test]
    fn named_keys_read_like_key_caps() {
        assert_eq!(parse_named_key("Right").unwrap(), NamedKey::Right);
        assert_eq!(parse_named_key("Esc").unwrap(), NamedKey::Escape);
        assert_eq!(parse_named_key("Escape").unwrap(), NamedKey::Escape);
        assert_eq!(parse_named_key("F6").unwrap(), NamedKey::F(6));
        assert_eq!(parse_named_key("F24").unwrap(), NamedKey::F(24));
        assert!(parse_named_key("F0").is_err());
        assert!(parse_named_key("F25").is_err());
        assert!(parse_named_key("Wiggle").is_err());
    }
}
