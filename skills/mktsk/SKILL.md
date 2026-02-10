---
name: mktsk
description: >-
  Convert TODO plans into Task tool entries (TaskCreate/TaskUpdate/TaskList/TaskGet)
  for persistent execution across auto-compaction. Ensures every task has an
  executor tag, DONE WHEN condition, and commit step. Enforces strict serial
  development with continuous execution until completion.
allowed-tools: TaskCreate, TaskUpdate, TaskList, TaskGet, Read, Grep, Glob, Bash, Write, Edit, AskUserQuestion
---

# mktsk: Make Task — Plan-to-Execution Bridge

## Purpose

After determining tasks in context (from todo.md, plan files, or discovered issues/bugs), **enforce use of Task tools** to create detailed execution plans that survive auto-compaction.

**Core value**:
- ✅ Tasks persist across auto-compact (Task tools stored in Claude Code process)
- ✅ Explicitly specify executor for each step (main/sub-agent/CSA)
- ✅ Enforce commit discipline and CSA review workflow
- ✅ Delay context window bloat, prevent context pollution
- ✅ Continuous execution — create tasks then run immediately without waiting

## When to Use

**Immediately use this skill when**:
- ✅ Discussed and determined a todo.md or plan file
- ✅ Discovered and confirmed issues/vulnerabilities/bugs
- ✅ About to start multi-step task (>3 steps)
- ✅ Context window usage > 60%
- ✅ Task spans multiple `/compact` cycles

**Do NOT use this skill when**:
- ❌ Single-step simple task (< 3 steps)
- ❌ Pure information query, no execution needed
- ❌ Temporary exploration with no clear goal

## MANDATORY: Serial Development Only (NEVER Parallel)

**All development tasks MUST execute strictly in serial. Parallel development is absolutely forbidden.**

### Why Serial Only

| Problem | Consequence of Parallel |
|---------|------------------------|
| **Pre-commit hooks** | Agent-A passes, but Agent-B's half-finished files break lint/test |
| **Atomic commits** | Two agents' changes mix together, preventing clean commits |
| **Agent conflicts** | Agent-A's tests fail due to Agent-B's intermediate state, agents "fix" each other's "bugs" |
| **Merge conflicts** | Token waste resolving conflicts vs. serial avoidance |

### FORBIDDEN Patterns

❌ **Parallel sub-agent development**:
```python
# WRONG - two development agents running simultaneously
TaskCreate(subject="[Sub:developer] Implement feature A", ...)  # Agent A
TaskCreate(subject="[Sub:developer] Implement feature B", ...)  # Agent B at same time!
```

❌ **Worktree parallel**:
```bash
# WRONG - do not use worktree
git worktree add ../feature-branch
```

### MANDATORY Pattern

✅ **Strict serial: Implement → Commit → Next**:
```
Implement A → Review → Commit A
                              ↓
            Implement B → Review → Commit B
                                        ↓
                          Implement C → Review → Commit C
```

### Parallel Exception

Only **read-only/analysis** tasks may run in parallel:
- ✅ Parallel search (Explore agents) — no file modification
- ✅ Parallel analysis (CSA analysis) — no file modification
- ❌ Any task that **writes files** — MUST be serial

## Task Tool API (MANDATORY)

Claude Code uses the Task tool suite:

| Tool | Purpose |
|------|---------|
| `TaskCreate` | Create new task (subject, description, activeForm) |
| `TaskUpdate` | Update task status/content (taskId, status, etc.) |
| `TaskList` | List all tasks |
| `TaskGet` | Get specific task details (taskId) |

### 1. Create Task - TaskCreate

**Required fields**:
- `subject`: Task title (including executor tag)
- `description`: Detailed description

**Optional fields**:
- `activeForm`: In-progress state description (present continuous)

**Subject format**: `[Executor] Action description`

