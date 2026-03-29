---
name = "pr-bot"
description = "Iterative PR review loop with configurable cloud bot: local review, push, bot trigger, false-positive arbitration, fix, merge"
allowed-tools = "Bash, Task, Read, Edit, Write, Grep, Glob"
tier = "tier-3-complex"
version = "0.1.0"
---

# PR Codex Bot Review Loop

Orchestrates iterative fix-and-review loop with cloud review bot on GitHub PRs.
Two-layer review: local pre-PR cumulative audit + cloud bot review.

**MANDATORY AUDIT TRAIL**: When an agent determines a PR-page review finding
(for example, a cloud bot finding) is NOT a real issue or is acceptable in
context (e.g., pre-production breaking change), the agent MUST post an
explanatory comment on the PR page BEFORE merging or proceeding. This creates a
permanent record of the rationale behind every dismissed PR-page finding.
Local pre-PR review findings must be fixed before PR creation; they do not use
the PR-page audit trail because no PR page exists yet. FORBIDDEN: merging with
dismissed PR-page findings without explanatory PR comments.

Staleness guard: before arbitration, each bot comment is checked against the
latest HEAD to detect whether the referenced code has been modified since the
comment was posted. Stale comments (referencing already-modified code) are
reclassified as Category A and skipped, preventing wasted debate cycles on
already-fixed issues.

FORBIDDEN: self-dismissing bot comments, skipping debate for arbitration,
running Step 2 in background, creating PR without Step 2 completion,
debating stale comments without staleness check, trusting `reviewed=true`
without SHA verification, auto-merging or auto-aborting at round limit,
proceeding when bot responds with environment/configuration setup message
instead of an actual code review (MUST stop and ask user to configure).

## Dispatcher Model Note

This pattern follows a 3-layer dispatcher architecture:
- **Layer 0 (Orchestrator)**: The main agent dispatches steps -- never touches code directly.
- **Layer 1 (Executors)**: CSA sub-agents and Task tool agents perform actual work.
- **Layer 2 (Sub-sub-agents)**: Spawned by Layer 1 for specific sub-tasks (invisible to Layer 0).

Each step below is annotated with its execution layer.

## Step 1: Commit Changes

> **Layer**: 0 (Orchestrator) -- lightweight shell command, no code reading.

Tool: bash

Ensure all changes committed. Set WORKFLOW_BRANCH once (persists through
clean branch switches in Step 11).

```bash
WORKFLOW_BRANCH="$(git branch --show-current)"
echo "CSA_VAR:WORKFLOW_BRANCH=$WORKFLOW_BRANCH"
```

## Step 2: Local Pre-PR Review (SYNCHRONOUS — MUST NOT background)

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches `csa review`, which spawns
> Layer 2 reviewer model(s) internally. Orchestrator waits for result.

Tool: bash
OnFail: abort

Run cumulative local review covering all commits since main.
This is the FOUNDATION — without it, bot unavailability cannot safely merge.

> Fast-path (SHA-verified): compare `git rev-parse HEAD` with the HEAD SHA
> stored in the latest `csa review` session metadata. If they match, skip
> Step 2 review. If they do not match (or metadata is missing), run full
> `csa review --branch main`. Any HEAD drift (including amend) auto-invalidates
> the fast-path.

```bash
set -euo pipefail
CURRENT_HEAD="$(git rev-parse HEAD)"
REVIEW_HEAD="$(csa session list --recent-review 2>/dev/null | parse_head_sha || true)"
if [ -n "${REVIEW_HEAD}" ] && [ "${CURRENT_HEAD}" = "${REVIEW_HEAD}" ]; then
  echo "Fast-path: local review already covers current HEAD."
else
  csa review --branch main
fi
REVIEW_COMPLETED=true
echo "CSA_VAR:REVIEW_COMPLETED=$REVIEW_COMPLETED"
```

## IF ${LOCAL_REVIEW_HAS_ISSUES}

## Step 3: Fix Local Review Issues

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches fix task to CSA. CSA reads
> code, applies fixes, and returns results. Orchestrator reviews outcome.

Tool: csa
Tier: tier-2-standard
OnFail: retry 3

Fix issues found by local review. Loop until clean (max 3 rounds).

## ENDIF

## Step 4: Push and Ensure PR

> **Layer**: 0 (Orchestrator) -- shell commands only, no code reading/writing.

Tool: bash
OnFail: abort

**PRECONDITION (MANDATORY)**: Step 2 local review MUST have completed successfully.
- If `REVIEW_COMPLETED` is not `true`, STOP and report:
  `ERROR: Local review (Step 2) was not completed. Run csa review --branch main before creating PR.`
- If Step 2 found unresolved issues that were not fixed in Step 3, STOP and report:
  `ERROR: Local review found unresolved issues. Fix them before PR creation.`
- FORBIDDEN: Creating a PR without completing Step 2. This violates the two-layer review guarantee.

```bash
# --- Precondition gate: review must be complete ---
if [ "${REVIEW_COMPLETED:-}" != "true" ]; then
  echo "ERROR: Local review (Step 2) was not completed."
  echo "Run csa review --branch main before creating PR."
  echo "FORBIDDEN: Creating PR without Step 2 completion."
  exit 1
fi

set -euo pipefail

# --- Early-push detection: warn if branch was already pushed before review ---
if git ls-remote --heads origin "${WORKFLOW_BRANCH}" 2>/dev/null | grep -q .; then
  echo "WARNING: Branch '${WORKFLOW_BRANCH}' was already pushed to remote before this skill ran."
  echo "Unreviewed code may have been visible to CI/reviewers. Continuing with force-push of reviewed code."
fi

git push --force-with-lease -u origin "${WORKFLOW_BRANCH}"
ORIGIN_URL="$(git remote get-url origin)"
SOURCE_OWNER="$(
  printf '%s\n' "${ORIGIN_URL}" | sed -nE \
    -e 's#^https?://([^@/]+@)?github\\.com/([^/]+)/[^/]+(\\.git)?$#\\2#p' \
    -e 's#^(ssh://)?([^@]+@)?github\\.com[:/]([^/]+)/[^/]+(\\.git)?$#\\3#p' \
    | head -n 1
)"
if [ -z "${SOURCE_OWNER}" ]; then
  SOURCE_OWNER="$(gh repo view --json owner -q '.owner.login')"
fi
find_branch_pr() {
  local owner_matches owner_count branch_matches branch_count
  owner_matches="$(
    gh pr list --base main --state open --head "${SOURCE_OWNER}:${WORKFLOW_BRANCH}" --json number \
      --jq '.[].number' 2>/dev/null || true
  )"
  owner_count="$(printf '%s\n' "${owner_matches}" | sed '/^$/d' | wc -l | tr -d ' ')"
  if [ "${owner_count}" = "1" ]; then
    printf '%s\n' "${owner_matches}" | sed '/^$/d' | head -n 1
    return 0
  fi
  if [ "${owner_count}" -gt 1 ]; then
    echo "ERROR: Multiple open PRs found for ${SOURCE_OWNER}:${WORKFLOW_BRANCH}. Resolve ambiguity manually." >&2
    return 1
  fi

  branch_matches="$(
    gh pr list --base main --state open --head "${WORKFLOW_BRANCH}" --json number \
      --jq '.[].number' 2>/dev/null || true
  )"
  branch_count="$(printf '%s\n' "${branch_matches}" | sed '/^$/d' | wc -l | tr -d ' ')"
  if [ "${branch_count}" = "1" ]; then
    printf '%s\n' "${branch_matches}" | sed '/^$/d' | head -n 1
    return 0
  fi
  if [ "${branch_count}" -gt 1 ]; then
    echo "ERROR: Multiple open PRs found for branch ${WORKFLOW_BRANCH}. Resolve ambiguity manually." >&2
    return 1
  fi
  return 2
}

set +e
PR_NUM="$(find_branch_pr)"
FIND_RC=$?
set -e
if [ "${FIND_RC}" = "0" ] && [ -n "${PR_NUM}" ]; then
  echo "Using existing PR #${PR_NUM} for branch ${WORKFLOW_BRANCH}"
elif [ "${FIND_RC}" = "1" ]; then
  exit 1
else
  set +e
  CREATE_OUTPUT="$(gh pr create --base main --head "${SOURCE_OWNER}:${WORKFLOW_BRANCH}" --title "${PR_TITLE}" --body "${PR_BODY}" 2>&1)"
  CREATE_RC=$?
  set -e
  if [ "${CREATE_RC}" != "0" ]; then
    if ! printf '%s\n' "${CREATE_OUTPUT}" | grep -Eiq 'already exists|a pull request already exists'; then
      echo "ERROR: gh pr create failed: ${CREATE_OUTPUT}" >&2
      exit 1
    fi
    echo "Detected existing PR during create race; resolving PR number by owner+branch."
  fi
  FIND_RC=2
  PR_NUM=""
  for attempt in 1 2 3 4 5; do
    set +e
    PR_NUM="$(find_branch_pr)"
    FIND_RC=$?
    set -e
    if [ "${FIND_RC}" = "0" ] && [ -n "${PR_NUM}" ]; then
      break
    fi
    if [ "${FIND_RC}" = "1" ]; then
      break
    fi
    sleep 2
  done
  if [ "${FIND_RC}" != "0" ] || [ -z "${PR_NUM}" ]; then
    echo "ERROR: Failed to resolve a unique PR for branch ${WORKFLOW_BRANCH} targeting main." >&2
    exit 1
  fi
fi
if [ -z "${PR_NUM:-}" ] || ! printf '%s' "${PR_NUM}" | grep -Eq '^[0-9]+$'; then
  echo "ERROR: Failed to resolve PR number for branch ${WORKFLOW_BRANCH} targeting main." >&2
  exit 1
fi
REPO="$(gh repo view --json nameWithOwner -q '.nameWithOwner')"
echo "CSA_VAR:PR_NUM=$PR_NUM"
echo "CSA_VAR:REPO=$REPO"
```

