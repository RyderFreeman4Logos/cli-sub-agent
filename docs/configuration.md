# Configuration

CSA uses TOML-based configuration with a two-level merge: global defaults
and per-project overrides.

## Configuration Precedence

```
Global config (~/.config/cli-sub-agent/config.toml)
  | lowest priority
Project config ({PROJECT_ROOT}/.csa/config.toml)
  | higher priority
CLI arguments (--tool, --model, --thinking, etc.)
  | highest priority
Final merged config
```

## File Locations

| File | Purpose |
|------|---------|
| `~/.config/cli-sub-agent/config.toml` | Global: API keys, concurrency limits, tool defaults |
| `{PROJECT_ROOT}/.csa/config.toml` | Project: tiers, aliases, tool restrictions |

**Initialization:** `csa init` creates the project config. Variants:

- `csa init` -- minimal config with `[project]` metadata only
- `csa init --full` -- auto-detect tools, generate tier configs
- `csa init --template` -- fully-commented reference config

## Global Config

```toml
# ~/.config/cli-sub-agent/config.toml

[defaults]
max_concurrent = 3
tool = "claude-code"             # Fallback for --tool auto

[review]
tool = "auto"                    # Enforce heterogeneous review

[debate]
tool = "auto"                    # Enforce heterogeneous debate
timeout_secs = 1800              # 30 minute default

[tools.codex]
max_concurrent = 5
[tools.codex.env]
OPENAI_API_KEY = "sk-..."

[tools.claude-code]
max_concurrent = 3

[todo]
show_command = "bat -l md --paging=always"
diff_command = "delta"
```

## Project Config

### `[project]` -- Metadata

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | String | required | Human-readable project name |
| `created_at` | DateTime | auto | ISO 8601 creation timestamp |
| `max_recursion_depth` | Integer | 5 | Maximum recursive sub-agent depth |

### `[resources]` -- Resource Limits

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `min_free_memory_mb` | Integer | 4096 | Minimum combined free memory (physical + swap) |

```toml
[resources]
min_free_memory_mb = 4096

[resources.initial_estimates]
gemini-cli = 1024       # MB, used until P95 data available
codex = 2048
opencode = 1536
claude-code = 2048
```

See [Resource Control](resource-control.md) for P95 estimation details.

### `[tools.{name}]` -- Tool Configuration

```toml
[tools.gemini-cli]
enabled = true

[tools.gemini-cli.restrictions]
allow_edit_existing_files = false    # Inject read-only restriction into prompt
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | Boolean | `true` | Whether this tool is available |
| `restrictions.allow_edit_existing_files` | Boolean | `true` | Allow modifying existing files |

Unconfigured tools default to enabled with no restrictions. Setting
`enabled = false` excludes the tool from tier resolution and auto mode.

### `[review]` -- Review Tool Selection

```toml
[review]
tool = "auto"    # or "codex", "claude-code", "gemini-cli", "opencode"
```

Overrides the global review tool for this project. In `auto` mode, CSA
enforces heterogeneity based on parent tool detection.

### `[tiers.{name}]` -- Model Tiers

Tiers group models by quality/cost/speed for automatic selection:

```toml
[tiers.tier-1-quick]
description = "Quick tasks (low thinking budget)"
models = [
    "gemini-cli/google/gemini-3-flash-preview/low",
    "opencode/google/gemini-2.5-pro/minimal",
]

[tiers.tier-2-standard]
description = "Standard development work"
models = [
    "codex/anthropic/claude-sonnet/medium",
    "gemini-cli/google/gemini-3-pro-preview/medium",
]

[tiers.tier-3-complex]
description = "Complex reasoning, security audits"
models = [
    "codex/anthropic/claude-opus/high",
    "claude-code/anthropic/claude-opus/xhigh",
]
```

**Model spec format:** `tool/provider/model/thinking_budget`

Thinking budget values: `low`, `medium`, `high`, `xhigh`, or a custom
token count.

**Selection logic:** Iterate models in order, return the first whose
tool is enabled.

### `[tier_mapping]` -- Task to Tier Mapping

```toml
[tier_mapping]
default = "tier-2-standard"
quick = "tier-1-quick"
analysis = "tier-2-standard"
code-review = "tier-2-standard"
complex-reasoning = "tier-3-complex"
security = "tier-3-complex"
```

### `[aliases]` -- Model Aliases

Shorthand names for frequently used model specs:

```toml
[aliases]
fast = "gemini-cli/google/gemini-3-flash-preview/low"
smart = "codex/anthropic/claude-opus/xhigh"
balanced = "codex/anthropic/claude-sonnet/medium"
```

Usage: `csa run --model fast "quick check"`

## Configuration Commands

```bash
csa config show                  # Show effective merged config
csa config get review.tool       # Query a single key
csa config get tools.codex.enabled --default true
csa config edit                  # Open project config in $EDITOR
csa config validate              # Validate config syntax and references
csa tiers list                   # View tier definitions
```

## Configuration Validation

CSA validates on load:

1. **Model spec format:** Each model must be `tool/provider/model/budget`
2. **Tier references:** All `tier_mapping` values must reference existing tiers
3. **Tool names:** Must be one of `gemini-cli`, `codex`, `opencode`, `claude-code`
4. **Thinking budget:** Must be `low`, `medium`, `high`, `xhigh`, or a number

## Migrations

Config schema evolves between CSA versions. The migration system handles
this automatically:

```bash
csa migrate --status     # Check pending migrations
csa migrate --dry-run    # Preview changes
csa migrate              # Apply pending migrations
```

`weave.lock` version alignment is checked on startup. If outdated,
CSA prints a warning: `Run 'csa migrate' to update`.

## Examples

### Research Project (Read-Only)

```toml
[project]
name = "research-analysis"
max_recursion_depth = 3

[tools.gemini-cli]
enabled = true
[tools.gemini-cli.restrictions]
allow_edit_existing_files = false

[tools.codex]
enabled = false
[tools.claude-code]
enabled = false

[tiers.analysis]
description = "Read-only analysis"
models = ["gemini-cli/google/gemini-3-pro-preview/medium"]

[tier_mapping]
default = "analysis"
```

### Cost-Conscious Setup

```toml
[project]
name = "budget-project"

[tools.codex]
enabled = false        # Anthropic models disabled
[tools.claude-code]
enabled = false

[tiers.primary]
description = "Google models only"
models = [
    "gemini-cli/google/gemini-3-flash-preview/low",
    "opencode/google/gemini-2.5-pro/medium",
]

[tier_mapping]
default = "primary"
```

## Troubleshooting

| Problem | Solution |
|---------|----------|
| "OOM Risk Prevention" error | Reduce `min_free_memory_mb` or wait for agents to finish |
| "Tool 'codex' is not enabled" | Set `tools.codex.enabled = true` or remove section |
| "No enabled tools found in tier" | Ensure at least one tool in the tier's models is enabled |
| Tier resolution always falls back | Check that `tier_mapping.default` exists |

## Related

- [Getting Started](getting-started.md) -- initial setup
- [Resource Control](resource-control.md) -- memory limits and P95 estimation
- [Commands](commands.md) -- `csa config` reference
