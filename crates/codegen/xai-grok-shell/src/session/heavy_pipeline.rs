//! Code-enforced multi-agent pipeline for Heavy / Swarm Heavy modes.
//!
//! Spawns **real** subagents via `SubagentEvent::Spawn` (same path as the
//! goal classifier — no model tool call). Stages:
//!
//! 1. **Research** (`explore`, read-only)  
//! 2. **Implement** (`general-purpose`, full tools) — sees research output  
//! 3. **Test** (`explore`, execute) — sees research + implement  
//! 4. **Synthesize** parent-visible final report
//!
//! Each child is a live session: open its card in the TUI to watch thinking.

use std::sync::Arc;

use agent_client_protocol as acp;
use tokio::sync::oneshot;
use xai_grok_sampling_types::OrchestrationMode;
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentEvent, SubagentRequest, SubagentResult, SubagentRuntimeOverrides,
};
use xai_tool_types::SubagentCapabilityMode as CapMode;

use super::SessionActor;
use crate::session::commands::{self, PromptTurnResult};

struct Stage {
    id_suffix: &'static str,
    description: &'static str,
    subagent_type: &'static str,
    capability: CapMode,
    system_lens: &'static str,
}

const STAGES: &[Stage] = &[
    Stage {
        id_suffix: "research",
        description: "[Pipeline/Research] investigate codebase & requirements",
        subagent_type: "explore",
        capability: CapMode::ReadOnly,
        system_lens: "\
You are the RESEARCH agent in a **code-enforced** multi-agent pipeline.
- Explore with tools (search, read, list, web if needed).
- Produce findings, constraints, risks, recommended approach.
- Do NOT implement code changes.
- Cite concrete file paths and evidence.",
    },
    Stage {
        id_suffix: "implement",
        description: "[Pipeline/Implement] make the code changes",
        subagent_type: "general-purpose",
        capability: CapMode::All,
        system_lens: "\
You are the IMPLEMENT agent in a **code-enforced** multi-agent pipeline.
- Use RESEARCH findings as your brief.
- Implement the user request with real edits/commands.
- Prefer minimal correct changes; summarize what changed.
- Do not run exhaustive full-repo test suites unless necessary.",
    },
    Stage {
        id_suffix: "test",
        description: "[Pipeline/Test] verify the implementation",
        subagent_type: "explore",
        capability: CapMode::Execute,
        system_lens: "\
You are the TEST / VERIFY agent in a **code-enforced** multi-agent pipeline.
- Review RESEARCH + IMPLEMENT outputs.
- Run relevant tests or checks when possible.
- Report pass/fail, gaps, regressions, residual risks.
- Prefer verification over rewriting the whole change.",
    },
];

/// Detect mode from system prompt (protocol inject from model_switch).
pub(crate) fn detect_mode_from_system(system: &str) -> OrchestrationMode {
    if system.contains("SWARM HEAVY MODE") || system.contains("⬢ SWARM HEAVY") {
        OrchestrationMode::SwarmHeavy
    } else if system.contains("## ⬡ SWARM MODE") || system.contains("SWARM MODE —") {
        OrchestrationMode::Swarm
    } else if system.contains("HEAVY MODE") || system.contains("◈ HEAVY") {
        OrchestrationMode::Heavy
    } else {
        OrchestrationMode::Normal
    }
}

impl SessionActor {
    pub(super) async fn should_run_heavy_pipeline(&self) -> bool {
        matches!(
            self.current_orchestration_mode().await,
            OrchestrationMode::Heavy | OrchestrationMode::SwarmHeavy
        )
    }

    async fn current_orchestration_mode(&self) -> OrchestrationMode {
        let conv = self.chat_state_handle.get_conversation().await;
        for item in &conv {
            if let crate::sampling::ConversationItem::System(sys) = item {
                return detect_mode_from_system(&sys.content);
            }
        }
        OrchestrationMode::Normal
    }

