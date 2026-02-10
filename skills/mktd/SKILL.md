---
name: mktd
description: >-
  Generate structured TODO plans with CSA-powered reconnaissance and adversarial debate.
  Outputs ./drafts/TODOs/{timestamp}/todo.md with ordered checklist items and executor tags.
  Triggers on: plan, mktd, design, architecture, implement feature, refactor,
  when non-trivial multi-step task (>3 steps, multi-file, architectural decision).
allowed-tools: TaskCreate, TaskUpdate, TaskList, TaskGet, Read, Grep, Glob, Bash, Write, Edit, AskUserQuestion
---

# mktd: Make TODO — Debate-Enhanced Planning

## Purpose

Create structured TODO files at `./drafts/TODOs/{timestamp}/todo.md` using:

- **Zero main-agent file reads during exploration** — CSA sub-agents gather context
- **Mandatory adversarial review** — heterogeneous model debate catches blind spots
- **Checklist format** — `[ ]` items with executor tags for clear execution planning
- **Pre-assigned execution** — every TODO item has an executor tag before work begins
- **Full audit trail** — debate transcripts + structured TODOs with decision rationale

Each TODO item is a `[ ]` checkbox with an executor tag indicating who should execute it. The user's native language is auto-detected and used for descriptions.

### Why Use This Skill

| Dimension | Without mktd | With mktd |
|-----------|-------------|-----------|
| Exploration context cost | Main agent reads files (high) | CSA reads files (zero) |
| Reasoning quality | Single model | Heterogeneous adversarial debate |
| Blind spot detection | None | Mandatory debate catches oversights |
| Execution planning | Ad-hoc decisions | Pre-assigned executor tags |
| Audit trail | Conversation only | Debate transcript + TODO file with decision history |
| Context efficiency | ~50,000+ tokens | ~5,000 tokens (Phase 1-4) |

## When to Use

**Use this skill when:**
- ✅ Non-trivial task (>3 steps or multi-file changes)
- ✅ Architectural or design decisions needed
- ✅ Multiple valid approaches exist
- ✅ Multi-module changes with cross-cutting concerns
- ✅ User explicitly requests planning or `/mktd`

**Do NOT use when:**
- ❌ Single-file trivial change (<5 lines)
- ❌ Typo fix, comment addition, one-line edit
- ❌ User gave very specific, detailed instructions (just execute)
- ❌ Pure research / exploration (use Explore agent instead)

## The 4 Phases

```
Phase 1: RECON ─── CSA parallel exploration (zero main-agent file reads)
    │
Phase 2: DRAFT ─── Main agent drafts TODO from CSA summaries
    │
Phase 3: DEBATE ── Mandatory adversarial review via csa debate
    │
Phase 4: APPROVE ── User reviews and approves
```

After approval, use the `mktsk` skill to convert TODOs into executable tasks.

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

## Phase 2: DRAFT (TODO Drafting)

**Goal**: Synthesize CSA reconnaissance into a structured TODO checklist.

### TODO File

Create the directory and write the draft TODO to `./drafts/TODOs/{timestamp}/todo.md` (use filesystem-safe timestamp format, e.g. `20260209T143000`). The `Write` tool creates parent directories automatically; if using Bash, run `mkdir -p` first:

```markdown
# TODO: [Feature/Task Name]

## Goal
[1-2 sentences describing the outcome]

## Context Brief
[Synthesized findings from Phase 1 RECON — key files, patterns, constraints]

## Key Decisions
- Decision 1: [choice] because [reasoning]
- Decision 2: [choice] because [reasoning]

## Tasks

- [ ] `[Sub:Explore]` Find all authentication entry points
- [ ] `[Sub:developer]` Implement JWT token validation in `src/auth/jwt.rs`
- [ ] `[Skill:commit]` Commit JWT validation — `feat(auth): add JWT validation`
- [ ] `[Sub:developer]` Add login endpoint to `src/api/auth.rs`
- [ ] `[Skill:commit]` Commit login endpoint — `feat(auth): add login endpoint`
- [ ] `[Main]` Verify git status clean

## Risks & Mitigations
- Risk 1: [description] → Mitigation: [approach]

## Debate Insights
(filled in after Phase 3)
```

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

### Guidelines

