---
name: ai-reviewed-commit
description: "Use when: committing with pre-commit review, authorship-aware fix-retry"
allowed-tools: Bash, Read, Grep, Glob, Edit
triggers:
  - "ai-reviewed-commit"
  - "/ai-reviewed-commit"
  - "reviewed commit"
  - "review before commit"
---

# AI-Reviewed Commit: Pre-Commit Review Loop

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the ai-reviewed-commit skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `../../PATTERN.md` relative to this `SKILL.md`, and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Ensure every commit is reviewed by an independent model before creation. Uses authorship-aware strategy: self-authored code gets adversarial debate review (`csa debate`), while other-authored code gets standard `csa review --diff --allow-fallback`. Includes an automated fix-and-retry loop with a **3-round hard cap**, mandatory AGENTS.md compliance checking, and Conventional Commits message generation.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- Changes must be staged: `git diff --staged` shows content

### Quick Start

```bash
csa run --sa-mode true --skill ai-reviewed-commit "Review and commit the staged changes"
```

### SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands. Omitting `--sa-mode`
at root depth causes a hard error; passing `false` when the caller is in SA mode
breaks prompt-guard propagation.

### Step-by-Step

1. **Stage changes**: `git add` the target files.
2. **Size check**: Run `git diff --stat --staged`. If >= 500 lines, consider splitting the commit.
3. **Authorship detection**: Determine if staged code is self-authored (generated in this session) or by another tool/human.
4. **Review**:
   - Self-authored: `csa debate "Review my staged changes for correctness, security, and test gaps. Run 'git diff --staged' yourself."`
   - Other-authored: `csa review --diff --allow-fallback`
5. **Fix loop** (if issues found): Dispatch sub-agent to fix issues, re-stage, re-review. Preserve original code intent -- do NOT delete code to silence warnings. Fix-and-retry up to **3 rounds (hard cap)**. After round 3, if review still reports non-false-positive P0/P1 findings, STOP and ask the user whether to continue. Exception: if the user's prior prompt explicitly authorized unbounded looping (e.g., "loop until clean", "keep fixing until review passes"), continue without asking. Also continue without asking if all round-3 findings are false positives per orchestrator judgement.
6. **AGENTS.md compliance**: Discover AGENTS.md chain for each staged file. Check every applicable rule. If the staged diff or generated commit body lists concrete `Timing/Race Scenarios`, verify that matching regression tests exist and are named under `Regression Tests Added`; missing or mismatched tests are a blocking failure. Zero unchecked items before proceeding.
7. **Generate commit message**: `csa run "Run 'git diff --staged' and generate a Conventional Commits message"` (tier-1).
8. **Commit**: `git commit -m "${COMMIT_MSG}"`.

## Example Usage

| Command | Effect |
|---------|--------|
| `/ai-reviewed-commit` | Review staged changes and commit |
| `/ai-reviewed-commit files=src/lib.rs` | Stage, review, and commit specific file |

## Integration

- **Uses**: `csa-review` (for other-authored code), `debate` (for self-authored code)
- **Used by**: `commit` (Step 8 pre-commit review)
- **Produces**: A single reviewed commit with Conventional Commits message

## Done Criteria

1. Changes staged and diff size checked.
2. Authorship-appropriate review strategy selected and executed.
3. All review issues fixed, or the workflow stopped at the 3-round hard cap and requested user direction before continuing.
4. AGENTS.md compliance checklist completed with zero unchecked items.
5. Conventional Commits message generated via CSA.
6. Commit created successfully.
7. `git status` confirms commit was created.
