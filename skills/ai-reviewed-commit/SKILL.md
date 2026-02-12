---
name: ai-reviewed-commit
description: Enforces pre-commit code review using `csa review` with fix-and-retry loop until no issues found. Prevents committing code that fails review.
allowed-tools: Bash, Task, Read, Edit
---

# AI-Reviewed Commit Skill

## Purpose

Ensures all code is reviewed by `csa review --diff` before committing. Implements an automated fix-and-retry loop: review → fix issues → re-review → repeat until clean.

## Workflow Overview

```
┌─────────────────────────────────────────────────────┐
│  1. Stage changes (git add)                         │
└──────────────────────┬──────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────┐
│  2. Size check (git diff --stat --staged)           │
└──────────────────────┬──────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────┐
│  3. csa review --diff                                 │
└──────────────────────┬──────────────────────────────┘
                       │
           ┌───────────┴───────────┐
           │                       │
      Issues Found            No Issues
           │                       │
           ▼                       ▼
┌──────────────────────┐  ┌────────────────────┐
│  4. Dispatch sub-    │  │  5. Commit with    │
│     agent to fix     │  │     message        │
└──────────┬───────────┘  └────────────────────┘
           │
           │ (loop back to step 3)
           ▼
┌─────────────────────────────────────────────────────┐
│  Re-review after fix                                │
└─────────────────────────────────────────────────────┘
```

## Step-by-Step Procedure

### Step 1: Stage Changes

```bash
git add <files>
# or
git add -A  # stage all
```

### Step 2: Size Check (MANDATORY)

```bash
git diff --stat --staged
```

**Decision**:
- < 500 lines: Proceed with review
- >= 500 lines: Consider splitting into smaller commits

### Step 2.5: Authorship-Aware Review Strategy (MANDATORY)

Before running the review, determine who authored the staged code:

1. **You (the caller) wrote the code**: Use `csa debate` for review — an independent perspective catches blind spots you cannot see in your own output. Run `csa debate "Review my staged changes for correctness, security, and test gaps. Run 'git diff --staged' yourself to see the full patch."` instead of `csa review --diff`. Do NOT pass `--stat` output — the arbiter needs the full diff to evaluate properly.

2. **Another tool or human wrote the code**: Use `csa review --diff` — you already have an independent perspective.

**Detection**: Check the commit context. If you generated the code in this session → use `csa debate`. If you're committing code from another tool/human → use `csa review --diff`.

### Step 3: Run csa review

**MUST use `csa review --diff`** to review all uncommitted changes relative to HEAD (or `csa debate` if authorship check in Step 2.5 indicates you wrote the code):

```bash
csa review --diff
```

**Parameter Reference**:

| Flag | Purpose |
|------|---------|
| `--diff` | Review all uncommitted changes vs HEAD (`git diff HEAD`) |
| `--commit <sha>` | Review a specific commit |
| `--branch <name>` | Compare against a branch |
| `--tool <tool>` | Override review tool (default: from config) |
| `--session <id>` | Resume a previous review session |

**IMPORTANT**: Do NOT use `--branch` or `--commit` for pre-commit review. Those are for different use cases.

### Step 4: Handle Review Results

#### If Issues Found:

1. **Analyze the issues** from csa review output
2. **Dispatch sub-agent to fix**:

```
Task(balanced sub-agent or fast sub-agent)
  prompt: "Fix the following issues found in code review:
           [paste issues from csa review]

           Files to fix: [list files]

           Apply fixes and report what was changed."
```

3. **Re-stage fixed files**:
```bash
git add <fixed-files>
```

4. **Loop back to Step 3** - run csa review again

#### If No Issues Found:

Proceed to Step 5 (commit).

### Step 5: Commit

Generate commit message following Conventional Commits:

```bash
git commit -m "$(cat <<'EOF'
type(scope): short description

[MOTIVATION]
Why this change is needed.

[IMPLEMENTATION DETAILS]
What was done and how.

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"
```

**Delegate message generation** (if using Opus):
- Use `csa run "Run git diff --staged and generate a Conventional Commits message"`
- Or extract from csa review output if it provided suggestions

## Loop Control

### Maximum Iterations

**Default limit**: 3 review-fix cycles

If issues persist after 3 iterations:
1. Stop the loop
2. Report remaining issues to user
3. Ask user how to proceed

### Breaking Conditions

Exit the loop when:
- ✅ No issues found in review
- ⚠️ Max iterations reached
- ❌ User requests to stop
- ❌ Unfixable issue detected (requires human decision)

## Sub-Agent Selection for Fixes

