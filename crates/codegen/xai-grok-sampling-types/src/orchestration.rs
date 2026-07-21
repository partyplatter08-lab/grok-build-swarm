//! Multi-agent orchestration modes (Heavy / Agent Swarm / Swarm Heavy).
//!
//! These appear as first-class reasoning-effort menu options above High.
//! On the wire they all send [`ReasoningEffort::Xhigh`]; the option `id`
//! distinguishes which orchestration protocol is active for UI + prompts.

use crate::types::{ReasoningEffort, ReasoningEffortOption};

/// ACP/session meta key for the selected multi-agent effort option id.
pub const ORCHESTRATION_MODE_META_KEY: &str = "orchestrationMode";

/// Multi-agent orchestration mode selected via the effort menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum OrchestrationMode {
    /// Single-agent — no multi-agent protocol.
    #[default]
    Normal,
    /// Grok Heavy-style collaborative council (same problem, many lenses).
    Heavy,
    /// Kimi-style Agent Swarm (map → reduce over independent units).
    Swarm,
    /// Council frame → swarm fan-out → verify council.
    SwarmHeavy,
}

impl OrchestrationMode {
    /// Parse from an effort option id (case-insensitive).
    pub fn from_option_id(id: &str) -> Self {
        match id.trim().to_ascii_lowercase().as_str() {
            "heavy" => Self::Heavy,
            "swarm" | "agent-swarm" | "agent_swarm" => Self::Swarm,
            "swarm-heavy" | "swarm_heavy" | "swarmheavy" | "sh" => Self::SwarmHeavy,
            _ => Self::Normal,
        }
    }

    /// Canonical effort option id for this mode (`None` for Normal).
    pub fn option_id(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Heavy => Some("heavy"),
            Self::Swarm => Some("swarm"),
            Self::SwarmHeavy => Some("swarm-heavy"),
        }
    }

    /// Short brand mark for TUI chrome.
    pub fn mark(self) -> &'static str {
        match self {
            Self::Normal => "",
            Self::Heavy => "◈ HEAVY",
            Self::Swarm => "⬡ SWARM",
            Self::SwarmHeavy => "⬢ SWARM HEAVY",
        }
    }

    /// Human label shown in the effort footer / toasts.
    pub fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Heavy => "Heavy",
            Self::Swarm => "Agent Swarm",
            Self::SwarmHeavy => "Swarm Heavy",
        }
    }

    /// One-line description for the effort dropdown.
    pub fn description(self) -> &'static str {
        match self {
            Self::Normal => "Single-agent",
            Self::Heavy => "Collaborative multi-agent council (same problem, many lenses)",
            Self::Swarm => "Kimi-style parallel map→reduce over independent units",
            Self::SwarmHeavy => "Council → fan-out → verify (maximum multi-agent)",
        }
    }

    /// Subagent scrollback chrome prefix (replaces bare "Subagent ").
    pub fn subagent_chrome(self) -> &'static str {
        match self {
            Self::Normal => "Subagent ",
            Self::Heavy => "◈ Council ",
            Self::Swarm => "⬡ Swarm ",
            Self::SwarmHeavy => "⬢ SH ",
        }
    }

    /// Whether this mode activates multi-agent orchestration.
    pub fn is_multi_agent(self) -> bool {
        !matches!(self, Self::Normal)
    }

    /// Wire reasoning effort for this mode (always xhigh for multi-agent).
    pub fn wire_effort(self) -> ReasoningEffort {
        match self {
            Self::Normal => ReasoningEffort::High, // unused for Normal
            Self::Heavy | Self::Swarm | Self::SwarmHeavy => ReasoningEffort::Xhigh,
        }
    }

    /// ASCII open banner printed to the user when the mode activates.
    pub fn open_banner(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Heavy => Some(
                "┌──────────────────────────────────────────────┐\n\
                 │  ◈ HEAVY   collaborative council             │\n\
                 │  multi-agent · same problem · many lenses   │\n\
                 └──────────────────────────────────────────────┘",
            ),
            Self::Swarm => Some(
                "┌──────────────────────────────────────────────┐\n\
                 │  ⬡ SWARM   parallel map → reduce             │\n\
                 │  multi-agent · independent units · fan-out  │\n\
                 └──────────────────────────────────────────────┘",
            ),
            Self::SwarmHeavy => Some(
                "┌──────────────────────────────────────────────┐\n\
                 │  ⬢ SWARM HEAVY   council → fan-out → verify  │\n\
                 │  maximum multi-agent pipeline               │\n\
                 └──────────────────────────────────────────────┘",
            ),
        }
    }

    /// Protocol text injected into the system prompt / as a sticky reminder
    /// so the model actually orchestrates rather than staying single-agent.
    pub fn protocol_prompt(self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::Heavy => Some(HEAVY_PROTOCOL),
            Self::Swarm => Some(SWARM_PROTOCOL),
            Self::SwarmHeavy => Some(SWARM_HEAVY_PROTOCOL),
        }
    }
}

