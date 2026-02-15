---
name: code-review
description: GitHub PR code review via gh CLI with scale-adaptive strategy and AGENTS.md compliance
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "code-review"
  - "/code-review"
  - "review PR"
  - "PR review"
---

# Code Review: Scale-Adaptive GitHub PR Review

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the code-review skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/code-review/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Perform AI-powered code review on GitHub PRs with automatic scale adaptation. Small PRs (< 200 lines) are reviewed directly; large PRs (> 800 lines) are delegated to CSA. Includes authorship detection to use adversarial debate for self-authored code, and mandatory AGENTS.md compliance checking for all changed files.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- `gh` CLI MUST be authenticated: `gh auth status`
- PR number must be known or discoverable

### Quick Start

```bash
csa run --skill code-review "Review PR #<number>"
```

### Step-by-Step

1. **Fetch PR context**: Use `gh pr view` and `gh pr diff --stat` to get metadata and diff statistics.
2. **Assess scale**: Small (< 200 lines) = direct review. Medium (200-800) = direct with progress. Large (> 800) = delegate to `csa review`.
3. **Authorship check**: If Co-Authored-By matches caller model family, use `csa debate` instead of direct review.
4. **For large PRs**: Checkout PR branch locally, run `csa review --branch <base>`. Do NOT pre-read diff.
5. **For small/medium PRs**: Fetch full diff via `gh pr diff`, analyze each file for quality, security, performance, and language-specific issues.
6. **AGENTS.md compliance**: Discover AGENTS.md chain for each changed file. Check every applicable rule. Violations are P2+; MUST/CRITICAL rules are P1.
7. **Generate review**: Produce structured output (summary, critical issues, suggestions, nitpicks, questions, AGENTS.md checklist).
8. **Post review** (optional): Submit as PR comment via `gh pr comment` only when user explicitly requests.

## Example Usage

| Command | Effect |
|---------|--------|
| `/code-review 42` | Review PR #42 with auto scale detection |
| `/code-review 42 post=true` | Review and post comment to the PR |
| `/code-review scope=base:main` | Review current branch changes vs main |

## Integration

- **May use**: `csa-review` (for large PRs), `debate` (for self-authored code arbitration)
- **Used by**: `pr-codex-bot` (as part of PR review loop)

## Done Criteria

1. PR metadata and diff statistics fetched successfully.
2. Scale-appropriate review strategy selected and executed.
3. If large PR: delegated to CSA review (not read into main agent context).
4. If self-authored: adversarial debate review used instead of standard review.
5. AGENTS.md compliance checklist generated with all applicable rules checked.
6. Structured review output produced (summary, issues, suggestions, checklist).
7. If post=true: review comment posted to PR via `gh pr comment`.
