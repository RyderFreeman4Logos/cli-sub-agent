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
cargo --version        # Rust toolchain (for weave only; csa uses prebuilt binary)

# Optional but recommended
mise --version         # Cross-platform tool version manager
gh --version           # GitHub CLI (for PR workflows)
```

If `mise` is not installed:

```bash
curl https://mise.run | sh
```

If `cargo` is not installed (needed only for weave):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## Step 1: Install CSA

### Option A: via mise (recommended)

```bash
mise use -g ubi:RyderFreeman4Logos/cli-sub-agent[exe=csa]
csa --version
```

Upgrade later with `mise upgrade`.

### Option B: from source

```bash
git clone https://github.com/RyderFreeman4Logos/agent-teams-rs.git
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

Weave is the skill-lang compiler and package manager. It is not yet in GitHub
Releases, so it must be built from source.

```bash
# If you already cloned in Step 1 Option B, skip the clone
git clone https://github.com/RyderFreeman4Logos/agent-teams-rs.git
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

Navigate to the target project and initialize CSA:

```bash
cd /path/to/your-project
csa init
```

This creates `.csa/config.toml` with default settings.

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
for pattern in sa dev-to-merge csa-issue-reporter; do
  weave compile .weave/deps/cli-sub-agent/patterns/$pattern/PATTERN.md \
    --output .csa/plans/$pattern.toml
done
```

---

### Install All (skip interactive selection)

If the user wants everything:

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

# Concurrency limits
[concurrency]
max_global_slots = 4
```

**ASK THE USER**: Which AI tools do they have access to? Adjust
`tool_priority` accordingly. Common configurations:

| Setup | Recommended `tool_priority` |
|-------|-----------------------------|
| Claude Code + Codex | `["claude-code", "codex"]` |
| Codex + Gemini CLI | `["codex", "gemini-cli"]` |
| All tools available | `["claude-code", "codex", "gemini-cli", "opencode"]` |
| Single tool only | Set `[review] tool = "<tool>"` explicitly |

---

## Step 7: Verify Everything

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
| `weave: command not found` | Build from source: `cargo install --path crates/weave` |
| `csa doctor` shows tool unavailable | Install the missing tool or remove from `tool_priority` |
| `weave audit` reports missing deps | Run `weave install RyderFreeman4Logos/cli-sub-agent` |
| Broken symlinks after update | Run `weave check --fix` |
| Codex rate limit / quota | Wait for cooldown or switch tool: `csa run --tool claude-code` |
