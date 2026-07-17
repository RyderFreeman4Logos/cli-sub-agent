---
name = "dev2merge"
description = "Hard-gated dev2merge"
allowed-tools = "Bash, Read, Edit, Write, Grep, Glob, Task, TaskCreate, TaskUpdate, TaskList, TaskGet"
tier = "tier-3-complex"
version = "0.4.0"
---

# Dev2Merge: Deterministic Development Pipeline

End-to-end development workflow enforced as a weave workflow. Every stage has
hard gates (`on_fail = "abort"`). No step can be skipped by the LLM.

Pipeline: Already-Resolved Check → Branch Validation → FAST_PATH Detection → mktd (planning) →
mktsk N*(implement → commit) → Bash L1/L2 Self-Review Gate → Pre-PR Cumulative Review → Push →
Pre-PR Verdict Check → PR Creation → **pr-bot Hard Gate** → Post-Merge Sync.

**CRITICAL PIPELINE INVARIANT**: Step 15 PR creation and Step 16 pr-bot are
separate hard gates. Creating a PR is not completion: run pr-bot after PR
creation, never skip Step 16, and never raw `gh pr merge`. If an approved
emergency requires manual merge after a recorded pr-bot pass, use
`csa merge <PR_NUMBER>` so the local gate still runs. Stopping after Step 15
leaves the PR unmerged.

### Implementation Executor Override

`IMPL_TIER`/`IMPL_TOOL` default empty (`[Sub:developer]`); set them so mktd
emits `[CSA:<tier-or-tool>]` plus Step 8 `csa run` override flags.

### Execution Modes (`DEV2MERGE_MODE`)

`DEV2MERGE_MODE` selects how much of the pipeline runs (default `full`):

- `full` — the complete pipeline (planning + implementation + all gates).
- `resume` — tail-only: the work is already implemented and committed on the
  branch, so planning (Step 7 mktd, Step 8 mktsk) is skipped. Step 9 (Resume
  Commit) commits any uncommitted remainder, then every downstream gate still
  runs unchanged.

```bash
# tail-only run for already-implemented work
csa plan run --sa-mode true patterns/dev2merge/workflow.toml --var DEV2MERGE_MODE=resume
```

`resume` keeps ALL hard gates — version bump, self-review, cumulative review,
push, verdict, PR creation, and pr-bot all still run; it ONLY skips planning.
Step 2 derives `RESUME_MODE` from `DEV2MERGE_MODE`; it gates the planning steps
off (`!(${RESUME_MODE})`) and the Resume Commit step on (`${RESUME_MODE}`).
`FAST_PATH` (docs-only) takes precedence: a docs-only resume run still follows
the FAST_PATH branch.

### ABSOLUTE PROHIBITIONS

These prohibitions apply at EVERY level of dev2merge (orchestrator, mktsk
executor, fix-loop). They are the recovery-path rules whose violation
produced #1121, #1122, and #1123. Surface failures upward instead of
escalating to any prohibited primitive.

#### Hook-bypass primitives (#1123)

FORBIDDEN — all of these silently disable registered git hooks:

- `git commit --no-verify` / `-n`
- `git push --no-verify`
- `LEFTHOOK=0`, `LEFTHOOK_DISABLED=1`
- `HUSKY=0`, `HUSKY_DISABLE=1`
- `SKIP_HOOKS=1`, `SKIP_GIT_HOOKS=1`
- `--no-gpg-sign`
- ANY equivalent env var or CLI flag that disables a registered hook

**Re-stage recovery primitive** (when `git commit` fails because lefthook
re-staged auto-formatted files):

1. `git diff --staged --quiet` — exit 0 means clean (rare)
2. `git add -u` — re-stage the formatter's output
3. Retry `git commit -m "..."` — hooks accept the formatted version on the second pass
4. If recovery loops >=3 iterations without converging, surface `recovery_loop_exhausted`. NEVER escalate to bypass.

#### Squash-merge primitives (#1122)

FORBIDDEN — all of these destroy per-commit audit trails:

- `gh pr merge --squash`
- `gh pr merge -s`
- `git merge --squash`
- GitHub Web UI "Squash and merge"
- ANY `--squash` flag on a merge command

