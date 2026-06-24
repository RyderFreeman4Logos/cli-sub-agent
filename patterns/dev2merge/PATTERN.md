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
mktsk N*(implement → commit) → Self-Review Gate → Pre-PR Cumulative Review → Push →
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
csa plan run patterns/dev2merge/workflow.toml --var DEV2MERGE_MODE=resume
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

## Step 3: L1/L2 Quality Gates (Always Run)
Tool: bash
OnFail: abort
Formatters and linters run regardless of FAST_PATH.
Language detection: Cargo.toml → Rust, pyproject.toml → Python, package.json → JS/TS, go.mod → Go.
Falls back to `just pre-commit` when available, skip otherwise.

```bash
set -euo pipefail
if [ -f Cargo.toml ]; then
  just fmt
  just clippy
elif [ -f pyproject.toml ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "lint"; then just lint
  elif command -v ruff >/dev/null 2>&1; then ruff check .; ruff format --check .;
  else echo "WARNING: Python project detected but no linter (just lint or ruff) found."; fi
elif [ -f package.json ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "lint"; then just lint
  elif command -v biome >/dev/null 2>&1; then biome check .;
  else echo "WARNING: JS/TS project detected but no linter (just lint or biome) found."; fi
elif [ -f go.mod ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "lint"; then just lint
  else
    go vet ./...
    if command -v golangci-lint >/dev/null 2>&1; then golangci-lint run; fi
  fi
elif just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "pre-commit"; then
  just pre-commit
else
  echo "WARNING: No recognized project type; skipping L1 lint gate."
fi
```

## IF ${FAST_PATH}

## Step 4: FAST_PATH Commit
Tool: bash
OnFail: abort
For docs/config-only changes, run a simplified commit flow:
stage, generate message, commit. No mktd/mktsk/security-audit overhead.

