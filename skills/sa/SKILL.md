---
name: sa
description: Three-tier recursive delegation for planning and implementing features with main agent dispatching and child CSA building
allowed-tools: Bash, Read, Grep, Glob, Task
---

# sa: Sub-Agent Orchestration (Three-Tier Architecture)

Three-tier recursive delegation for planning and implementing features.
Main agent dispatches, never touches files. Child CSA plans and builds.
Grandchild CSA explores and fixes errors.

## Architecture

```
┌──────────────────────────────────────────────────┐
│  Tier 0: Dispatcher (Main Agent / You)           │
│                                                  │
│  • Pure dispatch — NEVER read source/transcripts │
│  • Parse result.toml path → present to user      │
│  • Gate user approval between phases             │
│  • Resume child session for next phase           │
└────────────────┬─────────────────────────────────┘
                 │ csa run --tool claude-code
                 v
┌──────────────────────────────────────────────────┐
│  Tier 1: Planner / Implementer (claude-code)     │
│                                                  │
│  • Planning: explore via Tier 2, draft TODO      │
│  • Debate: adversarial review via csa debate     │
│  • Implementation: write code                    │
│  • Review: audit Tier 2 work, get reviewed       │
│  • Output: result.toml with artifact paths       │
└────────────────┬─────────────────────────────────┘
                 │ csa run --tool codex
                 v
┌──────────────────────────────────────────────────┐
│  Tier 2: Worker (codex)                          │
│                                                  │
│  • Exploration: codebase search, patterns        │
│  • Error fixing: compile, lint, pre-commit       │
│  • Grunt work: boilerplate, formatting           │
│  • Output: result.toml with findings / fixes     │
└──────────────────────────────────────────────────┘
```

## When to Use

| Use `sa` | Use `csa run` directly instead |
|----------|--------------------------------|
| Multi-step feature (planning + implementation) | Single well-defined task |
| Cross-cutting concerns (>3 files) | Isolated change (1-2 files) |
| Wants heterogeneous review | Simple change |
| Starting from requirements, not code | Already know what to change |

## Tier Responsibilities

### Tier 0: Dispatcher (Main Agent)

| MUST | MUST NOT |
|------|----------|
| Read result.toml (structured metadata) | Read source files |
| Present artifact paths to user | Read CSA session transcripts |
| Gate approval between phases | Fix code directly |
| Resume child sessions | Run `csa review`/`csa debate` |
| Track progress via Task tools | Pre-fetch data for child CSA |

**Why pure dispatch**: The main agent's context window is precious. Every
file read pollutes it. Tier 1/2 have their own context windows — let them
do the heavy lifting.

### Tier 1: Planner / Implementer (claude-code)

Spawned via `csa run --tool claude-code`. Full tool access within its session.

**Planning phase**:
- Spawn Tier 2 for parallel codebase exploration (up to 3 concurrent)
- Draft TODO from Tier 2 findings
- Run adversarial debate via `csa debate`
- Write `result.toml` with TODO artifact path

**Implementation phase**:
- Write code following the TODO plan
- Delegate compile/lint errors to Tier 2 (codex is cheap and fast)
- Run heterogeneous review before commit
- Write `result.toml` with commit hash and review result

### Tier 2: Worker (codex)

Spawned by Tier 1 via `csa run --tool codex`. Lightweight, focused tasks.

**Scope**: exploration, error fixing, mechanical changes, boilerplate.

**Out of scope**: architectural decisions, security-critical code, complex
design. These stay at Tier 1.

Codex processes are lightweight (Rust binary, low CPU/memory). Spawn freely —
the resource cost is negligible compared to the quality benefit of heterogeneous
execution.

## Output Protocol

### result.toml

Child CSA returns ONLY the result.toml path. All details live in session
artifacts. Tier 0 NEVER reads the artifacts directly — it reads result.toml
for status and paths, then presents to the user.

