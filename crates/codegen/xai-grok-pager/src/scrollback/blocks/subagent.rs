//! SubagentBlock — scrollback entries for subagent lifecycle.
//!
//! Similar to BgTaskBlock: always collapsed, animated bullet while running,
//! colored bullet when done. Enter / Ctrl-F opens the subagent view.
//!
//! Two modes:
//! - **Blocking** (sync): Single `Started` block. Blinks while running,
//!   turns green/red when done. Text: `Subagent "description"`
//! - **Background** (async): `Started` block stays forever (turns gray).
//!   A separate `Completed`/`Failed` block is added when done.
//!   Started text: `Subagent started: "description"`
//!   Completed text: `Subagent completed in 43s: "description"`

use std::time::Duration;

use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::subagent::format_subagent_meta;
use crate::render::color::blend_color;
use crate::render::line_utils::truncate_str;
use crate::scrollback::block::BlockContent;
use crate::scrollback::types::{AccentStyle, BlockContext, BlockOutput, DisplayMode};
use crate::theme::Theme;
use crate::util::format_duration;

/// What kind of subagent lifecycle event this block represents.
#[derive(Debug, Clone)]
pub enum SubagentBlockKind {
    /// Subagent is running (or was running — `finish_running` stops animation).
    Started,
    /// Subagent completed successfully.
    Completed { elapsed: Duration },
    /// Subagent failed.
    Failed {
        elapsed: Duration,
        error: Option<String>,
    },
    /// Subagent was cancelled.
    Cancelled { elapsed: Duration },
}

/// Subagent scrollback block.
///
/// Always collapsed, not foldable, groupable, selectable.
/// Enter / Ctrl-F opens the subagent view.
#[derive(Debug, Clone)]
pub struct SubagentBlock {
    /// Human-readable description of the task.
    pub description: String,
    /// Child session ID (for opening the subagent view).
    pub child_session_id: String,
    /// Subagent type (e.g. "general-purpose", "explore").
    pub subagent_type: String,
    /// Named persona applied to this subagent, if any.
    pub persona: Option<String>,
    /// Role that supplied defaults for this subagent, if any.
    pub role: Option<String>,
    /// Effective model ID used by the subagent, if available.
    pub model: Option<String>,
    /// Whether the subagent was launched in background mode.
    pub is_background: bool,
    /// Lifecycle kind.
    pub kind: SubagentBlockKind,
    /// Live activity label from the child session's turn tracker.
    ///
    /// Updated on each `SubagentProgress` tick while the subagent is running.
    /// Shown inline in the collapsed scrollback line (e.g. "Thinking",
    /// "Running: cargo build") so the user sees interactive progress without
    /// opening the subagent view.
    pub activity_label: Option<String>,
}

impl SubagentBlock {
    /// Create a "Subagent started" block (for both sync and async).
    pub fn started(
        description: impl Into<String>,
        child_session_id: impl Into<String>,
        subagent_type: impl Into<String>,
        persona: Option<String>,
        role: Option<String>,
        model: Option<String>,
        is_background: bool,
    ) -> Self {
        Self {
            description: description.into(),
            child_session_id: child_session_id.into(),
            subagent_type: subagent_type.into(),
            persona,
            role,
            model,
            is_background,
            kind: SubagentBlockKind::Started,
            activity_label: None,
        }
    }

    /// Create a "Subagent completed" block (background mode only).
    pub fn completed(
        description: impl Into<String>,
        child_session_id: impl Into<String>,
        elapsed: Duration,
    ) -> Self {
        Self {
            description: description.into(),
            child_session_id: child_session_id.into(),
            subagent_type: String::new(),
            persona: None,
            role: None,
            model: None,
            is_background: true,
            kind: SubagentBlockKind::Completed { elapsed },
            activity_label: None,
        }
    }

    /// Create a "Subagent failed" block (background mode only).
    pub fn failed(
        description: impl Into<String>,
        child_session_id: impl Into<String>,
        elapsed: Duration,
        error: Option<String>,
    ) -> Self {
        Self {
            description: description.into(),
            child_session_id: child_session_id.into(),
            subagent_type: String::new(),
            persona: None,
            role: None,
            model: None,
            is_background: true,
            kind: SubagentBlockKind::Failed { elapsed, error },
            activity_label: None,
        }
    }

    /// Create a "Subagent cancelled" block (background mode only).
    pub fn cancelled(
        description: impl Into<String>,
        child_session_id: impl Into<String>,
        elapsed: Duration,
    ) -> Self {
        Self {
            description: description.into(),
            child_session_id: child_session_id.into(),
            subagent_type: String::new(),
            persona: None,
            role: None,
            model: None,
            is_background: true,
            kind: SubagentBlockKind::Cancelled { elapsed },
            activity_label: None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.kind, SubagentBlockKind::Started)
    }
}

