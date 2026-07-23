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

# Human-readable byte size (e.g. 152M)
human_bytes() {
  local n="${1:-0}"
  if command -v awk >/dev/null 2>&1; then
    awk -v n="$n" 'BEGIN{
      split("B K M G T", u, " ")
      i=1
      while (n >= 1024 && i < 5) { n/=1024; i++ }
      if (i==1) printf "%d%s", n, u[i]
      else printf "%.1f%s", n, u[i]
    }'
  else
    printf '%sB' "$n"
  fi
}

# Multi-step progress bar: [████████░░░░]  50%  (3/6) verifying…
# Uses only bash + printf (no deps). Safe when stdout is piped.
_STEP_TOTAL=7
_STEP_CUR=0

step_begin() {
  _STEP_TOTAL="${1:-7}"
  _STEP_CUR=0
}

step() {
  # step "label"  — one line per phase so install history stays visible
  _STEP_CUR=$((_STEP_CUR + 1))
  local label="$*"
  local width=28
  local filled=$(( _STEP_CUR * width / _STEP_TOTAL ))
  local empty=$(( width - filled ))
  local pct=$(( _STEP_CUR * 100 / _STEP_TOTAL ))
  printf '[' >&2
  if [[ "$filled" -gt 0 ]]; then
    printf '%*s' "$filled" '' | tr ' ' '=' >&2
  fi
  if [[ "$empty" -gt 0 ]]; then
    printf '%*s' "$empty" '' | tr ' ' '.' >&2
  fi
  printf '] %3d%%  (%d/%d) %s\n' "$pct" "$_STEP_CUR" "$_STEP_TOTAL" "$label" >&2
}

# Download with a live progress bar (curl --progress-bar). Falls back to a
# spinner if the transfer has no Content-Length (rare for GitHub Releases).
download_with_progress() {
  local url="$1" out="$2" label="${3:-downloading}"
  local size="" code

  # Probe size for a nicer label (best-effort; ignore failures / redirects body)
  size="$(curl -fsSLI -A 'grok-swarm-install' "$url" 2>/dev/null \
    | tr -d '\r' \
    | awk 'tolower($1)=="content-length:"{print $2; exit}')" || true

  if [[ -n "$size" && "$size" -gt 0 ]] 2>/dev/null; then
    info "${label} $(human_bytes "$size")…"
  else
    info "${label}…"
  fi

  # -f fail on HTTP errors, -L follow redirects, -# progress bar to stderr,
  # --retry for flaky networks. Do NOT use -s (it hides the bar).
  if curl -fL --progress-bar --retry 3 --retry-delay 1 \
      -A 'grok-swarm-install' \
      -o "$out" "$url"; then
    # Ensure the progress bar ends with a newline (curl usually does).
    printf '\n' >&2
    return 0
  fi
  printf '\n' >&2
  return 1
}

