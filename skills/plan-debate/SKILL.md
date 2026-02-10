---
name: plan-debate
description: >-
  Debate-enhanced planning that replaces built-in plan mode.
  Uses CSA for context-efficient exploration, heterogeneous model debate
  for adversarial plan review, and task-based decomposition for execution.
  Triggers on: plan, plan-debate, design, architecture, implement feature,
  refactor, when non-trivial multi-step task
  (>3 steps, multi-file, architectural decision)
allowed-tools: TaskCreate, TaskUpdate, TaskList, TaskGet, Read, Grep, Glob, Bash, Write, Edit, AskUserQuestion
---

# plan-debate: Debate-Enhanced Planning & Execution

## Purpose

Replace Claude Code's built-in plan mode (`EnterPlanMode`/`ExitPlanMode`) with a superior workflow that:

- **Zero main-agent file reads during exploration** — CSA sub-agents gather context
- **Mandatory adversarial review** — heterogeneous model debate catches blind spots
- **Pre-assigned execution** — every task has an executor tag before work begins
- **Dual persistence** — Task tools (survive auto-compact) + plan file (full audit trail)
- **End-to-end** — from reconnaissance through final commit

### Why This Replaces Plan Mode

| Dimension | Built-in Plan Mode | plan-debate |
|-----------|-------------------|-------------|
| Exploration context cost | Main agent reads files (high) | CSA reads files (zero) |
| Reasoning quality | Single model | Heterogeneous adversarial debate |
| Blind spot detection | None | Mandatory debate catches oversights |
| Execution planning | None (manual after plan) | Task-based executor pre-assignment |
| Audit trail | Single plan file | Debate transcript + task decomposition + session IDs |
| Context efficiency | ~50,000+ tokens | ~5,000 tokens (Phase 1-5) |

## When to Use

**Use this skill when:**
- ✅ Non-trivial task (>3 steps or multi-file changes)
- ✅ Architectural or design decisions needed
- ✅ Multiple valid approaches exist
- ✅ Multi-module changes with cross-cutting concerns
- ✅ User explicitly requests planning or `/plan-debate`

**Do NOT use when:**
- ❌ Single-file trivial change (<5 lines)
- ❌ Typo fix, comment addition, one-line edit
- ❌ User gave very specific, detailed instructions (just execute)
- ❌ Pure research / exploration (use Explore agent instead)

## The 6 Phases

```
Phase 1: RECON ─── CSA parallel exploration (zero main-agent file reads)
    │
Phase 2: DRAFT ─── Main agent drafts plan from CSA summaries
    │
Phase 3: DEBATE ── Mandatory adversarial review via csa debate
    │
Phase 4: DECOMPOSE ── Task breakdown + dual persistence
    │
Phase 5: APPROVE ── User reviews and approves
    │
Phase 6: EXECUTE ── Delegated execution per task-based protocol
```

---

## Phase 1: RECON (CSA Parallel Reconnaissance)

**Goal**: Gather codebase context without polluting main agent's context window.

**CRITICAL**: Main agent MUST NOT use Read, Glob, Grep, or Bash to directly explore files (e.g. `cat`, `grep`, `find`). The ONLY permitted Bash usage in Phase 1 is invoking CSA commands (`csa run`, `csa debate`). CSA sub-agents have direct file system access and read files natively.

### Execution

Launch up to 3 parallel CSA tasks, each exploring a different dimension:

```bash
# Dimension 1: Structure — What exists and how it's organized
csa run "Analyze codebase structure relevant to [FEATURE].
Report in this format:
- Relevant files (path + purpose, max 20)
- Key types/structs/interfaces
- Module dependencies
- Entry points
Working directory: [CWD]"

# Dimension 2: Patterns — What similar work has been done before
csa run "Find existing patterns or similar features to [FEATURE] in this codebase.
Report:
- File paths with approach used
- Reusable components/functions/traits
- Conventions to follow (naming, structure, error handling)
Working directory: [CWD]"

# Dimension 3: Constraints — What could go wrong
csa run "Identify constraints and risks for implementing [FEATURE].
Report:
- Potential breaking changes
- Dependency conflicts
- Test coverage gaps
- Performance implications
- Security considerations
Working directory: [CWD]"
```

### Output

Main agent receives 3 structured summaries (~500 tokens total). Synthesize into a unified context brief.

### FORBIDDEN in Phase 1

