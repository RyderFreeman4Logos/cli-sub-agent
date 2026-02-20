# sa: Manager-Employee Orchestration (Three-Tier Model)

Three-tier recursive delegation for planning and implementation with strict role boundaries.

- Tier 0 is the **Department Manager**: dispatches work, reads structured reports, makes approval decisions.
- Tier 1/Tier 2 are **Employees**: execute work autonomously and return self-contained reports.

This skill exists to keep the main agent out of code-level work.

## Core Metaphor

```
┌──────────────────────────────────────────────────────────────┐
│ Tier 0: Department Manager (Main Agent)                     │
│                                                              │
│ Responsibilities:                                            │
│ • Define WHAT to do (objective, scope, done condition)       │
│ • Dispatch tasks via csa run                                 │
│ • Read result.toml (structured report only)                  │
│ • Decide approve/reject/escalate                             │
│ • Communicate summary to user                                │
│                                                              │
│ Forbidden: code reading/writing/testing/investigation        │
└─────────────────────────────┬────────────────────────────────┘
                              │
                              │ csa run < prompt_file
                              v
┌──────────────────────────────────────────────────────────────┐
│ Tier 1: Senior Employee (Planner / Implementer)             │
│                                                              │
│ Responsibilities:                                            │
│ • Plan, implement, review, validate                          │
│ • Decide HOW to execute                                      │
│ • Spawn Tier 2 workers when needed                           │
│ • Return complete result.toml                                │
└─────────────────────────────┬────────────────────────────────┘
                              │
                              │ csa run "sub-task"
                              v
┌──────────────────────────────────────────────────────────────┐
│ Tier 2: Employee Worker                                     │
│                                                              │
│ Responsibilities:                                            │
│ • Focused exploration/fixes/review support                   │
│ • Reports back to Tier 1                                     │
└──────────────────────────────────────────────────────────────┘
```

## Operating Contract

### Tier 0 Manager: What You MUST Do

1. Dispatch work with clear task contracts.
2. Require self-contained `result.toml` from every employee run.
3. Base decisions on reports, not direct code inspection.
4. Gate transitions: APPROVE / MODIFY / REJECT / ESCALATE.
5. If confidence is low, trigger cross-review by another employee.
6. **After PR creation, invoke `/pr-codex-bot`** — this is MANDATORY. The pr-codex-bot skill handles `@codex review` triggering, polling (10 min timeout), fallback to local review, and the full bot review loop. Tier 1 executors MUST invoke this skill; it is NOT optional.

### Tier 0 Manager: What You MUST NEVER Do

Absolute prohibitions (SOP breach):

- NEVER `Read` source files (`*.rs`, `*.ts`, `*.py`, etc.).
- NEVER `Grep`/`Glob` source code.
- NEVER run build/lint/test commands.
- NEVER run git investigation commands (`git diff`, `git show`, etc.) to verify quality.
- NEVER read CSA transcript logs.
- NEVER read artifact contents (`TODO.md`, `review.md`, `design.md`, etc.).
- NEVER write/edit code files.
- NEVER run `csa review` or `csa debate` as self-investigation.
- NEVER replace employee verification with personal inspection.

Tier 0 may only:

- Run `csa run` to dispatch employee tasks.
- Read manager-facing structured report files (`result.toml` from primary/verification runs).
- Write temporary prompt files for dispatch.
- Use task tracking tools (`TaskCreate` / `TaskUpdate`).
- Ask user decisions/questions via `AskUserQuestion`.
- Present artifact **paths** only (never artifact content).
- Summarize report conclusions to the user; never forward raw artifact content.

### Tier 1/Tier 2 Employees: Autonomy Rules

Employees are professionals with subjective agency.

- Manager specifies **WHAT**; employee decides **HOW**.
- Employee chooses implementation details (algorithms, structures, error handling).
- Employee may delegate to lower tiers as needed.
- If requirements are ambiguous, employee returns `status = "needs_clarification"` with concrete questions.

## Structured Communication Protocol

### Manager -> Employee (Dispatch Packet)

Every task prompt MUST include:

1. Objective
2. Input context
3. Output format
4. Scope boundaries
5. DONE WHEN (mechanically verifiable)

Example DONE WHEN:

