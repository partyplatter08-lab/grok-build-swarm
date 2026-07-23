//! Multi-agent orchestration pipelines: Heavy · Swarm · Swarm Heavy.
//!
//! ## Product shape
//!
//! The **parent chat is the captain** — a real model that talks to the user.
//! Workers are real subagents (reliable parallel spawn). Visual status uses a
//! compact live board (`●/○/✓/✗`) instead of spamming "still waiting" lines.
//!
//! | Mode | Flow |
//! |------|------|
//! | **Heavy** | Council (parallel) → Research → Implement → Test → synthesize |
//! | **Swarm** | Map wave (parallel units) → Implement → Verify → synthesize |
//! | **Swarm Heavy** | Council → RIT (same as Heavy, SH branding) |

use std::collections::BTreeMap;
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

/// Min gap between board heartbeats (and only when still outstanding).
const BOARD_HEARTBEAT: Duration = Duration::from_secs(20);
/// Hard cap so a hung worker never floods the parent feed.
const MAX_HEARTBEATS: u32 = 3;

// ── Council (Heavy / Swarm Heavy) ───────────────────────────────────────────

struct CouncilMember {
    id_suffix: &'static str,
    role: &'static str,
    lens: &'static str,
}

const COUNCIL: &[CouncilMember] = &[
    CouncilMember {
        id_suffix: "analyst",
        role: "Analyst",
        lens: "You are the ANALYST. Structure the problem, goals, constraints, success criteria. Be precise; cite paths when relevant.",
    },
    CouncilMember {
        id_suffix: "skeptic",
        role: "Skeptic",
        lens: "You are the SKEPTIC. Attack weak assumptions, edge cases, failure modes, and overconfidence.",
    },
    CouncilMember {
        id_suffix: "explorer",
        role: "Explorer",
        lens: "You are the EXPLORER. Survey the codebase/context for alternatives and non-obvious approaches. Use tools freely (read-only).",
    },
    CouncilMember {
        id_suffix: "builder",
        role: "Builder",
        lens: "You are the BUILDER. Propose a concrete implementation plan (steps, files, risks). Do NOT implement yet.",
    },
];

// ── Swarm map units (parallel research wave) ────────────────────────────────

struct SwarmUnit {
    id_suffix: &'static str,
    title: &'static str,
    lens: &'static str,
    capability: CapMode,
    subagent_type: &'static str,
}

const SWARM_MAP: &[SwarmUnit] = &[
    SwarmUnit {
        id_suffix: "u1-diagnose",
        title: "diagnose",
        lens: "You are Swarm unit **diagnose**. Reproduce the issue, find the root cause in code, cite exact lines. Return Status/Summary/Evidence/Handoff. Do not implement.",
        capability: CapMode::ReadOnly,
        subagent_type: "explore",
    },
    SwarmUnit {
        id_suffix: "u2-edges",
        title: "edges",
        lens: "You are Swarm unit **edges**. List edge cases, regressions, and test cases that must pass after a fix. Do not implement.",
        capability: CapMode::ReadOnly,
        subagent_type: "explore",
    },
    SwarmUnit {
        id_suffix: "u3-plan",
        title: "plan",
        lens: "You are Swarm unit **plan**. Propose the minimal correct fix (files + steps). Do not implement.",
        capability: CapMode::ReadOnly,
        subagent_type: "explore",
    },
    SwarmUnit {
        id_suffix: "u4-context",
        title: "context",
        lens: "You are Swarm unit **context**. Gather related code, docs, and constraints the implementer needs. Do not implement.",
        capability: CapMode::ReadOnly,
        subagent_type: "explore",
    },
];

