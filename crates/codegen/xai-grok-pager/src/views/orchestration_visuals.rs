//! Full multi-agent design system (Heavy / Agent Swarm / Swarm Heavy).
//!
//! Visual language inspired by Claude Code ultrathink/ultracode:
//! - **Heavy** — hot red, solid with traveling shine band
//! - **Agent Swarm** — purple, solid with shine
//! - **Swarm Heavy** — full ROYGBIV rainbow shimmer (ultracode energy)
//!
//! Surfaces: `/effort` menu, prompt keyword glow, footer chip, activation
//! banner, and multi-line subagent cards.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use xai_grok_shell::sampling::types::OrchestrationMode;

use crate::theme::Theme;

// ── Motion ──────────────────────────────────────────────────────────────────

/// Wall-clock phase for all multi-agent animations.
pub fn anim_phase_secs() -> f32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Quantized animation frame (~18 fps).
pub fn multi_agent_anim_frame() -> u64 {
    if reduce_motion() {
        return 0;
    }
    (anim_phase_secs() * 18.0) as u64
}

/// Honor reduced-motion (Claude Code parity).
///
/// - `GROK_REDUCE_MOTION=1` / `true` / `yes`
/// - or macOS-style `REDUCE_MOTION=1`
pub fn reduce_motion() -> bool {
    for key in ["GROK_REDUCE_MOTION", "REDUCE_MOTION"] {
        if let Ok(v) = std::env::var(key) {
            let t = v.trim().to_ascii_lowercase();
            if matches!(t.as_str(), "1" | "true" | "yes" | "on") {
                return true;
            }
        }
    }
    false
}

pub fn needs_multi_agent_animation(mode: OrchestrationMode) -> bool {
    mode.is_multi_agent() && !reduce_motion()
}

// ── Mode resolution ─────────────────────────────────────────────────────────

pub fn mode_from_effort_token(token: &str) -> OrchestrationMode {
    let last = token
        .split_whitespace()
        .last()
        .unwrap_or(token)
        .trim()
        .trim_matches(|c: char| c == '(' || c == ')');
    OrchestrationMode::from_option_id(last)
}

/// Keywords that light up in the prompt like Claude’s `ultrathink`.
pub const MODE_KEYWORDS: &[(&str, OrchestrationMode)] = &[
    ("swarm-heavy", OrchestrationMode::SwarmHeavy),
    ("swarm_heavy", OrchestrationMode::SwarmHeavy),
    ("swarmheavy", OrchestrationMode::SwarmHeavy),
    ("ultracode", OrchestrationMode::SwarmHeavy),
    ("ultrathink", OrchestrationMode::SwarmHeavy),
    ("agent-swarm", OrchestrationMode::Swarm),
    ("agent_swarm", OrchestrationMode::Swarm),
    ("heavy", OrchestrationMode::Heavy),
    ("swarm", OrchestrationMode::Swarm),
];

/// Byte ranges of multi-agent keywords in `text` (longest match first).
pub fn find_mode_keyword_ranges(text: &str) -> Vec<(std::ops::Range<usize>, OrchestrationMode)> {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let mut hit: Option<(usize, OrchestrationMode)> = None;
        for &(kw, mode) in MODE_KEYWORDS {
            let kb = kw.as_bytes();
            if i + kb.len() <= bytes.len() && &bytes[i..i + kb.len()] == kb {
                // Word boundary: not alphanumeric on either side.
                let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
                let after_ok = i + kb.len() == bytes.len()
                    || !bytes[i + kb.len()].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    hit = Some((kb.len(), mode));
                    break; // MODE_KEYWORDS ordered longest-first
                }
            }
        }
        if let Some((len, mode)) = hit {
            out.push((i..i + len, mode));
            i += len;
        } else {
            i += 1;
        }
    }
    out
}

// ── Palette ─────────────────────────────────────────────────────────────────