- Keep TODO concise (<2000 tokens)
- Reference specific file paths from RECON
- Identify key decisions explicitly (these will be debated)
- Note uncertainties honestly (debate will stress-test them)
- Every implementation task should be followed by `[Skill:commit]`

---

## Phase 3: DEBATE (Mandatory Adversarial Review)

**Goal**: Heterogeneous model debate catches blind spots, flawed assumptions, and missed alternatives.

**THIS PHASE IS ALWAYS MANDATORY. NO EXCEPTIONS.**

### Round 1: Initial Critique

```bash
csa debate "Review this implementation plan critically:

[paste draft TODO from Phase 2]

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
| **Valid concern, plan is wrong** | Revise TODO, debate again |
| **Valid concern, plan is incomplete** | Add missing tasks to TODO |
| **Minor suggestion** | Note in TODO, no revision needed |
| **False positive** | Counter-argue in Round 2 |

### Round 2+ (If Needed, Max 3 Total)

If TODO was revised or concerns need counter-argument:

```bash
csa debate --session <SESSION_ID> "I've revised the plan to address your concerns:

[revised sections or counter-arguments]

Remaining questions:
1. Does the revision adequately address [concern X]?
2. Any new concerns introduced by the revision?
3. Final recommendation: proceed, revise further, or abandon?"
```

### Record Debate Insights

Append to the TODO file:

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

## Phase 4: APPROVE (User Gate)

**Goal**: Present refined TODO to user for approval before execution.

### Present to User

Use AskUserQuestion to present:
1. **TODO summary** (from Phase 2, refined by Phase 3)
2. **Debate key insights** (what changed, what was caught)
3. **Task list** (with executor assignments)
4. **Estimated scope** (number of tasks, files touched)

### User Options

- **Approve** → User can proceed with `mktsk` skill to execute
- **Modify** → Adjust TODO per feedback, re-present
- **Reject** → Abandon plan, ask user for new direction

---

## Next Step: mktsk

After user approval, use the `mktsk` skill to convert this TODO file into Task tool entries (TaskCreate/TaskUpdate) for execution. The `mktsk` skill handles:

- Converting `[ ]` items to TaskCreate calls with executor tags
- Serial execution protocol
- Commit workflow per logical unit
- Context window management (/compact after stages)

**Reference**: See the `mktsk` skill documentation for the complete execution protocol.

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
- ❌ **Any phase**: Pre-fetch data for CSA

---

## Complete Example: Adding JWT Authentication

```python
# Phase 1: RECON — 3 parallel CSA tasks
csa run "Analyze auth-related code structure in this project..."
csa run "Find existing auth patterns, middleware, token handling..."
csa run "Identify security constraints, dependency requirements..."

# Phase 2: DRAFT — Main agent writes TODO from summaries
Write("./drafts/TODOs/20260209T143000/todo.md", todo_content)

# Phase 3: DEBATE — Mandatory adversarial review
csa debate "Review this JWT auth plan critically: [TODO]..."
# Debate catches: missing token refresh, CSRF risk, rate limiting gap
# Revise TODO, debate again:
csa debate --session 01JK... "Revised plan addresses: [changes]..."

# Phase 4: APPROVE — AskUserQuestion with TODO + debate insights

# After approval: User invokes mktsk skill to execute
```

Example TODO file after Phase 3:

```markdown
# TODO: Add JWT Authentication

## Goal
Implement JWT-based authentication for API endpoints with token validation, login endpoint, and secure token issuance.

## Context Brief

**Relevant files**:
- `src/auth/` — existing auth module with middleware
- `src/api/auth.rs` — auth API endpoints
- `src/middleware/auth.rs` — request authentication middleware

**Existing patterns**:
- Error handling uses `thiserror` for domain errors
- Middleware pattern: `tower::Service` implementations
- Tests use `rstest` for parameterized cases

**Critical constraints**:
- Must maintain backward compatibility with existing session-based auth
- Rate limiting required for login endpoint
- Token expiry and refresh mechanism needed

## Key Decisions

- Decision 1: Use `jsonwebtoken` crate over custom implementation — battle-tested, widely used, ECDSA support
- Decision 2: Store JWT secret in env var, not config file — security best practice
- Decision 3: 15-minute access token + 7-day refresh token — balance security vs UX

## Tasks

