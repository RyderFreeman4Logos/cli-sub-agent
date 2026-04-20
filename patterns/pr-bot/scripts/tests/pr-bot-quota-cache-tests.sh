#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
SCRIPT_PATH="${ROOT_DIR}/patterns/pr-bot/scripts/pr-bot-quota-cache.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

run_inline_branch_cache_regression_test() {
  local case_dir="${TMP_ROOT}/inline-branch"
  local quota_cache="${case_dir}/cloud_bot_quota.toml"
  local exhausted_at="2026-04-20T08:00:00Z"
  local expected_reset_at="2026-04-21T08:00:00Z"

  mkdir -p "${case_dir}"
  cat >"${quota_cache}" <<'EOF'
[cloud_bot_quota.other-bot]
exhausted_at = "2026-04-18T08:00:00Z"
expected_reset_at = "2026-04-19T08:00:00Z"
last_pr_seen = 7
last_quota_message = "keep me"
EOF

  (
    export QUOTA_CACHE_FILE="${quota_cache}"
    export BOT_NAME="test-bot"
    export PR_NUMBER="42"
    # REGRESSION: covers #898 inline-branch cache
    # shellcheck source=patterns/pr-bot/scripts/pr-bot-quota-cache.sh
    . "${SCRIPT_PATH}"
    write_quota_cache "${exhausted_at}" "${expected_reset_at}" "cloud_bot_quota_exhausted" "Daily quota limit reached for today."
    [ "${CLOUD_BOT_QUOTA_EXHAUSTED_AT}" = "${exhausted_at}" ]
    [ "${CLOUD_BOT_QUOTA_EXPECTED_RESET_AT}" = "${expected_reset_at}" ]
    [ "${MERGE_WITHOUT_BOT_REASON_KIND}" = "cloud_bot_quota_exhausted" ]
  )

  grep -q '\[cloud_bot_quota.other-bot\]' "${quota_cache}"
  grep -q '\[cloud_bot_quota.test-bot\]' "${quota_cache}"
  grep -q 'exhausted_at = "2026-04-20T08:00:00Z"' "${quota_cache}"
  grep -q 'expected_reset_at = "2026-04-21T08:00:00Z"' "${quota_cache}"
  grep -q 'last_pr_seen = 42' "${quota_cache}"
  grep -q 'last_quota_message = "Daily quota limit reached for today."' "${quota_cache}"
}

run_inline_branch_cache_regression_test

echo "pr-bot-quota-cache tests: PASS"
