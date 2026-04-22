#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
COMMIT_MSG_SCRIPT="${ROOT_DIR}/scripts/gen_commit_msg.sh"

if [ -n "${CSA_WORKFLOW_DIR:-}" ]; then
  CSA_HELPER_DIR="${CSA_WORKFLOW_DIR}/scripts/csa"
else
  CSA_HELPER_DIR="${ROOT_DIR}/patterns/pr-bot/scripts/csa"
fi

emit_var() {
  printf 'CSA_VAR:%s=%s\n' "$1" "$2"
}

emit_skip() {
  local reason="$1"

  echo "Skipping Step 10.5: ${reason}"
  emit_var "REBASE_CLEAN_HISTORY_APPLIED" "false"
  exit 0
}

normalize_verdict() {
  local raw="${1:-}"

  case "$(printf '%s' "${raw}" | tr '[:lower:]' '[:upper:]')" in
    PASS|CLEAN)
      printf 'Pass\n'
      ;;
    FAIL|HAS_ISSUES)
      printf 'Fail\n'
      ;;
    SKIP)
      printf 'Skip\n'
      ;;
    UNCERTAIN)
      printf 'Uncertain\n'
      ;;
    *)
      printf '\n'
      ;;
  esac
}

extract_commit_verdict() {
  local message="$1"
  local raw=""

  raw="$(
    printf '%s\n' "${message}" \
      | grep -oiE 'verdict=(pass|fail|skip|uncertain|clean|has_issues)' \
      | head -n 1 \
      | cut -d= -f2 \
      || true
  )"
  if [ -z "${raw}" ]; then
    raw="$(
      printf '%s\n' "${message}" \
        | grep -oiE 'summary=(pass|fail|skip|uncertain|clean|has_issues)' \
        | head -n 1 \
        | cut -d= -f2 \
        || true
    )"
  fi

  normalize_verdict "${raw}"
}

extract_commit_tool() {
  local message="$1"
  local tool=""

  tool="$(
    printf '%s\n' "${message}" \
      | grep -oiE 'tool=[A-Za-z0-9._-]+' \
      | head -n 1 \
      | cut -d= -f2 \
      || true
  )"
  if [ -z "${tool}" ]; then
    tool="$(
      printf '%s\n' "${message}" \
        | sed -nE 's/^Review: ([^ ]+) session.*/\1/p' \
        | head -n 1 \
        || true
    )"
  fi
  if [ -z "${tool}" ]; then
    tool="unknown"
  fi

  printf '%s\n' "${tool}"
}

extract_commit_round() {
  local subject="$1"
  local message="$2"

  printf '%s\n%s\n' "${subject}" "${message}" \
    | grep -oiE 'round[[:space:]]*[:=#-]?[[:space:]]*[0-9]+' \
    | head -n 1 \
    | grep -oE '[0-9]+' \
    || true
}

file_belongs_to_group() {
  local group="$1"
  local file="$2"

  case "${group}" in
    source)
      printf '%s\n' "${file}" | grep -Eq '^(src/|crates/|lib/|bin/)'
      ;;
    patterns)
      printf '%s\n' "${file}" | grep -Eq '^(patterns/|\.claude/)'
      ;;
    other)
      ! printf '%s\n' "${file}" | grep -Eq '^(src/|crates/|lib/|bin/|patterns/|\.claude/)'
      ;;
    *)
      echo "ERROR: unknown rebase group '${group}'." >&2
      exit 1
      ;;
  esac
}

commit_touches_group() {
  local sha="$1"
  local group="$2"
  local file=""

  while IFS= read -r -d '' file; do
    if file_belongs_to_group "${group}" "${file}"; then
      return 0
    fi
  done < <(git diff-tree --no-commit-id --name-only -r -z "${sha}")

  return 1
}

