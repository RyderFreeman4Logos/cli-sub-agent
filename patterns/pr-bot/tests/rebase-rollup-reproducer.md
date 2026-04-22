# Rebase Rollup Reproducer

Manual reproducer for Step 10.5 (`patterns/pr-bot/scripts/rebase-with-rollup.sh`).

## Goal

Simulate a branch with 5 commits, all carrying explicit pass metadata, then run
the Step 10.5 helper and verify that the rebased logical commits contain the
rollup trailer.

## Setup

1. Start from a disposable branch created off `main`.
2. Make 5 small commits touching at least two logical groups:
   - one or more files under `src/` or `crates/`
   - one or more files under `patterns/`
   - one file outside those groups (for example a `.md` doc)
3. Ensure each commit body includes explicit pass metadata. Minimal accepted
   example:

```text
Test commit summary

### AI Reviewer Metadata
- **Design Intent**: Reproducer only.
- **Key Decisions**: Reproducer only.
- **Reviewer Guidance**:
  - **Timing/Race Scenarios**: none
  - **Boundary Cases**: none
  - **Regression Tests Added**: none

Review: codex session 01TESTSESSION0000000000000000 (status=success, summary=PASS)
tool=codex
verdict=Pass
round=1
```

## Run

Export the variables pr-bot Step 10.5 expects, then invoke the helper:

```bash
export DEFAULT_BRANCH=main
export PR_NUM=999999
export REPO=owner/repo
export REMOTE_NAME=origin
export WORKFLOW_BRANCH="$(git branch --show-current)"
export CLOUD_BOT_NAME=gemini-code-assist
export CLOUD_BOT_LOGIN=gemini-code-assist[bot]
export CLOUD_BOT_RETRIGGER_CMD='/gemini review'
export CLOUD_BOT_WAIT_SECONDS=60
export CLOUD_BOT_POLL_MAX_SECONDS=240
export POST_REBASE_TIMEOUT=1800

bash patterns/pr-bot/scripts/rebase-with-rollup.sh
```

If you do not want to hit GitHub, stop after inspecting the rewritten local
commits and before the helper reaches the `gh pr comment` / delegated gate
portion.

## Verify

1. `git log --format='%h %s%n%b' -n 3` shows rebased logical commits.
2. Each rebased commit still contains the normal `### AI Reviewer Metadata`
   block.
3. Each rebased commit also contains:

```text
## AI Reviewer Metadata Rollup

This commit consolidates review history from:
- <sha> verdict=Pass tool=codex round=<n> (<subject>)
```

4. `git branch --list "backup-${PR_NUM}-pre-rebase"` shows the backup branch.
5. The force-push command uses `--force-with-lease`, never plain `--force`.
