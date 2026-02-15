---
name: csa-issue-reporter
description: File structured GitHub issues on cli-sub-agent when CSA encounters operational errors
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "csa-issue-reporter"
  - "/csa-issue-reporter"
  - "report csa issue"
  - "file csa bug"
---

# CSA Issue Reporter: Structured Error Reporting

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the csa-issue-reporter skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/csa-issue-reporter/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Automatically file a structured GitHub issue on the cli-sub-agent repository when CSA encounters an operational error (quota exhaustion, crash, parse failure, session state corruption, tool not found). The issue includes environment context (csa version, OS, rustc version), error classification with appropriate labels, and a structured body following Conventional Commits title format.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `gh` CLI MUST be authenticated: `gh auth status`
- Error context must be available (error_summary, csa_command, error_output)

### Quick Start

```bash
csa run --skill csa-issue-reporter \
  "Report error: <error_summary>. Command: <csa_command>. Output: <error_output>"
```

### Step-by-Step

1. **Gather environment context**: Collect `csa --version`, `uname -srm`, `rustc --version`, latest git commit hash.
2. **Determine labels**: Map error type to GitHub labels:
   - Quota/rate limit -> `bug, provider-quota`
   - Crash/panic -> `bug, crash`
   - Parse failure -> `bug, parsing`
   - Session state -> `bug, session`
   - Tool not found -> `bug, configuration`
   - Unexpected behavior -> `bug`
3. **Create issue**: `gh issue create --repo RyderFreeman4Logos/cli-sub-agent` with Conventional Commits title format (`type(scope): error_summary`).
4. **Report back**: Return the created issue URL to the caller.

## Example Usage

| Command | Effect |
|---------|--------|
| `/csa-issue-reporter "codex quota exhausted during review"` | File quota-related bug issue |
| `/csa-issue-reporter "session state.toml parse error"` | File parsing-related bug issue |

## Integration

- **Triggered by**: Any skill/pattern that encounters a CSA operational error
- **Standalone**: Does not depend on or trigger other skills
- **Target repo**: Always files on `RyderFreeman4Logos/cli-sub-agent`

## Done Criteria

1. Environment context gathered (csa version, OS, rustc version).
2. Error type mapped to appropriate GitHub labels.
3. Issue created on `RyderFreeman4Logos/cli-sub-agent` with structured body.
4. Issue title follows Conventional Commits format.
5. Issue URL returned to the caller.