```bash
set -euo pipefail
if [ -f Cargo.toml ]; then just test
elif [ -f pyproject.toml ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then just test
  elif command -v pytest >/dev/null 2>&1; then pytest; fi
elif [ -f package.json ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then just test
  elif command -v vitest >/dev/null 2>&1; then vitest run; fi
elif [ -f go.mod ]; then go test ./...
elif just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then just test
else echo "WARNING: No recognized test runner; skipping L2 test gate."; fi
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
Even FAST_PATH runs cumulative review before push.

```bash
set -euo pipefail
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
set -euo pipefail
CURRENT_BRANCH="$(git branch --show-current)"
FEATURE_INPUT="${FEATURE_INPUT:-${SCOPE:-current branch changes pending merge}}"
USER_LANGUAGE_OVERRIDE="${CSA_USER_LANGUAGE:-}"
MKTD_TOOL_EFFECTIVE="${MKTD_TOOL:-${CSA_MKTD_TOOL:-}}"
MKTD_TIMEOUT_SECONDS="${MKTD_TIMEOUT_SECONDS:-1800}"
MKTD_PLAN_TIER="${PLAN_TIER:-${TIER:-tier-3-complex}}"
CSA_BIN="${CSA_BIN:-csa}"
MKTD=(--pattern mktd); [ -n "${MKTD_WORKFLOW_PATH:-}" ] && MKTD=("$MKTD_WORKFLOW_PATH")
MKTD_TOOL_ARGS=(); [ -z "${MKTD_TOOL_EFFECTIVE}" ] || MKTD_TOOL_ARGS=(--tool "${MKTD_TOOL_EFFECTIVE}")
LIGHT_THRESHOLD_FILES="${PLANNING_LIGHT_THRESHOLD_FILES:-2}"
LIGHT_THRESHOLD_LINES="${PLANNING_LIGHT_THRESHOLD_LINES:-50}"
PLAN_CODE_FILES="$(git diff --name-only "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | grep -cvE '\.(md|txt|lock|toml)$' || true)"
PLAN_INSERTIONS="$(git diff --stat "${DEFAULT_BRANCH}...HEAD" 2>/dev/null | tail -1 | grep -oE '[0-9]+ insertion' | grep -oE '[0-9]+' || echo 0)"
FEATURE_INPUT_LEN=${#FEATURE_INPUT}
FEATURE_FILE_LINE_HITS="$(printf '%s' "${FEATURE_INPUT}" | grep -oE '[A-Za-z0-9_./-]+\.(rs|toml|md):[0-9]+' | wc -l | xargs || true)"
if [ "${FEATURE_INPUT_LEN}" -lt 4096 ] && [ "${FEATURE_FILE_LINE_HITS}" -ge 2 ]; then
  MKTD_INTENSITY="light"
  echo "Planning intensity: light (brief specificity: ${FEATURE_FILE_LINE_HITS} file:line refs in ${FEATURE_INPUT_LEN}-char brief)"
elif [ "${PLAN_CODE_FILES}" -le "${LIGHT_THRESHOLD_FILES}" ] && [ "${PLAN_INSERTIONS:-0}" -lt "${LIGHT_THRESHOLD_LINES}" ]; then
  MKTD_INTENSITY="light"
  echo "Planning intensity: light (${PLAN_CODE_FILES} code files, ${PLAN_INSERTIONS} insertions)"
else
  MKTD_INTENSITY="full"
  echo "Planning intensity: full (${PLAN_CODE_FILES} code files, ${PLAN_INSERTIONS} insertions)"
fi
echo "mktd timeout: ${MKTD_TIMEOUT_SECONDS}s"
set +e
MKTD_OUTPUT="$(timeout -k 30 "${MKTD_TIMEOUT_SECONDS}" "${CSA_BIN}" plan run --sa-mode true "${MKTD[@]}" \
  "${MKTD_TOOL_ARGS[@]}" \
  --var CWD="$(pwd)" \
  --var FEATURE="Plan dev2merge for branch ${CURRENT_BRANCH}. Scope: ${FEATURE_INPUT}." \
  --var USER_LANGUAGE="${USER_LANGUAGE_OVERRIDE}" \
  --var TIER="${TIER:-}" \
  --var PLAN_TIER="${MKTD_PLAN_TIER}" \
  --var IMPL_TIER="${IMPL_TIER:-}" \
  --var IMPL_TOOL="${IMPL_TOOL:-}" \
  --var INTENSITY="${MKTD_INTENSITY}" 2>&1)"
MKTD_EXIT=$?
set -e
printf '%s\n' "${MKTD_OUTPUT}"
print_mktd_failure_context() {
  echo "mktd exit code: ${MKTD_EXIT}" >&2
  echo "mktd failure context (step/exit lines):" >&2
  MKTD_FAILURE_CONTEXT="$(printf '%s\n' "${MKTD_OUTPUT}" | grep -Ei '(error|Step [0-9]+|timeout|fail)' | tail -40 || true)"
  if [ -n "${MKTD_FAILURE_CONTEXT}" ]; then
    printf '%s\n' "${MKTD_FAILURE_CONTEXT}" >&2
  else
    printf '%s\n' "${MKTD_OUTPUT}" | tail -80 >&2
  fi
}
fail_step7_gate() {
  echo "ERROR: ${1}" >&2
  if [ "${MKTD_EXIT}" -ne 0 ]; then
    print_mktd_failure_context
  else
    echo "mktd exit code: 0" >&2
  fi
  exit 1
}
if [ "${MKTD_EXIT}" -eq 124 ] || [ "${MKTD_EXIT}" -eq 137 ]; then
  echo "ERROR: mktd hard-timeout after ${MKTD_TIMEOUT_SECONDS}s (#1118 part A)." >&2
  print_mktd_failure_context
  exit 1
fi
LATEST_TS="$("${CSA_BIN}" todo list --format json | jq -r --arg br "${CURRENT_BRANCH}" '[.[] | select(.branch == $br)] | sort_by(.timestamp) | last | .timestamp // empty')"
if [ -z "${LATEST_TS}" ]; then
  fail_step7_gate "mktd did not produce a TODO for branch ${CURRENT_BRANCH}."
fi
TODO_PATH="$("${CSA_BIN}" todo show -t "${LATEST_TS}" --path)"
if ! grep -qF -- '- [ ] ' "${TODO_PATH}"; then
  fail_step7_gate "TODO missing checkbox tasks."
fi
if ! awk '
function flush() { if (in_open == 1 && has_clause == 0) bad = 1; in_open = 0; has_clause = 0 }
function scan(text,   pos, rest) {
  pos = index(text, "DONE WHEN:")
  if (pos > 0) { rest = substr(text, pos + 10); sub(/^[ \t]+/, "", rest); if (length(rest) > 0) has_clause = 1 }
}
{
  is_open = ($0 ~ /^- \[ \]/)
  if ($0 ~ /^- \[[ xX]\]/ || $0 ~ /^#/) { flush(); if (is_open == 1) { in_open = 1; scan($0) }; next }
  if (in_open == 1) scan($0)
}
END { flush(); exit (bad + 0) }
' "${TODO_PATH}"; then
  fail_step7_gate "TODO has an open task without a mechanically-verifiable DONE WHEN: clause."
fi
if [ "${MKTD_EXIT}" -ne 0 ]; then
  echo "WARNING: mktd exited ${MKTD_EXIT}, but TODO gates passed; treating Step 7 as successful." >&2
  print_mktd_failure_context
fi
echo "CSA_VAR:MKTD_TODO_TIMESTAMP=${LATEST_TS}"
echo "CSA_VAR:MKTD_TODO_PATH=${TODO_PATH}"
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
Resume mode skips mktd/mktsk because the work is already on the branch. Run the
L2 test gate, then commit any uncommitted remainder so review/push see the full
diff. Mirrors the FAST_PATH commit (Step 4); fails closed when there is no work.

```bash
set -euo pipefail
if [ -f Cargo.toml ]; then just test
elif [ -f pyproject.toml ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then just test
  elif command -v pytest >/dev/null 2>&1; then pytest; fi
elif [ -f package.json ]; then
  if just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then just test
  elif command -v vitest >/dev/null 2>&1; then vitest run; fi
elif [ -f go.mod ]; then go test ./...
elif just --summary 2>/dev/null | tr ' ' '\n' | grep -qx "test"; then just test
else echo "WARNING: No recognized test runner; skipping L2 test gate."; fi
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

Tool: manual (main agent action)
OnFail: abort
Before triggering `csa review`, the implementing agent MUST self-check the
entire branch diff and fix any issues it finds.

Required actions:
1. Run the project's lint command (Rust: `just clippy`, Python: `just lint` or `ruff check`, Go: `go vet`, JS/TS: `just lint` or `biome check`) and fix every warning.
2. Run the project's test command (Rust: `just test`, Python: `pytest`, Go: `go test ./...`, JS/TS: `vitest run`) and fix every failure.
3. If `.csa/review-checklist.md` exists, inspect `git diff "${DEFAULT_BRANCH}...HEAD"` against that checklist for known anti-patterns.
4. Fix any issues found during this self-review.
5. Only after completing all checks and fixes, continue to the cumulative `csa review` step.

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
set -euo pipefail
BRANCH="$(git branch --show-current)"
COMMIT_SUBJECT="$(git log -1 --format=%s)"

# --- Create or reuse PR ---
set +e
CREATE_OUTPUT="$(gh pr create --base "${DEFAULT_BRANCH}" --title "${COMMIT_SUBJECT}" --body "Auto-created by dev2merge pipeline." 2>&1)"
CREATE_RC=$?
set -e
if [ "${CREATE_RC}" -ne 0 ]; then
  if ! printf '%s\n' "${CREATE_OUTPUT}" | grep -Eiq 'already exists|a pull request already exists'; then
    echo "ERROR: gh pr create failed: ${CREATE_OUTPUT}" >&2
    exit 1
  fi
  echo "INFO: PR already exists for ${BRANCH}; continuing."
fi

# --- Resolve PR number (retry + fallback) ---
PR_NUMBER=""
PR_URL=""
for attempt in 1 2 3; do
  PR_JSON="$(gh pr view --json number,url,state -q 'select(.state == "OPEN")' 2>/dev/null || true)"
  PR_NUMBER="$(printf '%s' "${PR_JSON}" | jq -r '.number // empty' 2>/dev/null || true)"
  if [ -n "${PR_NUMBER}" ] && printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
    PR_URL="$(printf '%s' "${PR_JSON}" | jq -r '.url // empty')"
    break
  fi
  PR_NUMBER=""
  if [ "${attempt}" -lt 3 ]; then
    echo "INFO: PR resolution attempt ${attempt} failed, retrying in 5s..."
    sleep 5
  fi
done
# Fallback: gh pr list --head
if [ -z "${PR_NUMBER}" ]; then
  echo "INFO: gh pr view failed; falling back to gh pr list..."
  PR_NUMBER="$(gh pr list --head "${BRANCH}" --base "${DEFAULT_BRANCH}" --json number -q '.[0].number' 2>/dev/null || true)"
  if [ -n "${PR_NUMBER}" ] && printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
    PR_URL="$(gh pr view "${PR_NUMBER}" --json url -q '.url' 2>/dev/null || true)"
  fi
fi
if [ -z "${PR_NUMBER}" ] || ! printf '%s' "${PR_NUMBER}" | grep -Eq '^[0-9]+$'; then
  echo "ERROR: Cannot resolve open PR number for ${BRANCH} after retries." >&2
  exit 1
fi
echo "PR #${PR_NUMBER} resolved: ${PR_URL}"
echo "CSA_VAR:PR_NUMBER=${PR_NUMBER}"
echo "CSA_VAR:PR_URL=${PR_URL}"
echo '<!-- CSA:NEXT_STEP cmd="csa plan run --sa-mode true patterns/pr-bot/workflow.toml (Step 16)" required=true -->'
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
set -euo pipefail
if [ -z "${PR_NUMBER:-}" ]; then
  echo "ERROR: PR_NUMBER not set — Step 15 must run first." >&2
  exit 1
fi
HEAD_SHA="$(git rev-parse --verify HEAD)"

# --- Lock + Idempotency: skip if pr-bot already ran or is running ---
# Bind markers to repo identity to prevent cross-repo PR# collisions.
REPO_SLUG="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')" || true
if [ -z "${REPO_SLUG}" ]; then
  REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
fi
MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"
mkdir -p "${MARKER_DIR}"
MARKER_BASE="${MARKER_DIR}/${PR_NUMBER}-${HEAD_SHA}"
DONE_MARKER="${MARKER_BASE}.done"
LOCK_DIR="${MARKER_BASE}.lock"
LOCK_HELD=0

cleanup_lock() {
  if [ "${LOCK_HELD}" -eq 1 ]; then
    rmdir "${LOCK_DIR}" 2>/dev/null || true
  fi
}
trap cleanup_lock EXIT

if [ -f "${DONE_MARKER}" ]; then
  echo "pr-bot already completed for PR #${PR_NUMBER} at HEAD ${HEAD_SHA:0:11}; skipping."
  echo "CSA_VAR:PR_BOT_DONE_MARKER=${DONE_MARKER}"
  echo '<!-- CSA:NEXT_STEP cmd="post-merge local sync (Step 17)" required=true -->'
elif ! mkdir "${LOCK_DIR}" 2>/dev/null; then
  echo "ERROR: pr-bot already running for PR #${PR_NUMBER} at HEAD ${HEAD_SHA:0:11}." >&2
  echo "Wait for the other run to finish, or remove the lock: ${LOCK_DIR}" >&2
  exit 1
else
  LOCK_HELD=1
  echo "Running pr-bot for PR #${PR_NUMBER} (${PR_URL:-unknown})..."
  export CSA_PR_BOT_GUARD=1
  if csa plan run --sa-mode true patterns/pr-bot/workflow.toml; then
    touch "${DONE_MARKER}"
    echo "CSA_VAR:PR_BOT_DONE_MARKER=${DONE_MARKER}"
    echo '<!-- CSA:NEXT_STEP cmd="post-merge local sync (Step 17)" required=true -->'
    LOCK_HELD=0
    rmdir "${LOCK_DIR}" 2>/dev/null || true
  else
    echo "ERROR: pr-bot workflow failed for PR #${PR_NUMBER}." >&2
    exit 1
  fi
fi
```

## Step 17: Post-Merge Local Sync
Tool: bash
OnFail: abort
Verify pr-bot completion marker exists (deterministic gate — cannot be bypassed
by LLM executor) AND that the PR was actually merged. Both checks must pass.

```bash
set -euo pipefail
# NOTE: PR_NUMBER comes from Step 15 (gh pr view/list). In fork workflows,
# pr-bot may resolve a different PR via owner-aware lookup. For single-repo
# workflows (the common case), both resolve to the same PR.
if [ -n "${PR_NUMBER:-}" ]; then
  # --- Deterministic gate: verify pr-bot completion marker ---
  # Prefer exact marker path from Step 16 (CSA_VAR:PR_BOT_DONE_MARKER).
  # Fall back to repo-scoped glob if variable is unset (backwards compat).
  if [ -n "${PR_BOT_DONE_MARKER:-}" ]; then
    if [ ! -f "${PR_BOT_DONE_MARKER}" ]; then
      echo "ERROR: pr-bot marker not found: ${PR_BOT_DONE_MARKER}" >&2
      echo "Step 16 (pr-bot) must complete successfully before post-merge sync." >&2
      exit 1
    fi
    echo "pr-bot completion marker verified (exact): ${PR_BOT_DONE_MARKER}"
  else
    # Fallback: glob match by repo slug + PR number.
    # NOTE: glob may match stale markers from previous pr-bot runs on the same
    # PR. The exact CSA_VAR path (above) is the primary defense; this fallback
    # exists only for edge cases where the variable is lost.
    REPO_SLUG="$(gh repo view --json nameWithOwner -q '.nameWithOwner' 2>/dev/null | tr '/' '_')" || true
    if [ -z "${REPO_SLUG}" ]; then
      REPO_SLUG="$(git remote get-url origin 2>/dev/null | sed -E 's#^(https?://[^/]+/|ssh://[^/]+/|[^:]+:)##; s/\.git$//' | tr '/' '_')"
    fi
    MARKER_DIR="${HOME}/.local/state/cli-sub-agent/pr-bot-markers/${REPO_SLUG}"
    if ! ls "${MARKER_DIR}/${PR_NUMBER}"-*.done 1>/dev/null 2>&1; then
      echo "ERROR: No pr-bot completion marker found for PR #${PR_NUMBER}." >&2
      echo "Step 16 (pr-bot) must complete successfully before post-merge sync." >&2
      echo "Marker directory: ${MARKER_DIR}" >&2
      exit 1
    fi
    echo "pr-bot completion marker verified (glob) for PR #${PR_NUMBER}."
  fi

  # --- Verify PR is actually merged (defense in depth) ---
  PR_STATE="$(gh pr view "${PR_NUMBER}" --json state -q '.state' 2>/dev/null || echo "UNKNOWN")"
  if [ "${PR_STATE}" != "MERGED" ]; then
    echo "ERROR: PR #${PR_NUMBER} state is '${PR_STATE}', expected 'MERGED'." >&2
    echo "pr-bot marker exists but PR not merged — possible partial failure." >&2
    exit 1
  fi
  echo "PR #${PR_NUMBER} confirmed MERGED."
fi
FEATURE_BRANCH="$(git branch --show-current 2>/dev/null || true)"
SYNC_REMOTE="origin"
SYNC_DEFAULT_BRANCH="${DEFAULT_BRANCH:-}"
if [ -z "${SYNC_DEFAULT_BRANCH}" ]; then
  SYNC_DEFAULT_BRANCH="$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')"
fi
if [ -z "${SYNC_DEFAULT_BRANCH}" ]; then
  echo "WARNING: post-merge checkout skipped: could not determine default branch." >&2
  exit 0
fi
if ! git checkout "${SYNC_DEFAULT_BRANCH}"; then
  echo "WARNING: post-merge checkout of ${SYNC_DEFAULT_BRANCH} failed; leaving ${FEATURE_BRANCH:-current branch} checked out." >&2
  exit 0
fi
if ! git pull --ff-only "${SYNC_REMOTE}" "${SYNC_DEFAULT_BRANCH}"; then
  echo "WARNING: post-merge pull of ${SYNC_REMOTE}/${SYNC_DEFAULT_BRANCH} failed; merge already completed." >&2
  exit 0
fi
LOCAL_SHA="$(git rev-parse HEAD)"
REMOTE_SHA="$(git rev-parse "${SYNC_REMOTE}/${SYNC_DEFAULT_BRANCH}" 2>/dev/null || true)"
if [ -n "${REMOTE_SHA}" ] && [ "${LOCAL_SHA}" != "${REMOTE_SHA}" ]; then
  echo "WARNING: Local ${SYNC_DEFAULT_BRANCH} (${LOCAL_SHA}) does not match ${SYNC_REMOTE}/${SYNC_DEFAULT_BRANCH} (${REMOTE_SHA}) after sync." >&2
  exit 0
fi
echo "Local ${SYNC_DEFAULT_BRANCH} synced to ${LOCAL_SHA}."
```
