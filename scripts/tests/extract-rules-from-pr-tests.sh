#!/usr/bin/env bash
# Integration tests for scripts/extract-rules-from-pr.sh
#
# Uses a fixture JSON file and a `gh` shim to test the extractor
# without hitting the GitHub API.

set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
SCRIPT_PATH="${ROOT_DIR}/scripts/extract-rules-from-pr.sh"
FIXTURE_DIR="${ROOT_DIR}/tests/fixtures/pr-bot-findings"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

PASS=0
FAIL=0

pass() { PASS=$((PASS + 1)); echo "  PASS: $1"; }
fail() { FAIL=$((FAIL + 1)); echo "  FAIL: $1"; }

###############################################################################
# Create a gh shim that returns fixture data instead of calling the API
###############################################################################

GH_SHIM_DIR="${TMP_ROOT}/shim"
mkdir -p "${GH_SHIM_DIR}"

cat > "${GH_SHIM_DIR}/gh" << 'SHIMEOF'
#!/usr/bin/env bash
# gh shim for testing — returns fixture data based on the API path
# Expected call: gh api repos/<owner>/<repo>/pulls/<N>/comments --paginate
# or: gh api repos/<owner>/<repo>/pulls/<N>/reviews --paginate

for arg in "$@"; do
  case "${arg}" in
    repos/*/pulls/*/comments)
      cat "${GH_FIXTURE_PR_COMMENTS}"
      exit 0
      ;;
    repos/*/pulls/*/reviews)
      echo "[]"
      exit 0
      ;;
  esac
done

echo "gh shim: unhandled call: $*" >&2
exit 1
SHIMEOF
chmod +x "${GH_SHIM_DIR}/gh"

###############################################################################
# Test 1: HIGH + CRITICAL extracted, MEDIUM + no-badge filtered out
###############################################################################

echo "Test 1: Severity filter (HIGH + CRITICAL extracted, MEDIUM filtered)"

SESSION_DIR_1="${TMP_ROOT}/session-1"
mkdir -p "${SESSION_DIR_1}/output"

STDERR_1="$(
  PR_NUMBER=999 \
  SESSION_DIR="${SESSION_DIR_1}" \
  REPO_SLUG="example/repo" \
  PATH="${GH_SHIM_DIR}:${PATH}" \
  GH_FIXTURE_PR_COMMENTS="${FIXTURE_DIR}/pr-comments.json" \
  bash "${SCRIPT_PATH}" 2>&1
)"

# Check exactly 2 files emitted
FILE_COUNT_1="$(find "${SESSION_DIR_1}/output" -name 'proposed-rule-999-*.md' | wc -l | tr -d ' ')"
if [ "${FILE_COUNT_1}" = "2" ]; then
  pass "Emitted exactly 2 files (HIGH + CRITICAL)"
else
  fail "Expected 2 files, got ${FILE_COUNT_1}"
fi

# Check stderr summary
if echo "${STDERR_1}" | grep -q "extracted=2 findings=4"; then
  pass "Stderr summary correct: extracted=2 findings=4"
else
  fail "Stderr summary wrong: ${STDERR_1}"
fi

# Check file 1 (HIGH) frontmatter
FILE_1="${SESSION_DIR_1}/output/proposed-rule-999-1.md"
if [ -f "${FILE_1}" ]; then
  if grep -q "severity: high" "${FILE_1}"; then
    pass "File 1 severity = high"
  else
    fail "File 1 missing severity: high"
  fi
  if grep -q "pr: 999" "${FILE_1}"; then
    pass "File 1 pr = 999"
  else
    fail "File 1 missing pr: 999"
  fi
  if grep -q "finding-file: src/api/processor.rs" "${FILE_1}"; then
    pass "File 1 finding-file correct"
  else
    fail "File 1 finding-file wrong"
  fi
  if grep -q "finding-author: gemini-code-assist" "${FILE_1}"; then
    pass "File 1 finding-author correct"
  else
    fail "File 1 finding-author wrong"
  fi
  # Check body contains the original finding text (badge stripped)
  if grep -q "unwrap.*untrusted input" "${FILE_1}"; then
    pass "File 1 body contains finding text"
  else
    fail "File 1 body missing finding text"
  fi
else
  fail "File 1 does not exist: ${FILE_1}"
fi

# Check file 2 (CRITICAL) frontmatter
FILE_2="${SESSION_DIR_1}/output/proposed-rule-999-2.md"
if [ -f "${FILE_2}" ]; then
  if grep -q "severity: critical" "${FILE_2}"; then
    pass "File 2 severity = critical"
  else
    fail "File 2 missing severity: critical"
  fi
  if grep -q "session/manager.rs" "${FILE_2}"; then
    pass "File 2 finding-file correct"
  else
    fail "File 2 finding-file wrong"
  fi
  if grep -q "Race condition" "${FILE_2}"; then
    pass "File 2 body contains finding text"
  else
    fail "File 2 body missing finding text"
  fi
