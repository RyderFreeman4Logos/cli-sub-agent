---
name: csa-review
description: CSA-driven code review with independent model selection, session isolation, and structured outputs
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "csa-review"
  - "csa review"
  - "CSA code review"
---

# CSA Review: Independent Code Review Orchestration

## Purpose

Run structured code reviews through CSA, ensuring:
- **Session isolation**: Review sessions stored in `~/.local/state/csa/`, not `~/.codex/`.
- **Independent model selection**: CSA automatically routes to an appropriate review tool based on configuration.
- **Self-contained review agent**: The review agent reads CLAUDE.md and builds project understanding autonomously.
- **Structured outputs**: JSON findings + Markdown report following a tested, optimized prompt.

## Required Inputs

- `scope`: one of:
  - `uncommitted` (default)
  - `base:<branch>` (e.g., `base:main`)
  - `commit:<sha>`
  - `range:<from>...<to>`
  - `files:<pathspec>`
- `mode` (optional): `review-only` (default) or `review-and-fix`
- `security_mode` (optional): `auto` (default) | `on` | `off`
- `tool` (optional): override review tool (default: auto-detect independent reviewer)

## Execution Protocol

### Step 1: Determine Review Tool

The review tool is configured in `~/.config/cli-sub-agent/config.toml` under `[review]`:

```toml
[review]
tool = "auto"  # or "codex", "claude-code", "opencode"
```

**Auto mode** (default):
- Caller is `claude-code` -> review with `codex`
- Caller is `codex` -> review with `claude-code`
- Otherwise -> error with guidance to configure manually

Since this skill is designed to be invoked from Claude Code, the default auto behavior selects `codex` as the review tool.

If the user explicitly passes `tool`, use that instead.

### Step 2: Build Review Prompt

Construct a comprehensive review prompt that the review agent will execute autonomously. The prompt includes all review instructions so the agent is fully self-contained.

**IMPORTANT**: The review agent reads CLAUDE.md itself. Do NOT read CLAUDE.md in the orchestrator and pass its content. The agent needs to build its own project understanding.