| Issue Type | Recommended Agent | Reason |
|------------|-------------------|--------|
| Simple typos/formatting | Fast sub-agent | Fast, low cost |
| Logic/implementation bugs | Balanced sub-agent | Good balance |
| Security issues | Security-focused review + main agent | Critical, needs review |
| Architecture issues | Report to user | Requires human decision |

## AGENTS.md Compliance Check (MANDATORY)

In addition to standard review criteria and any context/prompt-specific review points:

1. The review agent MUST read `AGENTS.md` from repo root to each staged file directory (root-to-leaf).
2. Verify ALL staged code complies with every applicable rule from the combined `AGENTS.md` ruleset.
3. Report `AGENTS.md` violations as first-class review findings, and reference the exact rule ID in each violation.
4. If a violated rule is marked MUST/CRITICAL/FORBIDDEN, escalate to at least P1.
5. Hard gate: if AGENTS.md checklist is missing or incomplete, do NOT proceed to commit.

### Required AGENTS.md Checklist (mechanically verifiable)

The reviewer MUST produce and complete a checklist like:

```markdown
## AGENTS.md Checklist
- [ ] File path analyzed: <path>
- [ ] AGENTS chain discovered (root->leaf): <a/AGENTS.md>, <b/AGENTS.md>, <c/AGENTS.md>
- [ ] Rule checked: <rule-id> from <source AGENTS.md> -> PASS
- [ ] Rule checked: <rule-id> from <source AGENTS.md> -> PASS
- [ ] Rule checked: <rule-id> from <source AGENTS.md> -> VIOLATION (finding id: <id>)
```

Completion rule:
- Every applicable `AGENTS.md` rule for each staged file appears in checklist.
- Zero unchecked items allowed before commit.

## Anti-Patterns

### DO NOT:

```
❌ Skip size check before review
❌ Use compareBranch for uncommitted changes
❌ Commit without fixing review issues
❌ Fix issues without re-reviewing
❌ Infinite loop without max iterations
❌ Auto-fix security issues without human review
```

### DO:

```
✅ Always use csa review --diff for pre-commit
✅ Check size with git diff --stat first
✅ Dispatch sub-agent for fixes
✅ Re-review after every fix
✅ Set max iteration limit
✅ Escalate security issues to user
```

## Integration with Other Skills

### MCP Delegation Integration

If your project has an MCP delegation skill, this workflow integrates with it for auto-triggering pre-commit review.

### Sub-Agent Selection Strategy

Sub-agent selection strategy is described in the Sub-Agent Selection table above.

### Context Management

After completing the commit workflow, consider compacting context (`/compact`) to clean up review data.

## Example Session

```
User: "Let's commit these changes"

Claude:
1. git add -A
2. git diff --stat --staged
   # 3 files changed, 45 insertions(+), 12 deletions(-)

3. csa review --diff
   # Output: Found 2 issues:
   #   - Missing error handling in auth.ts:42
   #   - Unused import in utils.ts:3

4. Task(fast sub-agent):
   "Fix these issues:
    1. Add error handling in auth.ts:42
    2. Remove unused import in utils.ts:3"

   # Sub-agent fixes and reports

5. git add auth.ts utils.ts

6. csa review --diff  # RE-REVIEW
   # Output: No issues found

7. git commit -m "feat(auth): add token validation

   [MOTIVATION]
   Secure the API endpoints with proper token validation.

   [IMPLEMENTATION DETAILS]
   - Added JWT validation middleware
   - Clean up unused imports

   Co-Authored-By: Claude <noreply@anthropic.com>"

Claude: "Commit created successfully. All changes passed code review."
```

## Security Considerations

### Issues That Require User Approval

- Hardcoded secrets detected → STOP, ask user
- Security vulnerability found → STOP, ask user
- Removing security-related code → STOP, ask user

### Auto-Fixable Issues

- Formatting issues
- Unused imports
- Missing type annotations
- Simple null checks

## Output Format

```
🔍 Pre-Commit Review:

📊 Changes Summary:
   Files: 3
   Lines: +45, -12

🔄 Review Iteration: 1/3

📋 csa review Results:
   ❌ 2 issues found

   Issue 1: Missing error handling (auth.ts:42)
   Issue 2: Unused import (utils.ts:3)

🔧 Dispatching Fix:
   Agent: fast sub-agent
   Scope: 2 issues in 2 files

⏳ Fixing... Done.

🔄 Review Iteration: 2/3

📋 csa review Results:
   ✅ No issues found

✅ Ready to Commit:
   Message: feat(auth): add token validation

   Proceed with commit? (auto-proceeding in clean state)

🎉 Committed: abc1234
```

## ROI

**Quality Assurance**: Every commit is reviewed before merging to history.

**Automation**: Fix-and-retry loop reduces manual intervention.

**Token Efficiency**: Uses `csa review` (cheap) for review, fast/balanced sub-agents for fixes.
