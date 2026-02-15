---
name: security-audit
description: Three-phase pre-commit security audit with test completeness, vulnerability scan, and code quality check
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "security-audit"
  - "/security-audit"
  - "security audit"
  - "audit security"
---

# Security Audit: Adversarial Pre-Commit Audit

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the security-audit skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/security-audit/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Run a three-phase adversarial security audit on staged/changed files before committing. The auditor adopts an attacker mindset: find problems, not confirm correctness. Phases: test completeness verification, vulnerability scan, and code quality check. Returns PASS, PASS_DEFERRED, or FAIL verdict.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- Changes must be staged or committed (the audit targets `git diff --cached` or `git diff HEAD`)

### Quick Start

```bash
csa run --skill security-audit "Audit the staged changes for security issues"
```

### Step-by-Step

1. **Discover changed files**: `git diff --cached --name-only` (or `HEAD` for committed changes).
2. **Assess module size**: Count lines. If total > 19200 lines, delegate entire audit to CSA.
3. **Discover tests**: For each changed file, locate associated test files (inline `#[cfg(test)]`, `*_test.rs`, `tests/` directory).
4. **Phase 1 -- Test Completeness**: For each public function in changed files, verify tests exist for normal path, edge cases, and error conditions. Key question: "Can you propose a test case that doesn't exist?" YES = FAIL.
5. **Phase 2 -- Vulnerability Scan**: Check input validation, size limits, panic risks (unwrap/expect/index on untrusted input), resource exhaustion, timing attacks, integer overflow.
6. **Phase 3 -- Code Quality**: No debug code, no hardcoded secrets, no commented-out code, no TODO/FIXME security items, complete error handling.
7. **Generate audit report**: Structured tables for each phase with PASS/PASS_DEFERRED/FAIL verdict.

## Example Usage

| Command | Effect |
|---------|--------|
| `/security-audit` | Audit staged changes with all three phases |
| `/security-audit scope=src/executor/` | Audit only files in executor module |

## Integration

- **Used by**: `commit` (Step 7), `dev-to-merge` (Step 7)
- **May trigger**: Task creation for deferred issues (PASS_DEFERRED verdict)

## Done Criteria

1. All changed files identified and their test files discovered.
2. Phase 1 (test completeness) completed for every public function.
3. Phase 2 (vulnerability scan) completed for every function handling external input.
4. Phase 3 (code quality) checked for debug code, secrets, commented code.
5. Structured audit report generated with per-phase tables.
6. Final verdict is one of: PASS, PASS_DEFERRED, FAIL.
7. If FAIL: blocking issues listed. If PASS_DEFERRED: deferred issues recorded.