- ❌ Main agent reads any source file
- ❌ Main agent runs `git diff`, `grep`, `find`, etc.
- ❌ Pre-fetching data to pass to CSA
- ❌ Reading files "just to understand" — let CSA do it

---

## Phase 2: DRAFT (Plan Drafting)

**Goal**: Synthesize CSA reconnaissance into a concrete implementation plan.

### Plan File

Create the directory and write the draft plan to `./drafts/plans/{timestamp}/plan.md` (use filesystem-safe timestamp format, e.g. `20260209T143000`). The `Write` tool creates parent directories automatically; if using Bash, run `mkdir -p` first:

```markdown
# Plan: [Feature/Task Name]

## Goal
[1-2 sentences describing the outcome]

## Context Brief
[Synthesized findings from Phase 1 RECON — key files, patterns, constraints]

## Key Decisions
- Decision 1: [choice] because [reasoning]
- Decision 2: [choice] because [reasoning]

## Approach
1. [Step with file path]
2. [Step with file path]
...

## Risks & Mitigations
- Risk 1: [description] → Mitigation: [approach]

## Files to Modify
- `path/to/file.rs` — [what changes and why]
```

### Guidelines

- Keep plan concise (<2000 tokens)
- Reference specific file paths from RECON
- Identify key decisions explicitly (these will be debated)
- Note uncertainties honestly (debate will stress-test them)

---

## Phase 3: DEBATE (Mandatory Adversarial Review)

**Goal**: Heterogeneous model debate catches blind spots, flawed assumptions, and missed alternatives.

**THIS PHASE IS ALWAYS MANDATORY. NO EXCEPTIONS.**

### Round 1: Initial Critique

```bash
csa debate "Review this implementation plan critically:

[paste draft plan from Phase 2]

Critique for:
1. Completeness — missing steps, edge cases, error handling
2. Correctness — wrong assumptions, flawed logic, API misuse
3. Complexity — over-engineering, unnecessary abstractions, YAGNI violations
4. Alternatives — better approaches the plan missed
5. Risks — security, performance, maintenance concerns

Be adversarial. Challenge every assumption. Identify what will break."
```

### Evaluate Critique

Main agent reads the debate response and classifies each point:

| Classification | Action |
|---------------|--------|
| **Valid concern, plan is wrong** | Revise plan, debate again |
| **Valid concern, plan is incomplete** | Add missing steps to plan |
| **Minor suggestion** | Note in plan, no revision needed |
| **False positive** | Counter-argue in Round 2 |

### Round 2+ (If Needed, Max 3 Total)

If plan was revised or concerns need counter-argument:

```bash
csa debate --session <SESSION_ID> "I've revised the plan to address your concerns:

[revised sections or counter-arguments]

Remaining questions:
1. Does the revision adequately address [concern X]?
2. Any new concerns introduced by the revision?
3. Final recommendation: proceed, revise further, or abandon?"
```

### Record Debate Insights

Append to the plan file:

```markdown
## Debate Insights

**Session**: `<SESSION_ID>`
**Rounds**: N
**Key findings**:
- [insight that changed the plan]
- [risk that was newly identified]
- [alternative that was considered and rejected, with reason]

**Resolved tensions**:
- [tension]: resolved by [approach]

**Remaining uncertainties**:
- [uncertainty]: accepted because [reason]
```

---

## Phase 4: DECOMPOSE (Task Breakdown + Dual Persistence)

**Goal**: Break the refined plan into executable tasks with explicit executor assignment.

### Executor Tags

| Tag | When to Use |
|-----|-------------|
| `[Main]` | Info already in context, trivial (<10 lines), needs user interaction |
| `[Sub:developer]` | Well-defined implementation task |
| `[Sub:Explore]` | Large-scale code search, architecture understanding |
| `[CSA:auto]` | Large file analysis, documentation research |
| `[CSA:review]` | Code review, diff analysis |
| `[CSA:debate]` | Architecture decisions needing adversarial review |
| `[CSA:codex]` | Independent feature implementation (sandbox) |
| `[Skill:commit]` | Full commit workflow (format/lint/test/review/commit) |

### Dual Persistence

**1. Task tools (survive auto-compact)**:

```python
TaskCreate(
    subject="[Sub:developer] Implement JWT validation logic",
    description="Implement in src/auth/jwt.rs. DONE WHEN: targeted tests pass (e.g., `cargo test auth::jwt`, `npm test -- auth`, `pytest auth/`).",
    activeForm="Implementing JWT validation logic"
)

TaskCreate(
    subject="[Skill:commit] Commit JWT validation",
    description="Full commit workflow. DONE WHEN: git log --oneline -1 matches feat(auth):",
    activeForm="Committing JWT validation (format/lint/test/review/commit)"
)
```

