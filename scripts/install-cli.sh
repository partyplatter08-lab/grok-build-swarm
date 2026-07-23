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

# One managed copy under ~/.grok/downloads (survives cargo clean of target/).
# Everything else is a symlink — no duplicate 150MB binaries.
DOWNLOAD_DIR="${GROK_HOME:-$HOME/.grok}/downloads"
mkdir -p "$DOWNLOAD_DIR" "$HOME/.grok/bin" "$PREFIX"

# Peek version from the build artifact before installing
VER="$("$SRC" --version 2>/dev/null | head -1 | awk '{print $2}')"
[[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]] || VER="0.0.0-dev"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Darwin-aarch64) ASSET_NAME="grok-swarm-${VER}-macos-aarch64" ;;
  Darwin-x86_64) ASSET_NAME="grok-swarm-${VER}-macos-x86_64" ;;
  Linux-x86_64|Linux-amd64) ASSET_NAME="grok-swarm-${VER}-linux-x86_64" ;;
  Linux-aarch64|Linux-arm64) ASSET_NAME="grok-swarm-${VER}-linux-aarch64" ;;
  *) ASSET_NAME="grok-swarm-${VER}-unknown" ;;
esac
ASSET="$DOWNLOAD_DIR/$ASSET_NAME"

cp -f "$SRC" "$ASSET"
chmod +x "$ASSET"
if command -v codesign >/dev/null 2>&1; then
  codesign -s - --force --timestamp=none "$ASSET" 2>/dev/null || true
fi

# Drop previous grok-swarm downloads (keep only the asset we just wrote)
for old in "$DOWNLOAD_DIR"/grok-swarm-*; do
  [[ -f "$old" ]] || continue
  [[ "$(basename "$old")" == "$ASSET_NAME" ]] && continue
  echo "→ removing old $(basename "$old")"
  rm -f "$old"
done

ln -sfn "$ASSET" "$HOME/.grok/bin/grok-swarm"
# PREFIX gets a symlink (not a copy) so rebuild/clean of target/ is fine
DEST="$PREFIX/grok-swarm"
ln -sfn "$HOME/.grok/bin/grok-swarm" "$DEST"

checked_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cat >"${GROK_HOME:-$HOME/.grok}/version.json" <<EOF
{"version":"${VER}","stable_version":"${VER}","checked_at":"${checked_at}"}
EOF

echo "→ installed: $DEST → $ASSET"
"$DEST" --version || true

# Put PREFIX + ~/.grok/bin on PATH for future shells
user_shell="$(basename "${SHELL:-}")"
config_file=""
case "$user_shell" in
  bash) config_file="$HOME/.bashrc" ;;
  zsh)  config_file="$HOME/.zshrc" ;;
  fish) config_file="$HOME/.config/fish/config.fish" ;;
esac
MARKER_OPEN='# >>> grok-swarm installer >>>'
MARKER_CLOSE='# <<< grok-swarm installer <<<'
if [[ -n "$config_file" ]]; then
  mkdir -p "$(dirname "$config_file")"
  if [[ "$user_shell" == "fish" ]]; then
    new_block="${MARKER_OPEN}
fish_add_path \$HOME/.local/bin
fish_add_path \$HOME/.grok/bin
${MARKER_CLOSE}"
  else
    new_block="${MARKER_OPEN}
export PATH=\"\$HOME/.local/bin:\$HOME/.grok/bin:\$PATH\"
${MARKER_CLOSE}"
  fi
  if grep -qs "grok-swarm installer" "$config_file" 2>/dev/null; then
    tmp_rc="$config_file.tmp.$$"
    awk '
      /# >>> grok-swarm installer >>>/ { skip=1; next }
      /# <<< grok-swarm installer <<</ { skip=0; next }
      !skip { print }
    ' "$config_file" >"$tmp_rc" && mv "$tmp_rc" "$config_file"
  fi
  printf '\n%s\n' "$new_block" >>"$config_file"
  echo "→ added PATH to $config_file"
fi
export PATH="$PREFIX:${HOME}/.grok/bin:${PATH}"

case ":$PATH:" in
  *":$PREFIX:"*) ;;
  *)
    echo
    echo "Note: $PREFIX may not be active in this shell yet. Open a new terminal, or:"
    echo "  export PATH=\"$PREFIX:\$PATH\""
    ;;
esac

echo
echo "Launch a normal interactive session with:"
echo "  grok-swarm"
echo "  grok-swarm --effort swarm-heavy"
echo "  grok-swarm -p 'hello' --effort heavy   # headless"
