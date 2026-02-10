---
name: code-review
description: "CRITICAL: Code review skill using GitHub API. Triggers on: review PR, code review, PR review, pull request review, review changes, check this PR, analyze this PR, review #"
allowed-tools: Bash, Read, Grep, Glob
---

# Code Review

> AI-powered code review using GitHub API

## Overview

This skill provides comprehensive code review capabilities by integrating with GitHub's API:
- Fetch and analyze pull requests
- Review code changes with AI assistance
- Generate review comments and suggestions
- Check for common issues and anti-patterns

## Prerequisites

- `gh` CLI must be installed and authenticated (required — all commands use `gh`)
- `GITHUB_TOKEN` environment variable should be set if `gh auth` is not configured

## Usage

### Review a Pull Request

When you ask me to review a PR, I will:

1. **Fetch PR details** using `gh pr view`
2. **Assess PR scale** to choose the right review strategy
3. **Analyze the diff** using `gh pr diff`
4. **Review each file** for:
   - Code quality issues
   - Security vulnerabilities
   - Performance concerns
   - Best practices violations
   - Documentation gaps
5. **Provide actionable feedback**

### Trigger Patterns

| Trigger | Example |
|---------|---------|
| `review PR #123` | Review a specific PR by number |
| `review this PR` | Review the current branch's PR |
| `code review` | Start code review workflow |
| `check PR changes` | Analyze PR changes |
| `review https://github.com/user/repo/pull/123` | Review from URL |

## Scale-Adaptive Strategy

Before reviewing, assess the PR size and choose the appropriate strategy:

```bash
# Check PR scale first
gh pr diff 123 --stat
```

| PR Scale | Lines Changed | Strategy |
|----------|---------------|----------|
| Small | < 200 lines | Direct review in main agent |
| Medium | 200–800 lines | Direct review, use `update_plan` for progress |
| Large | > 800 lines | Delegate to `csa review --branch <base-branch>` or `csa run` |

### Small PR: Direct Review

Read the diff directly and provide feedback. No delegation needed.

### Medium PR: Review with Progress Tracking

Use `update_plan` to show progress through multi-file reviews:

```
update_plan({
  "explanation": "Reviewing PR #123 (8 files changed)",
  "plan": [
    {"step": "Fetch PR metadata and diff", "status": "completed"},
    {"step": "Review src/auth.rs (critical: auth logic)", "status": "in_progress"},
    {"step": "Review src/api/handler.rs", "status": "pending"},
    {"step": "Review tests/", "status": "pending"},
    {"step": "Generate review summary", "status": "pending"}
  ]
})
```

Rules:
- At most **one** step can be `in_progress` at a time
- Transition: `pending` → `in_progress` → `completed` (never skip `in_progress`)
- Update the plan as each file review completes

### Large PR: Delegate to CSA

For large diffs (> 800 lines), delegate to avoid context bloat:

**IMPORTANT**: `csa review --branch` uses `git diff {branch}...HEAD`, so the PR branch
must be checked out locally first. For remote PRs, use `gh pr checkout` before delegating.

```bash
# Ensure PR branch is checked out locally
gh pr checkout <pr-number>

# Option 1: Use csa review with the PR's base branch (preferred)
csa review --branch $(gh pr view --json baseRefName -q .baseRefName)

# Option 2: Use csa review with a specific commit
csa review --commit <sha>

# Option 3: Use csa run (CSA routes to appropriate backend with large context)
csa run "Review the changes in this PR comprehensively"
```

**DO NOT** pre-read the diff into main agent context before delegating — CSA backends read files themselves.

**Reference**: See the `csa` skill for CSA delegation strategy.

## Commands Reference

### Fetch PR Information

```bash
# View PR details (with scale assessment)
gh pr view 123 --json title,body,author,files,additions,deletions,reviewDecision

# View PR diff
gh pr diff 123

# View PR diff stats (check scale first)
gh pr diff 123 --stat

# View PR files changed
gh api repos/{owner}/{repo}/pulls/123/files

# View PR comments
gh api repos/{owner}/{repo}/pulls/123/comments

# View PR reviews
gh api repos/{owner}/{repo}/pulls/123/reviews
```

### Submit Review Comments

**IMPORTANT**: By default, generate review output locally only. Do NOT post
comments or submit reviews to GitHub unless the user explicitly requests it.
Always confirm the target PR number and repository before any write operation.

```bash
# Add a comment to a PR (ONLY when user explicitly requests posting)
gh pr comment 123 --body "Review comment here"

# Create a review with comments (ONLY when user explicitly requests posting)
gh api repos/{owner}/{repo}/pulls/123/reviews \
  -f body="Review summary" \
  -f event="COMMENT"

# Approve a PR (ONLY when user explicitly requests approval)
gh pr review 123 --approve --body "LGTM"

# Request changes (ONLY when user explicitly requests it)
gh pr review 123 --request-changes --body "Please address the issues"
```

## Authorship-Aware Review Strategy

Before starting a review, determine who authored the code under review:

1. **You (the caller) wrote the code**: Use `csa debate` for review — an independent perspective catches blind spots you cannot see in your own output. You are biased toward your own code.

2. **Another tool or human wrote the code**: Review it yourself directly — you already have an independent perspective.

