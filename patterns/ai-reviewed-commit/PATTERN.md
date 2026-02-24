---
name = "ai-reviewed-commit"
description = "Pre-commit code review loop: stage → size check → csa review → fix → re-review → commit"
allowed-tools = "Bash, Task, Read, Edit"
tier = "tier-2-standard"
version = "0.1.0"
---

# AI-Reviewed Commit

Ensures all code is reviewed by csa review before committing.
Automated fix-and-retry loop: review → fix → re-review → repeat until clean.
Maximum 3 iterations.

## Step 1: Stage Changes

Tool: bash

```bash
git add ${FILES}
```

## Step 2: Size Check

Tool: bash
OnFail: abort

Check staged diff size. If >= 500 lines, consider splitting.

```bash
git diff --stat --staged
```

## Step 3: Authorship-Aware Review Strategy

Determine who authored the staged code:
- Self-authored (generated in this session) → use csa debate
- Other tool/human authored → use csa review --diff --allow-fallback

## IF ${SELF_AUTHORED}

## Step 4a: Run Debate Review

Tool: bash

```bash
csa debate "Review my staged changes for correctness, security, and test gaps. Run 'git diff --staged' yourself to see the full patch."
```

## ELSE

## Step 4b: Run CSA Review

Tool: bash

```bash
csa review --diff --allow-fallback
```

## ENDIF

## IF ${REVIEW_HAS_ISSUES}

## Step 5: Dispatch Fix Sub-Agent

Tool: claude-code
Tier: tier-2-standard
OnFail: retry 3

Dispatch sub-agent to fix issues found in review.
Preserve original code intent. Do NOT delete code to silence warnings.

## Step 6: Re-stage Fixed Files

Tool: bash

```bash
git add ${FIXED_FILES}
```

## Step 7: Re-review

Tool: bash

Loop back to review. Maximum 3 review-fix cycles.

```bash
csa review --diff --allow-fallback
```

## ENDIF

## Step 8: AGENTS.md Compliance Check

The review MUST include AGENTS.md compliance checklist:
- Discover AGENTS.md chain (root-to-leaf) for each staged file
- Check every applicable rule
- If staged diff touches `PATTERN.md` or `workflow.toml`, MUST check rule 027 `pattern-workflow-sync`
- If staged diff touches process spawning/lifecycle code, MUST check Rust rule 015 `subprocess-lifecycle`
- Zero unchecked items before proceeding to commit

## Step 9: Generate Commit Message

Tool: csa
Tier: tier-1-quick

Delegate commit message generation to cheaper tool.

```bash
csa run "Run 'git diff --staged' and generate a Conventional Commits message"
```

## Step 10: Commit

Tool: bash
OnFail: abort

```bash
git commit -m "${COMMIT_MSG}"
```
