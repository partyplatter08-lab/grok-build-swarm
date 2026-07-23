//! Heavy / Swarm Heavy: **captain agent** + code-enforced workers.
//!
//! ## Product shape
//!
//! The **parent chat is the captain** — a real model that talks to the user,
//! decides what the workers should focus on, reacts to their results, and
//! owns the final answer. Subagents are the captain's tools, not the show.
//!
//! ```text
//! Captain (model)  opens: frames goal, tells user the plan
//!        │
//!        ├─► Parallel COUNCIL  (Analyst · Skeptic · Explorer · Builder)
//!        │     captain posts short status as each lands
//!        │
//! Captain (model)  cross-checks the board, sets the brief for RIT
//!        │
//!        ├─► Research  → captain reacts
//!        ├─► Implement → captain reacts
//!        └─► Test      → captain reacts
//!        │
//! Captain (model)  final synthesis → user-facing answer
//! ```
//!
//! Workers are real subagents (reliable parallel spawn). The captain is real
//! model turns — not canned status banners. Full worker thinking stays on
//! their cards; the parent feed is the captain's voice.

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol as acp;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::oneshot;
use xai_grok_sampling_types::OrchestrationMode;
use xai_grok_tools::implementations::grok_build::task::types::{
    SubagentEvent, SubagentRequest, SubagentResult, SubagentRuntimeOverrides,
};
use xai_tool_types::SubagentCapabilityMode as CapMode;

use super::SessionActor;
use crate::session::commands::{self, PromptTurnResult};