# Copy a large file while showing a simple byte progress bar.
copy_with_progress() {
  local src="$1" dst="$2" label="${3:-installing}"
  local total cur pct width filled empty
  total="$(wc -c <"$src" 2>/dev/null | tr -d ' ')" || total=0

  # Fast path for small files
  if [[ -z "$total" || "$total" -lt 1048576 ]]; then
    cp -f "$src" "$dst"
    return 0
  fi

  # Background copy + poll destination size
  cp -f "$src" "$dst" &
  local pid=$!
  width=28
  while kill -0 "$pid" 2>/dev/null; do
    cur=0
    if [[ -f "$dst" ]]; then
      cur="$(wc -c <"$dst" 2>/dev/null | tr -d ' ')" || cur=0
    fi
    if [[ "$total" -gt 0 ]]; then
      pct=$(( cur * 100 / total ))
      [[ "$pct" -gt 100 ]] && pct=100
      filled=$(( pct * width / 100 ))
      empty=$(( width - filled ))
      printf '\r\033[K[' >&2
      [[ "$filled" -gt 0 ]] && printf '%*s' "$filled" '' | tr ' ' '=' >&2
      [[ "$empty" -gt 0 ]] && printf '%*s' "$empty" '' | tr ' ' '.' >&2
      printf '] %3d%%  %s %s / %s' \
        "$pct" "$label" "$(human_bytes "$cur")" "$(human_bytes "$total")" >&2
    else
      printf '\r\033[K→ %s %s…' "$label" "$(human_bytes "$cur")" >&2
    fi
    sleep 0.15
  done
  wait "$pid"
  local rc=$?
  if [[ "$rc" -eq 0 && "$total" -gt 0 ]]; then
    printf '\r\033[K[' >&2
    printf '%*s' "$width" '' | tr ' ' '=' >&2
    printf '] 100%%  %s %s\n' "$label" "$(human_bytes "$total")" >&2
  else
    printf '\n' >&2
  fi
  return "$rc"
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

# ── Overall install steps (download is its own live bar) ──────────────────
# Steps after download: verify → place binary → install to PATH → codesign
# → write config → configure shell PATH → done
step_begin 6

info "release ${tag} · ${platform}"
if ! download_with_progress "$url" "$tmp" "downloading ${asset}"; then
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

step "verifying binary…"
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

step "installing to ${BIN_DIR}…"
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

# Always copy into ~/.local/bin (most systems already have this on PATH)
step "copying to ${LOCAL_BIN}…"
PATH_READY=""
if [[ -d "$LOCAL_BIN" ]] || mkdir -p "$LOCAL_BIN" 2>/dev/null; then
  # Large binary (~150MB) — show copy progress so install doesn't look stuck
  if ! copy_with_progress "$dest" "${LOCAL_BIN}/${BIN_NAME}" "installing"; then
    err "failed to copy binary to ${LOCAL_BIN}/${BIN_NAME}"
  fi
  chmod +x "${LOCAL_BIN}/${BIN_NAME}"
  if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
    codesign -s - --force --timestamp=none "${LOCAL_BIN}/${BIN_NAME}" 2>/dev/null || true
  fi
  if path_has_dir "$LOCAL_BIN" || path_has_dir "$BIN_DIR"; then
    PATH_READY="yes"
  fi
fi

# If ~/.local/bin is not writable/usable, try /usr/local/bin when already on PATH
if [[ -z "$PATH_READY" ]] && [[ "$os" != "windows" ]]; then
  for candidate in "/usr/local/bin"; do
    if path_has_dir "$candidate" && [[ -d "$candidate" ]] && [[ -w "$candidate" ]]; then
      ln -sfn "$link" "${candidate}/${BIN_NAME}"
      PATH_READY="yes"
      break
    fi
  done
fi

step "writing config…"
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

step "configuring PATH…"
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
fi

step "done"
ok "installed ${BIN_NAME} v${version} (${platform})"
ok "binary: ${link}"
[[ -x "${LOCAL_BIN}/${BIN_NAME}" ]] && ok "binary: ${LOCAL_BIN}/${BIN_NAME}"
[[ -n "${config_file:-}" ]] && ok "PATH configured in ${config_file}"
echo >&2

# Print version via absolute path (reliable even when PATH is not ready yet)
if [[ -x "$link" ]]; then
  "$link" --version || true
elif [[ -x "${LOCAL_BIN}/${BIN_NAME}" ]]; then
  "${LOCAL_BIN}/${BIN_NAME}" --version || true
fi
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

# Was the binary already reachable on the *incoming* PATH (before we wrote rc)?
# curl|bash runs in a subshell, so exporting PATH here never helps the parent shell.
if path_has_dir "$LOCAL_BIN" || path_has_dir "$BIN_DIR" || [[ -n "${PATH_READY:-}" ]]; then
  if command -v "$BIN_NAME" >/dev/null 2>&1; then
    ok "ready — type: ${BIN_NAME}"
  else
    ok "installed. In this terminal run:  export PATH=\"\$HOME/.local/bin:\$HOME/.grok/bin:\$PATH\""
    ok "then:  ${BIN_NAME}"
  fi
elif [[ -n "$config_file" ]]; then
  warn "PATH was written to ${config_file}."
  warn "Open a NEW terminal window, then type:  ${BIN_NAME}"
  echo >&2
  echo "Or activate in this terminal now:" >&2
  echo "  source ${config_file}" >&2
  echo "  # or:" >&2
  echo "  export PATH=\"\$HOME/.local/bin:\$HOME/.grok/bin:\$PATH\"" >&2
  echo "  ${BIN_NAME}" >&2
  echo >&2
  echo "Or run without PATH:" >&2
  echo "  ${LOCAL_BIN}/${BIN_NAME}" >&2
else
  warn "Add to PATH, then open a new terminal:"
  echo "  export PATH=\"\$HOME/.local/bin:\$HOME/.grok/bin:\$PATH\"" >&2
  echo "Or run:  ${LOCAL_BIN}/${BIN_NAME}" >&2
fi