```toml
[result]
status = "success"   # success | partial | error
error_code = ""      # empty on success
session_id = "019c4c24-9f5c-7502-96db-c72b71efd1c0"  # for Tier 0 to resume

[timing]
started_at = "2026-02-11T10:00:00Z"
ended_at = "2026-02-11T10:05:00Z"

[tool]
name = "claude-code"
version = "1.0.0"

[review]
author_tool = "claude-code"          # who wrote the code
reviewer_tool = "codex"              # who reviewed it (must differ)

[artifacts]
# Paths relative to session directory
# Planning phase:
todo_path = "artifacts/TODO.md"
# Implementation phase:
commit_hash = "abc1234"
review_result = "CLEAN"              # CLEAN | HAS_ISSUES
review_path = "artifacts/review.md"
```

**Path safety**: All artifact paths in result.toml MUST be relative and MUST
NOT contain `..`. Tier 0 should resolve paths against the session directory
and verify they don't escape it.

### Tier 0 Reads result.toml

Tier 0 extracts `session_id` and artifact paths from result.toml. Use
`csa session result <id>` when available; the grep examples below are
fallback for quick reference:

```bash
# result.toml path is printed by CSA as last output line
RESULT_PATH="<last line of csa output>"
SESSION_ID=$(grep 'session_id = ' "$RESULT_PATH" | cut -d'"' -f2)
STATUS=$(grep 'status = ' "$RESULT_PATH" | head -1 | cut -d'"' -f2)
TODO_PATH=$(grep 'todo_path = ' "$RESULT_PATH" | cut -d'"' -f2)
```

## Implementation Phase Protocol

### Heterogeneous Review

Before commit, code MUST be reviewed by a different tool family than the author:

| Author | Reviewer | Mechanism |
|--------|----------|-----------|
| Tier 1 (claude-code) | Tier 2 (codex) | `csa review --diff` (auto-selects codex) |
| Tier 2 (codex) | Tier 1 (claude-code) | Tier 1 reads Tier 2 output and verifies |

```
Tier 1 writes code
    │
    v
Tier 1 runs: csa review --diff
    │
    +── CLEAN → commit
    │
    +── HAS_ISSUES → fix → re-review (max 3 rounds)
```

### Error Delegation

When Tier 1 hits compile/lint/pre-commit errors:

```
Tier 1 runs: just pre-commit
    │
    +── PASS → continue
    │
    +── FAIL → delegate to Tier 2
                   │
                   v
              csa run --tool codex --prompt-file <errors>
                   │
                   +── Fixed → Tier 1 verifies intent preserved
                   │
                   +── Failed → Tier 1 fixes directly (escalation)
```

**Guard rail**: Tier 1 MUST verify Tier 2 fixes don't delete functionality,
comment out code, or change semantics to "fix" errors. If Tier 2 fails on
the same error, Tier 1 takes over — NEVER retry Tier 2 on the same failure.

### Commit Protocol

Each logical unit follows:

```
Implement → just pre-commit → csa review --diff → fix → commit
```

Tier 1 uses the `commit` skill workflow internally. Each commit produces
a result.toml entry with `commit_hash`.

## Forbidden Behaviors

### Tier 0 (Dispatcher)

- NEVER `Read` source files (only read result.toml for structured metadata)
- NEVER `Grep`/`Glob` for code exploration
- NEVER read CSA session transcripts (only the result.toml path)
- NEVER fix code — always delegate to Tier 1
- NEVER run `csa review` or `csa debate` directly

### Tier 1 (Planner / Implementer)

- NEVER make architectural decisions without debate
- NEVER skip heterogeneous review before commit
- NEVER retry Tier 2 on the same failure (escalate)
- NEVER accept Tier 2 fixes blindly (verify intent preservation)

### Tier 2 (Worker)

- NEVER make architectural decisions
- NEVER delete code to "fix" errors
- NEVER comment out problematic sections
- NEVER change function signatures without Tier 1 approval

## sa-mktd: Planning Phase Protocol

### Tier 0 Dispatches Planning

The main agent constructs a planning prompt and dispatches to Tier 1.
NEVER pre-read files — include only the user's requirements.

