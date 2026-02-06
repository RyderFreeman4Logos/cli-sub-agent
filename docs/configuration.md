# Configuration Reference

CSA uses TOML-based configuration stored in `.csa/config.toml` within the project root.

## Configuration File Location

**Path:** `{PROJECT_ROOT}/.csa/config.toml`

**Initialization:** Created automatically by `csa init` command.

## Complete Configuration Schema

```toml
[project]
name = "my-project"
created_at = 2024-02-06T10:00:00Z
max_recursion_depth = 5  # Default: 5

[resources]
min_free_memory_mb = 2048      # Default: 2048
min_free_swap_mb = 1024        # Default: 1024

[resources.initial_estimates]
gemini-cli = 1024    # Initial memory estimate in MB
codex = 2048
opencode = 1536
claude-code = 2048

[tools.gemini-cli]
enabled = true

[tools.gemini-cli.restrictions]
allow_edit_existing_files = false
allowed_operations = ["read", "analyze", "create"]

[tools.codex]
enabled = true

[tools.opencode]
enabled = true

[tools.claude-code]
enabled = true

[tiers.tier1]
description = "Quick tasks (low thinking budget)"
models = [
    "gemini-cli/google/gemini-3-flash-preview/low",
    "opencode/google/gemini-2.5-pro/minimal",
]

[tiers.tier2]
description = "Standard tasks (medium thinking)"
models = [
    "codex/anthropic/claude-sonnet/medium",
    "gemini-cli/google/gemini-3-pro-preview/medium",
]

[tiers.tier3]
description = "Complex reasoning (high thinking)"
models = [
    "codex/anthropic/claude-opus/high",
    "claude-code/anthropic/claude-opus/xhigh",
]

[tier_mapping]
default = "tier3"
quick = "tier1"
analysis = "tier2"
code-review = "tier2"
complex-reasoning = "tier3"

[aliases]
fast = "gemini-cli/google/gemini-3-flash-preview/low"
smart = "codex/anthropic/claude-opus/xhigh"
balanced = "codex/anthropic/claude-sonnet/medium"
```

## Section Details

### `[project]` - Project Metadata

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | String | Yes | - | Human-readable project name |
| `created_at` | DateTime | Yes | Current time | ISO 8601 timestamp of project initialization |
| `max_recursion_depth` | Integer | No | 5 | Maximum depth for recursive sub-agent spawning |

**Example:**
```toml
[project]
name = "backend-api"
created_at = 2024-02-06T10:00:00Z
max_recursion_depth = 3
```

**Notes:**
- `max_recursion_depth` prevents infinite recursion loops
- Depth is tracked via `CSA_DEPTH` environment variable
- Depth 0 = root session, each sub-agent increments by 1

### `[resources]` - Resource Limits

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `min_free_memory_mb` | Integer | No | 2048 | Minimum free RAM in MB before launching tools |
| `min_free_swap_mb` | Integer | No | 1024 | Minimum free swap in MB |

**Initial Estimates:**

```toml
[resources.initial_estimates]
gemini-cli = 1024
codex = 2048
opencode = 1536
claude-code = 2048
```

**Purpose:**
- Pre-flight resource checks prevent OOM kills
- Initial estimates used until P95 historical data is available
- Historical P95 estimates override initial values after 20+ runs

**Resource Check Formula:**
```
required_memory = min_free_memory_mb + P95_estimate(tool)
if available_memory < required_memory:
    abort with OOM risk warning
```

### `[tools.{tool_name}]` - Tool Configuration

Each tool has an optional configuration section:

```toml
[tools.gemini-cli]
enabled = true

[tools.gemini-cli.restrictions]
allow_edit_existing_files = false
allowed_operations = ["read", "analyze", "create"]
```

**Fields:**

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `enabled` | Boolean | No | `true` | Whether this tool is available |
| `restrictions.allow_edit_existing_files` | Boolean | No | `true` | Allow modifying existing files |
| `restrictions.allowed_operations` | Array[String] | No | All | List of permitted operations |

**Behavior:**
- **Unconfigured tools:** Default to enabled with no restrictions
- **`enabled = false`:** Tool is skipped during tier resolution
- **`allow_edit_existing_files = false`:** Prompt is modified to inject restriction message

**Example Restriction Injection:**

When `allow_edit_existing_files = false`:

```
Original Prompt:
    "Refactor the authentication module"

Modified Prompt:
    "IMPORTANT RESTRICTION: You MUST NOT edit or modify any existing files.
     You may only create new files or perform read-only analysis.

     Refactor the authentication module"
```

**Supported Tools:**
- `gemini-cli`
- `codex`
- `opencode`
- `claude-code`

### `[tiers.{tier_name}]` - Model Tiers

Tiers group models by quality/cost/speed for automatic selection.

**Structure:**
```toml
[tiers.tier1]
description = "Quick tasks (low thinking budget)"
models = [
    "gemini-cli/google/gemini-3-flash-preview/low",
    "opencode/google/gemini-2.5-pro/minimal",
]
```

**Fields:**

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | String | Yes | Human-readable tier purpose |
| `models` | Array[String] | Yes | List of model specs in priority order |

**Model Spec Format:**

```
tool/provider/model/thinking_budget
```

**Components:**
1. **Tool:** `gemini-cli`, `codex`, `opencode`, `claude-code`
2. **Provider:** `google`, `anthropic`, `openai`, etc.
3. **Model:** Full model name (e.g., `gemini-3-pro-preview`, `claude-opus`)
4. **Thinking Budget:** `low`, `medium`, `high`, `xhigh`, or custom token count

**Selection Logic:**
1. Iterate through `models` in order
2. Parse tool name from each spec
3. Return first model where tool is enabled
4. Return `None` if no enabled tools in tier

**Recommended Tiers:**

| Tier | Use Case | Budget | Example Models |
|------|----------|--------|----------------|
| tier1 | Quick tasks, simple queries | low | gemini-flash, opencode-minimal |
| tier2 | Standard development work | medium | claude-sonnet, gemini-pro |
| tier3 | Complex reasoning, security audits | high/xhigh | claude-opus |

### `[tier_mapping]` - Task to Tier Mapping

Maps task types to tier names for semantic selection:

```toml
[tier_mapping]
default = "tier3"
quick = "tier1"
analysis = "tier2"
code-review = "tier2"
complex-reasoning = "tier3"
```

**Usage:**

```bash
# Uses tier_mapping["code-review"] -> tier2
csa run --tier code-review "Review authentication changes"

# Uses tier_mapping["default"] -> tier3 (fallback)
csa run "Implement new feature"
```

**Fallback Behavior:**
1. Look up task type in `tier_mapping`
2. If not found, try to find tier named `tier3` or `tier-3-*`
3. If still not found, return `None` (error)

### `[aliases]` - Model Aliases

Shorthand names for frequently used model specs:

```toml
[aliases]
fast = "gemini-cli/google/gemini-3-flash-preview/low"
smart = "codex/anthropic/claude-opus/xhigh"
balanced = "codex/anthropic/claude-sonnet/medium"
```

**Usage:**

```bash
# Resolves to gemini-cli/google/gemini-3-flash-preview/low
csa run --model fast "Quick check"

# Resolves to codex/anthropic/claude-opus/xhigh
csa run --model smart "Complex refactoring"
```

**Resolution Priority:**
1. Check if input is an alias key → return alias value
2. Otherwise, return input unchanged (treat as direct model spec)

## Configuration Examples

### Example 1: Research Project (Read-Only Gemini)

```toml
[project]
name = "research-analysis"
max_recursion_depth = 3

[resources]
min_free_memory_mb = 1024
min_free_swap_mb = 512

[tools.gemini-cli]
enabled = true

[tools.gemini-cli.restrictions]
allow_edit_existing_files = false
allowed_operations = ["read", "analyze"]

[tools.codex]
enabled = false

[tools.opencode]
enabled = false

[tools.claude-code]
enabled = false

[tiers.tier1]
description = "Analysis only"
models = ["gemini-cli/google/gemini-3-pro-preview/medium"]

[tier_mapping]
default = "tier1"

[aliases]
analyze = "gemini-cli/google/gemini-3-pro-preview/medium"
```

### Example 2: Full-Stack Development