    /// Forced Research → Implement → Test subagent pipeline.
    pub(super) async fn run_heavy_pipeline(
        self: &Arc<Self>,
        prompt_id: &str,
        user_text: &str,
    ) -> PromptTurnResult {
        let mode = self.current_orchestration_mode().await;
        let Some(event_tx) = self.tool_context.subagent_event_tx.clone() else {
            tracing::warn!(
                session_id = %self.session_info.id.0,
                "heavy_pipeline: subagent_event_tx missing — cannot run enforced pipeline"
            );
            self.emit_agent_text(
                "◈ Heavy pipeline unavailable (subagent coordinator not ready). \
                 Falling back was skipped — retry after session fully starts.",
            )
            .await;
            return commands::ok_end_turn(0, None);
        };

        let session_id = self.session_info.id.0.to_string();
        tracing::info!(
            session_id = %session_id,
            mode = %mode,
            "heavy_pipeline: code-enforced RIT start"
        );

        self.emit_agent_text(&format!(
            "{}\n\n**Code-enforced pipeline** (real subagents, not prompt role-play):\n\
             1. Research → 2. Implement → 3. Test → 4. Synthesis\n\n\
             Open each **[Pipeline/…]** worker to watch its full thinking live.",
            mode_banner(mode)
        ))
        .await;

        let mut prior: Vec<(String, String)> = Vec::new();

        for (idx, stage) in STAGES.iter().enumerate() {
            let n = idx + 1;
            self.emit_agent_text(&format!(
                "── **Phase {n}/3** · {} ──\nSpawning `{}` as a real child session…",
                stage.description, stage.subagent_type
            ))
            .await;

            let prompt = build_stage_prompt(stage, user_text, &prior);
            // UUID-like uniqueness for child id
            let child_id = format!(
                "pipe-{}-{}-{}",
                stage.id_suffix,
                &session_id.replace('-', "")[..8.min(session_id.len())],
                &prompt_id.replace('-', "")[..8.min(prompt_id.len())]
            );

            match spawn_stage(
                &event_tx,
                &child_id,
                &session_id,
                Some(prompt_id.to_string()),
                stage,
                prompt,
            )
            .await
            {
                Ok(result) => {
                    let out = if result.success {
                        result.output.to_string()
                    } else {
                        format!(
                            "[stage failed] {}",
                            result
                                .error
                                .unwrap_or_else(|| "unknown error".into())
                        )
                    };
                    let preview: String = out.chars().take(1500).collect();
                    self.emit_agent_text(&format!(
                        "✓ **Phase {n} complete** · {}\n\n{}",
                        stage.description, preview
                    ))
                    .await;
                    prior.push((stage.id_suffix.to_string(), out));
                }
                Err(e) => {
                    self.emit_agent_text(&format!(
                        "✗ **Phase {n} failed** · {}: {e}",
                        stage.description
                    ))
                    .await;
                    prior.push((stage.id_suffix.to_string(), format!("[error] {e}")));
                }
            }
        }

        let synthesis = synthesize(user_text, &prior);
        self.emit_agent_text(&synthesis).await;

        tracing::info!(
            session_id = %session_id,
            stages = prior.len(),
            "heavy_pipeline: done"
        );
        commands::ok_end_turn(0, None)
    }

    async fn emit_agent_text(&self, text: &str) {
        self.send_update(
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
                acp::ContentBlock::Text(acp::TextContent::new(format!("{text}\n\n"))),
            )),
            None,
        )
        .await;
    }
}

fn mode_banner(mode: OrchestrationMode) -> String {
    match mode {
        OrchestrationMode::Heavy => "◈ **HEAVY** · Research → Implement → Test".into(),
        OrchestrationMode::SwarmHeavy => {
            "⬢ **SWARM HEAVY** · Research → Implement → Test".into()
        }
        OrchestrationMode::Swarm => "⬡ **SWARM** · Research → Implement → Test".into(),
        OrchestrationMode::Normal => "Pipeline".into(),
    }
}

fn build_stage_prompt(stage: &Stage, user_text: &str, prior: &[(String, String)]) -> String {
    let mut s = String::new();
    s.push_str(stage.system_lens);
    s.push_str("\n\n## User request\n\n");
    s.push_str(user_text);
    if !prior.is_empty() {
        s.push_str("\n\n## Prior pipeline outputs (do not ignore)\n");
        for (name, out) in prior {
            s.push_str("\n### ");
            s.push_str(name);
            s.push_str("\n\n");
            let clipped: String = out.chars().take(14_000).collect();
            s.push_str(&clipped);
            s.push('\n');
        }
    }
    s.push_str(
        "\n\n## Required return format\n\
         - **Status:** success | partial | blocked\n\
         - **Summary:** 3–8 bullets\n\
         - **Evidence:** paths, commands, key quotes\n\
         - **Handoff:** what the next stage must know\n",
    );
    s
}

async fn spawn_stage(
    event_tx: &tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
    id: &str,
    parent_session_id: &str,
    parent_prompt_id: Option<String>,
    stage: &Stage,
    prompt: String,
) -> Result<SubagentResult, String> {
    let (result_tx, result_rx) = oneshot::channel();
    let request = SubagentRequest {
        id: id.to_string(),
        prompt,
        description: stage.description.to_string(),
        subagent_type: stage.subagent_type.to_string(),
        parent_session_id: parent_session_id.to_string(),
        parent_prompt_id,
        resume_from: None,
        cwd: None,
        runtime_overrides: SubagentRuntimeOverrides {
            capability_mode: Some(stage.capability),
            reasoning_effort: Some("xhigh".into()),
            ..Default::default()
        },
        // Block until stage completes; ACP still streams child updates live.
        run_in_background: false,
        surface_completion: true,
        fork_context: false,
        result_tx,
    };
    event_tx
        .send(SubagentEvent::Spawn(Box::new(request)))
        .map_err(|_| "subagent coordinator channel closed".to_string())?;
    result_rx
        .await
        .map_err(|_| "subagent result channel dropped".to_string())
}

fn synthesize(user_text: &str, prior: &[(String, String)]) -> String {
    let mut out = String::from("# ◈ HEAVY PIPELINE RESULT\n\n");
    out.push_str("**Request:** ");
    out.push_str(user_text.trim());
    out.push_str(
        "\n\nProduced by a **code-enforced** Research → Implement → Test pipeline \
         (real subagents). Open each **[Pipeline/…]** worker for full thinking.\n\n",
    );
    for (name, body) in prior {
        out.push_str("## ");
        out.push_str(&name.to_ascii_uppercase());
        out.push_str("\n\n");
        let clipped: String = body.chars().take(10_000).collect();
        out.push_str(&clipped);
        out.push_str("\n\n");
    }
    out
}
