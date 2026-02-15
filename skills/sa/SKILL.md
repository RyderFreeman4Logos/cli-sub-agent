# sa: Sub-Agent Orchestration (Three-Tier Architecture)

Three-tier recursive delegation for planning and implementing features.
Main agent dispatches, never touches files. Child CSA plans and builds.

## Architecture

```
┌──────────────────────────────────────────────────┐
│  Tier 0: Dispatcher (Main Agent / You)           │
│                                                  │
│  • Pure dispatch — NEVER touch files/git/build   │
│  • Read result.toml path → present to user       │
│  • Gate user approval between phases             │
│  • Resume child session for next phase           │
│  • EVERYTHING ELSE → Tier 1                      │
└────────────────┬─────────────────────────────────┘
                 │ csa run --tool codex (default)
                 │ csa run --tool claude-code (Opus-only)
                 v
┌──────────────────────────────────────────────────┐
│  Tier 1: Planner / Implementer (codex default)   │
│                                                  │
│  • Planning: explore codebase, draft TODO        │
│  • Implementation: write code                    │
│  • Build: pre-commit, commit, push, PR           │
│  • Review: csa review, pr-codex-bot, merge       │
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
└──────────────────────────────────────────────────┘
```

## When to Use

| Use `sa` | Use `csa run` directly instead |
|----------|--------------------------------|
| Multi-step feature (planning + implementation) | Single well-defined task |
| Cross-cutting concerns (>3 files) | Isolated change (1-2 files) |
| Full lifecycle (branch → merge) | Already have branch/PR |
| Starting from requirements, not code | Already know what to change |

## Tool Selection (CRITICAL)

| Task Type | Tool | Rationale |
|-----------|------|-----------|
| Most tasks (default) | `codex` | Cheap, fast, good enough |
| Complex planning with debate | `claude-code` | Needs adversarial reasoning |
| Security-critical review | `claude-code` | Needs Opus-level depth |
| Everything else | `codex` | Cost efficiency |

**Rule**: Default to `codex`. Only escalate to `claude-code` when the task
explicitly requires Opus-level reasoning (architectural decisions, security
audit, subtle bug investigation). When in doubt, use `codex`.

## Model & Thinking Budget (CRITICAL)

Every `csa run` MUST specify model spec via `--thinking <level>`.
Match thinking budget to task difficulty and error probability.

**Format**: `tool/provider/model/thinking_budget`

### Baseline Levels

| Phase | Model Spec | Rationale |
|-------|-----------|-----------|
| Planning / exploration | `codex/openai/gpt-5.3-codex/medium` | Low risk, reconnaissance |
| Implementation (writing code) | `codex/openai/gpt-5.3-codex/medium` | Well-defined from TODO |
| Pre-commit fix | `codex/openai/gpt-5.3-codex/medium` | Mechanical, verifiable |
| PR creation + bot trigger | `codex/openai/gpt-5.3-codex/medium` | Mechanical flow |
| pr-codex-bot evaluation | `codex/openai/gpt-5.3-codex/medium` | Comment triage is routine |

### Escalation Levels (when issues found)

| Situation | Action | Model Spec |
|-----------|--------|-----------|
| Bot found real bug → fix | **Raise self** (same session) | `codex/openai/gpt-5.3-codex/xhigh` |
| Bot comment needs debate | **New CSA session** for arbiter | `claude/anthropic/claude-opus-4.6/xhigh` |
| Heterogeneous review (pre-merge) | **New CSA session** | `claude/anthropic/claude-opus-4.6/xhigh` |
| Complex planning with debate | **New CSA session** | `claude/anthropic/claude-opus-4.6/high` |
| Security-critical audit | **New CSA session** | `claude/anthropic/claude-opus-4.6/xhigh` |

### Escalation Rules

1. **Start low, escalate on failure** — begin at `medium`, only raise when
   the task proves harder than expected (compile failure, review issues, etc.)
2. **Fix = raise self** — when fixing a found bug, temporarily increase thinking
   budget within the same CSA session (avoid session proliferation)
3. **Debate/review = new session, different model** — adversarial review MUST
   use a different model family to provide independent cognitive diversity.
   Never debate with the same model that wrote the code.
4. **Never stay at xhigh** — after the hard sub-task is done, return to
   baseline `medium` for the next routine step

### CLI Syntax

