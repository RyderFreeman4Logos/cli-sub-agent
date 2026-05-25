---
name: code-health-agent
description: "Use when running a non-blocking CSA background code health scan that uses csa health and csa tokuin estimate to propose refactoring GitHub issues for files over token or complexity thresholds."
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "code-health-agent"
  - "/code-health-agent"
  - "code health agent"
  - "background code health"
---

# code-health-agent: Background Code Health Agent

## Role Detection (READ THIS FIRST -- MANDATORY)

Role MUST be determined by explicit mode marker, not natural-language matching.
Treat the run as executor ONLY when the initial prompt contains:
`<skill-mode>executor</skill-mode>`.

**YOU ARE THE EXECUTOR.** Follow the "Executor Workflow" directly.

Executor constraints:
1. Do NOT run `csa run`, `csa review`, `csa debate`, or `csa plan`.
2. You MAY run local inspection commands including `csa health` and
   `csa tokuin estimate`; these are required inputs, not recursive delegation.
3. Do not edit workspace files. Create GitHub issues only.

**Only if you are the main agent (Claude Code / human user)**:
- You are the orchestrator. Follow "Orchestrator Usage" and return the spawned
  session id. Do not wait unless explicitly asked.

---

## Purpose

Run a background, non-blocking code health scan. The agent identifies files that
exceed token or complexity thresholds, checks whether the size reflects cohesive
domain depth or SRP drift, then files GitHub issues proposing refactors. Findings
are advisory: this skill must never block PRs, commits, or release gates.

## Configuration

Defaults:

| Setting | Default | Meaning |
|---------|---------|---------|
| `TOKEN_THRESHOLD` | `8000` | File token budget for BLOCK candidates |
| `WARNING_THRESHOLD` | `6000` | Early warning budget passed to `csa health` |
| `EXTENSIONS` | `rs` | Comma-separated extensions for `csa health` |
| `SCAN_FREQUENCY` | `once` | Caller-owned schedule label; this skill scans once |
| `MAX_ISSUES` | `10` | Maximum GitHub issues to create per run |
| `DRY_RUN` | `false` | Print proposed issues instead of filing them |
| `ISSUE_LABELS` | `code-health,refactor` | Comma-separated labels for created issues |

Accept these settings from the user prompt first, then environment variables,
then defaults. Do not implement a sleep loop for `SCAN_FREQUENCY`; external
schedulers should invoke this skill at the requested cadence.

## Orchestrator Usage

Launch as a background CSA session and return immediately:

```bash
csa run --sa-mode true --skill code-health-agent --timeout 1800 \
  --description "background code health scan" \
  "Run one code health scan with TOKEN_THRESHOLD=8000 WARNING_THRESHOLD=6000 EXTENSIONS=rs."
```

Rules for orchestrators:

1. Do not pass `--no-daemon`.
2. Do not add this skill to required PR, pre-commit, or pre-push gates.
3. Do not wait for completion unless the user explicitly requests the result.

## Executor Workflow

### 1. Resolve Config

Record the effective values for `TOKEN_THRESHOLD`, `WARNING_THRESHOLD`,
`EXTENSIONS`, `MAX_ISSUES`, `DRY_RUN`, `ISSUE_LABELS`, and `SCAN_FREQUENCY`.
Reject only invalid numeric values or `WARNING_THRESHOLD >= TOKEN_THRESHOLD`.

### 2. Scan Token Health

Run `csa health` as the primary workspace scan:

```bash
csa health --json --threshold "$TOKEN_THRESHOLD" \
  --warning "$WARNING_THRESHOLD" --extensions "$EXTENSIONS"
```

Select files whose health status is `BLOCK`. If JSON parsing tooling is not
available, rerun with text output and parse conservatively by path.

### 3. Verify Candidate Token Counts

For every candidate, verify the token count with `csa tokuin estimate`:

```bash
csa tokuin estimate --json --budget "$TOKEN_THRESHOLD" "$file"
```

If `csa health` and `csa tokuin estimate` disagree materially, cite both values
in the issue body and prefer the `csa tokuin estimate` value for the final
threshold decision.

### 4. Analyze SRP and Code Judo

Read only candidate files. For each over-budget file, decide whether it is:

- **Cohesive depth**: large but still one responsibility; no issue unless there
  is a clear deletion or extraction path.
- **SRP drift**: multiple responsibilities, unrelated command modes, mixed IO
  and domain logic, or test scaffolding embedded in production code.
- **Code Judo opportunity**: a branch, condition family, compatibility layer,
  pass-through adapter, duplicated mode, or abstraction layer can be deleted or
  collapsed instead of split into smaller pieces.

Prefer proposals that remove behavior or layers over proposals that only move
code into more files. Do not propose a refactor unless you can name the branch,
condition, layer, or responsibility boundary that changes.

### 5. File GitHub Issues

Before creating an issue, check for an existing open issue for the same path:

```bash
GH_CONFIG_DIR=~/.config/gh-aider gh issue list \
  --state open --search "\"$file\" code health" --limit 5
```

Create at most `MAX_ISSUES` issues. Use `GH_CONFIG_DIR=~/.config/gh-aider` for
all `gh issue` commands. Never open a PR from this skill.

Issue title format:

```text
refactor: reduce code health risk in <path>
```

Issue body template:

```markdown
## Evidence

- File: `<path>`
- `csa health`: <tokens> tokens, threshold <TOKEN_THRESHOLD>
- `csa tokuin estimate`: <tokens> tokens
- Scan frequency label: `<SCAN_FREQUENCY>`

## SRP Analysis

<Explain whether the file mixes responsibilities, and name the boundaries.>

## Code Judo Analysis

<Name branches, conditions, layers, adapters, compatibility paths, or duplicate
flows that could be eliminated instead of merely split.>

## Proposal

<Smallest useful refactor. Prefer deletion/collapse before extraction.>

## DONE WHEN

- `csa health --threshold <TOKEN_THRESHOLD> --extensions <EXTENSIONS>` no longer
  reports this file as BLOCK, or the issue documents why the file is cohesive
  depth and should be exempted.
- Relevant tests for the touched module pass.
```

Apply labels from `ISSUE_LABELS` when possible. If label creation is unavailable
or unauthorized, file the issue without labels and mention that in the final
summary.

## Done Criteria

1. `csa health` ran with the effective threshold and extensions.
2. Every BLOCK candidate was verified with `csa tokuin estimate`.
3. Every over-threshold candidate was classified as cohesive depth, SRP drift,
   or Code Judo opportunity.
4. GitHub issues were created for actionable refactors, or `DRY_RUN=true`
   printed the exact issue title/body that would be created.
5. The final summary includes effective config, candidates scanned, issues
   created or skipped, and any command failures.
6. The run did not modify workspace files and did not create a PR.
