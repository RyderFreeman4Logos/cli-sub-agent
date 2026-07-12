set -euo pipefail
CURRENT_BRANCH="$(git branch --show-current)"
FEATURE_INPUT="${FEATURE_INPUT:-${SCOPE:-current branch changes pending merge}}"
USER_LANGUAGE_OVERRIDE="${CSA_USER_LANGUAGE:-}"
MKTD_TOOL_EFFECTIVE="${MKTD_TOOL:-${CSA_MKTD_TOOL:-}}"
MKTD_TIMEOUT_SECONDS="${MKTD_TIMEOUT_SECONDS:-1800}"
MKTD_PLAN_TIER="${PLAN_TIER:-${TIER:-tier-3-complex}}"
CSA_BIN="${CSA_BIN:-csa}"
MKTD=(--pattern mktd); [ -n "${MKTD_WORKFLOW_PATH:-}" ] && MKTD=("$MKTD_WORKFLOW_PATH")
MKTD_TOOL_ARGS=(); [ -z "${MKTD_TOOL_EFFECTIVE}" ] || MKTD_TOOL_ARGS=(--tool "${MKTD_TOOL_EFFECTIVE}")
LIGHT_THRESHOLD_FILES="${PLANNING_LIGHT_THRESHOLD_FILES:-2}"
LIGHT_THRESHOLD_LINES="${PLANNING_LIGHT_THRESHOLD_LINES:-50}"
PLAN_CODE_FILES="$(git diff --name-only "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | grep -cvE '\.(md|txt|lock|toml)$' || true)"
PLAN_INSERTIONS="$(git diff --stat "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | tail -1 | grep -oE '[0-9]+ insertion' | grep -oE '[0-9]+' || echo 0)"
FEATURE_INPUT_LEN=${#FEATURE_INPUT}
FEATURE_FILE_LINE_HITS="$(printf '%s' "${FEATURE_INPUT}" | grep -oE '[A-Za-z0-9_./-]+\.(rs|toml|md):[0-9]+' | wc -l | xargs || true)"
if [ "${FEATURE_INPUT_LEN}" -lt 4096 ] && [ "${FEATURE_FILE_LINE_HITS}" -ge 2 ]; then
  MKTD_INTENSITY="light"
  echo "Planning intensity: light (brief specificity: ${FEATURE_FILE_LINE_HITS} file:line refs in ${FEATURE_INPUT_LEN}-char brief)"
elif [ "${PLAN_CODE_FILES}" -le "${LIGHT_THRESHOLD_FILES}" ] && [ "${PLAN_INSERTIONS:-0}" -lt "${LIGHT_THRESHOLD_LINES}" ]; then
  MKTD_INTENSITY="light"
  echo "Planning intensity: light (${PLAN_CODE_FILES} code files, ${PLAN_INSERTIONS} insertions)"
else
  MKTD_INTENSITY="full"
  echo "Planning intensity: full (${PLAN_CODE_FILES} code files, ${PLAN_INSERTIONS} insertions)"
fi
echo "mktd timeout: ${MKTD_TIMEOUT_SECONDS}s"
set +e
MKTD_OUTPUT="$(timeout -k 30 "${MKTD_TIMEOUT_SECONDS}" "${CSA_BIN}" plan run --sa-mode true "${MKTD[@]}" \
  "${MKTD_TOOL_ARGS[@]}" \
  --var CWD="$(pwd)" \
  --var FEATURE="Plan dev2merge for branch ${CURRENT_BRANCH}. Scope: ${FEATURE_INPUT}." \
  --var USER_LANGUAGE="${USER_LANGUAGE_OVERRIDE}" \
  --var TIER="${TIER:-}" \
  --var PLAN_TIER="${MKTD_PLAN_TIER}" \
  --var IMPL_TIER="${IMPL_TIER:-}" \
  --var IMPL_TOOL="${IMPL_TOOL:-}" \
  --var INTENSITY="${MKTD_INTENSITY}" 2>&1)"
MKTD_EXIT=$?
set -e
printf '%s\n' "${MKTD_OUTPUT}"
print_mktd_failure_context() {
  echo "mktd exit code: ${MKTD_EXIT}" >&2
  echo "mktd failure context (step/exit lines):" >&2
  MKTD_FAILURE_CONTEXT="$(printf '%s\n' "${MKTD_OUTPUT}" | grep -Ei '(error|Step [0-9]+|timeout|fail)' | tail -40 || true)"
  if [ -n "${MKTD_FAILURE_CONTEXT}" ]; then
    printf '%s\n' "${MKTD_FAILURE_CONTEXT}" >&2
  else
    printf '%s\n' "${MKTD_OUTPUT}" | tail -80 >&2
  fi
}
fail_step7_gate() {
  echo "ERROR: ${1}" >&2
  if [ "${MKTD_EXIT}" -ne 0 ]; then
    print_mktd_failure_context
  else
    echo "mktd exit code: 0" >&2
  fi
  exit 1
}
if [ "${MKTD_EXIT}" -eq 124 ] || [ "${MKTD_EXIT}" -eq 137 ]; then
  echo "ERROR: mktd hard-timeout after ${MKTD_TIMEOUT_SECONDS}s (#1118 part A)." >&2
  print_mktd_failure_context
  exit 1
fi
LATEST_TS="$("${CSA_BIN}" todo list --format json | jq -r --arg br "${CURRENT_BRANCH}" '[.[] | select(.branch == $br)] | sort_by(.timestamp) | last | .timestamp // empty')"
if [ -z "${LATEST_TS}" ]; then
  fail_step7_gate "mktd did not produce a TODO for branch ${CURRENT_BRANCH}."
fi
TODO_PATH="$("${CSA_BIN}" todo show -t "${LATEST_TS}" --path)"
if ! grep -qF -- '- [ ] ' "${TODO_PATH}"; then
  fail_step7_gate "TODO missing checkbox tasks."
fi
if ! awk '
function flush() { if (in_open == 1 && has_clause == 0) bad = 1; in_open = 0; has_clause = 0 }
function scan(text,   pos, rest) {
  pos = index(text, "DONE WHEN:")
  if (pos > 0) { rest = substr(text, pos + 10); sub(/^[ \t]+/, "", rest); if (length(rest) > 0) has_clause = 1 }
}
{
  is_open = ($0 ~ /^- \[ \]/)
  if ($0 ~ /^- \[[ xX]\]/ || $0 ~ /^#/) { flush(); if (is_open == 1) { in_open = 1; scan($0) }; next }
  if (in_open == 1) scan($0)
}
END { flush(); exit (bad + 0) }
' "${TODO_PATH}"; then
  fail_step7_gate "TODO has an open task without a mechanically-verifiable DONE WHEN: clause."
fi
if [ "${MKTD_EXIT}" -ne 0 ]; then
  echo "WARNING: mktd exited ${MKTD_EXIT}, but TODO gates passed; treating Step 7 as successful." >&2
  print_mktd_failure_context
fi
echo "CSA_VAR:MKTD_TODO_TIMESTAMP=${LATEST_TS}"
echo "CSA_VAR:MKTD_TODO_PATH=${TODO_PATH}"
