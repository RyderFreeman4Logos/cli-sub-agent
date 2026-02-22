# Debate & Review

CSA provides two heterogeneous multi-model commands: `csa review` for code
review and `csa debate` for adversarial technical debates. Both enforce
model heterogeneity -- the reviewing/debating agent always uses a different
model family than the caller.

## Heterogeneous Model Selection

CSA detects the parent tool via `/proc` filesystem inspection. In
`--tool auto` mode (the default), it selects a tool from a **different
model family**:

| Parent Tool | Auto-selected Tool | Reason |
|-------------|--------------------|--------|
| claude-code | codex or gemini-cli | Different model family |
| codex | claude-code or gemini-cli | Different model family |
| gemini-cli | claude-code or codex | Different model family |

If no heterogeneous tool is available, CSA fails with an explicit error.
It never silently degrades to the same model.

**Why heterogeneous?** Same-model reviews suffer from shared blind spots.
A Claude model reviewing Claude-generated code tends to miss the same
classes of errors. Cross-model review catches issues that single-model
workflows miss.

## Code Review

### Basic Usage

```bash
# Review uncommitted changes
csa review --diff

# Review a commit range
csa review --range main...HEAD

# Review a specific commit
csa review --commit abc123

# Review specific files
csa review --files "src/auth/*.rs"
```

### Review Scopes

| Flag | Scope |
|------|-------|
| `--diff` | Uncommitted changes (`git diff HEAD`) |
| `--range <RANGE>` | Commit range (e.g., `main...HEAD`) |
| `--commit <SHA>` | Single commit |
| `--files <PATHSPEC>` | Specific files |
| `--branch <BRANCH>` | Compare against branch |

These are mutually exclusive; specify exactly one.

### Review-and-Fix Mode

```bash
csa review --diff --fix
```

In fix mode, the reviewer applies fixes directly to the codebase
instead of just reporting issues.

### Security Review

```bash
csa review --diff --security-mode on
```

Security mode (`auto`, `on`, `off`) controls whether the review includes
security-focused analysis (dependency vulnerabilities, injection risks,
secret exposure, etc.).

### Multi-Reviewer Consensus

For high-stakes reviews, CSA can run multiple reviewers in parallel and
aggregate their verdicts:

```bash
csa review --diff --reviewers 3 --consensus majority
```

#### How it works

1. CSA spawns N reviewer agents, distributing across available tools
2. Each reviewer emits a verdict: `CLEAN` or `HAS_ISSUES`
3. The consensus engine aggregates verdicts using the chosen strategy
4. Review artifacts (findings, reports) are written to session subdirectories

#### Consensus Strategies

| Strategy | Rule |
|----------|------|
| `majority` | More than half must agree |
| `unanimous` | All reviewers must agree |
| `weighted` | Weighted by tool priority from config |

The consensus engine is provided by the `agent-teams-rs` crate, which
implements `resolve_majority()`, `resolve_unanimous()`, and
`resolve_weighted()` functions.

#### Reviewer Tool Distribution

When using multiple reviewers without an explicit `--tool`, CSA
distributes reviewers across enabled tools in round-robin fashion,
prioritized by the global/project tool ordering. This ensures diverse
model perspectives.

### Review Pattern

`csa review` uses the `csa-review` pattern (installed via `csa skill install`)
to structure its review prompt. The pattern defines:

- Review scope extraction
- Code analysis instructions
- Finding classification (P0/P1/P2)
- Output format (JSON findings + markdown report)

If the pattern is not installed, use `--allow-fallback` to proceed
with a built-in prompt (with a warning).

## Adversarial Debate

### Basic Usage

```bash
# Start a debate
csa debate "Should we use anyhow or thiserror for error handling?"

# Multi-round debate
csa debate --rounds 5 "Redis vs Memcached for session storage"

# Resume a previous debate
csa debate --session 01JK "reconsider given the benchmark results"
```

### How Debate Works

1. CSA selects a heterogeneous tool pair (e.g., claude-code vs codex)
2. Round 1: First tool presents its position
3. Round 2: Second tool presents counter-arguments
4. Rounds alternate until `--rounds` is reached (default: 3)
5. Structured output is persisted to the session directory

### Debate Configuration

```bash
csa debate --rounds 5 --thinking high "complex architecture question"
csa debate --tool codex "force specific tool"
csa debate --timeout 600 "time-limited debate"
```

### Structured Output

Debate results are saved to the session directory as structured files
including:

- Per-round arguments and counter-arguments
- Final synthesis/recommendation
- Session metadata for resumption

## Timeouts

Both commands support timeout control:

| Flag | Description |
|------|-------------|
| `--timeout <SECS>` | Absolute wall-clock timeout |
| `--idle-timeout <SECS>` | Kill when no output for N seconds |
| `--no-idle-timeout` | Disable idle timeout (run until completion) |

Default timeouts are read from `~/.config/cli-sub-agent/config.toml`:

```toml
[debate]
timeout_secs = 1800     # 30 minutes default

[review]
timeout_secs = 600      # 10 minutes default
```

## Stream Control

Both commands stream output to stderr by default when connected to a TTY:

```bash
# Force streaming
csa review --diff --stream-stdout

# Suppress streaming
csa review --diff --no-stream-stdout
```

## Related

- [Commands](commands.md) -- full flag reference for review and debate
- [Architecture](architecture.md) -- heterogeneous routing details
- [Sessions](sessions.md) -- session persistence for debate resumption
- [Configuration](configuration.md) -- review/debate config sections
