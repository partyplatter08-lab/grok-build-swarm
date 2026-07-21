#!/usr/bin/env bash
# grok-swarm installer — install from GitHub Releases (no Rust toolchain needed)
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/partyplatter08-lab/grok-build-swarm/main/install.sh | bash
#   curl -fsSL ... | bash -s -- v0.2.106          # pin a tag/version
#   GROK_SWARM_BIN_DIR=~/bin bash install.sh     # custom install dir
#
# Installs `grok-swarm` next to stock `grok` (does not replace it).
# Enables GitHub auto-update via [cli] installer = "gh-release".

set -euo pipefail

REPO="${GROK_SWARM_REPO:-partyplatter08-lab/grok-build-swarm}"
BIN_NAME="grok-swarm"
GROK_HOME="${GROK_HOME:-$HOME/.grok}"
DOWNLOAD_DIR="${GROK_SWARM_DOWNLOAD_DIR:-$GROK_HOME/downloads}"
BIN_DIR="${GROK_SWARM_BIN_DIR:-$GROK_HOME/bin}"
LOCAL_BIN="${HOME}/.local/bin"
CHANNEL="${GROK_SWARM_CHANNEL:-stable}"
TARGET="${1:-}"

info()  { printf '→ %s\n' "$*" >&2; }
err()   { printf 'error: %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || err "'$1' is required"
}

need_cmd curl
need_cmd uname
need_cmd mktemp

os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os" in
  darwin) os="macos" ;;
  linux)  os="linux" ;;
  msys*|mingw*|cygwin*) os="windows" ;;
  *) err "unsupported OS: $os" ;;
esac
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) err "unsupported arch: $arch" ;;
esac
platform="${os}-${arch}"

# Resolve version: explicit arg, or latest GitHub release tag
if [[ -n "$TARGET" ]]; then
  version="${TARGET#v}"
  tag="v${version}"
else
  info "fetching latest release from github.com/${REPO}…"
  api="https://api.github.com/repos/${REPO}/releases/latest"
  tag="$(curl -fsSL -H 'Accept: application/vnd.github+json' \
    -H 'User-Agent: grok-swarm-install' "$api" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1)"
  [[ -n "$tag" ]] || err "could not resolve latest release tag from $api"
  version="${tag#v}"
fi

asset="${BIN_NAME}-${version}-${platform}"
url="https://github.com/${REPO}/releases/download/${tag}/${asset}"
if [[ "$os" == "windows" ]]; then
  asset="${asset}.exe"
  url="${url}.exe"
fi

mkdir -p "$DOWNLOAD_DIR" "$BIN_DIR"
tmp="$(mktemp "${DOWNLOAD_DIR}/${asset}.XXXXXX")"
trap 'rm -f "$tmp"' EXIT

info "downloading ${asset}…"
if ! curl -fsSL --retry 3 --retry-delay 1 -o "$tmp" "$url"; then
  err "download failed: $url
Is there a release asset for ${platform}? See https://github.com/${REPO}/releases"
fi

chmod +x "$tmp"
# Smoke-test before publishing
if ! "$tmp" --version >/dev/null 2>&1; then
  err "downloaded binary failed --version smoke test"
fi

dest="${DOWNLOAD_DIR}/${asset}"
mv -f "$tmp" "$dest"
trap - EXIT
chmod +x "$dest"

# Managed symlink in ~/.grok/bin/grok-swarm
link="${BIN_DIR}/${BIN_NAME}"
if [[ "$(dirname "$BIN_DIR")" == "$(dirname "$DOWNLOAD_DIR")" ]]; then
  rel="../downloads/${asset}"
else
  rel="$dest"
fi
ln -sfn "$rel" "$link"
info "linked ${link} → ${rel}"

# Also copy into ~/.local/bin for PATH convenience
if [[ -d "$LOCAL_BIN" ]] || mkdir -p "$LOCAL_BIN" 2>/dev/null; then
  cp -f "$dest" "${LOCAL_BIN}/${BIN_NAME}"
  chmod +x "${LOCAL_BIN}/${BIN_NAME}"
  info "copied ${LOCAL_BIN}/${BIN_NAME}"
fi

# Persist installer + auto_update in config.toml (best-effort, no deps)
config="${GROK_HOME}/config.toml"
mkdir -p "$GROK_HOME"
if [[ -f "$config" ]]; then
  if grep -q '^\[cli\]' "$config" 2>/dev/null; then
    # Update or insert installer / auto_update under [cli]
    if grep -q '^installer' "$config"; then
      # portable-ish sed
      if sed --version >/dev/null 2>&1; then
        sed -i 's/^installer.*/installer = "gh-release"/' "$config"
      else
        sed -i '' 's/^installer.*/installer = "gh-release"/' "$config"
      fi
    else
      # insert after [cli]
      if sed --version >/dev/null 2>&1; then
        sed -i '/^\[cli\]/a installer = "gh-release"' "$config"
      else
        # macOS: use a temp rewrite
        awk 'BEGIN{done=0} /^\[cli\]$/ && !done {print; print "installer = \"gh-release\""; done=1; next} {print}' \
          "$config" >"${config}.tmp" && mv "${config}.tmp" "$config"
      fi
    fi
    if grep -q '^auto_update' "$config"; then
      if sed --version >/dev/null 2>&1; then
        sed -i 's/^auto_update.*/auto_update = true/' "$config"
      else
        sed -i '' 's/^auto_update.*/auto_update = true/' "$config"
      fi
    else
      awk 'BEGIN{done=0} /^\[cli\]$/ && !done {print; print "auto_update = true"; done=1; next} {print}' \
        "$config" >"${config}.tmp" && mv "${config}.tmp" "$config"
    fi
  else
    printf '\n[cli]\ninstaller = "gh-release"\nauto_update = true\n' >>"$config"
  fi
else
  cat >"$config" <<EOF
[cli]
installer = "gh-release"
auto_update = true
EOF
fi

# Version cache for on-disk probes
cat >"${GROK_HOME}/version.json" <<EOF
{"version":"${version}","stable_version":"${version}","checked_at":"$(date -u +%Y-%m-%dT%H:%M:%SZ)"}
EOF

info "installed ${BIN_NAME} v${version} (${platform})"
echo
echo "Launch:"
echo "  ${BIN_NAME}"
echo "  ${BIN_NAME} --effort swarm-heavy"
echo
echo "Update later:"
echo "  ${BIN_NAME} update"
echo "  # or re-run this installer"
echo
case ":$PATH:" in
  *":$BIN_DIR:"*|*:${LOCAL_BIN}:*) ;;
  *)
    echo "Note: add to PATH if needed:"
    echo "  export PATH=\"${LOCAL_BIN}:${BIN_DIR}:\$PATH\""
    ;;
esac

# Print version if on PATH or via absolute path
if command -v "$BIN_NAME" >/dev/null 2>&1; then
  "$BIN_NAME" --version || true
else
  "$link" --version || true
fi
