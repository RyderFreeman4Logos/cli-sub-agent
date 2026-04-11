---
name: pr-bot
description: "Use when: PR review loop with cloud bot, arbitration, and merge"
allowed-tools: Bash, Read, Grep, Glob, Edit, Write
triggers:
  - "pr-bot"
  - "/pr-bot"
  - "cloud bot review"
  - "PR bot"
  - "PR review bot"
  - "merge PR"
---

# PR Bot: Two-Layer PR Review and Merge (Configurable Cloud Bot)

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the pr-bot skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `../../PATTERN.md` relative to this `SKILL.md`, and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Orchestrate the full PR review-and-merge lifecycle with two-layer review: local pre-PR cumulative audit (covering main...HEAD) plus configurable cloud bot review (default: gemini-code-assist; configurable via `pr_review.cloud_bot_name`). When bot times out, the workflow **aborts** (no silent fallback merge). Performs false-positive arbitration via adversarial debate, and manages fix-push-retrigger loops with user-prompted round limits (MAX_REVIEW_ROUNDS, default 10). Non-target bot comments (e.g., codex auto-review) are also detected and processed with a quota warning. Merges with `--merge` to preserve per-commit audit trail.

**MANDATORY AUDIT TRAIL**: When an agent determines a PR-page review finding
(for example, a cloud bot finding) is NOT a real issue or is acceptable in
context (e.g., pre-production breaking change), the agent MUST post an
explanatory comment on the PR page BEFORE merging or proceeding. This creates a
permanent record of the rationale behind every dismissed PR-page finding.
Local pre-PR review findings must be fixed before PR creation; they do not use
the PR-page audit trail because no PR page exists yet. FORBIDDEN: merging with
dismissed PR-page findings without explanatory PR comments.

FORBIDDEN: self-dismissing bot comments, skipping debate for arbitration, auto-merging at round limit, proceeding when bot responds with environment/configuration setup message instead of an actual code review (MUST stop and ask user to configure).

## Dispatcher Model

pr-bot follows a 3-layer dispatcher architecture. The main agent never
performs implementation work directly -- it orchestrates sub-agents that do the
actual review, fixing, and merging.

### Layer 0 -- Orchestrator (Main Agent)

The main agent (Claude Code / human user) acts as a **pure dispatcher**:

- Reads SKILL.md and PATTERN.md to understand the workflow
- Dispatches each step to the appropriate sub-agent or tool
- Evaluates sub-agent results and decides next action (fix, retry, merge, abort)
- **NEVER reads or writes code directly** -- all code-touching work is delegated
- **NEVER runs `csa review` / `csa debate` itself** -- spawns a Layer 1 executor

### Layer 1 -- Executor Sub-Agents (CSA / Task Tool)

Layer 1 agents perform the actual work dispatched by Layer 0:

| Step | Layer 1 Agent | Work Performed |
|------|-------------|----------------|
| Step 2 | `csa review --branch main` | Cumulative local review |
| Step 3 | `csa` (executor) | Fix local review issues |
| Step 7 | claude-code (Task tool) | Classify bot comments |
| Step 8 | `csa debate` | False-positive arbitration |
| Step 9 | `csa` (executor) | Fix real issues |

Layer 1 agents have full file system access and can read/write code, run tests,
and interact with git. They receive a scoped task from Layer 0 and return
results.

### Layer 2 -- Sub-Sub-Agents (Spawned by Layer 1)

Layer 1 agents may spawn their own sub-agents for specific sub-tasks:

- `csa review` internally spawns reviewer model(s) for independent analysis
- `csa debate` spawns two independent models for adversarial evaluation
- Task tool sub-agents may use Grep/Glob for targeted code search

Layer 2 agents are invisible to Layer 0 -- the orchestrator only sees Layer 1
results.

### Flow Diagram

