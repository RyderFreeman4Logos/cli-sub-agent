#!/usr/bin/env bash
set -euo pipefail
#
# PostRun hook: detect whether the current branch has an open PR that
# hasn't been reviewed by pr-codex-bot yet.  Safety net for cases where
# `gh pr create` was invoked outside a weave workflow step.
#
# Called automatically after every `csa run` session via the [hooks]
# post_run entry in `.csa/config.toml`.

# ── Recursion guard ──────────────────────────────────────────────────
# Set by both this script and post-pr-create.sh before launching
# pr-codex-bot.  Prevents re-entrant triggering from inner CSA sessions.
if [ -n "${CSA_PR_BOT_GUARD:-}" ]; then
    exit 0
fi

# ── Parse args ───────────────────────────────────────────────────────
BASE_BRANCH="main"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --base) shift; BASE_BRANCH="${1:-main}" ;;
        *) ;;
    esac
    shift
done

# ── Feature branch check ────────────────────────────────────────────
BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null) || exit 0
case "$BRANCH" in
    main|master|dev|develop|HEAD) exit 0 ;;
esac

# ── Prerequisite: gh CLI ─────────────────────────────────────────────
command -v gh >/dev/null 2>&1 || exit 0

# ── Check for open PR ───────────────────────────────────────────────
PR_NUMBER=$(
    gh pr view --json number,state,baseRefName \
        -q "select(.state == \"OPEN\" and .baseRefName == \"${BASE_BRANCH}\") | .number" \
        2>/dev/null
) || exit 0

if [ -z "$PR_NUMBER" ] || ! printf '%s' "$PR_NUMBER" | grep -qE '^[0-9]+$'; then
    exit 0
fi

# ── Marker: prevent double-trigger for same PR + HEAD ────────────────
HEAD_SHA=$(git rev-parse --short HEAD 2>/dev/null) || exit 0
MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers"
MARKER="${MARKER_DIR}/${PR_NUMBER}-${HEAD_SHA}"

if [ -f "$MARKER" ]; then
    exit 0
fi

mkdir -p "$MARKER_DIR"

echo "[post-run hook] PR #${PR_NUMBER} detected without pr-codex-bot run." >&2
echo "[post-run hook] Triggering pr-codex-bot in background..." >&2

# Resolve script location relative to project root (not cwd) so the hook
# works even when CSA is invoked from a different directory (csa run --cd).
REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
SCRIPT="${REPO_ROOT}/scripts/hooks/post-pr-create.sh"
if [ ! -x "$SCRIPT" ]; then
    echo "[post-run hook] WARNING: $SCRIPT not found or not executable" >&2
    exit 0
fi

export CSA_PR_BOT_GUARD=1
# Detach into own session so CSA's process-group cleanup won't kill the
# background workflow.  Use setsid where available (Linux); fall back to
# nohup-only on macOS where setsid is not shipped by default.  Redirect
# stdin from /dev/null to avoid blocking if the hook runner pipes stdio.
if command -v setsid >/dev/null 2>&1; then
    setsid nohup "$SCRIPT" --base "$BASE_BRANCH" \
        > "${MARKER_DIR}/${PR_NUMBER}-bot.log" 2>&1 < /dev/null &
else
    nohup "$SCRIPT" --base "$BASE_BRANCH" \
        > "${MARKER_DIR}/${PR_NUMBER}-bot.log" 2>&1 < /dev/null &
fi
BOT_PID=$!

# Only create deduplication marker if the background process was spawned.
# If launch failed, omitting the marker allows the next PostRun hook
# invocation to retry.
if kill -0 "$BOT_PID" 2>/dev/null; then
    touch "$MARKER"
else
    echo "[post-run hook] WARNING: Failed to launch pr-codex-bot (PID $BOT_PID)" >&2
fi

exit 0
