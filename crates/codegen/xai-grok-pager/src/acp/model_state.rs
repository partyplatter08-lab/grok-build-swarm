//! Model state — tracks available models and current selection.

use agent_client_protocol as acp;
use indexmap::IndexMap;
use xai_grok_shell::sampling::types::{
    ORCHESTRATION_MODE_META_KEY, OrchestrationMode, ReasoningEffort, ReasoningEffortOption,
    merge_multi_agent_effort_options, parse_reasoning_effort_meta, parse_reasoning_efforts_meta,
    supports_reasoning_effort_meta,
};

use crate::slash::commands::effort_levels::{effort_display_label, legacy_effort_options};

/// Why an effort token could not be applied to a model. Shared by every effort
/// surface (`/effort`, the CLI deferred switch, and headless) so they classify
/// the same input identically and differ only in how they surface the error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EffortTokenError {
    /// The target model does not advertise `supportsReasoningEffort`.
    Unsupported,
    /// The token is neither a menu id nor a canonical value offered by this
    /// model's menu. `offered` is the model-specific list of option ids the
    /// user can type (never a hardcoded global set — so we do not advertise
    /// `none`/`minimal` when the model does not offer them).
    UnknownToken { token: String, offered: Vec<String> },
    /// No active model to resolve the effort against.
    NoActiveModel,
}

impl EffortTokenError {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Unsupported => "current model does not support reasoning effort".to_string(),
            Self::UnknownToken { token, offered } => {
                if offered.is_empty() {
                    format!(
                        "unknown effort level '{token}'; this model has no selectable effort levels"
                    )
                } else {
                    format!(
                        "unknown effort level '{token}'; use one of: {}",
                        offered.join(", ")
                    )
                }
            }
            Self::NoActiveModel => "no active model to apply effort to".to_string(),
        }
    }
}

/// Per-agent model state.
#[derive(Debug, Clone, Default)]
pub struct ModelState {
    pub available: IndexMap<acp::ModelId, acp::ModelInfo>,
    pub current: Option<acp::ModelId>,
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Selected effort menu option id (e.g. `"swarm-heavy"`, `"high"`).
    /// Distinguishes multi-agent modes that share the same wire value (`xhigh`).
    pub reasoning_effort_option_id: Option<String>,
    /// External override for the context window size (tokens).
    /// When set, `get_context_window()` returns this instead of
    /// reading from the current model's metadata. Used for subagent
    /// views where SubagentProgress reports the actual window size.
    context_window_override: Option<u64>,
}

impl ModelState {
    pub fn is_empty(&self) -> bool {
        self.available.is_empty()
    }

    /// Display name for the current model.
    pub fn current_model_name(&self) -> Option<String> {
        let current = self.current.as_ref()?;
        if let Some(model_info) = self.available.get(current) {
            Some(model_info.name.clone())
        } else {
            Some(current.0.to_string())
        }
    }

    /// Machine-readable model ID string for the current model (e.g. "grok-4.5").
    pub fn current_model_id_str(&self) -> Option<&str> {
        Some(self.current.as_ref()?.0.as_ref())
    }

    /// Total context window tokens for the current model (if available).
    fn current_context_window_tokens(&self) -> Option<u64> {
        let meta = self.available.get(self.current.as_ref()?)?.meta.as_ref()?;
        meta.get("totalContextTokens")
            .and_then(|value| match value {
                serde_json::Value::Number(number) => number.as_u64(),
                _ => None,
            })
    }

    /// Whether the current model accepts image input, read from the model's
    /// `meta` (the ACP extension point — same source as `totalContextTokens`).
    ///
    /// Honors an explicit `acceptsImages` bool, else an `inputModalities` array
    /// containing `"image"`. DEFAULTS TO `true` when neither key is present:
    /// correct today (all current Grok models accept images, so nothing is
    /// suppressed) and forward-compatible (suppresses non-vision models once the
    /// ACP server populates the key). Populating that key server-side is a
    /// separate change.
    pub fn current_model_accepts_images(&self) -> bool {
        let Some(meta) = self
            .current
            .as_ref()
            .and_then(|id| self.available.get(id))
            .and_then(|info| info.meta.as_ref())
        else {
            return true;
        };
        if let Some(accepts) = meta.get("acceptsImages").and_then(|v| v.as_bool()) {
            return accepts;
        }
        if let Some(modalities) = meta.get("inputModalities").and_then(|v| v.as_array()) {
            return modalities
                .iter()
                .any(|m| m.as_str().is_some_and(|s| s.eq_ignore_ascii_case("image")));
        }
        true
    }

