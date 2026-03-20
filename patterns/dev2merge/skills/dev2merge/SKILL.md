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
3. **RECURSION GUARD**: Do NOT run `csa run --skill dev2merge` or `csa run --skill dev-to-merge` from inside this skill. Other `csa` commands required by the workflow (for example `csa run --skill mktd`, `csa run --skill mktsk`, `csa review`, `csa debate`) are allowed.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Execute the complete development lifecycle as a **deterministic weave workflow**.
Every stage has hard gates (`on_fail = "abort"`) — no step can be skipped by the LLM.

Pipeline: Branch Validation → FAST_PATH Detection → L1/L2 Quality Gates →
(FAST_PATH: commit → bump → review) or (Full: mktd → orchestrator TaskCreate → mktsk → bump → cumulative review) →
Push Gate → PR Transaction (create/reuse PR + inline pr-codex-bot trigger) → Local Sync.

## Execution Protocol (ORCHESTRATOR ONLY)

<prompt-guard name="no-verify-forbidden">
ABSOLUTE PROHIBITION: You MUST NOT use `--no-verify` or `-n` with any `git commit` or `git push` command. All quality hooks (pre-commit, etc.) MUST be allowed to run. Bypassing hooks is a critical SOP violation. If hooks fail, fix the underlying code issues instead of bypassing.
</prompt-guard>

### Prerequisites

- Must be on a feature branch (not `main` or `dev`)

### Quick Start

```bash
csa plan run patterns/dev2merge/workflow.toml
```

Or invoke as a skill:

```bash
csa run --sa-mode true --skill dev2merge "Implement, review, and merge <scope description>"
```

### SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands.

### Pipeline Steps

The workflow is compiled from the companion `../../PATTERN.md` file (relative to this `SKILL.md`) into `workflow.toml`.
All steps use `on_fail = "abort"`. Variables propagate via `CSA_VAR:KEY=value`.

| Step | Name | Gate | Tool |
|------|------|------|------|
| 1 | Validate Branch | Not main/dev | bash |
| 2 | FAST_PATH Detection | Diff-stat heuristic | bash |
| 3 | L1/L2 Quality Gates | `just fmt && just clippy` | bash |
| **IF FAST_PATH** | | | |
| 4 | Simplified Commit | `just test && git commit` | bash |
| 5 | Version Bump | `just bump-patch` if needed | bash |
| 6 | Pre-PR Review | `csa review --range` | bash |
| **ELSE (Full Pipeline)** | | | |
| 7 | Plan with mktd | `csa plan run patterns/mktd/workflow.toml` | bash |
| 8 | Orchestrator Task Registration | Orchestrator calls TaskCreate for each TODO item | orchestrator |
| 9 | Execute with mktsk | `csa run --skill mktsk` | bash |
| 10 | Version Bump | `just bump-patch` if needed | bash |
| 11 | Cumulative Review | `csa review --range main...HEAD` | bash |
| **ENDIF** | | | |
| 12 | Push Gate | `REVIEW_COMPLETED=true` required | bash |
| 13 | Create PR Transaction | `gh pr create` or reuse existing, then inline pr-codex-bot trigger | bash |
| 14 | Post-Merge Sync | `git checkout main && git merge --ff-only` | bash |

Step 12 is intentionally a single self-contained shell transaction. The pr-codex-bot
trigger logic is inlined directly in the workflow step (no external hook scripts required).
Marker files provide idempotency — if the bot already ran for the same PR/HEAD, it skips.

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

- **Composes**: `mktd` (planning + debate), `mktsk` (serial task execution), `commit` (per-task), `pr-codex-bot` (review loop + merge)
- **Enforced by**: weave workflow compiler (`on_fail = "abort"`), git pre-push hook
- **Standalone**: Complete workflow — does not need other skills invoked separately

## Done Criteria

1. Feature branch validated (not main/dev).
2. FAST_PATH detection completed (heuristic applied).
3. `just fmt` and `just clippy` exit 0 (L1/L2 gates).
4. If full pipeline: mktd plan saved with `DONE WHEN` clauses, mktsk executed all tasks.
5. If FAST_PATH: simplified commit created with tests passing.
6. Version bumped if needed.
7. Pre-PR cumulative review passed (`REVIEW_COMPLETED=true`).
8. Push completed via `--force-with-lease` (pre-push hook verified review HEAD).
9. PR transaction completed: PR created or reused on GitHub targeting main, pr-codex-bot triggered inline.
10. That transaction either triggered `pr-codex-bot` or detected an already-completed run for the same PR/HEAD and skipped.
11. Local main synced after the PR merge completed: `git fetch origin && git checkout main && git merge origin/main --ff-only`.
12. Feature branch deleted (local and remote).