```bash
# Baseline (medium thinking)
csa run --tool codex --thinking medium "implement feature X"

# Escalated fix (xhigh thinking, same tool)
csa run --tool codex --thinking xhigh --session $ID "fix this bug: ..."

# Debate/review (different model family, new session)
csa run --tool claude-code --thinking xhigh "review this diff for security issues"

# Or via csa debate (auto-routes to independent model)
csa debate "evaluate whether this bot comment is a false positive: ..."
```

## Tier 0: Dispatcher (STRICT RULES)

### MUST

| Action | How |
|--------|-----|
| Dispatch planning prompt to Tier 1 | `csa run --tool codex < prompt.txt` |
| Read result.toml | Parse status, session_id, artifact paths |
| Present TODO/results to user | Show artifact paths, ask for approval |
| Gate approval between phases | APPROVE / MODIFY / REJECT |
| Resume Tier 1 session | `csa run --tool codex --session <ID> < prompt.txt` |

### MUST NOT (ABSOLUTE — VIOLATION = SOP BREACH)

| Forbidden Action | Why |
|------------------|-----|
| `Read` any source file | Context pollution |
| `Grep` / `Glob` for code | Context pollution |
| `Bash` for git operations | Tier 1's job |
| `Bash` for pre-commit/build | Tier 1's job |
| `Edit` / `Write` source files | Tier 1's job |
| Create branches | Tier 1's job |
| Run `just pre-commit` | Tier 1's job |
| `git commit` / `git push` | Tier 1's job |
| `gh pr create` | Tier 1's job |
| Run `csa review` | Tier 1's job |
| Run pr-codex-bot workflow | Tier 1's job |
| Pre-fetch data for CSA | CSA reads files natively |

**The ONLY Bash Tier 0 may run**: `csa run`, `csa session show`, reading
result.toml. Nothing else.

## Tier 1: End-to-End Executor

Tier 1 handles the FULL lifecycle. Tier 0 never needs to touch anything.

### Planning Phase

```
Tier 1 receives planning prompt
    │
    ├── Spawn Tier 2 workers for parallel exploration
    │   └── csa run --tool codex "explore X"  (up to 3)
    │
    ├── Synthesize findings into TODO
    │   └── Write artifacts/TODO.md
    │
    ├── Run adversarial debate (optional, for complex tasks)
    │   └── csa debate < artifacts/TODO.md
    │
    └── Write result.toml
        └── { status, session_id, todo_path }
```

### Implementation Phase (END-TO-END)

After user approves TODO, Tier 1 does EVERYTHING:

```
1. Create feature branch
   └── git checkout -b <branch> main

2. Implement changes (per TODO)
   ├── Write code
   ├── Delegate errors to Tier 2
   │   └── csa run --tool codex "fix these errors: ..."
   └── Verify Tier 2 fixes preserve intent

3. Run pre-commit
   └── just pre-commit
       ├── PASS → continue
       └── FAIL → fix (self or Tier 2) → re-run

4. Commit
   └── git add + git commit (Conventional Commits)

5. Run heterogeneous review
   └── csa review --diff (or --branch main)

6. Push + Create PR
   └── git push -u origin <branch>
   └── gh pr create --base main

7. Run pr-codex-bot workflow
   ├── Trigger @codex review
   ├── Poll for bot response
   ├── Evaluate comments (fix real issues, debate false positives)
   ├── Clean resubmission if needed
   └── Merge when clean

8. Write final result.toml
   └── { status, commit_hash, pr_url, review_result }
```

**Tier 1 owns Steps 1-8. Tier 0 NEVER intervenes in this pipeline.**

### result.toml Schema

```toml
[result]
status = "success"   # success | partial | error
session_id = "01KH..."
error_code = ""

[artifacts]
# Planning phase:
todo_path = "artifacts/TODO.md"
# Implementation phase:
commit_hash = "abc1234"
pr_url = "https://github.com/user/repo/pull/92"
pr_number = 92
review_result = "CLEAN"  # CLEAN | HAS_ISSUES | MERGED
branch = "refactor/skills-to-patterns"
```

## Workflow

