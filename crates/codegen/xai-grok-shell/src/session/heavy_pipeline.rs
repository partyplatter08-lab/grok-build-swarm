//! Code-enforced multi-agent pipeline for Heavy / Swarm Heavy.
//!
//! ## Flow
//!
//! ```text
//! 0. Parallel COUNCIL  (Analyst · Skeptic · Explorer · Builder)  ── all at once
//! 1. Research   (explore, read-only)     ─┐
//! 2. Implement  (general-purpose)        ├─ sequential, each sees prior outputs
//! 3. Test       (explore + execute)      ─┘
//! 4. Synthesis  parent report
//! ```
//!
//! Council + RIT are **real** subagents via `SubagentEvent::Spawn`.
//! Worker streams live in subagent cards (open a card for full thinking);
//! the parent feed gets phase banners + synthesis only — not per-token
//! `[Council/…]` dumps (those interleave into unreadable noise).

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

// ── Council (parallel) ──────────────────────────────────────────────────────

struct CouncilMember {
    id_suffix: &'static str,
    description: &'static str,
    lens: &'static str,
}

const COUNCIL: &[CouncilMember] = &[
    CouncilMember {
        id_suffix: "analyst",
        description: "[Council/Analyst] structure the problem",
        lens: "You are the ANALYST. Structure the problem, goals, constraints, success criteria. Be precise; cite paths when relevant.",
    },
    CouncilMember {
        id_suffix: "skeptic",
        description: "[Council/Skeptic] attack weak assumptions",
        lens: "You are the SKEPTIC. Attack weak assumptions, edge cases, failure modes, and overconfidence. Prefer hard questions over soft agreement.",
    },
    CouncilMember {
        id_suffix: "explorer",
        description: "[Council/Explorer] find alternatives & context",
        lens: "You are the EXPLORER. Survey the codebase/context for alternatives, prior art, and non-obvious approaches. Use tools freely (read-only).",
    },
    CouncilMember {
        id_suffix: "builder",
        description: "[Council/Builder] propose a concrete plan",
        lens: "You are the BUILDER. Propose a concrete implementation plan (steps, files, risks). Do NOT implement yet — plan only.",
    },
];