// ── RIT stages (Heavy sequential) ───────────────────────────────────────────

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
You are RESEARCH for the multi-agent captain.
- Follow the captain brief.
- Deepen investigation with tools; do NOT implement.",
    },
    Stage {
        id_suffix: "implement",
        description: "[Pipeline/Implement] make the code changes",
        subagent_type: "general-purpose",
        capability: CapMode::All,
        system_lens: "\
You are IMPLEMENT for the multi-agent captain.
- Follow the captain brief and prior board.
- Make real edits; minimize scope; summarize changes.",
    },
    Stage {
        id_suffix: "test",
        description: "[Pipeline/Test] verify the implementation",
        subagent_type: "explore",
        capability: CapMode::Execute,
        system_lens: "\
You are TEST/VERIFY for the multi-agent captain.
- Run checks/tests when possible; report pass/fail and residual risks.",
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
    /// Any multi-agent effort mode that uses a code-enforced pipeline.
    pub(super) async fn should_run_heavy_pipeline(&self) -> bool {
        matches!(
            self.current_orchestration_mode().await,
            OrchestrationMode::Heavy | OrchestrationMode::Swarm | OrchestrationMode::SwarmHeavy
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
                "orchestration_pipeline: subagent_event_tx missing"
            );
            self.emit_agent_text(&format!(
                "{} pipeline unavailable (subagent coordinator not ready).",
                mode.mark()
            ))
            .await;
            return commands::ok_end_turn(0, None);
        };

        let session_id = self.session_info.id.0.to_string();
        let id_prefix = format!(
            "{}-{}",
            &session_id.replace('-', "")[..8.min(session_id.len())],
            &prompt_id.replace('-', "")[..8.min(prompt_id.len())]
        );

        // Visual open: mode banner box (always — looks right in the TUI).
        if let Some(banner) = mode.open_banner() {
            self.emit_agent_text(banner).await;
        }

        match mode {
            OrchestrationMode::Swarm => {
                self.run_swarm_pipeline(prompt_id, user_text, &event_tx, &session_id, &id_prefix)
                    .await
            }
            OrchestrationMode::Heavy => {
                // Collaborative council (argue) → Research → Implement → Test
                self.run_council_rit_pipeline(
                    mode,
                    prompt_id,
                    user_text,
                    &event_tx,
                    &session_id,
                    &id_prefix,
                )
                .await
            }
            OrchestrationMode::SwarmHeavy => {
                // Full stack: collaborative Heavy council → Swarm fan-out → H2 verify
                self.run_swarm_heavy_pipeline(
                    prompt_id,
                    user_text,
                    &event_tx,
                    &session_id,
                    &id_prefix,
                )
                .await
            }
            OrchestrationMode::Normal => commands::ok_end_turn(0, None),
        }
    }

    // ── Swarm: map → implement → verify ─────────────────────────────────

    async fn run_swarm_pipeline(
        self: &Arc<Self>,
        prompt_id: &str,
        user_text: &str,
        event_tx: &tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
        session_id: &str,
        id_prefix: &str,
    ) -> PromptTurnResult {
        tracing::info!(session_id = %session_id, "swarm_pipeline: map→reduce start");

        let open = self
            .captain_speak(
                CaptainPhase::OpenSwarm,
                user_text,
                &[],
                None,
                1_000,
            )
            .await;
        if let Some(text) = open {
            self.emit_agent_text(&text).await;
        } else {
            self.emit_agent_text(&format!(
                "⬡ **SWARM** map→reduce — I'll fan out independent units, \
                 then implement + verify.\n\n**Goal:** {}",
                first_line(user_text, 200)
            ))
            .await;
        }

        let mut prior: Vec<(String, String)> = Vec::new();

        // Wave 1 — parallel map
        self.emit_agent_text(
            "── **S2 fan-out · wave 1** · launching 4 map units ──\n\
             `diagnose` · `edges` · `plan` · `context`  (parallel, read-only cards)",
        )
        .await;

        let map_results = run_parallel_units(
            self,
            event_tx,
            session_id,
            prompt_id,
            id_prefix,
            user_text,
            "⬡ SWARM",
            SWARM_MAP
                .iter()
                .map(|u| ParallelSpec {
                    id: format!("swarm-{}-{}", u.id_suffix, id_prefix),
                    description: format!("[Swarm/{}] {}", u.id_suffix, u.title),
                    label: u.title.to_string(),
                    prompt: format!(
                        "{lens}\n\n## User request\n\n{user}\n\n## Return format\n\
                         - **Status:** success | partial | blocked\n\
                         - **Summary:** 3–8 bullets\n\
                         - **Evidence:** paths / facts\n\
                         - **Handoff:** what implement must know\n",
                        lens = u.lens,
                        user = user_text
                    ),
                    subagent_type: u.subagent_type,
                    capability: u.capability,
                    background: true,
                })
                .collect(),
        )
        .await;

        let mut map_digest = String::from("## Swarm map wave\n\n");
        for (label, result) in &map_results {
            let body = result_body(result);
            map_digest.push_str("### ");
            map_digest.push_str(label);
            map_digest.push_str("\n\n");
            map_digest.push_str(&body.chars().take(8_000).collect::<String>());
            map_digest.push_str("\n\n");
        }
        prior.push(("swarm-map".into(), map_digest));

        let brief = self
            .captain_speak(
                CaptainPhase::AfterMap,
                user_text,
                &prior,
                None,
                1_400,
            )
            .await;
        if let Some(ref text) = brief {
            self.emit_agent_text(text).await;
        }
        let captain_direction = brief.unwrap_or_default();

        // Wave 2 — implement (single writer)
        self.emit_agent_text("── **S2 fan-out · wave 2** · implement ──").await;
        let impl_prompt = build_stage_prompt(
            &STAGES[1],
            user_text,
            &prior,
            &captain_direction,
        );
        let impl_id = format!("swarm-impl-{}", id_prefix);
        match spawn_and_wait(
            event_tx,
            &impl_id,
            session_id,
            Some(prompt_id.to_string()),
            "[Swarm/impl] apply the fix",
            "general-purpose",
            CapMode::All,
            impl_prompt,
        )
        .await
        {
            Ok(r) => {
                prior.push(("implement".into(), result_body(&Ok(r.clone()))));
                let reaction = self
                    .captain_speak(
                        CaptainPhase::AfterStage {
                            stage: "implement",
                            phase_n: 2,
                        },
                        user_text,
                        &prior,
                        Some(&extract_stage_summary(&result_body(&Ok(r)))),
                        900,
                    )
                    .await;
                if let Some(t) = reaction {
                    self.emit_agent_text(&t).await;
                }
            }
            Err(e) => {
                prior.push(("implement".into(), format!("[error] {e}")));
                self.emit_agent_text(&format!("✗ implement unit failed: {e}")).await;
            }
        }

        // Wave 3 — verify
        self.emit_agent_text("── **S2 fan-out · wave 3** · verify ──").await;
        let test_prompt = build_stage_prompt(
            &STAGES[2],
            user_text,
            &prior,
            &captain_direction,
        );
        let test_id = format!("swarm-test-{}", id_prefix);
        match spawn_and_wait(
            event_tx,
            &test_id,
            session_id,
            Some(prompt_id.to_string()),
            "[Swarm/verify] run checks",
            "explore",
            CapMode::Execute,
            test_prompt,
        )
        .await
        {
            Ok(r) => {
                prior.push(("verify".into(), result_body(&Ok(r))));
            }
            Err(e) => {
                prior.push(("verify".into(), format!("[error] {e}")));
            }
        }

        let final_answer = self
            .captain_speak(
                CaptainPhase::FinalSwarm,
                user_text,
                &prior,
                Some(&captain_direction),
                4_096,
            )
            .await
            .unwrap_or_else(|| synthesize_board_dump(user_text, &prior, "⬡ SWARM RESULT"));
        self.emit_agent_text(&final_answer).await;
        commands::ok_end_turn(0, None)
    }

    // ── Swarm Heavy: collaborative council → swarm fan-out → H2 verify ───
    //
    // This is the full stack the product promises:
    //   H1  first-pass council (same problem, many lenses)
    //   H1b cross-check — each member *sees the others* and argues
    //   S2  swarm map fan-out (many independent units)
    //   S3  implement
    //   H2  re-council verify (Verifier + Skeptic attack the result)
    //   F   captain final

    async fn run_swarm_heavy_pipeline(
        self: &Arc<Self>,
        prompt_id: &str,
        user_text: &str,
        event_tx: &tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
        session_id: &str,
        id_prefix: &str,
    ) -> PromptTurnResult {
        tracing::info!(
            session_id = %session_id,
            "swarm_heavy_pipeline: H1 collaborate → S2 fan-out → H2 verify"
        );

        let open = self
            .captain_speak(CaptainPhase::OpenSwarmHeavy, user_text, &[], None, 1_200)
            .await;
        if let Some(text) = open {
            self.emit_agent_text(&text).await;
        } else {
            self.emit_agent_text(&format!(
                "⬢ **SWARM HEAVY** — collaborative council first (they argue), \
                 then a swarm fan-out of workers, then verify.\n\n**Goal:** {}",
                first_line(user_text, 200)
            ))
            .await;
        }

        let mut prior: Vec<(String, String)> = Vec::new();

        // ── H1 + H1b: collaborative Heavy council ───────────────────────
        let (council_digest, board_index, _) = self
            .run_collaborative_council(
                event_tx,
                session_id,
                prompt_id,
                id_prefix,
                user_text,
                "SH/H1",
                "⬢ SWARM HEAVY · H1 council",
            )
            .await;
        prior.push(("h1-collaborative-council".into(), council_digest));

        let captain_brief = self
            .captain_speak(
                CaptainPhase::AfterCollaborativeCouncil,
                user_text,
                &prior,
                Some(&board_index),
                2_000,
            )
            .await;
        if let Some(ref text) = captain_brief {
            self.emit_agent_text(text).await;
        } else {
            self.emit_agent_text(&format!(
                "Council agreed/disputed:\n\n{board_index}\n\n\
                 → Swarm fan-out next (many workers on units)."
            ))
            .await;
        }
        let captain_direction = captain_brief.unwrap_or_default();

        // ── S2: Swarm map fan-out (lots of agents) ──────────────────────
        self.emit_agent_text(
            "── **SH/S2 swarm fan-out · wave 1** · launching 4 map units ──\n\
             From the council plan: `diagnose` · `edges` · `plan` · `context`\n\
             (parallel — open cards for full streams)",
        )
        .await;

        let map_results = run_parallel_units(
            self,
            event_tx,
            session_id,
            prompt_id,
            id_prefix,
            user_text,
            "⬢ SH/S2 map",
            SWARM_MAP
                .iter()
                .map(|u| ParallelSpec {
                    id: format!("sh-s-{}-{}", u.id_suffix, id_prefix),
                    description: format!("[SH/S·{}] {}", u.id_suffix, u.title),
                    label: u.title.to_string(),
                    prompt: format!(
                        "{lens}\n\n## User request\n\n{user}\n\n\
                         ## Captain + collaborative council brief (follow this)\n\n{brief}\n\n\
                         ## Return format\n\
                         - **Status:** success | partial | blocked\n\
                         - **Summary:** 3–8 bullets\n\
                         - **Evidence:** paths / facts\n\
                         - **Handoff:** what implement must know\n",
                        lens = u.lens,
                        user = user_text,
                        brief = captain_direction.chars().take(4_000).collect::<String>(),
                    ),
                    subagent_type: u.subagent_type,
                    capability: u.capability,
                    background: true,
                })
                .collect(),
        )
        .await;

        let mut map_digest = String::from("## SH/S2 swarm map wave\n\n");
        for (label, result) in &map_results {
            let body = result_body(result);
            map_digest.push_str("### ");
            map_digest.push_str(label);
            map_digest.push_str("\n\n");
            map_digest.push_str(&body.chars().take(8_000).collect::<String>());
            map_digest.push_str("\n\n");
        }
        prior.push(("sh-s2-map".into(), map_digest));

        let after_map = self
            .captain_speak(
                CaptainPhase::AfterMap,
                user_text,
                &prior,
                Some(&captain_direction),
                1_400,
            )
            .await;
        if let Some(ref text) = after_map {
            self.emit_agent_text(text).await;
        }
        let mut direction = after_map.unwrap_or(captain_direction);

        // ── S3 implement ────────────────────────────────────────────────
        self.emit_agent_text("── **SH/S3 implement** · one writer from the swarm plan ──")
            .await;
        let impl_prompt = build_stage_prompt(&STAGES[1], user_text, &prior, &direction);
        match spawn_and_wait(
            event_tx,
            &format!("sh-impl-{}", id_prefix),
            session_id,
            Some(prompt_id.to_string()),
            "[SH/S·impl] apply the fix",
            "general-purpose",
            CapMode::All,
            impl_prompt,
        )
        .await
        {
            Ok(r) => {
                let body = result_body(&Ok(r));
                prior.push(("implement".into(), body.clone()));
                if let Some(t) = self
                    .captain_speak(
                        CaptainPhase::AfterStage {
                            stage: "implement",
                            phase_n: 3,
                        },
                        user_text,
                        &prior,
                        Some(&extract_stage_summary(&body)),
                        900,
                    )
                    .await
                {
                    self.emit_agent_text(&t).await;
                    direction = t;
                }
            }
            Err(e) => {
                prior.push(("implement".into(), format!("[error] {e}")));
                self.emit_agent_text(&format!("✗ SH/S3 implement failed: {e}")).await;
            }
        }

        // ── H2: re-council verify (collaborative attack on the result) ──
        self.emit_agent_text(
            "── **SH/H2 re-council** · Verifier + Skeptic attack the result ──\n\
             Same problem, two lenses on the *actual* outcome (argue if needed).",
        )
        .await;

        let impl_summary = prior
            .iter()
            .find(|(n, _)| n == "implement")
            .map(|(_, b)| b.as_str())
            .unwrap_or("(no implement output)");
        let h2_specs = vec![
            ParallelSpec {
                id: format!("sh-h2-verifier-{}", id_prefix),
                description: "[SH/H2·Verifier] verify the work".into(),
                label: "Verifier".into(),
                prompt: format!(
                    "You are the VERIFIER on a Swarm Heavy H2 re-council.\n\
                     Run/check evidence that the user's goal was met. Be concrete.\n\n\
                     ## User request\n\n{user}\n\n## Implement output\n\n{impl_out}\n\n\
                     ## Captain direction\n\n{dir}\n\n\
                     Return Thesis / Evidence / Residual risks / Verdict: pass|fail|partial.",
                    user = user_text,
                    impl_out = impl_summary.chars().take(10_000).collect::<String>(),
                    dir = direction.chars().take(3_000).collect::<String>(),
                ),
                subagent_type: "explore",
                capability: CapMode::Execute,
                background: true,
            },
            ParallelSpec {
                id: format!("sh-h2-skeptic-{}", id_prefix),
                description: "[SH/H2·Skeptic] attack weak claims".into(),
                label: "Skeptic".into(),
                prompt: format!(
                    "You are the SKEPTIC on a Swarm Heavy H2 re-council.\n\
                     Attack weak claims in the implement result. What could still be wrong?\n\n\
                     ## User request\n\n{user}\n\n## Implement output\n\n{impl_out}\n\n\
                     Return Thesis / Attacks / What would falsify PASS / Residual risks.",
                    user = user_text,
                    impl_out = impl_summary.chars().take(10_000).collect::<String>(),
                ),
                subagent_type: "explore",
                capability: CapMode::ReadOnly,
                background: true,
            },
        ];
        let h2_results = run_parallel_units(
            self,
            event_tx,
            session_id,
            prompt_id,
            id_prefix,
            user_text,
            "⬢ SH/H2 verify council",
            h2_specs,
        )
        .await;
        let mut h2_digest = String::from("## SH/H2 re-council\n\n");
        for (label, result) in &h2_results {
            h2_digest.push_str("### ");
            h2_digest.push_str(label);
            h2_digest.push_str("\n\n");
            h2_digest.push_str(&result_body(result).chars().take(8_000).collect::<String>());
            h2_digest.push_str("\n\n");
        }
        prior.push(("h2-verify-council".into(), h2_digest));

        let final_answer = self
            .captain_speak(
                CaptainPhase::FinalSwarmHeavy,
                user_text,
                &prior,
                Some(&direction),
                4_096,
            )
            .await
            .unwrap_or_else(|| {
                synthesize_board_dump(user_text, &prior, "⬢ SWARM HEAVY RESULT")
            });
        self.emit_agent_text(&final_answer).await;
        commands::ok_end_turn(0, None)
    }

    // ── Heavy: collaborative council + RIT ──────────────────────────────

    async fn run_council_rit_pipeline(
        self: &Arc<Self>,
        mode: OrchestrationMode,
        prompt_id: &str,
        user_text: &str,
        event_tx: &tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
        session_id: &str,
        id_prefix: &str,
    ) -> PromptTurnResult {
        tracing::info!(
            session_id = %session_id,
            mode = %mode,
            "heavy_pipeline: collaborative council + RIT"
        );

        let open = self
            .captain_speak(CaptainPhase::Open, user_text, &[], None, 1_200)
            .await;
        if let Some(text) = open {
            self.emit_agent_text(&text).await;
        } else {
            self.emit_agent_text(&format!(
                "{}\n\nI'm the **captain** — a collaborative council (they argue), \
                 then research → implement → test.\n\n**Goal:** {}",
                mode.brand(),
                first_line(user_text, 200),
            ))
            .await;
        }

        let mut prior: Vec<(String, String)> = Vec::new();

        let (council_digest, board_index, _) = self
            .run_collaborative_council(
                event_tx,
                session_id,
                prompt_id,
                id_prefix,
                user_text,
                "Council",
                "◈ HEAVY · council",
            )
            .await;
        prior.push(("collaborative-council".into(), council_digest));

        let captain_brief = self
            .captain_speak(
                CaptainPhase::AfterCollaborativeCouncil,
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
                "Council board (after debate):\n\n{board_index}\n\n→ Research → Implement → Test."
            ))
            .await;
        }
        let mut captain_direction = captain_brief.unwrap_or_default();

        for (idx, stage) in STAGES.iter().enumerate() {
            let n = idx + 1;
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
                    "── **Phase {n}/3 · {}** ──",
                    stage.id_suffix
                ))
                .await;
            }

            let prompt = build_stage_prompt(stage, user_text, &prior, &captain_direction);
            let child_id = format!("pipe-{}-{}", stage.id_suffix, id_prefix);

            match spawn_and_wait(
                event_tx,
                &child_id,
                session_id,
                Some(prompt_id.to_string()),
                stage.description,
                stage.subagent_type,
                stage.capability,
                prompt,
            )
            .await
            {
                Ok(result) => {
                    let out = result_body(&Ok(result));
                    prior.push((stage.id_suffix.to_string(), out.clone()));
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
                        captain_direction = text;
                    }
                }
                Err(e) => {
                    prior.push((stage.id_suffix.to_string(), format!("[error] {e}")));
                    self.emit_agent_text(&format!(
                        "✗ Phase {n} ({}) failed: {e}",
                        stage.id_suffix
                    ))
                    .await;
                }
            }
        }

        let final_answer = self
            .captain_speak(
                CaptainPhase::Final,
                user_text,
                &prior,
                Some(&captain_direction),
                4_096,
            )
            .await
            .unwrap_or_else(|| synthesize_board_dump(user_text, &prior, "◈ HEAVY RESULT"));
        self.emit_agent_text(&final_answer).await;
        commands::ok_end_turn(0, None)
    }

    /// H1 first pass (independent lenses) + H1b cross-check (they see each
    /// other and argue). Returns (full digest, thesis board index, pass2 bodies).
    async fn run_collaborative_council(
        &self,
        event_tx: &tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
        session_id: &str,
        prompt_id: &str,
        id_prefix: &str,
        user_text: &str,
        tag: &str,
        board_title: &str,
    ) -> (String, String, Vec<(String, String)>) {
        // ── Pass 1: independent first takes ─────────────────────────────
        self.emit_agent_text(&format!(
            "── **{tag} pass 1** · same problem, 4 independent lenses ──\n\
             Analyst · Skeptic · Explorer · Builder — first takes (no peeking yet)."
        ))
        .await;

        let pass1_specs: Vec<ParallelSpec> = COUNCIL
            .iter()
            .map(|m| ParallelSpec {
                id: format!("council-p1-{}-{}", m.id_suffix, id_prefix),
                description: format!("[{tag}·{}] first pass", m.role),
                label: m.role.to_string(),
                prompt: format!(
                    "{lens}\n\n## User request\n\n{user}\n\n## Return format\n\
                     - **Thesis** (1–3 sentences)\n\
                     - **Argument** (bullets)\n\
                     - **Evidence** (paths / facts)\n\
                     - **Risks**\n\
                     - **What to check next**\n\
                     Do NOT assume others' views — this is your independent first pass.",
                    lens = m.lens,
                    user = user_text
                ),
                subagent_type: "explore",
                capability: CapMode::ReadOnly,
                background: true,
            })
            .collect();

        let pass1 = run_parallel_units(
            self,
            event_tx,
            session_id,
            prompt_id,
            id_prefix,
            user_text,
            &format!("{board_title} · pass 1"),
            pass1_specs,
        )
        .await;

        let mut pass1_bodies: Vec<(String, String)> = Vec::new();
        let mut board_for_cross = String::from("## First-pass council board (anonymized peers)\n\n");
        for (label, result) in &pass1 {
            let body = result_body(result);
            pass1_bodies.push((label.clone(), body.clone()));
            board_for_cross.push_str("### ");
            board_for_cross.push_str(label);
            board_for_cross.push_str("\n\n");
            board_for_cross.push_str(&body.chars().take(6_000).collect::<String>());
            board_for_cross.push_str("\n\n");
        }

        // ── Pass 2: cross-check / debate (they see each other) ──────────
        self.emit_agent_text(&format!(
            "── **{tag} pass 2 · cross-check / debate** ──\n\
             Same four agents now **see each other's theses**. They must agree, \
             dissent, and update — this is the collaborative Heavy part."
        ))
        .await;

        let pass2_specs: Vec<ParallelSpec> = COUNCIL
            .iter()
            .map(|m| {
                let own = pass1_bodies
                    .iter()
                    .find(|(l, _)| l == m.role)
                    .map(|(_, b)| b.as_str())
                    .unwrap_or("");
                ParallelSpec {
                    id: format!("council-p2-{}-{}", m.id_suffix, id_prefix),
                    description: format!("[{tag}·{}] cross-check debate", m.role),
                    label: format!("{}′", m.role),
                    prompt: format!(
                        "{lens}\n\nYou already wrote a first-pass take. Now you see the FULL board \
                         from the other council members. **Argue.**\n\
                         - Cite who you agree/disagree with and why\n\
                         - Attack weak claims; strengthen yours with evidence\n\
                         - Update your final thesis after the debate\n\n\
                         ## User request\n\n{user}\n\n\
                         ## Your first-pass output\n\n{own}\n\n\
                         ## Full council board (peers)\n\n{board}\n\n\
                         ## Return format\n\
                         - **Agreements** (who/what)\n\
                         - **Disagreements** (who/what + why they're wrong or you're updating)\n\
                         - **Updated Thesis** (1–3 sentences after debate)\n\
                         - **Argument** (bullets)\n\
                         - **Evidence**\n\
                         - **Risks**\n\
                         - **What the captain / next workers must do**\n",
                        lens = m.lens,
                        user = user_text,
                        own = own.chars().take(5_000).collect::<String>(),
                        board = board_for_cross.chars().take(14_000).collect::<String>(),
                    ),
                    subagent_type: "explore",
                    capability: CapMode::ReadOnly,
                    background: true,
                }
            })
            .collect();

        let pass2 = run_parallel_units(
            self,
            event_tx,
            session_id,
            prompt_id,
            id_prefix,
            user_text,
            &format!("{board_title} · debate"),
            pass2_specs,
        )
        .await;

        let mut digest = String::from("## Collaborative council\n\n### Pass 1 (independent)\n\n");
        digest.push_str(&board_for_cross);
        digest.push_str("\n### Pass 2 (cross-check / debate)\n\n");
        let mut theses: Vec<(String, String)> = Vec::new();
        let mut pass2_bodies: Vec<(String, String)> = Vec::new();
        for (label, result) in &pass2 {
            let body = result_body(result);
            // strip prime for index
            let role = label.trim_end_matches('′').to_string();
            theses.push((role.clone(), extract_thesis(&body)));
            pass2_bodies.push((role, body.clone()));
            digest.push_str("### ");
            digest.push_str(label);
            digest.push_str("\n\n");
            digest.push_str(&body.chars().take(10_000).collect::<String>());
            digest.push_str("\n\n");
        }
        let board_index = theses
            .iter()
            .map(|(r, t)| format!("- **{r}:** {t}"))
            .collect::<Vec<_>>()
            .join("\n");

        (digest, board_index, pass2_bodies)
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
                tracing::warn!(error = %e, phase = %phase, "captain prepare failed");
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

        use crate::sampling::{ConversationItem, ConversationRequest};
        let request = ConversationRequest::from_items(vec![
            ConversationItem::system(phase.system_prompt()),
            ConversationItem::user(phase.user_prompt(user_text, prior, extra)),
        ])
        .with_model(model)
        .with_max_output_tokens(max_tokens);

        match sampling_client.conversation_collect(request).await {
            Ok(response) => {
                let t = response.assistant_text();
                let trimmed = t.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, phase = %phase, "captain call failed");
                None
            }
        }
    }
}

