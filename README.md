<div align="center">

# Grok Build Swarm (`grok-swarm`)

**Multi-agent coding agent for your terminal** — a public fork of
[xai-org/grok-build](https://github.com/xai-org/grok-build) with three native
effort modes: **Heavy**, **Agent Swarm**, and **Swarm Heavy**.

Installs as `grok-swarm` next to stock `grok`. They coexist safely.

</div>

---

## Install (one command)

```sh
curl -fsSL https://raw.githubusercontent.com/partyplatter08-lab/grok-build-swarm/main/install.sh | bash
```

Then open a **new terminal** (or run `source ~/.zshrc` / `source ~/.bashrc`) and:

```sh
grok-swarm
```

That’s it. The installer:

1. Downloads the binary for your Mac / Linux machine from [GitHub Releases](https://github.com/partyplatter08-lab/grok-build-swarm/releases)
2. Installs it to `~/.local/bin/grok-swarm` and `~/.grok/bin/grok-swarm`
3. Adds those folders to your shell `PATH` automatically
4. Turns on auto-update from GitHub Releases

### First launch

On first run you’ll authenticate the same way as stock Grok Build
(browser login, or reuse existing `~/.grok/` credentials if you already use `grok`).

### Update

```sh
grok-swarm update
# or re-run the installer any time
curl -fsSL https://raw.githubusercontent.com/partyplatter08-lab/grok-build-swarm/main/install.sh | bash
```

Only **one** `grok-swarm` binary is kept on disk (`~/.grok/downloads/…`).
Older versions are deleted after a successful install/update so 20 upgrades
do not leave 20 × ~150MB copies behind. PATH entries are symlinks, not copies.

---

## Quick start

```sh
# Interactive TUI (default)
grok-swarm

# Pick a multi-agent mode up front
grok-swarm --effort heavy
grok-swarm --effort swarm
grok-swarm --effort swarm-heavy

# One-shot (headless) task
grok-swarm -p "add unit tests for the auth module" --effort swarm
```

Inside the TUI you can also switch modes with:

```
/effort heavy
/effort swarm
/effort swarm-heavy
/effort high
```

Or use the model / effort picker UI.

---

## What are the multi-agent modes?

| Mode | Command | What it does |
|------|---------|--------------|
| **Heavy** | `--effort heavy` | Collaborative council — several agents debate the same problem, then a captain synthesizes |
| **Agent Swarm** | `--effort swarm` | Map → implement → verify — many workers in parallel on independent slices |
| **Swarm Heavy** | `--effort swarm-heavy` | Full pipeline: council first, then swarm fan-out, then final synthesis |

Classic single-agent efforts (`high`, `medium`, `low`, `xhigh`) still work as usual.

You’ll see live speaker turns in the feed (e.g. `▸ **Analyst:** …`) when workers talk, similar to Grok Heavy-style dialogue.

---

## “command not found: grok-swarm”

Almost always a **PATH** issue. Fix it in 30 seconds:

### 1. Re-run the installer

```sh
curl -fsSL https://raw.githubusercontent.com/partyplatter08-lab/grok-build-swarm/main/install.sh | bash
```

It now writes PATH into your shell config automatically.

### 2. Reload your shell

```sh
# zsh (default on modern macOS)
source ~/.zshrc

# bash
source ~/.bashrc
```

Or just **open a new terminal window**.

### 3. Run it by full path (always works after install)

```sh
~/.local/bin/grok-swarm --version
# or
~/.grok/bin/grok-swarm --version
```

### 4. Manual PATH (if needed)

Add this line to `~/.zshrc` or `~/.bashrc`, then reload:

```sh
export PATH="$HOME/.local/bin:$HOME/.grok/bin:$PATH"
```

### 5. Download failed for your platform?

Check that a binary exists for your OS/CPU:

👉 [github.com/partyplatter08-lab/grok-build-swarm/releases](https://github.com/partyplatter08-lab/grok-build-swarm/releases)

Asset names look like:

- `grok-swarm-<version>-macos-aarch64` — Apple Silicon Mac
- `grok-swarm-<version>-macos-x86_64` — Intel Mac
- `grok-swarm-<version>-linux-x86_64` — Linux Intel/AMD
- `grok-swarm-<version>-linux-aarch64` — Linux ARM

If your platform is missing, build from source (below) or open an issue.

---

## Build from source (optional)

Needs **Rust** (`rustup`) and **[DotSlash](https://dotslash-cli.com)** (for hermetic `protoc`).

```sh
git clone https://github.com/partyplatter08-lab/grok-build-swarm.git
cd grok-build-swarm
cargo install dotslash          # once
./scripts/install-cli.sh        # release build → ~/.local/bin/grok-swarm
```

Or manually:

```sh
cargo build -p xai-grok-pager-bin --release --bin grok-swarm
cp target/release/grok-swarm ~/.local/bin/grok-swarm
```

---

## Stock Grok Build vs this fork

| | Stock `grok` | This fork `grok-swarm` |
|--|--------------|------------------------|
| Install | `curl … x.ai/cli/install.sh` | `curl … install.sh` (this repo) |
| Binary name | `grok` | `grok-swarm` |
| Multi-agent modes | Grok Heavy (product) | Heavy / Swarm / Swarm Heavy (open fork) |
| Leader socket | `~/.grok/leader.sock` | `~/.grok/leader-swarm.sock` (isolated) |
| Auto-update | x.ai CDN | GitHub Releases |

You can keep both installed. Auth and config under `~/.grok/` are shared.

More technical detail: **[FORK.md](FORK.md)**.

---

## Documentation

- Upstream product docs: [docs.x.ai/build](https://docs.x.ai/build/overview)
- In-tree user guide: [`crates/codegen/xai-grok-pager/docs/user-guide/`](crates/codegen/xai-grok-pager/docs/user-guide/)
- Fork notes: [FORK.md](FORK.md)

---

## Repository layout (for contributors)

| Path | Contents |
|------|----------|
| `install.sh` | One-liner installer (PATH + GitHub Releases) |
| `scripts/install-cli.sh` | Build from source → install locally |
| `crates/codegen/xai-grok-pager-bin` | Binary package (`grok-swarm` + `xai-grok-pager`) |
| `crates/codegen/xai-grok-pager` | TUI |
| `crates/codegen/xai-grok-shell` | Agent runtime + multi-agent pipelines |
| `.github/workflows/release.yml` | Multi-platform release builds |

```sh
cargo check -p xai-grok-pager-bin
cargo test -p xai-grok-shell
```

> [!IMPORTANT]
> The root `Cargo.toml` is **generated** — treat it as read-only. Edit per-crate
> `Cargo.toml` files instead.

---

## License

First-party code: **Apache License 2.0** — see [`LICENSE`](LICENSE).

Third-party / vendored code keeps its original licenses — see
[`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES).

External contributions are not accepted by the upstream project; see
[`CONTRIBUTING.md`](CONTRIBUTING.md). This fork is provided as-is for the multi-agent work.
