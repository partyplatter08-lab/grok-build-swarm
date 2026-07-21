# Grok Build Swarm

**Public fork of [xai-org/grok-build](https://github.com/xai-org/grok-build)** with native multi-agent orchestration modes in the effort selector.

## What’s new

Three multi-agent modes appear **above High** in the effort selector (`/effort` and the model picker):

| Mode | Mark | Wire effort | Behavior |
|------|------|-------------|----------|
| **Swarm Heavy** | `⬢ SWARM HEAVY` | `xhigh` | Council → fan-out → verify pipeline |
| **Agent Swarm** | `⬡ SWARM` | `xhigh` | Kimi-style parallel map → reduce |
| **Heavy** | `◈ HEAVY` | `xhigh` | Collaborative multi-agent council |
| xhigh / high / medium / low | — | as named | Classic single-agent reasoning |

### Effort selector

```
/effort swarm-heavy
/effort swarm
/effort heavy
/effort high
```

Or pick them from the effort autocomplete dropdown. Mid-session switches are deferred like normal effort changes — **workers are not killed**.

### Visuals

- Footer / welcome show **“Swarm Heavy”**, **“Agent Swarm”**, **“Heavy”** (not bare `xhigh`)
- Mode activation prints an ASCII open banner in scrollback
- Subagent rows use mode chrome when descriptions are tagged:
  - `[Council/Analyst] …` → `◈ Council`
  - `[Swarm/u-3] …` → `⬡ Swarm`
  - `[SH/H1·Skeptic] …` → `⬢ SH`

### Meaningful multi-agent behavior

Selecting a multi-agent mode:

1. Sends **maximum reasoning** (`xhigh`) on the wire
2. Injects a full **orchestration protocol** into the session system prompt
3. Instructs the model to spawn parallel `spawn_subagent` workers with the right tags

Heavy = same problem, many lenses (debate).  
Swarm = many independent units (map/reduce).  
Swarm Heavy = both, in sequence.

## Build

Same as upstream Grok Build:

```sh
# Requirements: rustup (toolchain from rust-toolchain.toml), dotslash
cargo install dotslash
cargo build -p xai-grok-pager-bin --release
# binary: target/release/xai-grok-pager  (install as `grok` if you like)
cargo run -p xai-grok-pager-bin
```

## Tests

```sh
cargo test -p xai-grok-sampling-types --lib orchestration
cargo test -p xai-grok-pager --lib acp::model_state
cargo test -p xai-grok-pager --lib slash::commands::effort_levels
```

## Architecture notes

- Option **ids** (`heavy` / `swarm` / `swarm-heavy`) are presentation + protocol selection
- Option **values** all map to `ReasoningEffort::Xhigh` (API-compatible)
- `ModelState.reasoning_effort_option_id` tracks which multi-agent row is active
- Server-provided `reasoningEfforts` menus are **merged** so multi-agent modes always appear
- ACP `set_session_model` meta carries `orchestrationMode` for shell-side protocol inject

## Relationship to the plugin

The earlier `grok-agent-swarm` plugin is superseded by this native fork for real activation + UI. Skills from that plugin remain useful reference material for prompts.

## License

Same as upstream (Apache-2.0 / as shipped in `LICENSE`).
