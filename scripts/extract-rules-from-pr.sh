#!/usr/bin/env bash
# extract-rules-from-pr.sh — Phase A rule-draft extractor from PR-bot findings
#
# Reads inline PR review comments via `gh api`, filters for HIGH/CRITICAL
# severity (gemini-code-assist badge format), and emits draft rule files.
# Read-only — never commits to any repo, never files PRs.
#
# Usage:
#   PR_NUMBER=1037 bash scripts/extract-rules-from-pr.sh
#   PR_NUMBER=1037 SESSION_DIR=/tmp/test-session bash scripts/extract-rules-from-pr.sh
#
# Environment:
#   PR_NUMBER   — required, integer PR number
#   SESSION_DIR — optional, defaults to $CSA_SESSION_DIR; output lands in
#                 $SESSION_DIR/output/proposed-rule-<pr>-<index>.md
#   REPO_SLUG   — optional, defaults to auto-detect from git remote 'origin'
#
# Stdout:  nothing (draft files written to disk)
# Stderr:  summary line "extracted=N findings=M" on success
# Exit:    0 on success (even if no qualifying findings), 1 on error
#
# Shell-injection hardening: PR comment bodies are NEVER embedded into
# shell commands. All processing uses jq for JSON and printf for output.

set -euo pipefail

###############################################################################
# Validate inputs
###############################################################################

if [ -z "${PR_NUMBER:-}" ]; then
  echo "ERROR: PR_NUMBER is required" >&2
  exit 1
fi

if ! [[ "${PR_NUMBER}" =~ ^[0-9]+$ ]]; then
  echo "ERROR: PR_NUMBER must be an integer, got '${PR_NUMBER}'" >&2
  exit 1
fi

# Resolve output directory
SESSION_DIR="${SESSION_DIR:-${CSA_SESSION_DIR:-}}"
if [ -z "${SESSION_DIR}" ]; then
  echo "ERROR: SESSION_DIR or CSA_SESSION_DIR must be set" >&2
  exit 1
fi

OUTPUT_DIR="${SESSION_DIR}/output"
mkdir -p "${OUTPUT_DIR}"

# Resolve repo slug
if [ -z "${REPO_SLUG:-}" ]; then
  REPO_SLUG="$(git remote get-url origin 2>/dev/null \
    | sed -E 's|.*github\.com[:/]||; s|\.git$||')"
fi

if [ -z "${REPO_SLUG}" ]; then
  echo "ERROR: Could not determine REPO_SLUG from git remote" >&2
  exit 1
fi

###############################################################################
# Dedupe paths
###############################################################################

T4NATURE_RULES="${HOME}/project/github/t4nature/s/llm/coding/rules"
PROJECT_RULES_REF=".agents/project-rules-ref"

###############################################################################
# Fetch PR inline review comments
###############################################################################

COMMENTS_JSON="$(gh api "repos/${REPO_SLUG}/pulls/${PR_NUMBER}/comments" \
  --paginate 2>/dev/null)" || {
  echo "ERROR: Failed to fetch PR comments for ${REPO_SLUG}#${PR_NUMBER}" >&2
  exit 1
}

###############################################################################
# Filter for HIGH/CRITICAL severity
#
# Badge format (gemini-code-assist):
#   ![high](https://www.gstatic.com/codereviewagent/high-priority.svg)
#   ![critical](https://www.gstatic.com/codereviewagent/critical-priority.svg)
#
# We match the markdown image alt text: ![high] or ![critical]
###############################################################################

# Extract qualifying comments as JSON array with metadata
QUALIFYING="$(echo "${COMMENTS_JSON}" | jq -c '
  [.[] | {
    body: .body,
    user: .user.login,
    html_url: .html_url,
    commit_id: .commit_id,
    path: .path,
    line: .line,
    severity: (
      if (.body | test("!\\[critical\\]")) then "critical"
      elif (.body | test("!\\[high\\]")) then "high"
      else null end
    )
  } | select(.severity != null)]
')"

TOTAL_FINDINGS="$(echo "${COMMENTS_JSON}" | jq 'length')"
QUALIFYING_COUNT="$(echo "${QUALIFYING}" | jq 'length')"

###############################################################################
# Emit draft rule files
###############################################################################

