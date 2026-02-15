---
name = "file-audit"
description = "Per-file AGENTS.md compliance audit with Chinese reports and tech blog"
allowed-tools = "Bash, Read, Grep, Glob, Write"
tier = "tier-1-quick"
version = "0.1.0"
---

# Per-File AGENTS.md Compliance Audit

Audit each source file against applicable AGENTS.md rules. For every file,
generate a Chinese audit report with checkbox compliance evidence under
`./drafts/` mirroring the source directory structure. Conclude with a
Chinese tech blog summarizing findings.

## Step 1: Discover AGENTS.md Chain

Tool: bash

Discover all AGENTS.md files from repository root to source directories.
Build the rule chain (root -> leaf, deepest wins on conflict).

```bash
find "${PROJECT_ROOT}" -name "AGENTS.md" -not -path "*/node_modules/*" -not -path "*/target/*" | sort
```

## Step 2: Collect Target Files

Tool: bash

List the source files to audit. Focus on ${AUDIT_SCOPE} (e.g., a specific
crate or directory). Output one file path per line.

```bash
find "${AUDIT_SCOPE}" -name "*.rs" -not -name "*test*" | sort
```

## Step 3: Prepare Output Directory

Tool: bash

Create the mirrored directory structure under `./drafts/audit/` for each
target file. The mirror preserves the relative path from project root.

```bash
mkdir -p "./drafts/audit/$(dirname "${RELATIVE_PATH}")"
```

## FOR file IN ${TARGET_FILES}

## Step 4: Audit Single File

Tool: csa
Tier: tier-1-quick

Read the file at ${FILE_PATH} and audit it against the AGENTS.md rules.
Check each applicable rule and record pass/fail with evidence.

You MUST output a Chinese Markdown report with this structure:

1. File header (path, line count, crate)
2. Applicable AGENTS.md rules checklist (checkbox format)
3. Findings (violations with line numbers and evidence)
4. Summary verdict (PASS / PASS_WITH_NOTES / FAIL)

Example checkbox format:
- [x] Rust-002 error-handling: no unwrap/expect in library code
- [ ] Rust-004 modules: default pub(crate) -> VIOLATION at line 42

## Step 5: Write Audit Report

Tool: bash

Write the audit report to `./drafts/audit/${RELATIVE_PATH}.audit.md`.
The report is in Chinese with AGENTS.md compliance checkboxes.

## ENDFOR

## IF ${HAS_VIOLATIONS}

## Step 6: Generate Violation Summary

Tool: csa
Tier: tier-1-quick

Aggregate all per-file violations into a single summary table.
Group by rule ID, count occurrences, list affected files.
Output in Chinese Markdown.

## ENDIF

## Step 7: Write Tech Blog

Tool: csa
Tier: tier-1-quick

Write a Chinese tech blog post summarizing the audit process and findings.
Save to `./drafts/audit/blog.zh.md`.

The blog should cover:
1. Why per-file AGENTS.md compliance matters
2. The audit methodology (rule chain discovery, per-file checking)
3. Key findings and patterns
4. Lessons learned about maintaining coding standards with AI agents
5. How skill-lang patterns enable reproducible audit workflows
