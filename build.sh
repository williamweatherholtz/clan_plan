#!/usr/bin/env bash
# Build and (optionally) push the clanplan docker image.
#
#   ./build.sh                     # cheap local checks + docker build, tag = git short sha (+ -dirty if WIP)
#   ./build.sh --push              # also docker push
#   ./build.sh --tag v1.2.3        # explicit tag override
#   ./build.sh --strict            # additionally enforce cargo fmt + clippy -D warnings
#   ./build.sh --test              # additionally run `cargo test`
#
# Env:
#   IMAGE        full image repo, default docker.io/YOURUSER/clanplan
set -euo pipefail

cd "$(dirname "$0")"

# ── Logging ───────────────────────────────────────────────────────────────────
# Tee every byte of stdout+stderr to build.log so a window that closes too fast
# still leaves a record on disk. Rotate to .prev so you always have the last
# two runs available.
LOG="build.log"
[[ -f "$LOG" ]] && mv "$LOG" "${LOG}.prev"
exec > >(tee "$LOG") 2>&1
echo "build.sh started $(date '+%Y-%m-%d %H:%M:%S')"
echo "args: $*"
trap 'rc=$?; echo; echo "build.sh exited with code $rc — full log at $LOG"; exit $rc' EXIT

IMAGE="${IMAGE:-docker.io/williamweatherholtz/clanplan}"
TAG=""
PUSH=0
STRICT=0
RUN_TESTS=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --push)       PUSH=1; shift ;;
        --tag)        TAG="$2"; shift 2 ;;
        --strict)     STRICT=1; shift ;;
        --test)       RUN_TESTS=1; shift ;;
        -h|--help)    sed -n '2,12p' "$0"; exit 0 ;;
        *)            echo "unknown arg: $1" >&2; exit 1 ;;
    esac
done

log() { printf '\n\033[1;34m▶\033[0m %s\n' "$*"; }

# ── Locate cargo when running under Git Bash / MSYS ───────────────────────────
# Windows installs cargo to %USERPROFILE%\.cargo\bin, which isn't on MSYS's
# default PATH when bash is launched from PowerShell. Ask Windows itself.
add_to_path() {
    local win_path="$1"
    local unix_path
    if command -v cygpath >/dev/null 2>&1; then
        unix_path="$(cygpath -u "$win_path")"
    else
        # C:\foo\bar → /c/foo/bar
        local drive="${win_path:0:1}"
        local rest="${win_path:2}"
        unix_path="/${drive,,}${rest//\\//}"
    fi
    export PATH="$unix_path:$PATH"
    echo "build.sh: added $unix_path to PATH"
}

if ! command -v cargo >/dev/null 2>&1; then
    cargo_win=""

    # 1. PowerShell (most reliable — no quote-escaping pitfalls)
    if [[ -z "$cargo_win" ]] && command -v powershell.exe >/dev/null 2>&1; then
        cargo_win="$(powershell.exe -NoProfile -Command \
            "(Get-Command cargo -ErrorAction SilentlyContinue).Source" \
            2>/dev/null | tr -d '\r' | head -1 || true)"
    fi

    # 2. cmd.exe `where` (note: needs //c on MSYS to avoid path-conversion)
    if [[ -z "$cargo_win" ]] && command -v cmd.exe >/dev/null 2>&1; then
        cargo_win="$(cmd.exe //c "where cargo.exe" 2>/dev/null | tr -d '\r' | head -1 || true)"
    fi

    # Only honor it if it actually looks like a cargo.exe path
    if [[ "$cargo_win" =~ cargo(\.exe)?$ ]]; then
        add_to_path "$(dirname "$cargo_win")"
    fi

    # 3. Last resort: scan well-known install locations
    if ! command -v cargo >/dev/null 2>&1; then
        for candidate in \
            "$HOME/.cargo/bin" \
            "/c/Users/${USER:-$USERNAME}/.cargo/bin"; do
            if [[ -x "$candidate/cargo" || -x "$candidate/cargo.exe" ]]; then
                export PATH="$candidate:$PATH"
                echo "build.sh: added $candidate to PATH"
                break
            fi
        done
    fi
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "build.sh: cargo not found." >&2
    echo "  In PowerShell, run: where.exe cargo" >&2
    echo "  If that prints nothing, install rustup from https://rustup.rs" >&2
    exit 1
