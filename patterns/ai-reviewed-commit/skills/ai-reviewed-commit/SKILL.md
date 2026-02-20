---
name: ai-reviewed-commit
description: Pre-commit code review loop with authorship-aware strategy, fix-and-retry, and AGENTS.md compliance
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
2. **Read the pattern** at `patterns/ai-reviewed-commit/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Ensure every commit is reviewed by an independent model before creation. Uses authorship-aware strategy: self-authored code gets adversarial debate review (`csa debate`), while other-authored code gets standard `csa review --diff --allow-fallback`. Includes automated fix-and-retry loop (max 3 rounds), mandatory AGENTS.md compliance checking, and Conventional Commits message generation.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- Changes must be staged: `git diff --staged` shows content

### Quick Start

```bash
csa run --skill ai-reviewed-commit "Review and commit the staged changes"
```

### Step-by-Step

1. **Stage changes**: `git add` the target files.
2. **Size check**: Run `git diff --stat --staged`. If >= 500 lines, consider splitting the commit.
3. **Authorship detection**: Determine if staged code is self-authored (generated in this session) or by another tool/human.
4. **Review**:
   - Self-authored: `csa debate "Review my staged changes for correctness, security, and test gaps. Run 'git diff --staged' yourself."`
   - Other-authored: `csa review --diff --allow-fallback`
5. **Fix loop** (if issues found, max 3 rounds): Dispatch sub-agent to fix issues, re-stage, re-review. Preserve original code intent -- do NOT delete code to silence warnings.
6. **AGENTS.md compliance**: Discover AGENTS.md chain for each staged file. Check every applicable rule. Zero unchecked items before proceeding.
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
3. All review issues fixed (or max 3 fix-review rounds exhausted).
4. AGENTS.md compliance checklist completed with zero unchecked items.
5. Conventional Commits message generated via CSA.
6. Commit created successfully.
7. `git status` confirms commit was created.
