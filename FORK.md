# Grok Build Swarm

**Public fork of [xai-org/grok-build](https://github.com/xai-org/grok-build)** with native multi-agent orchestration modes in the effort selector.

## Install (one-liner, like stock `grok`)

Stock Grok Build stays as `grok`. This fork installs as **`grok-swarm`** so both coexist.

```sh
curl -fsSL https://raw.githubusercontent.com/partyplatter08-lab/grok-build-swarm/main/install.sh | bash
```

That downloads the latest **GitHub Release** binary for your platform into
`~/.grok/downloads/`, installs `~/.local/bin/grok-swarm` + `~/.grok/bin/grok-swarm`,
**writes those dirs onto your shell PATH**, and turns on **auto-update**
(`[cli] installer = "gh-release"`, `auto_update = true`).

If you see `command not found: grok-swarm`, open a new terminal (or
`source ~/.zshrc` / `source ~/.bashrc`), or run `~/.local/bin/grok-swarm` directly.
See the main [README](README.md) troubleshooting section.

Then launch a normal interactive session:

```sh
grok-swarm
grok-swarm --effort swarm-heavy
grok-swarm "scaffold a monorepo" --effort swarm
grok-swarm -p "quick question" --effort heavy   # headless one-shot
```

### Updates

With auto-update enabled (default from the installer):

- On launch the app checks GitHub Releases for a newer `grok-swarm` and
  downloads it in the background (same idea as stock `grok`).
- Force an update any time:

```sh
grok-swarm update
# or re-run the installer
curl -fsSL https://raw.githubusercontent.com/partyplatter08-lab/grok-build-swarm/main/install.sh | bash
```

### Build from source (optional)

```sh
git clone https://github.com/partyplatter08-lab/grok-build-swarm.git
cd grok-build-swarm
./scripts/install-cli.sh          # release build
# or: ./scripts/install-cli.sh --debug
```

Requires Rust + [DotSlash](https://dotslash-cli.com). Auth uses the same
`~/.grok/` credentials as stock `grok`.

### Releases (required for every change)

**Policy:** every change that should be testable with `grok-swarm update`
must ship a **new GitHub Release** (new semver). Code on `main` alone is
not enough — the updater only looks at release tags.

```sh
# One-shot: bump patch, build, tag, push, upload asset
./scripts/ship-release.sh

# Then on any machine:
grok-swarm update
grok-swarm --version
```

Options: `--minor`, `--major`, `--set 0.2.110`, `--skip-build`, `--no-push`.

Assets are named `grok-swarm-{version}-{os}-{arch}`
(e.g. `grok-swarm-0.2.107-macos-aarch64`). Leaving the tag at the same
version forever makes `update` report “up to date” while `main` moves on.

Optional CI (`.github/workflows/release.yml`) can also build multi-platform
binaries on tag push.

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
