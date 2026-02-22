# Skills & Patterns

CSA uses a two-layer system for packaging and composing agent behaviors:
**Skills** (atomic units) and **Patterns** (composed workflows).

## Concepts

| Term | Definition |
|------|------------|
| **Skill** | An atomic agent definition: prompt + tools + protocol, packaged as `SKILL.md` + `.skill.toml` |
| **Pattern** | A composed workflow written in skill-lang (`PATTERN.md`), compiled to a deterministic execution plan (`workflow.toml`) |
| **Loom** | A git repository that publishes skills and patterns |
| **Weave** | The skill-lang compiler that transforms `PATTERN.md` into `workflow.toml` |

## Skills

### Structure

A skill is a directory containing:

```
skills/my-skill/
  +-- SKILL.md          # Agent instructions (markdown with YAML frontmatter)
  +-- .skill.toml       # Optional: context config, tool restrictions
  +-- references/       # Optional: additional reference docs
```

### SKILL.md Format

```markdown
---
name: csa
description: Unified CLI interface for AI tools
allowed-tools: Bash, Read, Grep, Glob
---

# Skill Title

Instructions for the agent...

## Section

Detailed behavior specification...
```

The YAML frontmatter declares metadata: `name`, `description`,
`allowed-tools`, and optionally `version` and `dependencies`.

### .skill.toml

Controls how the skill loads into an agent's context:

```toml
[context]
no_load = ["CLAUDE.md", "AGENTS.md"]   # Skip default files
extra_load = ["./rules/security.md"]   # Load additional files

[tools]
allowed = ["Bash", "Read", "Grep"]     # Tool restrictions
```

### Installing Skills

```bash
# Install from a GitHub repository (loom)
csa skill install user/repo

# Install for a specific tool
csa skill install user/repo --target codex

# List installed skills
csa skill list
```

Skills from a loom are installed into the project's `.claude/skills/`
(for Claude Code) or equivalent tool-specific directories.

### Bundled Skills

CSA ships with these skills in the `skills/` directory:

| Skill | Description |
|-------|-------------|
| `csa` | Core CSA usage instructions for AI agents |
| `pattern-creator` | Guide for developing new skill-lang patterns |

Additional skills are published via separate loom repositories.

## Patterns (skill-lang)

### What is skill-lang?

skill-lang is a structured Markdown convention for defining multi-step
agent workflows. The "compiler" is the AI tool itself (`weave compile`),
and the runtime is CSA (`csa plan run`).

### PATTERN.md Syntax

```markdown
# Pattern Name

Description of the workflow.

## Step 1: Analyze

Analyze the codebase for issues.

Tool: gemini-cli
Tier: tier-1-quick

## Step 2: Fix

IF ${ISSUES_FOUND}
Fix the identified issues.

Tool: codex
Tier: tier-2-standard
ELSE
Report clean status.
ENDIF

## Step 3: Review

FOR reviewer IN ${REVIEWERS}
Review changes from perspective of ${reviewer}.
ENDFOR
```

### Syntax Elements

| Element | Description |
|---------|-------------|
| `## Step N: Title` | Step definition with sequential numbering |
| `Tool:` | Hint line specifying which tool to use |
| `Tier:` | Hint line specifying which tier for tool/model selection |
| `OnFail:` | Hint line specifying failure behavior |
| `IF/ELSE/ENDIF` | Conditional execution |
| `FOR/IN/ENDFOR` | Loop over a list |
| `INCLUDE` | Include another pattern |
| `${VAR}` | Variable substitution |

### Compiling Patterns

```bash
# Compile a pattern to workflow.toml
weave compile PATTERN.md

# The output is a deterministic execution plan
cat workflow.toml
```

### workflow.toml

The compiled output is a TOML file that CSA's plan runner executes:

```toml
[metadata]
name = "my-pattern"
version = "1.0.0"

[[steps]]
name = "Analyze"
tool = "gemini-cli"
tier = "tier-1-quick"
prompt = "Analyze the codebase for issues."

[[steps]]
name = "Fix"
tool = "codex"
tier = "tier-2-standard"
prompt = "Fix the identified issues."
condition = "${ISSUES_FOUND}"
```

### Running Workflows

```bash
# Execute a compiled workflow
csa plan run workflow.toml

# With variable overrides
csa plan run workflow.toml --var REVIEWERS="security,performance"

# Override tool for all steps
csa plan run workflow.toml --tool codex

# Dry run (show plan without executing)
csa plan run workflow.toml --dry-run
```

### Step Output Forwarding

Steps can consume output from previous steps. CSA forwards step output
as context to subsequent steps, enabling multi-step pipelines where
analysis informs implementation.

## Weave Global Registry

Weave manages patterns through a lockfile-based registry:

- **`weave.lock`** -- tracks installed pattern versions per project
- **Global store** -- `~/.local/share/weave/` caches downloaded loom repos
- **Config cascade** -- project `weave.lock` overrides global defaults
- **Auto-link** -- `weave` auto-links companion skills when installing patterns

### Registry Commands

```bash
weave compile PATTERN.md        # Compile a pattern
weave install user/repo         # Install from a loom
weave list                      # List installed patterns
```

## Prompt Guards

While not skills per se, prompt guards complement the skill system by
injecting runtime context into tool prompts. See [Hooks](hooks.md) for
the `[[prompt_guard]]` configuration.

## Related

- [Commands](commands.md) -- `csa skill`, `csa plan`, `weave` reference
- [Hooks](hooks.md) -- prompt guard system
- [Configuration](configuration.md) -- tier definitions used by patterns