```
User: "Implement feature X"
    │
    v
[Tier 0] Write planning prompt → csa run --tool codex < prompt
    │
    v
[Tier 1] Explore → Draft TODO → (Debate) → result.toml
    │
    v
[Tier 0] Read result.toml → Present TODO path to user
    │
    v
User: APPROVE / MODIFY / REJECT
    │
    v (APPROVE)
[Tier 0] Write impl prompt → csa run --tool codex --session <ID> < prompt
    │
    v
[Tier 1] Branch → Code → Pre-commit → Commit → Review → PR → Bot → Merge
    │  → result.toml { commit_hash, pr_url, review_result = "MERGED" }
    │
    v
[Tier 0] Read result.toml → Report to user
    │
    v
Done ✅
```

## Prompt Templates

### Planning Dispatch (Tier 0 → Tier 1)

```bash
PROMPT_FILE=$(mktemp /tmp/sa-planning-XXXXXX.txt)
cat > "$PROMPT_FILE" <<'EOF'
You are in sa planning mode (Tier 1).

MANDATORY: Read and follow ALL applicable rules in ./AGENTS.md and ./CLAUDE.md.
These define commit conventions, code style, testing requirements, security
practices, and workflow constraints. Violations are SOP breaches.

TASK:
[user's task description]

PROCEDURE:
1. Explore the codebase (spawn Tier 2 codex workers if needed)
2. Draft TODO to artifacts/TODO.md
3. Write result.toml with session_id and todo_path

OUTPUT: Print ONLY the result.toml path.
EOF

csa run --tool codex --thinking medium < "$PROMPT_FILE"
```

### Implementation Dispatch (Tier 0 → Tier 1)

```bash
PROMPT_FILE=$(mktemp /tmp/sa-impl-XXXXXX.txt)
cat > "$PROMPT_FILE" <<'EOF'
User approved the TODO. Execute the FULL lifecycle:

MANDATORY: Read and follow ALL applicable rules in ./AGENTS.md and ./CLAUDE.md.
These define commit conventions, code style, testing requirements, security
practices, and workflow constraints. Violations are SOP breaches.

1. Create feature branch from main
2. Implement TODO items IN ORDER, committing INCREMENTALLY:
   - After each TODO block (A, B, C, ...) passes `just pre-commit`, commit it
   - Do NOT accumulate all changes into one monolithic commit
   - Each commit = one logical TODO block (Conventional Commits format)
3. Run `just pre-commit` after each block — fix until passing before next block
4. Run `csa review --branch main` for heterogeneous review after all blocks done
   → Use: csa run --tool claude-code --thinking xhigh "review ..."
5. Push and create PR via `gh pr create --base main`
6. Execute pr-codex-bot workflow:
   - Comment `@codex review` on PR
   - Poll for bot response (max 10 min)
   - Evaluate bot comments at medium thinking
   - If real bug: escalate to xhigh thinking to fix (same session)
   - If needs debate: new csa session with claude-code/xhigh
   - Clean resubmission if needed
   - Merge when clean
7. Write result.toml with final status

MODEL BUDGET RULES:
- Default: codex/medium for routine steps
- Fix bugs: raise to codex/xhigh (same session)
- Debate/review: NEW session with claude-code/xhigh (different model family)
- After hard sub-task: return to medium

OUTPUT: Print ONLY the result.toml path.
EOF

csa run --tool codex --thinking medium --session "$SESSION_ID" < "$PROMPT_FILE"
```

## Error Handling

### Tier 1 Failures

If Tier 1 returns `status = "error"`:
1. Read `error_code` from result.toml
2. Report to user with error details
3. Ask user for guidance (retry, modify, abandon)
4. NEVER take over the task yourself

### Quota / Cooldown

If CSA reports quota exhaustion:
1. STOP immediately
2. Report to user which tool hit the limit
3. Ask user: wait, switch tool, or proceed without CSA
4. NEVER silently fall back to single-model

## Forbidden Behaviors

### Tier 0 (Dispatcher)

- NEVER read source files
- NEVER run git/build/test commands
- NEVER create branches, commits, PRs
- NEVER run csa review or pr-codex-bot
- NEVER pre-fetch data for CSA
- NEVER fix code — always delegate

### Tier 1 (Executor)

- NEVER skip heterogeneous review
- NEVER retry Tier 2 on the same failure (escalate)
- NEVER accept Tier 2 fixes blindly (verify intent)
- NEVER skip pr-codex-bot before merge

### Tier 2 (Worker)

- NEVER make architectural decisions
- NEVER delete code to "fix" errors
- NEVER change function signatures without Tier 1 approval
