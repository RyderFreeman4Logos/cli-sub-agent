---
name: mktd
description: >-
  Generate structured TODO plans with CSA-powered reconnaissance and adversarial debate.
  Uses `csa todo` for git-tracked plan lifecycle (create/save/find/show).
  Triggers on: plan, mktd, design, architecture, implement feature, refactor,
  when non-trivial multi-step task (>3 steps, multi-file, architectural decision).
allowed-tools: TaskCreate, TaskUpdate, TaskList, TaskGet, Read, Grep, Glob, Bash, Write, Edit, AskUserQuestion
---

# mktd: Make TODO — Debate-Enhanced Planning

## Purpose

Create git-tracked TODO plans via `csa todo` using:

- **Zero main-agent file reads during exploration** — CSA sub-agents gather context
- **Mandatory adversarial review** — heterogeneous model debate catches blind spots
- **Checklist format** — `[ ]` items with executor tags for clear execution planning
- **Pre-assigned execution** — every TODO item has an executor tag before work begins
- **Full audit trail** — debate transcripts + structured TODOs with decision rationale

Each TODO item is a `[ ]` checkbox with an executor tag indicating who should execute it. Descriptions are written in the user's preferred language (as configured in CLAUDE.md or inferred from conversation).

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

**CRITICAL**: In Phase 1, main agent MUST NOT use Read, Glob, Grep, or Bash to explore codebase files. The ONLY permitted Bash usage in Phase 1 is invoking CSA commands (`csa run`, `csa debate`). CSA sub-agents have direct file system access and read files natively.

**Note**: Read/Write/Edit are allowed in later phases (e.g., Phase 2 for writing `todo.md`, Phase 4 for presenting to user). The restriction applies **only to Phase 1 codebase exploration**.

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

### Check for Existing Plan

Before creating a new plan, check if one already exists for the current branch:

```bash
# Check for existing plan on current branch
csa todo find --branch "$(git branch --show-current)"
```

If a matching plan exists, resume editing it instead of creating a new one.

### Create Plan via `csa todo`

```bash
# Create a new plan (returns timestamp to stdout, path to stderr)
TIMESTAMP=$(csa todo create "Feature/Task Name" --branch "$(git branch --show-current)" 2>/dev/null)
TODO_PATH=$(csa todo show -t "$TIMESTAMP" --path)

# The plan is auto-committed with initial template.
# Now overwrite TODO.md with the full draft content using Write tool.
```

After `csa todo create`, use the `Write` tool to overwrite the TODO.md at `$TODO_PATH` with the full plan content, then save:

```bash
# After writing full content via Write tool:
csa todo save -t "$TIMESTAMP" "draft: initial plan"
```

### TODO File Format

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

### Language Detection (MANDATORY — OVERRIDES EXAMPLE LANGUAGE)

**CRITICAL: The TODO plan MUST be written in the user's preferred language, NOT in the language of the example below.** The example section uses Chinese to demonstrate the format, but your actual plan must match the detected user language. If your CLAUDE.md says `用中文与用户交流`, the plan MUST be in Chinese regardless of what language any example or template uses.

Detect language using this priority chain:

1. **CLAUDE.md directive** — Check project or global CLAUDE.md for explicit language instructions (e.g., a Chinese communication directive)
2. **Conversation language** — If the user has been writing in a specific language throughout the conversation, use that language
3. **Explicit request** — If the user explicitly requests a language, use it
4. **Default** — English (only if no signal detected)

**What to write in the detected language:**
- TODO title and Goal section
- Context Brief descriptions (file paths stay as-is)
- Key Decisions explanations
- Task descriptions (executor tags and commit messages stay in English per Conventional Commits)
- Risks & Mitigations descriptions
- Debate Insights summaries

**What ALWAYS stays in English:**
- Executor tags: `[Main]`, `[Sub:developer]`, `[Skill:commit]`, etc.
- Commit message suggestions: `fix(session): ...`
- File paths and code references
- Section headers (Markdown `##` headings) — keep in English for tooling compatibility

**Example (non-English user):**
```markdown
- [ ] `[Sub:developer]` <task description in user's language> review_cmd.rs:78
- [ ] `[Skill:commit]` Commit — `fix(session): dynamic description for review/debate sessions`
```

Note: Task descriptions use the detected language, but executor tags and commit messages stay in English.

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

Be adversarial. Challenge every assumption. Identify what will break.

IMPORTANT: Do NOT include secrets, credentials, API keys, or .env contents
in the plan text. Only include task descriptions, architectural decisions,
and risk summaries — not source code snippets containing sensitive data."
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

Append to the TODO file using Edit tool, then save the revision:

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

```bash
# Save post-debate revision
csa todo save -t "$TIMESTAMP" "post-debate revision"

# Update plan status to debating (tracks lifecycle)
csa todo status "$TIMESTAMP" debating
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

- **Approve, 使用 mktsk** → Update status and proceed with `mktsk` skill to execute:
  ```bash
  csa todo status "$TIMESTAMP" approved
  csa todo save -t "$TIMESTAMP" "approved by user"
  ```
- **Modify** → Adjust TODO per feedback, save, re-present:
  ```bash
  csa todo save -t "$TIMESTAMP" "revised per user feedback"
  ```
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

## Complete Example: Adding JWT Authentication (Chinese-speaking user)

This example demonstrates a Chinese-speaking user (detected via CLAUDE.md `用中文与用户交流` directive). Note how task descriptions are in Chinese while executor tags, commit messages, file paths, and section headers remain in English.

```bash
# Phase 1: RECON — 3 parallel CSA tasks
csa run "Analyze auth-related code structure in this project..."
csa run "Find existing auth patterns, middleware, token handling..."
csa run "Identify security constraints, dependency requirements..."