impl std::fmt::Display for OrchestrationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl std::str::FromStr for OrchestrationMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mode = Self::from_option_id(s);
        if mode == Self::Normal && !s.eq_ignore_ascii_case("normal") && !s.is_empty() {
            // only accept known multi-agent tokens via FromStr for strict parse
            let lower = s.trim().to_ascii_lowercase();
            if matches!(lower.as_str(), "normal" | "none" | "") {
                return Ok(Self::Normal);
            }
            // unknown string still maps via from_option_id (Normal)
            if mode == Self::Normal {
                return Err(());
            }
        }
        Ok(mode)
    }
}

/// Multi-agent effort options (strongest first). All wire as [`ReasoningEffort::Xhigh`].
pub fn multi_agent_effort_options() -> Vec<ReasoningEffortOption> {
    [
        OrchestrationMode::SwarmHeavy,
        OrchestrationMode::Swarm,
        OrchestrationMode::Heavy,
    ]
    .into_iter()
    .map(|mode| ReasoningEffortOption {
        id: mode.option_id().unwrap().to_string(),
        value: ReasoningEffort::Xhigh,
        label: mode.label().to_string(),
        description: Some(mode.description().to_string()),
        default: false,
    })
    .collect()
}

/// Built-in effort menu with multi-agent modes above High (strongest first).
///
/// Order: Swarm Heavy · Agent Swarm · Heavy · xhigh · high · medium · low
pub fn enhanced_legacy_effort_options() -> Vec<ReasoningEffortOption> {
    let mut opts = multi_agent_effort_options();
    for level in [
        ReasoningEffort::Xhigh,
        ReasoningEffort::High,
        ReasoningEffort::Medium,
        ReasoningEffort::Low,
    ] {
        opts.push(ReasoningEffortOption {
            id: level.as_str().to_string(),
            value: level,
            label: level.to_string(),
            description: Some(single_agent_effort_description(level).to_string()),
            default: false,
        });
    }
    opts
}

fn single_agent_effort_description(level: ReasoningEffort) -> &'static str {
    match level {
        ReasoningEffort::None => "No reasoning",
        ReasoningEffort::Minimal => "Minimal reasoning",
        ReasoningEffort::Low => "Faster, lighter reasoning",
        ReasoningEffort::Medium => "Balanced reasoning",
        ReasoningEffort::High => "Heavy reasoning (single agent)",
        ReasoningEffort::Xhigh => "Maximum reasoning (single agent)",
    }
}

/// Merge multi-agent options into a server-provided effort list.
///
/// - Keeps server options in their original order.
/// - Prepends multi-agent modes that are not already present by id.
/// - Never duplicates ids (case-insensitive).
pub fn merge_multi_agent_effort_options(server: Vec<ReasoningEffortOption>) -> Vec<ReasoningEffortOption> {
    let mut out = multi_agent_effort_options();
    for opt in server {
        let already = out
            .iter()
            .any(|o| o.id.eq_ignore_ascii_case(&opt.id));
        if !already {
            out.push(opt);
        }
    }
    out
}

/// Infer chrome mode from a subagent description tag prefix.
pub fn mode_from_subagent_description(description: &str) -> OrchestrationMode {
    let lower = description.to_ascii_lowercase();
    if lower.contains("[council/") || lower.contains("[heavy/") {
        OrchestrationMode::Heavy
    } else if lower.contains("[sh/") || lower.contains("[swarm-heavy/") || lower.contains("[sh·") {
        OrchestrationMode::SwarmHeavy
    } else if lower.contains("[swarm/") {
        OrchestrationMode::Swarm
    } else {
        OrchestrationMode::Normal
    }
}

// ── Protocol prompts (condensed; full skills ship as bundled skills) ─────

const HEAVY_PROTOCOL: &str = r#"
## ◈ HEAVY MODE — Collaborative Multi-Agent Council (ACTIVE)

