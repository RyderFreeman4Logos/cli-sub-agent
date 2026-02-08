---
name: CSA Init (Project Configuration)
description: Interactive project configuration wizard for CSA — tool selection, model discovery via CLI commands, intelligent tier grouping, and edit restrictions
allowed-tools: Bash, Read, Write, Edit, AskUserQuestion
---

# CSA Init: Project Configuration Wizard

Interactively initialize `.csa/config.toml` for a project.
Discovers installed tools, queries available models, groups them into
capability tiers, and sets tool-specific restrictions (e.g., gemini-cli edit ban).

## When to Use

- First time setting up CSA in a project
- Reconfiguring tool/model selection for changed requirements
- Adding newly installed tools or models

## Output

- `.csa/config.toml` — project configuration (TOML)
- `.csa/` added to `.gitignore`

---

## Workflow

### Phase 1: Detect Installed Tools

```bash
which gemini   2>/dev/null && echo "gemini-cli: installed"
which opencode 2>/dev/null && echo "opencode: installed"
which codex    2>/dev/null && echo "codex: installed"
which claude   2>/dev/null && echo "claude-code: installed"
```

### Phase 2: User Selects Tools

Use `AskUserQuestion` (multiSelect=true):

> **Which tools do you want to enable for this project?**
>
> Context for the user:
> - Important/sensitive projects: consider excluding `gemini-cli` (higher risk of deleting comments/code when editing)
> - Low-priority/quick projects: consider excluding `claude-code` and `codex` (higher cost)

Only show tools that are actually installed.

### Phase 3: Discover Available Models

For each **enabled** tool, discover models by running CLI commands with LLM assistance:

| Tool | Discovery Command | Provider |
|------|-------------------|----------|
| gemini-cli | `gemini models list` | google |
| opencode | `opencode models` | (parse from output) |
| codex | `codex --list-models` | (parse from output) |
| claude-code | Known: opus-4-6, sonnet-4-5, haiku-4-5 | anthropic |

**Fallback**: If a discovery command fails or doesn't exist, run `{tool} --help` to find the correct subcommand. If that also fails, use the well-known model list for that tool.

**Well-known models** (fallback reference):

| Tool | Provider | Models |
|------|----------|--------|
| gemini-cli | google | gemini-3-pro-preview, gemini-3-flash-preview, gemini-2.5-pro, gemini-2.5-flash |
| opencode | anthropic | claude-opus-4-6, claude-sonnet-4-5 |
| opencode | google | antigravity-gemini-3-pro, antigravity-gemini-3-flash |
| opencode | openai | gpt-5.2-codex, gpt-5.1-codex |
| codex | anthropic | claude-opus-4-6, claude-sonnet-4-5 |
| codex | openai | gpt-5.2-codex, gpt-5.1-codex |
| claude-code | anthropic | claude-opus-4-6, claude-sonnet-4-5, claude-haiku-4-5 |

### Phase 3.5: Filter Providers and Models

Tools like `opencode` return 80+ models across many providers. **The agent MUST NOT guess** which providers/models the user wants. Instead:

1. **Group discovered models by provider** (e.g., anthropic, google, openai, groq, xai, opencode-native)
2. **Ask user which providers to include** using `AskUserQuestion` (multiSelect=true):

> **Which providers do you want to use with {tool}?**
>
> Context for the user:
> - If you already have `claude-code` enabled, you may not need anthropic models via opencode
> - Free/groq models are fast but less capable
> - Google antigravity models are Google-hosted variants of other providers' models

3. **Within selected providers**, the agent filters to the **latest generation** models only:
   - Prefer latest version (e.g., gemini-3 over gemini-2.5, opus-4-6 over opus-4-1)
   - Exclude deprecated/preview-old models
   - Exclude embedding/TTS/vision-only models
   - Keep at most 2-3 models per provider (strongest + fastest)

4. **Show final selection** to user for confirmation before proceeding to expansion.

