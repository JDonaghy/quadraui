//! Decodes `run_shell`'s `config` dict — panel/bottom-panel lists
//! ([`panel_list_from_dict`], [`bottom_panel_config_from_value`]) plus
//! the top-level [`ShellConfig`] builder surface
//! ([`shell_config_from_value`]) itself.

use serde_json::Value;

use quadraui::{
    BottomPanelConfig, BottomPanelTab, PanelDefinition, ShellConfig, ShellPosition, WidgetId,
};

use crate::DecodeError;

use super::value::{
    check_unknown_keys, dict_bool, dict_f32, dict_required_str, dict_str, obj_get,
    DecodeDiagnostics, StaticTabContent,
};

/// Decodes one panel dict (`{"id", "icon", "tooltip", "title"}`, all but
/// `"id"` optional) into a [`PanelDefinition`] — one entry of `run_shell`'s
/// `"panels"`/`"bottom_items"` lists.
fn panel_definition_from_value(
    what: &str,
    value: &Value,
    diagnostics: &DecodeDiagnostics,
) -> Result<PanelDefinition, DecodeError> {
    check_unknown_keys(
        diagnostics,
        what,
        "panel",
        value,
        &["id", "icon", "tooltip", "title"],
    )?;
    let id = dict_required_str(what, value, "id")?;
    let icon = dict_str(what, value, "icon")?.unwrap_or_default();
    let tooltip = dict_str(what, value, "tooltip")?.unwrap_or_default();
    let title = dict_str(what, value, "title")?.unwrap_or_else(|| id.clone());
    Ok(PanelDefinition {
        id: WidgetId::new(id),
        icon,
        tooltip,
        title,
    })
}

/// Decodes a `list` of panel dicts via [`panel_definition_from_value`].
fn panel_list_from_value(
    what: &str,
    value: &Value,
    diagnostics: &DecodeDiagnostics,
) -> Result<Vec<PanelDefinition>, DecodeError> {
    let list = value
        .as_array()
        .ok_or_else(|| DecodeError::type_mismatch(what, "panels", "an array"))?;
    list.iter()
        .enumerate()
        .map(|(index, item)| {
            if !item.is_object() {
                return Err(DecodeError::type_mismatch(
                    what,
                    format!("panels[{index}]"),
                    "an object",
                ));
            }
            panel_definition_from_value(&format!("{what} › panels[{index}]"), item, diagnostics)
        })
        .collect()
}

/// [`panel_list_from_value`] applied to `value[key]`, defaulting to an
/// empty list when `key` is absent or `null`.
fn panel_list_from_dict(
    what: &str,
    value: &Value,
    key: &str,
    diagnostics: &DecodeDiagnostics,
) -> Result<Vec<PanelDefinition>, DecodeError> {
    match obj_get(value, key) {
        Some(v) => panel_list_from_value(what, v, diagnostics),
        None => Ok(Vec::new()),
    }
}

/// Decodes one bottom-panel-tab dict (`{"id", "label", "closable",
/// "badge"}`, all but `"id"` optional) into a [`BottomPanelTab`], with its
/// content set to a [`StaticTabContent`] naming the tab's own id.
fn bottom_panel_tab_from_value(
    what: &str,
    value: &Value,
    diagnostics: &DecodeDiagnostics,
) -> Result<BottomPanelTab, DecodeError> {
    check_unknown_keys(
        diagnostics,
        what,
        "bottom_panel tab",
        value,
        &["id", "label", "closable", "badge"],
    )?;
    let id = dict_required_str(what, value, "id")?;
    let label = dict_str(what, value, "label")?.unwrap_or_else(|| id.clone());
    let closable = dict_bool(what, value, "closable")?.unwrap_or(false);
    let badge = dict_str(what, value, "badge")?;
    Ok(BottomPanelTab {
        content: Box::new(StaticTabContent(id.clone())),
        id,
        label,
        closable,
        badge,
    })
}

