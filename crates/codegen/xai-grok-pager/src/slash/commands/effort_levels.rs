//! Shared reasoning-effort dropdown levels for `/model` and `/effort`.
//!
//! Built-in menu includes multi-agent orchestration modes (Heavy / Agent Swarm /
//! Swarm Heavy) above High. All multi-agent options wire as `xhigh` and are
//! distinguished by option id for UI + protocol injection.

use xai_grok_shell::sampling::types::{
    OrchestrationMode, ReasoningEffort, ReasoningEffortOption, enhanced_legacy_effort_options,
};

use crate::slash::command::ArgItem;

/// Effort levels in the built-in single-agent fallback (strongest first).
/// Multi-agent modes are layered on top via [`legacy_effort_options`].
#[cfg(test)]
pub(crate) const EFFORT_LEVELS: &[ReasoningEffort] = &[
    ReasoningEffort::Xhigh,
    ReasoningEffort::High,
    ReasoningEffort::Medium,
    ReasoningEffort::Low,
];

/// The built-in menu used when the server sends no `reasoningEfforts`.
/// Includes Swarm Heavy / Agent Swarm / Heavy above the classic low..xhigh rows.
pub(crate) fn legacy_effort_options() -> Vec<ReasoningEffortOption> {
    enhanced_legacy_effort_options()
}

/// Display label for a selected effort option id (or wire value fallback).
pub(crate) fn effort_display_label(
    option_id: Option<&str>,
    effort: Option<ReasoningEffort>,
) -> Option<String> {
    if let Some(id) = option_id {
        let mode = OrchestrationMode::from_option_id(id);
        if mode.is_multi_agent() {
            return Some(mode.label().to_string());
        }
        if let Some(opt) = legacy_effort_options()
            .into_iter()
            .find(|o| o.id.eq_ignore_ascii_case(id))
        {
            return Some(opt.label);
        }
        return Some(id.to_string());
    }
    effort.map(|e| e.to_string())
}

/// Build effort rows for autocomplete from a per-model option list.
///
/// - `mark_active` + `current_effort` mark the current session effort with `(active)`.
/// - `insert_text_for` controls what is inserted on select:
///   - `/effort`: the option id (`"swarm-heavy"`)
///   - `/model` chained phase: `"ModelName swarm-heavy"`
///
/// `match_text` gets an `a `/`b `/…` sort prefix so the matcher's alphabetical
/// tiebreak preserves the option order.
pub(crate) fn build_effort_arg_items(
    options: &[ReasoningEffortOption],
    current_effort: Option<ReasoningEffort>,
    mark_active: bool,
    insert_text_for: impl Fn(&ReasoningEffortOption) -> String,
) -> Vec<ArgItem> {
    build_effort_arg_items_with_option_id(options, current_effort, None, mark_active, insert_text_for)
}

/// Like [`build_effort_arg_items`] but prefers matching the active row by
/// option id so Heavy / Swarm / Swarm Heavy (all wire-xhigh) show correctly.
pub(crate) fn build_effort_arg_items_with_option_id(
    options: &[ReasoningEffortOption],
    current_effort: Option<ReasoningEffort>,
    current_option_id: Option<&str>,
    mark_active: bool,
    insert_text_for: impl Fn(&ReasoningEffortOption) -> String,
) -> Vec<ArgItem> {
    options
        .iter()
        .enumerate()
        .map(|(idx, option)| {
            let active = mark_active
                && if let Some(id) = current_option_id {
                    option.id.eq_ignore_ascii_case(id)
                } else {
                    current_effort == Some(option.value)
                        && !OrchestrationMode::from_option_id(&option.id).is_multi_agent()
                };
            let active_suffix = if active { " (active)" } else { "" };
            let insert_text = insert_text_for(option);
            let sort_prefix = char::from(b'a' + idx as u8);
            // Multi-agent rows get a brand glyph so the dropdown feels distinct
            // from plain high/medium (and pairs with colored hover styles).
            // mark() is glyph-only; option.label already carries "Heavy" / etc.
            let mode = OrchestrationMode::from_option_id(&option.id);
            let label = if mode.is_multi_agent() {
                format!("{} {}{active_suffix}", mode.mark(), option.label)
            } else {
                format!("{}{active_suffix}", option.label)
            };
            ArgItem {
                display: label,
                match_text: format!("{sort_prefix} {insert_text}"),
                insert_text,
                description: option.description.clone().unwrap_or_default(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_menu_includes_multi_agent_modes() {
        let opts = legacy_effort_options();
        let ids: Vec<_> = opts.iter().map(|o| o.id.as_str()).collect();
        assert!(ids.contains(&"swarm-heavy"));
        assert!(ids.contains(&"swarm"));
        assert!(ids.contains(&"heavy"));
        assert!(ids.contains(&"xhigh"));
        assert!(ids.contains(&"high"));
    }

    #[test]
    fn display_label_prefers_multi_agent_names() {
        assert_eq!(
            effort_display_label(Some("swarm-heavy"), Some(ReasoningEffort::Xhigh)).as_deref(),
            Some("Swarm Heavy")
        );
        assert_eq!(
            effort_display_label(Some("heavy"), Some(ReasoningEffort::Xhigh)).as_deref(),
            Some("Heavy")
        );
        assert_eq!(
            effort_display_label(Some("xhigh"), Some(ReasoningEffort::Xhigh)).as_deref(),
            Some("xhigh")
        );
    }

    #[test]
    fn active_row_uses_option_id_for_xhigh_aliases() {
        let opts = legacy_effort_options();
        let items = build_effort_arg_items_with_option_id(
            &opts,
            Some(ReasoningEffort::Xhigh),
            Some("swarm"),
            true,
            |o| o.id.clone(),
        );
        let swarm = items.iter().find(|i| i.insert_text == "swarm").unwrap();
        assert!(swarm.display.contains("(active)"), "{}", swarm.display);
        let xhigh = items.iter().find(|i| i.insert_text == "xhigh").unwrap();
        assert!(!xhigh.display.contains("(active)"), "{}", xhigh.display);
    }

    #[test]
    fn multi_agent_rows_do_not_duplicate_mode_name() {
        let opts = legacy_effort_options();
        let items = build_effort_arg_items_with_option_id(
            &opts,
            Some(ReasoningEffort::Xhigh),
            Some("heavy"),
            true,
            |o| o.id.clone(),
        );
        let heavy = items.iter().find(|i| i.insert_text == "heavy").unwrap();
        // Glyph + single label — never "◈ HEAVY Heavy" / "⬡ SWARM Agent Swarm".
        assert_eq!(heavy.display, "◈ Heavy (active)", "{}", heavy.display);
        let swarm = items.iter().find(|i| i.insert_text == "swarm").unwrap();
        assert_eq!(swarm.display, "⬡ Agent Swarm", "{}", swarm.display);
        let sh = items
            .iter()
            .find(|i| i.insert_text == "swarm-heavy")
            .unwrap();
        assert_eq!(sh.display, "⬢ Swarm Heavy", "{}", sh.display);
        assert!(
            !heavy.display.contains("HEAVY Heavy")
                && !swarm.display.contains("SWARM Agent")
                && !sh.display.contains("HEAVY Swarm"),
            "duplicate brand text leaked into dropdown: heavy={}, swarm={}, sh={}",
            heavy.display,
            swarm.display,
            sh.display
        );
    }
}
