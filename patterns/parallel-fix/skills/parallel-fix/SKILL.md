---
name: parallel-fix
description: "Use when: review found 2+ independent findings in different files, fix phase can parallelize RECON"
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "parallel-fix"
  - "/parallel-fix"
  - "parallel recon fix"
  - "multi-finding fix"
---

# Parallel Fix: Multi-Finding RECON/EDIT Split

## Purpose

Optimize fix rounds when a review produces multiple independent findings
in different files. The analysis (RECON) phase is read-only and can safely
parallelize. The edit (EDIT) phase remains serial to avoid git conflicts.

Typical savings: 3-5 minutes per fix round with 2-3 independent findings,
since RECON employees run concurrently instead of sequentially.

## When to Activate

This skill activates as an optimization within the commit/review-loop
fix phase. The orchestrator detects the multi-finding case automatically:

1. After `csa review` produces a FAIL verdict with 2+ findings.
2. Parse `output/findings.toml` from the review session.
3. Bucket findings by primary file (`file_ranges[0].path`).
4. If 2+ buckets exist → use parallel-fix instead of single-employee fix.
5. If 1 bucket → fall back to standard single-employee fix (no benefit).

## Execution Protocol (ORCHESTRATOR ONLY)

### SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands. Omitting `--sa-mode`
at root depth causes a hard error; passing `false` when the caller is in SA mode
breaks prompt-guard propagation.

### Step-by-Step

1. **Parse findings**: Read `$REVIEW_SESSION_DIR/output/findings.toml`.
2. **Bucket by primary file**: Group findings by `file_ranges[0].path`.
   Findings with no `file_ranges` go into a catch-all bucket.
3. **Check bucket count**: If only 1 bucket, fall back to standard fix.
4. **Launch parallel RECON**: For each bucket, dispatch a read-only CSA
   employee at `tier-1-quick`:
   ```bash
   SID_N=$(csa run --sa-mode true --tier tier-1-quick \
     --description "recon-fix: <primary_file>" \
     "You are a read-only analysis agent. Do NOT edit any files.

     Analyze the following review finding(s) and produce a fix-plan:

     Finding ID: <id>
     Severity: <severity>
     File: <primary_file>:<start_line>-<end_line>
     Description: <description>

     Instructions:
     1. Read the affected file(s) and surrounding context.
     2. Identify the root cause of each finding.
     3. Propose specific file:line edits to fix the issue.
     4. Propose test additions/modifications if applicable.
     5. Output your fix-plan between <!-- FIX-PLAN:START --> and <!-- FIX-PLAN:END --> markers.")
   ```
   Launch ALL employees before waiting for any.
5. **Wait for RECON employees**: Wait sequentially (no parallel waits,
   per AGENTS.md session-wait rules):
   ```bash
   csa session wait --session "$SID_1"
   csa session wait --session "$SID_2"
   ```
6. **Collect and merge fix-plans**: Read each RECON session's output.
   Extract content between `<!-- FIX-PLAN:START -->` and `<!-- FIX-PLAN:END -->`.
   Check for conflicts (two plans editing same file:line range).
   Order by severity (critical first), then file path.
7. **Serial EDIT**: Dispatch a single write-capable CSA employee at
   `tier-2-standard` with the merged fix-plan. The employee applies
   all fixes, runs `just fmt`, and stages files. No commits.
8. **Verify**: Run `just fmt && just clippy && just test`.

### Conflict Resolution

When two RECON plans propose edits to overlapping file:line ranges:
- If edits are complementary (different lines in same function), keep both.
- If edits conflict (same line, different change), prefer the higher-severity
  finding's fix. Note the conflict in the merged plan for the EDIT employee.

## While awaiting RECON/EDIT session

This is the while-waiting checklist. When you background a `csa session wait` via `run_in_background: true`, the next task-notification wakes you up automatically. Do not add manual sleep or polling on top.

**Safe parallel work**:
1. Draft the PR body or changelog entry for the current branch as local text only; do not run `gh pr create` yet.
2. For deferred MEDIUM findings from prior rounds, queue issue-template drafts locally and batch filing later when the review cluster is clear.
3. Read the next sprint task or issue to preload context for the next non-conflicting step.

**Do NOT**:
- Start new `csa run` or `csa review` sessions that could race on git branch or checkout state with the waiting one (single-checkout sequential rule, AGENTS.md 028).
- Edit source files while the main agent is acting as the Layer 0 orchestrator.
- Run state-mutating git commands such as `git commit`, `git checkout <other-branch>`, or `git push`.
- Stack a ScheduleWakeup backup on top of the backgrounded wait.

## Integration

- **Used by**: `commit` (Step 5 fix dispatch), `review-loop` (Step 4 fix issues),
  `ai-reviewed-commit` (Step 5 fix sub-agent)
- **Depends on**: `csa review` producing structured `output/findings.toml`
- **Constraint**: AGENTS.md rule 028 (no worktree) — serial EDIT is enforced

## Done Criteria

1. All RECON employees completed with fix-plan artifacts.
2. Fix-plans merged without unresolved conflicts.
3. Single EDIT employee applied all fixes.
4. `just fmt`, `just clippy`, `just test` pass after fixes.
5. Changed files staged (not committed).