# Phase 2: DRAFT — Create plan via csa todo
TIMESTAMP=$(csa todo create "JWT 身份认证" --branch "$(git branch --show-current)" 2>/dev/null)
TODO_PATH=$(csa todo show -t "$TIMESTAMP" --path)
# Write full plan content to $TODO_PATH using Write tool
csa todo save -t "$TIMESTAMP" "draft: initial plan"

# Phase 3: DEBATE — Mandatory adversarial review
csa debate "Review this JWT auth plan critically: [TODO]..."
csa todo save -t "$TIMESTAMP" "post-debate revision"
csa todo status "$TIMESTAMP" debating

# Phase 4: APPROVE — AskUserQuestion with TODO + debate insights
csa todo status "$TIMESTAMP" approved
csa todo save -t "$TIMESTAMP" "approved by user"
```

Example TODO file after Phase 3:

```markdown
# TODO: JWT 身份认证

## Goal

为 API 端点实现基于 JWT 的身份认证，包括 token 验证、登录端点和安全的 token 签发。

## Context Brief

**相关文件**:
- `src/auth/` — 现有认证模块，含中间件
- `src/api/auth.rs` — 认证 API 端点
- `src/middleware/auth.rs` — 请求认证中间件

**现有模式**:
- 错误处理使用 `thiserror` 定义域错误
- 中间件模式：`tower::Service` 实现
- 测试使用 `rstest` 做参数化用例

**关键约束**:
- 必须保持与现有 session-based 认证的向后兼容
- 登录端点需要限流
- 需要 token 过期和刷新机制

## Key Decisions

- Decision 1: 使用 `jsonwebtoken` crate 而非自行实现 — 久经考验，广泛使用，支持 ECDSA
- Decision 2: JWT 密钥存储在环境变量中，不放配置文件 — 安全最佳实践
- Decision 3: 15 分钟 access token + 7 天 refresh token — 平衡安全性与用户体验

## Tasks

- [ ] `[Sub:Explore]` 查找所有认证入口点和中间件使用模式
- [ ] `[Sub:developer]` 在 `src/auth/jwt.rs` 中实现 JWT token 验证逻辑
- [ ] `[Skill:commit]` Commit JWT validation — `feat(auth): add JWT token validation with ECDSA`
- [ ] `[Sub:developer]` 在 `src/api/auth.rs` 中添加登录端点，签发 JWT
- [ ] `[Sub:developer]` 为登录端点添加限流中间件
- [ ] `[Skill:commit]` Commit login endpoint — `feat(auth): add login endpoint with rate limiting`
- [ ] `[Sub:developer]` 在 `src/api/auth.rs` 中实现 token 刷新端点
- [ ] `[Skill:commit]` Commit token refresh — `feat(auth): add token refresh endpoint`
- [ ] `[CSA:review]` 审查所有认证变更的安全性
- [ ] `[Main]` 验证 git status 干净且所有测试通过

## Risks & Mitigations

- 风险 1: Token 密钥泄露 → 缓解: 仅使用环境变量，绝不写日志，.env 加入 .gitignore
- 风险 2: 暴力破解登录 → 缓解: 限流（每 IP 每分钟 10 次请求）
- 风险 3: Token 重放攻击 → 缓解: 短过期时间 + refresh token 轮转
- 风险 4: 刷新端点的 CSRF → 缓解: SameSite cookie 属性 + CSRF token

## Debate Insights

**Session**: `01JKX7R2M3N4P5Q6R7S8T9U0V1`
**Rounds**: 2
**Debate tool**: codex (gpt-5.3-codex)

### Findings That Changed the Plan
- 缺失: token 刷新机制（已添加刷新端点任务）
- 缺失: 登录限流（已添加限流任务）
- 新识别风险: 刷新端点的 CSRF（已添加缓解策略）

### Considered and Rejected Alternatives
- OAuth2 库：拒绝，对当前需求过度工程化

### Resolved Tensions
- Access token 过期时间（5 分钟 vs 15 分钟）：选择 15 分钟 + refresh token 以改善用户体验
- 存储位置（Redis vs 内存）：先从简单方案（内存）开始，后续需要时再扩展

### Remaining Uncertainties
- Token 撤销策略：作为后续增强接受，MVP 阶段低优先级
```

---

## Done Criteria

| Phase | Verification |
|-------|-------------|
| Phase 1 (RECON) | 3 CSA summaries received, zero main-agent file reads |
| Phase 2 (DRAFT) | `csa todo show -t $TIMESTAMP` returns full plan content |
| Phase 3 (DEBATE) | TODO file contains `## Debate Insights` with session ID; `csa todo history -t $TIMESTAMP` shows "post-debate revision" |
| Phase 4 (APPROVE) | User explicitly approved; `csa todo list --status approved` includes plan |

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

#### Plan Discovery

```bash
# Check if plan already exists for current branch
csa todo find --branch "$(git branch --show-current)"
# If found, resume with existing timestamp instead of creating new
```

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

After appending to TODO.md via Edit tool:

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

```bash
# Save debate revision to git
csa todo save -t "$TIMESTAMP" "post-debate revision"
csa todo status "$TIMESTAMP" debating
```

### Phase 4: APPROVE (No Templates Needed)

Phase 4 is procedural: use `AskUserQuestion` to present the refined TODO, task list, and debate insights to the user. No CSA prompts or specialized templates required — the TODO file itself serves as the presentation artifact.