Example:
| Executor | When to Use | Example |
|----------|-------------|---------|
| `[Main]` | Already in context, simple operation, needs user interaction | `[Main] Review user requirements and ask clarifications` |
| `[Skill:commit]` | Execute commit workflow (format/lint/test/review/commit) | `[Skill:commit] Commit JWT validation changes` |
| `[Sub:developer]` | Well-defined implementation task | `[Sub:developer] Implement JWT validation logic` |
| `[Sub:Explore]` | Large-scale code search, architecture understanding | `[Sub:Explore] Find all authentication-related files` |
| `[CSA:review]` | Code review, issue detection | `[CSA:review] Review uncommitted changes` |
| `[CSA:auto]` | Large file analysis, documentation research | `[CSA:auto] Analyze entire codebase` |
| `[CSA:codex]` | Independent feature implementation, needs sandbox | `[CSA:codex] Implement auth in sandbox` |
| `[CSA:debate]` | Architecture design, brainstorming | `[CSA:debate] Design authentication architecture` |

### 2. Update Task Status - TaskUpdate

**Status values**:
- `"pending"` - Not started
- `"in_progress"` - Currently executing (**only ONE at a time**)
- `"completed"` - Finished

**Parameters**:
- `taskId`: Task ID (required)
- `status`: New status
- `subject`: Update title
- `description`: Update description
- `activeForm`: Update in-progress state
- `addBlocks`: Set tasks this task blocks
- `addBlockedBy`: Set tasks that block this task

### 3. ActiveForm Field

Present continuous form, describes in-progress state:
- Subject: `"Run tests"` → ActiveForm: `"Running tests"`
- Subject: `"Fix bug in parser"` → ActiveForm: `"Fixing bug in parser"`

## DONE WHEN Conditions (MANDATORY)

Every task MUST include a mechanically verifiable DONE WHEN condition in its description.

**Why MANDATORY**:
- ✅ Prevents premature task completion
- ✅ Provides clear success criteria
- ✅ Enables automated verification
- ✅ No ambiguity about when task is done

**Examples**:

| Task Type | DONE WHEN Condition |
|-----------|---------------------|
| Run tests | `DONE WHEN: targeted tests pass (exit code 0)` |
| Commit changes | `DONE WHEN: git log -1 shows commit with message "feat(auth):"` |
| Clean working tree | `DONE WHEN: git status shows "nothing to commit, working tree clean"` |
| File creation | `DONE WHEN: src/auth/jwt.rs exists with validate_token() function` |
| Format code | `DONE WHEN: formatter returns exit code 0` |
| Sub-agent task | `DONE WHEN: sub-agent returns structured report` |
| Fix errors | `DONE WHEN: linter/compiler output is empty` |

**Template**:
```python
TaskCreate(
    subject="[Executor] Task description",
    description="""
    Detailed task description here.

    DONE WHEN: [mechanically verifiable condition]
    """,
    activeForm="Doing the task"
)
```

**Verification Protocol**:
1. Before marking task as completed, run verification command
2. Check output matches DONE WHEN condition
3. If condition not met, continue working on task
4. Only call `TaskUpdate(status="completed")` when condition passes

## CSA Delegation

For tasks requiring massive context (whole codebase analysis, large docs), use CSA sub-agents.

### CSA Tool Modes

CSA supports three `--tool` modes:
- `auto` (default): CSA selects the best available tool automatically
- `any-available`: Round-robin across available tools (for low-risk tasks)
- `<tool-name>`: Explicitly select a specific tool (e.g., `codex`, `opencode`, `gemini-cli`, `claude-code`)

### When to Use CSA

| Task | CSA Command | Why |
|------|-------------|-----|
| Analyze 10+ files | `csa run` | Large context window |
| Architecture design | `csa debate` | Multi-perspective reasoning |
| Review git diff | `csa review` | Specialized code review |
| Implement in sandbox | `csa run --tool codex` | Safe execution environment |

### CRITICAL: Never Pre-fetch for CSA

**FORBIDDEN** (wastes Claude tokens):
```
Read src/index.ts → pass content to csa run
Bash "git diff" → pass output to csa run
Grep "TODO" → pass results to csa run
```

**CORRECT** (zero Claude token waste):
```
csa run "analyze src/index.ts"
csa run "Run git diff and analyze changes"
csa run "find all TODO comments in src/**/*.ts"
```

**Why**: CSA tools have direct file system access. Pre-fetching wastes ~50,000 Claude tokens.

## Mandatory Commit/Review Workflow (STRICT)

### ⚠️ Commit Discipline (Use commit skill)

