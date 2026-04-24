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

Read the review session's findings and extract per-finding metadata.

```bash
# Read structured review output (contains finding IDs, severity, file ranges)
csa session result --session "${REVIEW_SID}" --section details

# If the details section is unavailable, fall back to full output
csa session result --session "${REVIEW_SID}" --full
```

> **Note**: Session directories are managed by CSA and may reside in sandboxed
> temp paths. There is no `--field session_dir` flag on `csa session result`.
> Use `--section details` or `--full` to read finding metadata. If raw
> `findings.toml` file access is needed, resolve the session directory via
> `csa_session::get_session_dir()` internally.

## Step 2: Bucket Findings by Primary File

Tool: bash

Group findings by their primary file path (`file_ranges[0].path`).
Findings sharing a primary file go into the same bucket.
Findings with no `file_ranges` go into a catch-all bucket.
Each bucket gets an independent RECON employee.

The bucketing must produce:
- `${BUCKET_COUNT}`: Number of independent buckets (workflow variable).
- `${MULTI_BUCKET}`: Set to `"yes"` when `BUCKET_COUNT > 1`, empty string `""` otherwise (workflow variable). Used as the Step 3/5/6 condition.
- `${SINGLE_BUCKET}`: Set to `"yes"` when `BUCKET_COUNT == 1`, empty string `""` otherwise (workflow variable). Used as the Step 4 condition. Introduced because `plan_condition` only supports truthiness checks and `!(expr)` negation — bare `!VAR` is not valid syntax.
- `BUCKET_N_IDS`: Finding IDs in bucket N, space-separated (shell-local, N = 1..BUCKET_COUNT).
- `BUCKET_N_FILE`: Primary file for bucket N (shell-local, N = 1..BUCKET_COUNT).

> `BUCKET_N_IDS` and `BUCKET_N_FILE` are shell-local variables created dynamically
> during bucketing — they are NOT workflow template variables (the count N varies
> at runtime and cannot be statically declared in workflow.toml).

If `${BUCKET_COUNT}` == 1, skip parallel RECON — fall back to standard
single-employee fix (no benefit from parallelism).

## Step 3: Parallel RECON Phase

Tool: bash
Condition: ${MULTI_BUCKET}

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

# Wait for all RECON employees sequentially (AGENTS.md rules 026/032: no parallel waits)
csa session wait --session "$SID_1"
csa session wait --session "$SID_2"
# ... repeat for each bucket
```

RECON employees are read-only — they MUST NOT edit files, run git commands
that modify state, or create commits. They analyze and plan only.

## Step 4: Single-Bucket Fallback

Tool: bash
Condition: ${SINGLE_BUCKET}

Single-bucket path: only one file-bucket exists, so parallel RECON has
no benefit. Dispatch a standard single-employee fix session that
combines RECON + EDIT in one pass.

The employee receives all findings from the single bucket, analyzes
root causes, applies fixes, runs `just fmt`, and stages changed files.
Does NOT create commits — the caller (commit skill) handles commit creation.

```bash
EDIT_SID=$(csa run --sa-mode true --tier tier-2-standard \
  --description "fix: single-bucket ${BUCKET_1_FILE}" \
  "Analyze and fix the following findings in ${BUCKET_1_FILE}.
   Apply each fix in order. After all fixes, run 'just fmt' and
   stage the changed files. Do NOT create any commits.

   Findings: ${BUCKET_1_IDS}")
csa session wait --session "$EDIT_SID"
```

## Step 5: Merge Fix Plans

Tool: bash
Condition: ${MULTI_BUCKET}

Collect fix-plan artifacts from all RECON sessions.
Detect conflicts: if two plans propose edits to the same file:line range,
flag as conflicting and merge into a single sequential fix instruction.

Produce a merged fix-plan document ordering fixes by:
1. Severity (critical first).
2. File path (alphabetical within same severity).

## Step 6: Serial EDIT Phase

Tool: bash
Condition: ${MULTI_BUCKET}

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

## Step 7: Verify Fixes

Tool: bash
OnFail: skip

Run quality gates after the EDIT phase completes.

```bash
just fmt
just clippy
just test
```

## Variables

### Workflow template variables (declared in workflow.toml)

- `${REVIEW_SID}`: Session ID of the review that produced findings.
- `${BUCKET_COUNT}`: Number of independent finding buckets.
- `${MULTI_BUCKET}`: Boolean gate — `"yes"` when `BUCKET_COUNT > 1`, `""` otherwise.
- `${SINGLE_BUCKET}`: Boolean gate — `"yes"` when `BUCKET_COUNT == 1`, `""` otherwise.
- `${MERGED_FIX_PLAN}`: Merged fix-plan document from all RECON outputs.
- `${EDIT_SID}`: Session ID of the EDIT employee.

### Shell-local variables (created dynamically during bucketing, N = 1..BUCKET_COUNT)

- `BUCKET_N_IDS`: Finding IDs in bucket N (space-separated).
- `BUCKET_N_FILE`: Primary file for bucket N.
