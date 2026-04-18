---
name = "ai-reviewed-commit"
description = "Pre-commit code review loop: stage → size check → csa review → fix → re-review → commit"
allowed-tools = "Bash, Task, Read, Edit"
tier = "tier-2-standard"
version = "0.1.0"
---

# AI-Reviewed Commit

Ensures all code is reviewed by csa review before committing.
The weave workflow is a **single-pass mechanical gate**: it can run the initial review, dispatch one fix/re-stage/re-review pass, and stop cleanly when the hard-cap gate requires user direction.
Multi-round fix-and-re-review for rounds 2 and 3 is **not implemented by native weave looping**. It is driven by the LLM orchestrator following the companion `SKILL.md` contract.
Fix-and-retry up to **3 rounds (hard cap)**. After round 3, if review still reports non-false-positive P0/P1 findings, STOP and ask the user whether to continue. Exception: if the user's prior prompt explicitly authorized unbounded looping (e.g., "loop until clean", "keep fixing until review passes"), continue without asking. Also continue without asking if all round-3 findings are false positives per orchestrator judgement. Those hard-cap and exception rules are binding on the orchestrator; the weave workflow provides the round-1 halt gate as a deterministic backup.
Because this pattern invokes the downstream `commit` skill, the AI reviewer should verify the
`Reviewer Guidance` schema required there, including `Timing/Race Scenarios`, `Boundary Cases`,
and `Regression Tests Added`.

### Variables

- `${FILES}`: Space-separated list of files to stage.
- `${FIXED_FILES}`: Space-separated list of files to re-stage after fixes.
- `${REVIEW_HAS_ISSUES}`: `"true"` when the latest review still has blocking issues.
- `${SELF_AUTHORED}`: `"true"` when the staged diff was authored in the current session.
- `${SID}`: Session ID returned by the review/debate/commit-message sub-command.
- `${REVIEW_ROUND}`: Current review round number (defaults to `1` for the initial review).
- `${MAX_REVIEW_ROUNDS}`: Hard cap for review rounds (defaults to `3`).
- `${UNBOUNDED_LOOP_AUTHORIZED}`: `"true"` only when the user explicitly authorized unbounded looping.
- `${ROUND_3_FALSE_POSITIVES_ONLY}`: `"true"` only when round-3 findings are all judged false positives by the orchestrator.
- `${USER_DECISION_REQUIRED}`: `"true"` when the hard cap was hit and the workflow must stop for user direction.

## Step 1: Stage Changes

Tool: bash

Initialize and declare workflow variables.

- `${FILES}`: Space-separated list of files to stage.
- `${FIXED_FILES}`: Space-separated list of files to re-stage after fixes.
- `${REVIEW_HAS_ISSUES}`: `"true"` when the latest review still has blocking issues.
- `${REVIEW_ROUND}`: Current review round number (defaults to `1` for the initial review).
- `${MAX_REVIEW_ROUNDS}`: Hard cap for review rounds (defaults to `3`).
- `${UNBOUNDED_LOOP_AUTHORIZED}`: `"true"` only when the user explicitly authorized unbounded looping.
- `${ROUND_3_FALSE_POSITIVES_ONLY}`: `"true"` only when round-3 findings are all judged false positives by the orchestrator.
- `${USER_DECISION_REQUIRED}`: `"true"` when the hard cap was hit and the workflow must stop for user direction.

```bash
: "${FILES}" "${FIXED_FILES}" "${REVIEW_HAS_ISSUES}" "${REVIEW_ROUND}" "${MAX_REVIEW_ROUNDS}" "${UNBOUNDED_LOOP_AUTHORIZED}" "${ROUND_3_FALSE_POSITIVES_ONLY}" "${USER_DECISION_REQUIRED}"
echo "CSA_VAR:REVIEW_ROUND=${REVIEW_ROUND:-1}"
echo "CSA_VAR:MAX_REVIEW_ROUNDS=${MAX_REVIEW_ROUNDS:-3}"
echo "CSA_VAR:UNBOUNDED_LOOP_AUTHORIZED=${UNBOUNDED_LOOP_AUTHORIZED:-false}"
echo "CSA_VAR:ROUND_3_FALSE_POSITIVES_ONLY=${ROUND_3_FALSE_POSITIVES_ONLY:-false}"
echo "CSA_VAR:USER_DECISION_REQUIRED=${USER_DECISION_REQUIRED:-false}"
git add ${FILES}
```

## Step 2: Size Check

Tool: bash
OnFail: abort

Check staged diff size. If >= 500 lines, consider splitting.

```bash
git diff --stat --staged
```