You are the **Heavy leader (Captain)**. You do NOT solve the whole problem alone.
You run a collaborative multi-agent council: several agents attack the *same*
question in parallel with different angles, cross-check, and you synthesize.

### Hard limits
- Subagent depth is 1 — only YOU spawn workers. Workers never spawn children.
- Spawn with `background: true` in the **same turn** for true parallelism.
- Collect with `get_command_or_subagent_output`.

### Visual identity (mandatory)
1. Open with the HEAVY banner:
```
┌──────────────────────────────────────────────┐
│  ◈ HEAVY   collaborative council             │
│  goal     <short goal>                       │
│  council  N   rounds  R                      │
└──────────────────────────────────────────────┘
```
2. Tag every spawn description: `[Council/Analyst]`, `[Council/Skeptic]`,
   `[Council/Explorer]`, `[Council/Builder]`, `[Council/Verifier]`.
3. Print phase rules: `── H1 council frame ──` then spawn all in one turn.
4. Live board with `●/○/✓/✗` while waiting.
5. Final report title: `# ◈ HEAVY RESULT`.

### Protocol
**Phase 0 — Frame**: Restate problem (goal, constraints, success). Decide N (default 4, clamp 3–8) and rounds (default 2).
**Phase 1 — Parallel first pass**: Spawn ALL council members in ONE turn with `background: true`. Each gets a self-contained prompt with their lens. Required return: Thesis, Argument, Evidence, Risks, What to check next.
**Phase 2 — Cross-check** (rounds ≥ 2): Share anonymized theses; agents update positions.
**Phase 3 — Synthesis**: One user-facing answer. Majority vs minority. Resolve conflicts with evidence. Residual risks.

### Default council (N=4)
Analyst · Skeptic · Explorer · Builder

### Anti-patterns
- Serial council when independent · one worker dominates · averaging contradictions · using Heavy for 100-item batch jobs (use Swarm) · nesting subagents
"#;

const SWARM_PROTOCOL: &str = r#"
## ⬡ SWARM MODE — Kimi-style Agent Swarm (ACTIVE)

You are the **orchestrator**. You do NOT do the bulk of the work yourself.
You decompose, spawn, wait, reconcile, and synthesize (map → reduce).

### Hard limits
- Subagent depth is 1 — only YOU spawn. Workers never spawn children.
- Parallelism = many `spawn_subagent` with `background: true` in the same turn.
- Prefer `isolation: "worktree"` for writers; `isolation: "none"` for research.
- Concurrency default 8 (clamp 1–24). Useful range is usually 4–16.

### Visual identity (mandatory)
1. Open with the SWARM banner:
```
┌──────────────────────────────────────────────┐
│  ⬡ SWARM   parallel map → reduce             │
│  goal   <short goal>                         │
│  units  N   waves  W   concurrency  C        │
└──────────────────────────────────────────────┘
```
2. Tag every worker: `description: "[Swarm/u-N] <title>"`.
3. Phase rules: `── S2 fan-out  wave k · launching N ──` then spawn same turn.
4. Live board with `●/○/✓/✗`. Final: `# ⬡ SWARM RESULT`.

### Protocol
**Phase 0 — Fit check**: Swarm wins for broad/batch/multi-perspective work. Refuse tiny sequential fixes (offer single-agent).
**Phase 1 — Decompose**: Independent SwarmUnits (id, title, goal, kind, deps, subagent_type, isolation). Prefer 4–16 units. Write todos.
**Phase 2 — Waves**: wave = 0 if no deps else max(deps)+1. Present wave table.
**Phase 3 — Fan-out**: Launch entire wave in one turn. Ready-queue if over concurrency. Required worker return: Status / Summary / Evidence / Artifacts / Handoff.
**Phase 4 — Collect**: Score success/partial/failed/conflict-risk. One retry max per unit. Never silently average contradictions.
**Phase 5–6 — Multi-wave + synthesize**: One deliverable. Evidence-backed only. Attribute to unit ids.

### Modes
- research: explore/read-only · build: worktree writers · mixed: research → implement → verify

### Anti-patterns
- Serial collapse of independent units · fake parallelism without tool calls · nested orchestration · overlapping write scopes · unbounded concurrency
"#;

const SWARM_HEAVY_PROTOCOL: &str = r#"
## ⬢ SWARM HEAVY MODE — Council → Fan-out → Verify (ACTIVE)