**2. Plan file (full context)**:

Append `## Execution Plan` to the plan file with all tasks, their DONE WHEN conditions, and dependencies.

### Mandatory Rules

- **Every task MUST have an executor tag** — no exceptions
- **Every task MUST have a DONE WHEN condition** — mechanically verifiable
- **Auto-inject `[Skill:commit]`** after each implementation task
- **Auto-inject `/compact`** after completing a logical stage
- **Serial writes, parallel reads** — NEVER parallel development sub-agents

### FORBIDDEN Operations for `[Main]`

| Operation | MUST Use Instead | Why |
|-----------|------------------|-----|
| Read git diff (>100 lines) | `[CSA:review]` or `[CSA:auto]` | Save Opus tokens |
| Read >3 files for analysis | `[CSA:auto]` or `[Sub:Explore]` | Prevent context bloat |
| Generate commit message | `[Skill:commit]` | Encapsulated workflow |
| Large-scale code search | `[Sub:Explore]` | Specialized tool |
| Review code changes | `[CSA:review]` | Specialized tool |

---

## Phase 5: APPROVE (User Gate)

**Goal**: Present refined plan to user for approval before execution.

### Present to User

Use AskUserQuestion to present:
1. **Plan summary** (from Phase 2, refined by Phase 3)
2. **Debate key insights** (what changed, what was caught)
3. **Task list** (with executor assignments)
4. **Estimated scope** (number of tasks, files touched)

### User Options

- **Approve** → Proceed to Phase 6
- **Modify** → Adjust plan/tasks per feedback, re-present
- **Reject** → Abandon plan, ask user for new direction

---

## Phase 6: EXECUTE (Delegated Execution)

**Goal**: Execute all tasks per task-based protocol until completion.

### Core Rules

| Rule | Requirement |
|------|-------------|
| **Immediate start** | After approval, begin executing immediately — no further confirmation |
| **Continuous execution** | Complete one task, move to next — do not pause between tasks |
| **Stop only when done** | All tasks completed, OR unresolvable blocker requiring user input |
| **Serial writes** | NEVER run parallel development sub-agents |
| **Atomic commits** | `[Skill:commit]` after each logical unit |
| **Proactive compact** | `/compact` when context > 80% or after stage completion |
| **Task persistence** | Task tools survive auto-compact — check TaskList after compact |

### Execution Loop

```
For each task in TaskList (in order):
    1. TaskUpdate(taskId, status="in_progress")
    2. Execute per executor tag:
       - [Main] → handle directly
       - [Sub:*] → delegate to sub-agent
       - [CSA:*] → delegate to CSA (NO pre-fetching)
       - [Skill:commit] → invoke /commit skill
    3. Verify DONE WHEN condition
    4. TaskUpdate(taskId, status="completed")
    5. If logical stage complete → /compact
    6. Continue to next task
```

### When to Pause (Wait for User)

✅ **Must pause**:
- Requirements unclear, need user clarification
- Multiple viable approaches, need user choice
- Critical issue discovered that changes scope
- Needs credentials or access permissions

❌ **Must NOT pause**:
- Normal task execution flow
- Auto-fixable errors (lint, format)
- Moving to next task
- Delegatable operations

### Commit Workflow

**Every implementation task is followed by `[Skill:commit]`** which automatically:
1. Run your project's formatter (as defined in CLAUDE.md)
2. Run your project's linter
3. Run your project's test suite (or targeted tests)
4. Security scan
5. Pre-commit review (`csa review --diff`)
6. Fix issues if any (codex in same session)
7. Re-review until clean
8. Generate commit message (Conventional Commits)
9. Commit

**Reference**: See the `commit` skill for full protocol.

### Context Window Management

| Trigger | Action |
|---------|--------|
| Logical stage complete | `/compact Keep [decisions]. Summarize [process].` |
| Context > 80% | `/compact` immediately |
| Read 5+ files (not referenced later) | `/compact` after gathering |
| Topic switch | `/compact` before switching |

**Task tools persist across `/compact`** — this is the core value of dual persistence.

---

## CSA Integration Rules

### FORBIDDEN: Pre-fetching Data for CSA

