use crate::acp::model_state::{EffortTokenError, ModelState};
use crate::app::dispatch::session::lifecycle::{DeferredSwitchOutcome, take_deferred_model_switch};
use agent_client_protocol as acp;
use std::sync::Arc;
use xai_grok_shell::sampling::types::ReasoningEffort;

fn model_with_support(id: &str, supports: bool) -> (acp::ModelId, acp::ModelInfo) {
    let id = acp::ModelId::new(Arc::from(id));
    let meta = if supports {
        Some(serde_json::json!({
            "supportsReasoningEffort": true,
            "reasoningEffort": "medium",
            "reasoningEfforts": [
                { "id": "deep", "value": "xhigh", "label": "Deep" },
                { "id": "high", "value": "high", "label": "High" },
            ],
        }))
    } else {
        Some(serde_json::json!({ "reasoningEffort": "medium" }))
    };
    let info = acp::ModelInfo::new(id.clone(), id.0.to_string())
        .meta(meta.and_then(|v| v.as_object().cloned()));
    (id, info)
}

fn models_with_current(supports: bool) -> ModelState {
    let (id, info) = model_with_support("grok-build", supports);
    let mut models = ModelState::default();
    models.available.insert(id.clone(), info);
    models.current = Some(id);
    models.reasoning_effort = Some(ReasoningEffort::Medium);
    models
}

#[test]
fn effort_only_resolves_canonical_token() {
    let models = models_with_current(true);
    let out = take_deferred_model_switch(None, &models, Some("high"));
    assert_eq!(
        out,
        DeferredSwitchOutcome {
            switch: Some((
                models.current.clone().unwrap(),
                Some(ReasoningEffort::High),
                Some("high".into()),
            )),
            effort_error: None,
        }
    );
}

#[test]
fn effort_only_resolves_remapped_menu_id() {
    let models = models_with_current(true);
    let out = take_deferred_model_switch(None, &models, Some("deep"));
    assert_eq!(
        out,
        DeferredSwitchOutcome {
            switch: Some((
                models.current.clone().unwrap(),
                Some(ReasoningEffort::Xhigh),
                Some("deep".into()),
            )),
            effort_error: None,
        }
    );
}

#[test]
fn effort_only_unsupported_canonical_token_is_unsupported() {
    // Gate-first: a canonical token on a model that doesn't support reasoning
    // effort surfaces Unsupported (matching `/effort` and headless) rather than
    // silently applying an effort the server would drop.
    let models = models_with_current(false);
    assert_eq!(
        take_deferred_model_switch(None, &models, Some("high")),
        DeferredSwitchOutcome {
            switch: None,
            effort_error: Some(EffortTokenError::Unsupported),
        }
    );
}

#[test]
fn effort_only_unsupported_unknown_token_is_unsupported() {
    let models = models_with_current(false);
    assert_eq!(
        take_deferred_model_switch(None, &models, Some("bogus")),
        DeferredSwitchOutcome {
            switch: None,
            effort_error: Some(EffortTokenError::Unsupported),
        }
    );
}

#[test]
fn effort_only_skips_when_already_equal() {
    let mut models = models_with_current(true);
    models.reasoning_effort = Some(ReasoningEffort::High);
    assert_eq!(
        take_deferred_model_switch(None, &models, Some("high")),
        DeferredSwitchOutcome {
            switch: None,
            effort_error: None,
        }
    );
}

#[test]
fn effort_only_errors_on_unknown_token() {
    let models = models_with_current(true);
    let out = take_deferred_model_switch(None, &models, Some("bogus"));
    assert!(out.switch.is_none());
    match out.effort_error {
        Some(EffortTokenError::UnknownToken { token, offered }) => {
            assert_eq!(token, "bogus");
            // Multi-agent options are merged into the menu; at least the server
            // ids must be present.
            assert!(offered.iter().any(|id| id == "deep"));
            assert!(offered.iter().any(|id| id == "high"));
        }
        other => panic!("expected UnknownToken, got {other:?}"),
    }
}