- `DONE WHEN: result.toml exists at $CSA_SESSION_DIR/result.toml and status is success.`
- `DONE WHEN: just pre-commit exits 0 and result.toml status is success.`

### Employee -> Manager (`result.toml`)

Employees MUST return a self-contained report that allows manager decision without opening any source/artifact content.
Employees MUST write this file to `$CSA_SESSION_DIR/result.toml` and print that path only.

Required schema:

```toml
[result]
status = "success"  # success | partial | error | needs_clarification
summary = "Implemented JWT validation with 15 test cases. All pass."
error_code = ""
session_id = "019c4c24-..."

[report]
what_was_done = "Added JwtValidator struct with verify_token method"
key_decisions = ["Used RS256 algorithm", "15-minute token expiry"]
risks_identified = ["No refresh token mechanism yet"]
files_changed = 3
tests_added = 15
tests_passing = true

[timing]
started_at = "2026-02-11T10:00:00Z"
ended_at = "2026-02-11T10:05:00Z"

[tool]
name = "claude-code"

[review]
author_tool = "claude-code"
reviewer_tool = "codex"

[artifacts]
todo_path = "$CSA_SESSION_DIR/artifacts/TODO.md"
commit_hash = "abc1234"
review_result = "CLEAN"
```

Clarification extension (when `status = "needs_clarification"`):

```toml
[clarification]
questions = [
  "Should token expiry be configurable?",
  "Should refresh tokens be included in this scope?"
]
blocking_reason = "Security requirement is ambiguous"
```

## Decision Rules for Manager

Given `result.status`:

1. `success`: summarize `result.summary` + `report.what_was_done` to user; continue workflow.
2. `partial`: summarize completed scope and unresolved risks; ask user whether to continue.
3. `needs_clarification`: ask user the employee's listed questions; do not proceed until answered.
4. `error`: report `error_code` + summary; ask user whether to retry, narrow scope, or stop.

Manager decision must rely on report fields, not source inspection.

## Trust Verification (Cross-Review, Not Self-Investigation)

When manager confidence is low, manager assigns an independent employee review.

### Trigger Conditions

- Employee report has unclear risk statements.
- Work is high-impact/security-sensitive.
- Contradictory findings across runs.
- User explicitly asks for independent verification.

### Verification Workflow

```
Manager receives Employee A result.toml
    │
    ├─ If confidence sufficient -> approve/reject directly
    │
    └─ If confidence insufficient:
        1) Assign Employee B for verification
           - Code quality verification: Employee B runs `csa review --diff`
           - Design correctness: Employee B runs `csa debate`
        2) Employee B returns verification result.toml
        3) Manager compares A vs B structured reports
        4) Manager reports combined assessment to user
```

Manager still MUST NOT inspect source code or diffs directly.

## End-to-End Workflow

### Phase 1: Planning

```
User request
  -> Manager writes planning prompt file
  -> Manager dispatches Tier 1 (csa run)
  -> Tier 1 explores/plans (can spawn Tier 2)
  -> Tier 1 writes TODO + result.toml
  -> Manager reads result.toml only
  -> Manager reports summary + TODO path to user
  -> User: APPROVE / MODIFY / REJECT
```

### Phase 2: Implementation

```
User APPROVE
  -> Manager writes implementation prompt file
  -> Manager dispatches Tier 1 with session continuity
  -> Tier 1 implements autonomously
  -> Tier 1 performs validation/review/commit workflow
  -> Tier 1 returns result.toml (commit_hash, review_result, summary)
  -> Manager reads result.toml only
  -> Manager reports outcome to user
```

### Phase 3: Verification (Optional, Recommended on Risk)

```
Manager not fully confident
  -> Manager dispatches independent verification employee
  -> Reviewer returns result.toml with verdict
  -> Manager synthesizes two reports (A + reviewer)
  -> Manager gives final recommendation to user
```

## Practical Dispatch Templates

### Template A: Planning Dispatch (Manager -> Tier 1)