// ── RIT stages (sequential) ─────────────────────────────────────────────────

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
You are RESEARCH in a code-enforced pipeline (after a parallel council).
- Use council theses + tools to deepen investigation.
- Produce findings, constraints, risks, recommended approach.
- Do NOT implement code changes.",
    },
    Stage {
        id_suffix: "implement",
        description: "[Pipeline/Implement] make the code changes",
        subagent_type: "general-purpose",
        capability: CapMode::All,
        system_lens: "\
You are IMPLEMENT in a code-enforced pipeline.
- Use council + research as your brief.
- Implement with real edits/commands; minimize scope; summarize changes.",
    },
    Stage {
        id_suffix: "test",
        description: "[Pipeline/Test] verify the implementation",
        subagent_type: "explore",
        capability: CapMode::Execute,
        system_lens: "\
You are TEST/VERIFY in a code-enforced pipeline.
- Review research + implement; run checks/tests when possible.
- Report pass/fail, gaps, regressions, residual risks.",
    },
];

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

    pub(super) async fn run_heavy_pipeline(
        self: &Arc<Self>,
        prompt_id: &str,
        user_text: &str,
    ) -> PromptTurnResult {
        let mode = self.current_orchestration_mode().await;
        let Some(event_tx) = self.tool_context.subagent_event_tx.clone() else {
            tracing::warn!(
                session_id = %self.session_info.id.0,
                "heavy_pipeline: subagent_event_tx missing"
            );
            self.emit_agent_text(
                "◈ Heavy pipeline unavailable (subagent coordinator not ready).",
            )
            .await;
            return commands::ok_end_turn(0, None);
        };

        let session_id = self.session_info.id.0.to_string();
        let id_prefix = format!(
            "{}-{}",
            &session_id.replace('-', "")[..8.min(session_id.len())],
            &prompt_id.replace('-', "")[..8.min(prompt_id.len())]
        );

        tracing::info!(
            session_id = %session_id,
            mode = %mode,
            "heavy_pipeline: council + RIT start"
        );

        self.emit_agent_text(&format!(
            "{}\n\n**Code-enforced pipeline**\n\
             0. **Parallel council** (Analyst · Skeptic · Explorer · Builder)\n\
             1–3. **Research → Implement → Test** (sequential)\n\
             4. Synthesis\n\n\
             Worker thinking streams into this chat with labels; open cards for full detail.",
            mode_banner(mode)
        ))
        .await;

        let mut prior: Vec<(String, String)> = Vec::new();

        // ── Phase 0: parallel council ───────────────────────────────────
        self.emit_agent_text(
            "── **Phase 0 · Parallel council** ──\n\
             Spawning Analyst · Skeptic · Explorer · Builder **in parallel**…",
        )
        .await;

        let council_results =
            run_council_parallel(&event_tx, &session_id, prompt_id, &id_prefix, user_text).await;

        let mut council_digest = String::from("## Council theses (parallel)\n\n");
        for (desc, result) in &council_results {
            let body = match result {
                Ok(r) if r.success => r.output.to_string(),
                Ok(r) => format!(
                    "[failed] {}",
                    r.error.clone().unwrap_or_else(|| "unknown".into())
                ),
                Err(e) => format!("[error] {e}"),
            };
            let preview: String = body.chars().take(2000).collect();
            self.emit_agent_text(&format!("✓ Council member done · {desc}\n\n{preview}"))
                .await;
            council_digest.push_str("### ");
            council_digest.push_str(desc);
            council_digest.push_str("\n\n");
            council_digest.push_str(&body.chars().take(12_000).collect::<String>());
            council_digest.push_str("\n\n");
        }
        prior.push(("council".into(), council_digest));

        // ── Phases 1–3: RIT sequential ──────────────────────────────────
        for (idx, stage) in STAGES.iter().enumerate() {
            let n = idx + 1;
            self.emit_agent_text(&format!(
                "── **Phase {n}/3 · RIT** · {} ──\nSpawning `{}`…",
                stage.description, stage.subagent_type
            ))
            .await;

            let prompt = build_stage_prompt(stage, user_text, &prior);
            let child_id = format!("pipe-{}-{}", stage.id_suffix, id_prefix);

            match spawn_and_wait(
                &event_tx,
                &child_id,
                &session_id,
                Some(prompt_id.to_string()),
                stage.description,
                stage.subagent_type,
                stage.capability,
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
                            result.error.unwrap_or_else(|| "unknown".into())
                        )
                    };
                    let preview: String = out.chars().take(1500).collect();
                    self.emit_agent_text(&format!(
                        "✓ **Phase {n} complete** · {}\n\n{preview}",
                        stage.description
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

        self.emit_agent_text(&synthesize(user_text, &prior)).await;
        tracing::info!(
            session_id = %session_id,
            stages = prior.len(),
            "heavy_pipeline: done (council + RIT)"
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
        OrchestrationMode::Heavy => {
            "◈ **HEAVY** · Parallel council → Research → Implement → Test".into()
        }
        OrchestrationMode::SwarmHeavy => {
            "⬢ **SWARM HEAVY** · Parallel council → Research → Implement → Test".into()
        }
        OrchestrationMode::Swarm => {
            "⬡ **SWARM** · Parallel council → Research → Implement → Test".into()
        }
        OrchestrationMode::Normal => "Pipeline".into(),
    }
}

/// Fire all council spawns, then await every result (true parallelism).
async fn run_council_parallel(
    event_tx: &tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
    parent_session_id: &str,
    prompt_id: &str,
    id_prefix: &str,
    user_text: &str,
) -> Vec<(String, Result<SubagentResult, String>)> {
    enum Pending {
        Failed(String),
        Waiting(oneshot::Receiver<SubagentResult>),
    }
    let mut pending: Vec<(String, Pending)> = Vec::with_capacity(COUNCIL.len());

    for member in COUNCIL {
        let (result_tx, result_rx) = oneshot::channel();
        let id = format!("council-{}-{}", member.id_suffix, id_prefix);
        let prompt = format!(
            "{lens}\n\n## User request\n\n{user}\n\n## Return format\n\
             - **Thesis** (1–3 sentences)\n\
             - **Argument** (bullets)\n\
             - **Evidence** (paths / facts)\n\
             - **Risks**\n\
             - **What to check next**\n",
            lens = member.lens,
            user = user_text
        );
        let request = SubagentRequest {
            id,
            prompt,
            description: member.description.to_string(),
            subagent_type: "explore".to_string(),
            parent_session_id: parent_session_id.to_string(),
            parent_prompt_id: Some(prompt_id.to_string()),
            resume_from: None,
            cwd: None,
            runtime_overrides: SubagentRuntimeOverrides {
                capability_mode: Some(CapMode::ReadOnly),
                reasoning_effort: Some("xhigh".into()),
                ..Default::default()
            },
            // Background:true so all four start before any finishes.
            run_in_background: true,
            surface_completion: true,
            fork_context: false,
            result_tx,
        };
        let desc = member.description.to_string();
        if event_tx
            .send(SubagentEvent::Spawn(Box::new(request)))
            .is_err()
        {
            pending.push((
                desc,
                Pending::Failed("subagent coordinator channel closed".into()),
            ));
            continue;
        }
        pending.push((desc, Pending::Waiting(result_rx)));
    }

    let mut out = Vec::with_capacity(pending.len());
    for (desc, p) in pending {
        match p {
            Pending::Failed(e) => out.push((desc, Err(e))),
            Pending::Waiting(rx) => {
                let res = rx
                    .await
                    .map_err(|_| "subagent result channel dropped".to_string());
                out.push((desc, res));
            }
        }
    }
    out
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
            s.push_str(&out.chars().take(14_000).collect::<String>());
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

async fn spawn_and_wait(
    event_tx: &tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
    id: &str,
    parent_session_id: &str,
    parent_prompt_id: Option<String>,
    description: &str,
    subagent_type: &str,
    capability: CapMode,
    prompt: String,
) -> Result<SubagentResult, String> {
    let (result_tx, result_rx) = oneshot::channel();
    let request = SubagentRequest {
        id: id.to_string(),
        prompt,
        description: description.to_string(),
        subagent_type: subagent_type.to_string(),
        parent_session_id: parent_session_id.to_string(),
        parent_prompt_id,
        resume_from: None,
        cwd: None,
        runtime_overrides: SubagentRuntimeOverrides {
            capability_mode: Some(capability),
            reasoning_effort: Some("xhigh".into()),
            ..Default::default()
        },
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
        "\n\n**Code-enforced** parallel council + Research → Implement → Test.\n\
         Thinking from each worker was also streamed into this chat with labels.\n\n",
    );
    for (name, body) in prior {
        out.push_str("## ");
        out.push_str(&name.to_ascii_uppercase());
        out.push_str("\n\n");
        out.push_str(&body.chars().take(10_000).collect::<String>());
        out.push_str("\n\n");
    }
    out
}
