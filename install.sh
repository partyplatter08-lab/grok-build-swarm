#!/usr/bin/env bash
# grok-swarm installer — install from GitHub Releases (no Rust toolchain needed)
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/partyplatter08-lab/grok-build-swarm/main/install.sh | bash
#   curl -fsSL ... | bash -s -- v0.2.106          # pin a tag/version
#   GROK_SWARM_BIN_DIR=~/bin bash install.sh     # custom install dir
#
# Installs `grok-swarm` next to stock `grok` (does not replace it).
# Puts the binary on your PATH automatically (shell rc + ~/.local/bin).

set -euo pipefail

REPO="${GROK_SWARM_REPO:-partyplatter08-lab/grok-build-swarm}"
BIN_NAME="grok-swarm"
GROK_HOME="${GROK_HOME:-$HOME/.grok}"
DOWNLOAD_DIR="${GROK_SWARM_DOWNLOAD_DIR:-$GROK_HOME/downloads}"
BIN_DIR="${GROK_SWARM_BIN_DIR:-$GROK_HOME/bin}"
LOCAL_BIN="${HOME}/.local/bin"
TARGET="${1:-}"

info()  { printf '→ %s\n' "$*" >&2; }
ok()    { printf '✓ %s\n' "$*" >&2; }
warn()  { printf '! %s\n' "$*" >&2; }
err()   { printf 'error: %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || err "'$1' is required (install it and re-run)"
}

path_has_dir() {
  case ":$PATH:" in *":$1:"*) return 0 ;; *) return 1 ;; esac
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
  *) err "unsupported OS: $(uname -s)
Supported: macOS (Intel + Apple Silicon), Linux (x86_64 + aarch64)" ;;
esac
case "$arch" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) err "unsupported architecture: $(uname -m)
Supported: x86_64, aarch64/arm64" ;;
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
  [[ -n "$tag" ]] || err "could not resolve latest release tag from $api
Check https://github.com/${REPO}/releases"
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
  err "download failed for your platform (${platform}).

  URL: $url

Common causes:
  • No release asset yet for ${platform}
  • Network / GitHub rate limit

See available builds:
  https://github.com/${REPO}/releases

Or build from source:
  git clone https://github.com/${REPO}.git
  cd grok-build-swarm && ./scripts/install-cli.sh"
fi

chmod +x "$tmp"
# macOS: re-sign after download (curl can leave a broken signature → SIGKILL).
if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
  codesign -s - --force --timestamp=none "$tmp" 2>/dev/null || true
fi
# Smoke-test before publishing
if ! "$tmp" --version >/dev/null 2>&1; then
  err "downloaded binary failed --version smoke test
File may be corrupt or blocked by security software. Re-run the installer."
fi

dest="${DOWNLOAD_DIR}/${asset}"
mv -f "$tmp" "$dest"
trap - EXIT
chmod +x "$dest"
if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
  codesign -s - --force --timestamp=none "$dest" 2>/dev/null || true
fi

# Managed symlink in ~/.grok/bin/grok-swarm
link="${BIN_DIR}/${BIN_NAME}"
if [[ "$(dirname "$BIN_DIR")" == "$(dirname "$DOWNLOAD_DIR")" ]]; then
  rel="../downloads/${asset}"
else
  rel="$dest"
fi
ln -sfn "$rel" "$link"
ok "linked ${link} → ${rel}"

# Always copy into ~/.local/bin (most systems already have this on PATH)
PATH_READY=""
if [[ -d "$LOCAL_BIN" ]] || mkdir -p "$LOCAL_BIN" 2>/dev/null; then
  cp -f "$dest" "${LOCAL_BIN}/${BIN_NAME}"
  chmod +x "${LOCAL_BIN}/${BIN_NAME}"
  if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
    codesign -s - --force --timestamp=none "${LOCAL_BIN}/${BIN_NAME}" 2>/dev/null || true
  fi
  ok "installed ${LOCAL_BIN}/${BIN_NAME}"
  if path_has_dir "$LOCAL_BIN" || path_has_dir "$BIN_DIR"; then
    PATH_READY="yes"
  fi
fi

# If ~/.local/bin is not writable/usable, try /usr/local/bin when already on PATH
if [[ -z "$PATH_READY" ]] && [[ "$os" != "windows" ]]; then
  for candidate in "/usr/local/bin"; do
    if path_has_dir "$candidate" && [[ -d "$candidate" ]] && [[ -w "$candidate" ]]; then
      ln -sfn "$link" "${candidate}/${BIN_NAME}"
      ok "symlinked ${candidate}/${BIN_NAME} → ${link}"
      PATH_READY="yes"
      break
    fi
  done