/// Truncate description and wrap in quotes for display.
fn quoted_desc(desc: &str, max_width: usize) -> String {
    // Reserve 2 chars for quotes
    if max_width <= 2 {
        return "\u{201C}\u{2026}\u{201D}".to_string(); // "…"
    }
    let inner = truncate_str(desc, max_width - 2);
    format!("\u{201C}{inner}\u{201D}")
}

/// Leading `[tag]` parse (same rules as `format_subagent_label`).
fn parse_desc_tag(description: &str) -> (Option<&str>, &str) {
    if let Some(rest) = description.strip_prefix('[')
        && let Some(close) = rest.find(']')
    {
        let tag = rest[..close].trim();
        if !tag.is_empty() {
            // Prefer the role after a slash: [Council/Analyst] → Analyst
            let display = tag.rsplit('/').next().unwrap_or(tag);
            return (Some(display), rest[close + 1..].trim_start());
        }
    }
    (None, description)
}

impl BlockContent for SubagentBlock {
    fn output(&self, ctx: &BlockContext) -> BlockOutput {
        let theme = Theme::current();
        let bold = if ctx.is_selected {
            theme.primary().add_modifier(Modifier::BOLD)
        } else {
            theme.muted().add_modifier(Modifier::BOLD)
        };
        let muted = theme.muted();
        let w = ctx.width as usize;

        let orch_mode = xai_grok_shell::sampling::types::mode_from_subagent_description(
            &self.description,
        );
        let chrome = orch_mode.subagent_chrome();
        let chrome_width = chrome.width();
        let chrome_style = if orch_mode.is_multi_agent() {
            crate::views::orchestration_visuals::mode_chrome_style(
                orch_mode,
                &theme,
                ctx.is_selected,
            )
        } else {
            bold
        };

        // Full multi-agent card (Claude/Grok Heavy-style worker row).
        if orch_mode.is_multi_agent() && w >= 28 {
            // Role label: persona → role → type → [tag] from description.
            let (tag, clean_desc) = parse_desc_tag(&self.description);
            let role = self
                .persona
                .as_deref()
                .filter(|s| !s.is_empty())
                .or(self.role.as_deref().filter(|s| !s.is_empty()))
                .map(|s| {
                    let mut c = s.chars();
                    match c.next() {
                        Some(ch) => ch.to_uppercase().chain(c).collect::<String>(),
                        None => s.to_string(),
                    }
                })
                .or_else(|| {
                    if self.subagent_type != "general-purpose" && !self.subagent_type.is_empty() {
                        Some(crate::app::subagent::format_type_label(&self.subagent_type).to_string())
                    } else {
                        tag.map(|t| {
                            let mut c = t.chars();
                            match c.next() {
                                Some(ch) => ch.to_uppercase().chain(c).collect(),
                                None => t.to_string(),
                            }
                        })
                    }
                })
                .unwrap_or_else(|| "Worker".to_string());
            let meta = format_subagent_meta(
                self.persona.as_deref(),
                self.role.as_deref(),
                self.model.as_deref(),
            );
            let meta_line = {
                let mut parts: Vec<&str> = Vec::new();
                if !self.subagent_type.is_empty() {
                    parts.push(self.subagent_type.as_str());
                }
                let m = meta.trim().trim_start_matches('(').trim_end_matches(')');
                if !m.is_empty() {
                    parts.push(m);
                }
                parts.join(" · ")
            };
            let card_state = match &self.kind {
                SubagentBlockKind::Started => {
                    crate::views::orchestration_visuals::SubagentCardState::Running
                }
                SubagentBlockKind::Completed { .. } => {
                    crate::views::orchestration_visuals::SubagentCardState::Completed
                }
                SubagentBlockKind::Failed { .. } => {
                    crate::views::orchestration_visuals::SubagentCardState::Failed
                }
                SubagentBlockKind::Cancelled { .. } => {
                    crate::views::orchestration_visuals::SubagentCardState::Cancelled
                }
            };
            let activity_owned: Option<String> = match &self.kind {
                SubagentBlockKind::Started => self.activity_label.clone(),
                SubagentBlockKind::Completed { elapsed } => {
                    Some(format!("completed in {}", format_duration(*elapsed)))
                }
                SubagentBlockKind::Failed { elapsed, error } => Some(format!(
                    "failed in {}{}",
                    format_duration(*elapsed),
                    error
                        .as_deref()
                        .map(|e| format!(" ({e})"))
                        .unwrap_or_default()
                )),
                SubagentBlockKind::Cancelled { elapsed } => {
                    Some(format!("cancelled in {}", format_duration(*elapsed)))
                }
            };
            let lines = crate::views::orchestration_visuals::subagent_card_lines(
                orch_mode,
                &role,
                if clean_desc.is_empty() {
                    self.description.as_str()
                } else {
                    clean_desc
                },
                activity_owned.as_deref(),
                &meta_line,
                card_state,
                &theme,
                w,
            );
            return BlockOutput {
                lines: lines.into_iter().map(Into::into).collect(),
            };
        }

        // Stock single-line layout for normal subagents.
        let line = match (&self.kind, self.is_background) {
            (SubagentBlockKind::Started, bg) => {
                let verb = if bg { "started: " } else { "running: " };
                let activity_suffix: String = self
                    .activity_label
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|a| format!(" \u{2014} {a}"))
                    .unwrap_or_default();
                let meta = format_subagent_meta(
                    self.persona.as_deref(),
                    self.role.as_deref(),
                    self.model.as_deref(),
                );
                let overhead = chrome_width + 9 + meta.width() + activity_suffix.width();
                let desc = quoted_desc(&self.description, w.saturating_sub(overhead));
                let mut spans = vec![
                    Span::styled(chrome, chrome_style),
                    Span::styled(verb, muted),
                    Span::styled(desc, muted),
                ];
                if !activity_suffix.is_empty() {
                    spans.push(Span::styled(activity_suffix, muted));
                }
                spans.push(Span::styled(meta, muted));
                Line::from(spans)
            }
            (SubagentBlockKind::Completed { elapsed }, _) => {
                let time_str = format_duration(*elapsed);
                let prefix_len = chrome_width + 17 + time_str.len();
                let desc = quoted_desc(&self.description, w.saturating_sub(prefix_len));
                Line::from(vec![
                    Span::styled(chrome, chrome_style),
                    Span::styled(format!("completed in {time_str}: "), muted),
                    Span::styled(desc, muted),
                ])
            }
            (SubagentBlockKind::Failed { elapsed, error }, _) => {
                let time_str = format_duration(*elapsed);
                let detail = error
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default();
                let prefix_len = chrome_width + 12 + time_str.len() + detail.len();
                let desc = quoted_desc(&self.description, w.saturating_sub(prefix_len));
                Line::from(vec![
                    Span::styled(chrome, chrome_style),
                    Span::styled(format!("failed in {time_str}{detail}: "), muted),
                    Span::styled(desc, muted),
                ])
            }
            (SubagentBlockKind::Cancelled { elapsed }, _) => {
                let time_str = format_duration(*elapsed);
                let prefix_len = chrome_width + 17 + time_str.len();
                let desc = quoted_desc(&self.description, w.saturating_sub(prefix_len));
                Line::from(vec![
                    Span::styled(chrome, chrome_style),
                    Span::styled(format!("cancelled in {time_str}: "), muted),
                    Span::styled(desc, muted),
                ])
            }
        };

