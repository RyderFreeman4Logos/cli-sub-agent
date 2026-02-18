---
name = "csa-setup"
version = "0.1.0"
description = "Install CSA, Weave, and coding workflow patterns for AI agent bots"
homepage = "https://github.com/RyderFreeman4Logos/cli-sub-agent"
target = "agent-bots"
---

# CSA Setup

> Installation guide for AI agent bots (openclaw, Moltis, etc.) to set up
> CSA and Weave with optional coding workflow patterns.

## Skill Files

| File | Description |
|------|-------------|
| **skill.md** (this file) | Installation and setup guide |
| **skills/AGENTS.md** | Full skill & pattern catalog |
| **README.md** | Project documentation |

---

## Prerequisites

Before starting, verify these tools are available:

```bash
# Required
git --version          # Git 2.30+
cargo --version        # Rust toolchain (only needed for building from source)

# Optional but recommended
mise --version         # Cross-platform tool version manager
gh --version           # GitHub CLI (for PR workflows)
```

If `mise` is not installed (see [mise.jdx.dev/installing](https://mise.jdx.dev/installing-mise.html) for alternatives):

```bash
# Verify domain before piping to shell
curl https://mise.run | sh
```

If `cargo` is not installed (see [rustup.rs](https://rustup.rs/) for alternatives):

```bash
# Official Rust installer — verify TLS and domain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## Step 1: Install CSA

### Option A: Quick install (prebuilt binaries)

```bash
curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh | sh
```

This installs both `csa` and `weave` from prebuilt binaries.

### Option B: via mise (recommended for version management)

```bash
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=csa]
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=weave]
csa --version
```

Upgrade later with `mise upgrade`.

### Option C: from source

```bash
git clone https://github.com/RyderFreeman4Logos/cli-sub-agent.git
cd cli-sub-agent
cargo install --path crates/cli-sub-agent
```

### Verify

```bash
csa --version
# Expected: csa <version>
```

---

## Step 2: Install Weave

Weave is the skill-lang compiler and package manager.

If you used Option A (install.sh) in Step 1, weave is already installed. Otherwise:

```bash
# Via mise
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=weave]

# Or from source (if you already cloned in Step 1 Option C, skip the clone)
git clone https://github.com/RyderFreeman4Logos/cli-sub-agent.git
cd cli-sub-agent
cargo install --path crates/weave
```

### Verify

```bash
weave --help
# Expected: weave <version> - Skill-lang compiler and package manager
```

---

## Step 3: Initialize Project

Navigate to the target project:

```bash
cd /path/to/your-project
```

### Decide whether to create a project config

Check if a global config already exists:

```bash
ls ~/.config/cli-sub-agent/config.toml 2>/dev/null && echo "GLOBAL_EXISTS" || echo "NO_GLOBAL"
```

- **If `GLOBAL_EXISTS`**: Project config is usually unnecessary — CSA falls back to
  global config automatically. **Skip `csa init` by default.** Only run it if
  the user explicitly requests project-specific overrides.

- **If `NO_GLOBAL`**: **ASK THE USER** which mode to use:
  - **`csa init --minimal`** (Recommended): Creates only `[project]` metadata.
    Tools, tiers, and resources inherit from built-in defaults. Best for most
    projects.
  - **`csa init`** (Full): Creates a complete config with tool detection, smart
    tiers, and resource estimates. Use when the project needs custom tool
    configuration.

**CRITICAL**: Do NOT run `csa init` without user consent. Running it
unconditionally overrides global settings with project-local defaults.

### Install git branch protection (recommended)

Prevents commits on protected branches (main, dev, master). Works with all
tools — Claude Code, Codex, Gemini, manual git:

```bash
mkdir -p .githooks
cat > .githooks/pre-commit << 'HOOK'
#!/usr/bin/env bash
set -euo pipefail
branch=$(git symbolic-ref --short HEAD 2>/dev/null) || exit 0
[ -z "$branch" ] && exit 0
for pb in main dev master; do
  if [ "$branch" = "$pb" ]; then
    echo "BLOCKED: Cannot commit directly to '$branch'. Create a feature branch first."
    exit 1
  fi
done
HOOK
chmod +x .githooks/pre-commit
git config core.hooksPath .githooks
```

CSA also ships **built-in prompt guards** (branch-protection, dirty-tree-reminder,
commit-workflow) that automatically inject workflow reminders into every
`csa run` session — no installation needed.

### Check tool availability

```bash
csa doctor
```

This reports which AI tools (claude-code, codex, gemini-cli, opencode) are
available and properly configured.

---

## Step 4: Install Core Skills

Install the base persona skills that enable CSA's core capabilities:

```bash
# Install from the CSA repository
weave install RyderFreeman4Logos/cli-sub-agent
```

This installs all skills and patterns into `.weave/deps/cli-sub-agent/`.

### Verify installation

```bash
weave audit
weave check --fix
```

---

## Step 4b: Install Pattern Skills

Each pattern ships a **companion skill** that serves as its entry point. These
skills tell the orchestrator (Claude Code, etc.) how to invoke the pattern.
The companion skill is NOT executed by the pattern workflow (that would cause
infinite recursion) — it is a facade for tool discovery.

`weave install` automatically creates symlinks from `.claude/skills/` (and
other tool-specific directories) into the global package store. No manual
setup is needed.

### Verify

```bash
ls -la .claude/skills/
```

You should see symlinks for each pattern skill (e.g., `commit`, `mktd`, `sa`).

### Maintenance

If symlinks become stale or broken (e.g., after updating packages):

```bash
weave link sync          # Reconcile: create missing, remove stale, fix broken
weave check --fix        # Remove broken symlinks only
```

### Conflict resolution

If two packages expose the same skill name, `weave install` will report a
conflict. To resolve, install with `--no-link` and create renamed symlinks:

```bash
weave install user/repo --no-link
cd .claude/skills
ln -s ../../.weave/store/repo/.../patterns/commit/skills/commit/ my-commit
```

The renamed symlink still points to the canonical companion skill directory.

### Scope control

```bash
weave install user/repo                          # Default: project-level (.claude/skills/)
weave install user/repo --link-scope user        # User-level (~/.claude/skills/)
weave install user/repo --no-link                # Skip linking entirely
```

---

## Step 5: Programming Patterns (Interactive)

CSA ships with 13 compiled workflow patterns for coding tasks. Not all projects
need all patterns.

**ASK THE USER**: Present the following categories and let the user choose which
patterns to install. Use checkboxes or a numbered menu.

---

### Category A: Commit & Review (recommended for all coding projects)

> These patterns enforce strict commit discipline with security audit, test
> verification, and heterogeneous model review.

| Pattern | What it does |
|---------|--------------|
| `commit` | Audited commits: format, lint, test, security scan, AI review, then commit |
| `ai-reviewed-commit` | Review-fix-re-review loop until clean before committing |
| `code-review` | Scale-adaptive GitHub PR review (small/medium/large) |
| `pr-codex-bot` | Iterative PR review with Codex bot feedback and merge |

**Install**:

```bash
# Patterns are already in .weave/deps/ from Step 4.
# Compile them for your project:
mkdir -p .csa/plans
for pattern in commit ai-reviewed-commit code-review pr-codex-bot; do
  weave compile .weave/deps/cli-sub-agent/patterns/$pattern/PATTERN.md \
    --output .csa/plans/$pattern.toml
done
```

---

### Category B: Security & Audit

> Adversarial security analysis and compliance auditing.

| Pattern | What it does |
|---------|--------------|
| `security-audit` | Pre-commit vulnerability scan and test-completeness check |
| `file-audit` | Per-file AGENTS.md compliance audit with report generation |
| `csa-review` | Independent CSA-driven code review with structured output |

**Install**:

```bash
mkdir -p .csa/plans
for pattern in security-audit file-audit csa-review; do
  weave compile .weave/deps/cli-sub-agent/patterns/$pattern/PATTERN.md \
    --output .csa/plans/$pattern.toml
done
```

---

### Category C: Planning & Task Management

> Structured planning workflows with debate and version control.

| Pattern | What it does |
|---------|--------------|
| `mktd` | Make TODO: reconnaissance, drafting, debate, approval |
| `mktsk` | Convert TODO plans into persistent serial tasks |
| `debate` | Adversarial multi-tool strategy debate with convergence |

**Install**:

```bash
mkdir -p .csa/plans
for pattern in mktd mktsk debate; do
  weave compile .weave/deps/cli-sub-agent/patterns/$pattern/PATTERN.md \
    --output .csa/plans/$pattern.toml
done
```

---

### Category D: Advanced Workflows

> End-to-end orchestration and issue reporting.

| Pattern | What it does |
|---------|--------------|
| `sa` | Three-tier recursive sub-agent orchestration |
| `dev-to-merge` | Branch-to-merge: implement, validate, PR, review, merge |
| `csa-issue-reporter` | Structured GitHub issue filing for CSA errors |

**Install**:

```bash
mkdir -p .csa/plans
for pattern in sa dev-to-merge csa-issue-reporter; do
  weave compile .weave/deps/cli-sub-agent/patterns/$pattern/PATTERN.md \
    --output .csa/plans/$pattern.toml
done
```

---

### Install All (ONLY on explicit user request)

**CRITICAL**: Do NOT run this automatically. Only execute when the user
**explicitly says** they want all patterns installed (e.g., "install everything",
"install all patterns"). Most projects only need Category A (Commit & Review).

If the user explicitly requests all patterns:

```bash
mkdir -p .csa/plans
for pattern in .weave/deps/cli-sub-agent/patterns/*/; do
  name=$(basename "$pattern")
  if [ -f "$pattern/PATTERN.md" ]; then
    weave compile "$pattern/PATTERN.md" --output ".csa/plans/$name.toml"
  fi
done
```

---

## Step 6: Configure Global Settings

Create or edit `~/.config/cli-sub-agent/config.toml`:

```toml
# Tool selection priority (first = most preferred)
[preferences]
tool_priority = ["claude-code", "codex", "gemini-cli", "opencode"]

# Review tool (auto = heterogeneous selection)
[review]
tool = "auto"

# Debate tool
[debate]
tool = "auto"

# Concurrency limits (default: 3 per tool)
[defaults]
max_concurrent = 3

# Per-tool overrides (uncomment as needed)
# [tools.codex]
# max_concurrent = 5
```

**ASK THE USER**: Which AI tools do they have access to? Adjust
`tool_priority` accordingly. Common configurations:

| Setup | Recommended `tool_priority` |
|-------|-----------------------------|
| Claude Code + Codex | `["claude-code", "codex"]` |
| Codex + Gemini CLI | `["codex", "gemini-cli"]` |
| All tools available | `["claude-code", "codex", "gemini-cli", "opencode"]` |
| Single tool only | Set `[review] tool = "<tool>"` and `[debate] tool = "<tool>"` explicitly |

---

## Step 7: Configure Hooks (Optional)

CSA has a hook system that runs shell scripts at key lifecycle events. Three
**built-in prompt guards** are enabled by default — no configuration needed:

| Guard | What it does |
|-------|--------------|
| `branch-protection` | Warns when running on protected branches (main, dev, release/*) |
| `dirty-tree-reminder` | Reminds about uncommitted changes in the working tree |
| `commit-workflow` | Reminds about unpushed commits on feature branches |

### Adding custom guards

Custom guards **stack on top of built-ins**. Create or edit
`~/.config/cli-sub-agent/hooks.toml` (global) or
`~/.local/state/csa/{project}/hooks.toml` (project-level):

```toml
[[prompt_guard]]
name = "pr-reminder"
command = "/path/to/remind-pr.sh"
timeout_secs = 5
```

Guard scripts receive a JSON context on stdin (`project_root`, `session_id`,
`tool`, `is_resume`, `cwd`) and write injection text to stdout. Empty stdout
means no injection. Non-zero exit or timeout is warned and skipped.

### Disabling built-in guards

```toml
builtin_guards = false
```

### Hook events

Beyond prompt guards, CSA supports lifecycle hooks:

```toml
[pre_run]
enabled = true
command = "echo pre-run: {session_id} {tool}"
timeout_secs = 30

[post_run]
enabled = true
command = "echo post-run: {session_id} exit={exit_code}"
timeout_secs = 30

[session_complete]
enabled = true
# Built-in default auto-commits session data (active when command is not set)
```

See `docs/hooks.md` in the CSA repository for full reference.

---

## Step 8: Verify Everything

```bash
# Check CSA is working
csa --version

# Check weave is working
weave --help

# Check tool availability
csa doctor

# Check installed skills
weave audit

# Check for broken symlinks
weave check --fix

# Test a simple run (replace with your preferred tool)
csa run --tool codex "echo hello from CSA"
```

---

## Quick Reference

### CSA Commands

```bash
csa run --tool <tool> "prompt"          # Run a task
csa run --tool auto "prompt"            # Auto-select tool
csa run --last "continue"               # Resume last session
csa review --diff                       # Review uncommitted changes
csa review --reviewers 3                # Multi-reviewer consensus
csa debate "design question"            # Adversarial model debate
csa session list --tree                 # List session tree
csa gc --dry-run                        # Preview garbage collection
```

### Weave Commands

```bash
weave compile PATTERN.md                # Compile to execution plan
weave compile PATTERN.md -o plan.toml   # Compile to file
weave install user/repo                 # Install skill from GitHub
weave install --path ./local-skill      # Install from local path
weave lock                              # Generate lockfile
weave update                            # Update all dependencies
weave audit                             # Check consistency
weave check --fix                       # Fix broken symlinks
weave visualize plan.toml               # ASCII workflow diagram
weave visualize plan.toml --mermaid     # Mermaid flowchart
```

---

## Troubleshooting

| Problem | Solution |
|---------|----------|
| `csa: command not found` | Run `mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=csa]` |
| `weave: command not found` | Run `curl -fsSL https://raw.githubusercontent.com/RyderFreeman4Logos/cli-sub-agent/main/install.sh \| sh` or build from source: `cargo install --path crates/weave` |
| `csa doctor` shows tool unavailable | Install the missing tool or remove from `tool_priority` |
| `weave audit` reports missing deps | Run `weave install RyderFreeman4Logos/cli-sub-agent` |
| Broken symlinks after update | Run `weave check --fix` |
| Codex rate limit / quota | Wait for cooldown or switch tool: `csa run --tool claude-code` |