**Why this step is critical**: Without provider filtering, the agent picks models based on guesswork. Users have strong preferences about which providers to route through which tools (cost, latency, API key management, trust level).

### Phase 4: Expand Model Specs

For each discovered model, generate specs with multiple thinking budgets.
**Different thinking budgets of the same base model are treated as different models.**

Format: `{tool}/{provider}/{model}/{thinking_budget}`

Thinking budget values: `low`, `medium`, `high`, `xhigh`

Example expansion for one base model:
```
opencode/anthropic/claude-sonnet-4-5/low
opencode/anthropic/claude-sonnet-4-5/medium
opencode/anthropic/claude-sonnet-4-5/high
```

**Not every model needs all budgets.** Use judgment:
- Flash/haiku models: `low`, `medium` only (high budget wastes money on weak models)
- Sonnet/pro models: `low`, `medium`, `high`
- Opus models: `high`, `xhigh` only (low budget wastes a strong model)

### Phase 5: Intelligent Tier Grouping

Group all expanded model specs into tiers. The agent **must decide grouping intelligently** based on these signals:

| Signal | Tier Direction |
|--------|---------------|
| Flash / haiku base model | Lower tier |
| Sonnet / pro base model | Middle tier |
| Opus base model | Higher tier |
| Low thinking budget | Lower tier |
| High / xhigh budget | Higher tier |
| gemini-cli (read-only preferred) | Analysis / lower tiers |
| claude-code / codex (full sandbox) | Implementation / higher tiers |

**Minimum 3 tiers** (can be more if the model set warrants it):

| Tier | Purpose | Typical Contents |
|------|---------|-----------------|
| `tier-1-quick` | Quick lookups, formatting, simple questions | flash/low, haiku/low |
| `tier-2-standard` | Standard development, routine implementation | pro/medium, sonnet/medium |
| `tier-3-complex` | Architecture design, deep reasoning, security audit | opus/high, pro/high |
| `tier-4-critical` | (Optional) Security audit, critical decisions | opus/xhigh |

Users **can rename tiers, add more tiers, or move model specs between tiers** by editing the TOML in any text editor. The format is designed for easy cut-and-paste.

### Phase 6: Set Tool Restrictions

| Restriction | Default | Rationale |
|-------------|---------|-----------|
| `gemini-cli.restrictions.allow_edit_existing_files` | **`false`** | Significantly higher probability of deleting comments, annotations, and even functional code when editing existing files |
| All other tools `.restrictions.allow_edit_existing_files` | `true` | Normal editing behavior |

**Why gemini-cli defaults to no-edit:**
gemini-cli has a well-documented tendency to:
- Delete comments written by other tools or humans
- Remove annotations and metadata during "cleanup"
- Occasionally delete functional code it deems unnecessary
- Silently drop important context from edited files

Restricting it to **new file creation and read-only analysis** eliminates these risks while preserving its strengths (large context, fast analysis, low cost).

### Phase 7: Generate Config

Write `.csa/config.toml` with clear section comments for easy editing:

```toml
[project]
name = "my-project"
created_at = "2026-02-06T10:00:00Z"
max_recursion_depth = 5

# ─── Tool Selection ───────────────────────────────────────────
# enabled = true/false to toggle tools for this project

[tools.gemini-cli]
enabled = true

[tools.gemini-cli.restrictions]
# gemini-cli has significantly higher probability of deleting
# comments and code when editing existing files. Default: false.
allow_edit_existing_files = false

[tools.opencode]
enabled = true

[tools.codex]
enabled = false

[tools.claude-code]
enabled = true

# ─── Resource Limits ─────────────────────────────────────────

[resources]
min_free_memory_mb = 512
min_free_swap_mb = 256

# ─── Model Tiers ─────────────────────────────────────────────
# Format: "tool/provider/model/thinking_budget"
#
# To adjust: move lines between [tiers.*] sections.
# To add a tier: create a new [tiers.tier-N-name] section.
# To remove a model: delete the line.

[tiers.tier-1-quick]
description = "Quick tasks, low cost"
models = [
    "gemini-cli/google/gemini-2.0-flash/low",
    "gemini-cli/google/gemini-2.5-flash/low",
    "opencode/anthropic/claude-haiku-4-5/low",
]

[tiers.tier-2-standard]
description = "Standard development tasks"
models = [
    "opencode/anthropic/claude-sonnet-4-5/medium",
    "gemini-cli/google/gemini-2.5-pro/medium",
]

[tiers.tier-3-complex]
description = "Complex reasoning, architecture"
models = [
    "claude-code/anthropic/claude-sonnet-4-5/high",
    "opencode/anthropic/claude-sonnet-4-5/high",
    "gemini-cli/google/gemini-2.5-pro/high",
]

[tiers.tier-4-critical]
description = "Security-critical, deep analysis"
models = [
    "claude-code/anthropic/claude-opus-4-6/high",
    "claude-code/anthropic/claude-opus-4-6/xhigh",
]

# ─── Task-to-Tier Mapping ────────────────────────────────────
# Which tier to use for each task type

[tier_mapping]
analysis = "tier-1-quick"
implementation = "tier-2-standard"
architecture = "tier-3-complex"
security = "tier-3-complex"  # or "tier-4-critical" if defined

# ─── Aliases ─────────────────────────────────────────────────
# Shorthand names for frequently used model specs

[aliases]
fast = "gemini-cli/google/gemini-2.0-flash/low"
default = "opencode/anthropic/claude-sonnet-4-5/medium"
heavy = "claude-code/anthropic/claude-opus-4-6/high"
```

### Phase 8: Gitignore

Add `.csa/` to `.gitignore` if not already present:

```bash
grep -qxF '.csa/' .gitignore 2>/dev/null || echo '.csa/' >> .gitignore
```

### Phase 9: Open in Editor

After generating, offer to open the config in `$EDITOR` for manual adjustment:

```bash
${EDITOR:-vi} .csa/config.toml
```

This lets the user:
- Rename tiers to project-specific names
- Move model specs between tiers
- Add custom tiers (e.g., `tier-5-paranoid` for ultra-critical code)
- Adjust aliases
- Toggle tool restrictions

---

## Design Rationale

### Different Thinking Budgets = Different Models

The same base model with different thinking budgets behaves very differently:

| Spec | Behavior |
|------|----------|
| `sonnet/low` | Fast, surface-level responses — good for formatting, lookups |
| `sonnet/medium` | Balanced — standard development work |
| `sonnet/high` | Deep reasoning — complex refactoring, bug investigation |

These **must** be treated as separate entries for tier assignment. A `sonnet/high` belongs in a higher tier than `sonnet/low`, even though the base model is the same.

### Why >= 3 Tiers

Two tiers (fast/heavy) are too coarse. Three tiers (quick/standard/complex) cover most projects well. Four tiers add a `critical` level for security-sensitive codebases.

| Tiers | Best For |
|-------|----------|
| 3 tiers | Most projects — quick/standard/complex covers daily workflow |
| 4+ tiers | Security-critical projects needing a dedicated audit tier |

Users can always add more tiers. The TOML format makes this trivial — just add a new `[tiers.tier-N-name]` section.

### Why gemini-cli Default No-Edit

This is **not optional**. gemini-cli has a significantly higher probability of destructive edits compared to other tools:

1. **Comment deletion**: Silently removes comments written by humans or other tools
2. **Code deletion**: Occasionally removes functional code it considers "unnecessary"
3. **Annotation loss**: Strips metadata, type annotations, safety comments
4. **Context amnesia**: Doesn't preserve surrounding context when editing

Default `allow_edit_existing_files = false` restricts gemini-cli to:
- Reading and analyzing existing files (safe)
- Creating entirely new files from scratch (safe)
- Running commands and tools (safe)

It **cannot** modify files that already have content. Users can override this per-project if they accept the risk.
