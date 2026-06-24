---
name: dev2merge
description: "Use when: full dev cycle branch->plan->implement->review->PR->merge"
allowed-tools: Bash, Read, Grep, Glob, Edit, Write
triggers:
  - "dev2merge"
  - "/dev2merge"
  - "dev-to-merge"
  - "/dev-to-merge"
  - "full dev cycle"
  - "implement and merge"
---

# Dev2Merge: Deterministic Development Pipeline

## Role Detection (READ THIS FIRST -- MANDATORY)

Role MUST be determined by explicit mode marker, not fragile natural-language substring matching.
Treat the run as executor ONLY when initial prompt contains:
`<skill-mode>executor</skill-mode>`.

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `../../PATTERN.md` relative to this `SKILL.md`, and follow it step by step.
3. **RECURSION GUARD**: Do NOT run `csa run --skill dev2merge` or `csa run --skill dev-to-merge` from inside this skill. Other `csa` commands required by the workflow (for example `csa run --skill mktd`, `csa review`, `csa debate`) are allowed. mktsk MUST be invoked directly (not via `csa run`) — see Step 8.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Execute the complete development lifecycle as a **deterministic weave workflow**.
Every stage has hard gates (`on_fail = "abort"`) — no step can be skipped by the LLM.

Pipeline: Branch Validation → FAST_PATH Detection → L1/L2 Quality Gates →
(FAST_PATH: commit → bump → review) or (Full: mktd → mktsk [direct, TaskCreate] → bump → cumulative review) →
Push Gate → Pre-PR Verdict Check → PR Creation → pr-bot Hard Gate → Post-Merge Sync.

## Execution Protocol (ORCHESTRATOR ONLY)

