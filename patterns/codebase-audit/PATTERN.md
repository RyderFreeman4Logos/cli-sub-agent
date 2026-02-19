---
name = "codebase-audit"
description = "Bottom-up per-module security audit of AI-generated codebases with structured reports"
allowed-tools = "Bash, Read, Grep, Glob, Write"
tier = "tier-2-deep"
version = "0.1.0"
---

# Bottom-Up Codebase Security Audit

Systematic security audit of AI-generated codebases, processing modules in
topological order (leaf dependencies first). Each module receives a structured
audit report covering input validation, error handling, resource limits,
secrets, memory safety, and concurrency correctness. Prior module reports
serve as compressed context for cross-module analysis.

## Step 1: Initialize Audit State

Tool: bash
OnFail: abort

Initialize the `csa audit` manifest if it does not exist, or synchronize it
with the current file tree. This ensures every source file is tracked.

```bash
if [ ! -f ".csa/audit/manifest.toml" ]; then
  csa audit init
else
  csa audit sync
fi
```

## Step 2: Get Work Queue

Tool: bash
OnFail: abort

Retrieve the list of pending files in topological order. Leaf modules (those
with no internal dependencies) come first, ensuring that when a module is
audited, all of its dependencies have already been reviewed.

```bash
csa audit status --format json --order topo --filter pending
```

Parse the JSON output into an ordered list of file paths: `${WORK_QUEUE}`.

If the work queue is empty, report "All modules already audited" and stop.

## Step 3: Prepare Output Directory

Tool: bash

Create the mirror directory structure for audit reports. Reports are stored
under `./drafts/security-audit/` preserving the source file's relative path.

```bash
for file in ${WORK_QUEUE}; do
  mkdir -p "./drafts/security-audit/$(dirname "${file}")"
done
```

## FOR module IN ${WORK_QUEUE}

## Step 4: Load Prior Context

Tool: read

If any of this module's dependencies have already been audited, read their
audit reports from `./drafts/security-audit/`. Use the Verdict and Findings
sections as compressed context -- do NOT re-audit dependencies, only use their
results to inform cross-module analysis (e.g., "does this module pass
unvalidated input to a dependency that assumes validated input?").

Skip this step for leaf modules with no internal dependencies.

## Step 5: Audit Single Module

Tool: read, grep

Read the source file at `${MODULE_PATH}`. Perform a structured security audit
covering the following checklist:

### 5a: Input Validation
- All public function parameters validated (bounds, types, sizes)
- No panic-inducing indexing on untrusted input (use `.get()` not `[]`)
- String/buffer length limits enforced

### 5b: Error Handling
- No `unwrap()` or `expect()` on untrusted input
- Errors propagated with `?`, not swallowed
- Error types provide sufficient context for callers

### 5c: Resource Limits
- Memory allocations bounded (no unbounded `Vec::new()` from user input)
- CPU-intensive operations have timeouts or iteration limits
- File handles and connections properly closed (RAII / `Drop`)

### 5d: Secrets and Sensitive Data
- No hardcoded secrets (API keys, passwords, tokens)
- No sensitive data in log output
- Constant-time comparison for secret values

### 5e: Memory Safety
- Every `unsafe` block has a `// SAFETY:` comment
- Public `unsafe fn` has `# Safety` documentation
- No raw pointer casts that could cause UB

### 5f: Concurrency Correctness
- No sync locks held across `.await` points
- Shared mutable state properly guarded (Mutex/RwLock)
- No data races from interior mutability without synchronization

### 5g: Cross-Module Boundaries
- Trust boundaries between modules clearly defined
- Input from other modules re-validated if crossing trust boundary
- Error propagation across module boundaries preserves context

For each checklist item, record: PASS, WARN (non-blocking concern), or FAIL
(security vulnerability) with line numbers and evidence.

## Step 6: Write Audit Report

Tool: write

Write the structured audit report to
`./drafts/security-audit/${MODULE_PATH}.audit.md` using this format:

```markdown
# Security Audit: ${MODULE_PATH}

## Module Info
- Path: ${MODULE_PATH}
- Lines: ${LINE_COUNT}
- Dependencies: ${DEPENDENCY_LIST}
- Auditor: ${MODEL_NAME}
- Date: ${AUDIT_DATE}

## Audit Checklist
- [x] Input validation
- [x] Error handling (no unwrap on untrusted input)
- [ ] Resource limits -- WARN: unbounded allocation at line 142
- [x] No hardcoded secrets
- [x] Memory safety (unsafe blocks documented)
- [x] Concurrency correctness

## Findings

### Critical
(None, or list of security vulnerabilities with line numbers)

### Warning
- **Line 142**: `Vec::new()` grows unbounded from user-supplied iterator.
  Consider adding a capacity limit or `take(MAX_ITEMS)`.

### Info
- **Line 20-25**: Good use of `thiserror` for structured error types.

## Cross-Module Notes
(Observations about how this module interacts with already-audited dependencies)

## Verdict: PASS | PASS_WITH_NOTES | FAIL
```

Verdict criteria:
- **PASS**: Zero Critical, zero Warning findings.
- **PASS_WITH_NOTES**: Zero Critical, one or more Warning or Info findings.
- **FAIL**: One or more Critical findings.

## Step 7: Update Manifest

Tool: bash

Record the audit result in the `csa audit` manifest so the file is no longer
pending.

```bash
csa audit update "${MODULE_PATH}" --status generated --auditor "${MODEL_NAME}"
```

## ENDFOR

## Step 8: Generate Codebase Summary

Tool: write

Aggregate all per-module audit reports into a single summary at
`./drafts/security-audit/SUMMARY.md`:

```markdown
# Codebase Security Audit Summary

## Overview
- Total modules audited: ${COUNT}
- PASS: ${PASS_COUNT}
- PASS_WITH_NOTES: ${NOTES_COUNT}
- FAIL: ${FAIL_COUNT}

## Critical Findings
| Module | Line | Description |
|--------|------|-------------|
| (aggregated from all module reports) |

## Warning Findings
| Module | Line | Description |
|--------|------|-------------|
| (aggregated from all module reports) |

## Dependency Trust Map
(Which modules trust input from which other modules, and whether that trust
is validated)

## Recommendations
1. (Prioritized list of actions to improve security posture)
```

## IF ${FAIL_COUNT} > 0

## Step 9: Generate Remediation Plan

Tool: write

For each FAIL verdict, generate a remediation entry in
`./drafts/security-audit/REMEDIATION.md`:

```markdown
# Remediation Plan

## Critical: ${MODULE_PATH} -- ${FINDING_TITLE}
- **Line**: ${LINE_NUMBER}
- **Issue**: ${DESCRIPTION}
- **Fix**: ${SUGGESTED_FIX}
- **Priority**: P0
```

## ENDIF