        BlockOutput {
            lines: vec![line.into()],
        }
    }

    fn accent(&self, ctx: &BlockContext) -> Option<AccentStyle> {
        let theme = Theme::current();
        let orch = xai_grok_shell::sampling::types::mode_from_subagent_description(
            &self.description,
        );
        match &self.kind {
            SubagentBlockKind::Started if ctx.is_running => {
                let c = if orch.is_multi_agent() {
                    crate::views::orchestration_visuals::mode_running_color(orch)
                } else {
                    theme.accent_running
                };
                Some(AccentStyle::animated(c))
            }
            _ => None,
        }
    }

    fn bullet(&self, ctx: &BlockContext) -> Option<AccentStyle> {
        let theme = Theme::current();
        let orch = xai_grok_shell::sampling::types::mode_from_subagent_description(
            &self.description,
        );
        match &self.kind {
            SubagentBlockKind::Started => {
                if ctx.is_running {
                    let base = if orch.is_multi_agent() {
                        crate::views::orchestration_visuals::mode_running_color(orch)
                    } else {
                        theme.accent_running
                    };
                    let dim = ctx.appearance.scrollback.display.dim_accent;
                    let dimmed = blend_color(theme.bg_base, base, dim).unwrap_or(base);
                    Some(AccentStyle::animated(dimmed))
                } else {
                    None
                }
            }
            SubagentBlockKind::Completed { .. } => {
                Some(AccentStyle::static_color(theme.accent_success))
            }
            SubagentBlockKind::Failed { .. } | SubagentBlockKind::Cancelled { .. } => {
                Some(AccentStyle::static_color(theme.accent_error))
            }
        }
    }

    fn has_vpad(&self, _ctx: &BlockContext) -> bool {
        false
    }

    fn has_raw_mode(&self) -> bool {
        false
    }

    fn is_foldable(&self) -> bool {
        false
    }

    fn default_display_mode(&self) -> DisplayMode {
        DisplayMode::Collapsed
    }

    fn is_selectable(&self) -> bool {
        true
    }

    fn has_bullet(&self, _ctx: &BlockContext) -> bool {
        true
    }

    fn is_groupable(&self) -> bool {
        true
    }
}