pub fn mode_base_color(mode: OrchestrationMode) -> Option<Color> {
    match mode {
        OrchestrationMode::Normal => None,
        OrchestrationMode::Heavy => Some(Color::Rgb(255, 70, 70)),
        OrchestrationMode::Swarm => Some(Color::Rgb(168, 85, 247)),
        OrchestrationMode::SwarmHeavy => Some(Color::Rgb(236, 72, 153)),
    }
}

pub fn mode_hover_bg(mode: OrchestrationMode, theme: &Theme) -> Option<Color> {
    let accent = mode_base_color(mode)?;
    Some(blend(theme.bg_light, accent, 0.22).unwrap_or(theme.bg_hover))
}

pub fn mode_selected_bg(mode: OrchestrationMode, theme: &Theme) -> Option<Color> {
    let accent = mode_base_color(mode)?;
    Some(blend(theme.bg_visual, accent, 0.35).unwrap_or(theme.bg_visual))
}

/// Status / bullet color for running multi-agent work.
pub fn mode_running_color(mode: OrchestrationMode) -> Color {
    mode_base_color(mode).unwrap_or(Color::Rgb(100, 180, 255))
}

const RAINBOW: &[(u8, u8, u8)] = &[
    (255, 70, 70),
    (255, 140, 40),
    (250, 210, 50),
    (80, 220, 120),
    (60, 160, 255),
    (120, 90, 255),
    (220, 80, 255),
];

fn rainbow_at(phase: f32, char_idx: usize) -> Color {
    if reduce_motion() {
        return Color::Rgb(236, 72, 153);
    }
    let n = RAINBOW.len() as f32;
    let t = (char_idx as f32 * 0.55 + phase * 2.4).rem_euclid(n);
    let i0 = t.floor() as usize % RAINBOW.len();
    let i1 = (i0 + 1) % RAINBOW.len();
    let f = t - t.floor();
    let (r0, g0, b0) = RAINBOW[i0];
    let (r1, g1, b1) = RAINBOW[i1];
    Color::Rgb(lerp_u8(r0, r1, f), lerp_u8(g0, g1, f), lerp_u8(b0, b1, f))
}