stage_group_files() {
  local group="$1"
  local file=""

  mapfile -d '' -t changed_files < <(git diff --name-only -z HEAD)
  for file in "${changed_files[@]}"; do
    if file_belongs_to_group "${group}" "${file}"; then
      git add -- "${file}"
    fi
  done
}

build_rollup_block() {
  local group="$1"
  local sha=""
  local subject=""
  local message=""
  local verdict=""
  local tool=""
  local round=""
  local short_sha=""
  local entry=""

  printf '## AI Reviewer Metadata Rollup\n\n'
  printf 'This commit consolidates review history from:\n'
  for sha in "${ORIGINAL_COMMITS[@]}"; do
    if ! commit_touches_group "${sha}" "${group}"; then
      continue
    fi

    subject="$(git show -s --format=%s "${sha}")"
    message="$(git show -s --format=%B "${sha}")"
    verdict="$(extract_commit_verdict "${message}")"
    tool="$(extract_commit_tool "${message}")"
    round="$(extract_commit_round "${subject}" "${message}")"
    short_sha="$(git rev-parse --short=7 "${sha}")"

    entry="- ${short_sha} verdict=${verdict} tool=${tool}"
    if [ -n "${round}" ]; then
      entry="${entry} round=${round}"
    fi
    entry="${entry} (${subject})"
    printf '%s\n' "${entry}"
  done
}

create_group_commit() {
  local group="$1"
  local commit_subject=""
  local commit_body=""
  local rollup_block=""

  if git diff --cached --quiet; then
    return 0
  fi

  commit_subject="$("${COMMIT_MSG_SCRIPT}" --subject)"
  commit_body="$("${COMMIT_MSG_SCRIPT}" --body)"
  rollup_block="$(build_rollup_block "${group}")"
  commit_body="${commit_body}

${rollup_block}"

  git commit -m "${commit_subject}" -m "${commit_body}"
}

validate_all_commits_pass() {
  local sha=""
  local subject=""
  local message=""
  local verdict=""
  local short_sha=""

  for sha in "${ORIGINAL_COMMITS[@]}"; do
    subject="$(git show -s --format=%s "${sha}")"
    message="$(git show -s --format=%B "${sha}")"
    verdict="$(extract_commit_verdict "${message}")"
    short_sha="$(git rev-parse --short=7 "${sha}")"

    if [ "${verdict}" = "Fail" ]; then
      emit_skip "commit ${short_sha} (${subject}) has verdict=Fail; preserve the full fix chain for audit."
    fi
    if [ "${verdict}" != "Pass" ]; then
      emit_skip "commit ${short_sha} (${subject}) does not carry explicit verdict=Pass metadata."
    fi
  done
}

REBASE_PUSHED=false
BACKUP_BRANCH="backup-${PR_NUM}-pre-rebase"
ORIGINAL_HEAD="$(git rev-parse HEAD)"
MERGE_BASE="$(git merge-base "${DEFAULT_BRANCH}" "${ORIGINAL_HEAD}")"
mapfile -t ORIGINAL_COMMITS < <(git rev-list --reverse "${MERGE_BASE}..${ORIGINAL_HEAD}")
COMMIT_COUNT="${#ORIGINAL_COMMITS[@]}"

restore_pre_push_state() {
  local exit_code="$1"

  if [ "${exit_code}" -ne 0 ] && [ "${REBASE_PUSHED}" != "true" ] && git show-ref --verify --quiet "refs/heads/${BACKUP_BRANCH}"; then
    git reset --hard "${BACKUP_BRANCH}" >/dev/null 2>&1 || true
  fi
}

trap 'restore_pre_push_state "$?"' EXIT

if [ "${COMMIT_COUNT}" -lt 3 ]; then
  emit_skip "branch has ${COMMIT_COUNT} commit(s); Step 10.5 only fires at 3+ commits."
fi

validate_all_commits_pass

git branch -f "${BACKUP_BRANCH}" "${ORIGINAL_HEAD}"
git reset --soft "${MERGE_BASE}"
git reset HEAD .

