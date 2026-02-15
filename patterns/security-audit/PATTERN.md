---
name = "security-audit"
description = "Pre-commit security audit with test completeness verification and adversarial vulnerability scan"
allowed-tools = "Read, Grep, Glob, Bash"
tier = "tier-2-standard"
version = "0.1.0"
---

# Security Audit

Three-phase adversarial audit: test completeness, vulnerability scan, code
quality. The auditor adopts an attacker mindset — find problems, not confirm
correctness.

## Step 1: Discover Changed Files

Tool: bash
OnFail: abort

Identify all files modified in the current changeset.

```bash
git diff --cached --name-only || git diff HEAD --name-only
```

## Step 2: Assess Module Size

Tool: bash

Count tokens to decide whether to audit locally or delegate to CSA.
If > 19200 lines total, delegate to CSA for analysis.

```bash
git diff --cached --name-only | xargs wc -l 2>/dev/null | tail -1
```

## IF ${NEEDS_CSA_DELEGATION}

## Step 3a: Delegate Audit to CSA

Tool: csa
Tier: tier-2-standard
OnFail: abort

Module too large for local audit. Delegate entire audit to CSA.

```bash
csa run "Perform security audit following security-audit skill protocol.
         Review changed files and associated tests.
         Three phases: test completeness, vulnerability scan, code quality.
         Output structured audit report."
```

## ELSE

## Step 3b: Discover Tests

Tool: bash

For each changed source file, locate associated test files.
Search same file (#[cfg(test)]), same directory (*_test.rs, test_*.rs),
and tests/ directory.

```bash
git diff --cached --name-only | while read -r f; do
  dir=$(dirname "$f")
  base=$(basename "$f" .rs)
  echo "=== Tests for $f ==="
  grep -l "mod tests" "$f" 2>/dev/null
  ls "${dir}/${base}_test"* "${dir}/test_${base}"* 2>/dev/null
  find tests/ -name "*.rs" -exec grep -l "$base" {} \; 2>/dev/null
done
```

## Step 4: Phase 1 — Test Completeness Check

Tool: claude-code
Tier: tier-2-standard
OnFail: abort

For each public function in changed files:
- Does it have tests for normal path?
- Does it have tests for edge cases (empty, max, boundary)?
- Does it have tests for error conditions?
- Key question: "Can you propose a test case that doesn't exist?"
  YES → FAIL. NO → PASS.

## Step 5: Phase 2 — Security Vulnerability Scan

Tool: claude-code
Tier: tier-2-standard
OnFail: abort

For each function handling external input:
- Input validation present?
- Size/length limits enforced?
- Can malformed input cause panic? (unwrap, expect, index access)
- Can crafted input cause resource exhaustion?
- Timing attack vectors (constant-time comparison for secrets)?
- Integer overflow (checked/saturating arithmetic)?

## Step 6: Phase 3 — Code Quality Check

Tool: claude-code
Tier: tier-2-standard

Check for:
- No debug code (println!, dbg!, console.log)
- No hardcoded secrets (API_KEY, PASSWORD, PRIVATE_KEY)
- No commented-out code
- No TODO/FIXME security items
- Error handling complete (no unwrap on untrusted input)

## ENDIF

## Step 7: Generate Audit Report

Tool: claude-code

Produce structured report with tables for each phase:
- Phase 1: Function | Normal | Edge | Error | Missing Tests
- Phase 2: Severity | Location | Issue | Recommendation
- Phase 3: Issue | Location | Fix

Final verdict: PASS | PASS_DEFERRED | FAIL

## IF ${VERDICT_IS_FAIL}

## Step 8: Report Blocking Issues

OnFail: abort

List all blocking issues. Commit is rejected until resolved.

## ENDIF

## IF ${VERDICT_IS_PASS_DEFERRED}

## Step 9: Record Deferred Issues

Record deferred issues (in other modules) via Task tools for
immediate post-commit fixing. Priority: Critical > High > Medium.

## ENDIF