/// How often the captain posts a short "still waiting" note while council runs.
const COUNCIL_HEARTBEAT: Duration = Duration::from_secs(12);

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
        lens: "You are the ANALYST on a Heavy captain's council. Structure the problem, goals, constraints, success criteria. Be precise; cite paths when relevant. The captain will synthesize — be opinionated and useful.",
    },
    CouncilMember {
        id_suffix: "skeptic",
        description: "[Council/Skeptic] attack weak assumptions",
        lens: "You are the SKEPTIC on a Heavy captain's council. Attack weak assumptions, edge cases, failure modes, and overconfidence. Prefer hard questions over soft agreement.",
    },
    CouncilMember {
        id_suffix: "explorer",
        description: "[Council/Explorer] find alternatives & context",
        lens: "You are the EXPLORER on a Heavy captain's council. Survey the codebase/context for alternatives, prior art, and non-obvious approaches. Use tools freely (read-only).",
    },
    CouncilMember {
        id_suffix: "builder",
        description: "[Council/Builder] propose a concrete plan",
        lens: "You are the BUILDER on a Heavy captain's council. Propose a concrete implementation plan (steps, files, risks). Do NOT implement yet — plan only.",
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
You are RESEARCH working for the Heavy captain (after a parallel council).
- Follow the captain's brief below when present.
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
You are IMPLEMENT working for the Heavy captain.
- Follow the captain's brief below when present.
- Use council + research as your brief.
- Implement with real edits/commands; minimize scope; summarize changes.",
    },
    Stage {
        id_suffix: "test",
        description: "[Pipeline/Test] verify the implementation",
        subagent_type: "explore",
        capability: CapMode::Execute,
        system_lens: "\
You are TEST/VERIFY working for the Heavy captain.
- Follow the captain's brief below when present.
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
            "heavy_pipeline: captain-led council + RIT start"
        );

        // ── Captain opens the turn (real model, not a status banner) ────
        let open = self
            .captain_speak(
                CaptainPhase::Open,
                user_text,
                &[],
                None,
                1_200,
            )
            .await;
        if let Some(text) = open {
            self.emit_agent_text(&text).await;
        } else {
            // Fallback only if the captain model is unavailable.
            self.emit_agent_text(&format!(
                "{}\n\nI'm taking this as **Heavy captain** — I'll run a \
                 multi-lens council, then research → implement → test, and \
                 I'll keep talking to you here while workers run on cards.\n\n\
                 **Your ask:** {}",
                mode_banner(mode),
                first_line(user_text, 200),
            ))
            .await;
        }

        let mut prior: Vec<(String, String)> = Vec::new();

        // ── Phase 0: parallel council (workers; captain stays in this chat) ─
        self.emit_agent_text(
            "Putting four specialists on this **in parallel** now \
             (Analyst · Skeptic · Explorer · Builder). Full streams are on \
             their cards — I'll keep the board here.",
        )
        .await;

        let council_results = run_council_parallel(
            self,
            &event_tx,
            &session_id,
            prompt_id,
            &id_prefix,
            user_text,
        )
        .await;

        let mut council_digest = String::from("## Council board (parallel)\n\n");
        let mut theses: Vec<(String, String)> = Vec::new();
        for (desc, result) in &council_results {
            let body = match result {
                Ok(r) if r.success => r.output.to_string(),
                Ok(r) => format!(
                    "[failed] {}",
                    r.error.clone().unwrap_or_else(|| "unknown".into())
                ),
                Err(e) => format!("[error] {e}"),
            };
            let role = short_role(desc);
            let thesis = extract_thesis(&body);
            theses.push((role.clone(), thesis.clone()));
            council_digest.push_str("### ");
            council_digest.push_str(desc);
            council_digest.push_str("\n\n");
            council_digest.push_str(&body.chars().take(12_000).collect::<String>());
            council_digest.push_str("\n\n");
        }
        prior.push(("council".into(), council_digest));

        // Compact board index for the captain model (not dumped raw to the user).
        let board_index = theses
            .iter()
            .map(|(role, thesis)| format!("- **{role}:** {thesis}"))
            .collect::<Vec<_>>()
            .join("\n");

        // ── Captain cross-checks the board (real judgment + brief) ──────
        let captain_brief = self
            .captain_speak(
                CaptainPhase::AfterCouncil,
                user_text,
                &prior,
                Some(&board_index),
                1_800,
            )
            .await;
        if let Some(ref text) = captain_brief {
            self.emit_agent_text(text).await;
        } else {
            self.emit_agent_text(&format!(
                "Council's in. Here's the quick board:\n\n{board_index}\n\n\
                 Moving into Research → Implement → Test with this as context."
            ))
            .await;
        }
        let mut captain_direction = captain_brief.unwrap_or_default();

        // ── Phases 1–3: RIT sequential, each under captain direction ────
        for (idx, stage) in STAGES.iter().enumerate() {
            let n = idx + 1;
            // Brief captain line before each stage — model when possible.
            let pre = self
                .captain_speak(
                    CaptainPhase::BeforeStage {
                        stage: stage.id_suffix,
                        phase_n: n,
                    },
                    user_text,
                    &prior,
                    Some(&captain_direction),
                    700,
                )
                .await;
            if let Some(text) = pre {
                self.emit_agent_text(&text).await;
            } else {
                self.emit_agent_text(&format!(
                    "── Phase {n}/3 · {} — sending a worker, I'll be right back. ──",
                    stage.description
                ))
                .await;
            }

            let prompt = build_stage_prompt(stage, user_text, &prior, &captain_direction);
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
                    prior.push((stage.id_suffix.to_string(), out.clone()));

                    // Captain reacts — this is the main voice, not a checkmark.
                    let reaction = self
                        .captain_speak(
                            CaptainPhase::AfterStage {
                                stage: stage.id_suffix,
                                phase_n: n,
                            },
                            user_text,
                            &prior,
                            Some(&extract_stage_summary(&out)),
                            1_200,
                        )
                        .await;
                    if let Some(text) = reaction {
                        self.emit_agent_text(&text).await;
                        // Let later stages inherit the latest captain judgment.
                        captain_direction = text;
                    } else {
                        self.emit_agent_text(&format!(
                            "Phase {n} landed ({}) — {}",
                            stage.id_suffix,
                            extract_stage_summary(&out)
                        ))
                        .await;
                    }
                }
                Err(e) => {
                    prior.push((stage.id_suffix.to_string(), format!("[error] {e}")));
                    let reaction = self
                        .captain_speak(
                            CaptainPhase::AfterStage {
                                stage: stage.id_suffix,
                                phase_n: n,
                            },
                            user_text,
                            &prior,
                            Some(&format!("STAGE FAILED: {e}")),
                            900,
                        )
                        .await;
                    if let Some(text) = reaction {
                        self.emit_agent_text(&text).await;
                        captain_direction = text;
                    } else {
                        self.emit_agent_text(&format!(
                            "Phase {n} ({}) failed: {e}. I'll keep going with what we have.",
                            stage.id_suffix
                        ))
                        .await;
                    }
                }
            }
        }

        // ── Captain final answer ────────────────────────────────────────
        let final_answer = self
            .captain_speak(
                CaptainPhase::Final,
                user_text,
                &prior,
                Some(&captain_direction),
                4_096,
            )
            .await
            .unwrap_or_else(|| synthesize_board_dump(user_text, &prior));
        self.emit_agent_text(&final_answer).await;

        tracing::info!(
            session_id = %session_id,
            stages = prior.len(),
            "heavy_pipeline: captain-led done"
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

    /// One captain model turn. The captain is the main agent the user hears.
    async fn captain_speak(
        &self,
        phase: CaptainPhase<'_>,
        user_text: &str,
        prior: &[(String, String)],
        extra: Option<&str>,
        max_tokens: u32,
    ) -> Option<String> {
        let sampling_client = match self.prepare_chat_completion(false).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, phase = %phase, "heavy_pipeline: captain prepare failed");
                return None;
            }
        };
        let model = self
            .chat_state_handle
            .get_sampling_config()
            .await
            .map(|c| c.model)
            .unwrap_or_default();
        if model.is_empty() {
            return None;
        }

        let system = phase.system_prompt();
        let user = phase.user_prompt(user_text, prior, extra);

        use crate::sampling::{ConversationItem, ConversationRequest};
        let request = ConversationRequest::from_items(vec![
            ConversationItem::system(system),
            ConversationItem::user(user),
        ])
        .with_model(model)
        .with_max_output_tokens(max_tokens);

        match sampling_client.conversation_collect(request).await {
            Ok(response) => {
                let text = response.assistant_text();
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    tracing::warn!(phase = %phase, "heavy_pipeline: captain returned empty");
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, phase = %phase, "heavy_pipeline: captain call failed");
                None
            }
        }
    }
}