```
Layer 0 (Orchestrator)
  |
  +-- dispatch --> Layer 1: csa review --branch main
  |                  |
  |                  +-- spawn --> Layer 2: reviewer model(s)
  |
  +-- evaluate result, decide next step
  |
  +-- dispatch --> Layer 1: csa (fix issues)
  |
  +-- dispatch --> Layer 1: bash (push, create PR, trigger bot)
  |
  +-- dispatch --> Layer 1: claude-code (classify comments)
  |
  +-- dispatch --> Layer 1: csa debate (arbitrate false positives)
  |                  |
  |                  +-- spawn --> Layer 2: independent models
  |
  +-- dispatch --> Layer 1: bash (merge)
```

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- All changes must be committed on a feature branch
- Feature branch must be ahead of main
- **FORBIDDEN**: Pushing the feature branch to remote BEFORE invoking this skill.
  Step 2 (local review) MUST complete before any push. If you push first,
  unreviewed code reaches the remote and CI/reviewers may act on it prematurely.
  The skill's Step 4 handles push after review passes.

### Configuration

The cloud bot is configurable per-project/global via `.csa/config.toml`:

```toml
[pr_review]
cloud_bot = true                           # false to skip cloud review entirely
cloud_bot_name = "gemini-code-assist"      # bot name (for @mention and display)
cloud_bot_trigger = "auto"                 # "auto" (bot auto-reviews) | "comment" (@bot review)
cloud_bot_login = ""                       # bot GitHub login override (default: "${cloud_bot_name}[bot]")
cloud_bot_retrigger_command = ""           # command to re-trigger after fix push (default: derived from name)
merge_strategy = "merge"                   # "merge" | "rebase" (squash is forbidden for audit)
delete_branch = false                      # delete remote branch after merge
```

**Check at runtime**: `csa config get pr_review.cloud_bot_name --default gemini-code-assist`

**Trigger modes**:
- `"auto"` (default): Bot auto-reviews on PR creation push. No @mention needed.
- `"comment"`: Posts `@{cloud_bot_name} review` comment to trigger review.

**Retrigger** (round 2+ after fix push): Bots like gemini-code-assist do NOT
auto-review on subsequent pushes — only on PR creation. The workflow ALWAYS posts
an explicit retrigger command on round 2+, regardless of `cloud_bot_trigger`.
Default: `/gemini review` for gemini-code-assist, `@{name} review` for others.
Override via `cloud_bot_retrigger_command`.

**Timeout behavior**: If bot does not respond within the configured polling window
(`cloud_bot_wait_seconds` + `cloud_bot_poll_max_seconds`, default ~5 minutes via `kv_cache.frequent_poll_seconds = 60` and `kv_cache.long_poll_seconds = 240`),
the workflow **aborts** and presents options to the user. It does NOT silently
fall back to local review and merge.

When `cloud_bot = false`:
- Steps 4-9 (cloud bot trigger, delegated wait gate, classify, arbitrate, fix) are **skipped entirely**
- A SHA-verified fast-path check is applied before supplementary local review
- The workflow proceeds directly to merge after local review passes
- This avoids the cloud bot wait and GitHub API dependency

### Quick Start

```bash
csa run --sa-mode true --skill pr-bot "Review and merge the current PR"
```

### SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands. Omitting `--sa-mode`
at root depth causes a hard error; passing `false` when the caller is in SA mode
breaks prompt-guard propagation.

### Step-by-Step