**YOU ARE ABSOLUTELY FORBIDDEN from using Read, Glob, Grep, or Bash to gather information for CSA.**

| Action | FORBIDDEN | CORRECT |
|--------|-----------|---------|
| Analyze files | Read 50 files, pass content | `csa run "analyze src/**/*.ts"` |
| Check git changes | `git diff`, pass output | `csa run "run git diff yourself"` |
| Search codebase | Grep pattern, pass results | `csa run "find pattern in **/*.ts"` |

**Why**: CSA tools have direct file system access. Pre-fetching wastes ~50,000 Claude tokens.

### CSA Tool Selection

| Tool Mode | When | Command |
|-----------|------|---------|
| `auto` (default) | High-value tasks | `csa run "..."` |
| `any-available` | Low-risk exploration | `csa run --tool any-available "..."` |
| Explicit | Specific backend needed | `csa run --tool codex "..."` |
| Debate | Adversarial review | `csa debate "..."` |
| Review | Code review | `csa review --diff` |

---

## Forbidden Operations (STRICT)

- ❌ **Phase 1**: Main agent reads files (must use CSA)
- ❌ **Phase 3**: Skip debate (always mandatory, no exceptions)
- ❌ **Phase 4**: Task without executor tag
- ❌ **Phase 4**: Task without DONE WHEN condition
- ❌ **Phase 6**: Parallel write operations
- ❌ **Phase 6**: Pre-fetch data for CSA
- ❌ **Phase 6**: Commit without `[Skill:commit]`
- ❌ **Phase 6**: Accumulate changes across logical units
- ❌ **Any phase**: Multiple tasks `in_progress` simultaneously
- ❌ **Any phase**: Use `EnterPlanMode`/`ExitPlanMode` (this skill replaces them)

---

## Complete Example: Adding JWT Authentication

```python
# Phase 1: RECON — 3 parallel CSA tasks
csa run "Analyze auth-related code structure in this project..."
csa run "Find existing auth patterns, middleware, token handling..."
csa run "Identify security constraints, dependency requirements..."

# Phase 2: DRAFT — Main agent writes plan from summaries
Write("./drafts/plans/20260209T143000/plan.md", plan_content)

# Phase 3: DEBATE — Mandatory adversarial review
csa debate "Review this JWT auth plan critically: [plan]..."
# Debate catches: missing token refresh, CSRF risk, rate limiting gap
# Revise plan, debate again:
csa debate --session 01JK... "Revised plan addresses: [changes]..."

# Phase 4: DECOMPOSE — Create tasks with dual persistence

TaskCreate(
    subject="[Sub:Explore] Find all authentication entry points",
    description="Search for auth middleware, login routes, token checks. DONE WHEN: sub-agent returns structured report listing file paths.",
    activeForm="Finding authentication entry points"
)

TaskCreate(
    subject="[Sub:developer] Implement JWT token validation",
    description="Create src/auth/jwt.rs with validate_token(). DONE WHEN: targeted tests pass (e.g., `cargo test auth::jwt`, `npm test -- auth`, `pytest auth/`).",
    activeForm="Implementing JWT validation"
)

TaskCreate(
    subject="[Skill:commit] Commit JWT validation",
    description="Conventional Commits: feat(auth): add JWT token validation. DONE WHEN: git log -1 matches.",
    activeForm="Committing JWT validation"
)

TaskCreate(
    subject="[Sub:developer] Add login endpoint with JWT issuance",
    description="Add POST /api/auth/login to src/api/auth.rs. DONE WHEN: targeted tests pass.",
    activeForm="Adding login endpoint"
)

TaskCreate(
    subject="[Skill:commit] Commit login endpoint",
    description="Conventional Commits: feat(auth): add login endpoint with JWT. DONE WHEN: git log -1 matches.",
    activeForm="Committing login endpoint"
)

TaskCreate(
    subject="[Main] Verify git status clean and report completion",
    description="DONE WHEN: git status shows clean working tree.",
    activeForm="Verifying completion"
)

# Phase 5: APPROVE — AskUserQuestion with plan + tasks + debate insights

# Phase 6: EXECUTE — Serial execution per task-based protocol
# TaskUpdate → Execute → Verify DONE WHEN → TaskUpdate completed → Next
```

---

## Done Criteria