/// Solid-color traveling shine (Claude ultrathink on a single hue).
fn solid_shimmer(base: Color, phase: f32, char_idx: usize, len: usize) -> Color {
    if reduce_motion() {
        return base;
    }
    let Color::Rgb(r, g, b) = base else {
        return base;
    };
    // Shine band sweeps 0..1 along the string.
    let pos = if len <= 1 {
        0.5
    } else {
        char_idx as f32 / (len.saturating_sub(1) as f32)
    };
    let band = (phase * 0.9).rem_euclid(1.4) - 0.2; // parks off ends briefly
    let d = (pos - band).abs();
    let shine = if d < 0.28 {
        0.55 * (1.0 + (std::f32::consts::PI * d / 0.28).cos()) * 0.5
    } else {
        0.0
    };
    let pulse = 0.06 * (0.5 - 0.5 * (std::f32::consts::TAU * phase / 1.8).cos());
    let k = (1.0 + shine + pulse).clamp(0.75, 1.35);
    Color::Rgb(
        (r as f32 * k).clamp(0.0, 255.0) as u8,
        (g as f32 * k).clamp(0.0, 255.0) as u8,
        (b as f32 * k).clamp(0.0, 255.0) as u8,
    )
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

fn blend(a: Color, b: Color, t: f32) -> Option<Color> {
    crate::render::color::blend_color(a, b, t)
}

fn solid_pulse(secs: f32) -> f32 {
    if reduce_motion() {
        return 1.0;
    }
    0.92 + 0.08 * (std::f32::consts::TAU * secs / 1.6).sin()
}

fn scale_rgb(c: Color, k: f32) -> Color {
    match c {
        Color::Rgb(r, g, b) => Color::Rgb(
            (r as f32 * k).clamp(0.0, 255.0) as u8,
            (g as f32 * k).clamp(0.0, 255.0) as u8,
            (b as f32 * k).clamp(0.0, 255.0) as u8,
        ),
        other => other,
    }
}

// ── Spans ───────────────────────────────────────────────────────────────────

pub fn mode_label_style(
    mode: OrchestrationMode,
    theme: &Theme,
    emphasized: bool,
    row_bg: Color,
) -> Style {
    let secs = anim_phase_secs();
    let base = mode_base_color(mode).unwrap_or(theme.text_primary);
    let fg = if matches!(mode, OrchestrationMode::Heavy | OrchestrationMode::Swarm) {
        scale_rgb(base, if emphasized { solid_pulse(secs) } else { 0.9 })
    } else {
        base
    };
    let mut style = Style::default().fg(fg).bg(row_bg);
    if emphasized {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

/// Per-glyph spans: solid shimmer (Heavy/Swarm) or rainbow (Swarm Heavy).
pub fn mode_label_spans(
    text: &str,
    mode: OrchestrationMode,
    theme: &Theme,
    emphasized: bool,
    row_bg: Color,
) -> Vec<Span<'static>> {
    if text.is_empty() {
        return vec![];
    }
    if !mode.is_multi_agent() {
        return vec![Span::styled(
            text.to_string(),
            Style::default().fg(theme.text_primary).bg(row_bg),
        )];
    }

    let phase = anim_phase_secs();
    let bold = if emphasized {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    match mode {
        OrchestrationMode::SwarmHeavy => chars
            .into_iter()
            .enumerate()
            .map(|(i, ch)| {
                Span::styled(
                    ch.to_string(),
                    Style::default()
                        .fg(rainbow_at(phase, i))
                        .bg(row_bg)
                        .add_modifier(bold),
                )
            })
            .collect(),
        OrchestrationMode::Heavy | OrchestrationMode::Swarm => {
            let base = mode_base_color(mode).unwrap_or(theme.text_primary);
            chars
                .into_iter()
                .enumerate()
                .map(|(i, ch)| {
                    Span::styled(
                        ch.to_string(),
                        Style::default()
                            .fg(solid_shimmer(base, phase, i, len))
                            .bg(row_bg)
                            .add_modifier(bold),
                    )
                })
                .collect()
        }
        OrchestrationMode::Normal => vec![Span::styled(
            text.to_string(),
            Style::default().fg(theme.text_primary).bg(row_bg),
        )],
    }
}

pub fn mode_footer_mark_spans(mode: OrchestrationMode, theme: &Theme) -> Vec<Span<'static>> {
    if !mode.is_multi_agent() {
        return vec![];
    }
    let mark = mode.mark();
    let mut spans = mode_label_spans(mark, mode, theme, true, theme.bg_base);
    spans.push(Span::styled(
        " · ".to_string(),
        Style::default().fg(theme.gray).bg(theme.bg_base),
    ));
    spans
}

pub fn mode_chrome_style(mode: OrchestrationMode, theme: &Theme, bold: bool) -> Style {
    let mut s = mode_label_style(mode, theme, true, theme.bg_base);
    if bold {
        s = s.add_modifier(Modifier::BOLD);
    }
    s
}

/// Color for a single character of a mode keyword (prompt glow).
pub fn mode_keyword_char_color(mode: OrchestrationMode, char_idx: usize, total_chars: usize) -> Color {
    let phase = anim_phase_secs();
    match mode {
        OrchestrationMode::SwarmHeavy => rainbow_at(phase, char_idx),
        OrchestrationMode::Heavy | OrchestrationMode::Swarm => {
            let base = mode_base_color(mode).unwrap_or(Color::Rgb(200, 200, 200));
            solid_shimmer(base, phase, char_idx, total_chars.max(1))
        }
        OrchestrationMode::Normal => Color::Rgb(180, 180, 180),
    }
}

// ── Activation banner (rich, multi-line) ────────────────────────────────────

/// Multi-line activation panel pushed into scrollback as system text.
///
/// Looks like a live status board, not a plain gray string.
pub fn rich_activation_banner(mode: OrchestrationMode) -> Option<String> {
    if !mode.is_multi_agent() {
        return None;
    }
    let (title, goal, pipeline, workers) = match mode {
        OrchestrationMode::Heavy => (
            "◈  HEAVY",
            "collaborative multi-agent council",
            "frame → parallel lenses → cross-check → synthesize",
            "Council members: Analyst · Skeptic · Explorer · Builder",
        ),
        OrchestrationMode::Swarm => (
            "⬡  AGENT SWARM",
            "parallel map → reduce over independent units",
            "decompose → fan-out → collect → merge",
            "Workers: unit agents (depth 1, parallel spawn)",
        ),
        OrchestrationMode::SwarmHeavy => (
            "⬢  SWARM HEAVY",
            "council → fan-out → verify (maximum multi-agent)",
            "H1 frame → S1 map → H2 verify → result",
            "Pipeline: council + swarm units + verifiers",
        ),
        OrchestrationMode::Normal => return None,
    };
    Some(format!(
        "\
╭──────────────────────────────────────────────────────────╮
│  {title:<54} │
│  {goal:<54} │
├──────────────────────────────────────────────────────────┤
│  ● LIVE   protocol loaded · workers appear as mode rows  │
│  {pipeline:<54} │
│  {workers:<54} │
│  Depth limit 1 · spawn with background:true for parallel │
╰──────────────────────────────────────────────────────────╯"
    ))
}

// ── Subagent multi-line card ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub enum SubagentCardState {
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// Build multi-line card lines for a multi-agent subagent row.
///
/// ```text
/// ◈ Council/Analyst          ● running
///   “review auth paths” — Thinking
///   explore · grok-4.5
/// ```
pub fn subagent_card_lines(
    mode: OrchestrationMode,
    role_label: &str,
    description: &str,
    activity: Option<&str>,
    meta: &str,
    state: SubagentCardState,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let bg = theme.bg_base;
    let muted = Style::default().fg(theme.gray).bg(bg);
    let dim = Style::default()
        .fg(blend(theme.bg_base, theme.gray, 0.7).unwrap_or(theme.gray))
        .bg(bg);

    let status = match state {
        SubagentCardState::Running => ("●", "running", mode_running_color(mode)),
        SubagentCardState::Completed => ("✓", "done", Color::Rgb(80, 200, 120)),
        SubagentCardState::Failed => ("✗", "failed", Color::Rgb(255, 90, 90)),
        SubagentCardState::Cancelled => ("○", "cancelled", theme.gray),
    };

    // Line 1: chrome + role | status
    let header_left = format!("{}{}", mode.subagent_chrome().trim_end(), role_label);
    let status_txt = format!("{} {}", status.0, status.1);
    let pad = width
        .saturating_sub(unicode_width::UnicodeWidthStr::width(header_left.as_str()))
        .saturating_sub(unicode_width::UnicodeWidthStr::width(status_txt.as_str()))
        .saturating_sub(1);

    let mut line1 = mode_label_spans(&header_left, mode, theme, true, bg);
    if pad > 0 {
        line1.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
    } else {
        line1.push(Span::styled(" ".to_string(), Style::default().bg(bg)));
    }
    line1.push(Span::styled(
        status_txt,
        Style::default()
            .fg(status.2)
            .bg(bg)
            .add_modifier(if matches!(state, SubagentCardState::Running) {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    ));

    // Line 2: description + activity
    let desc_budget = width.saturating_sub(2).max(8);
    let desc = crate::render::line_utils::truncate_str(description, desc_budget);
    let mut line2_text = format!("  “{desc}”");
    if let Some(act) = activity.map(str::trim).filter(|s| !s.is_empty()) {
        line2_text.push_str(" — ");
        line2_text.push_str(act);
    }
    let line2 = Line::from(Span::styled(
        crate::render::line_utils::truncate_str(&line2_text, width),
        muted,
    ));

    // Line 3: meta (type · model)
    let mut lines = vec![Line::from(line1), line2];
    if !meta.is_empty() {
        lines.push(Line::from(Span::styled(
            crate::render::line_utils::truncate_str(&format!("  {meta}"), width),
            dim,
        )));
    }
    lines
}