INDEX=0

while IFS= read -r finding; do
  INDEX=$((INDEX + 1))

  SEVERITY="$(echo "${finding}" | jq -r '.severity')"
  AUTHOR="$(echo "${finding}" | jq -r '.user')"
  COMMIT_SHA="$(echo "${finding}" | jq -r '.commit_id')"
  COMMENT_URL="$(echo "${finding}" | jq -r '.html_url')"
  FILE_PATH="$(echo "${finding}" | jq -r '.path // "unknown"')"
  LINE_NUM="$(echo "${finding}" | jq -r '.line // "unknown"')"

  # Extract body: strip the severity badge line, get first meaningful line
  # for dedupe matching
  BODY="$(echo "${finding}" | jq -r '.body')"
  # Remove badge image markdown from first line
  BODY_NO_BADGE="$(echo "${BODY}" | sed '1s/^!\['"${SEVERITY}"'\]([^)]*)[[:space:]]*//')"
  # First non-empty line after badge removal = dedupe key
  FIRST_LINE="$(echo "${BODY_NO_BADGE}" | sed '/^[[:space:]]*$/d' | head -1)"

  # --- Dedupe check ---
  DUPLICATE_CANDIDATE=""
  if [ -n "${FIRST_LINE}" ]; then
    # Escape regex special chars for grep -F (fixed string)
    DEDUPE_HIT=""
    if [ -d "${T4NATURE_RULES}" ]; then
      DEDUPE_HIT="$(grep -rlF "${FIRST_LINE}" "${T4NATURE_RULES}" 2>/dev/null | head -1)" || true
    fi
    if [ -z "${DEDUPE_HIT}" ] && [ -d "${PROJECT_RULES_REF}" ]; then
      DEDUPE_HIT="$(grep -rlF "${FIRST_LINE}" "${PROJECT_RULES_REF}" 2>/dev/null | head -1)" || true
    fi
    if [ -n "${DEDUPE_HIT}" ]; then
      DUPLICATE_CANDIDATE="${DEDUPE_HIT}"
    fi
  fi

  # --- Build frontmatter ---
  OUTFILE="${OUTPUT_DIR}/proposed-rule-${PR_NUMBER}-${INDEX}.md"
  {
    echo "---"
    echo "source: pr-bot-finding"
    echo "pr: ${PR_NUMBER}"
    echo "extracted-at: $(date -u +%Y-%m-%d)"
    echo "severity: ${SEVERITY}"
    echo "finding-author: ${AUTHOR}"
    echo "finding-commit: ${COMMIT_SHA}"
    printf 'raw-comment-url: %s\n' "${COMMENT_URL}"
    printf 'finding-file: %s\n' "${FILE_PATH}"
    printf 'finding-line: %s\n' "${LINE_NUM}"
    if [ -n "${DUPLICATE_CANDIDATE}" ]; then
      printf 'duplicate-candidate: %s\n' "${DUPLICATE_CANDIDATE}"
    fi
    echo "---"
    echo ""
    echo "## Anti-patterns"
    echo ""
    echo "<!-- What the reviewer flagged -->"
    echo ""
    # Include the original finding text as context for Layer 0
    echo "### Original finding"
    echo ""
    echo "${BODY_NO_BADGE}"
    echo ""
    echo "## Preferred primitives / patterns"
    echo ""
    echo "<!-- The fix approach — to be filled by Layer 0 -->"
    echo ""
    echo "## Decision rule"
    echo ""
    echo "<!-- One-sentence: if X, then Y -->"
    echo ""
    echo "## Case study"
    echo ""
    printf -- '- PR: [#%s](https://github.com/%s/pull/%s)\n' \
      "${PR_NUMBER}" "${REPO_SLUG}" "${PR_NUMBER}"
    printf -- '- Comment: [finding](%s)\n' "${COMMENT_URL}"
    printf -- '- File: `%s:%s`\n' "${FILE_PATH}" "${LINE_NUM}"
    echo ""
  } > "${OUTFILE}"

done < <(echo "${QUALIFYING}" | jq -c '.[]')

###############################################################################
# Summary
###############################################################################

echo "extracted=${INDEX} findings=${TOTAL_FINDINGS}" >&2
exit 0