stage_group_files "source"
create_group_commit "source"

stage_group_files "patterns"
create_group_commit "patterns"

git add -A
create_group_commit "other"

NEW_COMMIT_COUNT="$(git rev-list --count "${MERGE_BASE}..HEAD")"
if [ "${NEW_COMMIT_COUNT}" -eq 0 ]; then
  echo "ERROR: No replacement commits created after soft reset. Restoring backup." >&2
  git reset --hard "${BACKUP_BRANCH}"
  exit 1
fi

echo "Step 10.5 guard passed: ${COMMIT_COUNT} commits with explicit verdict=Pass metadata. Rebase rollup is active."
git push --force-with-lease "${REMOTE_NAME}" "${WORKFLOW_BRANCH}"
REBASE_PUSHED=true

REBASE_CURRENT_SHA="$(git rev-parse HEAD)"
REBASE_TRIGGER_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
REBASE_TRIGGER_BODY="${CLOUD_BOT_RETRIGGER_CMD}

<!-- csa-retrigger:post-rebase:${REBASE_CURRENT_SHA}:${REBASE_TRIGGER_TS} -->"
gh pr comment "${PR_NUM}" --repo "${REPO}" --body "${REBASE_TRIGGER_BODY}"
echo "Triggered post-rebase review via '${CLOUD_BOT_RETRIGGER_CMD}' for HEAD ${REBASE_CURRENT_SHA}."

GATE_PROMPT=$(cat <<EOF
Bounded post-rebase gate task only. Do NOT invoke pr-bot skill or any full PR workflow. Operate on PR #${PR_NUM} in repo ${REPO} (branch ${WORKFLOW_BRANCH}). Complete the post-rebase review gate end-to-end. For each cloud bot trigger, wait ${CLOUD_BOT_WAIT_SECONDS} seconds quietly, then poll up to ${CLOUD_BOT_POLL_MAX_SECONDS} seconds for a response. If response contains P0/P1/P2 findings, iteratively fix/commit/push/re-trigger and re-check with the same wait policy (max 3 rounds). If bot times out, abort and report to user; return exactly one marker line REBASE_GATE=PASS when clean, otherwise REBASE_GATE=FAIL and exit non-zero.
EOF
)

set +e
GATE_SID="$(csa run --sa-mode true --tier tier-1 --timeout "${POST_REBASE_TIMEOUT}" --idle-timeout "${POST_REBASE_TIMEOUT}" "${GATE_PROMPT}")"
DAEMON_RC=$?
set -e
if [ "${DAEMON_RC}" -ne 0 ] || [ -z "${GATE_SID}" ]; then
  REBASE_REVIEW_HAS_ISSUES=true
  FALLBACK_REVIEW_HAS_ISSUES=true
  emit_var "REBASE_REVIEW_HAS_ISSUES" "${REBASE_REVIEW_HAS_ISSUES}"
  emit_var "FALLBACK_REVIEW_HAS_ISSUES" "${FALLBACK_REVIEW_HAS_ISSUES}"
  echo "ERROR: Failed to launch daemon for post-rebase gate (rc=${DAEMON_RC})." >&2
  exit 1
fi

set +e
GATE_RESULT="$(bash "${CSA_HELPER_DIR}/session-wait-until-done.sh" "${GATE_SID}")"
GATE_RC=$?
set -e
if [ "${GATE_RC}" -ne 0 ]; then
  REBASE_REVIEW_HAS_ISSUES=true
  FALLBACK_REVIEW_HAS_ISSUES=true
  emit_var "REBASE_REVIEW_HAS_ISSUES" "${REBASE_REVIEW_HAS_ISSUES}"
  emit_var "FALLBACK_REVIEW_HAS_ISSUES" "${FALLBACK_REVIEW_HAS_ISSUES}"
  echo "ERROR: Post-rebase delegated gate failed (rc=${GATE_RC})." >&2
  exit 1
fi