```
REVIEW_PROMPT=$(cat <<'REVIEW_EOF'
# Code Review Task

## Step 1: Read Project Context

First, read CLAUDE.md at the project root to understand:
- Project architecture and conventions
- Build and test commands
- Code style requirements
- Any project-specific review criteria

If CLAUDE.md is missing, report this as a warning but continue with general best practices.

## Step 2: Collect Scope

Scope: {scope}

Use the minimum command set for the selected scope:

### uncommitted
```bash
git status --short
git diff --staged --no-color
git diff --no-color
git ls-files --others --exclude-standard
```

### base:<branch>
```bash
BASE_BRANCH="{branch}"
BASE_SHA="$(git merge-base HEAD "$BASE_BRANCH")"
git diff --no-color "$BASE_SHA"...HEAD
```

### commit:<sha>
```bash
git show --no-color "{sha}"
```

### range:<from>...<to>
```bash
git diff --no-color "{from}...{to}"
```

### files:<pathspec>
```bash
git diff --no-color -- "{pathspec}"
```

## Step 3: Three-Pass Review

### Pass 1: Broad Issue Discovery (maximize recall)
Scan all changed code for:
- Correctness issues
- Regressions
- Missing error handling
- Test gaps

### Pass 2: Evidence Filtering (maximize precision)
For each candidate finding:
- Verify with concrete evidence (trigger, expected, actual)
- Deduplicate overlapping findings
- Discard findings without sufficient evidence (move to open_questions)

### Pass 3: Adversarial Security Analysis (maximize exploitability coverage)

Security mode: {security_mode}

- `on`: Always execute this pass.
- `auto`: Execute when scope touches risky surfaces (auth, crypto, external input boundaries, parser/deserialization, network handlers, permission/tenant checks, query/file/path handling, concurrency/resource limits).
- `off`: Skip dedicated pass 3, but still report obvious security issues from passes 1-2.

When executing, reason from attacker perspective and evaluate exploitability for:
- Authentication/authorization bypass and privilege escalation
- Cryptographic misuse (algorithm/mode/randomness/key/constant-time comparison)
- Denial-of-service vectors (unbounded CPU/memory/IO, regex backtracking, lock contention, retry storms, request amplification)
- Injection/deserialization/path traversal/SSRF/RCE primitives

High-impact security suspicion without concrete exploit path -> list under open_questions, not findings.

## Non-Negotiable Rules

1. Always read CLAUDE.md before any review reasoning.
2. Do not call `codex review` subcommand.
3. Prefer read-only inspection for review steps.
4. Focus findings on correctness, regressions, security, and missing tests.
5. Treat insufficient tests as first-class findings using finding_type: test-gap with explicit priority.
6. Every finding must include concrete evidence with trigger, expected, actual, and file+line references.
7. If evidence is insufficient, do not emit a finding; emit an open_questions item instead.
8. Any high-impact security suspicion without a concrete exploit path must be listed under open_questions instead of findings.
9. Confidence must be calibrated with evidence strength. High confidence without concrete evidence is invalid.

## Step 4: Generate Outputs

### review-findings.json

```json
{
  "findings": [
    {
      "id": "string",
      "priority": "P0|P1|P2|P3",
      "finding_type": "correctness|regression|security|test-gap|maintainability",
      "file": "string",
      "line": 0,
      "summary": "string",
      "trigger": "string",
      "expected": "string",
      "actual": "string",
      "impact": "string",
      "evidence": "string",
      "verification": "string",
      "attack_path": "string",
      "preconditions": "string",
      "exploit_steps": "string",
      "blast_radius": "string",
      "mitigation": "string",
      "cwe": "string",
      "fix_hint": "string",
      "test_case_hint": "string",
      "confidence": 0.0
    }
  ],
  "overall_risk": "low|medium|high|critical",
  "overall_summary": "string",
  "test_gaps": ["string"],
  "open_questions": [
    {
      "id": "string",
      "question": "string",
      "needed_evidence": "string"
    }
  ],
  "security_review": {
    "security_mode": "auto|on|off",
    "adversarial_pass_executed": true,
    "triggered_by": ["string"]
  },
  "suggested_next_actions": ["string"]
}
```

### review-report.md

```markdown
# Code Review Report

## Scope
- Scope: {scope}
- Mode: {mode}
- Context source: CLAUDE.md
- Security mode: {security_mode}

## Findings (ordered by severity)
1. [P?][<finding_type>] <summary> (`<file>:<line>`, confidence=<0.00>)

## Security Findings (attacker perspective)
1. [P?][security] <summary> (`<file>:<line>`)
- Attack path: <...>
- Preconditions: <...>
- Exploit steps: <...>
- Blast radius: <...>
- Mitigation: <...>

## Test Coverage Findings
1. [P?][test-gap] <summary> (`<file>:<line>`)

## Test Gaps
- <gap>

## Open Questions
- <question + needed evidence>

## Overall Risk
- <risk>

## Recommended Actions
1. <action>
```

Write both files to the current working directory (or a designated output location).

REVIEW_EOF
)
```

### Step 3: Execute Review via CSA

```bash
csa run --tool {review_tool} \
  --description "code-review: {scope}" \
  "{REVIEW_PROMPT}"
```

Key behaviors:
- CSA manages the session in `~/.local/state/csa/` (not `~/.codex/`).
- The review agent has full autonomy: it reads CLAUDE.md, runs git commands, reads source files, and generates outputs.
- CSA handles concurrency control via global slots.
- The session is persistent and can be resumed for fixes.

### Step 4: Present Results

After CSA returns:
1. Read and display `review-report.md` if generated.
2. Read and display `review-findings.json` summary (finding count by priority).
3. Report the CSA session ID for potential follow-up.