else
  fail "File 2 does not exist: ${FILE_2}"
fi

###############################################################################
# Test 2: Zero qualifying findings → 0 files, exit 0
###############################################################################

echo ""
echo "Test 2: Zero qualifying findings (all MEDIUM or no badge)"

# Create a fixture with only MEDIUM and no-badge comments
FIXTURE_ZERO="${TMP_ROOT}/zero-findings.json"
cat > "${FIXTURE_ZERO}" << 'EOF'
[
  {
    "id": 200001,
    "body": "![medium](https://www.gstatic.com/codereviewagent/medium-priority.svg)\n\nMinor style issue.",
    "user": {"login": "gemini-code-assist[bot]"},
    "html_url": "https://github.com/example/repo/pull/888#discussion_r200001",
    "commit_id": "def456",
    "path": "src/lib.rs",
    "line": 10,
    "created_at": "2026-04-20T10:00:00Z"
  },
  {
    "id": 200002,
    "body": "Looks good to me!",
    "user": {"login": "human-reviewer"},
    "html_url": "https://github.com/example/repo/pull/888#discussion_r200002",
    "commit_id": "def456",
    "path": "src/lib.rs",
    "line": 20,
    "created_at": "2026-04-20T10:01:00Z"
  }
]
EOF

SESSION_DIR_2="${TMP_ROOT}/session-2"
mkdir -p "${SESSION_DIR_2}/output"

STDERR_2="$(
  PR_NUMBER=888 \
  SESSION_DIR="${SESSION_DIR_2}" \
  REPO_SLUG="example/repo" \
  PATH="${GH_SHIM_DIR}:${PATH}" \
  GH_FIXTURE_PR_COMMENTS="${FIXTURE_ZERO}" \
  bash "${SCRIPT_PATH}" 2>&1
)"
EXIT_CODE_2=$?

if [ "${EXIT_CODE_2}" = "0" ]; then
  pass "Exit code 0 on zero qualifying findings"
else
  fail "Expected exit 0, got ${EXIT_CODE_2}"
fi

FILE_COUNT_2="$(find "${SESSION_DIR_2}/output" -name 'proposed-rule-*.md' | wc -l | tr -d ' ')"
if [ "${FILE_COUNT_2}" = "0" ]; then
  pass "Zero files emitted for zero qualifying findings"
else
  fail "Expected 0 files, got ${FILE_COUNT_2}"
fi

if echo "${STDERR_2}" | grep -q "extracted=0 findings=2"; then
  pass "Stderr summary correct: extracted=0 findings=2"
else
  fail "Stderr summary wrong: ${STDERR_2}"
fi

###############################################################################
# Test 3: Missing PR_NUMBER → exit 1
###############################################################################

echo ""
echo "Test 3: Missing PR_NUMBER error handling"

if ! (env -u PR_NUMBER SESSION_DIR="${TMP_ROOT}/session-3" PATH="${GH_SHIM_DIR}" bash "${SCRIPT_PATH}" 2>/dev/null); then
  pass "Exit 1 when PR_NUMBER is missing"
else
  fail "Expected exit 1 when PR_NUMBER is missing"
fi

###############################################################################
# Test 4: Frontmatter YAML is parseable
###############################################################################

echo ""
echo "Test 4: Frontmatter structure validation"

# Re-use session-1 output
for f in "${SESSION_DIR_1}/output"/proposed-rule-999-*.md; do
  basename="$(basename "${f}")"
  # Check frontmatter delimiters
  first_line="$(head -1 "${f}")"
  if [ "${first_line}" = "---" ]; then
    pass "${basename}: starts with ---"
  else
    fail "${basename}: does not start with ---"
  fi
  # Check closing delimiter exists
  if sed -n '2,$ p' "${f}" | grep -qm1 "^---$"; then
    pass "${basename}: has closing ---"
  else
    fail "${basename}: missing closing ---"
  fi
  # Check required frontmatter keys
  for key in source pr extracted-at severity finding-author finding-commit raw-comment-url; do
    if grep -q "^${key}:" "${f}"; then
      pass "${basename}: has ${key}"
    else
      fail "${basename}: missing ${key}"
    fi
  done
done

###############################################################################
# Summary
###############################################################################

echo ""
echo "========================================="
echo "Results: ${PASS} passed, ${FAIL} failed"
echo "========================================="

if [ "${FAIL}" -gt 0 ]; then
  exit 1
fi
exit 0