/// What the captain is doing this model call.
enum CaptainPhase<'a> {
    Open,
    AfterCouncil,
    BeforeStage { stage: &'a str, phase_n: usize },
    AfterStage { stage: &'a str, phase_n: usize },
    Final,
}

impl std::fmt::Display for CaptainPhase<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::AfterCouncil => write!(f, "after_council"),
            Self::BeforeStage { stage, .. } => write!(f, "before_{stage}"),
            Self::AfterStage { stage, .. } => write!(f, "after_{stage}"),
            Self::Final => write!(f, "final"),
        }
    }
}

impl CaptainPhase<'_> {
    fn system_prompt(&self) -> String {
        let base = "\
You are the **Heavy captain** — the main agent the user is talking to.\n\
Subagents work for you; they are not the conversation. You own the plan, \
the judgment, and the relationship with the user.\n\n\
Voice rules:\n\
- First person. Natural, decisive, collaborative (Grok energy).\n\
- Talk TO the user. Do not sound like a CI log or status bot.\n\
- Do not dump worker transcripts or invent work that did not happen.\n\
- Be concrete when the board has facts (paths, decisions, risks).\n\
- Keep mid-turn updates tight; the final answer can be thorough.\n";

        let phase = match self {
            Self::Open => "\
PHASE: OPENING\n\
The user just asked something. You are about to run a multi-agent Heavy run \
(parallel council of 4, then research → implement → test).\n\
Write 1 short paragraphs + a tight plan:\n\
- Restate the goal in your own words\n\
- What you will put the council on and why\n\
- What success looks like\n\
Do NOT say you already finished anything. Do NOT list every pipeline stage like a README.",
            Self::AfterCouncil => "\
PHASE: AFTER COUNCIL\n\
Four specialists just reported (board is in the user message). You must:\n\
1. Tell the user what the council actually concluded (agreement + real conflict)\n\
2. Decide what Research → Implement → Test should focus on\n\
3. Write a short **captain brief** the workers will follow\n\
End with a clear next step in plain language. No worker log dumps.",
            Self::BeforeStage { stage, phase_n } => {
                return format!(
                    "{base}\n\
PHASE: BEFORE STAGE {phase_n} ({stage})\n\
In 2–4 sentences, tell the user what you are about to have a worker do and why, \
given everything so far. No fluff. No 'spawning subagent' robot speak."
                );
            }
            Self::AfterStage { stage, phase_n } => {
                return format!(
                    "{base}\n\
PHASE: AFTER STAGE {phase_n} ({stage})\n\
A worker finished. React as captain:\n\
- What matters from their result for the user\n\
- What you are adjusting for the remaining work\n\
- If the stage failed, say how you will recover\n\
Keep it tight (one short section). Do not restate the entire board."
                );
            }
            Self::Final => "\
PHASE: FINAL ANSWER\n\
Write the complete user-facing answer. Title it `# ◈ HEAVY RESULT` if natural.\n\
You received a full multi-agent board. Your job:\n\
1. Answer the user's request directly and completely\n\
2. Prefer conclusions supported by multiple lenses or hard evidence\n\
3. Call out real disagreements and residual risks briefly\n\
4. Be concrete (paths, commands, decisions) when the board has them\n\
You ARE the answer channel — not a summarizer of logs.",
        };
        format!("{base}\n{phase}")
    }

    fn user_prompt(
        &self,
        user_text: &str,
        prior: &[(String, String)],
        extra: Option<&str>,
    ) -> String {
        let mut s = String::new();
        s.push_str("## User request\n\n");
        s.push_str(user_text.trim());
        s.push_str("\n\n");

        if !prior.is_empty() {
            s.push_str("## Pipeline board (for you — do not paste wholesale to the user)\n\n");
            for (name, body) in prior {
                s.push_str("### ");
                s.push_str(name);
                s.push_str("\n\n");
                let cap = match self {
                    Self::Final => 8_000,
                    Self::AfterCouncil => 5_000,
                    _ => 3_500,
                };
                s.push_str(&body.chars().take(cap).collect::<String>());
                s.push_str("\n\n");
            }
        }

        if let Some(extra) = extra {
            s.push_str("## Extra context for this phase\n\n");
            s.push_str(extra.trim());
            s.push_str("\n\n");
        }

        match self {
            Self::Open => s.push_str("Open the Heavy run for the user now."),
            Self::AfterCouncil => {
                s.push_str("Cross-check the council and set the captain brief for RIT now.")
            }
            Self::BeforeStage { stage, .. } => {
                s.push_str(&format!("Brief the user before the `{stage}` worker runs."))
            }
            Self::AfterStage { stage, .. } => {
                s.push_str(&format!("React to the `{stage}` worker result now."))
            }
            Self::Final => s.push_str("Write the final Heavy answer for the user now."),
        }
        s
    }
}