// ── Parallel unit runner + live board ───────────────────────────────────────

struct ParallelSpec {
    id: String,
    description: String,
    label: String,
    prompt: String,
    subagent_type: &'static str,
    capability: CapMode,
    background: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cell {
    Pending,
    Running,
    Ok,
    Fail,
}

impl Cell {
    fn glyph(self) -> &'static str {
        match self {
            Self::Pending => "○",
            Self::Running => "●",
            Self::Ok => "✓",
            Self::Fail => "✗",
        }
    }
}

/// Render a compact multi-line board for the TUI (one message, not spam).
fn render_board(title: &str, cells: &BTreeMap<String, (Cell, String)>) -> String {
    let mut s = format!("**{title}**\n");
    for (label, (cell, note)) in cells {
        if note.is_empty() {
            s.push_str(&format!("  {} {}\n", cell.glyph(), label));
        } else {
            s.push_str(&format!("  {} {} — {}\n", cell.glyph(), label, note));
        }
    }
    s
}

async fn run_parallel_units(
    actor: &SessionActor,
    event_tx: &tokio::sync::mpsc::UnboundedSender<SubagentEvent>,
    parent_session_id: &str,
    prompt_id: &str,
    _id_prefix: &str,
    _user_text: &str,
    board_title: &str,
    specs: Vec<ParallelSpec>,
) -> Vec<(String, Result<SubagentResult, String>)> {
    let mut futs: FuturesUnordered<
        std::pin::Pin<
            Box<dyn std::future::Future<Output = (String, Result<SubagentResult, String>)> + Send>,
        >,
    > = FuturesUnordered::new();
    let mut cells: BTreeMap<String, (Cell, String)> = BTreeMap::new();
    let mut early: Vec<(String, Result<SubagentResult, String>)> = Vec::new();

    for spec in specs {
        let label = spec.label.clone();
        cells.insert(label.clone(), (Cell::Running, String::new()));
        let (result_tx, result_rx) = oneshot::channel();
        let request = SubagentRequest {
            id: spec.id,
            prompt: spec.prompt,
            description: spec.description,
            subagent_type: spec.subagent_type.to_string(),
            parent_session_id: parent_session_id.to_string(),
            parent_prompt_id: Some(prompt_id.to_string()),
            resume_from: None,
            cwd: None,
            runtime_overrides: SubagentRuntimeOverrides {
                capability_mode: Some(spec.capability),
                reasoning_effort: Some("xhigh".into()),
                ..Default::default()
            },
            run_in_background: spec.background,
            surface_completion: true,
            fork_context: false,
            result_tx,
        };
        if event_tx
            .send(SubagentEvent::Spawn(Box::new(request)))
            .is_err()
        {
            cells.insert(label.clone(), (Cell::Fail, "spawn channel closed".into()));
            early.push((label, Err("subagent coordinator channel closed".into())));
            continue;
        }
        futs.push(Box::pin(async move {
            let res = result_rx
                .await
                .map_err(|_| "subagent result channel dropped".to_string());
            (label, res)
        }));
    }

    // Initial board (visual: all ● running).
    actor
        .emit_agent_text(&render_board(board_title, &cells))
        .await;

    let mut out = early;
    let mut heartbeat = tokio::time::interval(BOARD_HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut heartbeats: u32 = 0;

    while !futs.is_empty() {
        tokio::select! {
            biased;
            Some((label, res)) = futs.next() => {
                match &res {
                    Ok(r) if r.success => {
                        let note = extract_thesis(&r.output);
                        cells.insert(label.clone(), (Cell::Ok, first_line(&note, 100)));
                    }
                    Ok(r) => {
                        let err = r.error.clone().unwrap_or_else(|| "failed".into());
                        cells.insert(label.clone(), (Cell::Fail, first_line(&err, 80)));
                    }
                    Err(e) => {
                        cells.insert(label.clone(), (Cell::Fail, first_line(e, 80)));
                    }
                }
                // One clean board redraw per landing — looks right in the TUI.
                actor
                    .emit_agent_text(&render_board(board_title, &cells))
                    .await;
                out.push((label, res));
            }
            _ = heartbeat.tick(), if !futs.is_empty() && heartbeats < MAX_HEARTBEATS => {
                heartbeats += 1;
                // Quiet heartbeat: re-post board only (no "still waiting" spam).
                actor
                    .emit_agent_text(&render_board(
                        &format!("{board_title} · waiting"),
                        &cells,
                    ))
                    .await;
            }
        }
    }
    out
}

// ── Captain phases ──────────────────────────────────────────────────────────

enum CaptainPhase<'a> {
    Open,
    OpenSwarm,
    OpenSwarmHeavy,
    AfterCouncil,
    AfterCollaborativeCouncil,
    AfterMap,
    BeforeStage { stage: &'a str, phase_n: usize },
    AfterStage { stage: &'a str, phase_n: usize },
    Final,
    FinalSwarm,
    FinalSwarmHeavy,
}

impl std::fmt::Display for CaptainPhase<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open => write!(f, "open"),
            Self::OpenSwarm => write!(f, "open_swarm"),
            Self::OpenSwarmHeavy => write!(f, "open_swarm_heavy"),
            Self::AfterCouncil => write!(f, "after_council"),
            Self::AfterCollaborativeCouncil => write!(f, "after_collab_council"),
            Self::AfterMap => write!(f, "after_map"),
            Self::BeforeStage { stage, .. } => write!(f, "before_{stage}"),
            Self::AfterStage { stage, .. } => write!(f, "after_{stage}"),
            Self::Final => write!(f, "final"),
            Self::FinalSwarm => write!(f, "final_swarm"),
            Self::FinalSwarmHeavy => write!(f, "final_swarm_heavy"),
        }
    }
}