## Step 3: Authorship-Aware Review Strategy

Determine who authored the staged code:
- Self-authored (generated in this session) → use csa debate
- Other tool/human authored → use csa review --diff --allow-fallback

## IF ${SELF_AUTHORED}

## Step 4a: Run Debate Review

Tool: bash

```bash
SID=$(csa debate "Review my staged changes for correctness, security, and test gaps. Run 'git diff --staged' yourself to see the full patch.")
bash scripts/csa/session-wait-until-done.sh "$SID"
```

## ELSE

## Step 4b: Run CSA Review

Tool: bash

```bash
SID=$(csa review --diff --allow-fallback)
bash scripts/csa/session-wait-until-done.sh "$SID"
```

## ENDIF

## IF ${REVIEW_HAS_ISSUES}

## Step 5: Dispatch Fix Sub-Agent

Tool: claude-code
Tier: tier-2-standard
OnFail: retry 3

Dispatch sub-agent to fix issues found in review.
Preserve original code intent. Do NOT delete code to silence warnings.

## Step 6: Re-stage Fixed Files

Tool: bash

```bash
git add ${FIXED_FILES}
```

## Step 7: Round Cap Check

Tool: bash

Enforce the 3-round hard cap before triggering the next review attempt inside this single-pass workflow.
If round 3 still has non-false-positive P0/P1 findings, stop and require user direction.
Continue without asking only when `${UNBOUNDED_LOOP_AUTHORIZED}` is `"true"` or
`${ROUND_3_FALSE_POSITIVES_ONLY}` is `"true"`.

```bash
REVIEW_ROUND="${REVIEW_ROUND:-1}"
MAX_REVIEW_ROUNDS="${MAX_REVIEW_ROUNDS:-3}"
UNBOUNDED_LOOP_AUTHORIZED="${UNBOUNDED_LOOP_AUTHORIZED:-false}"
ROUND_3_FALSE_POSITIVES_ONLY="${ROUND_3_FALSE_POSITIVES_ONLY:-false}"

if [ "${REVIEW_ROUND}" -ge "${MAX_REVIEW_ROUNDS}" ] && \
   [ "${UNBOUNDED_LOOP_AUTHORIZED}" != "true" ] && \
   [ "${ROUND_3_FALSE_POSITIVES_ONLY}" != "true" ]; then
  echo "CSA_VAR:USER_DECISION_REQUIRED=true"
  echo "Reached the hard cap of ${MAX_REVIEW_ROUNDS} review rounds."
  echo "User decision required: continue fix-and-review loop?"
  exit 0
fi

echo "CSA_VAR:USER_DECISION_REQUIRED=false"
echo "CSA_VAR:REVIEW_ROUND=$((REVIEW_ROUND + 1))"
```

## ENDIF

## Step 8: Re-review

Tool: bash
Condition: ${REVIEW_HAS_ISSUES} && !(${USER_DECISION_REQUIRED})

Run the next review only if the hard-cap check allowed it.
This is still a single-pass workflow step, not a native weave loop.

```bash
SID=$(csa review --diff --allow-fallback)
bash scripts/csa/session-wait-until-done.sh "$SID"
```

## Step 9: AGENTS.md Compliance Check
Condition: !(${USER_DECISION_REQUIRED})

The review MUST include AGENTS.md compliance checklist:
- Discover AGENTS.md chain (root-to-leaf) for each staged file
- Check every applicable rule
- If the staged diff or generated commit body lists concrete `Timing/Race Scenarios`, verify that
  matching regression tests exist and are named under `Regression Tests Added`. Missing or
  mismatched tests are a blocking review failure.
- If staged diff touches `PATTERN.md` or `workflow.toml`, MUST check rule 027 `pattern-workflow-sync`
- If staged diff touches process spawning/lifecycle code, MUST check Rust rule 015 `subprocess-lifecycle`
- Zero unchecked items before proceeding to commit
- Skip this and all later post-review steps when `${USER_DECISION_REQUIRED}` is `"true"` so the workflow halts cleanly instead of committing past the cap

## Step 10: Generate Commit Message

Tool: csa
Tier: tier-1-quick
Condition: !(${USER_DECISION_REQUIRED})
OnFail: abort

Run 'git diff --staged' and generate a Conventional Commits message.
Output ONLY the commit message, nothing else.

## Step 11: Commit

Tool: bash
OnFail: abort
Condition: !(${USER_DECISION_REQUIRED})

Skip when `${USER_DECISION_REQUIRED}` is `"true"` so the workflow never commits after the hard-cap gate asked for user direction.

```bash
git commit -m "${COMMIT_MSG}"
```
