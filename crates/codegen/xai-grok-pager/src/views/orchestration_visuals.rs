//! Alive multi-agent effort visuals (Heavy / Swarm / Swarm Heavy).
//!
//! Inspired by Claude Code's ultrathink/ultracode treatment: solid accent
//! colors for high modes, and a traveling rainbow shimmer for maximum mode.
//! Used by the `/effort` dropdown (hover + selection), the prompt footer
//! model chip, and subagent scrollback chrome.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use xai_grok_shell::sampling::types::OrchestrationMode;

use crate::theme::Theme;

/// Wall-clock phase shared with the welcome logo shimmer.
fn anim_phase_secs() -> f32 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// Quantized animation frame (~18 fps) so multi-agent chrome can request redraws.
pub fn multi_agent_anim_frame() -> u64 {
    (anim_phase_secs() * 18.0) as u64
}

/// Whether any UI surface currently needs multi-agent animation ticks.
pub fn needs_multi_agent_animation(mode: OrchestrationMode) -> bool {
    mode.is_multi_agent()
}

/// Resolve mode from a slash-arg insert token (`heavy`, `swarm`, `swarm-heavy`).
pub fn mode_from_effort_token(token: &str) -> OrchestrationMode {
    // insert_text for chained model phase can be "Grok 4.5 heavy" — take last word.
    let last = token
        .split_whitespace()
        .last()
        .unwrap_or(token)
        .trim()
        .trim_matches(|c: char| c == '(' || c == ')');
    OrchestrationMode::from_option_id(last)
}

/// Base accent for the mode (Claude-style solid brand, not theme-muted).
pub fn mode_base_color(mode: OrchestrationMode) -> Option<Color> {
    match mode {
        OrchestrationMode::Normal => None,
        // Heavy → hot red (council / intensity)
        OrchestrationMode::Heavy => Some(Color::Rgb(255, 70, 70)),
        // Swarm → purple (map→reduce network)
        OrchestrationMode::Swarm => Some(Color::Rgb(168, 85, 247)),
        // Swarm Heavy base (rainbow overrides per-glyph)
        OrchestrationMode::SwarmHeavy => Some(Color::Rgb(236, 72, 153)),
    }
}

/// Hover-tinted background for effort rows (subtle wash of the mode color).
pub fn mode_hover_bg(mode: OrchestrationMode, theme: &Theme) -> Option<Color> {
    let accent = mode_base_color(mode)?;
    Some(blend(theme.bg_light, accent, 0.22).unwrap_or(theme.bg_hover))
}

/// Selected-row background wash (stronger than hover).
pub fn mode_selected_bg(mode: OrchestrationMode, theme: &Theme) -> Option<Color> {
    let accent = mode_base_color(mode)?;
    Some(blend(theme.bg_visual, accent, 0.32).unwrap_or(theme.bg_visual))
}

/// Claude-like rainbow palette (ROYGBIV) for Swarm Heavy / ultracode energy.
const RAINBOW: &[(u8, u8, u8)] = &[
    (255, 70, 70),   // red
    (255, 140, 40),  // orange
    (250, 210, 50),  // yellow
    (80, 220, 120),  // green
    (60, 160, 255),  // blue
    (120, 90, 255),  // indigo
    (220, 80, 255),  // violet
];

fn rainbow_at(phase: f32, char_idx: usize) -> Color {
    let n = RAINBOW.len() as f32;
    // Traveling band: phase shifts the gradient along the string.
    let t = (char_idx as f32 * 0.55 + phase * 2.4).rem_euclid(n);
    let i0 = t.floor() as usize % RAINBOW.len();
    let i1 = (i0 + 1) % RAINBOW.len();
    let f = t - t.floor();
    let (r0, g0, b0) = RAINBOW[i0];
    let (r1, g1, b1) = RAINBOW[i1];
    Color::Rgb(
        lerp_u8(r0, r1, f),
        lerp_u8(g0, g1, f),
        lerp_u8(b0, b1, f),
    )
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

fn blend(a: Color, b: Color, t: f32) -> Option<Color> {
    crate::render::color::blend_color(a, b, t)
}

/// Soft pulse multiplier for solid modes (Heavy/Swarm) so hover feels alive.
fn solid_pulse(secs: f32) -> f32 {
    // 0.85 .. 1.0 breathing
    0.925 + 0.075 * (std::f32::consts::TAU * secs / 1.6).sin()
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

/// Style for a multi-agent effort label when selected or hovered.
pub fn mode_label_style(
    mode: OrchestrationMode,
    theme: &Theme,
    emphasized: bool, // selected or hovered
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

/// Build per-character spans for a label — solid pulse for Heavy/Swarm,
/// traveling rainbow for Swarm Heavy (Claude ultrathink energy).
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

    if matches!(mode, OrchestrationMode::SwarmHeavy) {
        let phase = anim_phase_secs();
        let bold = if emphasized {
            Modifier::BOLD
        } else {
            Modifier::empty()
        };
        return text
            .chars()
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
            .collect();
    }

    // Heavy / Swarm: single accent with breath pulse when emphasized.
    vec![Span::styled(
        text.to_string(),
        mode_label_style(mode, theme, emphasized, row_bg),
    )]
}

/// Footer chip: `◈ HEAVY` / `⬡ SWARM` / rainbow `⬢ SWARM HEAVY` next to the model.
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

/// Colored subagent chrome (`◈ Council `, `⬡ Swarm `, `⬢ SH `).
pub fn mode_chrome_style(mode: OrchestrationMode, theme: &Theme, bold: bool) -> Style {
    let bg = theme.bg_base;
    let mut s = mode_label_style(mode, theme, true, bg);
    if bold {
        s = s.add_modifier(Modifier::BOLD);
    }
    s
}
