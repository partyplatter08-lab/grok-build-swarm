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
//!
//! Parent chat is the **captain channel** (Grok Heavy style): it narrates the
//! plan, heartbeats while workers run, and posts clean per-member theses as
//! they finish. Full worker thinking stays on subagent cards — not per-token
//! dumps into the parent feed (those interleave into unreadable noise).

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

/// How often the captain posts a "still working" heartbeat while council runs.
const COUNCIL_HEARTBEAT: Duration = Duration::from_secs(5);

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

        let goal_line = first_line(user_text, 160);
        self.emit_agent_text(&format!(
            "{}\n\n\
             I'll run this the way **Grok Heavy** does: same problem, many lenses, \
             then I synthesize. You stay in this chat — I'll narrate live.\n\n\
             **Your ask:** {goal}\n\n\
             **Plan**\n\
             0. **Parallel council** — Analyst · Skeptic · Explorer · Builder (all at once)\n\
             1. **Research** — deepen with tools (read-only)\n\
             2. **Implement** — make the change\n\
             3. **Test** — verify\n\
             4. **Synthesis** — my final answer from the full board\n\n\
             Open a worker card for full thinking; I'll post theses and progress here.",
            mode_banner(mode),
            goal = goal_line,
        ))
        .await;

        let mut prior: Vec<(String, String)> = Vec::new();

        // ── Phase 0: parallel council ───────────────────────────────────
        self.emit_agent_text(
            "── **Phase 0 · Parallel council** ──\n\
             Launching four lenses on the **same** problem:\n\
             - ◈ **Analyst** — frame goals, constraints, success\n\
             - ◈ **Skeptic** — attack weak assumptions\n\
             - ◈ **Explorer** — alternatives & context\n\
             - ◈ **Builder** — concrete plan\n\n\
             Spawning all four **now**…",
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

        let mut council_digest = String::from("## Council theses (parallel)\n\n");
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

        // Captain cross-check after the board is full.
        let mut board = String::from(
            "── **Council board (all in)** ──\n\
             Here's how the four lenses landed:\n\n",
        );
        for (role, thesis) in &theses {
            board.push_str(&format!("- **{role}:** {thesis}\n"));
        }
        board.push_str(
            "\nI'll carry this into Research → Implement → Test. \
             If they disagreed, the next stages must resolve it with evidence.",
        );
        self.emit_agent_text(&board).await;

        // ── Phases 1–3: RIT sequential ──────────────────────────────────
        for (idx, stage) in STAGES.iter().enumerate() {
            let n = idx + 1;
            let phase_talk = match stage.id_suffix {
                "research" => {
                    "Going deeper with tools before we touch code. Research is read-only."
                }
                "implement" => {
                    "Council + research are the brief. Time to make the change."
                }
                "test" => "Verifying the work — checks, tests, residual risks.",
                _ => "Pipeline stage running.",
            };
            self.emit_agent_text(&format!(
                "── **Phase {n}/3 · {}** ──\n\
                 {phase_talk}\n\
                 Spawning `{stype}`… I'll report when this stage lands.",
                stage.description,
                stype = stage.subagent_type,
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
                    let summary = extract_stage_summary(&out);
                    self.emit_agent_text(&format!(
                        "✓ **Phase {n} complete** · {}\n\n{summary}",
                        stage.description
                    ))
                    .await;
                    prior.push((stage.id_suffix.to_string(), out));
                }
                Err(e) => {
                    self.emit_agent_text(&format!(
                        "✗ **Phase {n} failed** · {}: {e}\n\
                         I'll note the gap and keep going so we still get a synthesis.",
                        stage.description
                    ))
                    .await;
                    prior.push((stage.id_suffix.to_string(), format!("[error] {e}")));
                }
            }
        }

        self.emit_agent_text(
            "── **Synthesis** ──\n\
             Pulling the board together into one answer for you…",
        )
        .await;
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

/// Fire all council spawns, then collect results as they finish (true parallelism).
///
/// Narrates live: immediate post when a member lands, plus heartbeats while
/// others are still working — so the parent chat never goes silent mid-council.
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
             Keep Thesis crisp — the captain will quote it in the live board.\n",
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
        outstanding.push(role.clone());
        futs.push(Box::pin(async move {
            let res = result_rx
                .await
                .map_err(|_| "subagent result channel dropped".to_string());
            (desc, res)
        }));
        actor
            .emit_agent_text(&format!("● **{role}** is live (card open for full stream)."))
            .await;
    }

    if !outstanding.is_empty() {
        actor
            .emit_agent_text(&format!(
                "Council is running in parallel · **{}** outstanding. \
                 I'll post each thesis the moment it lands.",
                outstanding.join(" · ")
            ))
            .await;
    }

    let mut out = early;
    let mut heartbeat = tokio::time::interval(COUNCIL_HEARTBEAT);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Skip the immediate first tick so we don't double-announce.
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
                            .emit_agent_text(&format!(
                                "✓ **{role}** in\n\
                                 **Thesis:** {thesis}"
                            ))
                            .await;
                    }
                    Ok(r) => {
                        let err = r.error.clone().unwrap_or_else(|| "unknown".into());
                        actor
                            .emit_agent_text(&format!("✗ **{role}** failed · {err}"))
                            .await;
                    }
                    Err(e) => {
                        actor
                            .emit_agent_text(&format!("✗ **{role}** error · {e}"))
                            .await;
                    }
                }
                if !outstanding.is_empty() {
                    actor
                        .emit_agent_text(&format!(
                            "Still waiting on: **{}**",
                            outstanding.join(" · ")
                        ))
                        .await;
                }
                out.push((desc, res));
            }
            _ = heartbeat.tick(), if !futs.is_empty() => {
                if outstanding.is_empty() {
                    continue;
                }
                actor
                    .emit_agent_text(&format!(
                        "⏳ Council still working · **{}** outstanding… \
                         (full reasoning is on their cards)",
                        outstanding.join(" · ")
                    ))
                    .await;
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
    let mut out = String::from("# ◈ HEAVY RESULT\n\n");
    out.push_str("**Request:** ");
    out.push_str(user_text.trim());
    out.push_str(
        "\n\nRan a **code-enforced** Heavy pipeline: parallel council, then \
         Research → Implement → Test. Full worker traces live on the cards above.\n\n",
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
        // "[Council/Analyst]" → "Analyst"
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
    let lower = body.to_ascii_lowercase();
    // Prefer an explicit Thesis section.
    for marker in ["**thesis**", "thesis:", "## thesis", "### thesis"] {
        if let Some(idx) = lower.find(marker) {
            let after = body[idx + marker.len()..].trim_start();
            let chunk = after
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#'))
                .unwrap_or(after);
            return first_line(chunk, 280);
        }
    }
    // Fall back to first non-heading, non-empty line.
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| {
            !l.is_empty()
                && !l.starts_with('#')
                && !l.starts_with("---")
                && !l.eq_ignore_ascii_case("thesis")
        })
        .unwrap_or(body.trim());
    first_line(line, 280)
}

