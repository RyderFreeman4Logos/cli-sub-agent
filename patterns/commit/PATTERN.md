---
name = "commit"
description = "Strict commit discipline with Conventional Commits, mandatory security audit, test verification, and pre-commit review"
allowed-tools = "Bash, Read, Grep, Edit, Task, TaskCreate, TaskUpdate, TaskList, TaskGet"
tier = "tier-2-standard"
version = "0.1.0"
---

# Commit

Commit = Audited. Each commit passes security audit, test completeness
verification, code review with AGENTS.md compliance, and quality gates.

## Step 1: Branch Check

Tool: bash
OnFail: abort

Verify not on protected branch. Must be on feature branch.

```bash
default_branch=$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')
if [ -z "$default_branch" ]; then default_branch="main"; fi
branch=$(git branch --show-current)
if [ "$branch" = "$default_branch" ] || [ "$branch" = "dev" ]; then
  echo "ERROR: Cannot commit directly to $branch. Create a feature branch."
  exit 1
fi
```

## Step 2: Run Formatters

Tool: bash
OnFail: retry 2

```bash
just fmt
```

## Step 3: Run Linters

Tool: bash
OnFail: retry 2

```bash
just clippy
```

## Step 4: Run Tests

Tool: bash
OnFail: abort

```bash
just test
```

## Step 5: Stage Changes

Tool: bash
OnFail: abort

Stage all relevant files. Verify no untracked files remain.

```bash
git add ${FILES}
if git ls-files --others --exclude-standard | grep -q .; then
  echo "ERROR: Untracked files detected."
  git ls-files --others --exclude-standard
  exit 1
fi
```

## Step 6: Security Scan

Tool: bash
OnFail: abort

Check staged files for hardcoded secrets, debug statements.

```bash
git diff --cached --name-only | while read -r file; do
  if grep -nE '(API_KEY|SECRET|PASSWORD|PRIVATE_KEY)=' "$file" 2>/dev/null; then
    echo "FAIL: Potential secret in $file"
    exit 1
  fi
done
```

## Step 7: Security Audit

Tool: csa
Tier: tier-2-standard
OnFail: abort

## INCLUDE security-audit

Three-phase audit: test completeness, vulnerability scan, code quality.
Returns PASS, PASS_DEFERRED, or FAIL.

## IF ${AUDIT_FAIL}

## Step 7a: Fix Audit Issues

Fix blocking issues and re-run from Step 2.

## ENDIF

## IF ${AUDIT_PASS_DEFERRED}

## Step 7b: Record Deferred Issues

Record deferred issues (other modules) via TaskCreate for
immediate post-commit fixing.

## ENDIF

## Step 8: Pre-Commit Review

Tool: csa
Tier: tier-2-standard

## INCLUDE ai-reviewed-commit

Run csa review --diff (or csa debate if self-authored).
MUST include AGENTS.md compliance checklist.
Fix-and-retry loop (max 3 rounds).

## IF ${REVIEW_HAS_ISSUES}

## Step 9: Fix Review Issues

Tool: csa
Tier: tier-2-standard
OnFail: retry 3

Fix issues, re-run quality gates, re-review.

```bash
just pre-commit
```

## ENDIF

## Step 10: Generate Commit Message

Tool: csa
Tier: tier-1-quick

Delegate message generation to a lightweight tier. Commit messages are mechanical
(read diff → format) — no deep reasoning needed.

Tool and thinking budget are determined by the `tier-1-quick` config in
`~/.config/cli-sub-agent/config.toml`. Do NOT hardcode `--tool` or `--thinking`
flags — the tier system already specifies the right model.

**Session reuse** (PREFERRED): If a review session already ran in this workflow,
resume it with `--session <REVIEW_SESSION_ID>`. The model already "saw" the
changes, so generating a message costs near-zero new tokens. When resuming,
keep the same tool (sessions are tool-locked).

## Step 11: Commit

Tool: bash
OnFail: abort

```bash
git commit -m "${COMMIT_MSG}"
```

## IF ${IS_MILESTONE}

## Step 12: Auto PR

Tool: bash
OnFail: abort

Push and create PR when feature complete, bug fixed, or refactor done.
Steps 12-14 are ATOMIC — do not stop after PR creation.

```bash
git push -u origin "${BRANCH}"
gh pr create --base main --title "${COMMIT_MSG}" --body "${PR_BODY}"
```

## Step 13: Invoke PR Codex Bot

## INCLUDE pr-codex-bot

IMMEDIATELY invoke pr-codex-bot after PR creation.
Handles local review, cloud bot trigger, false-positive arbitration, merge.

## ENDIF

## IF ${HAS_DEFERRED_ISSUES}

## Step 14: Fix Deferred Issues

Fix deferred issues by priority (Critical > High > Medium).
Each fix goes through full commit workflow (Steps 1-11).

## ENDIF