**Detection**: Check `git log` for commits in scope. If `Co-Authored-By` matches your model family → use `csa debate`. If commits are by a different tool/human → review directly.

## AGENTS.md Compliance Check

In addition to standard review criteria and any context/prompt-specific review points:

1. Read `AGENTS.md` at the repo/project root (if present).
2. Verify ALL changed code complies with every applicable rule listed in AGENTS.md.
3. Report violations as review findings referencing the specific AGENTS.md rule ID (e.g., "Violates rule 010 naming: variable `x` should be descriptive").
4. AGENTS.md violations are at least P2 (maintainability) — promote to P1 if the rule is marked MUST/CRITICAL.

## Review Workflow

### Step 1: Fetch PR Context

```bash
# Get PR metadata (including review status)
gh pr view 123 --json title,body,author,files,additions,deletions,reviewDecision

# Get diff stats to assess scale
gh pr diff 123 --stat

# Get full diff (only for small/medium PRs in main agent)
gh pr diff 123
```

### Step 2: Analyze Changes

For each file in the PR, I analyze:

**Code Quality:**
- Naming conventions
- Code organization
- DRY principle violations
- Complex logic that needs refactoring
- Unused code or imports

**Security:**
- Input validation
- SQL injection risks
- XSS vulnerabilities
- Hardcoded secrets
- Improper error handling

**Performance:**
- N+1 queries
- Unnecessary allocations
- Blocking operations in async code
- Missing indexes (for DB changes)

**Language-Specific Considerations:**
- Ownership issues (for Rust: E0382, E0507, etc.)
- Lifetime annotations
- Unsafe code usage
- Error handling with Result/Option
- Linter warnings

### Step 3: Generate Review

I provide:
1. **Summary** - Overall assessment
2. **Critical Issues** - Must-fix before merge
3. **Suggestions** - Recommended improvements
4. **Nitpicks** - Optional style improvements
5. **Questions** - Clarifications needed

## Review Templates

### Standard Review

```markdown
## Code Review Summary

**PR:** #123 - Title
**Author:** @username
**Files Changed:** N files (+X/-Y lines)

### Overall Assessment
[APPROVE / REQUEST_CHANGES / COMMENT]

Brief summary of the changes and overall quality.

### Critical Issues
- [ ] Issue 1: Description (file:line)
- [ ] Issue 2: Description (file:line)

### Suggestions
- Consider using X instead of Y in `file.rs:42`
- The function could be simplified by...

### Nitpicks
- Style: Prefer `foo` over `bar` per project conventions
- Typo in comment at line 15

### Questions
- What is the expected behavior when...?
- Should this be documented in the README?

### Files Reviewed
- [x] src/main.rs
- [x] src/lib.rs
- [x] tests/test.rs (no concerns)
```

### Security-Focused Review

```markdown
## Security Review

**PR:** #123
**Risk Level:** [LOW / MEDIUM / HIGH]

### Security Checklist
- [ ] Input validation on all user inputs
- [ ] No hardcoded credentials
- [ ] Proper error handling (no stack traces exposed)
- [ ] SQL queries use parameterized statements
- [ ] File paths are validated
- [ ] Authentication/authorization checks in place

### Findings
| Severity | Issue | Location | Recommendation |
|----------|-------|----------|----------------|
| HIGH | ... | file:line | ... |
| MEDIUM | ... | file:line | ... |

### Recommendations
1. ...
2. ...
```

## Example Usage

### Review Current PR

```
User: Review the current PR

Claude: Let me fetch the PR details and review the changes.

[Executes: gh pr view --json number,title,body,files,additions,deletions]
[Executes: gh pr diff --stat]
[Assesses scale → chooses strategy]
[Executes: gh pr diff]

Based on my analysis...
```

### Review Specific PR

```
User: Review PR #42 in user/repo

Claude: I'll review PR #42.

[Executes: gh pr view 42 -R user/repo --json ...]
[Executes: gh pr diff 42 -R user/repo --stat]
[Assesses scale → delegates if large]

Here's my review...
```

### Quick Check

```
User: Quick check the PR for security issues

Claude: I'll perform a security-focused review.

[Analyzes for security patterns only]

Security assessment...
```

## Integration with Other Skills

| Skill | Integration |
|-------|-------------|
| `csa-review` | CSA-driven code review with session isolation and backend routing |
| `csa-analyze` | Delegate large diff analysis via CSA to appropriate backend |
| `security-audit` | Pre-commit security audit with test completeness verification |
| `ai-reviewed-commit` | Enforces review before commit with fix-and-retry loop |
| `debate` | Use for adversarial review when you authored the code under review |

**Reference**: See the `csa` skill for CSA delegation strategy.

## Best Practices

### For Reviewers

1. **Check scale first** - Run `gh pr diff --stat` before reading the full diff
2. **Be specific** - Reference exact file:line locations
3. **Be constructive** - Suggest solutions, not just problems
4. **Prioritize** - Mark critical vs. nice-to-have
5. **Track progress** - Use `update_plan` for multi-file reviews
6. **Be timely** - Review PRs promptly

### For Authors

1. **Small PRs** - Easier to review thoroughly
2. **Good descriptions** - Explain the why, not just what
3. **Self-review first** - Check your own code before requesting
4. **Address all comments** - Don't leave feedback unresolved
