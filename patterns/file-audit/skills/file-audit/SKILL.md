---
name: file-audit
description: Per-file AGENTS.md compliance audit with Chinese reports and tech blog summary
allowed-tools: Bash, Read, Grep, Glob, Write
triggers:
  - "file-audit"
  - "/file-audit"
  - "audit files"
  - "AGENTS.md compliance"
---

# File Audit: Per-File AGENTS.md Compliance Audit

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the file-audit skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/file-audit/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Audit each source file in a given scope against the applicable AGENTS.md rules chain (root-to-leaf). For every file, generate a Chinese audit report with checkbox compliance evidence under `./drafts/audit/`, mirroring the source directory structure. Conclude with a Chinese tech blog summarizing findings, patterns, and lessons learned about maintaining coding standards with AI agents.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- At least one AGENTS.md file must exist in the project
- Target scope (directory or file pattern) must be specified

### Quick Start

```bash
csa run --skill file-audit "Audit all files in src/executor/ against AGENTS.md rules"
```

### Step-by-Step

1. **Discover AGENTS.md chain**: Find all AGENTS.md files from repository root to source directories. Build rule chain (root -> leaf, deepest wins on conflict).
2. **Collect target files**: List source files in the audit scope (e.g., `find src/ -name "*.rs"`).
3. **Prepare output directory**: Create mirrored directory structure under `./drafts/audit/`.
4. **Per-file audit** (via CSA, tier-1 for each file): Read file, check each applicable AGENTS.md rule, record pass/fail with evidence. Output a Chinese Markdown report with:
   - File header (path, line count, crate)
   - Applicable rules checklist (checkbox format)
   - Findings (violations with line numbers)
   - Summary verdict (PASS / PASS_WITH_NOTES / FAIL)
5. **Write audit reports**: Save each report to `./drafts/audit/${RELATIVE_PATH}.audit.md`.
6. **Violation summary** (if violations found): Aggregate all violations into a summary table grouped by rule ID.
7. **Tech blog**: Write a Chinese tech blog to `./drafts/audit/blog.zh.md` covering methodology, key findings, patterns, and lessons learned.

## Example Usage

| Command | Effect |
|---------|--------|
| `/file-audit src/executor/` | Audit all .rs files in executor module |
| `/file-audit src/` | Audit entire src directory |
| `/file-audit crates/csa-acp/src/` | Audit the csa-acp crate |

## Integration

- **Standalone**: Does not depend on other skills
- **Related to**: `csa-review` (diff-based review vs per-file audit), `security-audit` (security focus vs compliance focus)
- **Output**: `./drafts/audit/` directory with per-file reports and summary blog

## Done Criteria

1. AGENTS.md chain discovered from root to all source directories.
2. All target files in scope identified and audited.
3. Per-file Chinese audit report generated with checkbox compliance evidence.
4. Reports written to `./drafts/audit/` mirroring source directory structure.
5. If violations found: aggregated violation summary table produced.
6. Chinese tech blog written to `./drafts/audit/blog.zh.md`.
7. Each report includes verdict: PASS, PASS_WITH_NOTES, or FAIL.