**Empty-diff structural guard**: Before any merge (or before delegating to
pr-bot), verify `gh pr diff <PR>` is non-empty AND the branch has commits
ahead of the default branch with a non-empty cumulative diff. An empty-diff PR is the
structural fingerprint of the lefthook re-stage race documented in #1122 —
when seen, surface `merge_blocked_empty_diff` instead of pushing through.
Squash-merging an empty-diff PR produces an empty squash commit on the default branch;
this is the exact corruption #1122 documents.

dev2merge delegates merging to pr-bot (Step 16), which reads
`pr_review.merge_strategy` from config (default `merge`). Even if a normal
`--merge` fails, DO NOT escalate to `--squash`. Surface `merge_blocked` to
the orchestrator.

Sub-workflows are included via `## INCLUDE`, not inlined.

## Step 0: Already-Resolved Check
Tool: bash
OnFail: abort
Short-circuit no-op runs before branch validation:

- Best-effort issue check: with `ISSUE_NUMBER`, `gh issue view` skips `CLOSED`
  issues; `gh` failures warn and continue.
- Merge-completion skip requires both ancestor confirmation and a merged PR for
  the current branch.
- Ancestor success without a merged PR means a fresh branch and continues.

`--issue` absent skips issue lookup. Skips print
`dev2merge: ... nothing to do`, set `DEV2MERGE_SKIP=true`, exit 0.
Issue and PR lookup failures fail open.

```bash
set -euo pipefail
echo "CSA_VAR:DEV2MERGE_SKIP=false"
skip() { echo "$1"; echo "CSA_VAR:DEV2MERGE_SKIP=true"; exit 0; }

if [ -n "${ISSUE_NUMBER:-}" ]; then
  ISSUE_STATE="$(GH_CONFIG_DIR=~/.config/gh-aider gh issue view "$ISSUE_NUMBER" --repo "${GH_REPO:-$(gh repo view --json nameWithOwner -q .nameWithOwner 2>/dev/null)}" --json state -q .state 2>&1)" || {
    # best-effort: gh failure → warn + continue (not a hard gate)
    echo "dev2merge: WARNING: issue check failed; continuing"
    ISSUE_STATE=
  }
  if [ "$ISSUE_STATE" = CLOSED ]; then
    skip "dev2merge: issue #${ISSUE_NUMBER} is already CLOSED — nothing to do"
  fi
fi

BRANCH="$(git branch --show-current)"
[ -n "${BRANCH}" ] && [ "${BRANCH}" != "HEAD" ] || exit 0
DEFAULT_BRANCH="$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@' || true)"
[ -n "${DEFAULT_BRANCH}" ] || DEFAULT_BRANCH="main"
[ -z "$(git status --porcelain)" ] || exit 0

if git merge-base --is-ancestor HEAD "origin/${DEFAULT_BRANCH}" 2>/dev/null; then
  MERGED_PR="$(gh pr list --head "${BRANCH}" --state merged --json number -q '.[0].number' 2>/dev/null || true)"
  if [ -n "${MERGED_PR}" ]; then skip "dev2merge: branch ${BRANCH} already merged via PR #${MERGED_PR}; HEAD is ancestor of ${DEFAULT_BRANCH} — nothing to do"; fi
fi
```

## Step 1: Validate Branch
Tool: bash
OnFail: abort
Verify the current branch is a feature branch, not protected.

```bash
BRANCH="$(git branch --show-current)"
if [ -z "${BRANCH}" ] || [ "${BRANCH}" = "HEAD" ]; then
  echo "ERROR: Cannot determine current branch."
  exit 1
fi
DEFAULT_BRANCH=$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')
if [ -z "$DEFAULT_BRANCH" ]; then DEFAULT_BRANCH="main"; fi
if [ "$BRANCH" = "$DEFAULT_BRANCH" ] || [ "$BRANCH" = "dev" ]; then
  echo "ERROR: Cannot work directly on $BRANCH. Create a feature branch."
  exit 1
fi
echo "CSA_VAR:WORKFLOW_BRANCH=$BRANCH"
echo "CSA_VAR:DEFAULT_BRANCH=$DEFAULT_BRANCH"
```