#[test]
fn stashed_model_switch_prefers_explicit_stash() {
    let models = models_with_current(true);
    let other = acp::ModelId::new(Arc::from("other-model"));
    let out = take_deferred_model_switch(
        Some((other.clone(), Some(ReasoningEffort::Low))),
        &models,
        Some("high"),
    );
    assert_eq!(
        out,
        DeferredSwitchOutcome {
            // Stash effort wins. Option id is only filled when the stashed
            // model is in the catalog (other-model is not).
            switch: Some((other, Some(ReasoningEffort::Low), None)),
            effort_error: None,
        }
    );
}

#[test]
fn stashed_model_re_resolves_remap_when_effort_missing() {
    let models = models_with_current(true);
    let current = models.current.clone().unwrap();
    let out = take_deferred_model_switch(Some((current.clone(), None)), &models, Some("deep"));
    assert_eq!(
        out,
        DeferredSwitchOutcome {
            switch: Some((
                current,
                Some(ReasoningEffort::Xhigh),
                Some("deep".into()),
            )),
            effort_error: None,
        }
    );
}

#[test]
fn stashed_model_keeps_model_when_token_unresolvable() {
    let models = models_with_current(true);
    let current = models.current.clone().unwrap();
    let out = take_deferred_model_switch(Some((current.clone(), None)), &models, Some("bogus"));
    assert_eq!(out.switch, Some((current, None, None)));
    match out.effort_error {
        Some(EffortTokenError::UnknownToken { token, .. }) => assert_eq!(token, "bogus"),
        other => panic!("expected UnknownToken, got {other:?}"),
    }
}

#[test]
fn stashed_model_keeps_model_when_unsupported() {
    // -m targets a non-reasoning model plus an effort token: keep the model
    // switch, drop the effort, and surface Unsupported.
    let mut models = models_with_current(true);
    let (plain, plain_info) = model_with_support("plain-model", false);
    models.available.insert(plain.clone(), plain_info);
    let out = take_deferred_model_switch(Some((plain.clone(), None)), &models, Some("high"));
    assert_eq!(
        out,
        DeferredSwitchOutcome {
            switch: Some((plain, None, None)),
            effort_error: Some(EffortTokenError::Unsupported),
        }
    );
}

#[test]
fn effort_only_accepts_max_as_xhigh() {
    let models = models_with_current(true);
    let out = take_deferred_model_switch(None, &models, Some("max"));
    // "max" maps to xhigh wire; option id is the matching non-multi-agent menu
    // entry ("deep" in this catalog) when present.
    assert_eq!(out.effort_error, None);
    let Some((mid, Some(effort), option_id)) = out.switch else {
        panic!("expected switch, got {out:?}");
    };
    assert_eq!(mid, models.current.clone().unwrap());
    assert_eq!(effort, ReasoningEffort::Xhigh);
    assert!(option_id.is_some());
}

#[test]
fn effort_only_errors_without_active_model() {
    let models = ModelState::default();
    assert_eq!(
        take_deferred_model_switch(None, &models, Some("high")),
        DeferredSwitchOutcome {
            switch: None,
            effort_error: Some(EffortTokenError::NoActiveModel),
        }
    );
}

#[test]
fn multi_agent_token_switches_even_when_wire_effort_already_xhigh() {
    // Config default is often xhigh; Heavy/Swarm still need a switch so the
    // option id is set and protocol inject runs.
    let mut models = models_with_current(true);
    models.reasoning_effort = Some(ReasoningEffort::Xhigh);
    let out = take_deferred_model_switch(None, &models, Some("heavy"));
    assert_eq!(out.effort_error, None);
    let Some((_, Some(effort), Some(oid))) = out.switch else {
        panic!("expected multi-agent switch, got {out:?}");
    };
    assert_eq!(effort, ReasoningEffort::Xhigh);
    assert_eq!(oid, "heavy");
}

#[test]
fn multi_agent_token_skips_when_already_active() {
    let mut models = models_with_current(true);
    models.reasoning_effort = Some(ReasoningEffort::Xhigh);
    models.reasoning_effort_option_id = Some("swarm".into());
    let out = take_deferred_model_switch(None, &models, Some("swarm"));
    assert_eq!(
        out,
        DeferredSwitchOutcome {
            switch: None,
            effort_error: None,
        }
    );
}
