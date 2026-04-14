---
name: commit
description: "Use when: committing code with security audit, tests, review gates"
allowed-tools: Bash, Read, Grep, Glob, Edit
triggers:
  - "commit"
  - "/commit"
  - "audited commit"
---

# Commit: Audited Commit Workflow

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the commit skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `../../PATTERN.md` relative to this `SKILL.md`, and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Enforce "Commit = Audited" discipline: every commit passes branch check, formatting, linting, tests, security audit, and heterogeneous code review before being created. Includes automatic PR creation when a logical milestone is reached, with pr-bot integration for cloud review.

## Execution Protocol (ORCHESTRATOR ONLY)

<prompt-guard name="hook-bypass-forbidden">
ABSOLUTE PROHIBITION on ALL hook bypass methods. You MUST NOT:
- Use `--no-verify` or `-n` with `git commit` or `git push`
- Set `LEFTHOOK=0` environment variable (e.g., `env LEFTHOOK=0 git commit`, `export LEFTHOOK=0`)
- Set `LEFTHOOK_SKIP` environment variable
- Modify `.git/hooks/*` files to disable or weaken hooks
- Set `core.hooksPath` to an empty or permissive directory
- Use ANY other mechanism that prevents Lefthook/pre-commit hooks from running

All quality hooks MUST be allowed to run. Bypassing hooks is a critical SOP violation.

When `just pre-commit` fails:
1. Code quality issues (clippy, fmt, test) → FIX the code, do NOT bypass
2. Environment/sandbox limitations → report status="needs_clarification" with exact error, do NOT bypass
3. Pre-existing failures from unrelated crates → report as blocker with exact error, do NOT bypass
NEVER treat pre-existing failures as justification for LEFTHOOK=0.
</prompt-guard>

### Prerequisites

- Must be on a feature branch (not `main` or `dev`)

### Quick Start

```bash
csa run --sa-mode true --skill commit "Commit the current changes with scope: <scope>"
```

### SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands. Omitting `--sa-mode`
at root depth causes a hard error; passing `false` when the caller is in SA mode
breaks prompt-guard propagation.

### Step-by-Step

1. **Branch check**: Verify on a feature branch (not main/dev). Abort if protected.
2. **Quality gates**: Run `just fmt`, `just clippy`, `just test` sequentially. Fix any failures.
3. **Stage changes**: `git add` relevant files. Verify no untracked files remain.
4. **Security scan**: Grep staged files for hardcoded secrets (API_KEY, SECRET, PASSWORD, PRIVATE_KEY).
5. **Security audit**: Invoke the `security-audit` pattern via CSA -- three-phase audit (test completeness, vulnerability scan, code quality).
6. **Pre-commit review**: Invoke the `ai-reviewed-commit` pattern via CSA -- authorship-aware review (debate for self-authored, `csa review --diff --allow-fallback` for others). Fix-and-retry up to 3 rounds.
7. **Generate commit message**: Delegate to CSA at `tier-1-quick` (tool and thinking budget come from config). The commit body MUST include the AI Reviewer Metadata block from `Commit Message Format (AI Era)`. If a review session already ran in this workflow, prefer resuming it with `--session <review-session-id>` (reuses cached context, near-zero new tokens). When resuming, keep the same tool (sessions are tool-locked).
8. **Commit**: `git commit -m "${COMMIT_MSG}"`.
9. **Auto PR** (standalone by default): Push branch, create PR targeting main, invoke `/pr-bot`.
   Runs automatically when commit is standalone. Skipped when parent workflow
   (mktsk/dev2merge) sets `CSA_SKIP_PUBLISH=true`, or automatically when
   invoked from inside a CSA session (`CSA_DEPTH` set and non-zero) so that
   employee sessions stay orchestration-pure and only the Layer 0 orchestrator
   runs the PR + pr-bot transaction (#752 Bug 4).
   - **NOTE**: `/pr-bot` internally runs a **separate cumulative review** (`csa review --range main...HEAD`) covering ALL commits on the branch before push. This is distinct from Step 6's per-commit review (`csa review --diff`). Do NOT skip pr-bot's internal review even if Step 6 passed.

## Two-Layer Review Architecture

| Layer | Command | Scope | Timing |
|-------|---------|-------|--------|
| Per-commit | `csa review --diff` | Staged changes only | Before `git commit` (Step 6) |
| Pre-PR cumulative | `csa review --range main...HEAD` | Full feature branch | Before `git push` (inside `/pr-bot` Step 2) |

Both layers are mandatory. The per-commit review catches issues in each individual change; the cumulative review catches cross-commit issues and ensures the full branch is coherent.

## Commit Message Format (AI Era)

All commits created by this workflow must use:

```text
<type>(<scope>): <subject>

<Description of what changed>

### AI Reviewer Metadata
- **Design Intent**: <Why this change was made, what problem it solves. Context not obvious from the diff.>
- **Key Decisions**: <Significant architectural or implementation choices made during the task.>
- **Reviewer Guidance**: List areas needing careful review, with REQUIRED sub-fields:
  - **Timing/Race Scenarios**: any timing-sensitive ordering, concurrency race, file-system race, or async ordering the change must survive. Cite the scenario + the test that exercises it.
  - **Boundary Cases**: null/empty/max/min/off-by-one inputs, and the test(s) that cover them.
  - **Risk Areas**: the parts of the change that, if wrong, would not be caught by compile + unit tests.
```

## Example Usage

| Command | Effect |
|---------|--------|
| `/commit` | Commit current staged changes with full audit pipeline |
| `/commit scope=executor` | Commit with explicit scope for commit message |

## Integration

- **Depends on**: `security-audit` (Step 5), `ai-reviewed-commit` (Step 6)
- **Triggers**: `pr-bot` (Step 9, when milestone)
- **Used by**: `mktsk` (as commit step after each implementation task), `dev2merge`, `dev-to-merge` (legacy alias)

## Done Criteria

1. Branch is a feature branch (not main/dev).
2. `just fmt`, `just clippy`, `just test` all exit 0.
3. No untracked files remain after staging.
4. Security scan found no hardcoded secrets.
5. Security audit returned PASS or PASS_DEFERRED.
6. Pre-commit review completed with zero unresolved P0/P1 issues.
7. Commit created with Conventional Commits format AND includes AI Reviewer Metadata block.
8. `git status` shows clean working tree.
9. PR created and `/pr-bot` invoked (skipped when `CSA_SKIP_PUBLISH=true`).
