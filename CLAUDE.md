# CLAUDE.md - csa (cli-sub-agent)

Recursive Agent Container: Standardized, composable Unix processes for LLM CLI tools.
CSA wraps four heterogeneous AI coding tools (claude-code, codex, gemini-cli, opencode)
behind a unified CLI, providing session management, resource sandboxing, consensus-based
review, and fractal sub-agent spawning. Each `csa` invocation is an isolated Unix process
with its own ULID-identified session, lock files, and lifecycle tracking.

## Commands

Most commands are documented in `csa --help` and subcommand help (`csa <subcommand> --help`).

High-frequency command groups:

- Development: `just pre-commit`, `just clippy`, `just test`, `just fmt`.
- Execution: `csa run`, `csa review`, `csa debate`.
- Session management: `csa session list|result|logs|fork|checkpoint`.
- Planning: `csa todo ...` and `csa plan run workflow.toml` (`csa todo show --spec` renders persisted
  spec criteria from `spec.toml` when present). `csa todo ref list|show|add|import-transcript`
  for progressive disclosure references.
- Token estimation: `csa tokuin estimate [files...]`.
- Transcript reading: `csa xurl threads`.
- MCP hub: `csa mcp-hub serve|status|stop|gen-skill`.
- Ops: `csa doctor`, `csa gc`, `csa migrate`, `csa self-update`.

### Timeout Policy

- **Minimum Timeout**: The absolute wall-clock timeout (`--timeout`) MUST be at least 1800 seconds (30 minutes). Short timeouts waste tokens because the agent starts working but gets killed before producing output. This is enforced at the CLI level.

### CSA Run

```bash
csa run --fork-call "prompt"                     # fork-call mode (child returns to parent)
csa run --fork-call --return-to last "prompt"   # return to most recent session
csa run --fork-call --return-to <ULID> "prompt" # return to specific session
```

## Git Workflow

Feature branches ONLY (`feat/`/`fix/`/`refactor/`/`docs/`/`test/`/`chore/`), NEVER push to `dev`/`main` directly. Full pipeline (branch→plan→implement→review→PR→merge) is enforced by `dev2merge` pattern via `csa plan run`. Do NOT rely on LLM instruction-following for pipeline steps — use deterministic weave workflows. CSA tasks MUST NOT include PR creation. Conventional Commits with crate scope. `just pre-commit` before every commit.
PRs MUST be merged with merge commits (`gh pr merge --merge`), NEVER squash (`--squash`). Each commit contains AI Reviewer Metadata for audit trails; squash merge destroys this per-commit history.
→ `.agents/project-rules-ref/git-workflow.md`

## Planning (MANDATORY)

NEVER use Claude Code's native `EnterPlanMode`. Always use `/mktd` skill instead,
which provides debate-enhanced planning with CSA reconnaissance and adversarial review.

## Task Tracking (MANDATORY)

Any insight, requirement, or issue discovered mid-work MUST be immediately
recorded via `TaskCreate` before continuing. This includes:

- CSA failures (timeout, quota, unexpected kill)
- Mid-task requirement changes from user
- Bugs or design issues discovered during implementation
- CLI syntax errors found in skills/patterns
- Configuration gaps (missing timeout, wrong defaults)

**Why**: Context compaction and CSA session kills lose unrecorded information.
TaskCreate persists across compaction and session boundaries.

## Verification

- Commit: `just pre-commit` exit 0
- Feature complete: `cargo test` exit 0 + `git status` clean + branch is not main/dev
- CSA run: session result written to `~/.local/state/cli-sub-agent/`
- PR ready: `csa review --diff` verdict is Pass

## Compact Instructions

After `/compact`, restore context:
1. Read `CLAUDE.md` and `.claude/rules/csa-architecture-ref.md`
2. Run `csa todo show` for current task state
3. Check `git log --oneline -5` and `git status` for working state
4. Check TaskList for in-progress items

## Memory

Architecture decisions and user preferences in auto-memory system.
Check MEMORY.md for: timeout policy, config state, tool preferences, feedback history.

### Process Issues

When discovering workflow/process issues, fix them in the relevant skill/pattern/workflow/CSA hook files so ALL CSA users benefit. Agent memory is for user preferences and project context only — never use it as a substitute for fixing the actual skill/pattern code.
