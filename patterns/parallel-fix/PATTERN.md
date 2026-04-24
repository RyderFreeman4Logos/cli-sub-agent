---
name = "parallel-fix"
description = "Parallel RECON / serial EDIT for multi-finding fix rounds"
allowed-tools = "Bash, Read, Grep, Glob"
tier = "tier-2-standard"
version = "0.1.0"
---

# Parallel Fix: Multi-Finding RECON/EDIT Split

Optimizes fix rounds when a review produces multiple independent findings.
Read-only analysis (RECON) runs in parallel; code edits (EDIT) run serially.

## When to Use

Use this pattern when ALL of these conditions are met:
1. A `csa review` session produced `output/findings.toml` with **2+ findings**.
2. Findings affect **different primary files** (non-overlapping `file_ranges`).
3. The orchestrator is in fix mode (either commit Step 5 or review-loop Step 4).

When findings share a primary file, they must be grouped into the same bucket
and analyzed together — their fixes may interact.

## Step 1: Parse Findings

Tool: bash

Read the review session's `output/findings.toml` and extract per-finding metadata.

```bash
REVIEW_SESSION_DIR=$(csa session result --session "${REVIEW_SID}" --field session_dir 2>/dev/null)
FINDINGS_TOML="${REVIEW_SESSION_DIR}/output/findings.toml"

if [ ! -f "$FINDINGS_TOML" ]; then
  echo "ERROR: findings.toml not found at $FINDINGS_TOML"
  exit 1
fi

cat "$FINDINGS_TOML"
```

## Step 2: Bucket Findings by Primary File

Tool: bash

Group findings by their primary file path (`file_ranges[0].path`).
Findings sharing a primary file go into the same bucket.
Each bucket gets an independent RECON employee.

The bucketing must produce:
- `${BUCKET_COUNT}`: Number of independent buckets.
- `${BUCKET_N_IDS}`: Finding IDs in bucket N (space-separated).
- `${BUCKET_N_FILE}`: Primary file for bucket N.

If `${BUCKET_COUNT}` == 1, skip parallel RECON — fall back to standard
single-employee fix (no benefit from parallelism).

## Step 3: Parallel RECON Phase

Tool: bash
Condition: ${BUCKET_COUNT} > 1

Dispatch one read-only CSA employee per bucket **concurrently**.
Each employee receives:
- The finding(s) from its bucket (IDs, severity, description, file ranges).
- The current diff context (`git diff --staged` or `git diff`).
- A read-only mandate: analyze the finding, identify root cause, propose
  a fix plan with specific file:line edits and test additions.

Each employee writes its fix-plan to stdout as a structured artifact:

```
<!-- FIX-PLAN:START -->
## Finding: {finding_id}
### Root Cause
{analysis}
### Proposed Fix
{file:line changes}
### Test Plan
{new/modified tests}
<!-- FIX-PLAN:END -->
```

**Dispatch pattern** (one per bucket, all launched before any wait):

```bash
# Launch all RECON employees concurrently
SID_1=$(csa run --sa-mode true --tier tier-1-quick \
  --description "recon-fix: ${BUCKET_1_FILE}" \
  "Analyze finding ${BUCKET_1_IDS} in ${BUCKET_1_FILE}. ...")
SID_2=$(csa run --sa-mode true --tier tier-1-quick \
  --description "recon-fix: ${BUCKET_2_FILE}" \
  "Analyze finding ${BUCKET_2_IDS} in ${BUCKET_2_FILE}. ...")
# ... repeat for each bucket

# Wait for all RECON employees sequentially (AGENTS.md rule: no parallel waits)
csa session wait --session "$SID_1"
csa session wait --session "$SID_2"
# ... repeat for each bucket
```

RECON employees are read-only — they MUST NOT edit files, run git commands
that modify state, or create commits. They analyze and plan only.

## Step 4: Merge Fix Plans

Tool: bash

Collect fix-plan artifacts from all RECON sessions.
Detect conflicts: if two plans propose edits to the same file:line range,
flag as conflicting and merge into a single sequential fix instruction.

Produce a merged fix-plan document ordering fixes by:
1. Severity (critical first).
2. File path (alphabetical within same severity).

## Step 5: Serial EDIT Phase

Tool: bash

Dispatch a **single** CSA employee with write permissions to apply all
fix plans serially. The employee receives the merged fix-plan and must:

1. Apply each fix in order.
2. Run `just fmt` after all fixes.
3. Stage changed files.
4. Do NOT commit — the caller (commit skill) handles commit creation.

```bash
EDIT_SID=$(csa run --sa-mode true --tier tier-2-standard \
  --description "apply-fixes: ${BUCKET_COUNT} findings" \
  "Apply the following fix plans to the codebase. Apply each fix in order.
   After all fixes, run 'just fmt' and stage the changed files.
   Do NOT create any commits.

   ${MERGED_FIX_PLAN}")
csa session wait --session "$EDIT_SID"
```

## Step 6: Verify Fixes

Tool: bash
OnFail: skip

Run quality gates after the EDIT phase completes.

```bash
just fmt
just clippy
just test
```

## Variables

- `${REVIEW_SID}`: Session ID of the review that produced findings.
- `${BUCKET_COUNT}`: Number of independent finding buckets.
- `${BUCKET_N_IDS}`: Finding IDs in bucket N.
- `${BUCKET_N_FILE}`: Primary file for bucket N.
- `${MERGED_FIX_PLAN}`: Merged fix-plan document from all RECON outputs.
- `${EDIT_SID}`: Session ID of the EDIT employee.