fn mode_banner(mode: OrchestrationMode) -> String {
    match mode {
        OrchestrationMode::Heavy => "◈ **HEAVY** · captain + multi-agent council".into(),
        OrchestrationMode::SwarmHeavy => {
            "⬢ **SWARM HEAVY** · captain + multi-agent pipeline".into()
        }
        OrchestrationMode::Swarm => "⬡ **SWARM** · captain + multi-agent".into(),
        OrchestrationMode::Normal => "Captain".into(),
    }
}

/// Fire all council spawns, then collect results as they finish (true parallelism).
///
/// Short status lines only — the captain model owns the real talking.
async fn run_council_parallel(
    actor: &SessionActor,
    event_tx: &tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
    parent_session_id: &str,
    prompt_id: &str,
    id_prefix: &str,
    user_text: &str,
) -> Vec<(String, Result<SubagentResult, String>)> {
    let mut futs: FuturesUnordered<
        std::pin::Pin<
            Box<dyn std::future::Future<Output = (String, Result<SubagentResult, String>)> + Send>,
        >,
    > = FuturesUnordered::new();
    let mut early: Vec<(String, Result<SubagentResult, String>)> = Vec::new();
    let mut outstanding: Vec<String> = Vec::new();

    for member in COUNCIL {
        let (result_tx, result_rx) = oneshot::channel();
        let id = format!("council-{}-{}", member.id_suffix, id_prefix);
        let prompt = format!(
            "{lens}\n\n## User request\n\n{user}\n\n## Return format\n\
             - **Thesis** (1–3 sentences)\n\
             - **Argument** (bullets)\n\
             - **Evidence** (paths / facts)\n\
             - **Risks**\n\
             - **What to check next**\n\
             Keep Thesis crisp — the captain will quote it.\n",
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
            run_in_background: true,
            surface_completion: true,
            fork_context: false,
            result_tx,
        };
        let desc = member.description.to_string();
        let role = short_role(&desc);
        if event_tx
            .send(SubagentEvent::Spawn(Box::new(request)))
            .is_err()
        {
            early.push((
                desc,
                Err("subagent coordinator channel closed".into()),
            ));
            continue;
        }
        outstanding.push(role);
        futs.push(Box::pin(async move {
            let res = result_rx
                .await
                .map_err(|_| "subagent result channel dropped".to_string());
            (desc, res)
        }));
    }

    let mut out = early;
    let mut heartbeat = tokio::time::interval(COUNCIL_HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;

    while !futs.is_empty() {
        tokio::select! {
            biased;
            Some((desc, res)) = futs.next() => {
                let role = short_role(&desc);
                outstanding.retain(|r| r != &role);
                match &res {
                    Ok(r) if r.success => {
                        let thesis = extract_thesis(&r.output);
                        actor
                            .emit_agent_text(&format!("✓ **{role}** — {thesis}"))
                            .await;
                    }
                    Ok(r) => {
                        let err = r.error.clone().unwrap_or_else(|| "unknown".into());
                        actor
                            .emit_agent_text(&format!("✗ **{role}** failed — {err}"))
                            .await;
                    }
                    Err(e) => {
                        actor
                            .emit_agent_text(&format!("✗ **{role}** error — {e}"))
                            .await;
                    }
                }
                out.push((desc, res));
            }
            _ = heartbeat.tick(), if !futs.is_empty() => {
                if outstanding.is_empty() {
                    continue;
                }
                actor
                    .emit_agent_text(&format!(
                        "…still waiting on **{}** (I'm watching the board).",
                        outstanding.join(" · ")
                    ))
                    .await;
            }
        }
    }
    out
}

fn build_stage_prompt(
    stage: &Stage,
    user_text: &str,
    prior: &[(String, String)],
    captain_direction: &str,
) -> String {
    let mut s = String::new();
    s.push_str(stage.system_lens);
    s.push_str("\n\n## User request\n\n");
    s.push_str(user_text);
    if !captain_direction.trim().is_empty() {
        s.push_str("\n\n## Captain brief (follow this)\n\n");
        s.push_str(&captain_direction.chars().take(4_000).collect::<String>());
        s.push('\n');
    }
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
         - **Handoff:** what the next stage / captain must know\n",
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

/// Fallback when the captain model call fails — board dump, not a real judgment.
fn synthesize_board_dump(user_text: &str, prior: &[(String, String)]) -> String {
    let mut out = String::from("# ◈ HEAVY RESULT (board dump — captain synthesis unavailable)\n\n");
    out.push_str("**Request:** ");
    out.push_str(user_text.trim());
    out.push_str(
        "\n\nRan parallel council + Research → Implement → Test. \
         Full worker traces live on the cards above.\n\n",
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

fn short_role(description: &str) -> String {
    if let Some(rest) = description.strip_prefix('[')
        && let Some(close) = rest.find(']')
    {
        let tag = rest[..close].trim();
        if let Some((_, role)) = tag.split_once('/') {
            return role.trim().to_string();
        }
        if !tag.is_empty() {
            return tag.to_string();
        }
    }
    description.chars().take(24).collect()
}

fn first_line(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(text.trim());
    let mut s: String = line.chars().take(max_chars).collect();
    if line.chars().count() > max_chars {
        s.push('…');
    }
    s
}

/// Pull a short thesis from council / stage output for live narration.
fn extract_thesis(body: &str) -> String {
    for line in body.lines() {
        let t = line.trim();
        let lower = t.to_ascii_lowercase();
        if lower.contains("thesis") && lower.contains(':') {
            if let Some(idx) = lower.find("thesis") {
                let after = &t[idx..];
                if let Some(colon) = after.find(':') {
                    let v = after[colon + 1..].trim().trim_start_matches('*').trim();
                    if !v.is_empty() {
                        return first_line(v, 280);
                    }
                }
            }
        }
    }
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("---"))
        .map(|l| first_line(l, 280))
        .unwrap_or_else(|| "(no thesis)".into())
}

fn extract_stage_summary(body: &str) -> String {
    for line in body.lines() {
        let t = line.trim();
        let lower = t.to_ascii_lowercase();
        if lower.contains("summary") && lower.contains(':') {
            if let Some(colon) = t.find(':') {
                let v = t[colon + 1..].trim().trim_start_matches('*').trim();
                if !v.is_empty() {
                    return first_line(v, 400);
                }
            }
        }
    }
    // First few non-empty lines.
    let mut out = String::new();
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()).take(4) {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
        if out.len() > 400 {
            break;
        }
    }
    if out.is_empty() {
        "(no summary)".into()
    } else {
        first_line(&out, 400)
    }
}
