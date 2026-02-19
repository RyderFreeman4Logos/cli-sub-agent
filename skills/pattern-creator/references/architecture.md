# Pattern Architecture Reference

## Directory Layout

```
patterns/<name>/
├── PATTERN.md                    # Workflow definition
│   ├── TOML frontmatter          # name, description, allowed-tools, tier, version
│   └── Markdown body             # Steps, control flow, tool annotations
│
├── .skill.toml                   # Agent configuration sidecar
│   ├── [skill] name, version
│   └── [agent] tier, max_turns, token_budget, tools, skip_context, extra_context
│
├── workflow.toml                  # Machine-readable step/variable manifest
│   ├── [plan] name, description
│   ├── [[plan.variables]]        # All ${VAR} used in PATTERN.md
│   └── [[plan.steps]]            # Structured step definitions
│
└── skills/
    └── <name>/                   # Companion skill (MUST match pattern name)
        └── SKILL.md              # Entry point for orchestrators
```

## The Companion Skill Mechanism

### Why It Exists

Tools like Claude Code and Codex only discover capabilities through skill
directories (`.claude/skills/<name>/SKILL.md`). They do not understand
PATTERN.md files directly. The companion skill bridges this gap:

```
User types /commit
    ↓
Claude Code finds .claude/skills/commit/SKILL.md (via symlink)
    ↓
SKILL.md "Execution Protocol" says: csa run --skill commit "..."
    ↓
CSA pattern_resolver finds patterns/commit/PATTERN.md
    ↓
CSA spawns executor agent with SKILL.md content injected
    ↓
Executor reads PATTERN.md and follows steps
```

### Recursion Prevention

The companion skill is NOT the workflow itself. If the executor (spawned by CSA)
were to run `csa run --skill commit`, it would spawn another executor, which
would spawn another, creating infinite recursion.

**Mandatory guard**: Every companion skill MUST include:

```markdown
## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string
`"Use the <name> skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator.
2. **Read the pattern** at `patterns/<name>/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`,
   or ANY `csa` command. You must perform the work DIRECTLY.
```

### What Companion Skill Contains

| Section | For Whom | Purpose |
|---------|----------|---------|
| Role Detection | Both | Determines orchestrator vs executor path |
| Purpose | Both | What the pattern does (brief) |
| Execution Protocol | Orchestrator only | How to invoke via `csa run --skill` |
| Quick Start | Orchestrator only | One-liner command |
| Step-by-Step | Orchestrator only | Numbered summary of workflow |
| Example Usage | Both | Command examples with expected effects |
| Integration | Both | Dependencies and consumers |
| Done Criteria | Both | Mechanically verifiable completion conditions |

## Symlink Routing & Installation Scopes

### How Weave Auto-Links

After `weave install`, the `link.rs` module:

1. Reads `weave.lock` to find installed packages
2. Scans each package's `patterns/*/skills/*/SKILL.md`
3. Creates relative symlinks in `.claude/skills/` (project) or `~/.claude/skills/` (user)

```
.claude/skills/commit -> ../../patterns/commit/skills/commit/
```

### Installation Scopes

| Scope | Symlink Location | Points To | Effect |
|-------|------------------|-----------|--------|
| Project | `.claude/skills/<name>` | `../../patterns/<name>/skills/<name>/` | Only this project sees the pattern |
| User | `~/.claude/skills/<name>` | `/abs/path/to/patterns/<name>/skills/<name>/` | All projects see the pattern |

### Symlink Renaming

The symlink itself can be renamed to avoid naming conflicts:

```bash
# Original: /commit triggers commit pattern
ln -s ../../patterns/commit/skills/commit/ .claude/skills/commit

# Renamed: /my-commit triggers commit pattern
ln -s ../../patterns/commit/skills/commit/ .claude/skills/my-commit
```

**Rules**:
- The symlink NAME determines the trigger command (`/my-commit`)
- The symlink TARGET must point to a companion skill directory whose name
  matches the pattern name
- CSA pattern resolver uses the companion skill's directory name (not the
  symlink name) to locate `patterns/<name>/PATTERN.md`

### Scope Switching

To switch a pattern from project-level to user-level:

```bash
# Remove project-level
rm .claude/skills/commit

# Add user-level (absolute path required)
ln -s /home/user/project/patterns/commit/skills/commit/ ~/.claude/skills/commit
```

This is useful when:
- Multiple projects need the same pattern version
- User wants to override a project pattern with a personal variant
- Testing a pattern globally before committing to a project

## CSA Resolution Order

### Pattern Resolver (`pattern_resolver.rs`)

When CSA receives `--skill <name>`:

1. `.csa/patterns/<name>/` — Project-local fork (custom override)
2. `patterns/<name>/` — Repo-shipped patterns
3. `<global_store>/<pkg>/<commit>/patterns/<name>/` — Weave global store

### Skill Resolver (`skill_resolver.rs`)

When CSA receives a skill name (no pattern found):

1. `.csa/skills/<name>/` — Project-local
2. `~/.config/cli-sub-agent/skills/<name>/` — Global user
3. `<global_store>/<pkg>/<commit>/` — Weave global store

### Config Cascade (`.skill.toml`)

Three-tier merge (later overrides earlier):

1. **Package-embedded**: `patterns/<name>/.skill.toml`
2. **User-level**: `~/.config/cli-sub-agent/patterns/<name>.toml`
3. **Project-level**: `.csa/patterns/<name>.toml` (file, not directory)

Tables are deep-merged; scalar values are replaced.

## Discovery: How Tools Find Patterns

```
Claude Code                         CSA
-----------                         ---
.claude/skills/<name>/SKILL.md      pattern_resolver searches:
  ↑ symlink                           1. .csa/patterns/<name>/
  |                                   2. patterns/<name>/
patterns/<name>/skills/<name>/        3. weave global store
```

Claude Code sees skills. CSA sees patterns. The companion skill is the bridge.