| Phase | Verification |
|-------|-------------|
| Phase 1 (RECON) | 3 CSA summaries received, zero main-agent file reads |
| Phase 2 (DRAFT) | Plan file exists at `./drafts/plans/{timestamp}/plan.md` |
| Phase 3 (DEBATE) | Plan file contains `## Debate Insights` with session ID |
| Phase 4 (DECOMPOSE) | All tasks created via TaskCreate with executor + DONE WHEN |
| Phase 5 (APPROVE) | User explicitly approved |
| Phase 6 (EXECUTE) | All tasks status=completed AND `git status` clean |

## ROI

| Benefit | Impact |
|---------|--------|
| Context efficiency | ~90% reduction (5K vs 50K+ tokens for planning) |
| Plan quality | Adversarial debate catches blind spots single-model misses |
| Execution clarity | Every task has pre-assigned executor — no ad-hoc decisions |
| Persistence | Tasks survive auto-compact; plan file preserves full audit trail |
| Audit trail | Debate session IDs + task history = complete decision record |
| Code quality | Mandatory commit skill = format + lint + test + review per unit |

---

## Appendix: Phase Templates

Detailed prompt templates for plan-debate Phases 1-4. Phases 5-6 are procedural (no prompt templates needed — see notes at end).

### Phase 1: RECON Templates

#### Template 1A: Structure Analysis

```bash
csa run "Analyze the codebase structure relevant to implementing [FEATURE].

Working directory: [CWD]

Report (max 500 tokens):
1. **Relevant files** (path + one-line purpose, max 20 files)
2. **Key types** (structs, traits, interfaces used in this area)
3. **Module dependencies** (what imports what)
4. **Entry points** (where control flow starts for this feature area)
5. **Config/env** (any configuration or environment variables involved)

Focus on files that would need to change or be referenced."
```

#### Template 1B: Pattern Discovery

```bash
csa run "Find existing patterns or similar features to [FEATURE] in this codebase.

Working directory: [CWD]

Report (max 500 tokens):
1. **Similar implementations** (file path + what it does + approach used)
2. **Reusable components** (functions, traits, modules that can be leveraged)
3. **Conventions** (naming style, error handling approach, module layout)
4. **Test patterns** (how similar features are tested, test file locations)
5. **Anti-patterns** (existing code that should NOT be copied, with reason)

If no similar feature exists, say so and suggest analogous patterns from the codebase."
```

#### Template 1C: Constraint Identification

```bash
csa run "Identify constraints and risks for implementing [FEATURE].

Working directory: [CWD]

Report (max 500 tokens):
1. **Breaking changes** (what existing behavior might change)
2. **Dependency risks** (new deps needed, version conflicts, license issues)
3. **Test coverage** (areas with poor coverage that this feature touches)
4. **Performance** (hot paths, large data, concurrency concerns)
5. **Security** (input validation, auth, data exposure risks)
6. **Compatibility** (API stability, backward compat requirements)

Be pessimistic — flag anything that could go wrong."
```

### Phase 2: DRAFT Template

#### Plan File Structure

```markdown
# Plan: [Feature/Task Name]

## Goal
[1-2 sentences: what this achieves and why it matters]

## Context Brief
[Synthesized from RECON — key files, patterns found, constraints identified]

**Key files**: `path/a.rs`, `path/b.rs`, `path/c.rs`
**Existing patterns**: [what similar code does and how]
**Critical constraints**: [security/perf/compat requirements]

## Key Decisions
- **Decision 1**: [what] — chose [option A] over [option B] because [reasoning]
- **Decision 2**: [what] — chose [option A] because [reasoning]

## Approach
1. [Step]: [what to do] in `path/to/file`
2. [Step]: [what to do] in `path/to/file`
3. [Step]: [what to do] in `path/to/file`
...

## Risks & Mitigations
- **Risk**: [description] → **Mitigation**: [approach]
- **Risk**: [description] → **Mitigation**: [approach]

## Files to Modify
| File | Change | Reason |
|------|--------|--------|
| `path/to/file.rs` | Add [function/struct] | [why] |
| `path/to/test.rs` | Add [test cases] | [coverage for what] |

## Open Questions
- [Anything unresolved that debate should address]
```

### Phase 3: DEBATE Templates

#### Template 3A: Initial Critique (Round 1)

