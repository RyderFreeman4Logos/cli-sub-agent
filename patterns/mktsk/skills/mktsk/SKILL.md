---
name: mktsk
description: "Use when: converting TODO plan into deterministic execution checklist"
allowed-tools: Bash, Read, Grep, Glob, Write, Edit, TaskCreate, TaskUpdate, TaskGet, TaskList
triggers:
  - "mktsk"
  - "/mktsk"
  - "make tasks"
  - "execute plan"
  - "todo to tasks"
---

# mktsk: Make Task -- Plan-to-Execution Bridge

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the mktsk skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `../../PATTERN.md` relative to this `SKILL.md`, and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" below.

---

## Purpose

Execute TODO plans (from `mktd` or user-provided) as deterministic, resumable serial checklists.
The workflow.toml contains hard bash gates that enforce review and pr-codex-bot — these gates
cannot be skipped by any LLM, unlike natural-language instructions.

## SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**.

## Example Usage

| Command | Effect |
|---------|--------|
| `/mktsk` | Execute the most recent TODO plan for the current branch |
| `/mktsk path=./plans/feature.md` | Execute tasks from a specific plan file |
| `/mktsk timestamp=01JK...` | Execute tasks from a csa todo by timestamp |

## Integration

- **Depends on**: `mktd` (provides TODO plan), `commit` (per-task commit workflow)
- **Uses**: `csa-review` (per-task review), `security-audit` (via commit skill)
- **Boundary**: Standalone mktsk completes the full pipeline (push/PR/pr-codex-bot/merge).
  When called from dev2merge (`CSA_SKIP_PUBLISH=true`), publish steps are skipped.

## Done Criteria

1. `csa plan run --pattern mktsk` exits 0 (all workflow steps passed, including hard review gates).
2. All TODO items marked complete.
3. Branch pushed, PR created, pr-codex-bot completed (or skipped when `CSA_SKIP_PUBLISH=true`).

---

## Execution Protocol (ORCHESTRATOR ONLY)

**CRITICAL: You MUST execute this as a deterministic workflow via `csa plan run`.
Do NOT manually interpret or execute the individual workflow steps yourself.
The workflow.toml contains hard bash review gates that enforce `csa review` and
`pr-codex-bot` — manually executing steps WILL skip these gates.**

```bash
csa plan run --pattern mktsk --sa-mode true
```

If you need to pass variables (e.g., the TODO plan path):

```bash
csa plan run --pattern mktsk --sa-mode true --var TODO_PATH="<path>"
```

**DO NOT** read the step-by-step details below and execute them yourself.
**DO NOT** skip `csa plan run` to "save time".
**The workflow enforces review gates that you cannot replicate by manual execution.**