fi

# ── Tag derivation ────────────────────────────────────────────────────────────
# Default: short git sha, with -dirty suffix if working tree has changes.
# This lets you ship WIP test builds without bypassing the sanity gates below.
if [[ -z "$TAG" ]]; then
    TAG="$(git rev-parse --short HEAD)"
    if ! git diff --quiet || ! git diff --cached --quiet; then
        TAG="${TAG}-dirty"
        echo "build.sh: working tree dirty → tag = $TAG"
    fi
fi

# ── 1. Migration sequence — the real bug we're guarding against ───────────────
log "checking migrations"
bash scripts/check-migrations.sh

# ── 2. Uncommitted template/asset warning ─────────────────────────────────────
# Edits to templates/ or assets/ that aren't committed are an extremely common
# "I built but the deployment looks wrong" footgun. Surface them loudly but
# don't refuse — sometimes you genuinely want to ship a WIP test image.
dirty_ui="$(git status --short -- templates/ assets/ 2>/dev/null || true)"
if [[ -n "$dirty_ui" ]]; then
    echo
    echo "⚠ build.sh: uncommitted changes in templates/ or assets/ — they WILL be in the image:"
    echo "$dirty_ui" | sed 's/^/    /'
    echo "  (commit them if you want the git sha tag to actually identify what shipped)"
fi

# ── 3. Rebuild tailwind.css from input.css + tailwind.config.js ───────────────
# Tailwind is JIT — assets/tailwind.css only contains utilities that existed in
# templates at the time it was last compiled. If you added new utility classes
# and forgot to recompile, those classes silently produce no styling in prod.
# We auto-rebuild every time so the committed tailwind.css matches templates.
if command -v npx >/dev/null 2>&1; then
    log "rebuilding tailwind.css"
    npx --yes tailwindcss -i assets/input.css -o assets/tailwind.css --minify
    if ! git diff --quiet assets/tailwind.css; then
        echo "⚠ assets/tailwind.css changed during rebuild — commit it before pushing the image"
        echo "  (otherwise the next build will rebuild it again and you'll keep seeing this)"
    fi
else
    echo "⚠ npx not found — skipping tailwind rebuild."
    echo "  Install Node.js if your templates use new utility classes that need compiling."
fi

# ── 4. Fast local typecheck ───────────────────────────────────────────────────
log "cargo check"
cargo check --locked

# ── 5. Local release compile — same profile docker uses, catches release-only issues
log "cargo build --release"
cargo build --release --locked

# ── 6. Opt-in strict checks ───────────────────────────────────────────────────
if (( STRICT )); then
    log "cargo fmt --check (--strict)"
    cargo fmt --all -- --check
    log "cargo clippy (--strict)"
    cargo clippy --all-targets --locked -- -D warnings
fi

# ── 7. Opt-in tests ───────────────────────────────────────────────────────────
if (( RUN_TESTS )); then
    log "cargo test (--test)"
    cargo test --locked
fi

# ── 8. Docker build ───────────────────────────────────────────────────────────
log "docker build → $IMAGE:$TAG"
docker build \
    --tag "$IMAGE:$TAG" \
    --tag "$IMAGE:latest" \
    --label "org.opencontainers.image.revision=$(git rev-parse HEAD)" \
    --label "org.opencontainers.image.created=$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    .

# ── 9. Push (opt-in) ──────────────────────────────────────────────────────────
if (( PUSH )); then
    log "docker push $IMAGE:$TAG"
    docker push "$IMAGE:$TAG"
    log "docker push $IMAGE:latest"
    docker push "$IMAGE:latest"
else
    echo
    echo "Built $IMAGE:$TAG and $IMAGE:latest locally. Re-run with --push to publish."
fi

log "done"
