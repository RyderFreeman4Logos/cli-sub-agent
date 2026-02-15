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

1. Daily commits: use the `commit` skill (includes formatting, linting, testing, security audit, CSA review)
2. PR review loops: use the `pr-codex-bot` skill (iterative bot review with false-positive arbitration)
3. Feature planning: use `mktd` (CSA recon + adversarial debate + TODO output) then `mktsk` (convert TODOs to Task tools + serial execution)

## Skill Catalog

### Core Runtime

| Skill | Purpose | Status |
|-------|---------|--------|
| `csa` | Core CSA runtime: run tasks, manage sessions, parallel execution | Active (global) |

### Analysis & Review

| Skill | Purpose | CSA Integration | Pattern |
|-------|---------|-----------------|---------|
| ~~`csa-analyze`~~ | ~~Delegate analysis to CSA~~ | ~~`csa run`~~ | **DEPRECATED** — use `mktd` RECON or direct `csa run` |
| `csa-review` | Structured code review with session isolation and 3-pass protocol | `csa review` | `drafts/patterns/csa-review/` |
| `code-review` | GitHub PR review via `gh` CLI, scale-adaptive strategy | `csa review` for large PRs | `drafts/patterns/code-review/` |
| `security-audit` | Pre-commit security audit with test completeness verification | `csa run` for large codebases | `drafts/patterns/security-audit/` |
| `debate` | Adversarial multi-model debate for strategy formulation | `csa debate` | `drafts/patterns/debate/` |

### Commit & CI

| Skill | Purpose | CSA Integration | Pattern |
|-------|---------|-----------------|---------|
| `commit` | Conventional Commits with mandatory pre-commit audit and review | `csa review --diff`, `csa run` for message generation | `drafts/patterns/commit/` |
| `ai-reviewed-commit` | Pre-commit review loop: review -> fix -> re-review until clean | `csa review --diff` or `csa debate` | `drafts/patterns/ai-reviewed-commit/` |
| `pr-codex-bot` | Iterative PR review loop with cloud review bots | `csa debate` for false-positive arbitration | `drafts/patterns/pr-codex-bot/` |
| `csa-issue-reporter` | File GitHub issues when CSA encounters errors | `gh issue create` | `drafts/patterns/csa-issue-reporter/` |

### Planning & Execution

| Skill | Purpose | CSA Integration | Pattern |
|-------|---------|-----------------|---------|
| `mktd` | Make TODO: CSA recon + draft + adversarial debate | `csa run`, `csa debate` | `drafts/patterns/mktd/` |
| `mktsk` | Make Task: convert TODO plans into Task tool entries for persistent serial execution | Uses executor tags from `mktd` output | `drafts/patterns/mktsk/` |
| `sa` | Three-tier recursive delegation (dispatch, plan/implement, explore/fix) | `csa run --tool claude-code`, `csa run --tool codex` | `drafts/patterns/sa/` |

### Specialized (Global Skills — No Pattern Equivalent)

| Skill | Purpose |
|-------|---------|
| `csa-async-debug` | Expert diagnosis for Tokio/async Rust issues |
| `csa-doc-writing` | Clear, practical documentation for APIs, README, architecture |
| `csa-rust-dev` | Comprehensive Rust development guide |
| `csa-security` | Adversarial security analysis expertise |
| `csa-test-gen` | Comprehensive test design for Rust projects |

### Deprecated

| Skill | Status | Migration |
|-------|--------|-----------|
| ~~`install-update-csa`~~ | **DELETED** | Manual `cargo install` or project Justfile |
| ~~`csa-cc`~~ | **DELETED** | Use CSA's built-in tool routing + patterns |
| ~~`csa-analyze`~~ | **DELETED** | Use `mktd` RECON or direct `csa run` |

## Skill Dependency Graph

```
csa (core runtime)
     ├── csa-review (structured review)
     ├── debate (adversarial strategy)
     │
     ├── security-audit (uses csa for large codebases)
     │    └── commit (invokes security-audit before commit)
     │         └── ai-reviewed-commit (wraps csa review + fix loop)
     │
     ├── code-review (uses csa-review, debate for large PRs)
     │
     ├── mktd (uses csa run, debate for TODO generation)
     │    └── mktsk (converts TODOs to Task tools, uses commit in execution)
     │
     ├── pr-codex-bot (uses csa-review, debate, commit)
     │
     ├── sa (three-tier orchestration using all of the above)
     │
     └── csa-issue-reporter (standalone issue filing)
```

