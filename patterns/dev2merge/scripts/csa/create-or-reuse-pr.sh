set -euo pipefail
BRANCH="$(git branch --show-current)"
COMMIT_SUBJECT="$(git log -1 --format=%s)"

# --- Create or reuse PR ---
set +e
CREATE_OUTPUT="$(gh pr create --base "${DEFAULT_BRANCH}" --title "${COMMIT_SUBJECT}" --body "Auto-created by dev2merge pipeline." 2>&1)"
CREATE_RC=$?
set -e
if [ "${CREATE_RC}" -ne 0 ]; then
  if ! printf '%s\n' "${CREATE_OUTPUT}" | grep -Eiq 'already exists|a pull request already exists'; then
    echo "ERROR: gh pr create failed: ${CREATE_OUTPUT}" >&2
    exit 1
  fi
  echo "INFO: PR already exists for ${BRANCH}; continuing."
fi

# --- Resolve PR number (retry + fallback) ---
PR_NUMBER=""
PR_URL=""
for attempt in 1 2 3; do
  PR_JSON="$(gh pr view --json number,url,state -q 'select(.state == "OPEN")' 2>/dev/null || true)"
  PR_NUMBER="$(printf '%s' "${PR_JSON}" | jq -r '.number // empty' 2>/dev/null || true)"
  if [ -n "${PR_NUMBER}" ] && printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
    PR_URL="$(printf '%s' "${PR_JSON}" | jq -r '.url // empty')"
    break
  fi
  PR_NUMBER=""
  if [ "${attempt}" -lt 3 ]; then
    echo "INFO: PR resolution attempt ${attempt} failed, retrying in 5s..."
    sleep 5
  fi
done
# Fallback: gh pr list --head
if [ -z "${PR_NUMBER}" ]; then
  echo "INFO: gh pr view failed; falling back to gh pr list..."
  PR_NUMBER="$(gh pr list --head "${BRANCH}" --base "${DEFAULT_BRANCH}" --json number -q '.[0].number' 2>/dev/null || true)"
  if [ -n "${PR_NUMBER}" ] && printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
    PR_URL="$(gh pr view "${PR_NUMBER}" --json url -q '.url' 2>/dev/null || true)"
  fi
fi
if [ -z "${PR_NUMBER}" ] || ! printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
  echo "ERROR: Cannot resolve open PR number for ${BRANCH} after retries." >&2
  exit 1
fi
echo "PR #${PR_NUMBER} resolved: ${PR_URL}"
echo "CSA_VAR:PR_NUMBER=${PR_NUMBER}"
echo "CSA_VAR:PR_URL=${PR_URL}"
echo '<!-- CSA:NEXT_STEP cmd="csa plan run --sa-mode true patterns/pr-bot/workflow.toml (Step 16)" required=true -->'