GATE_MARKER="$(
  printf '%s\n' "${GATE_RESULT}" \
    | grep -E '^[[:space:]]*REBASE_GATE=(PASS|FAIL)[[:space:]]*$' \
    | tail -n 1 \
    | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//' \
    || true
)"
if [ "${GATE_MARKER}" != "REBASE_GATE=PASS" ]; then
  REBASE_REVIEW_HAS_ISSUES=true
  FALLBACK_REVIEW_HAS_ISSUES=true
  emit_var "REBASE_REVIEW_HAS_ISSUES" "${REBASE_REVIEW_HAS_ISSUES}"
  emit_var "FALLBACK_REVIEW_HAS_ISSUES" "${FALLBACK_REVIEW_HAS_ISSUES}"
  echo "ERROR: Post-rebase review gate failed." >&2
  exit 1
fi

BOT_SETTLE_SECS="${BOT_SETTLE_SECS:-20}"
sleep "${BOT_SETTLE_SECS}"
set +e
LATE_ACTIONABLE_COUNT="$(
  gh api --paginate --slurp "repos/${REPO}/pulls/${PR_NUM}/comments" \
    | jq -r '[.[] | .[] | select(.user.login == "'"${CLOUD_BOT_LOGIN}"'") | select(.created_at > "'"${REBASE_TRIGGER_TS}"'") | select((.body | test("P0|P1|P2"))) ] | length' \
    2>/dev/null
)"
LATE_ACTIONABLE_RC=$?
set -e
if [ "${LATE_ACTIONABLE_RC}" -ne 0 ]; then
  REBASE_REVIEW_HAS_ISSUES=true
  FALLBACK_REVIEW_HAS_ISSUES=true
  emit_var "REBASE_REVIEW_HAS_ISSUES" "${REBASE_REVIEW_HAS_ISSUES}"
  emit_var "FALLBACK_REVIEW_HAS_ISSUES" "${FALLBACK_REVIEW_HAS_ISSUES}"
  echo "ERROR: Failed to query post-rebase actionable bot comments (rc=${LATE_ACTIONABLE_RC})." >&2
  exit 1
fi

case "${LATE_ACTIONABLE_COUNT:-}" in
  ''|*[!0-9]*)
    REBASE_REVIEW_HAS_ISSUES=true
    FALLBACK_REVIEW_HAS_ISSUES=true
    emit_var "REBASE_REVIEW_HAS_ISSUES" "${REBASE_REVIEW_HAS_ISSUES}"
    emit_var "FALLBACK_REVIEW_HAS_ISSUES" "${FALLBACK_REVIEW_HAS_ISSUES}"
    echo "ERROR: Invalid post-rebase actionable comment count from GitHub API: '${LATE_ACTIONABLE_COUNT}'." >&2
    exit 1
    ;;
esac

if [ "${LATE_ACTIONABLE_COUNT}" -gt 0 ]; then
  REBASE_REVIEW_HAS_ISSUES=true
  FALLBACK_REVIEW_HAS_ISSUES=true
  emit_var "REBASE_REVIEW_HAS_ISSUES" "${REBASE_REVIEW_HAS_ISSUES}"
  emit_var "FALLBACK_REVIEW_HAS_ISSUES" "${FALLBACK_REVIEW_HAS_ISSUES}"
  echo "ERROR: Detected ${LATE_ACTIONABLE_COUNT} actionable bot comment(s) after post-rebase trigger window." >&2
  exit 1
fi

REBASE_REVIEW_HAS_ISSUES=false
FALLBACK_REVIEW_HAS_ISSUES=false
emit_var "REBASE_CLEAN_HISTORY_APPLIED" "true"
emit_var "REBASE_REVIEW_HAS_ISSUES" "${REBASE_REVIEW_HAS_ISSUES}"
emit_var "FALLBACK_REVIEW_HAS_ISSUES" "${FALLBACK_REVIEW_HAS_ISSUES}"