```bash
# Use mktemp for unique temp files (avoids race conditions / symlink attacks)
PROMPT_FILE=$(mktemp /tmp/sa-planning-XXXXXX.txt)
trap 'rm -f "$PROMPT_FILE"' EXIT

# Build prompt safely: write user description first, then append instructions
{
  echo "You are in sa planning mode (Tier 1)."
  echo ""
  echo "TASK:"
  cat /tmp/sa-feature-desc.txt   # user input stays in data position, never in shell syntax
  echo ""
  cat <<'INSTRUCTIONS'
PROCEDURE:
1. Spawn up to 3 parallel Tier 2 workers for codebase exploration:
   - csa run --tool codex "Analyze structure relevant to the task. Report: files, types, dependencies."
   - csa run --tool codex "Find existing patterns similar to the task. Report: reusable components, conventions."
   - csa run --tool codex "Identify constraints and risks. Report: breaking changes, security, perf."

2. Synthesize Tier 2 findings into a TODO draft.
   Write the TODO to session artifacts/TODO.md.

3. Run adversarial debate on the draft:
   csa debate --prompt-file artifacts/TODO.md

4. Revise TODO based on debate findings.

5. Write result.toml with session_id and:
   [artifacts]
   todo_path = "artifacts/TODO.md"

OUTPUT: Print ONLY the result.toml path. Do NOT print TODO content.
INSTRUCTIONS
} > "$PROMPT_FILE"

csa run --tool claude-code --prompt-file "$PROMPT_FILE"
```

**Prompt safety rules**:
- Always use `mktemp` for temp files (unique names, no race conditions)
- Use `cat` to include user data in data position (not shell syntax)
- Use heredoc with literal delimiters (`<<'EOF'`) for static template parts
- NEVER interpolate user input or error output into shell command strings

### Tier 0 Presents TODO

After Tier 1 returns:

```bash
# 1. Extract result.toml path (last line of CSA structured output)
RESULT_PATH="<last line>"

# 2. Read session_id from result.toml (Tier 0's only source for session ID)
SESSION_ID=$(grep 'session_id = ' "$RESULT_PATH" | cut -d'"' -f2)
STATUS=$(grep 'status = ' "$RESULT_PATH" | head -1 | cut -d'"' -f2)

# 3. If success, read TODO path with path escape validation
TODO_REL=$(grep 'todo_path = ' "$RESULT_PATH" | cut -d'"' -f2)
SESSION_DIR=$(dirname "$RESULT_PATH")
TODO_ABS=$(realpath -m "${SESSION_DIR}/${TODO_REL}")
# Path escape check: must stay within session directory
case "$TODO_ABS" in
  "${SESSION_DIR}"/*) ;; # OK
  *) echo "ERROR: artifact path escapes session directory"; exit 1 ;;
esac
echo "TODO plan ready at: ${TODO_ABS}"
```

Present the TODO path to the user. Let the user read and approve/modify.

### User Approval Gate

| User says | Action |
|-----------|--------|
| APPROVE | Resume Tier 1 for implementation (sa-mktsk) |
| MODIFY | Resume Tier 1 with feedback for revision |
| REJECT | Abandon plan, ask user for new direction |

```bash
RESUME_FILE=$(mktemp /tmp/sa-resume-XXXXXX.txt)
trap 'rm -f "$RESUME_FILE"' EXIT

# APPROVE: resume for implementation
printf '%s' "User approved the TODO. Begin implementation." > "$RESUME_FILE"
csa run --tool claude-code --session "$SESSION_ID" --prompt-file "$RESUME_FILE"

# MODIFY: write feedback to file, then resume
printf '%s' "User feedback follows. Revise the TODO accordingly." > "$RESUME_FILE"
cat /tmp/sa-user-feedback.txt >> "$RESUME_FILE"  # user data appended safely
csa run --tool claude-code --session "$SESSION_ID" --prompt-file "$RESUME_FILE"
```

## sa-mktsk: Implementation Phase Protocol

### Tier 0 Dispatches Implementation

After user approval, resume the Tier 1 session:

```bash
IMPL_FILE=$(mktemp /tmp/sa-impl-XXXXXX.txt)
trap 'rm -f "$IMPL_FILE"' EXIT

cat > "$IMPL_FILE" <<'PROMPT_EOF'
User approved the TODO. Begin implementation.

PROCEDURE:
1. Read the TODO from artifacts/TODO.md.
2. For each task in order (strictly serial):
   a. Implement the change.
   b. Run: just pre-commit
      - If FAIL: delegate to Tier 2. Write errors to a temp file (mktemp), then:
        csa run --tool codex --prompt-file "$ERRORS_FILE"
        (prompt content: "Fix these errors. Do NOT delete code or change semantics.")
        Verify Tier 2 fix preserves intent. If Tier 2 fails, fix it yourself.
   c. Run heterogeneous review: csa review --diff
      - If HAS_ISSUES: fix and re-review (max 3 rounds).
   d. Commit via commit skill workflow.
3. After all tasks complete, write result.toml with session_id and:
   [artifacts]
   commit_hash = "<last commit hash>"
   review_result = "CLEAN"

OUTPUT: Print ONLY the result.toml path.
PROMPT_EOF

csa run --tool claude-code --session "$SESSION_ID" --prompt-file "$IMPL_FILE"
```

### Error Delegation Detail

Tier 1's internal loop for each task:

```
Write code for task N
    │
    v
just pre-commit
    │
    +── PASS ──────────────────────────┐
    │                                  │
    +── FAIL                           │
         │                             │
         v                             │
    csa run --tool codex               │
      --prompt-file /tmp/errors.txt   │
         │                             │
         +── Fixed                     │
         │    │                        │
         │    v                        │
         │  Verify intent preserved    │
         │    │                        │
         │    +── OK ─────────────────>│
         │    +── Bad → revert, fix    │
         │              yourself ─────>│
         +── Failed                    │
              │                        │
              v                        │
         Fix yourself ────────────────>│
                                       │
                                       v
                               csa review --diff
                                       │
                                       +── CLEAN → commit
                                       +── HAS_ISSUES → fix → re-review
```

### Heterogeneous Review Detail

The review MUST use a different model family than the code author:

```bash
# Tier 1 (claude-code) wrote the code → review routes to codex
csa review --diff
# CSA auto-selects codex as reviewer (heterogeneous rule)

# If Tier 2 (codex) wrote a fix → Tier 1 reviews it directly
# (Tier 1 reads the diff and verifies intent preservation)
```

### Implementation Result

After all tasks complete, Tier 1 writes final result.toml:

```toml
[result]
status = "success"
error_code = ""
session_id = "019c4c24-9f5c-7502-96db-c72b71efd1c0"

[timing]
started_at = "2026-02-11T10:00:00Z"
ended_at = "2026-02-11T10:30:00Z"

[tool]
name = "claude-code"
version = "1.0.0"

[review]
author_tool = "claude-code"
reviewer_tool = "codex"

[artifacts]
commit_hash = "abc1234def"
review_result = "CLEAN"
tasks_completed = 5
tasks_total = 5
```

All result.toml files (planning and implementation phases) follow the same
schema. Required fields: `result.status`, `result.session_id`, `tool.name`.
Other fields are phase-dependent (planning has `todo_path`, implementation
has `commit_hash`/`review_result`).

## Integration

| Skill | Role in sa workflow |
|-------|---------------------|
| `mktd` | Planning — Tier 1 runs mktd internally |
| `mktsk` | Execution — Tier 1 converts TODO to tasks |
| `commit` | Per logical unit — Tier 1 commits |
| `csa-review` | Heterogeneous review — auto-selects different tool |
| `debate` | Planning — adversarial review of TODOs |

## Workflow Summary

```
User: "Implement feature X"
    │
    v
[Tier 0] Dispatch planning to Tier 1
    │  csa run --tool claude-code "{planning prompt}"
    v
[Tier 1] Explore (via Tier 2) → Draft TODO → Debate → result.toml
    │
    v
[Tier 0] Read result.toml → Present TODO to user
    │
    v
User: APPROVE / MODIFY / REJECT
    │
    v (APPROVE)
[Tier 0] Resume Tier 1 session for implementation
    │  csa run --tool claude-code --session <ID> "implement the TODO"
    v
[Tier 1] Write code → Delegate errors to Tier 2 → Review → Commit
    │  → result.toml { commit_hash, review_result }
    v
[Tier 0] Read result.toml → Report to user
    │
    v
Done (or iterate if HAS_ISSUES)
```