```toml
[project]
name = "webapp-backend"
max_recursion_depth = 5

[resources]
min_free_memory_mb = 3072
min_free_swap_mb = 2048

[resources.initial_estimates]
gemini-cli = 1536
codex = 2560
opencode = 2048
claude-code = 2560

[tools.gemini-cli]
enabled = true

[tools.codex]
enabled = true

[tools.opencode]
enabled = true

[tools.claude-code]
enabled = true

[tiers.tier-1-quick]
description = "Quick iterations and simple tasks"
models = [
    "gemini-cli/google/gemini-3-flash-preview/low",
    "opencode/google/gemini-2.5-pro/minimal",
]

[tiers.tier-2-standard]
description = "Standard development work"
models = [
    "codex/anthropic/claude-sonnet/medium",
    "opencode/google/gemini-2.5-pro/medium",
]

[tiers.tier-3-complex]
description = "Complex refactoring and architecture"
models = [
    "codex/anthropic/claude-opus/high",
    "claude-code/anthropic/claude-opus/xhigh",
]

[tier_mapping]
default = "tier-2-standard"
quick = "tier-1-quick"
simple = "tier-1-quick"
standard = "tier-2-standard"
refactor = "tier-3-complex"
architecture = "tier-3-complex"
security = "tier-3-complex"

[aliases]
fast = "gemini-cli/google/gemini-3-flash-preview/low"
dev = "codex/anthropic/claude-sonnet/medium"
expert = "codex/anthropic/claude-opus/xhigh"
```

### Example 3: Cost-Conscious Setup

```toml
[project]
name = "budget-project"
max_recursion_depth = 4

[resources]
min_free_memory_mb = 1536
min_free_swap_mb = 1024

[tools.gemini-cli]
enabled = true

[tools.codex]
enabled = false  # Anthropic models disabled to save costs

[tools.opencode]
enabled = true

[tools.claude-code]
enabled = false

[tiers.tier1]
description = "Primary tier (Google models only)"
models = [
    "gemini-cli/google/gemini-3-flash-preview/low",
    "opencode/google/gemini-2.5-pro/medium",
]

[tiers.tier2]
description = "Higher quality for critical tasks"
models = [
    "opencode/google/gemini-2.5-pro/high",
    "gemini-cli/google/gemini-3-pro-preview/high",
]

[tier_mapping]
default = "tier1"
critical = "tier2"

[aliases]
cheap = "gemini-cli/google/gemini-3-flash-preview/low"
quality = "opencode/google/gemini-2.5-pro/high"
```

## CLI Integration

### Using Tiers

```bash
# Auto-select tool from default tier
csa run "Implement user authentication"

# Use specific tier mapping
csa run --tier code-review "Review PR #123"

# Use explicit tier name
csa run --tier tier1 "Quick syntax check"
```

### Using Aliases

```bash
# Use alias
csa run --model fast "Check for errors"

# Use full model spec (bypasses tiers)
csa run --model "codex/anthropic/claude-opus/xhigh" "Complex task"
```

### Using Direct Tool Selection

```bash
# Use tool name (no model override)
csa run --tool gemini-cli "Analyze code"

# Combine tool and session
csa run --tool codex --session 01JH4Q "Continue previous work"
```

## Configuration Validation

CSA performs the following validations on load:

1. **Model Spec Format:** Each model must be `tool/provider/model/budget`
2. **Tier References:** All `tier_mapping` values must reference existing tiers
3. **Tool Names:** All tool names in model specs must be valid (`gemini-cli`, `codex`, `opencode`, `claude-code`)
4. **Thinking Budget:** Budget must be `low`, `medium`, `high`, `xhigh`, or a number

**Error Example:**

```toml
[tier_mapping]
default = "nonexistent-tier"  # Error: Tier 'nonexistent-tier' not defined
```

## Best Practices

1. **Start with defaults:** Use `csa init` to generate a template, then customize
2. **Define tiers semantically:** Name tiers by use case, not by model names
3. **Use aliases for frequently used specs:** Avoid typing long model paths
4. **Set appropriate resource limits:** Leave headroom for system operations
5. **Disable unused tools:** Set `enabled = false` for tools you don't have access to
6. **Document restrictions:** Use comments to explain why certain tools have restrictions
7. **Version control:** Commit `.csa/config.toml` to share settings with team

## Troubleshooting

**Problem:** "OOM Risk Prevention" error

**Solution:** Reduce `min_free_memory_mb` or wait for running agents to finish

---

**Problem:** "Tool 'codex' is not enabled"

**Solution:** Set `tools.codex.enabled = true` or remove tool-specific config section

---

**Problem:** "No enabled tools found in tier"

**Solution:** Ensure at least one tool in the tier's models list is enabled

---

**Problem:** Tier resolution always falls back to tier3

**Solution:** Check that `tier_mapping` keys match your usage (e.g., `default` exists)