1. **Commit check**: Ensure all changes are committed. Record `WORKFLOW_BRANCH`.
2. **Local pre-PR review** (SYNCHRONOUS -- MUST NOT background): use SHA-verified fast-path first (`CURRENT_HEAD` vs latest reviewed session HEAD SHA from `review_meta.json`). If matched, skip review; if mismatched/missing, run full `csa review --branch main --fix --max-rounds 3` (the `--fix` flag resumes the same reviewer session to fix issues, preserving full review context). This is the foundation -- without it, bot unavailability cannot safely merge. Sets `REVIEW_COMPLETED=true` on success.
3. **Push and ensure PR** (PRECONDITION: `REVIEW_COMPLETED=true`): Detect if branch was already pushed (early-push warning). Push with `--force-with-lease`, derive `source_owner` from `origin` remote URL, then resolve PR strictly by owner-aware lookup (`base=main + head=<source_owner>:${WORKFLOW_BRANCH}`). If none exists, create with `--head <source_owner>:<branch>` and re-resolve; handle create races where PR was created concurrently. FORBIDDEN: creating/reusing PR without Step 2 completion.
3a. **Check cloud bot config**: Run `csa config get pr_review.cloud_bot --default true`.
    If `false` → skip Steps 4-9. Apply the same SHA-verified fast-path before
    supplementary review. If SHA matches, skip review; if SHA mismatches/missing
    (HEAD drift fallback), run full `csa review --branch main`. Then route through
    the bot-unavailable merge path (Step 6a).
4. **Trigger cloud bot and delegate waiting** (SELF-CONTAINED -- trigger + wait gate are atomic):
   - **Round 0** (initial PR): follows `cloud_bot_trigger` config (`"comment"` → @mention, `"auto"` → skip).
   - **Round 1+** (after fix push): ALWAYS posts explicit retrigger command (`cloud_bot_retrigger_command`, default: `/gemini review` for gemini-code-assist) because bots do NOT auto-review on subsequent pushes.
   - Wait `cloud_bot_wait_seconds` quietly, then delegate `cloud_bot_poll_max_seconds` polling to CSA. Defaults are `kv_cache.frequent_poll_seconds` (60s) for the quiet wait and `kv_cache.long_poll_seconds` (240s) for the total poll budget unless explicitly overridden.
   - **Positive signal**: verifies a review EVENT exists (via `pulls/{pr}/reviews` API with `submitted_at` > push time), not merely absence of comments.
   - If bot times out: **ABORT workflow** and present options to user. NO silent fallback.
   - Non-target bot comments (e.g., codex auto-review) are also detected and included with a quota warning.
5. **Evaluate bot comments**: Classify each as:
   - Category A (already fixed): react and acknowledge.
   - Category B (suspected false positive): queue for staleness filter, then arbitrate.
   - Category C (real issue): queue for staleness filter, then fix.
6. **Staleness filter** (before arbitration/fix): For each comment classified as B or C, check if the referenced code has been modified since the comment was posted. Compare comment file paths and line ranges against `git diff main...HEAD` and `git log --since="${COMMENT_TIMESTAMP}"`. Comments referencing lines changed after the comment timestamp are reclassified as Category A (potentially stale, already addressed) and skipped. This prevents debates and fix cycles on already-resolved issues.
7. **Arbitrate non-stale false positives**: For surviving Category B comments, arbitrate via `csa debate` with independent model. Require structured debate output, then post the PR audit trail through an explicit `gh pr comment` step. If debate overturns the false-positive classification, reroute that comment into the real-issue fix step instead of posting a dismissal comment.
8. **Fix non-stale real issues**: For surviving Category C comments, fix using `csa review --fix` to resume the reviewer session (preserves review context, avoids 50K+ token waste of spawning fresh). Commit fixes, then run `csa review --range main...HEAD` (review gate) BEFORE pushing — unreviewed fix code must not reach the remote.
9. **Continue loop**: Push fixes and loop back (next trigger is issued in Step 4). Track iteration count via `REVIEW_ROUND`. When `REVIEW_ROUND` reaches `MAX_REVIEW_ROUNDS` (default: 10), STOP and present options to the user: (A) Merge now, (B) Continue for more rounds, (C) Abort and investigate manually. The workflow MUST NOT auto-merge or auto-abort at the round limit.
10. **Clean resubmission** (if fixes accumulated): Create clean branch for final review.
10.5. ~~**Rebase for clean history**~~: DISABLED. With merge commits (not squash), rebase destroys per-commit audit trail. Squash merges are forbidden for audit reasons.
11. **Merge**: When `cloud_bot=false`, leave audit trail comment explaining merge rationale (bot disabled + local review CLEAN). When `cloud_bot=true`, bot must have confirmed no issues before reaching this step (timeout aborts the workflow, never falls through to merge). Read merge strategy from `csa config get pr_review.merge_strategy --default merge` and branch deletion from `csa config get pr_review.delete_branch --default false`. Then `gh pr merge --${MERGE_STRATEGY} [--delete-branch]`, then `git fetch origin && git checkout main && git merge origin/main --ff-only`.

