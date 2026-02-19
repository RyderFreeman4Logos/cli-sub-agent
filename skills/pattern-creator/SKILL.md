---
name: pattern-creator
description: "Guide for creating CSA patterns — composable, auditable workflow definitions with companion skill entry points. Use when users want to create a new pattern, convert an existing skill into a pattern, or understand pattern architecture (PATTERN.md, companion skill, .skill.toml, plan.toml, weave linking, installation scope)."
---

# Pattern Creator

Create CSA patterns: composable workflow definitions that orchestrate multi-step
agent work through structured Markdown documents.

## Patterns vs Skills — When to Use Which

| Aspect | Skill (SKILL.md) | Pattern (PATTERN.md + companion skill) |
|--------|-------------------|----------------------------------------|
| Purpose | Static knowledge / persona | Executable workflow with steps |
| Triggered by | Claude Code auto-match | `/name` command or `csa run --skill name` |
| Has steps | No (free-form instructions) | Yes (`## Step N:` structure) |
| Control flow | None | `## IF`, `## FOR`, `## INCLUDE` |
| Recursion safety | N/A | Role Detection mandatory |
| Config cascade | N/A | `.skill.toml` (package → user → project) |
| Composability | N/A | `## INCLUDE other-pattern` |

**Rule**: If the workflow has more than 2 sequential steps with tool invocations,
it should be a pattern, not a plain skill.

## Pattern Architecture (Progressive Loading)

Read `references/architecture.md` for the full directory layout, companion skill
mechanism, symlink routing, and installation scopes.

**Key concepts (always in context):**

1. **Companion Skill** — `patterns/<name>/skills/<name>/SKILL.md` is the entry
   point that Claude Code / Codex discovers. It does NOT execute the workflow
   itself — it dispatches to the PATTERN.md. This prevents infinite recursion.

2. **Role Detection** — Every companion skill MUST include a "Role Detection"
   section that checks whether the agent is the orchestrator (human user invoked
   `/name`) or the executor (CSA spawned with `"Use the <name> skill"`).

3. **Symlink Routing** — Weave creates symlinks in `.claude/skills/` pointing
   to the companion skill directory. The symlink can be renamed (to avoid
   collisions) as long as it points to the correct companion skill. Changing
   the symlink target from project-level to global-level determines which
   PATTERN.md gets executed.

## Creation Process

### Step 1: Define Scope

Before writing anything, answer:

1. What problem does this pattern solve?
2. What are the steps (draft a numbered list)?
3. Which steps need `csa` (delegation) vs `bash` (local execution)?
4. Does it compose with existing patterns (`## INCLUDE`)?
5. What variables does it need (`${VAR_NAME}`)?

### Step 2: Scaffold

Create the directory structure:

```
patterns/<name>/
├── PATTERN.md              # Workflow definition (TOML frontmatter + steps)
├── .skill.toml             # Agent config (tier, tools, token_budget)
├── plan.toml               # Machine-readable step+variable manifest
└── skills/
    └── <name>/
        └── SKILL.md        # Companion skill (YAML frontmatter, entry point)
```

Read `references/architecture.md` for detailed specs of each file.

### Step 3: Write PATTERN.md

Read `references/pattern-syntax.md` for the full syntax reference:
frontmatter format, step structure, control flow directives, tool annotations,
tier annotations, and composition via `## INCLUDE`.

### Step 4: Write Companion Skill

Read `references/companion-skill.md` for the template and rules:
Role Detection block, Execution Protocol, Done Criteria, and the absolute
prohibition on `csa` commands inside executor mode.

### Step 5: Write .skill.toml

```toml
[skill]
name = "<pattern-name>"
version = "0.1.0"

[agent]
tier = "tier-2-standard"       # tier-1-quick | tier-2-standard | tier-3-complex
max_turns = 30                 # advisory, not enforced
tools = [{ tool = "auto" }]    # or explicit [{ tool = "codex" }, { tool = "claude-code" }]
```

### Step 6: Write plan.toml

```toml
[plan]
name = "<pattern-name>"
description = "One-line description"

[[plan.variables]]
name = "VAR_NAME"
# Repeat for each variable used in PATTERN.md

[[plan.steps]]
id = 1
title = "Step Title"
prompt = "What this step does"
tool = "bash"          # optional: bash | csa | omit for orchestrator logic
on_fail = "abort"      # "abort" | "skip" | { retry = N } | { delegate = "target" }
tier = "tier-1-quick"  # optional tier override
condition = "${VAR}"   # optional: only run if truthy
```

### Step 7: Install

**Project-level** (default):
```bash
# Weave auto-links after install:
weave install

# Or manual symlink:
ln -s ../../patterns/<name>/skills/<name>/ .claude/skills/<name>
```

**User-level** (global):
```bash
ln -s /path/to/patterns/<name>/skills/<name>/ ~/.claude/skills/<name>
```

**Renamed symlink** (avoid collisions):
```bash
# Symlink name can differ from pattern name
ln -s ../../patterns/<name>/skills/<name>/ .claude/skills/my-custom-alias
```
The symlink name determines the trigger (`/my-custom-alias`), but CSA pattern
resolver follows the companion skill's actual directory name back to the pattern.

### Step 8: Test

1. Invoke as orchestrator: `/name` — should dispatch to CSA
2. Invoke as executor: `csa run --skill name "prompt"` — should read PATTERN.md
3. Verify Role Detection: executor must NOT run `csa` commands
4. Verify `## INCLUDE` resolves correctly (if used)

## Quick Reference

| File | Format | Required | Purpose |
|------|--------|----------|---------|
| `PATTERN.md` | TOML frontmatter + Markdown | Yes | Workflow definition |
| `skills/<name>/SKILL.md` | YAML frontmatter + Markdown | Yes | Entry point for Claude Code |
| `.skill.toml` | TOML | Recommended | Agent config (tier, tools, budget) |
| `plan.toml` | TOML | Recommended | Machine-readable manifest |