/// Decodes `run_shell`'s `"bottom_panel"` dict (`{"tabs", "active_tab_id",
/// "maximised", "height_fraction"}`, all optional except at least one tab)
/// into a [`BottomPanelConfig`]. `"active_tab_id"` falls back to the first
/// tab's id, mirroring [`BottomPanelConfig::active_tab_id`]'s own
/// "unknown id falls back to the first tab" contract.
fn bottom_panel_config_from_value(
    what: &str,
    value: &Value,
    diagnostics: &DecodeDiagnostics,
) -> Result<BottomPanelConfig, DecodeError> {
    check_unknown_keys(
        diagnostics,
        what,
        "bottom_panel",
        value,
        &["tabs", "active_tab_id", "maximised", "height_fraction"],
    )?;
    let tabs = match obj_get(value, "tabs") {
        Some(v) => {
            let list = v
                .as_array()
                .ok_or_else(|| DecodeError::type_mismatch(what, "bottom_panel.tabs", "an array"))?;
            list.iter()
                .enumerate()
                .map(|(index, item)| {
                    if !item.is_object() {
                        return Err(DecodeError::type_mismatch(
                            what,
                            format!("bottom_panel.tabs[{index}]"),
                            "an object",
                        ));
                    }
                    bottom_panel_tab_from_value(
                        &format!("{what} › bottom_panel.tabs[{index}]"),
                        item,
                        diagnostics,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?
        }
        None => Vec::new(),
    };
    let active_tab_id = dict_str(what, value, "active_tab_id")?
        .or_else(|| tabs.first().map(|tab| tab.id.clone()))
        .unwrap_or_default();
    let maximised = dict_bool(what, value, "maximised")?.unwrap_or(false);
    let height_fraction = dict_f32(what, value, "height_fraction")?.unwrap_or(0.3);
    Ok(BottomPanelConfig {
        tabs,
        active_tab_id,
        maximised,
        height_fraction,
    })
}

/// Decodes `run_shell`'s `config` dict into a [`ShellConfig`]. There is no
/// `Deserialize` impl on `ShellConfig`/`PanelDefinition`/`BottomPanelConfig`
/// upstream, so this reads the subset of `ShellConfig`'s builder surface a
/// binding supports field-by-field, the same `*_from_value` style every
/// other decoder in this module uses, rather than depending on one.
/// Recognized keys: `"title"` (required), `"panels"`, `"bottom_items"`,
/// `"position"` (`"left"`/`"right"`), `"sidebar_visible"`,
/// `"default_sidebar_width"`, `"min_sidebar_width"`, `"max_sidebar_width"`,
/// `"activity_bar_width"`, `"has_title_bar"` + `"title_bar_height_lh"`,
/// `"has_status_bar"`, `"has_command_line"`, `"has_bottom_panel"` +
/// `"bottom_panel_height_lh"` (the bare, untabbed region), and
/// `"bottom_panel"` (a tabbed [`BottomPanelConfig`], see
/// [`bottom_panel_config_from_value`]). Any other key is reported through
/// `diagnostics` (warn once per session, or a hard error under `strict`)
/// rather than silently ignored.
pub(super) fn shell_config_from_value(
    value: &Value,
    diagnostics: &DecodeDiagnostics,
) -> Result<ShellConfig, DecodeError> {
    let what = "run_shell";
    check_unknown_keys(
        diagnostics,
        what,
        "run_shell config",
        value,
        &[
            "title",
            "panels",
            "bottom_items",
            "position",
            "sidebar_visible",
            "default_sidebar_width",
            "min_sidebar_width",
            "max_sidebar_width",
            "activity_bar_width",
            "has_title_bar",
            "title_bar_height_lh",
            "has_status_bar",
            "has_command_line",
            "has_bottom_panel",
            "bottom_panel_height_lh",
            "bottom_panel",
        ],
    )?;
    let title = dict_required_str(what, value, "title")?;
    let panels = panel_list_from_dict(what, value, "panels", diagnostics)?;
    let mut config = ShellConfig::new(title, panels);

    let bottom_items = panel_list_from_dict(what, value, "bottom_items", diagnostics)?;
    if !bottom_items.is_empty() {
        config = config.with_bottom_items(bottom_items);
    }

    if let Some(position) = dict_str(what, value, "position")? {
        let position = match position.as_str() {
            "left" => ShellPosition::Left,
            "right" => ShellPosition::Right,
            other => {
                return Err(DecodeError::unknown_variant(
                    what,
                    "position",
                    other,
                    &["left", "right"],
                ));
            }
        };
        config = config.with_position(position);
    }

    if let Some(visible) = dict_bool(what, value, "sidebar_visible")? {
        config = config.with_sidebar_visible(visible);
    }
    if let Some(width) = dict_f32(what, value, "default_sidebar_width")? {
        config.default_sidebar_width = width;
    }
    if let Some(width) = dict_f32(what, value, "min_sidebar_width")? {
        config.min_sidebar_width = width;
    }
    if let Some(width) = dict_f32(what, value, "max_sidebar_width")? {
        config.max_sidebar_width = width;
    }
    if let Some(width) = dict_f32(what, value, "activity_bar_width")? {
        config = config.with_activity_bar_width(width);
    }
    if dict_bool(what, value, "has_title_bar")?.unwrap_or(false) {
        let height = dict_f32(what, value, "title_bar_height_lh")?.unwrap_or(1.5);
        config = config.with_title_bar(height);
    }
    if dict_bool(what, value, "has_status_bar")?.unwrap_or(false) {
        config = config.with_status_bar();
    }
    if dict_bool(what, value, "has_command_line")?.unwrap_or(false) {
        config = config.with_command_line();
    }
    if dict_bool(what, value, "has_bottom_panel")?.unwrap_or(false) {
        let height = dict_f32(what, value, "bottom_panel_height_lh")?.unwrap_or(10.0);
        config = config.with_bottom_panel(height);
    }
    if let Some(bp_value) = obj_get(value, "bottom_panel") {
        if !bp_value.is_object() {
            return Err(DecodeError::type_mismatch(
                what,
                "bottom_panel",
                "an object",
            ));
        }
        config = config.with_bottom_panel_config(bottom_panel_config_from_value(
            what,
            bp_value,
            diagnostics,
        )?);
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use crate::wire::decode_shell_config;
    use crate::wire::test_support::err;

    use super::*;

    /// `run_shell`'s `config` dict applies its documented defaults
    /// (`height_fraction` 0.3, `title_bar_height_lh` 1.5,
    /// `bottom_panel_height_lh` 10.0) when a flag is set but its paired
    /// height isn't given.
    #[test]
    fn decode_shell_config_applies_documented_defaults() {
        let value = json!({
            "title": "My Shell",
            "has_title_bar": true,
            "has_bottom_panel": true,
        });
        let config =
            decode_shell_config(&value, &DecodeDiagnostics::warn()).expect("valid shell config");
        assert_eq!(config.title, "My Shell");
        assert!(config.has_title_bar);
        assert_eq!(config.title_bar_height_lh, 1.5);
        assert!(config.has_bottom_panel);
        assert_eq!(config.bottom_panel_height_lh, 10.0);
    }

    /// `run_shell`'s `"position"` key accepts `"left"`/`"right"` and
    /// rejects anything else.
    #[test]
    fn decode_shell_config_validates_position() {
        let value = json!({"title": "My Shell", "position": "right"});
        let config =
            decode_shell_config(&value, &DecodeDiagnostics::warn()).expect("valid position");
        assert_eq!(config.position, ShellPosition::Right);

        let value = json!({"title": "My Shell", "position": "sideways"});
        let err = err(decode_shell_config(&value, &DecodeDiagnostics::warn()));
        assert!(matches!(err, DecodeError::UnknownVariant { .. }), "{err:?}");
        assert!(
            err.to_string().contains("\"sideways\""),
            "{}",
            err.to_string()
        );
    }

    /// `run_shell`'s tabbed `"bottom_panel"` dict applies its own
    /// documented default `height_fraction` (0.3) and falls back
    /// `"active_tab_id"` to the first tab when omitted.
    #[test]
    fn decode_shell_config_bottom_panel_defaults() {
        let value = json!({
            "title": "My Shell",
            "bottom_panel": {
                "tabs": [{"id": "t1"}, {"id": "t2"}],
            },
        });
        let config =
            decode_shell_config(&value, &DecodeDiagnostics::warn()).expect("valid bottom_panel");
        let bottom_panel = config.bottom_panel.expect("bottom_panel is set");
        assert_eq!(bottom_panel.active_tab_id, "t1");
        assert_eq!(bottom_panel.height_fraction, 0.3);
        assert_eq!(bottom_panel.tabs.len(), 2);
    }

    /// `run_shell`'s `config` dict requires `"title"`.
    #[test]
    fn decode_shell_config_requires_title() {
        let err = err(decode_shell_config(json!({}), &DecodeDiagnostics::warn()));
        assert!(matches!(err, DecodeError::MissingField { .. }), "{err:?}");
        assert!(err.to_string().contains("\"title\""), "{}", err.to_string());
    }

    /// `run_shell`'s own config dict gets the same warn/strict policy as a
    /// layout-tree node dict.
    #[test]
    fn decode_shell_config_warns_then_rejects_an_unknown_key() {
        let value = json!({"title": "App", "bottom_panel_heigth": 5.0});

        let warn = DecodeDiagnostics::warn();
        decode_shell_config(&value, &warn).expect("warn mode still decodes");
        assert_eq!(
            warn.warned_keys(),
            HashSet::from(["bottom_panel_heigth".to_string()])
        );

        let strict = DecodeDiagnostics::strict();
        let error = err(decode_shell_config(&value, &strict));
        assert_eq!(error.code(), "unknown_key");
    }
}