## Step 4a: Check Cloud Bot Configuration

> **Layer**: 0 (Orchestrator) -- config check, lightweight.

Tool: bash
OnFail: abort

Check whether cloud bot review is enabled for this project.

```bash
set -euo pipefail
CLOUD_BOT=$(csa config get pr_review.cloud_bot --default true)
CLOUD_BOT_NAME=$(csa config get pr_review.cloud_bot_name --default gemini-code-assist)
CLOUD_BOT_TRIGGER=$(csa config get pr_review.cloud_bot_trigger --default auto)
CLOUD_BOT_LOGIN_RAW=$(csa config get pr_review.cloud_bot_login --default "")
if [ -z "${CLOUD_BOT_LOGIN_RAW}" ]; then
  CLOUD_BOT_LOGIN="${CLOUD_BOT_NAME}[bot]"
else
  CLOUD_BOT_LOGIN="${CLOUD_BOT_LOGIN_RAW}"
fi
# Retrigger command for post-fix re-review (round 2+).
# Default: "/gemini review" for gemini-code-assist, "@<name> review" for others.
CLOUD_BOT_RETRIGGER_CMD_RAW=$(csa config get pr_review.cloud_bot_retrigger_command --default "")
if [ -z "${CLOUD_BOT_RETRIGGER_CMD_RAW}" ]; then
  if [ "${CLOUD_BOT_NAME}" = "gemini-code-assist" ]; then
    CLOUD_BOT_RETRIGGER_CMD="/gemini review"
  else
    CLOUD_BOT_RETRIGGER_CMD="@${CLOUD_BOT_NAME} review"
  fi
else
  CLOUD_BOT_RETRIGGER_CMD="${CLOUD_BOT_RETRIGGER_CMD_RAW}"
fi
if [ "${CLOUD_BOT}" = "false" ]; then
  BOT_UNAVAILABLE=true
  FALLBACK_REVIEW_HAS_ISSUES=false
  CURRENT_HEAD="$(git rev-parse HEAD)"
  REVIEW_HEAD="$(csa session list --recent-review 2>/dev/null | parse_head_sha || true)"
  if [ -n "${REVIEW_HEAD}" ] && [ "${CURRENT_HEAD}" = "${REVIEW_HEAD}" ]; then
    echo "Cloud bot disabled, fast-path active: local review already covers HEAD ${CURRENT_HEAD}."
  else
    echo "Cloud bot disabled and fast-path invalid. Running full local review."
    csa review --branch main
  fi
fi
BOT_UNAVAILABLE="${BOT_UNAVAILABLE:-false}"
FALLBACK_REVIEW_HAS_ISSUES="${FALLBACK_REVIEW_HAS_ISSUES:-false}"
echo "CSA_VAR:CLOUD_BOT_NAME=$CLOUD_BOT_NAME"
echo "CSA_VAR:CLOUD_BOT_TRIGGER=$CLOUD_BOT_TRIGGER"
echo "CSA_VAR:CLOUD_BOT_LOGIN=$CLOUD_BOT_LOGIN"
echo "CSA_VAR:CLOUD_BOT_RETRIGGER_CMD=$CLOUD_BOT_RETRIGGER_CMD"
echo "CSA_VAR:BOT_UNAVAILABLE=$BOT_UNAVAILABLE"
echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
```

If `CLOUD_BOT` is `false`:
- Skip Steps 5 through 10 (cloud bot trigger, delegated wait gate, classify, arbitrate, fix).
- Reuse the same SHA-verified fast-path before supplementary review:
  - If current `HEAD` matches latest reviewed session HEAD SHA → skip review.
  - Otherwise run full `csa review --branch main`.
- Route to Step 6a (Merge Without Bot) after supplementary local review gate passes.

## IF ${CLOUD_BOT} != "false"

## Step 5: Trigger Cloud Bot Review and Delegate Waiting

> **Layer**: 0 + 1 (Orchestrator + CSA executor).
> Layer 0 triggers cloud bot review (via @mention or auto-review depending on
> `cloud_bot_trigger` config and review round) and delegates the long wait to a
> single CSA-managed step. No explicit caller-side polling loop.

Tool: bash
OnFail: abort
Condition: !(${BOT_UNAVAILABLE})

Trigger cloud bot review for current HEAD. Trigger method is **round-aware**:
- **Round 0** (initial PR creation): follows `cloud_bot_trigger` config
  ("comment" posts `@bot review`, "auto" skips trigger since bot auto-reviews).
