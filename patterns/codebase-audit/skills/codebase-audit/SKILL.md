---
name: codebase-audit
description: Bottom-up per-module security audit of AI-generated codebases with structured reports
allowed-tools: Bash, Read, Grep, Glob, Write
triggers:
  - "codebase-audit"
  - "/codebase-audit"
  - "security audit codebase"
  - "audit codebase"
---

# Codebase Audit: Bottom-Up Security Audit

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the codebase-audit skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/codebase-audit/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Perform a systematic, bottom-up security audit of an entire codebase (or scoped subset). Modules are processed in topological order -- leaf dependencies first -- so that when a module is audited, all of its dependencies already have reports. This enables cross-module trust boundary analysis that single-file audits miss.

Each module receives a structured audit report covering: input validation, error handling, resource limits, secrets, memory safety, and concurrency correctness. A codebase-wide summary aggregates findings and generates a prioritized remediation plan for any critical issues.

Uses the `csa audit` CLI for manifest tracking, ensuring audit progress is persistent and resumable.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- The project must have source files trackable by `csa audit`
- Audit scope should be defined (full codebase or specific directory)

### Quick Start

```bash
csa run --skill codebase-audit "Audit the entire codebase for security vulnerabilities"
```

Or with a specific scope:

```bash
csa run --skill codebase-audit "Audit src/executor/ and src/config/ modules"
```

### Step-by-Step

1. **Initialize manifest**: Run `csa audit init` (first time) or `csa audit sync` (refresh with new files).
2. **Get work queue**: `csa audit status --format json --order topo --filter pending` returns modules in dependency order (leaves first).
3. **Prepare output**: Create `./drafts/security-audit/` directory mirroring source structure.
4. **Per-module audit** (bottom-up):
   - Load prior dependency audit reports as compressed context
   - Read source file, perform structured security checklist
   - Write audit report to `./drafts/security-audit/${path}.audit.md`
   - Update manifest: `csa audit update <file> --status generated --auditor <model>`
5. **Generate summary**: Aggregate all findings into `./drafts/security-audit/SUMMARY.md`.
6. **Remediation plan** (if critical findings): Write `./drafts/security-audit/REMEDIATION.md` with prioritized fixes.

### Resumability

This pattern is fully resumable. If interrupted:
- `csa audit status --filter pending` shows remaining work
- Already-audited modules are skipped (manifest tracks completion)
- Re-run the same command to continue from where it left off

## Example Usage

| Command | Effect |
|---------|--------|
| `/codebase-audit` | Audit all pending modules in topological order |
| `/codebase-audit src/executor/` | Audit only executor module and its dependencies |
| `/codebase-audit --resume` | Continue a previously interrupted audit |

## Integration

- **Depends on**: `csa audit` CLI (init, status, update, sync subcommands)
- **Related to**: `file-audit` (AGENTS.md compliance vs security focus), `security-audit` (per-commit vs codebase-wide)
- **Output**: `./drafts/security-audit/` directory with per-module reports, summary, and remediation plan

## Done Criteria

1. `csa audit init` or `csa audit sync` completed successfully.
2. All modules in scope processed in topological order (leaves first).
3. Per-module audit report generated with structured checklist and verdict.
4. Reports written to `./drafts/security-audit/` mirroring source structure.
5. Manifest updated for each audited module (`csa audit status --filter pending` returns empty for scope).
6. `SUMMARY.md` generated with aggregated findings and statistics.
7. If any FAIL verdicts: `REMEDIATION.md` generated with prioritized fix plan.