fn extract_stage_summary(body: &str) -> String {
    let lower = body.to_ascii_lowercase();
    for marker in ["**summary**", "summary:", "## summary", "### summary", "- **summary"] {
        if let Some(idx) = lower.find(marker) {
            let after = body[idx..].trim();
            // Take a few lines after the marker for a readable handoff.
            let mut lines = Vec::new();
            for (i, line) in after.lines().enumerate() {
                if i > 0 && (line.starts_with("## ") || line.starts_with("**Evidence")) {
                    break;
                }
                let t = line.trim();
                if !t.is_empty() {
                    lines.push(t);
                }
                if lines.len() >= 8 {
                    break;
                }
            }
            if !lines.is_empty() {
                return lines.join("\n");
            }
        }
    }
    // Compact fallback — not a 1500-char dump.
    let mut out = String::new();
    for line in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if out.len() + line.len() > 700 {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        if out.lines().count() >= 10 {
            break;
        }
    }
    if out.is_empty() {
        first_line(body, 400)
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_role_from_council_tag() {
        assert_eq!(
            short_role("[Council/Analyst] structure the problem"),
            "Analyst"
        );
        assert_eq!(short_role("[Council/Skeptic] attack"), "Skeptic");
    }

    #[test]
    fn extract_thesis_prefers_labeled_section() {
        let body = "## Stuff\n\n**Thesis**\nShip the fix behind a flag.\n\n**Argument**\n- a\n";
        assert_eq!(extract_thesis(body), "Ship the fix behind a flag.");
    }

    #[test]
    fn extract_thesis_falls_back_to_first_line() {
        assert_eq!(
            extract_thesis("Just a plain answer without headers."),
            "Just a plain answer without headers."
        );
    }
}