## Step 2: FAST_PATH Detection
Tool: bash
OnFail: abort
Detect whether changes are docs/config-only. When FAST_PATH=true,
skip mktd/mktsk/debate but keep L1/L2 quality checks. An empty diff
vs the base branch is NOT docs-only; it must stay on the full plan
path to avoid no-op commit/version-bump steps. Also classify resume
runs: when DEV2MERGE_MODE=resume the implementation already exists, so
RESUME_MODE gates the planning steps (mktd/mktsk) off while every
review/merge gate downstream still runs.

```bash
set -euo pipefail
# Resume-mode classification (#1662): skip planning when DEV2MERGE_MODE=resume.
RESUME_MODE=false
if [ "${DEV2MERGE_MODE:-full}" = "resume" ]; then
  RESUME_MODE=true
fi
echo "CSA_VAR:RESUME_MODE=${RESUME_MODE}"
CODE_FILES="$(git diff --name-only "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | awk '!/\.(md|txt|lock|toml)$/ { count++ } END { print count + 0 }')"
TOTAL_FILES="$(git diff --name-only "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | wc -l | xargs)"
TOTAL_INSERTIONS="$(git diff --stat "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | tail -1 | grep -oE '[0-9]+ insertion' | grep -oE '[0-9]+' || echo 0)"
if [ "${TOTAL_FILES}" -eq 0 ]; then
  # Empty diff means HEAD matches the base branch; do not treat that as docs-only,
  # or the workflow will skip planning and then hit no-op commit/version-bump steps.
  echo "Full pipeline: branch has no diff vs ${DEFAULT_BRANCH}; running plan."
  echo "CSA_VAR:FAST_PATH=false"
elif [ "${CODE_FILES}" -eq 0 ] && [ "${TOTAL_INSERTIONS:-0}" -lt 100 ]; then
  echo "FAST_PATH: docs/config-only changes detected. Skipping mktd/mktsk."
  echo "CSA_VAR:FAST_PATH=true"
else
  echo "Full pipeline: ${CODE_FILES} code files, ${TOTAL_INSERTIONS} insertions."
  echo "CSA_VAR:FAST_PATH=false"
fi
```

## Step 3: Cheap Repo Preconditions
Tool: bash
OnFail: abort
Cheap repo-local checks run before any cargo build, test, or review gate: staged-scope ownership and optional version-bump detection.
```bash
set -euo pipefail
STAGED_FILES="$(git diff --cached --name-only)"
if [ -n "${STAGED_FILES}" ]; then
  echo "ERROR: staged-scope precondition failed — staged files exist before dev2merge owns staging." >&2
  printf '%s\n' "${STAGED_FILES}" >&2
  echo "Commit or unstage pre-existing staged work before retrying." >&2
  exit 1
fi
if [ -f Cargo.toml ] && just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "check-version-bumped"; then
  if just check-version-bumped; then
    echo "Version precondition: already bumped."
  else
    echo "Version precondition: bump required; version bump step will run before expensive review gates."
  fi
else
  echo "Version precondition: optional check-version-bumped recipe absent."
fi
echo "Cheap preconditions passed before build/review gates."
```

## IF ${FAST_PATH}

## Step 4: FAST_PATH Commit
Tool: bash
OnFail: abort
For docs/config-only changes, run a simplified commit flow:
stage, generate message, commit. No mktd/mktsk/security-audit overhead.

```bash
set -euo pipefail
DEFAULT_BRANCH="${DEFAULT_BRANCH:-main}"
if [ -z "$(git status --porcelain)" ]; then
  COMMITS_AHEAD="$(git rev-list --count "${DEFAULT_BRANCH}..HEAD" 2>/dev/null || echo 0)"
  if [ "${COMMITS_AHEAD}" -gt 0 ]; then
    echo "FAST_PATH Commit: existing commits detected, working tree clean — skipping commit step"
    echo "CSA_VAR:FAST_PATH_COMMITTED=true"
    exit 0
  else
    echo "ERROR: No commits and no staged files."
    exit 1
  fi
fi
git add -A
if ! git diff --cached --name-only | grep -q .; then
  echo "ERROR: No staged files after staging dirty working tree."
  exit 1
fi
git diff --cached --check
COMMIT_MSG="$(scripts/gen_commit_msg.sh "${SCOPE:-}" 2>/dev/null || echo "docs: update documentation")"
git commit -m "${COMMIT_MSG}"
echo "CSA_VAR:FAST_PATH_COMMITTED=true"
```

