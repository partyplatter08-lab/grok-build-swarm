#!/usr/bin/env bash
# Ship a new grok-swarm GitHub Release so `grok-swarm update` picks up the change.
#
# Policy: every user-facing change that should be testable via
#   grok-swarm update
# must ship a new semver (patch bump by default). Code on main alone is NOT
# enough — the updater only looks at GitHub release tags.
#
# Usage:
#   ./scripts/ship-release.sh                  # bump patch, build, tag, upload
#   ./scripts/ship-release.sh --minor          # 0.2.x → 0.3.0
#   ./scripts/ship-release.sh --major          # 0.x → 1.0.0
#   ./scripts/ship-release.sh --set 0.2.110    # exact version
#   ./scripts/ship-release.sh --skip-build     # reuse target/release/grok-swarm
#   ./scripts/ship-release.sh --no-push        # tag/release locally only
#   ./scripts/ship-release.sh --notes "fix x" # release notes body
#
# Requires: git, gh (authenticated), cargo, curl. macOS arm64 host by default.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

REPO="${GROK_SWARM_REPO:-partyplatter08-lab/grok-build-swarm}"
REMOTE="${GROK_SWARM_REMOTE:-fork}"
BRANCH="${GROK_SWARM_BRANCH:-swarm-public}"
BUMP="patch"
SET_VER=""
SKIP_BUILD=0
NO_PUSH=0
NOTES=""
PLATFORMS=()  # filled after detect

info() { printf '→ %s\n' "$*" >&2; }
ok()   { printf '✓ %s\n' "$*" >&2; }
err()  { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
  sed -n '2,22p' "$0"
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --patch) BUMP=patch; shift ;;
    --minor) BUMP=minor; shift ;;
    --major) BUMP=major; shift ;;
    --set)
      [[ $# -ge 2 ]] || err "--set needs a version"
      SET_VER="$2"
      shift 2
      ;;
    --set=*)
      SET_VER="${1#--set=}"
      shift
      ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    --no-push) NO_PUSH=1; shift ;;
    --notes)
      [[ $# -ge 2 ]] || err "--notes needs text"
      NOTES="$2"
      shift 2
      ;;
    --notes=*)
      NOTES="${1#--notes=}"
      shift
      ;;
    -h|--help) usage ;;
    *) err "unknown arg: $1 (try --help)" ;;
  esac
done

need() { command -v "$1" >/dev/null 2>&1 || err "'$1' is required"; }
need git
need gh
need cargo

# ── current version from pager-bin Cargo.toml ─────────────────────────────
VERSION_FILES=(
  crates/codegen/xai-grok-pager-bin/Cargo.toml
  crates/codegen/xai-grok-version/Cargo.toml
  crates/codegen/xai-grok-pager/Cargo.toml
  crates/codegen/xai-grok-shell/Cargo.toml
)

current_version() {
  sed -n 's/^version = "\([0-9][^"]*\)"/\1/p' \
    crates/codegen/xai-grok-pager-bin/Cargo.toml | head -1
}

bump_semver() {
  local v="$1" kind="$2"
  local major minor patch
  IFS=. read -r major minor patch <<<"$v"
  patch="${patch%%-*}"  # strip any pre-release
  case "$kind" in
    major) echo "$((major + 1)).0.0" ;;
    minor) echo "${major}.$((minor + 1)).0" ;;
    patch) echo "${major}.${minor}.$((patch + 1))" ;;
    *) err "bad bump kind: $kind" ;;
  esac
}

OLD_VER="$(current_version)"
[[ -n "$OLD_VER" ]] || err "could not read current version from Cargo.toml"

if [[ -n "$SET_VER" ]]; then
  NEW_VER="${SET_VER#v}"
else
  NEW_VER="$(bump_semver "$OLD_VER" "$BUMP")"
