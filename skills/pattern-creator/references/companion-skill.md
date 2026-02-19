# Companion Skill Template & Rules

## Template

Replace all `<name>` with the actual pattern name (kebab-case).

```yaml
---
name: <name>
description: "<One sentence describing the pattern's purpose>"
allowed-tools: Bash, Read, Grep, Glob, Edit
triggers:
  - "<name>"
  - "/<name>"
  - "<alternative trigger phrase>"
---
```

```markdown
# <Name>: <Human-Readable Title>

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string
`"Use the <name> skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator,
   not you.
2. **Read the pattern** at `patterns/<name>/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`,
   or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa`
   command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

<What this pattern does and its key guarantees. 2-3 sentences.>

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- <other prerequisites>

### Quick Start

```bash
csa run --skill <name> "<default prompt>"
```

### Step-by-Step

<Numbered summary of the PATTERN.md steps, written for the orchestrator.
Each bullet should be one sentence describing what happens.>

1. **Step name**: What it does.
2. **Step name**: What it does.
...

## Example Usage

| Command | Effect |
|---------|--------|
| `/<name>` | <default behavior> |
| `/<name> arg=value` | <with arguments> |

## Integration

- **Depends on**: <patterns this composes with via INCLUDE>
- **Used by**: <patterns that INCLUDE this one>
- **Triggers**: <patterns invoked conditionally>

## Done Criteria

<Numbered list of mechanically verifiable conditions.
Each criterion should be checkable via a command or observation.>

1. <condition 1>
2. <condition 2>
...
```

## Rules

### MUST

1. **Role Detection is mandatory** — without it, executor agents will try to
   run `csa` commands and cause infinite recursion.

2. **YAML frontmatter** — companion skills use YAML (`name: value`), NOT TOML
   (`name = "value"`). PATTERN.md uses TOML.

3. **Triggers include the pattern name** — at minimum, include `"<name>"` and
   `"/<name>"` as triggers.

4. **Done Criteria are mechanically verifiable** — each criterion should be
   checkable by a command (exit code, file exists, git status clean, etc.).

5. **allowed-tools must match PATTERN.md** — the companion skill's
   `allowed-tools` should be a superset of what the pattern needs.

### MUST NOT

1. **Do NOT put workflow steps in the companion skill** — the skill is a
   dispatcher, not the workflow. All step logic lives in PATTERN.md.

2. **Do NOT duplicate PATTERN.md content** — the companion skill summarizes
   the workflow in "Step-by-Step" but does not reproduce the full steps.

3. **Do NOT use TOML frontmatter** — YAML only for companion skills.

### Differences from Regular Skills

| Aspect | Regular Skill | Companion Skill |
|--------|---------------|-----------------|
| Location | `skills/<name>/SKILL.md` | `patterns/<name>/skills/<name>/SKILL.md` |
| Frontmatter | YAML | YAML |
| Role Detection | Not needed | MANDATORY |
| Workflow logic | In the skill itself | In PATTERN.md (skill dispatches only) |
| CSA integration | Optional | Required (orchestrator calls `csa run --skill`) |
| Recursion risk | None | High (must be guarded) |

## Executor Behavior

When CSA spawns an executor with `--skill <name>`:

1. CSA `pattern_resolver` finds `patterns/<name>/`
2. Reads `skills/<name>/SKILL.md` content
3. Injects it into the executor's initial prompt with `"Use the <name> skill"`
4. Executor sees the literal string, enters executor mode
5. Executor reads `patterns/<name>/PATTERN.md` and follows steps
6. Executor performs all work DIRECTLY (no `csa` delegation)

This is why the "ABSOLUTE PROHIBITION" on `csa` commands exists in executor
mode — the executor IS the final worker, it cannot delegate further.