## Example Usage

| Command | Effect |
|---------|--------|
| `/pr-bot` | Full review loop on current branch's PR |
| `/pr-bot pr=42` | Run review loop on existing PR #42 |

## Integration

- **Depends on**: `csa-review` (Step 2 local review), `debate` (Step 6 false-positive arbitration)
- **Used by**: `commit` (Step 13 auto PR), `dev2merge` (Steps 17-25), `dev-to-merge` (legacy alias)
- **ATOMIC with**: PR creation -- Steps 1-9 are an atomic unit; NEVER stop after PR creation

## Done Criteria

1. Step 2 completed synchronously (not backgrounded) via one of:
   - full path: `csa review --branch main`, or
   - fast-path: current HEAD SHA matches latest reviewed session HEAD SHA.
   - `REVIEW_COMPLETED=true` is set after successful completion.
2. Any local review issues are fixed before PR creation.
3. PR resolved for the workflow branch (existing PR reused or a new PR created via strict owner-aware match, with create-race recovery and Step 4 precondition verified: `REVIEW_COMPLETED=true`).
4. Cloud bot config checked (`csa config get pr_review.cloud_bot --default true`).
5. **If cloud_bot enabled (default)**: cloud bot triggered (round-aware: auto on round 0, explicit retrigger on round 1+), wait handled by delegated CSA gate with hard timeout and positive review-event signal checks, and timeout path handled. If bot responds with environment/configuration setup message instead of actual review, workflow STOPS and reports to user (Step 5a).
6. **If cloud_bot disabled**: supplementary check completed via one of:
   - fast-path: SHA match, review skipped, or
   - fallback path: SHA mismatch/missing (HEAD drift) and full `csa review --branch main` executed.
7. Every bot comment classified (A/B/C) and actioned appropriately (cloud_bot enabled only).
8. Staleness filter applied (cloud_bot enabled only).
9. Non-stale false positives arbitrated via `csa debate` (cloud_bot enabled only).
10. Real issues fixed and re-reviewed (cloud_bot enabled only).
10a. **Post-fix re-review gate** (HARD GATE): After fixing bot findings, bot is re-triggered on current HEAD via explicit retrigger command (NOT relying on auto-review), uses the same configurable wait policy as the initial gate (`cloud_bot_wait_seconds` quiet wait + `cloud_bot_poll_max_seconds` polling), and requires a **positive review event** (via `pulls/{pr}/reviews` API, filtered by `commit_id`) with zero actionable findings. If no review event or API failure, falls back to local `csa review --range main...HEAD`. If new findings appear, workflow aborts (user must re-run pr-bot).
10b. **Round limit**: If `REVIEW_ROUND` reaches `MAX_REVIEW_ROUNDS` (default: 10), user was prompted with options (merge/continue/abort) and explicitly chose before proceeding.
10c. ~~**Rebase for clean history**~~ (Step 10.5): DISABLED — merge commits preserve audit trail directly.
11. **Audit trail**: Every dismissed PR-page finding (for example, a bot finding) has a corresponding explanatory PR comment posted by an explicit workflow step BEFORE proceeding or merging.
12. PR merged via configured strategy (default: merge commit, full history preserved). Branch deletion controlled by `pr_review.delete_branch` config (default: false — branches preserved for audit).
13. Local main updated: `git fetch origin && git checkout main && git merge origin/main --ff-only`.