### Step 5: Fix Mode (optional, when mode=review-and-fix)

If mode is `review-and-fix`:

```bash
csa run --tool {review_tool} \
  --session {csa_session_id} \
  "Based on the review findings, fix all P0 and P1 issues:

1. Apply fixes for all P0 and P1 findings, including test-gap findings (add/update tests).
2. For security findings, verify exploit paths are closed and document residual risk.
3. Re-run targeted checks/tests for touched areas and record verification evidence.
4. Generate:
   - fix-summary.md (what was fixed and how)
   - post-fix-review-findings.json (remaining findings after fixes)
5. If any P0/P1 remains, explicitly mark as incomplete with explanation."
```

This resumes the same CSA session, preserving the review context.

### Step 6: Verification (optional)

After fixes, optionally run:
```bash
just pre-commit
```
or trigger another review round to verify fixes.

## Comparison with Original Workflow

| Aspect | Original orchestrator | CSA Review |
|--------|----------------------|------------|
| Session storage | `~/.codex/` (pollutes user sessions) | `~/.local/state/csa/` |
| Session management | None | `csa session list`, `csa gc` |
| Project understanding | Caller pre-reads CLAUDE.md | Review agent reads it autonomously |
| Tool selection | Hardcoded codex | Auto independent + configurable |
| Prompt | Bash heredoc | Embedded review prompt (see Step 2 below) |
| Concurrency control | None | CSA global slots |
| Session resume | Manual thread_id tracking | `csa review --session {id}` |
| Fix workflow | Separate script invocation | Same session resume |

## Example Usage

### Basic Review (uncommitted changes)
```
User: /csa-review
```
-> Auto-selects codex (since caller is claude-code)
-> Reviews uncommitted changes with security_mode=auto
-> Generates review-findings.json + review-report.md

### Review Against Main Branch
```
User: /csa-review scope=base:main security_mode=on
```
-> Reviews all changes since main with mandatory security pass

### Review and Fix
```
User: /csa-review scope=uncommitted mode=review-and-fix
```
-> Reviews, then fixes P0/P1 in the same session

### Explicit Tool Override
```
User: /csa-review tool=opencode scope=base:dev
```
-> Uses opencode instead of auto-detected tool

## Disagreement Escalation (when findings are contested)

When the developer (or orchestrating agent) disagrees with a csa-review finding:

1. **NEVER silently dismiss findings.** Every finding was produced by an independent
   model with evidence — it deserves adversarial evaluation, not unilateral dismissal.

2. **Use the `debate` skill** to arbitrate contested findings:
   - The finding becomes the "question" for debate
   - The reviewer's evidence is the initial proposal
   - The developer's counter-argument is the critique
   - The debate MUST use independent models (CSA routes to a different backend from both the reviewer and developer)

3. **Record the outcome**: If a finding is dismissed after debate, document the
   debate verdict (with model specs) in the review report or PR comment.

4. **Escalate to user** if debate reaches deadlock (both sides have valid points).

**FORBIDDEN**: Dismissing a csa-review finding without adversarial arbitration.
The code author's confidence alone is NOT sufficient justification.

## Done Criteria

1. Review prompt was sent to CSA with the correct tool.
2. CSA session was created in `~/.local/state/csa/` (verify with `csa session list`).
3. No sessions were created in `~/.codex/`.
4. Review agent read CLAUDE.md autonomously (not pre-fed by orchestrator).
5. `review-findings.json` and `review-report.md` were generated.
6. Every finding has concrete evidence (trigger, expected, actual) and calibrated confidence.
7. If security_mode required pass 3, adversarial_pass_executed=true.
8. If mode=review-and-fix, fix artifacts exist and session was resumed (not new).
9. CSA session ID was reported for potential follow-up.
10. **If any finding was contested**: debate skill was used with independent models, and outcome documented with model specs.