```bash
csa debate "Review this implementation plan critically:

[PASTE DRAFT PLAN HERE]

Critique across 5 dimensions:

1. **Completeness** — Are any steps missing? Edge cases unhandled? Error scenarios ignored?
2. **Correctness** — Are assumptions valid? Is the API usage correct? Will the logic work?
3. **Complexity** — Is anything over-engineered? Are there YAGNI violations? Can it be simpler?
4. **Alternatives** — Is there a better approach? A library that solves this? A pattern that fits better?
5. **Risks** — Security gaps? Performance traps? Maintenance burden? Breaking changes?

Be adversarial. Assume the plan has flaws. Your job is to find them.
If the plan is genuinely good, say so — but challenge at least 3 specific assumptions."
```

#### Template 3B: Counter-Argument (Round 2+)

```bash
csa debate --session <SESSION_ID> "I've addressed your concerns:

**Concern 1** ([their concern]):
→ Response: [your counter-argument or plan revision]

**Concern 2** ([their concern]):
→ Response: [your counter-argument or plan revision]

**Concern 3** ([their concern]):
→ Conceded: [how you revised the plan]

Remaining questions:
1. Does revision X adequately address concern Y?
2. Any new risks introduced by the revisions?
3. Final recommendation: proceed as revised, revise further, or abandon approach?"
```

#### Template 3C: Debate Insights Record

```markdown
## Debate Insights

**Session**: `<SESSION_ID>`
**Rounds**: N
**Debate tool**: [auto-resolved tool, e.g., codex when parent is claude-code]

### Findings That Changed the Plan
- [Finding 1]: [how the plan was revised]
- [Finding 2]: [how the plan was revised]

### Considered and Rejected Alternatives
- [Alternative]: rejected because [reason]

### Resolved Tensions
- [Tension]: [how it was resolved]

### Remaining Uncertainties
- [Uncertainty]: accepted because [mitigation exists / low probability / acceptable risk]
```

### Phase 4: DECOMPOSE Templates

#### Executor Decision Tree

```
For each implementation step in the plan:
    │
    ├─ Is info already in context AND step is trivial (<10 lines)?
    │   └─ YES → [Main]
    │
    ├─ Does it need user interaction?
    │   └─ YES → [Main]
    │
    ├─ Is it a commit operation?
    │   └─ YES → [Skill:commit]
    │
    ├─ Is it code search / architecture understanding?
    │   └─ YES → [Sub:Explore]
    │
    ├─ Is it large file analysis (>8000 tokens)?
    │   └─ YES → [CSA:auto]
    │
    ├─ Is it code review?
    │   └─ YES → [CSA:review]
    │
    ├─ Is it an architectural decision needing adversarial review?
    │   └─ YES → [CSA:debate]
    │
    ├─ Is it well-defined implementation task?
    │   └─ YES → [Sub:developer]
    │
    └─ Default → [CSA:auto] (safe fallback)
```

#### TaskCreate Template

```python
TaskCreate(
    subject="[EXECUTOR] Action description (imperative)",
    description="""
Implement [what] in [where].

Context:
- [relevant context from plan]
- [dependencies or prerequisites]

DONE WHEN: [mechanically verifiable condition]
- Example: targeted tests pass (e.g., `cargo test module::test`, `npm test -- module`, `pytest module/`)
- Example: `git log --oneline -1` contains `feat(scope):`
- Example: `git status` shows clean working tree
""",
    activeForm="[Present continuous: Implementing X]"
)
```

#### Standard Task Sequence

For each implementation unit, create this task sequence:

```python
# 1. Implementation task
TaskCreate(
    subject="[Sub:developer] Implement [feature]",
    description="... DONE WHEN: targeted tests pass",
    activeForm="Implementing [feature]"
)

# 2. Commit task (auto-injected after every implementation)
TaskCreate(
    subject="[Skill:commit] Commit [feature]",
    description="Full workflow: format/lint/test/review/commit. DONE WHEN: git log -1 matches.",
    activeForm="Committing [feature]"
)

# 3. Compact task (after logical stage, not after every commit)
# Only add when transitioning between major stages
TaskCreate(
    subject="[Main] Compact context after [stage name]",
    description="/compact Keep [decisions from stage]. Summarize [process details].",
    activeForm="Compacting context"
)
```

### Phase 5: APPROVE (No Templates Needed)

Phase 5 is procedural: use `AskUserQuestion` to present the refined plan, task list, and debate insights to the user. No CSA prompts or specialized templates required — the plan file itself serves as the presentation artifact.

### Phase 6: EXECUTE (No Templates Needed)

Phase 6 follows the task-based execution protocol. Refer to Task tools documentation for the complete execution strategy, commit workflow, and context management rules. No additional prompt templates are needed beyond the TaskCreate patterns shown in Phase 4 above.