    /// Get the effective context window size (tokens).
    ///
    /// Returns the override if set, otherwise reads from the current model's
    /// metadata. The override is set by `override_context_window()` when an
    /// external source (e.g., SubagentProgress) reports the actual window size.
    pub fn get_context_window(&self) -> Option<u64> {
        self.context_window_override
            .or_else(|| self.current_context_window_tokens())
    }

    /// Override the context window size.
    ///
    /// Used for subagent views where the actual context window is reported
    /// via SubagentProgress and may differ from the inherited model's metadata.
    pub fn override_context_window(&mut self, tokens: u64) {
        self.context_window_override = Some(tokens);
    }

    /// Replace the available models, preserving current selection if still valid.
    pub fn update_catalog(
        &mut self,
        new_available: IndexMap<acp::ModelId, acp::ModelInfo>,
        fallback_current: Option<acp::ModelId>,
    ) {
        let previous_current_model = self.current.clone();
        self.available = new_available;
        if let Some(ref id) = self.current {
            if !self.available.contains_key(id) {
                self.current = fallback_current;
            }
        } else {
            self.current = fallback_current;
        }
        // The models/update broadcast carries each model's static default effort,
        // not this session's choice; only re-derive when the model changed so a
        // catalog refresh can't clobber a user-set effort.
        if self.current != previous_current_model {
            self.reasoning_effort = self
                .current
                .as_ref()
                .and_then(|id| self.available.get(id))
                .and_then(|info| parse_reasoning_effort_meta(info.meta.as_ref()));
        }
    }

    /// Set the current model and resolve reasoning effort from catalog meta.
    pub fn set_current(
        &mut self,
        model_id: acp::ModelId,
        effort_override: Option<ReasoningEffort>,
    ) {
        self.set_current_with_option(model_id, effort_override, None);
    }

    /// Set the current model, effort, and optional menu option id.
    pub fn set_current_with_option(
        &mut self,
        model_id: acp::ModelId,
        effort_override: Option<ReasoningEffort>,
        option_id: Option<String>,
    ) {
        self.current = Some(model_id.clone());
        self.reasoning_effort = effort_override.or_else(|| {
            self.available
                .get(&model_id)
                .and_then(|info| parse_reasoning_effort_meta(info.meta.as_ref()))
        });
        if let Some(id) = option_id {
            self.reasoning_effort_option_id = Some(id);
        } else if let Some(effort) = self.reasoning_effort {
            // Preserve multi-agent option ids (heavy/swarm/swarm-heavy) when a
            // ModelChanged / set_current only carries the wire effort (xhigh).
            // Without this, the footer silently reverts to "xhigh" after send.
            let keep = self.reasoning_effort_option_id.as_ref().is_some_and(|id| {
                let mode = OrchestrationMode::from_option_id(id);
                if mode.is_multi_agent() {
                    return mode.wire_effort() == effort;
                }
                self.reasoning_effort_options_for(&model_id)
                    .iter()
                    .any(|o| o.id.eq_ignore_ascii_case(id) && o.value == effort)
            });
            if !keep {
                self.reasoning_effort_option_id = Some(effort.as_str().to_string());
            }
        } else {
            self.reasoning_effort_option_id = None;
        }
    }

    /// Active orchestration mode derived from the selected option id.
    pub fn orchestration_mode(&self) -> OrchestrationMode {
        self.reasoning_effort_option_id
            .as_deref()
            .map(OrchestrationMode::from_option_id)
            .unwrap_or(OrchestrationMode::Normal)
    }

    /// Human-facing effort label for footer / welcome / toasts.
    pub fn effort_display_label(&self) -> Option<String> {
        effort_display_label(
            self.reasoning_effort_option_id.as_deref(),
            self.reasoning_effort,
        )
    }