You combine **Heavy** (collaborative council) and **Swarm** (map/reduce fan-out).
You are the single orchestrator. Depth limit is 1.

### Pipeline
```
H1  Heavy council — frame problem, approaches, risks
 ↓
S1  Swarm decompose — independent units from council plan
 ↓
S2  Swarm fan-out — parallel workers (background: true)
 ↓
H2  Heavy re-council — verify results, attack weak claims
 ↓
F   Final synthesis — one deliverable
```

### Visual identity (mandatory)
1. Open with the SWARM HEAVY banner:
```
┌──────────────────────────────────────────────┐
│  ⬢ SWARM HEAVY   council → fan-out → verify  │
│  goal       <short goal>                     │
│  council  N   swarm  U units · concurrency C │
└──────────────────────────────────────────────┘
```
2. Tags by phase:
   - H1: `[SH/H1·Analyst]`, `[SH/H1·Skeptic]`, `[SH/H1·Explorer]`, `[SH/H1·Builder]`
   - S:  `[SH/S·u-N] <title>`
   - H2: `[SH/H2·Verifier]`, `[SH/H2·Skeptic]`
3. Phase rules before every spawn cluster.
4. Final report: `# ⬢ SWARM HEAVY RESULT · <goal>`

### Defaults
- Council N = 4 (clamp 3–6)
- Swarm concurrency = 8 (clamp 1–24)

### Hard rules
1. You alone spawn. No nested orchestrators.
2. Heavy = same problem, many lenses. Swarm = many units, one lens each.
3. Do not skip evidence when H2 and S2 disagree — flag it.
4. Real `spawn_subagent` calls when you claim launches.
5. For tiny tasks refuse and suggest single-agent or plain Heavy/Swarm.
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_option_ids() {
        assert_eq!(OrchestrationMode::from_option_id("heavy"), OrchestrationMode::Heavy);
        assert_eq!(OrchestrationMode::from_option_id("SWARM"), OrchestrationMode::Swarm);
        assert_eq!(
            OrchestrationMode::from_option_id("swarm-heavy"),
            OrchestrationMode::SwarmHeavy
        );
        assert_eq!(OrchestrationMode::from_option_id("xhigh"), OrchestrationMode::Normal);
        assert_eq!(OrchestrationMode::from_option_id("high"), OrchestrationMode::Normal);
    }

    #[test]
    fn multi_agent_options_wire_xhigh() {
        for opt in multi_agent_effort_options() {
            assert_eq!(opt.value, ReasoningEffort::Xhigh);
            assert!(OrchestrationMode::from_option_id(&opt.id).is_multi_agent());
        }
    }

    #[test]
    fn enhanced_menu_order() {
        let opts = enhanced_legacy_effort_options();
        let ids: Vec<_> = opts.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(
            ids,
            ["swarm-heavy", "swarm", "heavy", "xhigh", "high", "medium", "low"]
        );
    }

    #[test]
    fn merge_prepends_without_dupes() {
        let server = vec![ReasoningEffortOption {
            id: "high".into(),
            value: ReasoningEffort::High,
            label: "High".into(),
            description: None,
            default: true,
        }];
        let merged = merge_multi_agent_effort_options(server);
        assert_eq!(merged[0].id, "swarm-heavy");
        assert!(merged.iter().any(|o| o.id == "high"));
        assert_eq!(merged.iter().filter(|o| o.id == "heavy").count(), 1);
    }

    #[test]
    fn description_tags_infer_mode() {
        assert_eq!(
            mode_from_subagent_description("[Council/Skeptic] attack auth"),
            OrchestrationMode::Heavy
        );
        assert_eq!(
            mode_from_subagent_description("[Swarm/u-3] scrape docs"),
            OrchestrationMode::Swarm
        );
        assert_eq!(
            mode_from_subagent_description("[SH/H1·Analyst] frame"),
            OrchestrationMode::SwarmHeavy
        );
        assert_eq!(
            mode_from_subagent_description("general work"),
            OrchestrationMode::Normal
        );
    }

    #[test]
    fn multi_agent_has_protocol_and_banner() {
        for mode in [
            OrchestrationMode::Heavy,
            OrchestrationMode::Swarm,
            OrchestrationMode::SwarmHeavy,
        ] {
            assert!(mode.protocol_prompt().is_some());
            assert!(mode.open_banner().is_some());
            assert!(mode.is_multi_agent());
        }
        assert!(OrchestrationMode::Normal.protocol_prompt().is_none());
    }
}