**All commit operations MUST use the `commit` skill**, which enforces:
- ✅ Conventional Commits format (English)
- ✅ Pre-commit review (csa review)
- ✅ Quality gates (formatters, linters, tests)
- ✅ Security checks (no secrets, no debug code)
- ✅ Atomic commits (one logical unit per commit)

### Standard Commit Workflow

**After completing each logical unit, MUST commit immediately.**

```
1. [Sub/CSA] Implement logical unit (e.g., JWT validation)
   ↓
2. [Main] Run formatters (as defined in your CLAUDE.md)
   ↓
3. [Main] Run linters
   ↓
4. [Main] Run tests (full suite or targeted)
   ↓
5. [CSA:review] Review uncommitted changes
   ↓
6. Issues found?
   ├─ YES → [CSA:codex] Fix issues (same session)
   │         ↓
   │      [CSA:review] Re-review (--session <ID>)
   │         ↓
   │      Loop until no issues
   │
   └─ NO → [Main] Commit with proper message (from csa review output)
```

### Task Tool Representation (Complete Workflow)

```python
# Use TaskCreate to create tasks
TaskCreate(
    subject="[Sub:developer] Implement JWT validation logic",
    description="Implement JWT token validation in src/auth/jwt.rs. DONE WHEN: targeted tests pass.",
    activeForm="Implementing JWT validation logic"
)

TaskCreate(
    subject="[Skill:commit] Commit JWT validation changes",
    description="Run commit workflow: format/lint/test/review/commit. DONE WHEN: git log -1 shows feat(auth): message.",
    activeForm="Committing JWT validation (format/lint/test/review/commit)"
)

# When executing, update status (TaskCreate returns taskId)
TaskUpdate(taskId=task1.taskId, status="in_progress")
# ... complete task ...
TaskUpdate(taskId=task1.taskId, status="completed")
```

**Note**: `[Skill:commit]` automatically executes:
1. Run formatters (project-specific)
2. Run linters (project-specific)
3. Run tests (full or targeted)
4. Security scan
5. Pre-commit review (csa review --diff)
6. Fix issues if any (codex in same session)
7. Re-review until clean
8. Generate commit message (from csa review)
9. Commit with proper Conventional Commits format

### Why codex (not Main) Fixes Issues

**Question**: After csa review finds issues, who fixes them?

**Answer**: `[CSA:codex]` in the same session

**Rationale**:
- ✅ csa review already provides full context (code, issues, suggestions)
- ✅ Reuse session to avoid re-transmitting context (save tokens)
- ✅ codex has sandbox protection, can safely execute fixes
- ✅ Main agent stays clean, no context pollution

**Session reuse example**:
```python
# Step 1: csa review
csa review --diff
# Returns: session ID in output

# Step 2: codex fixes in same session
csa run --tool codex --session <session-id> "Fix the issues you identified"

# Step 3: csa review again
csa review --diff
```

## Forbidden Operations (MUST NOT DO)

### 1. Operations Main Agent Must NOT Handle

| Operation | MUST Use Instead | Why |
|-----------|------------------|-----|
| Read git diff (>100 lines) | `[CSA:review]` | Save Claude tokens |
| Read >3 files for analysis | `[CSA:auto]` | Avoid context bloat |
| Generate commit message | Use commit skill | Encapsulated workflow |
| Large-scale code search | `[Sub:Explore]` | Specialized tool |
| Review code changes | `[CSA:review]` | Specialized tool |

### 2. Forbidden: Pre-fetching Data for CSA

**Wrong approach** (❌):
```
Read src/index.ts → pass content to csa run
Bash "git diff" → pass output to csa run
Grep "TODO" → pass results to csa run
```

**Correct approach** (✅):
```
csa run "analyze src/index.ts"
csa run "Run git diff and analyze changes"
csa run "find all TODO comments in src/**/*.ts"
```

**Why**: CSA backend tools have direct file system access, pre-fetching wastes 50,000+ Claude tokens

### 3. Forbidden: Not Using commit Skill

**Every commit MUST use**:
```
[Skill:commit] Commit <change description>
```

