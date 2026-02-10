# Skills

> Agent skills for CSA-enhanced coding workflows.
> These skills leverage CSA (CLI Sub-Agent) for heterogeneous model
> orchestration, independent code review, and context-efficient delegation.

## Data Handling

CSA skills may send code diffs and file contents to external LLM providers
for analysis. Before using CSA with private repositories or code containing
secrets/credentials:

1. Run a secrets scan first (e.g., `git secrets --scan`, `trufflehog`)
2. Verify your CSA config routes to approved providers only
3. Use `--tool opencode` for local-only analysis when data sensitivity requires it

## Quick Start

1. Install CSA: use the `install-update-csa` skill
2. Daily commits: use the `commit` skill (includes formatting, linting, testing, security audit, CSA review)
3. PR review loops: use the `pr-codex-bot` skill (iterative bot review with false-positive arbitration)
4. Feature planning: use the `plan-debate` skill (CSA recon + adversarial debate + task decomposition)

## Skill Catalog

### Foundation (Install First)

| Skill | Purpose |
|-------|---------|
| `install-update-csa` | Install, configure, and update CSA binary and project config |
| `csa` | Core CSA runtime: run tasks, manage sessions, parallel execution |

### Analysis & Review

| Skill | Purpose | CSA Integration |
|-------|---------|-----------------|
| `csa-analyze` | Delegate analysis to CSA — zero main-agent file reads | `csa run` |
| `csa-review` | Structured code review with session isolation and 3-pass protocol | `csa review` |
| `code-review` | GitHub PR review via `gh` CLI, scale-adaptive strategy | `csa review` for large PRs |
| `security-audit` | Pre-commit security audit with test completeness verification | `csa run` for large codebases |
| `debate` | Adversarial multi-model debate for strategy formulation | `csa debate` |

### Commit & CI

| Skill | Purpose | CSA Integration |
|-------|---------|-----------------|
| `commit` | Conventional Commits with mandatory pre-commit audit and review | `csa review --diff`, `csa run` for message generation |
| `ai-reviewed-commit` | Pre-commit review loop: review -> fix -> re-review until clean | `csa review --diff` or `csa debate` |
| `pr-codex-bot` | Iterative PR review loop with cloud review bots | `csa debate` for false-positive arbitration |

### Planning

| Skill | Purpose | CSA Integration |
|-------|---------|-----------------|
| `plan-debate` | Debate-enhanced planning: recon -> draft -> debate -> decompose -> execute | `csa run`, `csa debate`, `csa review` |

## Skill Dependency Graph

```
install-update-csa
    └── csa (core runtime)
         ├── csa-analyze (analysis delegation)
         ├── csa-review (structured review)
         ├── debate (adversarial strategy)
         │
         ├── security-audit (uses csa for large codebases)
         │    └── commit (invokes security-audit before commit)
         │         └── ai-reviewed-commit (wraps csa review + fix loop)
         │
         ├── code-review (uses csa-review, debate for large PRs)
         │
         ├── plan-debate (uses csa run, debate, commit in execution)
         │
         └── pr-codex-bot (uses csa-review, debate, commit)
```

## Workflow Patterns

### Pattern 1: Single Commit (Most Common)

```
code changes -> commit skill
                  ├── Run formatter / linter / tests
                  ├── security-audit skill
                  ├── csa review --diff (or csa debate if self-authored)
                  └── git commit (Conventional Commits format)
```

### Pattern 2: PR Review Loop

```
commit -> pr-codex-bot skill
            ├── csa-review (local pre-PR cumulative audit)
            ├── push + create PR + trigger bot review
            ├── poll for bot response
            ├── debate skill (false-positive arbitration)
            └── fix -> push -> re-review loop (max 10 iterations)
```

### Pattern 3: Feature Planning & Implementation

```
plan-debate skill
  Phase 1: RECON — csa run (parallel reconnaissance, zero main-agent reads)
  Phase 2: DRAFT — main agent drafts plan from CSA summaries
  Phase 3: DEBATE — csa debate (mandatory adversarial review)
  Phase 4: DECOMPOSE — task breakdown with executor tags
  Phase 5: APPROVE — user gate
  Phase 6: EXECUTE — delegated execution (commit skill per unit)
```

### Pattern 4: PR Code Review

```
code-review skill
  ├── small PR (<200 lines): direct review
  ├── medium PR (200-1000 lines): review with progress tracking
  └── large PR (>1000 lines): delegate to csa-review or csa-analyze
```

## Customization

Skills use generic terms for project-specific commands. Define your actual commands in `CLAUDE.md`:

```markdown
## Commands
- Format: `just fmt` or `npm run format` or `cargo fmt` or `black .`
- Lint: `just lint` or `npm run lint` or `cargo clippy` or `ruff check .`
- Test: `just test` or `npm test` or `cargo test` or `pytest`
- Pre-commit: `just pre-commit` or `npm run precommit` or `pre-commit run --all-files`
```

Skills will follow whatever commands your `CLAUDE.md` specifies.

## Prerequisites

- **CSA binary** installed and configured (use `install-update-csa` skill)
- **`gh` CLI** installed and authenticated (for `code-review` and `pr-codex-bot`)
- **At least one CSA-supported tool** installed: `codex`, `claude-code`, or `opencode`

## Core Principles

These skills are built on shared principles:

1. **Heterogeneous model review**: Never trust a single model's judgment — use CSA to get independent perspectives
2. **Context efficiency**: Main agent stays clean; CSA sub-agents handle heavy file reading
3. **Audit trail**: All adversarial decisions include participant model specs (`tool/provider/model/thinking_budget`)
4. **Review-and-escalate**: If CSA fails, the caller takes over — never retry with the same approach
5. **Quota awareness**: If CSA hits rate limits, stop and ask the user — never silently degrade to single-model