impl CaptainPhase<'_> {
    fn system_prompt(&self) -> String {
        let base = "\
You are the multi-agent **captain** in the parent chat. Workers run on cards; \
you talk to the user in first person — decisive, concrete, not a CI log.\n\
Do not dump worker transcripts. Do not invent completed work.\n";
        let phase = match self {
            Self::Open => "\
PHASE OPEN (Heavy): 2 short paragraphs — restate goal, say you'll run a \
*collaborative* 4-lens council (first independent, then they debate each other), \
then research/implement/test. No fake completion.",
            Self::OpenSwarm => "\
PHASE OPEN (Swarm): 2 short paragraphs — restate goal, say you'll map→reduce with \
parallel units then implement+verify. No fake completion.",
            Self::OpenSwarmHeavy => "\
PHASE OPEN (Swarm Heavy): 2 short paragraphs — restate goal. Promise BOTH:\n\
(1) Heavy-style collaborative council that argues among themselves, then\n\
(2) a Swarm fan-out of many workers, then H2 verify council.\n\
No fake completion.",
            Self::AfterCouncil => "\
PHASE AFTER COUNCIL: judge the board, note agreement/conflict, write a short \
captain brief for Research→Implement→Test.",
            Self::AfterCollaborativeCouncil => "\
PHASE AFTER COLLABORATIVE COUNCIL: The council did two passes — independent then \
*debate* (they saw each other). Summarize agreements vs real fights. Write a \
clear captain brief that a swarm of implementers can execute. Name the units \
you want fanned out if useful.",
            Self::AfterMap => "\
PHASE AFTER MAP WAVE: reduce map-unit findings into a clear implement brief.",
            Self::BeforeStage { stage, phase_n } => {
                return format!(
                    "{base}\nPHASE BEFORE {phase_n} ({stage}): 2–4 sentences on what \
                     you are sending a worker to do and why."
                );
            }
            Self::AfterStage { stage, phase_n } => {
                return format!(
                    "{base}\nPHASE AFTER {phase_n} ({stage}): react tightly — what \
                     matters, what changes next. If failed, how you recover."
                );
            }
            Self::Final => "\
PHASE FINAL: complete user-facing answer. Prefer title `# ◈ HEAVY RESULT`. \
Answer the request; cite evidence; residual risks brief.",
            Self::FinalSwarm => "\
PHASE FINAL: complete user-facing answer. Prefer title `# ⬡ SWARM RESULT`. \
Answer the request; attribute key findings to units when useful.",
            Self::FinalSwarmHeavy => "\
PHASE FINAL: complete user-facing answer. Prefer title `# ⬢ SWARM HEAVY RESULT`.\n\
You had: collaborative council debate + swarm map + implement + H2 verify council.\n\
Answer the user fully; note where H2 disagreed with implement if relevant.",
        };
        format!("{base}\n{phase}")
    }

    fn user_prompt(
        &self,
        user_text: &str,
        prior: &[(String, String)],
        extra: Option<&str>,
    ) -> String {
        let mut s = format!("## User request\n\n{}\n\n", user_text.trim());
        if !prior.is_empty() {
            s.push_str("## Board (for you)\n\n");
            let cap = match self {
                Self::Final | Self::FinalSwarm | Self::FinalSwarmHeavy => 8_000,
                Self::AfterCouncil | Self::AfterCollaborativeCouncil | Self::AfterMap => 5_000,
                _ => 3_500,
            };
            for (name, body) in prior {
                s.push_str("### ");
                s.push_str(name);
                s.push_str("\n\n");
                s.push_str(&body.chars().take(cap).collect::<String>());
                s.push_str("\n\n");
            }
        }
        if let Some(extra) = extra {
            s.push_str("## Extra\n\n");
            s.push_str(extra.trim());
            s.push_str("\n\n");
        }
        s.push_str("Respond for this phase now.");
        s
    }
}