fi
[[ "$NEW_VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] \
  || err "invalid version: $NEW_VER"

TAG="v${NEW_VER}"
info "version ${OLD_VER} → ${NEW_VER} (tag ${TAG})"

# ── bump Cargo.toml package versions (line 1–10 only: package.version) ────
for f in "${VERSION_FILES[@]}"; do
  [[ -f "$f" ]] || err "missing $f"
  # Only replace the package version line near the top of the file.
  awk -v ver="$NEW_VER" '
    BEGIN { done=0 }
    !done && /^version = "/ {
      print "version = \"" ver "\""
      done=1
      next
    }
    { print }
  ' "$f" >"${f}.tmp" && mv "${f}.tmp" "$f"
done
ok "bumped package versions to ${NEW_VER}"

# ── platform ──────────────────────────────────────────────────────────────
os="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch="$(uname -m)"
case "$os" in darwin) os=macos ;; linux) os=linux ;; esac
case "$arch" in x86_64|amd64) arch=x86_64 ;; arm64|aarch64) arch=aarch64 ;; esac
platform="${os}-${arch}"
ASSET_NAME="grok-swarm-${NEW_VER}-${platform}"

# ── commit version bump (and any staged work the caller left) ─────────────
git add "${VERSION_FILES[@]}"
if ! git diff --cached --quiet; then
  git commit -m "release: grok-swarm v${NEW_VER}"
  ok "committed version bump"
else
  info "no version-file changes to commit (already at ${NEW_VER}?)"
fi

# ── build ─────────────────────────────────────────────────────────────────
BIN="target/release/grok-swarm"
if [[ "$SKIP_BUILD" -eq 1 ]]; then
  [[ -x "$BIN" ]] || err "--skip-build but ${BIN} missing"
  info "reusing existing ${BIN}"
else
  info "building release binary (this can take several minutes on a cold cache)…"
  cargo build -p xai-grok-pager-bin --release --bin grok-swarm
fi
[[ -x "$BIN" ]] || err "binary not found at ${BIN}"
ver_out="$("$BIN" --version 2>&1 | head -1)"
ok "binary: ${ver_out}"
# Soft check that version string appears
if ! printf '%s' "$ver_out" | grep -q "$NEW_VER"; then
  info "warning: binary --version does not contain ${NEW_VER}: ${ver_out}"
  info "(build may have used a stale env; continuing)"
fi

# ── stage asset ───────────────────────────────────────────────────────────
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/grok-swarm-ship.XXXXXX")"
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

ASSET_PATH="${STAGE}/${ASSET_NAME}"
cp -f "$BIN" "$ASSET_PATH"
chmod +x "$ASSET_PATH"
if [[ "$(uname -s)" == "Darwin" ]] && command -v codesign >/dev/null 2>&1; then
  codesign -s - --force --timestamp=none "$ASSET_PATH" 2>/dev/null || true
fi

# ── tag ───────────────────────────────────────────────────────────────────
if git rev-parse "$TAG" >/dev/null 2>&1; then
  err "tag ${TAG} already exists — bump again or delete the tag first"
fi
git tag -a "$TAG" -m "grok-swarm ${TAG}"
ok "tagged ${TAG}"

# ── push + release ────────────────────────────────────────────────────────
NOTES_BODY="${NOTES:-Automated release ${TAG}.

Install / update:
\`\`\`bash
grok-swarm update
# or
curl -fsSL https://raw.githubusercontent.com/${REPO}/main/install.sh | bash
\`\`\`

\`\`\`
$ver_out
\`\`\`
}"

if [[ "$NO_PUSH" -eq 1 ]]; then
  info "--no-push: tag is local only; create the release manually"
  info "asset staged at: ${ASSET_PATH}"
  trap - EXIT
  exit 0
fi

info "pushing ${BRANCH} + ${TAG} to ${REMOTE}…"
git push "$REMOTE" "HEAD:main" 2>/dev/null \
  || git push "$REMOTE" "${BRANCH}:main" 2>/dev/null \
  || git push "$REMOTE" HEAD
git push "$REMOTE" "$TAG"

info "creating GitHub release ${TAG}…"
gh release create "$TAG" \
  -R "$REPO" \
  --title "grok-swarm ${TAG}" \
  --notes "$NOTES_BODY" \
  "$ASSET_PATH"

ok "released ${TAG} with ${ASSET_NAME}"
info "latest: https://github.com/${REPO}/releases/tag/${TAG}"
info ""
info "On any machine with grok-swarm installed:"
info "  grok-swarm update"
info "  grok-swarm --version   # expect ${NEW_VER}"