## Step 5: FAST_PATH Version Bump
Tool: bash
OnFail: abort
Optional bump; missing recipes skip (#1658/#2305).

```bash
set -euo pipefail
h(){ just --summary 2>/dev/null|tr ' ' '\n'|grep -qx "$1"; }
s(){ echo "Version bump skipped: $1"; exit 0; }
[ -f Cargo.toml ] || s no-Cargo.toml
h check-version-bumped || s no-check-version-bumped
just check-version-bumped&&exit 0||true
h bump-patch || s no-bump-patch
just bump-patch
cargo run -p weave -- lock 2>/dev/null || true
git add Cargo.toml Cargo.lock weave.lock 2>/dev/null || git add Cargo.toml weave.lock
VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')"
git commit -m "chore(release): bump workspace version to ${VERSION}"
```

## Step 6: FAST_PATH Pre-PR Review
Tool: bash
OnFail: abort
FAST_PATH runs L1/L2 gates and cumulative review before push.
```bash
set -euo pipefail
if [ -f Cargo.toml ]; then
  just fmt
  just clippy
  just test
elif [ -f pyproject.toml ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "lint"; then
    just lint
  elif command -v ruff >/dev/null 2>&1; then
    ruff check .
    ruff format --check .
  fi
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then
    just test
  elif command -v pytest >/dev/null 2>&1; then
    pytest
  fi
elif [ -f package.json ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "lint"; then
    just lint
  elif command -v biome >/dev/null 2>&1; then
    biome check .
  fi
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then
    just test
  elif command -v vitest >/dev/null 2>&1; then
    vitest run
  fi
elif [ -f go.mod ]; then
  go vet ./...
  if command -v golangci-lint >/dev/null 2>&1; then
    golangci-lint run
  fi
  go test ./...
elif just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "pre-commit"; then
  just pre-commit
else
  echo "WARNING: No recognized project type; skipping FAST_PATH L1/L2 gate."
fi
bash "${CSA_WORKFLOW_DIR:-patterns/dev2merge}/scripts/csa/cumulative-review-batch.sh" --default-branch "${DEFAULT_BRANCH}" -- \
  csa review --sa-mode true --range "${DEFAULT_BRANCH}...HEAD"
echo "CSA_VAR:REVIEW_COMPLETED=true"
echo '<!-- CSA:NEXT_STEP cmd="push to origin (Step 13)" required=true -->'
```

## ELSE

## Step 7: Plan with mktd
Tool: bash
OnFail: abort
Condition: !(${RESUME_MODE})
mktd TODO; --pattern mktd default, MKTD_WORKFLOW_PATH override, auto light/full.

```bash
bash "${CSA_WORKFLOW_DIR:-patterns/dev2merge}/scripts/csa/plan-with-mktd.sh"
```

## Step 8: Execute Plan with mktsk

OnFail: abort
Tool: manual (main agent action)
Condition: !(${RESUME_MODE})

Skipped in resume mode (work already implemented).
Run mktsk in main context with TODO `${MKTD_TODO_TIMESTAMP}` and
`CSA_SKIP_PUBLISH=true`; impl `${IMPL_TIER}`/`${IMPL_TOOL}`.
`[CSA:<value>]` impl tasks use `csa run`; `Implementation override: csa run ...`
wins, else tier-*→`--tier`, other→`--tool`.

## Step 9: Resume Commit
Tool: bash
OnFail: abort
Condition: ${RESUME_MODE}
Resume mode skips mktd/mktsk because the work is already on the branch. Commit any uncommitted remainder so the version, review, and push gates see the full diff. Mirrors the FAST_PATH commit (Step 4); fails closed when there is no work.
```bash
set -euo pipefail
DEFAULT_BRANCH="${DEFAULT_BRANCH:-main}"
if [ -z "$(git status --porcelain)" ]; then
  COMMITS_AHEAD="$(git rev-list --count "${DEFAULT_BRANCH}..HEAD" 2>/dev/null || echo 0)"
  if [ "${COMMITS_AHEAD}" -gt 0 ]; then
    echo "Resume Commit: working tree clean, ${COMMITS_AHEAD} commit(s) ahead — nothing to commit."
    exit 0
  fi
  echo "ERROR: resume mode requires committed or staged work, but the tree is clean with 0 commits ahead of ${DEFAULT_BRANCH}."
  exit 1
fi
git add -A
if ! git diff --cached --name-only | grep -q .; then
  echo "ERROR: No staged files after staging dirty working tree."
  exit 1
fi
git diff --cached --check
COMMIT_MSG="$(scripts/gen_commit_msg.sh "${SCOPE:-}" 2>/dev/null || echo "chore: commit resumed work")"
git commit -m "${COMMIT_MSG}"
```

## Step 10: Ensure Version Bumped
Tool: bash
OnFail: abort
Optional bump; missing recipes skip (#1658/#2305).

```bash
set -euo pipefail
h(){ just --summary 2>/dev/null|tr ' ' '\n'|grep -qx "$1"; }
s(){ echo "Version bump skipped: $1"; exit 0; }
[ -f Cargo.toml ] || s no-Cargo.toml
h check-version-bumped || s no-check-version-bumped
just check-version-bumped&&exit 0||true
h bump-patch || s no-bump-patch
just bump-patch
cargo run -p weave -- lock 2>/dev/null || true
git add Cargo.toml Cargo.lock weave.lock 2>/dev/null || git add Cargo.toml weave.lock
VERSION="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')"
git commit -m "chore(release): bump workspace version to ${VERSION}"
```

## Step 11: Self-Review Gate

Tool: bash
OnFail: abort
Full and resume paths run the deterministic L1/L2 quality gate before
cumulative CSA review. The shared authoritative recipe publishes an exact-input
receipt when available; other repositories retain language-aware fallback checks.

```bash
set -euo pipefail
if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "quality-gates"; then
  just quality-gates
elif [ -f Cargo.toml ]; then
  just fmt
  just clippy
  just test
elif [ -f pyproject.toml ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "lint"; then
    just lint
  elif command -v ruff >/dev/null 2>&1; then
    ruff check .
    ruff format --check .
  fi
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then
    just test
  elif command -v pytest >/dev/null 2>&1; then
    pytest
  fi
elif [ -f package.json ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "lint"; then
    just lint
  elif command -v biome >/dev/null 2>&1; then
    biome check .
  fi
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then
    just test
  elif command -v vitest >/dev/null 2>&1; then
    vitest run
  fi
elif [ -f go.mod ]; then
  go vet ./...
  if command -v golangci-lint >/dev/null 2>&1; then
    golangci-lint run
  fi
  go test ./...
elif just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "pre-commit"; then
  just pre-commit
else
  echo "WARNING: No recognized project type; skipping L1/L2 gate."
fi
```

## Step 11.5: Decomposition Review Depth Warning
Tool: bash
OnFail: skip
Warn when the branch diff is broad enough to dilute review depth. This is
advisory only and does not block Step 12.

```bash
# Decomposition check — WARN only, does not block
CHANGED_FILES=$(git diff --name-only "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | wc -l | tr -d ' ')
if [ "${CHANGED_FILES:-0}" -gt 5 ]; then
  echo "WARN: Branch has $CHANGED_FILES changed files (>5). Large diffs dilute review depth. Consider splitting into separate issue-scoped PRs."
  echo "Consider using --depth audit for this review."
fi
```

## Step 12: Pre-PR Cumulative Review Gate
Tool: bash
OnFail: abort
Cumulative review covering all commits since the default branch.
Helper owns verdict check; batching skip needs no current-head artifact.
Sets REVIEW_COMPLETED=true for the push gate.

```bash
set -euo pipefail
bash "${CSA_WORKFLOW_DIR:-patterns/dev2merge}/scripts/csa/cumulative-review-batch.sh" --default-branch "${DEFAULT_BRANCH}" -- \
  csa review --sa-mode true --range "${DEFAULT_BRANCH}...HEAD"
echo "CSA_VAR:REVIEW_COMPLETED=true"
echo '<!-- CSA:NEXT_STEP cmd="push to origin (Step 13)" required=true -->'
```

When global config sets `[review].batch_commits >= 2`, intermediate cumulative
reviews can be skipped until the branch accumulates enough new commits after
the last passed `${DEFAULT_BRANCH}...HEAD` review. Set `CSA_REVIEW_NOW=1` to bypass batching
for the current run.

## ENDIF

## Step 13: Push Gate
Tool: bash
OnFail: abort
Hard gates before any push:

1. `REVIEW_COMPLETED` must be true.
2. **Empty-diff structural guard (#1122)**: branch must have at least one
   commit ahead of base AND the cumulative diff vs base must be non-empty.
   An empty-diff branch is the lefthook-race fingerprint — it produces an
   empty PR which can be silently squashed into an empty commit on the default branch.
   Aborting here surfaces `merge_blocked_empty_diff` to the orchestrator
   instead of letting it propagate to pr-bot.

```bash
if [ "${REVIEW_COMPLETED:-}" != "true" ]; then
  echo "ERROR: Push blocked — pre-PR review not completed."
  echo "REVIEW_COMPLETED=${REVIEW_COMPLETED:-unset}"
  exit 1
fi
BRANCH="$(git branch --show-current)"
COMMITS_AHEAD="$(git rev-list --count "${DEFAULT_BRANCH}..HEAD" 2>/dev/null || echo 0)"
if [ "${COMMITS_AHEAD}" -eq 0 ]; then
  echo "ERROR: merge_blocked_empty_diff — branch has 0 commits ahead of ${DEFAULT_BRANCH}."
  echo "Refusing to push an empty branch (#1122 structural guard)."
  exit 1
fi
DIFF_LINES="$(git diff "${DEFAULT_BRANCH}...HEAD" --shortstat 2>/dev/null | grep -oE '[0-9]+ (insertion|deletion)' | wc -l)"
if [ "${DIFF_LINES}" -eq 0 ]; then
  echo "ERROR: merge_blocked_empty_diff — cumulative diff vs ${DEFAULT_BRANCH} is empty."
  echo "Branch has commits but no actual changes — likely lefthook re-stage drift (#1122)."
  echo "Investigate the working tree and the failed commit's intended files; do NOT escalate to squash merge."
  exit 1
fi
CSA_SKIP_REVIEW_CHECK=1 \
CSA_SKIP_REVIEW_CHECK_REASON="dev2merge Step 13 push after Step 12 review verification" \
  git push -u origin "${BRANCH}" --force-with-lease
echo "CSA_VAR:PUSHED=true"
echo '<!-- CSA:NEXT_STEP cmd="verify review verdict (Step 14)" required=true -->'
```

## Step 14: Pre-PR Review Verdict Check
Tool: bash
OnFail: abort
Hard gate before PR creation: require REVIEW_COMPLETED, or fall back to an
exact-head verdict check for `${DEFAULT_BRANCH}...HEAD`.

```bash
set -euo pipefail
if [ "${REVIEW_COMPLETED:-}" = "true" ]; then
  echo "Pre-PR review already verified."
else
  csa review --check-verdict --range "${DEFAULT_BRANCH}...HEAD"
fi
echo "CSA_VAR:REVIEW_VERDICT_CHECKED=true"
echo '<!-- CSA:NEXT_STEP cmd="create or reuse PR (Step 15)" required=true -->'
```

## Step 15: Create or Reuse Pull Request
Tool: bash
OnFail: abort
Create or reuse a PR for the current branch. Outputs PR_NUMBER and PR_URL
as CSA_VARs for the next step. This step does NOT trigger pr-bot —
that is a separate hard gate in Step 16.

```bash
bash "${CSA_WORKFLOW_DIR:-patterns/dev2merge}/scripts/csa/create-or-reuse-pr.sh"
```

## Step 16: pr-bot Review & Merge Gate (HARD GATE)
Tool: bash
OnFail: abort
**MANDATORY**: This step MUST NOT be skipped. It runs pr-bot which performs
cloud review (if enabled) and the actual merge. Without this step completing
successfully, the PR remains unmerged and Step 17 will fail.

Uses marker files for idempotency: skips if pr-bot already completed for
the same PR/HEAD combination.

```bash
bash "${CSA_WORKFLOW_DIR:-patterns/dev2merge}/scripts/csa/pr-bot-review-merge.sh"
```

## Step 17: Post-Merge Local Sync
Tool: bash
OnFail: abort
Verify pr-bot completion marker exists (deterministic gate — cannot be bypassed
by LLM executor) AND that the PR was actually merged. Both checks must pass.

```bash
bash "${CSA_WORKFLOW_DIR:-patterns/dev2merge}/scripts/csa/post-merge-local-sync.sh"
```