- **Round 1+** (after fix push): ALWAYS posts explicit retrigger command
  (`cloud_bot_retrigger_command`, default: `/gemini review` for gemini-code-assist)
  because bots do NOT auto-review on subsequent pushes (#506).

Wait 5 minutes, then delegate 10-minute polling to CSA. Total wait: ~15 minutes.
If bot times out, **ABORT the workflow** — user must decide next action.
Also detects non-target bot comments (e.g., codex auto-review when
configured bot is gemini-code-assist) and includes them with a quota warning.

```bash
set -euo pipefail
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
run_with_hard_timeout() {
  local timeout_secs="$1"
  shift
  if [ -n "${TIMEOUT_BIN}" ]; then
    "${TIMEOUT_BIN}" -k 5s "${timeout_secs}s" "$@" 2>&1
    return $?
  fi

  local tmp_out timeout_flag child_pid watcher_pid rc use_pgroup
  tmp_out="$(mktemp)"
  timeout_flag="$(mktemp)"
  rm -f "${timeout_flag}"
  use_pgroup=false
  if command -v setsid >/dev/null 2>&1; then
    setsid "$@" >"${tmp_out}" 2>&1 &
    use_pgroup=true
    child_pid=$!
  else
    "$@" >"${tmp_out}" 2>&1 &
    child_pid=$!
  fi
  (
    sleep "${timeout_secs}"
    if kill -0 "${child_pid}" 2>/dev/null; then
      : >"${timeout_flag}"
      if [ "${use_pgroup}" = "true" ]; then
        kill -TERM "-${child_pid}" 2>/dev/null || true
      else
        kill -TERM "${child_pid}" 2>/dev/null || true
      fi
      sleep 2
      if kill -0 "${child_pid}" 2>/dev/null; then
        if [ "${use_pgroup}" = "true" ]; then
          kill -KILL "-${child_pid}" 2>/dev/null || true
        else
          kill -KILL "${child_pid}" 2>/dev/null || true
        fi
      fi
    fi
  ) &
  watcher_pid=$!
  wait "${child_pid}"
  rc=$?
  kill "${watcher_pid}" 2>/dev/null || true
  cat "${tmp_out}"
  rm -f "${tmp_out}"
  if [ -f "${timeout_flag}" ]; then
    rm -f "${timeout_flag}"
    return 124
  fi
  rm -f "${timeout_flag}"
  return "${rc}"
}

# --- Trigger cloud bot review for current HEAD ---
CURRENT_SHA="$(git rev-parse HEAD)"
TRIGGER_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
REVIEW_ROUND="${REVIEW_ROUND:-0}"

# Round-aware trigger logic (#506):
# - Round 0 (initial PR creation) + auto: bot auto-reviews on push, skip trigger
# - Round 1+ (after fix push): bot does NOT auto-review on force-push,
#   MUST explicitly trigger re-review regardless of cloud_bot_trigger setting
if [ "${REVIEW_ROUND}" -gt 0 ]; then
  # Post-fix round: ALWAYS trigger explicitly via retrigger command
  TRIGGER_BODY="${CLOUD_BOT_RETRIGGER_CMD}

<!-- csa-retrigger:round${REVIEW_ROUND}:${CURRENT_SHA}:${TRIGGER_TS} -->"
  gh pr comment "${PR_NUM}" --repo "${REPO}" --body "${TRIGGER_BODY}"
  echo "Round ${REVIEW_ROUND}: Triggered re-review via '${CLOUD_BOT_RETRIGGER_CMD}' for HEAD ${CURRENT_SHA}."
  WAIT_BASE_TS="${TRIGGER_TS}"
elif [ "${CLOUD_BOT_TRIGGER}" = "comment" ]; then
  # Round 0 + comment mode: trigger via @mention
  TRIGGER_BODY="@${CLOUD_BOT_NAME} review

<!-- csa-trigger:${CURRENT_SHA}:${TRIGGER_TS} -->"
  gh pr comment "${PR_NUM}" --repo "${REPO}" --body "${TRIGGER_BODY}"
  echo "Triggered @${CLOUD_BOT_NAME} review via comment for HEAD ${CURRENT_SHA}."
  WAIT_BASE_TS="${TRIGGER_TS}"
else
  # Round 0 + auto mode: bot auto-reviews on PR push
  echo "Cloud bot trigger mode is '${CLOUD_BOT_TRIGGER}' (auto-review); skipping @mention trigger."
  echo "Bot will auto-review the PR push. Proceeding to polling phase."
  # Backdate the window by 10 minutes to catch responses that arrived between push and now.
  WAIT_BASE_TS="$(date -u -d '10 minutes ago' +%Y-%m-%dT%H:%M:%SZ 2>/dev/null || date -u -v-10M +%Y-%m-%dT%H:%M:%SZ)"
fi

# --- Initial quiet wait (5 min) — bot responses rarely arrive faster ---
echo "Waiting 5 minutes before polling (bot responses rarely arrive faster)..."
sleep 300

# --- Delegate remaining polling to CSA-managed step (max 10 min) ---
BOT_UNAVAILABLE=true
FALLBACK_REVIEW_HAS_ISSUES=false
BOT_HAS_ISSUES=false
WAIT_RESULT_FILE="$(mktemp)"
set +e
run_with_hard_timeout 650 csa run --force-ignore-tier-setting --tool auto --idle-timeout 650 "Bounded wait task only. Do NOT invoke pr-bot skill or any full PR workflow. Operate on PR #${PR_NUM} in repo ${REPO}. Wait for @${CLOUD_BOT_NAME} review on HEAD ${CURRENT_SHA}. Check for a review EVENT via 'gh api repos/${REPO}/pulls/${PR_NUM}/reviews' with submitted_at after ${WAIT_BASE_TS} and user.login matching the bot. Also check issue comments for bot activity. Max wait 10 minutes (5-minute quiet wait already elapsed before this step). Do not edit code. Return exactly one marker line: BOT_REPLY=received or BOT_REPLY=timeout." | tee "${WAIT_RESULT_FILE}"
WAIT_RC=${PIPESTATUS[0]}
set -e
WAIT_RESULT="$(cat "${WAIT_RESULT_FILE}")"
rm -f "${WAIT_RESULT_FILE}"
if [ "${WAIT_RC}" -eq 124 ]; then
  BOT_UNAVAILABLE=true
elif [ "${WAIT_RC}" -ne 0 ]; then
  echo "WARN: Delegated bot wait failed (rc=${WAIT_RC}); treating cloud bot as unavailable." >&2
  BOT_UNAVAILABLE=true
else
  WAIT_MARKER="$(
    printf '%s\n' "${WAIT_RESULT}" \
      | grep -E '^[[:space:]]*BOT_REPLY=(received|timeout)[[:space:]]*$' \
      | tail -n 1 \
      | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//' \
      || true
  )"
  if [ "${WAIT_MARKER}" = "BOT_REPLY=received" ]; then
    BOT_UNAVAILABLE=false
    BOT_SETTLE_SECS="${BOT_SETTLE_SECS:-20}"
    sleep "${BOT_SETTLE_SECS}"

    # --- Positive signal check (#505): verify a review EVENT exists ---
    # A review event with submitted_at > WAIT_BASE_TS AND commit_id matching
    # CURRENT_SHA is the positive confirmation that the bot actually reviewed
    # current HEAD. Without this, "0 new comments" is ambiguous (could mean
    # bot reviewed and found nothing, or bot hasn't reviewed yet). The
    # commit_id filter prevents a late review of a previous push from being
    # mistaken for a review of the current HEAD.
    set +e
    REVIEW_EVENT_RAW="$(
      gh api --paginate "repos/${REPO}/pulls/${PR_NUM}/reviews?per_page=100" \
        --jq '[.[] | select(.user.login == "'"${CLOUD_BOT_LOGIN}"'") | select(.submitted_at > "'"${WAIT_BASE_TS}"'") | select(.commit_id == "'"${CURRENT_SHA}"'" or .commit_id == null)] | length' \
        2>/dev/null
    )"
    REVIEW_EVENT_RC=$?
    set -e
    REVIEW_EVENT_COUNT="$(echo "${REVIEW_EVENT_RAW}" | awk '{s+=$1} END {print s+0}')"
    if [ "${REVIEW_EVENT_RC}" -ne 0 ]; then
      echo "WARN: Failed to query review events (rc=${REVIEW_EVENT_RC}); treating as bot unavailable." >&2
      BOT_UNAVAILABLE=true
    fi
    if [ "${BOT_UNAVAILABLE}" = "false" ] && [ "${REVIEW_EVENT_COUNT:-0}" -eq 0 ]; then
      echo "WARN: Bot activity detected but no review event for HEAD ${CURRENT_SHA} found after ${WAIT_BASE_TS}." >&2
      echo "This likely means the bot posted a comment but did not submit a formal review." >&2
      # Check if it's a setup/configuration message
      set +e
      SETUP_BODY="$(
        gh api "repos/${REPO}/issues/${PR_NUM}/comments?per_page=100" \
          --jq '[.[] | select(.user.login == "'"${CLOUD_BOT_LOGIN}"'") | select(.created_at > "'"${WAIT_BASE_TS}"'")] | .[0].body // ""' \
          2>/dev/null
      )"
      set -e
      if [ -n "${SETUP_BODY}" ] && echo "${SETUP_BODY}" | grep -qEi 'configur|set.?up.*(environment|repo)|environment.*(set.?up|configur|need)|unable.to.(review|access)|cannot.*(complete|access|review)|not.*configured|permission|credential'; then
        echo "ERROR: Cloud bot responded with a setup/configuration message instead of a code review." >&2
        echo "Bot response (truncated): $(echo "${SETUP_BODY}" | head -c 500)" >&2
        echo "" >&2
        echo "ACTION REQUIRED: Configure the cloud bot, then re-run pr-bot." >&2
        BOT_NEEDS_SETUP=true
      else
        echo "Treating as bot unavailable — no positive review signal." >&2
        BOT_UNAVAILABLE=true
      fi
    fi

    # --- Check inline comments for actionable findings (only if review confirmed) ---
    if [ "${BOT_UNAVAILABLE}" = "false" ] && [ "${BOT_NEEDS_SETUP:-false}" = "false" ]; then
      set +e
      ACTIONABLE_COMMENT_COUNT="$(
        gh api "repos/${REPO}/pulls/${PR_NUM}/comments?per_page=100" \
          --jq '[.[] | select(.user.login == "'"${CLOUD_BOT_LOGIN}"'") | select(.created_at > "'"${WAIT_BASE_TS}"'") | select((.body | test("P0|P1|P2"))) ] | length' \
          2>/dev/null
      )"
      ACTIONABLE_COMMENT_RC=$?
      set -e
      if [ "${ACTIONABLE_COMMENT_RC}" -ne 0 ]; then
        echo "ERROR: Failed to query actionable bot comments for trigger window (rc=${ACTIONABLE_COMMENT_RC})." >&2
        exit 1
      fi
      case "${ACTIONABLE_COMMENT_COUNT:-}" in
        ''|*[!0-9]*)
          echo "ERROR: Invalid actionable comment count from GitHub API: '${ACTIONABLE_COMMENT_COUNT}'." >&2
          exit 1
          ;;
      esac
      if [ "${ACTIONABLE_COMMENT_COUNT}" -gt 0 ]; then
        echo "Detected ${ACTIONABLE_COMMENT_COUNT} actionable bot comment(s) after trigger window; marking BOT_HAS_ISSUES=true."
        BOT_HAS_ISSUES=true
      fi
    fi
  elif [ "${WAIT_MARKER}" = "BOT_REPLY=timeout" ]; then
    BOT_UNAVAILABLE=true
  else
    echo "WARN: Delegated bot wait returned no marker; treating cloud bot as unavailable." >&2
    BOT_UNAVAILABLE=true
  fi
fi

# --- Check for non-target bot comments (e.g., codex auto-review) ---
if [ "${BOT_UNAVAILABLE}" = "false" ] && [ "${BOT_HAS_ISSUES}" = "false" ]; then
  set +e
  OTHER_BOT_COUNT="$(
    gh api "repos/${REPO}/pulls/${PR_NUM}/comments?per_page=100" \
      --jq '[.[] | select(.user.type == "Bot") | select(.user.login != "'"${CLOUD_BOT_LOGIN}"'") | select(.created_at > "'"${WAIT_BASE_TS}"'") | select((.body | test("P0|P1|P2"))) ] | length' \
      2>/dev/null || echo "0"
  )"
  set -e
  if [ "${OTHER_BOT_COUNT:-0}" -gt 0 ]; then
    echo "WARNING: Detected ${OTHER_BOT_COUNT} actionable comment(s) from non-target bot(s)." >&2
    echo "WARNING: These may consume coding quota for the originating bot service." >&2
    echo "Including non-target bot findings in review."
    BOT_HAS_ISSUES=true
  fi
fi

if [ "${BOT_UNAVAILABLE}" = "true" ]; then
  echo "" >&2
  echo "ERROR: Cloud bot (${CLOUD_BOT_NAME}) did not respond within the polling window (~15 min)." >&2
  echo "" >&2
  echo "Options:" >&2
  echo "  1. Wait and re-run: csa plan run patterns/pr-bot/workflow.toml" >&2
  echo "  2. Disable cloud bot: csa config set pr_review.cloud_bot false && csa plan run patterns/pr-bot/workflow.toml" >&2
  echo "  3. Investigate bot configuration" >&2
  echo "" >&2
  echo "ABORTING: Will not merge without cloud bot confirmation." >&2
  exit 1
fi
echo "CSA_VAR:BOT_REVIEW_WINDOW_START=$WAIT_BASE_TS"
echo "CSA_VAR:BOT_UNAVAILABLE=$BOT_UNAVAILABLE"
echo "CSA_VAR:BOT_NEEDS_SETUP=${BOT_NEEDS_SETUP:-false}"
echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
echo "CSA_VAR:BOT_HAS_ISSUES=$BOT_HAS_ISSUES"
```

## IF ${BOT_NEEDS_SETUP}

## Step 5a: Abort — Bot Needs Environment Configuration

Tool: none (orchestrator action)
OnFail: abort

The Cloud bot responded but did not perform an actual code review — it sent a
message indicating environment configuration is needed. The workflow MUST stop
here and report to the user. FORBIDDEN: falling back to local review, skipping
the bot review, or proceeding in any way.

**Orchestrator action**: Report the bot's response to the user and STOP. The
user must:
1. Configure the cloud bot (follow the bot's instructions)
2. Re-run `pr-bot` after configuration is complete

## ENDIF

## IF ${BOT_UNAVAILABLE}

## Step 6-fix: Fallback Review Fix Cycle (Bot Timeout Path)

> **Layer**: 0 + 1 (Orchestrator + CSA executor) -- wrapper step enforces
> timeout/return-code/marker contracts, while CSA performs the fix cycle.

Tool: bash
OnFail: abort

When `FALLBACK_REVIEW_HAS_ISSUES=true` (set in Step 5 when `csa review`
found issues during bot timeout), this dedicated fix cycle runs WITHIN the
timeout branch. Steps 7-10 are structurally inside the `BOT_UNAVAILABLE=false`
branch and are NOT reachable from here.

Delegate this cycle to CSA as a single operation and enforce hard bounds:
- command-level hard timeout: `timeout 1800s`
- no `|| true` silent downgrade
- success requires marker `FALLBACK_FIX=clean`
- on success, orchestrator explicitly sets `FALLBACK_REVIEW_HAS_ISSUES=false`

```bash
set -euo pipefail
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
run_with_hard_timeout() {
  local timeout_secs="$1"
  shift
  if [ -n "${TIMEOUT_BIN}" ]; then
    "${TIMEOUT_BIN}" -k 5s "${timeout_secs}s" "$@" 2>&1
    return $?
  fi

  local tmp_out timeout_flag child_pid watcher_pid rc use_pgroup
  tmp_out="$(mktemp)"
  timeout_flag="$(mktemp)"
  rm -f "${timeout_flag}"
  use_pgroup=false
  if command -v setsid >/dev/null 2>&1; then
    setsid "$@" >"${tmp_out}" 2>&1 &
    use_pgroup=true
    child_pid=$!
  else
    "$@" >"${tmp_out}" 2>&1 &
    child_pid=$!
  fi
  (
    sleep "${timeout_secs}"
    if kill -0 "${child_pid}" 2>/dev/null; then
      : >"${timeout_flag}"
      if [ "${use_pgroup}" = "true" ]; then
        kill -TERM "-${child_pid}" 2>/dev/null || true
      else
        kill -TERM "${child_pid}" 2>/dev/null || true
      fi
      sleep 2
      if kill -0 "${child_pid}" 2>/dev/null; then
        if [ "${use_pgroup}" = "true" ]; then
          kill -KILL "-${child_pid}" 2>/dev/null || true
        else
          kill -KILL "${child_pid}" 2>/dev/null || true
        fi
      fi
    fi
  ) &
  watcher_pid=$!
  wait "${child_pid}"
  rc=$?
  kill "${watcher_pid}" 2>/dev/null || true
  cat "${tmp_out}"
  rm -f "${tmp_out}"
  if [ -f "${timeout_flag}" ]; then
    rm -f "${timeout_flag}"
    return 124
  fi
  rm -f "${timeout_flag}"
  return "${rc}"
}
set +e
FIX_RESULT_FILE="$(mktemp)"
run_with_hard_timeout 1800 csa run --force-ignore-tier-setting --tool auto --idle-timeout 1800 "Bounded fallback-fix task only. Do NOT invoke pr-bot skill or any full PR workflow. Operate on PR #${PR_NUM} in repo ${REPO}. Bot is unavailable and fallback local review found issues. Run a self-contained max-3-round fix cycle: read latest findings from csa review --range main...HEAD, apply fixes with commits, re-run review, repeat until clean. Return exactly one marker line FALLBACK_FIX=clean when clean; otherwise return FALLBACK_FIX=failed and exit non-zero." | tee "${FIX_RESULT_FILE}"
FIX_RC=${PIPESTATUS[0]}
set -e
FIX_RESULT="$(cat "${FIX_RESULT_FILE}")"
rm -f "${FIX_RESULT_FILE}"

if [ "${FIX_RC}" -eq 124 ]; then
  echo "ERROR: Fallback fix cycle exceeded hard timeout (1800s)." >&2
  exit 1
fi
if [ "${FIX_RC}" -ne 0 ]; then
  echo "ERROR: Fallback fix cycle failed (rc=${FIX_RC})." >&2
  exit 1
fi
FIX_MARKER="$(
  printf '%s\n' "${FIX_RESULT}" \
    | grep -E '^[[:space:]]*FALLBACK_FIX=(clean|failed)[[:space:]]*$' \
    | tail -n 1 \
    | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//' \
    || true
)"
if [ "${FIX_MARKER}" != "FALLBACK_FIX=clean" ]; then
  echo "ERROR: Fallback fix cycle returned invalid marker." >&2
  exit 1
fi

FALLBACK_REVIEW_HAS_ISSUES=false
echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
```

## Step 6a: Merge Without Bot

> **Layer**: 0 (Orchestrator) -- merge command, no code analysis.

Tool: bash

Cloud bot explicitly disabled (cloud_bot=false). Local fallback review passed
(either initially in Step 4a, or after fix cycle in Step 6-fix). Step 6-fix
guarantees `FALLBACK_REVIEW_HAS_ISSUES=false` before reaching this point.
NOTE: This step only executes when cloud_bot=false (not on timeout — timeout aborts).

**MANDATORY**: Before merging, leave a PR comment explaining the merge rationale
(bot disabled + local review CLEAN). This provides audit trail for reviewers.

```bash
# --- Hard gate: unconditional pre-merge check ---
if [ "${FALLBACK_REVIEW_HAS_ISSUES:-false}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved fallback review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi
if [ "${REBASE_REVIEW_HAS_ISSUES:-false}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved post-rebase review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi

# Push fallback fix commits so the remote PR head includes them.
# Without this, gh pr merge uses the stale remote HEAD and omits fixes.
git push origin "${WORKFLOW_BRANCH}"

# Audit trail: explain why merging without bot review.
gh pr comment "${PR_NUM}" --repo "${REPO}" --body \
  "**Merge rationale**: Cloud bot (${CLOUD_BOT_NAME}) is disabled (pr_review.cloud_bot=false). Local \`csa review --branch main\` passed CLEAN (or issues were fixed in fallback cycle). Proceeding to merge with local review as the review layer."

MERGE_STRATEGY=$(csa config get pr_review.merge_strategy --default merge)
DELETE_BRANCH_FLAG=""
if [ "$(csa config get pr_review.delete_branch --default false)" = "true" ]; then
  DELETE_BRANCH_FLAG="--delete-branch"
fi
# shellcheck disable=SC2086
gh pr merge "${PR_NUM}" --repo "${REPO}" --"${MERGE_STRATEGY}" ${DELETE_BRANCH_FLAG}

# Post-merge: sync local main with remote
git fetch origin
git checkout main
git merge origin/main --ff-only
git log --oneline -1  # verify local matches remote
```

## ELSE

## IF ${BOT_HAS_ISSUES}

## Step 7: Select Current Bot Comment

> **Layer**: 0 (Orchestrator) -- shell-only selection of a single current
> bot comment for v1 flat execution. The workflow handles one current bot
> comment per loop iteration; after Step 10 pushes, the next bot review
> trigger picks up any remaining findings.

Tool: bash
OnFail: abort

Select one actionable bot review comment from the current review window and
export its metadata as `CURRENT_COMMENT_ID`, `COMMENT_PATH`, and
`COMMENT_TIMESTAMP`. Initialize `COMMENT_IS_FALSE_POSITIVE=true` and
`COMMENT_IS_STALE=false` so the current comment always enters the arbitration
path first unless the staleness guard suppresses it.

```bash
set -euo pipefail
if [ -z "${BOT_REVIEW_WINDOW_START:-}" ]; then
  echo "ERROR: BOT_REVIEW_WINDOW_START is unset."
  exit 1
fi

# Query from ANY bot (not just target) to also catch non-target bot findings
COMMENT_RECORD="$(
  gh api "repos/${REPO}/pulls/${PR_NUM}/comments?per_page=100" \
    --jq '[.[] | select(.user.type == "Bot") | select(.created_at > "'"${BOT_REVIEW_WINDOW_START}"'") | select((.body | test("P0|P1|P2"))) ] | sort_by(.created_at) | .[0] | [(.id | tostring), (.path // ""), .created_at] | @tsv'
)"
if [ -z "${COMMENT_RECORD}" ] || [ "${COMMENT_RECORD}" = "null" ]; then
  echo "ERROR: BOT_HAS_ISSUES=true but no actionable current bot comment was found."
  exit 1
fi

IFS=$'\t' read -r CURRENT_COMMENT_ID COMMENT_PATH COMMENT_TIMESTAMP <<EOF
${COMMENT_RECORD}
EOF

echo "CSA_VAR:CURRENT_COMMENT_ID=$CURRENT_COMMENT_ID"
echo "CSA_VAR:COMMENT_PATH=$COMMENT_PATH"
echo "CSA_VAR:COMMENT_TIMESTAMP=$COMMENT_TIMESTAMP"
echo "CSA_VAR:COMMENT_IS_FALSE_POSITIVE=true"
echo "CSA_VAR:COMMENT_IS_STALE=false"
```

## Step 7a: Staleness Filter

Tool: bash
OnFail: skip

For the current bot comment selected in Step 7, run a conservative file-level
staleness check. When the referenced file changed on this branch after the
comment timestamp, set `COMMENT_IS_STALE=true` and skip arbitration/fix for
this loop iteration.

```bash
set -euo pipefail
COMMENT_IS_STALE=false

if [ -n "${COMMENT_PATH:-}" ] && [ -n "${COMMENT_TIMESTAMP:-}" ]; then
  if ! git diff --quiet main...HEAD -- "${COMMENT_PATH}"; then
    if git log --since="${COMMENT_TIMESTAMP}" --format=%H -- "${COMMENT_PATH}" | grep -q .; then
      COMMENT_IS_STALE=true
    fi
  fi
fi

echo "CSA_VAR:COMMENT_IS_STALE=$COMMENT_IS_STALE"
```

## IF ${COMMENT_IS_FALSE_POSITIVE} && !(${COMMENT_IS_STALE})

## Step 8: Arbitrate via Debate

> **Layer**: 1 (CSA debate) -- Layer 0 dispatches to `csa debate`, which
> internally spawns Layer 2 independent models for adversarial evaluation.
> Orchestrator receives the verdict and posts audit trail to PR.

Tool: csa
Tier: tier-2-standard

## INCLUDE debate

MUST use independent model for arbitration.
NEVER dismiss bot comments using own reasoning alone.
Emit structured output for the caller:
- `VERDICT: DISMISSED|CONFIRMED`
- `RATIONALE: ...`
- `PR_COMMENT_START` / `PR_COMMENT_END`
- For `DISMISSED`, the comment body must include:
  `**Local arbitration result: DISMISSED.**`, `## Participants`,
  `## Bot Concern`, `## Debate Summary`, `## Conclusion`, and
  `CSA session ID: ...`

Emit each marker exactly once, in the order shown, and do not repeat the
format description in the answer.

The workflow posts the audit trail to PR in a dedicated `gh pr comment` step
and aborts if comment creation fails.

**MANDATORY AUDIT TRAIL**: If the debate determines the PR-page finding is NOT
a real issue (e.g., false positive, project status justifies it), the agent
MUST post an explanatory comment on the PR page BEFORE proceeding. The comment
must include the debate result and the specific rationale (e.g.,
'Pre-production: breaking API changes are acceptable per versioning rule 019').
FORBIDDEN: dismissing findings without an explanatory PR comment.

Use the current comment metadata exported by Step 7:
- `CURRENT_COMMENT_ID`
- `COMMENT_PATH`
- `COMMENT_TIMESTAMP`

The debate sub-agent MUST fetch the review comment body itself:
- `gh api repos/${REPO}/pulls/comments/${CURRENT_COMMENT_ID}`

## Step 8a: Post Debate Audit Trail Comment

> **Layer**: 0 (Orchestrator) -- explicit bash step that posts the PR comment
> or reroutes to the fix path based on the debate verdict.

Tool: bash
OnFail: abort

Parse the structured debate result from Step 8.
- If `VERDICT=DISMISSED`: post the explanatory PR comment explicitly via
  `gh pr comment` and fail if comment creation fails.
- If `VERDICT=CONFIRMED`: do not post a dismissal comment; set
  `COMMENT_IS_FALSE_POSITIVE=false` so the workflow routes the current comment into
  Step 9 (fix real issue).

```bash
DEBATE_OUTPUT="${STEP_10_OUTPUT}"
VERDICT_COUNT="$(
  printf '%s\n' "${DEBATE_OUTPUT}" \
    | grep -Ec '^[[:space:]]*VERDICT: (DISMISSED|CONFIRMED)[[:space:]]*$' \
    || true
)"
if [ "${VERDICT_COUNT}" != "1" ]; then
  echo "ERROR: Debate output must contain exactly one VERDICT marker." >&2
  exit 1
fi
VERDICT_MARKER="$(
  printf '%s\n' "${DEBATE_OUTPUT}" \
    | grep -E '^[[:space:]]*VERDICT: (DISMISSED|CONFIRMED)[[:space:]]*$' \
    | tail -n 1 \
    | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//' \
    || true
)"

if [ -z "${VERDICT_MARKER}" ]; then
  echo "ERROR: Debate output missing VERDICT marker." >&2
  exit 1
fi

VERDICT="${VERDICT_MARKER#VERDICT: }"

case "${VERDICT}" in
  DISMISSED)
    COMMENT_START_COUNT="$(printf '%s\n' "${DEBATE_OUTPUT}" | grep -Ec '^[[:space:]]*PR_COMMENT_START[[:space:]]*$' || true)"
    COMMENT_END_COUNT="$(printf '%s\n' "${DEBATE_OUTPUT}" | grep -Ec '^[[:space:]]*PR_COMMENT_END[[:space:]]*$' || true)"
    if [ "${COMMENT_START_COUNT}" != "1" ] || [ "${COMMENT_END_COUNT}" != "1" ]; then
      echo "ERROR: Debate output must contain exactly one PR comment marker pair." >&2
      exit 1
    fi
    COMMENT_START_LINE="$(printf '%s\n' "${DEBATE_OUTPUT}" | grep -n -E '^[[:space:]]*PR_COMMENT_START[[:space:]]*$' | tail -n 1 | cut -d: -f1 || true)"
    COMMENT_END_LINE="$(printf '%s\n' "${DEBATE_OUTPUT}" | grep -n -E '^[[:space:]]*PR_COMMENT_END[[:space:]]*$' | tail -n 1 | cut -d: -f1 || true)"
    if [ -z "${COMMENT_START_LINE}" ] || [ -z "${COMMENT_END_LINE}" ] || [ "${COMMENT_END_LINE}" -le "${COMMENT_START_LINE}" ]; then
      echo "ERROR: Debate output has an invalid PR comment marker range." >&2
      exit 1
    fi
    COMMENT_FILE="$(mktemp)"
    printf '%s\n' "${DEBATE_OUTPUT}" \
      | sed -n "${COMMENT_START_LINE},${COMMENT_END_LINE}p" \
      | sed '1d;$d' > "${COMMENT_FILE}"
    if [ ! -s "${COMMENT_FILE}" ]; then
      echo "ERROR: Debate output missing PR comment body." >&2
      exit 1
    fi
    grep -Eq '^\*\*Local arbitration result: DISMISSED\.\*\*$' "${COMMENT_FILE}" || { echo "ERROR: Debate output missing a DISMISSED arbitration result heading." >&2; exit 1; }
    grep -Eq '^## Participants$' "${COMMENT_FILE}" || { echo "ERROR: Debate output missing Participants section." >&2; exit 1; }
    grep -Eq '^## Bot Concern$' "${COMMENT_FILE}" || { echo "ERROR: Debate output missing Bot Concern section." >&2; exit 1; }
    grep -Eq '^## Debate Summary$' "${COMMENT_FILE}" || { echo "ERROR: Debate output missing Debate Summary section." >&2; exit 1; }
    grep -Eq '^## Conclusion$' "${COMMENT_FILE}" || { echo "ERROR: Debate output missing Conclusion section." >&2; exit 1; }
    grep -Eq '^CSA session ID:' "${COMMENT_FILE}" || { echo "ERROR: Debate output missing CSA session ID." >&2; exit 1; }
    gh pr comment "${PR_NUM}" --repo "${REPO}" --body-file "${COMMENT_FILE}"
    rm -f "${COMMENT_FILE}"
    echo "CSA_VAR:AUDIT_TRAIL_POSTED=true"
    ;;
  CONFIRMED)
    echo "CSA_VAR:AUDIT_TRAIL_POSTED=false"
    echo "CSA_VAR:COMMENT_IS_FALSE_POSITIVE=false"
    ;;
  *)
    echo "ERROR: Debate output missing a supported VERDICT marker." >&2
    exit 1
    ;;
esac
```

## ELSE

<!-- COMMENT_IS_STALE check is enforced via step conditions in workflow.toml -->

## Step 9: Fix Real Issue

> **Layer**: 1 (CSA executor) -- Layer 0 dispatches fix to CSA sub-agent.
> CSA reads code, applies fix, commits. Orchestrator verifies result.

Tool: csa
Tier: tier-2-standard
OnFail: retry 2

Fix the real issue for the current bot review comment (non-stale,
non-false-positive). Fetch the comment body via
`gh api repos/${REPO}/pulls/comments/${CURRENT_COMMENT_ID}`, apply the fix,
and commit it.

## ENDIF

## Step 10: Push Fixes and Continue Loop

> **Layer**: 0 (Orchestrator) -- shell commands to push fixes and continue loop.

Tool: bash

Track iteration count via `REVIEW_ROUND`. Check the round cap BEFORE
pushing fixes and continuing to the next review loop iteration. When `REVIEW_ROUND`
reaches `MAX_REVIEW_ROUNDS` (default: 10), STOP and present options to
the user — no new review is triggered until the user decides:

- **Option A**: Merge now (review is good enough)
- **Option B**: Continue for `MAX_REVIEW_ROUNDS` more rounds
- **Option C**: Abort and investigate manually

The workflow MUST NOT auto-merge or auto-abort at the round limit.
The user MUST explicitly choose an option before proceeding.

**Orchestrator protocol**: When the round cap is hit, the bash block exits
with code 0 after printing `ROUND_LIMIT_HALT`. The orchestrator (Layer 0)
MUST then use `AskUserQuestion` to present options A/B/C and collect the
user's choice. Based on the answer, set `ROUND_LIMIT_ACTION` and re-enter
this step. The action handler at the TOP of the script processes the user's
choice BEFORE the round cap check, so the chosen action always takes effect:
- **A**: Set `ROUND_LIMIT_ACTION=merge` → clears `ROUND_LIMIT_REACHED`, prints `ROUND_LIMIT_MERGE`, exits 0. Orchestrator routes to Step 12/12b.
- **B**: Set `ROUND_LIMIT_ACTION=continue` → clears `ROUND_LIMIT_REACHED`, extends `MAX_REVIEW_ROUNDS`, falls through to push loop.
- **C**: Set `ROUND_LIMIT_ACTION=abort` → leaves `ROUND_LIMIT_REACHED=true`, prints `ROUND_LIMIT_ABORT`, exits 1.

**CRITICAL**: The `merge` and `continue` branches MUST clear `ROUND_LIMIT_REACHED=false`
before proceeding. Steps 11 and 12 are gated by `!(${ROUND_LIMIT_REACHED})`,
so a stale `true` value blocks all downstream merge/rebase paths even after the user
explicitly chose to proceed. The `abort` branch intentionally leaves the flag set,
as it halts the workflow.

**Signal disambiguation**: The orchestrator distinguishes re-entry outcomes by
output markers, NOT exit codes alone. `ROUND_LIMIT_HALT` (exit 0) = ask user.
`ROUND_LIMIT_MERGE` (exit 0) = proceed to merge. `ROUND_LIMIT_ABORT` (exit 1) = stop.

```bash
REVIEW_ROUND=$((REVIEW_ROUND + 1))
MAX_REVIEW_ROUNDS="${MAX_REVIEW_ROUNDS:-10}"
echo "CSA_VAR:REVIEW_ROUND=$REVIEW_ROUND"
echo "CSA_VAR:MAX_REVIEW_ROUNDS=$MAX_REVIEW_ROUNDS"

# --- Handle orchestrator re-entry with user decision (FIRST) ---
# When the orchestrator re-enters after collecting user choice via
# AskUserQuestion, ROUND_LIMIT_ACTION is set. Process it BEFORE the round
# cap check so the user's choice always takes effect regardless of round count.
#
# CRITICAL: The merge path prints ROUND_LIMIT_MERGE (distinct from
# ROUND_LIMIT_HALT) so the orchestrator can unambiguously route to Step 12/12b.
# The abort path exits non-zero. The continue path falls through to push loop.
if [ -n "${ROUND_LIMIT_ACTION}" ]; then
  case "${ROUND_LIMIT_ACTION}" in
    merge)
      echo "User chose: Merge now. Pushing local commits before merge."
      ROUND_LIMIT_REACHED=false  # Clear so Steps 10.5/11/12 are unblocked
      # Push any Category C fixes from Step 9 so remote HEAD includes them.
      # Without this, gh pr merge merges the stale remote head.
      git push origin "${WORKFLOW_BRANCH}"
      echo "CSA_VAR:ROUND_LIMIT_REACHED=$ROUND_LIMIT_REACHED"
      echo "CSA_VAR:ROUND_LIMIT_ACTION="
      echo "ROUND_LIMIT_MERGE: Routing to merge step."
      # Orchestrator MUST route to Step 12/12b upon seeing ROUND_LIMIT_MERGE.
      # Distinct from ROUND_LIMIT_HALT — this is an affirmative merge decision.
      exit 0
      ;;
    continue)
      echo "User chose: Continue. Extending by ${MAX_REVIEW_ROUNDS} rounds."
      ROUND_LIMIT_REACHED=false  # Clear so review loop and downstream steps are unblocked
      MAX_REVIEW_ROUNDS=$((REVIEW_ROUND + MAX_REVIEW_ROUNDS))
      unset ROUND_LIMIT_ACTION
      echo "CSA_VAR:ROUND_LIMIT_REACHED=$ROUND_LIMIT_REACHED"
      echo "CSA_VAR:MAX_REVIEW_ROUNDS=$MAX_REVIEW_ROUNDS"
      echo "CSA_VAR:ROUND_LIMIT_ACTION="
      # Fall through to push loop below (bypasses round cap check)
      ;;
    abort)
      echo "User chose: Abort workflow."
      echo "ROUND_LIMIT_ABORT: Workflow terminated by user."
      exit 1
      ;;
  esac
fi

# --- Round cap check BEFORE push/next-loop ---
# This block ONLY fires when ROUND_LIMIT_ACTION is unset (first hit, or after
# continue already extended the cap). When ROUND_LIMIT_ACTION was set, the case
# block above already handled it and either exited or fell through past this check.
if [ "${REVIEW_ROUND}" -ge "${MAX_REVIEW_ROUNDS}" ]; then
  ROUND_LIMIT_REACHED=true
  echo "Reached MAX_REVIEW_ROUNDS (${MAX_REVIEW_ROUNDS})."
  echo "Options:"
  echo "  A) Merge now (review is good enough)"
  echo "  B) Continue for ${MAX_REVIEW_ROUNDS} more rounds"
  echo "  C) Abort and investigate manually"
  echo ""
  echo "CSA_VAR:ROUND_LIMIT_REACHED=$ROUND_LIMIT_REACHED"
  echo "ROUND_LIMIT_HALT: Awaiting user decision."
  # HALT: The orchestrator MUST use AskUserQuestion to collect user's choice.
  # The shell script block ENDS here. The orchestrator handles routing based on
  # the user's answer OUTSIDE this script block. This ensures non-interactive
  # execution environments (CSA sub-agents) do not hang on stdin.
  #
  # Orchestrator routing logic (executed at Layer 0, NOT in this bash block):
  #   User answers "A" → set ROUND_LIMIT_ACTION=merge, re-enter this step
  #   User answers "B" → set ROUND_LIMIT_ACTION=continue, re-enter this step
  #   User answers "C" → set ROUND_LIMIT_ACTION=abort, re-enter this step
  #
  # FORBIDDEN: Falling through to push loop without a user decision.
  exit 0  # Yield control to orchestrator for AskUserQuestion
fi

# --- Push fixes only (next trigger happens in Step 5) ---
git push origin "${WORKFLOW_BRANCH}"
ROUND_LIMIT_REACHED=false
echo "CSA_VAR:ROUND_LIMIT_REACHED=$ROUND_LIMIT_REACHED"
echo "CSA_VAR:REVIEW_ROUND=$REVIEW_ROUND"
echo "CSA_VAR:MAX_REVIEW_ROUNDS=$MAX_REVIEW_ROUNDS"
```

Loop back to Step 5 (delegated wait gate).

## Step 10b: Post-Fix Re-Review Gate (HARD GATE)

After fixing bot findings, verify the bot has actually re-reviewed the current
HEAD and found zero actionable findings before any merge path can execute.

This is a **deterministic hard gate** — it prevents the linear workflow
from falling through to merge without re-verification. The "Loop back
to Step 5" above is guidance for LLM orchestrators but is NOT enforced
by the workflow engine (`csa plan run` executes steps linearly).

**Positive confirmation signal** (#505): The gate checks for a **review event**
(via `pulls/{pr}/reviews` API) with `submitted_at` > push timestamp, NOT
merely the absence of inline comments. This distinguishes "bot re-reviewed
and found nothing" from "bot hasn't re-reviewed yet."

The gate:
1. Records push timestamp before checking
2. Polls for a review event from `${CLOUD_BOT_LOGIN}` with `submitted_at` > push time
3. If review event found AND has P0/P1/P2 inline comments → **abort** (user must re-run pr-bot)
4. If review event found AND clean → clears `BOT_HAS_ISSUES=false` so merge steps can proceed
5. If no review event within timeout → falls back to local `csa review --range main...HEAD`

## ELSE

## Step 10a: Bot Review Clean

No issues found by bot. Proceed to merge.

## Step 10.5: Rebase for Clean History (DISABLED)

> **Layer**: 0 (Orchestrator) -- git history cleanup before merge.
> **Status**: Disabled. With `--merge` (not `--squash`), rebase destroys the
> per-commit audit trail instead of cleaning it up. Set `REBASE_ENABLED=true`
> to re-enable for squash-merge workflows.

Tool: bash

Reorganize accumulated fix commits into logical groups (source, patterns, other)
before merging. Skip if <= 3 commits.

After rebase: backup branch, soft reset to merge-base, dynamic file grouping,
force-push with lease, trigger final `@${CLOUD_BOT_NAME} review`, then delegate the long
wait/fix/review loop to a single CSA-managed step.

**Post-rebase review gate** (BLOCKING):
- CSA delegated step handles both paths:
  - Bot responds with P0/P1/P2 badges → CSA runs bounded fix/review retries (max 3 rounds), using the same 15-minute wait policy for each trigger (5-minute quiet wait + 10-minute polling).
  - Bot times out → CSA runs fallback `csa review --range main...HEAD` and bounded fix/review retries (max 3 rounds).
- command-level hard timeout is enforced for the delegated gate (`timeout 5400s`).
- if `timeout/gtimeout` is unavailable, a built-in watchdog fallback still enforces the same timeout bound.
- delegated execution failures are hard failures (no `|| true` silent downgrade).
- On delegated gate failure (timeout, non-zero, or non-PASS marker), set `REBASE_REVIEW_HAS_ISSUES=true` (and `FALLBACK_REVIEW_HAS_ISSUES=true` when appropriate), then block merge.
- On success, both `REBASE_REVIEW_HAS_ISSUES` and `FALLBACK_REVIEW_HAS_ISSUES` must be false.

```bash
set -euo pipefail
TIMEOUT_BIN="$(command -v timeout || command -v gtimeout || true)"
run_with_hard_timeout() {
  local timeout_secs="$1"
  shift
  if [ -n "${TIMEOUT_BIN}" ]; then
    "${TIMEOUT_BIN}" -k 5s "${timeout_secs}s" "$@" 2>&1
    return $?
  fi

  local tmp_out timeout_flag child_pid watcher_pid rc use_pgroup
  tmp_out="$(mktemp)"
  timeout_flag="$(mktemp)"
  rm -f "${timeout_flag}"
  use_pgroup=false
  if command -v setsid >/dev/null 2>&1; then
    setsid "$@" >"${tmp_out}" 2>&1 &
    use_pgroup=true
    child_pid=$!
  else
    "$@" >"${tmp_out}" 2>&1 &
    child_pid=$!
  fi
  (
    sleep "${timeout_secs}"
    if kill -0 "${child_pid}" 2>/dev/null; then
      : >"${timeout_flag}"
      if [ "${use_pgroup}" = "true" ]; then
        kill -TERM "-${child_pid}" 2>/dev/null || true
      else
        kill -TERM "${child_pid}" 2>/dev/null || true
      fi
      sleep 2
      if kill -0 "${child_pid}" 2>/dev/null; then
        if [ "${use_pgroup}" = "true" ]; then
          kill -KILL "-${child_pid}" 2>/dev/null || true
        else
          kill -KILL "${child_pid}" 2>/dev/null || true
        fi
      fi
    fi
  ) &
  watcher_pid=$!
  wait "${child_pid}"
  rc=$?
  kill "${watcher_pid}" 2>/dev/null || true
  cat "${tmp_out}"
  rm -f "${tmp_out}"
  if [ -f "${timeout_flag}" ]; then
    rm -f "${timeout_flag}"
    return 124
  fi
  rm -f "${timeout_flag}"
  return "${rc}"
}

COMMIT_COUNT=$(git rev-list --count main..HEAD)
if [ "${COMMIT_COUNT}" -gt 3 ]; then
  git branch -f "backup-${PR_NUM}-pre-rebase" HEAD

  MERGE_BASE=$(git merge-base main HEAD)
  git reset --soft $MERGE_BASE

  git reset HEAD .
  git diff --name-only -z HEAD | { grep -zE '^(src/|crates/|lib/|bin/)' || true; } | xargs -0 --no-run-if-empty git add --
  if ! git diff --cached --quiet; then
    git commit -m "feat(scope): primary implementation changes"
  fi

  git diff --name-only -z HEAD | { grep -zE '^(patterns/|\.claude/)' || true; } | xargs -0 --no-run-if-empty git add --
  if ! git diff --cached --quiet; then
    git commit -m "fix(scope): pattern and skill updates"
  fi

  git add -A
  if ! git diff --cached --quiet; then
    git commit -m "chore(scope): config and documentation updates"
  fi

  NEW_COMMIT_COUNT=$(git rev-list --count ${MERGE_BASE}..HEAD)
  if [ "${NEW_COMMIT_COUNT}" -eq 0 ]; then
    echo "ERROR: No replacement commits after soft reset. Restoring backup."
    git reset --hard "backup-${PR_NUM}-pre-rebase"
    exit 1
  fi

  git push --force-with-lease
  REBASE_CURRENT_SHA="$(git rev-parse HEAD)"
  REBASE_TRIGGER_TS="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  REBASE_TRIGGER_BODY="@${CLOUD_BOT_NAME} review

<!-- csa-trigger:${REBASE_CURRENT_SHA}:${REBASE_TRIGGER_TS} -->"
  gh pr comment "${PR_NUM}" --repo "${REPO}" --body "${REBASE_TRIGGER_BODY}"

  set +e
  GATE_RESULT_FILE="$(mktemp)"
  run_with_hard_timeout 5400 csa run --force-ignore-tier-setting --tool auto --idle-timeout 5400 "Bounded post-rebase gate task only. Do NOT invoke pr-bot skill or any full PR workflow. Operate on PR #${PR_NUM} in repo ${REPO} (branch ${WORKFLOW_BRANCH}). Complete the post-rebase review gate end-to-end. For each @codex trigger, wait 5 minutes quietly, then poll up to 10 minutes for a response. If response contains P0/P1/P2 findings, iteratively fix/commit/push/re-trigger and re-check with the same 15-minute wait policy (max 3 rounds). If bot times out, run csa review --range main...HEAD and execute a max-3-round fix/review cycle; leave an audit-trail PR comment whenever timeout fallback path is used; return exactly one marker line REBASE_GATE=PASS when clean, otherwise REBASE_GATE=FAIL and exit non-zero." | tee "${GATE_RESULT_FILE}"
  GATE_RC=${PIPESTATUS[0]}
  set -e
  GATE_RESULT="$(cat "${GATE_RESULT_FILE}")"
  rm -f "${GATE_RESULT_FILE}"
  if [ "${GATE_RC}" -eq 124 ]; then
    REBASE_REVIEW_HAS_ISSUES=true
    FALLBACK_REVIEW_HAS_ISSUES=true
    echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=$REBASE_REVIEW_HAS_ISSUES"
    echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
    echo "ERROR: Post-rebase delegated gate exceeded hard timeout (5400s)." >&2
    exit 1
  fi
  if [ "${GATE_RC}" -ne 0 ]; then
    REBASE_REVIEW_HAS_ISSUES=true
    FALLBACK_REVIEW_HAS_ISSUES=true
    echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=$REBASE_REVIEW_HAS_ISSUES"
    echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
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
    echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=$REBASE_REVIEW_HAS_ISSUES"
    echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
    echo "ERROR: Post-rebase review gate failed."
    exit 1
  fi

  BOT_SETTLE_SECS="${BOT_SETTLE_SECS:-20}"
  sleep "${BOT_SETTLE_SECS}"
  set +e
  LATE_ACTIONABLE_COUNT="$(
    gh api "repos/${REPO}/pulls/${PR_NUM}/comments?per_page=100" \
      --jq '[.[] | select(.user.login == "'"${CLOUD_BOT_LOGIN}"'") | select(.created_at > "'"${REBASE_TRIGGER_TS}"'") | select((.body | test("P0|P1|P2"))) ] | length' \
      2>/dev/null
  )"
  LATE_ACTIONABLE_RC=$?
  set -e
  if [ "${LATE_ACTIONABLE_RC}" -ne 0 ]; then
    REBASE_REVIEW_HAS_ISSUES=true
    FALLBACK_REVIEW_HAS_ISSUES=true
    echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=$REBASE_REVIEW_HAS_ISSUES"
    echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
    echo "ERROR: Failed to query post-rebase actionable bot comments (rc=${LATE_ACTIONABLE_RC})." >&2
    exit 1
  fi
  case "${LATE_ACTIONABLE_COUNT:-}" in
    ''|*[!0-9]*)
      REBASE_REVIEW_HAS_ISSUES=true
      FALLBACK_REVIEW_HAS_ISSUES=true
      echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=$REBASE_REVIEW_HAS_ISSUES"
      echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
      echo "ERROR: Invalid post-rebase actionable comment count from GitHub API: '${LATE_ACTIONABLE_COUNT}'." >&2
      exit 1
      ;;
  esac
  if [ "${LATE_ACTIONABLE_COUNT}" -gt 0 ]; then
    REBASE_REVIEW_HAS_ISSUES=true
    FALLBACK_REVIEW_HAS_ISSUES=true
    echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=$REBASE_REVIEW_HAS_ISSUES"
    echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
    echo "ERROR: Detected ${LATE_ACTIONABLE_COUNT} actionable bot comment(s) after post-rebase trigger window." >&2
    exit 1
  fi

  REBASE_REVIEW_HAS_ISSUES=false
  FALLBACK_REVIEW_HAS_ISSUES=false
  echo "CSA_VAR:REBASE_REVIEW_HAS_ISSUES=$REBASE_REVIEW_HAS_ISSUES"
  echo "CSA_VAR:FALLBACK_REVIEW_HAS_ISSUES=$FALLBACK_REVIEW_HAS_ISSUES"
  git push origin "${WORKFLOW_BRANCH}"
fi
```

## ENDIF

## ENDIF

## ENDIF
<!-- End of CLOUD_BOT != "false" block -->

## IF !(${BOT_UNAVAILABLE})

## IF ${FIXES_ACCUMULATED}

## Step 11: Clean Resubmission (if needed)

> **Layer**: 0 (Orchestrator) -- git branch management, no code reading.

Tool: bash

If fixes accumulated, create clean branch for final review.

```bash
CLEAN_BRANCH="${WORKFLOW_BRANCH}-clean"
git checkout -b "${CLEAN_BRANCH}"
git push -u origin "${CLEAN_BRANCH}"
gh pr create --base main --head "${CLEAN_BRANCH}" --title "${PR_TITLE}" --body "${PR_BODY}"
```

## Step 12: Final Merge

> **Layer**: 0 (Orchestrator) -- final merge command, no code analysis.

Tool: bash
OnFail: abort

Merge and update local main.

```bash
# --- Hard gate: unconditional pre-merge check ---
if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved fallback review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi
if [ "${REBASE_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved post-rebase review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi

git push origin "${WORKFLOW_BRANCH}"
MERGE_STRATEGY=$(csa config get pr_review.merge_strategy --default merge)
DELETE_BRANCH_FLAG=""
if [ "$(csa config get pr_review.delete_branch --default false)" = "true" ]; then
  DELETE_BRANCH_FLAG="--delete-branch"
fi
# shellcheck disable=SC2086
gh pr merge "${WORKFLOW_BRANCH}-clean" --repo "${REPO}" --"${MERGE_STRATEGY}" ${DELETE_BRANCH_FLAG}

# Post-merge: sync local main with remote
git fetch origin
git checkout main
git merge origin/main --ff-only
git log --oneline -1  # verify local matches remote
```

## ELSE

## Step 12b: Final Merge (Direct)

> **Layer**: 0 (Orchestrator) -- direct merge, no code analysis needed.

Tool: bash
OnFail: abort

First-pass clean review: merge the existing PR directly.

```bash
# --- Hard gate: unconditional pre-merge check ---
if [ "${FALLBACK_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved fallback review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi
if [ "${REBASE_REVIEW_HAS_ISSUES}" = "true" ]; then
  echo "ERROR: Reached merge with unresolved post-rebase review issues."
  echo "This is a workflow violation. Aborting merge."
  exit 1
fi

git push origin "${WORKFLOW_BRANCH}"
MERGE_STRATEGY=$(csa config get pr_review.merge_strategy --default merge)
DELETE_BRANCH_FLAG=""
if [ "$(csa config get pr_review.delete_branch --default false)" = "true" ]; then
  DELETE_BRANCH_FLAG="--delete-branch"
fi
# shellcheck disable=SC2086
gh pr merge "${PR_NUM}" --repo "${REPO}" --"${MERGE_STRATEGY}" ${DELETE_BRANCH_FLAG}

# Post-merge: sync local main with remote
git fetch origin
git checkout main
git merge origin/main --ff-only
git log --oneline -1  # verify local matches remote
```

## ENDIF

## ENDIF