## Pattern Migration Status

11 skills have been converted to skill-lang patterns (draft status).
Pattern files live in the external `drafts/` directory (symlink to
`../drafts/cli-sub-agent`, outside git tracking). To access them, set up
the drafts symlink: `ln -s ../drafts/cli-sub-agent drafts`.
All 11 compile successfully with the weave compiler.

| Pattern | Source Skill | Weave Compile | Key Features |
|---------|-------------|---------------|--------------|
| `security-audit` | `security-audit` | PASS | 3-phase audit, CSA delegation for large modules |
| `debate` | `debate` | PASS | Tier escalation, convergence evaluation |
| `csa-issue-reporter` | `csa-issue-reporter` | PASS | Structured GitHub issue filing |
| `csa-review` | `csa-review` | PASS | Role detection, TODO alignment, fix mode |
| `code-review` | `code-review` | PASS | Scale-adaptive, authorship-aware |
| `ai-reviewed-commit` | `ai-reviewed-commit` | PASS | Review-fix loop, authorship detection |
| `commit` | `commit` | PASS | INCLUDE security-audit + ai-reviewed-commit |
| `mktd` | `mktd` | PASS | 4-phase planning, INCLUDE debate |
| `mktsk` | `mktsk` | PASS | FOR loop, serial execution, INCLUDE commit |
| `pr-codex-bot` | `pr-codex-bot` | PASS | FOR comment loop, INCLUDE debate |
| `sa` | `sa` | PASS | 3-tier dispatch, INCLUDE commit |

Skills in `skills/` remain the runtime reference. Patterns in `drafts/patterns/`
are the new canonical workflow definitions, pending runtime executor support.

## Workflow Patterns

### Pattern 1: Single Commit (Most Common)

```
code changes -> commit pattern
                  ├── Run formatter / linter / tests
                  ├── security-audit pattern
                  ├── csa review --diff (or csa debate if self-authored)
                  └── git commit (Conventional Commits format)
```

### Pattern 2: PR Review Loop

```
commit -> pr-codex-bot pattern
            ├── csa-review (local pre-PR cumulative audit)
            ├── push + create PR + trigger bot review
            ├── poll for bot response
            ├── debate pattern (false-positive arbitration)
            └── fix -> push -> re-review loop (max 10 iterations)
```

### Pattern 3: Feature Planning & Implementation

```
mktd pattern (planning)
  Phase 1: RECON — csa run (parallel reconnaissance, zero main-agent reads)
  Phase 2: DRAFT — main agent drafts todo.md with [ ] items and executor tags
  Phase 3: DEBATE — csa debate (mandatory adversarial review)
  Phase 4: APPROVE — user gate

mktsk pattern (execution)
  Parse todo.md → TaskCreate entries with executor + DONE WHEN
  Serial execution: implement → review → commit → next
  Context management: /compact after logical stages
```

### Pattern 4: PR Code Review

```
code-review pattern
  ├── small PR (<200 lines): direct review
  ├── medium PR (200-800 lines): review with progress tracking
  └── large PR (>800 lines): delegate to csa-review or csa run
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

- **CSA binary** installed and configured
- **`gh` CLI** installed and authenticated (for `code-review` and `pr-codex-bot`)
- **At least one CSA-supported tool** installed: `codex`, `claude-code`, or `opencode`

## Core Principles

These skills are built on shared principles:

1. **Heterogeneous model review**: Never trust a single model's judgment — use CSA to get independent perspectives
2. **Context efficiency**: Main agent stays clean; CSA sub-agents handle heavy file reading
3. **Audit trail**: All adversarial decisions include participant model specs (`tool/provider/model/thinking_budget`)
4. **Review-and-escalate**: If CSA fails, the caller takes over — never retry with the same approach
5. **Quota awareness**: If CSA hits rate limits, stop and ask the user — never silently degrade to single-model