```bash
PROMPT_FILE=$(mktemp /tmp/sa-plan-XXXXXX.txt)
cat > "$PROMPT_FILE" <<'PLAN_EOF'
You are Tier 1 Employee in sa manager-employee mode.

Read and follow AGENTS.md and CLAUDE.md.

OBJECTIVE:
[what to plan]

INPUT:
[user requirements and constraints]

OUTPUT FORMAT:
- Write TODO artifact to $CSA_SESSION_DIR/artifacts/TODO.md
- Write manager-facing result.toml to $CSA_SESSION_DIR/result.toml using required schema
- Print ONLY the result.toml path

SCOPE:
- You may read code, analyze architecture, and spawn Tier 2 workers.
- You own implementation strategy decisions.

DONE WHEN:
- $CSA_SESSION_DIR/artifacts/TODO.md exists
- $CSA_SESSION_DIR/result.toml exists with status in {success, partial, needs_clarification, error}
- $CSA_SESSION_DIR/result.toml contains [result], [report], [timing], [tool], [artifacts]
PLAN_EOF

csa run < "$PROMPT_FILE"
```

### Template B: Implementation Dispatch (Manager -> Tier 1)

```bash
PROMPT_FILE=$(mktemp /tmp/sa-impl-XXXXXX.txt)
cat > "$PROMPT_FILE" <<'IMPL_EOF'
You are Tier 1 Employee in sa manager-employee mode.

Read and follow AGENTS.md and CLAUDE.md.

OBJECTIVE:
Implement approved plan end-to-end.

INPUT:
- Approved TODO path: [path]
- Session context: [session id if any]

OUTPUT FORMAT:
- Perform implementation and validation autonomously
- Write manager-facing result.toml to $CSA_SESSION_DIR/result.toml using required schema
- Include commit_hash/review_result in [artifacts] when available
- Print ONLY the result.toml path

SCOPE:
- You choose HOW to implement.
- You may spawn Tier 2 workers.
- You must perform appropriate review before reporting success.

DONE WHEN:
- Implementation tasks are complete or explicitly marked partial/error
- $CSA_SESSION_DIR/result.toml exists and is self-contained for manager decision
IMPL_EOF

csa run --session "$SESSION_ID" < "$PROMPT_FILE"
```

### Template C: Trust Verification Dispatch (Manager -> Reviewer Employee)

```bash
PROMPT_FILE=$(mktemp /tmp/sa-verify-XXXXXX.txt)
cat > "$PROMPT_FILE" <<'VERIFY_EOF'
You are the independent reviewer (Employee B).

Read and follow AGENTS.md and CLAUDE.md.

OBJECTIVE:
Verify Employee A's reported outcome independently.

INPUT:
- Employee A result.toml path: [path]
- Verification type: [code-review | design-review]

OUTPUT FORMAT:
- Run independent verification (e.g. csa review --diff or csa debate)
- Write manager-facing result.toml to $CSA_SESSION_DIR/result.toml
- In [report], clearly state agreement/disagreement and why
- Print ONLY the result.toml path

SCOPE:
- Independent judgment required.
- Do not assume Employee A is correct.

DONE WHEN:
- Verification completed
- $CSA_SESSION_DIR/result.toml includes clear verdict in summary/report
VERIFY_EOF

csa run < "$PROMPT_FILE"
```

## Model Selection Guidelines

- Tool and thinking budget are determined by the tier system in
  `~/.config/cli-sub-agent/config.toml`. Do NOT hardcode `--tool` or `--thinking`.
- Cross-review should prefer a different model family than the original author
  (configured via `[review] tool = "auto"`).

## Forbidden Behaviors (Enforced)

### Tier 0 Manager

- No code reading
- No code editing
- No local testing
- No local review via diff inspection
- No transcript mining
- No artifact content inspection
- No self-verification by technical investigation

### Tier 1 Senior Employee

- No under-specified reports (must be self-contained)
- No hiding risks in vague wording
- No success status without explicit validation outcome

### Tier 2 Worker

- No unilateral scope expansion
- No irreversible architectural changes without Tier 1 approval

## Success Criteria for This Skill

This `sa` skill is working as designed only when all are true:

1. Manager can make decisions using only `result.toml`.
2. Manager never reads code or artifacts directly.
3. Verification is done by independent employee cross-review.
4. Employee reports include enough detail to avoid manager investigation loops.
5. User receives concise natural-language summaries rather than raw artifact dumps.
