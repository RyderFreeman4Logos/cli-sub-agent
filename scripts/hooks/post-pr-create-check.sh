#!/usr/bin/env bash
set -euo pipefail
#
# PostRun hook: detect whether the current branch has an open PR that
# hasn't been reviewed by pr-bot yet.  Safety net for cases where
# `gh pr create` was invoked outside a weave workflow step.
#
# Called automatically after every `csa run` session via the [hooks]
# post_run entry in `.csa/config.toml`.

usage() {
    cat <<'EOF'
Usage: scripts/hooks/post-pr-create-check.sh [--base <branch>] [--session-dir <path>]

PostRun safety-net: detect a freshly created GitHub PR from the just-finished
session output and launch the deterministic post-create transaction. Falls back
to the current branch's open PR when the session output contains no PR URL.
EOF
}

# ── Recursion guard ──────────────────────────────────────────────────
# Set by both this script and post-pr-create.sh before launching
# pr-bot.  Prevents re-entrant triggering from inner CSA sessions.
if [ -n "${CSA_PR_BOT_GUARD:-}" ]; then
    exit 0
fi

# ── Parse args ───────────────────────────────────────────────────────
BASE_BRANCH="main"
SESSION_DIR=""
while [ "$#" -gt 0 ]; do
    case "$1" in
        --base)
            shift
            BASE_BRANCH="${1:-main}"
            ;;
        --session-dir)
            shift
            SESSION_DIR="${1:-}"
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "ERROR: Unknown argument: $1" >&2
            usage >&2
            exit 1
            ;;
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

extract_pr_number_from_session_output() {
    local session_dir="$1"
    local output_log pr_url pr_number

    [ -n "${session_dir}" ] || return 1
    output_log="${session_dir}/output.log"
    [ -f "${output_log}" ] || return 1

    pr_url="$(
        tr -d '\r' < "${output_log}" \
            | grep -Eo 'https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/pull/[0-9]+' \
            | tail -n 1 || true
    )"

    if [ -z "${pr_url}" ]; then
        pr_url="$(
            tr -d '\r' < "${output_log}" \
                | grep -Eo 'github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/pull/[0-9]+' \
                | tail -n 1 || true
        )"
    fi

    pr_number="$(printf '%s\n' "${pr_url}" | sed -nE 's#.*?/pull/([0-9]+)$#\1#p' | tail -n 1)"
    [ -n "${pr_number}" ] || return 1
    printf '%s\n' "${pr_number}"
}

resolve_pr_from_session_output() {
    local pr_number="$1"
    local pr_json

    [ -n "${pr_number}" ] || return 1
    printf '%s' "${pr_number}" | grep -Eq '^[0-9]+$' || return 1

    pr_json="$(
        gh pr view "${pr_number}" --json number,state,baseRefName,headRefName,url 2>/dev/null || true
    )"
    [ -n "${pr_json}" ] || return 1

    if [ "$(printf '%s' "${pr_json}" | jq -r '.state')" != "OPEN" ]; then
        return 1
    fi

    if [ "$(printf '%s' "${pr_json}" | jq -r '.baseRefName')" != "${BASE_BRANCH}" ]; then
        return 1
    fi

    if [ "$(printf '%s' "${pr_json}" | jq -r '.headRefName')" != "${BRANCH}" ]; then
        return 1
    fi

    printf '%s\n' "${pr_json}"
}

resolve_current_pr() {
    gh pr view --json number,state,baseRefName,headRefName,url \
        -q "select(.state == \"OPEN\" and .baseRefName == \"${BASE_BRANCH}\" and .headRefName == \"${BRANCH}\")" \
        2>/dev/null || true
}

SESSION_PR_NUMBER="$(extract_pr_number_from_session_output "${SESSION_DIR}" || true)"
PR_JSON=""

if [ -n "${SESSION_PR_NUMBER}" ]; then
    PR_JSON="$(resolve_pr_from_session_output "${SESSION_PR_NUMBER}")"
fi

if [ -z "${PR_JSON}" ]; then
    PR_JSON="$(resolve_current_pr)"
fi

PR_NUMBER="$(printf '%s' "${PR_JSON}" | jq -r '.number // empty' 2>/dev/null || true)"

if [ -z "$PR_NUMBER" ] || ! printf '%s' "$PR_NUMBER" | grep -qE '^[0-9]+$'; then
    exit 0
fi

echo "[post-run hook] PR #${PR_NUMBER} detected without pr-bot run." >&2
echo "[post-run hook] Triggering pr-bot in background..." >&2

# Resolve script location relative to project root (not cwd) so the hook
# works even when CSA is invoked from a different directory (csa run --cd).
REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
SCRIPT="${REPO_ROOT}/scripts/hooks/post-pr-create.sh"
if [ ! -x "$SCRIPT" ]; then
    echo "[post-run hook] WARNING: $SCRIPT not found or not executable" >&2
    exit 0
fi

export CSA_PR_BOT_GUARD=1
REPO_SLUG="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')"
if [ -z "${REPO_SLUG}" ]; then
  REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
fi
MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"
mkdir -p "${MARKER_DIR}"
# Detach into own session so CSA's process-group cleanup won't kill the
# background workflow.  Use setsid where available (Linux); fall back to
# nohup-only on macOS where setsid is not shipped by default.  Redirect
# stdin from /dev/null to avoid blocking if the hook runner pipes stdio.
if command -v setsid >/dev/null 2>&1; then
    setsid nohup "$SCRIPT" --base "$BASE_BRANCH" --pr-number "$PR_NUMBER" \
        > "${MARKER_DIR}/${PR_NUMBER}-bot.log" 2>&1 < /dev/null &
else
    nohup "$SCRIPT" --base "$BASE_BRANCH" --pr-number "$PR_NUMBER" \
        > "${MARKER_DIR}/${PR_NUMBER}-bot.log" 2>&1 < /dev/null &
fi
BOT_PID=$!

if ! kill -0 "$BOT_PID" 2>/dev/null; then
    echo "[post-run hook] WARNING: Failed to launch pr-bot (PID $BOT_PID)" >&2
fi

exit 0