// ── Shared helpers ──────────────────────────────────────────────────────────

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
        s.push_str("\n\n## Captain brief\n\n");
        s.push_str(&captain_direction.chars().take(4_000).collect::<String>());
    }
    if !prior.is_empty() {
        s.push_str("\n\n## Prior board\n");
        for (name, out) in prior {
            s.push_str("\n### ");
            s.push_str(name);
            s.push_str("\n\n");
            s.push_str(&out.chars().take(14_000).collect::<String>());
            s.push('\n');
        }
    }
    s.push_str(
        "\n\n## Return format\n\
         - **Status:** success | partial | blocked\n\
         - **Summary:** 3–8 bullets\n\
         - **Evidence:** paths, commands\n\
         - **Handoff:** next stage / captain\n",
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

fn result_body(result: &Result<SubagentResult, String>) -> String {
    match result {
        Ok(r) if r.success => r.output.to_string(),
        Ok(r) => format!(
            "[failed] {}",
            r.error.clone().unwrap_or_else(|| "unknown".into())
        ),
        Err(e) => format!("[error] {e}"),
    }
}

fn synthesize_board_dump(user_text: &str, prior: &[(String, String)], title: &str) -> String {
    let mut out = format!("# {title}\n\n**Request:** {}\n\n", user_text.trim());
    for (name, body) in prior {
        out.push_str("## ");
        out.push_str(&name.to_ascii_uppercase());
        out.push_str("\n\n");
        out.push_str(&body.chars().take(8_000).collect::<String>());
        out.push_str("\n\n");
    }
    out
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

fn extract_thesis(body: &str) -> String {
    for line in body.lines() {
        let t = line.trim();
        let lower = t.to_ascii_lowercase();
        if (lower.contains("thesis") || lower.contains("summary")) && lower.contains(':') {
            if let Some(colon) = t.find(':') {
                let v = t[colon + 1..].trim().trim_start_matches('*').trim();
                if !v.is_empty() {
                    return first_line(v, 280);
                }
            }
        }
    }
    body.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("---"))
        .map(|l| first_line(l, 280))
        .unwrap_or_else(|| "(no summary)".into())
}

fn extract_stage_summary(body: &str) -> String {
    extract_thesis(body)
}