fi

# Persist installer + auto_update in config.toml (best-effort, no deps)
config="${GROK_HOME}/config.toml"
mkdir -p "$GROK_HOME"
if [[ -f "$config" ]]; then
  if grep -q '^\[cli\]' "$config" 2>/dev/null; then
    if grep -q '^installer' "$config"; then
      if sed --version >/dev/null 2>&1; then
        sed -i 's/^installer.*/installer = "gh-release"/' "$config"
      else
        sed -i '' 's/^installer.*/installer = "gh-release"/' "$config"
      fi
    else
      if sed --version >/dev/null 2>&1; then
        sed -i '/^\[cli\]/a installer = "gh-release"' "$config"
      else
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
cat >"${GROK_HOME}/version-swarm.json" <<EOF
{"version":"${version}","stable_version":"${version}","checked_at":"$(date -u +%Y-%m-%dT%H:%M:%SZ)"}
EOF

# --- Ensure grok-swarm is on PATH for future shells (and this one when possible) ---
user_shell="$(basename "${SHELL:-}")"
config_file=""
case "$user_shell" in
  bash) config_file="$HOME/.bashrc" ;;
  zsh)  config_file="$HOME/.zshrc" ;;
  fish) config_file="$HOME/.config/fish/config.fish" ;;
esac

# Prefer writing our own marker so we do not clobber the stock grok PATH block.
MARKER_OPEN='# >>> grok-swarm installer >>>'
MARKER_CLOSE='# <<< grok-swarm installer <<<'

if [[ -n "$config_file" ]]; then
  mkdir -p "$(dirname "$config_file")"

  # Resolve symlinks so tmp+mv rewrites the real file (dotfiles/stow).
  if [[ -e "$config_file" ]] || [[ -L "$config_file" ]]; then
    _cf="$config_file"
    _depth=0
    while [[ -L "$_cf" ]] && [[ "$_depth" -lt 40 ]]; do
      _link="$(readlink "$_cf")" || break
      case "$_link" in
        /*) _cf="$_link" ;;
        *)  _cf="$(cd "$(dirname "$_cf")" && pwd -P)/$_link" ;;
      esac
      _depth=$((_depth + 1))
    done
    if [[ ! -L "$_cf" ]]; then
      config_file="$(cd "$(dirname "$_cf")" && pwd -P)/$(basename "$_cf")"
    fi
    unset _cf _link _depth
  fi

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
  else
    if [[ -f "$config_file" ]]; then
      cp "$config_file" "$config_file.bak.$(date +%s)" 2>/dev/null || true
    fi
    # macOS bash login shells often only read .bash_profile
    if [[ "$user_shell" == "bash" ]] && [[ "$(uname -s)" == "Darwin" ]]; then
      if [[ -f "$HOME/.bash_profile" ]] && ! grep -qs "source ~/.bashrc" "$HOME/.bash_profile" 2>/dev/null; then
        printf '\n[[ -r ~/.bashrc ]] && source ~/.bashrc\n' >>"$HOME/.bash_profile"
      fi
    fi
  fi

  printf '\n%s\n' "$new_block" >>"$config_file"
  ok "added PATH to ${config_file}"
fi

# Export for the current (piped) shell session when possible
export PATH="${LOCAL_BIN}:${BIN_DIR}:${PATH}"

ok "installed ${BIN_NAME} v${version} (${platform})"
echo >&2
echo "Get started:" >&2
echo "  ${BIN_NAME}" >&2
echo "  ${BIN_NAME} --effort heavy" >&2
echo "  ${BIN_NAME} --effort swarm" >&2
echo "  ${BIN_NAME} --effort swarm-heavy" >&2
echo >&2
echo "Update later:" >&2
echo "  ${BIN_NAME} update" >&2
echo "  # or re-run this installer" >&2
echo >&2

# Can we run it right now?
if command -v "$BIN_NAME" >/dev/null 2>&1; then
  "$BIN_NAME" --version || true
  echo >&2
  ok "ready — type: ${BIN_NAME}"
elif [[ -x "$link" ]]; then
  "$link" --version || true
  echo >&2
  if [[ -n "$config_file" ]]; then
    warn "PATH updated in ${config_file}."
    warn "Open a new terminal window, then type: ${BIN_NAME}"
    warn "Or run now:  ${link}"
  else
    warn "Add to PATH, then open a new terminal:"
    echo "  export PATH=\"\$HOME/.local/bin:\$HOME/.grok/bin:\$PATH\"" >&2
    warn "Or run now:  ${link}"
  fi
else
  err "install finished but binary is missing at ${link}"
fi