- [ ] `[Sub:Explore]` Find all authentication entry points and middleware usage patterns
- [ ] `[Sub:developer]` Implement JWT token validation logic in `src/auth/jwt.rs`
- [ ] `[Skill:commit]` Commit JWT validation — `feat(auth): add JWT token validation with ECDSA`
- [ ] `[Sub:developer]` Add login endpoint with JWT issuance to `src/api/auth.rs`
- [ ] `[Sub:developer]` Add rate limiting middleware for login endpoint
- [ ] `[Skill:commit]` Commit login endpoint — `feat(auth): add login endpoint with rate limiting`
- [ ] `[Sub:developer]` Implement token refresh endpoint in `src/api/auth.rs`
- [ ] `[Skill:commit]` Commit token refresh — `feat(auth): add token refresh endpoint`
- [ ] `[CSA:review]` Review all auth changes for security issues
- [ ] `[Main]` Verify git status clean and all tests passing

## Risks & Mitigations

- Risk 1: Token secret exposure → Mitigation: env var only, never logged, .env in .gitignore
- Risk 2: Brute force login attempts → Mitigation: rate limiting (10 req/min per IP)
- Risk 3: Token replay attacks → Mitigation: short expiry + refresh token rotation
- Risk 4: CSRF on token refresh → Mitigation: SameSite cookie attribute + CSRF token

## Debate Insights

**Session**: `01JKX7R2M3N4P5Q6R7S8T9U0V1`
**Rounds**: 2
**Key findings**:
- Missing: token refresh mechanism (added refresh endpoint task)
- Missing: rate limiting on login (added rate limiting task)
- Risk identified: CSRF on refresh endpoint (added mitigation strategy)
- Alternative considered: OAuth2 library → rejected because over-engineered for current needs

**Resolved tensions**:
- Access token expiry (5min vs 15min): resolved by 15min + refresh token for better UX
- Storage location (Redis vs in-memory): resolved by starting simple (in-memory), scale later if needed

**Remaining uncertainties**:
- Token revocation strategy: accepted as future enhancement, low priority for MVP
```

---

## Done Criteria

| Phase | Verification |
|-------|-------------|
| Phase 1 (RECON) | 3 CSA summaries received, zero main-agent file reads |
| Phase 2 (DRAFT) | TODO file exists at `./drafts/TODOs/{timestamp}/todo.md` |
| Phase 3 (DEBATE) | TODO file contains `## Debate Insights` with session ID |
| Phase 4 (APPROVE) | User explicitly approved |

After approval, execution is handled by the `mktsk` skill.

## ROI

| Benefit | Impact |
|---------|--------|
| Context efficiency | ~90% reduction (5K vs 50K+ tokens for planning) |
| Plan quality | Adversarial debate catches blind spots single-model misses |
| Execution clarity | Every task has pre-assigned executor — no ad-hoc decisions |
| Audit trail | Debate session IDs + TODO file = complete decision record |
| Flexibility | Checklist format easy to modify, re-order, or adapt |

---

## Appendix: Phase Templates

Detailed prompt templates for mktd Phases 1-3. Phase 4 is procedural (no prompt templates needed).

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

#### TODO File Structure

```markdown
# TODO: [Feature/Task Name]

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

## Tasks

- [ ] `[Sub:Explore]` Find [what] in [where]
- [ ] `[Sub:developer]` Implement [what] in `path/to/file`
- [ ] `[Skill:commit]` Commit [feature] — `type(scope): description`
- [ ] `[Sub:developer]` Add [what] to `path/to/file`
- [ ] `[Skill:commit]` Commit [feature] — `type(scope): description`
- [ ] `[Main]` Verify git status clean

## Risks & Mitigations
- **Risk**: [description] → **Mitigation**: [approach]
- **Risk**: [description] → **Mitigation**: [approach]

## Debate Insights
(filled in after Phase 3)
```

### Phase 3: DEBATE Templates

#### Template 3A: Initial Critique (Round 1)

```bash
csa debate "Review this implementation plan critically:

[PASTE DRAFT TODO HERE]

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

### Phase 4: APPROVE (No Templates Needed)

Phase 4 is procedural: use `AskUserQuestion` to present the refined TODO, task list, and debate insights to the user. No CSA prompts or specialized templates required — the TODO file itself serves as the presentation artifact.