**commit skill automatically executes**:
- ✅ Formatters (project-specific)
- ✅ Linters (project-specific)
- ✅ Tests (full or targeted)
- ✅ Security scan
- ✅ Pre-commit review (csa review)
- ✅ Fix issues if any
- ✅ Generate commit message
- ✅ Commit with Conventional Commits format

**NOT allowed**:
- ❌ "Manually execute commit workflow"
- ❌ "Skip commit skill and commit directly"
- ❌ "Code is simple, doesn't need skill"

### 4. Forbidden: Accumulating Changes

**Wrong** (❌):
```
Implement feature A
Implement feature B
Implement feature C
Commit all together
```

**Correct** (✅):
```
Implement feature A → Review → Fix → Commit
Implement feature B → Review → Fix → Commit
Implement feature C → Review → Fix → Commit
```

## Context Window Management

### Rule: Proactive /compact Execution

**When MUST execute `/compact`**:

| Trigger | Action | Reason |
|---------|--------|--------|
| Logical stage complete | `/compact Keep [decisions]. Summarize [process].` | Preserve key decisions, clean process details |
| Context > 80% | `/compact` immediately | Prevent auto-compact at critical moments |
| Read 5+ files (won't reference again) | `/compact` after gathering info | Clean up information gathering process |
| Switch to completely different task | `/compact` before switching | Separate different topic contexts |

**When NOT to execute**:
- ❌ In the middle of implementing a feature
- ❌ During debugging (need error context)
- ❌ Just read files that will be modified next

**Task tools persist across `/compact`** — this is the core value of using Task tools!

## Delegation Decision Protocol

When writing Task plan, **MUST** follow priority protocol:

### Priority 0: File Size Check (HIGHEST PRIORITY)

**BEFORE reading any file for a task, MUST check token count first.**

```bash
# Check file size BEFORE reading
tokuin estimate --model gpt-4 --format json <file> | jq '.tokens'

# If >8000 tokens → MUST delegate to CSA or sub-agent
# If <8000 tokens → May continue with other priority checks
```

**Fallback** (if `tokuin` unavailable):
```bash
wc -l <file>  # >1000 lines → likely >8000 tokens
ls -lh <file>  # >50KB → likely >8000 tokens
```

**Why CRITICAL**: Reading large files causes auto-compact death loops:
- Read large file → auto compact → task needs file again → re-read → auto compact → infinite loop

### Priority 1: Information Location

```
Check: Information already in context?
✅ YES → [Main] handles it
❌ NO  → Continue to Priority 2
```

**Exception**: Even if in context, if file >8000 tokens, delegate to prevent future auto-compact loops.

### Priority 2: Interaction Requirements

```
Check: May need to ask user questions?
✅ YES → [Main] handles it
❌ NO  → Continue to Priority 3
```

### Priority 3: Task Scale

```
Check: Simple task (<5 lines of code)?
✅ YES → [Main] handles it
❌ NO  → Delegate to [Sub] or [CSA]
```

### Priority 4: Output Size Check

**Before reading dynamic output, MUST check size first**:

```bash
# Git diff
git diff --stat  # Check size
# If >500 lines → [CSA:auto]

# File
wc -l file  # Check line count
# If >1000 lines → [CSA:auto]

# Command output
command | head -50  # Preview first 50 lines
```

## Complete Example: Feature Implementation

```python
# Step 1: Create all tasks (create once)
# Note: This only creates tasks, does not execute

task1 = TaskCreate(
    subject="[Sub:Explore] Find all authentication-related files",
    description="Search pattern: auth, jwt, token. DONE WHEN: sub-agent returns structured report.",
    activeForm="Finding authentication files"
)

task2 = TaskCreate(
    subject="[CSA:auto] Analyze current auth architecture",
    description="Analyze all files in auth module. DONE WHEN: CSA returns architecture summary.",
    activeForm="Analyzing authentication architecture"
)

task3 = TaskCreate(
    subject="[Sub:developer] Implement JWT token validation logic",
    description="Implement in src/auth/jwt.rs. DONE WHEN: targeted tests pass.",
    activeForm="Implementing JWT validation logic"
)

task4 = TaskCreate(
    subject="[Skill:commit] Commit JWT validation changes",
    description="Full commit workflow. DONE WHEN: git log -1 shows feat(auth): message.",
    activeForm="Committing JWT validation (format/lint/test/review/commit)"
)

task5 = TaskCreate(
    subject="[Sub:developer] Add login endpoint to API",
    description="Implement in src/api/auth.rs. DONE WHEN: targeted tests pass.",
    activeForm="Adding login endpoint"
)

task6 = TaskCreate(
    subject="[Skill:commit] Commit login endpoint changes",
    description="Full commit workflow. DONE WHEN: git log -1 shows feat(auth): message.",
    activeForm="Committing login endpoint (format/lint/test/review/commit)"
)

task7 = TaskCreate(
    subject="[Main] Check git status for unstaged files",
    description="Verify no files left unstaged. DONE WHEN: git status shows clean working tree.",
    activeForm="Checking git status"
)

task8 = TaskCreate(
    subject="[Main] Handle all unstaged files",
    description="Commit, .gitignore, or explain. DONE WHEN: git status clean or all files justified.",
    activeForm="Handling unstaged files"
)

# Step 2: After plan approval, start execution immediately
# From first task to last, execute continuously

# Execute task 1
TaskUpdate(taskId=task1.taskId, status="in_progress")
# ... perform exploration ...
TaskUpdate(taskId=task1.taskId, status="completed")

# Execute task 2 (don't stop, continue)
TaskUpdate(taskId=task2.taskId, status="in_progress")
# ... perform analysis ...
TaskUpdate(taskId=task2.taskId, status="completed")

# Execute task 3 (don't stop, continue)
TaskUpdate(taskId=task3.taskId, status="in_progress")
# ... perform implementation ...
TaskUpdate(taskId=task3.taskId, status="completed")

# Execute task 4 (don't stop, continue)
TaskUpdate(taskId=task4.taskId, status="in_progress")
# ... execute commit ...
TaskUpdate(taskId=task4.taskId, status="completed")

# Execute task 5 (don't stop, continue)
TaskUpdate(taskId=task5.taskId, status="in_progress")
# ... perform implementation ...
TaskUpdate(taskId=task5.taskId, status="completed")

# Execute task 6 (don't stop, continue)
TaskUpdate(taskId=task6.taskId, status="in_progress")
# ... execute commit ...
TaskUpdate(taskId=task6.taskId, status="completed")

# Execute task 7 (don't stop, continue)
TaskUpdate(taskId=task7.taskId, status="in_progress")
# ... execute check ...
TaskUpdate(taskId=task7.taskId, status="completed")

# Execute task 8 (don't stop, continue)
TaskUpdate(taskId=task8.taskId, status="in_progress")
# ... handle files ...
TaskUpdate(taskId=task8.taskId, status="completed")

# All tasks complete, report to user
# "All tasks completed. Implemented JWT validation and login endpoint, both reviewed and committed."
```

## Execution Strategy: Create and Run (STRICT)

**After creating tasks from an approved plan (mktd Phase 4 approval or explicit user instruction), MUST immediately start execution and continue until all tasks complete.**

### Prerequisite: Approval Required

Execution ONLY begins when ONE of these conditions is met:
- User explicitly approved via mktd Phase 4 (APPROVE)
- User explicitly says "start", "execute", "go", or equivalent
- User invokes `/mktsk` with a plan file they authored themselves

**NEVER auto-execute** from a plan that hasn't been approved.

### Core Rules

| Rule | Requirement |
|------|-------------|
| **Immediate start** | After creating tasks from an approved plan, start first task immediately — no further confirmation |
| **Continuous execution** | After completing one task, immediately execute next, do not pause between tasks |
| **Stop only when done** | Only stop when all tasks marked completed, OR encounter unresolvable blocker requiring user input |
| **Serial writes** | NEVER run parallel development sub-agents |
| **Atomic commits** | `[Skill:commit]` after each logical unit |
| **Proactive compact** | `/compact` when context > 80% or after stage completion |
| **Task persistence** | Task tools survive auto-compact — check TaskList after compact |
| **Single in_progress** | Only ONE task may be `in_progress` at any time |

### When to Pause (Wait for User)

✅ **MUST pause when**:
- Requirements unclear, need user clarification
- Multiple viable approaches, need user choice
- Critical issue discovered that changes scope
- Needs credentials or access permissions

❌ **MUST NOT pause when**:
- Normal task execution flow
- Encountering auto-fixable errors (lint, format)
- Need to move to next task
- Delegatable operations

### When Pausing

When you must pause:
1. Use `TaskUpdate` to set current task status to `pending` (not in_progress)
2. Clearly tell the user:
   - Which task you're on
   - What problem was encountered
   - What decision or information is needed
   - Suggested options (if any)
3. Wait for user reply, then continue

## Workflow Summary

```
1. User provides task/plan/issue
   ↓
2. [MANDATORY] Use Task tools to create detailed plan
   - Assign executor for each step ([Main]/[Sub:...]/[CSA:...])
   - Include DONE WHEN condition for each task
   - Include verification steps (tests, checks)
   - Include pre-commit review steps (csa review)
   - Include commit steps (after each logical unit)
   - Include /compact steps (after stages)
   ↓
3. [GATE] Execution requires approval (mktd Phase 4, user says "go", or user-authored plan)
   ↓
4. Execute plan step-by-step (continuous execution, no stopping)
   - Use TaskUpdate to mark ONE task as "in_progress"
   - Complete task
   - Verify DONE WHEN condition
   - Mark as "completed" IMMEDIATELY with TaskUpdate
   - Move to next task immediately
   - Unless encountering decision blocker, do not stop between tasks
   ↓
5. After each logical unit:
   - [Skill:commit] Execute complete commit workflow
     (formatters → linters → tests → review → fix → commit)
   ↓
6. When stage completes or context > 80%:
   - [Main] Execute /compact with custom instructions
   - Task plan persists across compaction
   - Continue executing next task, do not stop
   ↓
7. After all tasks complete:
   - [Main] git status check
   - [Main] Handle all unstaged files
   - [Main] Verify all tasks "completed" with TaskList
   - Report completion to user
```

## Anti-Patterns (FORBIDDEN)

- ❌ Starting multi-step tasks without Task tools
- ❌ Task items without executor tags
- ❌ Task items without DONE WHEN conditions
- ❌ **Creating tasks from approved plan but not executing (MUST start after approval)**
- ❌ **Pausing between tasks without a decision blocker (MUST continue)**
- ❌ **Stopping before all tasks complete (MUST execute continuously)**
- ❌ Committing without `[Skill:commit]` (should use commit skill)
- ❌ Manually executing commit workflow without skill (violates encapsulation)
- ❌ Accumulating multiple logical units before commit
- ❌ Pre-fetching data for CSA (should let it read with direct access)
- ❌ Completing tasks but leaving unstaged files
- ❌ Spanning multiple `/compact` cycles without Task tools
- ❌ Context > 80% still not executing `/compact`
- ❌ Having multiple tasks `"in_progress"` simultaneously (MUST be exactly one)
- ❌ Launching multiple development sub-agents in parallel (MUST be strictly serial)
- ❌ Using git worktree for parallel development (storage limited, merge conflicts waste tokens)
- ❌ Starting next development task without committing current changes
- ❌ Marking task completed without verifying DONE WHEN condition

## ROI (Return on Investment)

| Benefit | Impact |
|---------|--------|
| **Prevent task loss** | 100% task continuity — auto compact never loses the plan |
| **Token savings** | 50-90% reduction via proper delegation |
| **Code quality** | Mandatory CSA review prevents issues |
| **Git hygiene** | Granular commits, clean history |
| **Context efficiency** | Proactive `/compact`, delayed bloat |
| **Execution clarity** | Explicit executor, no ambiguity |
| **Verifiable completion** | DONE WHEN conditions ensure quality |

## Conclusion

**This skill is MANDATORY for any multi-step task.** It ensures:
- ✅ Tasks survive auto compaction (Task tools persist)
- ✅ Proper delegation to sub-agents/CSA (token efficiency)
- ✅ Strict commit and review discipline (code quality)
- ✅ Minimal context pollution (proactive /compact)
- ✅ Clear execution plan visible to user (transparency)
- ✅ **Continuous execution until completion (no unnecessary stops)**
- ✅ **Verifiable completion criteria (DONE WHEN conditions)**

**Always use Task tools. Always assign executor. Always include DONE WHEN. Always review before commit. Always /compact after stages. Always execute immediately and continuously.**
