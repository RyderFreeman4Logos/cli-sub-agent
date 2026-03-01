---
name = "csa-review"
description = "CSA-driven code review with independent model selection, session isolation, and structured outputs"
allowed-tools = "Bash, Read, Grep, Glob"
tier = "tier-3-complex"
version = "0.1.0"
---

# CSA Review: Independent Code Review Orchestration

Structured code review through CSA with session isolation,
independent model selection, and three-pass review protocol.

Inputs: scope (uncommitted|base:<branch>|commit:<sha>|range:<from>...<to>|files:<pathspec>),
mode (review-only|review-and-fix), security_mode (auto|on|off), tool (optional override),
context (optional TODO plan path for alignment checking).

## Step 1: Role Detection

Check initial prompt for "Use the csa-review skill" literal string.
If present → review agent mode (skip orchestration, execute review directly).
If invoked by user → orchestrator mode (follow steps below).
Review agents MUST NOT run csa commands (prevents recursion).

## Step 2: Determine Review Tool

Tool: bash

Read review tool from config. Auto mode: claude-code caller → codex reviewer,
codex caller → claude-code reviewer.

```bash
csa config get review.tool 2>/dev/null || echo "auto"
```

## IF ${SCOPE_IS_PRE_PR}

## Step 3: Auto-Detect TODO Plan

Tool: bash
OnFail: abort

For pre-PR reviews (scope main...HEAD), auto-detect the associated TODO plan.
FATAL if no TODO found — pre-PR reviews require alignment checking.

```bash
csa todo find --branch "$(git branch --show-current)"
```

## ENDIF

## Step 4: Build Review Prompt

Construct comprehensive review prompt. The review agent reads CLAUDE.md
and AGENTS.md autonomously — do NOT pre-read them here.

Review prompt instructs agent to:
1. Read project context (CLAUDE.md + AGENTS.md)
2. Collect diff for given scope
3. Three-pass review (discovery, evidence filtering, adversarial security)
4. AGENTS.md compliance checklist (root-to-leaf, all applicable rules), including:
   - Rule 027 `pattern-workflow-sync` when diff touches `PATTERN.md` or `workflow.toml`
   - Rust rule 015 `subprocess-lifecycle` when diff touches process spawning/lifecycle code
5. Generate review-findings.json and review-report.md
6. Parse `[project_profile: <value>]` metadata from the instruction and apply
   framework-aware review dimensions from `references/review-protocol.md`

## Step 5: Execute Review via CSA

Tool: csa
Tier: tier-3-complex

The review tool is resolved from `[review] tool` in config (Step 2).
If `review.tool = "auto"`, CSA selects a heterogeneous counterpart automatically.

### Fork-Based Review (Recommended for Post-Implementation)

For post-implementation review, fork the implementer's session to inherit full
implementation context. Note: `csa review` does not yet support `--fork-from`
directly. Use `csa run --fork-from` with a review prompt instead:

```bash
csa run --fork-from <impl-session-id> "Review the uncommitted changes: $(git diff)"
```

**Benefits**: The reviewer inherits the implementer's context (files read,
design decisions, constraints, rejected alternatives). This catches
inconsistencies between intent and implementation that a cold reviewer would
miss — the forked context knows what the code *should* do, so deviations
are immediately visible. Also saves tokens by avoiding redundant file reads.

> **Planned**: Native `csa review --fork-from` support is tracked for a future release.

### Standard Review (No Fork)

When no implementation session is available, use standard review:

```bash
csa run --tool ${REVIEW_TOOL} --description "code-review: ${SCOPE}" "${REVIEW_PROMPT}"
```

## Step 6: Present Results

Read and display review-report.md, review-findings.json summary,
AGENTS.md checklist summary. Report CSA session ID for follow-up.

## IF ${MODE_IS_REVIEW_AND_FIX}

## Step 7: Fix Mode

Tool: csa
Tier: tier-3-complex

Resume same CSA session to fix all P0 and P1 issues.
Generate fix-summary.md and post-fix-review-findings.json.
Mark remaining P0/P1 as incomplete.

```bash
csa run --tool ${REVIEW_TOOL} --session ${SESSION_ID} "${FIX_PROMPT}"
```

## ENDIF

## Step 8: Verification

Tool: bash
OnFail: skip

Optionally verify fixes pass quality gates.

```bash
just pre-commit
```