    /// The reasoning-effort menu for the current model. Gate-first: an unset or
    /// unsupported model yields no menu; a supported model uses the server list
    /// when present, else the built-in fallback.
    pub fn reasoning_effort_options(&self) -> Vec<ReasoningEffortOption> {
        match self.current.as_ref() {
            Some(id) => self.reasoning_effort_options_for(id),
            None => Vec::new(),
        }
    }

    /// Menu for a specific catalog model id (used by `/model`'s effort phase).
    /// `parse_reasoning_efforts_meta` returns `None` for absent, non-array, or
    /// present-but-unusable lists, so all of those fall back to the built-in menu
    /// exactly as the shell's session picker does.
    pub(crate) fn reasoning_effort_options_for(
        &self,
        id: &acp::ModelId,
    ) -> Vec<ReasoningEffortOption> {
        let Some(info) = self.available.get(id) else {
            return Vec::new();
        };
        if !supports_reasoning_effort_meta(info.meta.as_ref()) {
            return Vec::new();
        }
        match parse_reasoning_efforts_meta(info.meta.as_ref()) {
            Some(server) => merge_multi_agent_effort_options(server),
            None => legacy_effort_options(),
        }
    }

    /// Resolve a token to (wire effort, option id) for the current model.
    pub fn resolve_effort_token_with_id(
        &self,
        token: &str,
    ) -> Option<(ReasoningEffort, String)> {
        match self.current.as_ref() {
            Some(id) => self.resolve_effort_token_with_id_for(id, token),
            None => token
                .parse::<ReasoningEffort>()
                .ok()
                .map(|e| (e, e.as_str().to_string()))
                .or_else(|| {
                    let mode = OrchestrationMode::from_option_id(token);
                    mode.option_id()
                        .map(|id| (mode.wire_effort(), id.to_string()))
                }),
        }
    }

    pub(crate) fn resolve_effort_token_with_id_for(
        &self,
        id: &acp::ModelId,
        token: &str,
    ) -> Option<(ReasoningEffort, String)> {
        let options = self.reasoning_effort_options_for(id);
        if let Some(option) = options
            .iter()
            .find(|opt| opt.id.eq_ignore_ascii_case(token))
        {
            return Some((option.value, option.id.clone()));
        }
        let parsed = token.parse::<ReasoningEffort>().ok()?;
        options
            .iter()
            .find(|opt| {
                opt.value == parsed
                    && !OrchestrationMode::from_option_id(&opt.id).is_multi_agent()
            })
            .or_else(|| options.iter().find(|opt| opt.value == parsed))
            .map(|o| (o.value, o.id.clone()))
    }

    /// Map a typed/selected effort token to its canonical value for the current
    /// model. Accepts a menu option id (case-insensitive) or a canonical level
    /// that appears as a **value** in that model's menu. Levels the model does
    /// not offer (e.g. `none` on grok-4.5) are rejected so we fail in the TUI
    /// instead of sending a blocked effort to the API.
    pub fn resolve_effort_token(&self, token: &str) -> Option<ReasoningEffort> {
        self.resolve_effort_token_with_id(token).map(|(e, _)| e)
    }

    /// [`Self::resolve_effort_token`] scoped to a specific catalog model id.
    pub(crate) fn resolve_effort_token_for(
        &self,
        id: &acp::ModelId,
        token: &str,
    ) -> Option<ReasoningEffort> {
        self.resolve_effort_token_with_id_for(id, token)
            .map(|(e, _)| e)
    }

    /// Canonical effort-token policy: gate on the model's support flag first,
    /// then resolve the token (menu id or canonical level). This is the single
    /// decision shared by `/effort`, the CLI deferred switch, and headless —
    /// each caller only maps the [`EffortTokenError`] to its own surface.
    pub(crate) fn resolve_effort_for_model(
        &self,
        id: &acp::ModelId,
        token: &str,
    ) -> Result<ReasoningEffort, EffortTokenError> {
        let supports = self
            .available
            .get(id)
            .map(|info| supports_reasoning_effort_meta(info.meta.as_ref()))
            .unwrap_or(false);
        if !supports {
            return Err(EffortTokenError::Unsupported);
        }
        self.resolve_effort_token_for(id, token)
            .ok_or_else(|| EffortTokenError::UnknownToken {
                token: token.to_string(),
                // Menu option ids only — matches `/effort` autocomplete and
                // never invents levels (none/minimal/…) the model does not offer.
                offered: self
                    .reasoning_effort_options_for(id)
                    .into_iter()
                    .map(|opt| opt.id)
                    .collect(),
            })
    }

