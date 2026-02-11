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

## Role Detection (READ THIS FIRST — MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the csa-review skill"`, then:

**YOU ARE THE REVIEW AGENT.** Follow these rules:
1. **SKIP the entire "Execution Protocol" section below** — it is for the orchestrator, not you.
2. **Read [Review Protocol](references/review-protocol.md)** and follow it step by step. That file tells you exactly how to perform the review.
3. **Read [Output Schema](references/output-schema.md)** for the required output format.
4. Your scope/mode/security_mode parameters are in your initial prompt. Parse them from there.
5. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the review DIRECTLY by running `git diff`, reading files, and analyzing code yourself. Running any `csa` command causes infinite recursion and will be terminated.

**Only if you are Claude Code and a human user typed `/csa-review` in the chat**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Run structured code reviews through CSA, ensuring:
- **Session isolation**: Review sessions stored in `~/.local/state/csa/`, not `~/.codex/`.
- **Independent model selection**: CSA automatically routes to an appropriate review tool based on configuration.
- **Self-contained review agent**: The review agent reads CLAUDE.md and builds project understanding autonomously.
- **Structured outputs**: JSON findings + Markdown report following a tested, optimized prompt.

## Required Inputs (Orchestrator)

- `scope`: one of:
  - `uncommitted` (default)
  - `base:<branch>` (e.g., `base:main`)
  - `commit:<sha>`
  - `range:<from>...<to>`
  - `files:<pathspec>`
- `mode` (optional): `review-only` (default) or `review-and-fix`
- `security_mode` (optional): `auto` (default) | `on` | `off`
- `tool` (optional): override review tool (default: auto-detect independent reviewer)
- `context` (optional): path to a TODO plan file (e.g., from `csa todo show -t <timestamp>`) to check implementation alignment against the planned design

## Execution Protocol (ORCHESTRATOR ONLY — review agents MUST skip this section)

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

### Step 1.5: Pre-PR TODO Plan Alignment (MANDATORY for pre-PR mode)

When the review scope covers `main...HEAD` (i.e., pre-PR review), the orchestrator MUST:

1. **Auto-detect associated TODO**: Run `csa todo find --branch $(git branch --show-current)` to find the TODO plan for the current branch.
2. **Pass as context**: If a TODO is found, pass its path as the `context` parameter so the review agent performs alignment checking (see [Review Protocol](references/review-protocol.md) Step 2.5).
3. **FATAL on failure**: If `csa todo find --branch` returns empty or fails, this is a **FATAL error**. Stop immediately and notify the user. Do NOT proceed without a TODO plan. Do NOT fallback to review-without-alignment.

**Why strict**: Pre-PR reviews verify that the branch implements its planned work correctly. Without a TODO plan, alignment checking is impossible, and the review provides incomplete assurance.

**Exception**: If the user explicitly provides `context=<path>`, skip auto-detection and use the provided path.

### Step 2: Build Review Prompt

Construct a comprehensive review prompt that the review agent will execute autonomously. The prompt includes all review instructions so the agent is fully self-contained.

**IMPORTANT**: The review agent reads CLAUDE.md itself. Do NOT read CLAUDE.md in the orchestrator and pass its content. The agent needs to build its own project understanding.

The review prompt instructs the agent to: read project context (CLAUDE.md + AGENTS.md), collect the diff for the given scope, perform a three-pass review (discovery, evidence filtering, adversarial security), and generate structured outputs.

> **See**: [Review Protocol](references/review-protocol.md) for the full agent instructions (scope commands, AGENTS.md compliance, three-pass review, non-negotiable rules).

> **See**: [Output Schema](references/output-schema.md) for the JSON findings schema and Markdown report template.

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

If mode is `review-and-fix`, resume the same CSA session to fix all P0 and P1 issues, generate fix-summary.md and post-fix-review-findings.json, and mark any remaining P0/P1 as incomplete.

> **See**: [Fix Workflow](references/fix-workflow.md) for the full fix mode protocol and verification steps.

### Step 6: Verification (optional)

After fixes, optionally run:
```bash
just pre-commit
```
or trigger another review round to verify fixes.

## Example Usage

| Command | Effect |
|---------|--------|
| `/csa-review` | Auto-selects codex, reviews uncommitted changes, security_mode=auto |
| `/csa-review scope=base:main security_mode=on` | Reviews all changes since main with mandatory security pass |
| `/csa-review scope=uncommitted mode=review-and-fix` | Reviews, then fixes P0/P1 in the same session |
| `/csa-review scope=uncommitted context=$(csa todo show -t <ts> --path)` | Reviews and checks alignment against a TODO plan |
| `/csa-review tool=opencode scope=base:dev` | Uses opencode instead of auto-detected tool |

## Disagreement Escalation

When findings are contested, use the `debate` skill for adversarial arbitration. Findings must never be silently dismissed — every finding deserves independent evaluation.

> **See**: [Disagreement Escalation](references/disagreement-escalation.md) for the full dispute resolution protocol.

## References

| File | Purpose |
|------|---------|
| [references/review-protocol.md](references/review-protocol.md) | Full agent review instructions: project context, scope commands, AGENTS.md compliance, three-pass review, non-negotiable rules |
| [references/output-schema.md](references/output-schema.md) | JSON findings schema (`review-findings.json`) and Markdown report template (`review-report.md`) |
| [references/fix-workflow.md](references/fix-workflow.md) | Fix mode protocol (Step 5) and verification (Step 6) for `review-and-fix` mode |
| [references/disagreement-escalation.md](references/disagreement-escalation.md) | Finding dispute resolution via `debate` skill with independent models |

## Done Criteria

1. Review prompt was sent to CSA with the correct tool.
2. CSA session was created in `~/.local/state/csa/` (verify with `csa session list`).
3. No sessions were created in `~/.codex/`.
4. **No recursive `csa run` or `csa review` calls** from the review agent (session tree depth = 2 max: orchestrator → review agent).
5. Review agent read CLAUDE.md autonomously (not pre-fed by orchestrator).
6. Review agent discovered and applied AGENTS.md files (root-to-leaf) for all changed paths.
7. `review-findings.json` and `review-report.md` were generated.
8. Every finding has concrete evidence (trigger, expected, actual) and calibrated confidence. AGENTS.md violations reference rule IDs.
9. If security_mode required pass 3, adversarial_pass_executed=true.
10. If mode=review-and-fix, fix artifacts exist and session was resumed (not new).
11. CSA session ID was reported for potential follow-up.
12. **If any finding was contested**: debate skill was used with independent models, and outcome documented with model specs.
