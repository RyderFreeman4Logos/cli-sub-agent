---
name: install-update-csa
description: Install, update, and configure CSA — binary check, global config setup, project-level tool selection, model discovery, intelligent tier grouping, and edit restrictions
allowed-tools: Bash, Read, Write, Edit, AskUserQuestion
triggers:
  - "install csa"
  - "update csa"
  - "setup csa"
  - "csa init"
  - "configure csa"
  - "init project"
---

# Install & Update CSA

Install, verify, and configure CSA at both global and project levels.
Discovers installed tools, queries available models, groups them into
capability tiers, and sets tool-specific restrictions.

## When to Use

- First time installing or setting up CSA
- Updating CSA binary to a new version
- Checking or creating global user config (`~/.config/csa/config.toml`)
- First time setting up CSA in a project (project-level config)
- Reconfiguring tool/model selection for changed requirements
- Adding newly installed tools or models

## Output

- CSA binary verified in PATH
- `~/.config/csa/config.toml` — global user configuration (TOML)
- `.csa/config.toml` — project configuration (TOML)
- `.csa/` added to `.gitignore`

---

## Workflow

### Phase 0: Verify CSA Installation

**Step 1**: Check if `csa` binary is in PATH:

```bash
which csa 2>/dev/null && csa --version
```

- If **found**: Display version, proceed to Phase 0.5.
- If **not found**: Provide installation instructions:
  ```
  CSA is not installed. Install options:
  1. From source: `cargo install --path .` (if in the csa repo)
  2. From crates.io: `cargo install cli-sub-agent` (when published)
  3. Manual build: `cargo build --release && cp target/release/csa ~/.cargo/bin/`
  ```
  Ask user if they want to proceed with installation or skip.

**Step 2**: Check if version is current (if a known latest version exists):
```bash
csa --version
```
Report the version. If it looks outdated, suggest updating.

### Phase 0.5: Check Global Config

Check if the global user config exists at `~/.config/csa/config.toml`:

```bash
ls ~/.config/csa/config.toml 2>/dev/null
```

- If **not found**:
  - Inform the user that a global config provides user-level defaults (tiers, tool settings) shared across all projects.
  - Ask if they want to create one now using `AskUserQuestion`.
  - If yes: CSA can generate a default template via `ProjectConfig::save_user_config_template()`, or write a sensible default with common tiers and tool settings.
  - The global config does NOT need a `[project]` section (it applies to all projects).

- If **found**:
  - Display a summary of the current global config (enabled tools, tier count, key settings).
  - Ask if the user wants to update it.

**Key settings to include in global config**:
```toml
# ~/.config/csa/config.toml
# Global defaults — project configs override these values.

[tools.codex]
enabled = true
suppress_notify = true  # Suppress codex desktop notifications when run via CSA

[tools.opencode]
enabled = true

[tools.claude-code]
enabled = true

[tiers.tier-1-quick]
description = "Quick tasks, low cost"
models = [
    "claude-code/anthropic/claude-haiku-4-5/low",
    "codex/openai/gpt-5.1-codex/low",
]

[tiers.tier-2-standard]
description = "Standard development tasks"
models = [
    "claude-code/anthropic/claude-sonnet-4-5/medium",
    "codex/openai/gpt-5.2-codex/medium",
]

[tiers.tier-3-complex]
description = "Complex reasoning, architecture, deep analysis"
models = [
    "claude-code/anthropic/claude-sonnet-4-5/high",
    "codex/openai/gpt-5.3-codex/high",
]

[tiers.tier-4-critical]
description = "Security-critical, deep analysis, final decisions"
models = [
    "claude-code/anthropic/claude-opus-4-6/high",
    "claude-code/anthropic/claude-opus-4-6/xhigh",
]
```

### Phase 1: Detect Installed Tools

```bash
which opencode 2>/dev/null && echo "opencode: installed"
which codex    2>/dev/null && echo "codex: installed"
which claude   2>/dev/null && echo "claude-code: installed"
```

### Phase 2: User Selects Tools

Use `AskUserQuestion` (multiSelect=true):

> **Which tools do you want to enable for this project?**
>
> Context for the user:
> - Low-priority/quick projects: consider excluding `claude-code` and `codex` (higher cost)

Only show tools that are actually installed.

### Phase 3: Discover Available Models

For each **enabled** tool, discover models by running CLI commands with LLM assistance:

| Tool | Discovery Method | Provider |
|------|-----------------|----------|
| opencode | `opencode models` | (parse from output) |
| codex | Read `~/.codex/config.toml` (`model` field) + well-known list | openai |
| claude-code | Known models (no discovery command) | anthropic |

**Note on codex**: Codex CLI has no `--list-models` command. Discover the user's current model from `~/.codex/config.toml`, then supplement with the well-known model list.

**Fallback**: If a discovery command fails or doesn't exist, run `{tool} --help` to find the correct subcommand. If that also fails, use the well-known model list for that tool.

**Well-known models** (fallback reference):

| Tool | Provider | Models |
|------|----------|--------|
| opencode | anthropic | claude-opus-4-6, claude-sonnet-4-5 |
| opencode | google | antigravity-gemini-3-pro, antigravity-gemini-3-flash |
| opencode | openai | gpt-5.3-codex, gpt-5.2-codex, gpt-5.1-codex |
| codex | openai | gpt-5.3-codex, gpt-5.2-codex, gpt-5.1-codex |
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
| claude-code / codex (full sandbox) | Implementation / higher tiers |

**Minimum 3 tiers** (can be more if the model set warrants it):

| Tier | Purpose | Typical Contents |
|------|---------|-----------------|
| `tier-1-quick` | Quick lookups, formatting, simple questions | flash/low, haiku/low |
| `tier-2-standard` | Standard development, routine implementation | pro/medium, sonnet/medium |
| `tier-3-complex` | Architecture design, deep reasoning, security audit | opus/high, pro/high |
| `tier-4-critical` | (Optional) Security audit, critical decisions | opus/xhigh |

Users **can rename tiers, add more tiers, or move model specs between tiers** by editing the TOML in any text editor. The format is designed for easy cut-and-paste.

### Phase 6: Set Tool-Specific Settings

Use `AskUserQuestion` to configure tool-specific settings. Only ask about tools that are **enabled** for this project.

#### 6A: Codex `suppress_notify`

If codex is enabled, ask the user:

> **Suppress codex desktop notifications when running via CSA?**
>
> Context: When CSA spawns codex as a sub-agent, codex's default desktop notifications
> (configured in `~/.codex/config.toml`) can produce excessive noise. Enabling this
> passes `-c 'notify=[]'` to codex at launch, overriding its notification settings.

| Option | Effect |
|--------|--------|
| `suppress_notify = true` (Recommended) | CSA passes `-c 'notify=[]'` to codex, silencing desktop notifications |
| `suppress_notify = false` | Codex uses its own `~/.codex/config.toml` notification settings |

Default: **`true`** (recommended for most users since CSA is non-interactive).


### Phase 7: Generate Config

Write `.csa/config.toml` with clear section comments for easy editing:

```toml
[project]
name = "my-project"
created_at = "2026-02-06T10:00:00Z"
max_recursion_depth = 5

# ─── Tool Selection ───────────────────────────────────────────
# enabled = true/false to toggle tools for this project

[tools.opencode]
enabled = false

[tools.codex]
enabled = true
suppress_notify = true  # Suppress codex desktop notifications when run via CSA

[tools.claude-code]
enabled = true

# ─── Resource Limits ─────────────────────────────────────────
# min_free_memory_mb: combined (physical + swap) free memory threshold

[resources]
min_free_memory_mb = 4096

# ─── Model Tiers ─────────────────────────────────────────────
# Format: "tool/provider/model/thinking_budget"
#
# To adjust: move lines between [tiers.*] sections.
# To add a tier: create a new [tiers.tier-N-name] section.
# To remove a model: delete the line.

[tiers.tier-1-quick]
description = "Quick tasks, low cost"
models = [
    "claude-code/anthropic/claude-haiku-4-5/low",
    "codex/openai/gpt-5.1-codex/low",
]

[tiers.tier-2-standard]
description = "Standard development tasks"
models = [
    "claude-code/anthropic/claude-sonnet-4-5/medium",
    "codex/openai/gpt-5.2-codex/medium",
]

[tiers.tier-3-complex]
description = "Complex reasoning, architecture, deep analysis"
models = [
    "claude-code/anthropic/claude-sonnet-4-5/high",
    "codex/openai/gpt-5.3-codex/high",
]

[tiers.tier-4-critical]
description = "Security-critical, deep analysis, final decisions"
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
security = "tier-4-critical"

# ─── Aliases ─────────────────────────────────────────────────
# Shorthand names for frequently used model specs

[aliases]
fast = "codex/openai/gpt-5.1-codex/low"
default = "claude-code/anthropic/claude-sonnet-4-5/medium"
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