    /// Resolve a user-supplied name to a `ModelId` via case-insensitive
    /// ASCII match against the catalog.
    pub fn resolve_by_name_or_id(&self, query: &str) -> Option<acp::ModelId> {
        self.available.iter().find_map(|(id, info)| {
            if info.name.eq_ignore_ascii_case(query) || id.0.as_ref().eq_ignore_ascii_case(query) {
                Some(id.clone())
            } else {
                None
            }
        })
    }

    /// Look up the display name for a `ModelId` in the catalog.
    pub fn display_name_for(&self, id: &acp::ModelId) -> String {
        self.available
            .get(id)
            .map(|info| info.name.clone())
            .unwrap_or_else(|| id.0.to_string())
    }

    /// Cycle to the next model.
    pub fn next_model(&self) -> Option<acp::ModelId> {
        if self.available.is_empty() {
            None
        } else if let Some(ref current) = self.current {
            let idx = self.available.get_index_of(current)?;
            let idx = (idx + 1) % self.available.len();
            Some(self.available.get_index(idx)?.0.clone())
        } else {
            Some(self.available.first()?.0.clone())
        }
    }
}

impl From<Option<acp::SessionModelState>> for ModelState {
    fn from(state: Option<acp::SessionModelState>) -> Self {
        state
            .map(|state| {
                let mut models = IndexMap::new();
                for model in state.available_models {
                    models.insert(model.model_id.clone(), model);
                }
                let current_model = models
                    .contains_key(&state.current_model_id)
                    .then_some(state.current_model_id);
                let current_meta = current_model
                    .as_ref()
                    .and_then(|id| models.get(id))
                    .and_then(|info| info.meta.as_ref());
                let reasoning_effort = parse_reasoning_effort_meta(current_meta);
                // Prefer multi-agent option id (heavy/swarm/swarm-heavy) when
                // the agent stamped orchestrationMode on the current model.
                // Wire reasoningEffort alone is always xhigh for those modes,
                // so resume would otherwise collapse to the plain "xhigh" label.
                let orchestration_option_id = current_meta
                    .and_then(|m| m.get(ORCHESTRATION_MODE_META_KEY))
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .filter(|id| OrchestrationMode::from_option_id(id).is_multi_agent());
                let reasoning_effort_option_id = orchestration_option_id
                    .or_else(|| reasoning_effort.map(|e| e.as_str().to_string()));
                Self {
                    available: models,
                    current: current_model,
                    reasoning_effort,
                    reasoning_effort_option_id,
                    context_window_override: None,
                }
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sample_models() -> ModelState {
        let mut state = ModelState::default();
        let id_a = acp::ModelId::new(Arc::from("model-a"));
        let id_b = acp::ModelId::new(Arc::from("model-b"));
        state.available.insert(
            id_a.clone(),
            acp::ModelInfo::new(id_a.clone(), "Model A".to_string()),
        );
        state.available.insert(
            id_b.clone(),
            acp::ModelInfo::new(id_b.clone(), "Model B".to_string()),
        );
        state.current = Some(id_a);
        state
    }

    #[test]
    fn test_current_model_name() {
        let state = sample_models();
        assert_eq!(state.current_model_name(), Some("Model A".to_string()));
    }

    #[test]
    fn test_next_model_cycles() {
        let state = sample_models();
        let next = state.next_model().unwrap();
        assert_eq!(next.0.as_ref(), "model-b");
    }

    #[test]
    fn test_next_model_wraps() {
        let mut state = sample_models();
        state.current = Some(acp::ModelId::new(Arc::from("model-b")));
        let next = state.next_model().unwrap();
        assert_eq!(next.0.as_ref(), "model-a");
    }

    #[test]
    fn test_empty_state() {
        let state = ModelState::default();
        assert!(state.is_empty());
        assert!(state.current_model_name().is_none());
        assert!(state.next_model().is_none());
    }

    fn model_with_effort(id: &str, name: &str, effort: &str) -> acp::ModelInfo {
        acp::ModelInfo::new(acp::ModelId::new(Arc::from(id)), name.to_string()).meta(
            serde_json::json!({
                "supportsReasoningEffort": true,
                "reasoningEffort": effort,
            })
            .as_object()
            .cloned(),
        )
    }

    #[test]
    fn update_catalog_preserves_user_effort_when_model_unchanged() {
        let id = acp::ModelId::new(Arc::from("grok-build"));
        let mut state = ModelState::default();
        state.available.insert(
            id.clone(),
            model_with_effort("grok-build", "Grok Build", "high"),
        );
        state.set_current(id.clone(), Some(ReasoningEffort::Xhigh));
        assert_eq!(state.reasoning_effort, Some(ReasoningEffort::Xhigh));

        // The broadcast carries the model's static default (high) for the same model.
        let mut refreshed = IndexMap::new();
        refreshed.insert(
            id.clone(),
            model_with_effort("grok-build", "Grok Build", "high"),
        );
        state.update_catalog(refreshed, Some(id.clone()));

        assert_eq!(
            state.reasoning_effort,
            Some(ReasoningEffort::Xhigh),
            "catalog refresh must not clobber a user-set per-session effort"
        );
    }

    #[test]
    fn from_session_model_state_restores_orchestration_option_id() {
        let id = acp::ModelId::new(Arc::from("grok-4"));
        let mut info = model_with_effort("grok-4", "Grok 4", "xhigh");
        let mut meta = info.meta.take().unwrap_or_default();
        meta.insert(
            ORCHESTRATION_MODE_META_KEY.to_string(),
            serde_json::Value::String("heavy".into()),
        );
        info.meta = Some(meta);
        let session = acp::SessionModelState::new(id, vec![info]);
        let state = ModelState::from(Some(session));
        assert_eq!(state.reasoning_effort, Some(ReasoningEffort::Xhigh));
        assert_eq!(
            state.reasoning_effort_option_id.as_deref(),
            Some("heavy"),
            "LoadSession models must restore multi-agent option id, not plain xhigh"
        );
        assert_eq!(state.orchestration_mode(), OrchestrationMode::Heavy);
    }

    #[test]
    fn set_current_preserves_multi_agent_option_id_on_wire_xhigh_echo() {
        let id = acp::ModelId::new(Arc::from("grok-4"));
        let mut state = ModelState::default();
        state.available.insert(
            id.clone(),
            model_with_effort("grok-4", "Grok 4", "xhigh"),
        );
        // User selected Heavy (option id) which wires as xhigh.
        state.set_current_with_option(
            id.clone(),
            Some(ReasoningEffort::Xhigh),
            Some("heavy".into()),
        );
        assert_eq!(state.reasoning_effort_option_id.as_deref(), Some("heavy"));
        assert_eq!(state.orchestration_mode(), OrchestrationMode::Heavy);

        // ModelChanged echo: same model, wire xhigh, no option id.
        state.set_current(id.clone(), Some(ReasoningEffort::Xhigh));
        assert_eq!(
            state.reasoning_effort_option_id.as_deref(),
            Some("heavy"),
            "wire-only set_current must not revert multi-agent option id to xhigh"
        );
        assert_eq!(state.orchestration_mode(), OrchestrationMode::Heavy);

        // Same for swarm / swarm-heavy.
        for (oid, mode) in [
            ("swarm", OrchestrationMode::Swarm),
            ("swarm-heavy", OrchestrationMode::SwarmHeavy),
        ] {
            state.set_current_with_option(
                id.clone(),
                Some(ReasoningEffort::Xhigh),
                Some(oid.into()),
            );
            state.set_current(id.clone(), Some(ReasoningEffort::Xhigh));
            assert_eq!(
                state.reasoning_effort_option_id.as_deref(),
                Some(oid),
                "preserved option for {oid}"
            );
            assert_eq!(state.orchestration_mode(), mode);
        }

        // Real effort change to high *should* clear multi-agent option.
        state.set_current_with_option(
            id.clone(),
            Some(ReasoningEffort::Xhigh),
            Some("heavy".into()),
        );
        state.set_current(id, Some(ReasoningEffort::High));
        assert_eq!(
            state.reasoning_effort_option_id.as_deref(),
            Some("high"),
            "changing wire effort away from multi-agent must drop the mode"
        );
    }

    #[test]
    fn update_catalog_rederives_effort_when_current_model_changes() {
        let id_a = acp::ModelId::new(Arc::from("model-a"));
        let mut state = ModelState::default();
        state.available.insert(
            id_a.clone(),
            model_with_effort("model-a", "Model A", "high"),
        );
        state.set_current(id_a.clone(), Some(ReasoningEffort::Xhigh));

        // Refresh drops model-a; fall back to model-b whose default is low.
        let id_b = acp::ModelId::new(Arc::from("model-b"));
        let mut refreshed = IndexMap::new();
        refreshed.insert(id_b.clone(), model_with_effort("model-b", "Model B", "low"));
        state.update_catalog(refreshed, Some(id_b.clone()));

        assert_eq!(state.current, Some(id_b));
        assert_eq!(state.reasoning_effort, Some(ReasoningEffort::Low));
    }

    fn state_with_meta(meta: Option<serde_json::Value>) -> ModelState {
        let id = acp::ModelId::new(Arc::from("m"));
        let mut state = ModelState::default();
        state.available.insert(
            id.clone(),
            acp::ModelInfo::new(id.clone(), "M".to_string())
                .meta(meta.and_then(|v| v.as_object().cloned())),
        );
        state.current = Some(id);
        state
    }

    #[test]
    fn accepts_images_defaults_true_when_meta_absent() {
        // No current model, empty meta, and a meta without the key all default
        // permissive — correct today and a no-op until the server populates it.
        assert!(ModelState::default().current_model_accepts_images());
        assert!(state_with_meta(None).current_model_accepts_images());
        assert!(
            state_with_meta(Some(serde_json::json!({ "totalContextTokens": 256000 })))
                .current_model_accepts_images()
        );
    }

    #[test]
    fn reasoning_effort_options_renders_server_list() {
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                { "id": "balanced", "value": "medium", "label": "Balanced" },
                { "id": "deep", "value": "xhigh", "label": "Deep", "description": "Max" },
            ],
        })));
        let opts = state.reasoning_effort_options();
        assert!(opts.len() >= 5, "opts={opts:?}");
        assert_eq!(opts[0].id, "swarm-heavy");
        let deep = opts.iter().find(|o| o.id == "deep").unwrap();
        assert_eq!(deep.description.as_deref(), Some("Max"));
        let balanced = opts.iter().find(|o| o.id == "balanced").unwrap();
        assert_eq!(balanced.label, "Balanced");
        assert_eq!(balanced.value, ReasoningEffort::Medium);
    }

    #[test]
    fn reasoning_effort_options_gate_first_empty_when_unsupported() {
        // No current model → empty.
        assert!(ModelState::default().reasoning_effort_options().is_empty());
        // Current model that does not support effort → empty (even with a list).
        let state = state_with_meta(Some(serde_json::json!({
            "reasoningEfforts": [{ "value": "high" }],
        })));
        assert!(state.reasoning_effort_options().is_empty());
    }

    #[test]
    fn reasoning_effort_options_falls_back_to_builtin_menu() {
        // Supported but no server list → today's four-row built-in menu.
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
        })));
        let ids: Vec<_> = state
            .reasoning_effort_options()
            .into_iter()
            .map(|o| o.id)
            .collect();
        assert_eq!(ids, ["swarm-heavy", "swarm", "heavy", "xhigh", "high", "medium", "low"]);
    }

    #[test]
    fn reasoning_effort_options_falls_back_when_list_present_but_unusable() {
        // Matches the shell picker: an explicit empty list, and a list where every
        // entry skip-invalidated under version skew, both fall back to the built-in
        // menu rather than silently vanishing.
        for meta in [
            serde_json::json!({ "supportsReasoningEffort": true, "reasoningEfforts": [] }),
            serde_json::json!({
                "supportsReasoningEffort": true,
                "reasoningEfforts": [{ "value": "quantum" }],
            }),
        ] {
            let ids: Vec<_> = state_with_meta(Some(meta.clone()))
                .reasoning_effort_options()
                .into_iter()
                .map(|o| o.id)
                .collect();
            assert_eq!(ids, ["swarm-heavy", "swarm", "heavy", "xhigh", "high", "medium", "low"], "for meta {meta}");
        }
    }

    #[test]
    fn resolve_effort_token_maps_remap_id_to_canonical_value() {
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                { "id": "deep", "value": "xhigh", "label": "Deep" },
                { "id": "high", "value": "high", "label": "High" },
            ],
        })));
        // Design-2 remap: the typed id resolves to its canonical wire value.
        assert_eq!(
            state.resolve_effort_token("deep"),
            Some(ReasoningEffort::Xhigh)
        );
        assert_eq!(
            state.resolve_effort_token("DEEP"),
            Some(ReasoningEffort::Xhigh)
        );
        // Canonical level offered by the menu is accepted by value.
        assert_eq!(
            state.resolve_effort_token("high"),
            Some(ReasoningEffort::High)
        );
        // Levels the model does not offer (none/minimal on 4.5-style menus)
        // are rejected — better than a server-side 400.
        assert!(state.resolve_effort_token("minimal").is_none());
        assert!(state.resolve_effort_token("none").is_none());
        assert!(state.resolve_effort_token("bogus").is_none());
    }

    #[test]
    fn resolve_effort_token_accepts_none_only_when_menu_offers_it() {
        let with_none = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                { "value": "none", "label": "None", "default": true },
                { "value": "high", "label": "High" },
            ],
        })));
        assert_eq!(
            with_none.resolve_effort_token("none"),
            Some(ReasoningEffort::None)
        );

        let without_none = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEfforts": [
                { "value": "high", "label": "High", "default": true },
                { "value": "low", "label": "Low" },
            ],
        })));
        assert!(without_none.resolve_effort_token("none").is_none());
        let err = without_none
            .resolve_effort_for_model(without_none.current.as_ref().unwrap(), "none")
            .unwrap_err();
        assert_eq!(
            err,
            EffortTokenError::UnknownToken {
                token: "none".to_string(),
                // Multi-agent modes are prepended to every effort menu.
                offered: vec![
                    "swarm-heavy".to_string(),
                    "swarm".to_string(),
                    "heavy".to_string(),
                    "high".to_string(),
                    "low".to_string(),
                ],
            }
        );
        // Error copy must list only this model's options — never hardcode
        // none/minimal/… as offered values (the rejected token may still appear
        // quoted in "unknown effort level '…'").
        let msg = err.message();
        assert!(msg.contains("use one of:"), "msg={msg}");
        assert!(msg.contains("high") && msg.contains("low"), "msg={msg}");
        let offered_half = msg
            .split_once("; ")
            .map(|(_, rest)| rest)
            .expect("message should have '; ' separator");
        assert!(
            !offered_half.contains("none"),
            "must not advertise blocked level: {msg}"
        );
        assert!(
            !offered_half.contains("minimal"),
            "must not advertise blocked level: {msg}"
        );
        assert!(
            !msg.contains("unset"),
            "unset is log-only, not a user token: {msg}"
        );
    }

    #[test]
    fn resolve_effort_token_legacy_menu_rejects_none() {
        // supportsReasoningEffort without a server list → built-in low..xhigh.
        let state = state_with_meta(Some(serde_json::json!({
            "supportsReasoningEffort": true,
        })));
        assert!(state.resolve_effort_token("none").is_none());
        assert!(state.resolve_effort_token("minimal").is_none());
        assert_eq!(
            state.resolve_effort_token("low"),
            Some(ReasoningEffort::Low)
        );
    }

    #[test]
    fn accepts_images_honors_explicit_meta() {
        assert!(
            !state_with_meta(Some(serde_json::json!({ "acceptsImages": false })))
                .current_model_accepts_images()
        );
        assert!(
            state_with_meta(Some(serde_json::json!({ "acceptsImages": true })))
                .current_model_accepts_images()
        );
        // inputModalities array form.
        assert!(
            state_with_meta(Some(
                serde_json::json!({ "inputModalities": ["text", "image"] })
            ))
            .current_model_accepts_images()
        );
        assert!(
            !state_with_meta(Some(serde_json::json!({ "inputModalities": ["text"] })))
                .current_model_accepts_images()
        );
    }
}
