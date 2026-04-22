# Step 10.5: Rebase for Clean History (Detail)

> **Layer**: 0 (Orchestrator) -- git history cleanup before merge.

Tool: bash helper (`patterns/pr-bot/scripts/rebase-with-rollup.sh`)

## Intent

When a PR branch has accumulated multiple fix commits across review rounds, Step
10.5 rewrites the branch into a smaller set of logical commits before merge,
then forces one more bot review on the compressed history.

This path exists to catch regressions that only appear after the fix chain is
compacted. It must not destroy auditability, so the rebased commits preserve the
current commit's AI Reviewer Metadata block and append a rollup trailer for the
original commits they consolidate.

## Guard

Step 10.5 is eligible only when all of the following are true:

- `FIXES_ACCUMULATED=true`
- branch has 3+ commits since `${DEFAULT_BRANCH}`
- every original commit carries explicit Pass review metadata

If any original commit has `Fail` metadata, or if a commit lacks explicit pass
metadata, Step 10.5 logs a skip reason and the workflow falls back to the
existing clean-branch path (Step 11 / Step 12).

### Accepted Pass Metadata

Preferred explicit format:

```text
verdict=Pass
tool=codex
round=3
```

Legacy compatibility format accepted by the guard:

```text
Review: codex session 01ABC... (status=success, summary=PASS)
Review: gemini-cli session 01DEF... (status=success, summary=CLEAN)
```

`summary=FAIL` (or `verdict=Fail`) blocks compression. A commit with no explicit
pass marker is treated as non-eligible and causes a skip.

## Rollup Trailer Format

Each rebased logical commit keeps its normal `### AI Reviewer Metadata` block,
then appends:

```text
## AI Reviewer Metadata Rollup

This commit consolidates review history from:
- <short-sha> verdict=<Pass|Fail|Skip|Uncertain> tool=<tool> round=<n> (<subject>)
- ...
```

Format rules:

- heading is `## AI Reviewer Metadata Rollup`
- entries use 7-character short SHAs
- `verdict=` is mandatory in the rollup entry
- `tool=` is mandatory in the rollup entry (`unknown` if not detectable)
- `round=` is included only when detectable from the original commit subject/body
- subject is the original commit subject in parentheses for fast scanning

The rollup is additive. It does not replace the current logical commit's own AI
Reviewer Metadata block.

## Logical Grouping

After a soft reset to the merge-base, the helper rebuilds at most three logical
commits:

1. Source files: `src/`, `crates/`, `lib/`, `bin/`
2. Patterns / agent workflow files: `patterns/`, `.claude/`
3. Everything else: docs, config, tests, lockfiles, etc.

For each group:

1. Stage matching files.
2. Generate the current commit subject/body via `scripts/gen_commit_msg.sh`.
3. Append the rollup trailer for the original commits that touched that group.
4. Commit.

## Post-Rebase Review Gate

After the rewrite:

1. Create/update `backup-${PR_NUM}-pre-rebase`
2. `git push --force-with-lease "${REMOTE_NAME}" "${WORKFLOW_BRANCH}"`
3. Post an explicit `@${CLOUD_BOT_NAME}` retrigger comment for the new HEAD
4. Launch the bounded delegated post-rebase gate
5. Require the delegated session to emit `REBASE_GATE=PASS`
6. Re-check late actionable bot comments before allowing merge

On success:

- `REBASE_CLEAN_HISTORY_APPLIED=true`
- `REBASE_REVIEW_HAS_ISSUES=false`
- `FALLBACK_REVIEW_HAS_ISSUES=false`

On delegated gate failure or late actionable comments:

- `REBASE_REVIEW_HAS_ISSUES=true`
- `FALLBACK_REVIEW_HAS_ISSUES=true`
- workflow aborts

## Routing After Step 10.5

- If Step 10.5 applies successfully: skip Step 11 / Step 12 clean-branch
  resubmission and go straight to the direct merge path.
- If Step 10.5 skips: keep the old clean-branch path unchanged.
- If Step 10.5 fails after rewriting: abort and preserve the rewritten branch
  plus the `backup-${PR_NUM}-pre-rebase` backup branch for audit.