<prompt-guard name="hook-bypass-forbidden">
ABSOLUTE PROHIBITION (#1123): All hook-bypass primitives are FORBIDDEN. Each disables registered git hooks (pre-commit / pre-push / commit-msg) and silently violates SOP:

- `git commit --no-verify` / `-n`
- `git push --no-verify`
- `LEFTHOOK=0`, `LEFTHOOK_DISABLED=1`
- `HUSKY=0`, `HUSKY_DISABLE=1`
- `SKIP_HOOKS=1`, `SKIP_GIT_HOOKS=1`, `PRE_COMMIT_ALLOW_NO_CONFIG=1` (when used to skip)
- `--no-gpg-sign` (signing bypass)
- ANY equivalent env var or CLI flag that disables a registered hook

If hooks fail, fix the underlying code issue. If `git commit` fails because lefthook (or another formatter hook) auto-formatted and re-staged files, the correct re-stage recovery primitive is:

1. `git diff --staged --quiet` -- exit 0 means staging is clean (rare; normally files were re-staged)
2. `git add -u` -- re-stage the formatted versions of already-tracked files
3. Retry `git commit -m "..."` -- hooks accept the formatted version on the second pass
4. If the recovery loop runs >=3 iterations without converging, surface `recovery_loop_exhausted` to the orchestrator. DO NOT escalate to any bypass primitive above.

Bypassing hooks is a critical SOP violation. If you encounter this scenario, follow the re-stage recipe above. NEVER use any of the FORBIDDEN bypass primitives.
</prompt-guard>

<prompt-guard name="squash-merge-forbidden">
ABSOLUTE PROHIBITION (#1122): Squash-merge primitives are FORBIDDEN at every level of dev2merge. Each destroys per-commit AI Reviewer Metadata, lefthook-gate evidence, author attribution (codex / gemini / main-agent split), and the iteration trail (review-then-fix rounds become invisible).

- `gh pr merge --squash`
- `gh pr merge -s` (short form)
- `git merge --squash`
- GitHub Web UI "Squash and merge" button
- ANY `--squash` flag passed to a merge command

If an explicitly approved emergency path requires a manual merge after a
recorded pr-bot pass, use `csa merge <PR_NUMBER>` rather than raw
`gh pr merge` so the deterministic local pr-bot gate still runs.

dev2merge delegates the actual merge to pr-bot (Step 16). pr-bot reads `pr_review.merge_strategy` from config (default `merge`). If a normal `gh pr merge --merge` fails (e.g. lefthook re-stage race producing an empty-diff PR, or upstream advancing during the wait), DO NOT escalate to `--squash`. Surface `merge_blocked` (or the structural variant `merge_blocked_empty_diff`) to the orchestrator.

EMPTY-DIFF GUARD: Before any merge, verify `gh pr diff <PR>` is non-empty. An empty-diff PR is the structural fingerprint of the lefthook-race scenario in #1122 -- the branch tip drifted, the PR body still references the intended fix, but the actual diff vs the default branch is empty. Aborting at the empty-diff signal is the correct behavior. Squash-merging an empty-diff PR produces an empty squash commit on the default branch and corrupts the audit trail; this is the exact bug #1122 documents.

Once squashed, the original commits cannot be reconstructed from the default branch. The audit cost is irreversible and silent.
</prompt-guard>

### Prerequisites

- Must be on a feature branch (not the default branch or `dev`)

### Quick Start

```bash
csa plan run patterns/dev2merge/workflow.toml
```

To drive the pipeline from a specific GitHub issue, pass `--issue <N>`. CSA
fetches the issue body (using the project's configured `GH_CONFIG_DIR`, falling
back to default `gh` auth on a permission/not-found error) and injects it as the
`FEATURE_INPUT` workflow variable that the dev2merge planning step consumes:

```bash
csa plan run patterns/dev2merge/workflow.toml --issue 1638
```

`--issue <N>` is mutually exclusive with an explicit `--var FEATURE_INPUT=...`:
supply only one source for the workflow's `FEATURE_INPUT`. (The flag is generic
to `csa plan run` — it replaces the removed dev2merge `--issue` subcommand and
works with any pattern that reads `FEATURE_INPUT`.)

### Resume Mode (tail-only)

When the implementation already exists and is committed on the feature branch
(e.g. a writer agent finished the code), run the **tail only** — review → push →
PR → merge → sync — without re-planning:

```bash
csa plan run patterns/dev2merge/workflow.toml --var DEV2MERGE_MODE=resume
```

`DEV2MERGE_MODE` defaults to `full` (the complete pipeline). `resume` skips
only planning (Step 7 mktd, Step 8 mktsk); Step 9 (Resume Commit) commits any
uncommitted remainder after the L2 test gate, then **every hard gate still
runs**: version bump, self-review, cumulative review, push, verdict check, PR
creation, pr-bot, and post-merge sync. Resume fails closed when the branch has
no work (clean tree with 0 commits ahead of the default branch). A docs-only
resume run still follows the FAST_PATH branch.

Or invoke as a skill:

```bash
csa run --sa-mode true --skill dev2merge "Implement, review, and merge <scope description>"
```

### SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands.

### Review/Debate Waiting Discipline (MANDATORY)

When a pipeline step requires review or debate, use the built-in command for the
matching intent:

- Review step -> `csa review`
- Debate step -> `csa debate`

Do NOT replace these with a hand-written `csa run` prompt unless the built-in
command is blocked by a concrete, documented error.

In slow Rust repositories, one healthy review/debate session taking 30-60
minutes is normal. Sparse early output or a `csa session wait` timeout is not
failure by itself.

If the original session is still healthy, keep waiting on the same session id.
Do NOT launch narrowed or duplicate review/debate sessions for the same scope
unless there is explicit crash/error evidence, persistent liveness failure, or
direct user instruction.

## While awaiting review/fix session

This is the while-waiting checklist. When you background a `csa session wait` via `run_in_background: true`, the next task-notification wakes you up automatically. Do not add sleep, hand-rolled polling, a redundant `ScheduleWakeup`, or `/loop` on top.

**Safe parallel work**:
1. Draft the PR body or changelog entry for the current branch as local text only; do not run `gh pr create` yet.
2. For deferred MEDIUM findings from prior rounds, queue issue-template drafts locally and batch filing later when the review cluster is clear.
3. Read the next sprint task or issue to preload context for the next non-conflicting step.
4. Check existing issues for possible duplicate-of candidates for findings already queued.
5. Clean up stale TaskCreate or TaskUpdate entries.

**Do NOT**:
- Start new `csa run` or `csa review` sessions that could race on git branch or checkout state with the waiting one (single-checkout sequential rule, AGENTS.md 028).
- Edit source files while the main agent is acting as the Layer 0 orchestrator; that violates the SA-mode separation this wait is protecting.
- Run state-mutating git commands such as `git commit`, `git checkout <other-branch>`, or `git push`.
- Stack a ScheduleWakeup or /loop backup on top of the backgrounded wait; the task-notification is already the wake signal (AGENTS.md 042f / 046).

If there is no useful parallel work available, return control and wait for the notification. Do not invent speculative work just to stay busy.

### Pipeline Steps

The workflow is compiled from the companion `../../PATTERN.md` file (relative to this `SKILL.md`) into `workflow.toml`.
All steps use `on_fail = "abort"`. Variables propagate via `CSA_VAR:KEY=value`.

| Step | Name | Gate | Tool |
|------|------|------|------|
| 1 | Validate Branch | Not default branch/dev | bash |
| 2 | FAST_PATH Detection | Diff-stat heuristic | bash |
| 3 | L1/L2 Quality Gates | language-aware lint/test gate | bash |
| **IF FAST_PATH** | | | |
| 4 | Simplified Commit | `just test && git commit` | bash |
| 5 | Version Bump | optional local Just version recipes; missing recipes skip | bash |
| 6 | Pre-PR Review | cumulative review helper runs `csa review --range` and owns exact-head verdict check when review runs | bash |
| **ELSE (Full Pipeline)** | | | |
| 7 | Plan with mktd | `csa plan run --pattern mktd` by default, `MKTD_WORKFLOW_PATH` override allowed (skipped in resume mode) | bash |
| 8 | Execute with mktsk | Follow mktsk PATTERN.md directly, TaskCreate/TaskUpdate (skipped in resume mode) | main agent |
| 9 | Resume Commit | resume mode only: L2 tests + commit uncommitted remainder | bash |
| 10 | Version Bump | optional local Just version recipes; missing recipes skip | bash |
| 11 | Self-Review Gate | Main agent checks and fixes the full branch diff before CSA review | main agent |
| 12 | Pre-PR Cumulative Review Gate | cumulative review helper runs `csa review --range ${DEFAULT_BRANCH}...HEAD` and owns exact-head verdict check when review runs | bash |
| **ENDIF** | | | |
| 13 | Push Gate | `REVIEW_COMPLETED=true` required | bash |
| 14 | Pre-PR Review Verdict Check | accepts `REVIEW_COMPLETED=true`; falls back to `csa review --check-verdict --range ${DEFAULT_BRANCH}...HEAD` on resume gaps | bash |
| 15 | Create or Reuse PR | `gh pr create` or reuse existing, outputs `PR_NUMBER`/`PR_URL` | bash |
| 16 | pr-bot Hard Gate | **MANDATORY** — runs pr-bot (review + merge) | bash |
| 17 | Post-Merge Sync | Verifies PR MERGED, then checkout and fast-forward the default branch | bash |

Steps 14-17 form the PR transaction. Step 14 verifies the pre-PR review
completion marker before any PR can be created, and falls back to an exact-head
verdict check when the marker is missing. Step 15 creates the PR, Step 16 is a **hard gate**
that runs pr-bot (which performs cloud review and the actual merge). Step 17
verifies the PR reached MERGED state before syncing — this is defense in depth against
a skipped Step 16. Marker files provide idempotency in Step 16.

### FAST_PATH Heuristic

Changes are classified as FAST_PATH when:
- Only `.md`, `.txt`, `.lock`, `.toml` files changed (no code files)
- Total insertions < 100 lines

FAST_PATH skips mktd/mktsk/debate but **keeps** L1/L2 quality checks and pre-PR review.

### Physical Enforcement

A git pre-push hook at `scripts/hooks/pre-push` verifies that a `csa review` session
exists for the current HEAD before allowing push. Install:

```bash
ln -sf ../../scripts/hooks/pre-push .git/hooks/pre-push
```

## Example Usage

| Command | Effect |
|---------|--------|
| `/dev2merge` | Full deterministic pipeline for current branch |
| `/dev2merge scope="executor refactor"` | Pipeline with scope hint for mktd |
| `/dev-to-merge` | Backward-compatible alias |

## Integration

- **Composes**: `mktd` (planning + debate), `mktsk` (serial task execution), `commit` (per-task), `pr-bot` (review loop + merge)
- **Enforced by**: weave workflow compiler (`on_fail = "abort"`), git pre-push hook
- **Standalone**: Complete workflow — does not need other skills invoked separately

## Done Criteria

1. Feature branch validated (not default branch/dev).
2. FAST_PATH detection completed (heuristic applied).
3. Language-aware L1/L2 gates exit 0.
4. If full pipeline: mktd plan saved with `DONE WHEN` clauses, mktsk executed all tasks via main agent.
5. If FAST_PATH: simplified commit created with tests passing.
6. Rust repos with local version recipes bump if needed; non-Rust repos or repos missing those optional recipes skip without aborting.
7. Pre-PR cumulative review helper either accepted a documented batch skip or passed the exact-head verdict check before setting `REVIEW_COMPLETED=true`.
8. Push completed via `--force-with-lease` (pre-push hook verified review HEAD).
9. Pre-PR review completion check passed (`REVIEW_COMPLETED=true`, or fallback `csa review --check-verdict --range ${DEFAULT_BRANCH}...HEAD` on resume gaps).
10. PR created or reused on GitHub targeting the default branch, `PR_NUMBER` and `PR_URL` resolved.
11. pr-bot hard gate completed: either triggered `pr-bot` or detected an already-completed run for the same PR/HEAD.
12. PR state verified as MERGED (defense in depth against skipped Step 16).
13. Local default branch synced from its remote tracking branch.
14. Feature branch cleaned up.
