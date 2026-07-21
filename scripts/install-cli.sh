#!/usr/bin/env bash
# Install the Grok Build Swarm CLI as `grok-swarm` on your PATH.
#
# Usage:
#   ./scripts/install-cli.sh           # release build → ~/.local/bin/grok-swarm
#   ./scripts/install-cli.sh --debug   # faster debug build
#   PREFIX=~/bin ./scripts/install-cli.sh
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

PROFILE=release
BIN_SRC="target/release/grok-swarm"
for arg in "$@"; do
  case "$arg" in
    --debug)
      PROFILE=debug
      BIN_SRC="target/debug/grok-swarm"
      ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
  esac
done

PREFIX="${PREFIX:-$HOME/.local/bin}"
mkdir -p "$PREFIX"

echo "→ building grok-swarm ($PROFILE)…"
if [[ "$PROFILE" == "release" ]]; then
  cargo build -p xai-grok-pager-bin --release --bin grok-swarm
else
  cargo build -p xai-grok-pager-bin --bin grok-swarm
fi

SRC="$ROOT/$BIN_SRC"
if [[ ! -x "$SRC" ]]; then
  # Older checkouts may only produce xai-grok-pager; fall back to that artifact.
  FALLBACK="${SRC/grok-swarm/xai-grok-pager}"
  if [[ -x "$FALLBACK" ]]; then
    SRC="$FALLBACK"
  else
    echo "error: expected binary not found at $BIN_SRC" >&2
    exit 1
  fi
fi

DEST="$PREFIX/grok-swarm"
# Copy (not symlink) so the command keeps working if you rebuild/clean target/.
cp -f "$SRC" "$DEST"
chmod +x "$DEST"

# Also mirror under ~/.grok/bin when present (stock grok lives there).
if [[ -d "$HOME/.grok/bin" ]]; then
  cp -f "$SRC" "$HOME/.grok/bin/grok-swarm"
  chmod +x "$HOME/.grok/bin/grok-swarm"
  echo "→ also installed to $HOME/.grok/bin/grok-swarm"
fi

echo "→ installed: $DEST"
"$DEST" --version || true

case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *)
    echo
    echo "Note: $PREFIX is not on your PATH. Add this to your shell rc:"
    echo "  export PATH=\"$PREFIX:\$PATH\""
    ;;
esac

echo
echo "Launch a normal interactive session with:"
echo "  grok-swarm"
echo "  grok-swarm --effort swarm-heavy"
echo "  grok-swarm -p 'hello' --effort heavy   # headless"
