---
name: commit
description: Enforces strict commit discipline following Conventional Commits with mandatory pre-commit security audit and test completeness verification
allowed-tools: Bash, Read, Grep, Edit, Task, TaskCreate, TaskUpdate, TaskList, TaskGet
---

# Commit Skill

## Purpose

**Enforces strict commit discipline**, ensuring each commit follows Conventional Commits standard, passes **security audit** and **test completeness verification**, and passes all quality checks.

**Core philosophy**: **Commit = Audited**

**Core value**:
- ✅ Conventional Commits format (English)
- ✅ Atomic commits (one logical unit per commit)
- ✅ **Mandatory security audit (the `security-audit` skill)**
- ✅ **Test completeness verification ("can't write more tests" standard)**
- ✅ Pre-commit code review (csa review)
- ✅ Security checks (no secrets, no debug code)
- ✅ Quality gates (formatters, linters, tests)

## When to Use

**Use this skill immediately** when:
- ✅ Completed a logical unit (feature, fix, refactor)
- ✅ All code is written and ready to commit
- ✅ Need to generate compliant commit message

**Don't use this skill** when:
- ❌ Code is still being written
- ❌ Tests are not yet passing
- ❌ Temporary WIP commit

## Conventional Commits Format (MANDATORY)

### Basic Format

```
type(scope): short description

[MOTIVATION]
Why this change is needed.

[IMPLEMENTATION DETAILS]
What was done and how.
```

### Commit Types

| Type | Purpose | Example |
|------|---------|---------|
| `feat` | New feature | `feat(auth): implement JWT validation` |
| `fix` | Bug fix | `fix(parser): handle null input gracefully` |
| `refactor` | Refactoring (no behavior change) | `refactor(api): extract validation logic` |
| `test` | Test-related | `test(auth): add JWT validation tests` |
| `docs` | Documentation | `docs(readme): update installation guide` |
| `style` | Formatting (no logic change) | `style(api): fix indentation` |
| `perf` | Performance optimization | `perf(db): add query caching` |
| `chore` | Build, dependencies, config | `chore(deps): update tokio to 1.35` |

### Language Requirement

**⚠️ All commit messages MUST be in English.**

- ✅ Code, comments, commit messages: English
- ❌ Non-English commit messages are NOT allowed

### Examples

**Minimal (simple changes):**
```
fix(auth): handle expired token gracefully
```

**With body (significant changes):**
```
feat(parser): add support for nested expressions

[MOTIVATION]
Users requested ability to write complex expressions like `a * (b + c)`.
Current parser only handles flat expressions.

[IMPLEMENTATION DETAILS]
- Added recursive descent for parenthesized expressions
- Updated AST node to support nesting
- Added 15 test cases for edge cases
```

## Pre-Commit Workflow (MANDATORY)

### Step 0: Branch Check (MUST DO FIRST)

**Before ANY commit, verify you are NOT on the default/protected branch:**

```bash
# Detect the default branch (try origin/HEAD, fall back to main/master)
default_branch=$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')
if [ -z "$default_branch" ]; then
  if git show-ref --verify --quiet refs/heads/main 2>/dev/null; then
    default_branch="main"
  elif git show-ref --verify --quiet refs/heads/master 2>/dev/null; then
    default_branch="master"
  else
    # Could not detect default branch — override in CLAUDE.md if needed
    default_branch="main"
  fi
fi

branch=$(git branch --show-current)
if [ "$branch" = "$default_branch" ]; then
  echo "ERROR: Cannot commit directly to $default_branch. Create a feature branch first."
  echo "  git checkout -b <type>/<description>"
  exit 1
fi
```

Per your project's git workflow (as defined in CLAUDE.md):
- **NEVER** commit directly to the default/protected branch
- **MUST** be on a feature branch: `feat/`, `fix/`, `refactor/`, `chore/`, `docs/`
- If on a protected branch, create a feature branch first: `git checkout -b <type>/<description>`
- If your project protects additional branches (`develop`, `release/*`, etc.), extend the check in your CLAUDE.md

### Step-by-Step Checklist

Each commit **must** complete the following steps:

```
0. ✅ Branch check (not on main, proper branch name)
   ↓
1. ✅ Run formatters (as defined in CLAUDE.md)
   ↓
2. ✅ Run linters (as defined in CLAUDE.md)
   ↓
3. ✅ Run tests (full or targeted, as defined in CLAUDE.md)
   ↓
4. ✅ Security scan (check for secrets, debug code)
   ↓
5. ✅ Stage changes (git add <files>) + verify working tree is clean
   ↓
6. ✅ **Security Audit (the `security-audit` skill)**
   │   - Phase 1: Test Completeness Check ("can't write more tests" standard)
   │   - Phase 2: Security Vulnerability Scan
   │   - Phase 3: Code Quality Check
   │   - Returns: PASS / PASS with deferred issues / FAIL
   ↓
7. ✅ Pre-commit review (csa review --diff — reviews all uncommitted changes vs HEAD)
   ↓
8. Blocking issues found (in current changes)?
   ├─ YES → Fix issues → Re-run from step 1
   │
   └─ NO → Continue to step 9
   ↓
9. ✅ Generate commit message → Commit
   ↓
10. ✅ **Post-Commit: Push & PR consideration** (see below)
   ↓
11. Deferred issues found (in other modules)?
    ├─ YES → Invoke Task tools (TaskCreate/TaskUpdate) → Fix immediately (step 12)
    │
    └─ NO → Done
    ↓
12. ✅ **Post-Commit Fix** (if deferred issues exist)
    - Fix deferred issues by priority (Critical → High → Medium)
    - Each fix goes through full workflow (steps 0-8)
    - Continue until all deferred issues resolved
```

**CRITICAL**: Use **Task tools (TaskCreate/TaskUpdate)** to record deferred issues.
- Ensures issues persist through auto-compact cycles
- Forces explicit executor assignment
- Prevents issue loss in long sessions

### Security Audit (CRITICAL)

**Core principle**: Code must pass **the `security-audit` skill** before commit. **Audit failure = Commit rejected**.

Detailed audit workflow is in `security-audit/SKILL.md`, core three phases:
1. **Test Completeness** — "Can you propose a test case that doesn't exist?" Yes → FAIL
2. **Security Vulnerability Scan** — Input validation, size limits, panic risks
3. **Code Quality Check** — No debug code, secrets, commented-out code

| Verdict | Action |
|---------|--------|
| **PASS** | Commit directly |
| **PASS with deferred** | Commit → **Task tools** record → **Fix immediately** (no "later") |
| **FAIL** | Fix → Re-audit |

**Deferred issues use Task tools** (TaskCreate) for recording, ensuring they persist after auto-compact. Fix by priority Critical → High → Medium, each fix goes through full commit workflow.

> **Detailed reference**: See Appendix B below for three-phase detailed checklist, verdict handling rules, FORBIDDEN behaviors list.

### Post-Commit: Push & PR (Git Workflow)

After commit, **evaluate whether to push and create PR**:

```
Commit done
   ↓
Is this a meaningful milestone (feature complete, bug fixed, refactor done)?
   ├─ YES → Push to origin (personal fork) + consider creating PR for LLM audit
   │        git push origin <branch>
   │        Then: PR to upstream for review, or continue on same branch
   └─ NO (mid-feature, more commits needed) → Continue development
```

**Remind user to consider**:
- Earlier push = earlier problem discovery
- No need to wait for "perfect" before pushing — draft PR is fine
- If your project has CI/bot review pipelines, pushing triggers them automatically

### Security Scan Checklist

**Before committing, verify NO:**

| Check | What to Look For | How to Fix |
|-------|------------------|------------|
| Debug code | `println!`, `console.log`, `dbg!`, `print()` | Remove all debug statements |
| Hardcoded secrets | API keys, passwords, tokens, private keys | Use env vars or secret manager |
| Commented-out code | Dead code kept "just in case" | Delete it (git remembers) |
| Sensitive logs | PII, passwords, tokens in log output | Sanitize logging |
| TODO/FIXME security | `// TODO: fix security`, `// FIXME: validate input` | Fix now or create issue |

### Quality Gates

**All of these MUST pass before commit:**

```bash
# 1. Run your project's formatter (as defined in CLAUDE.md)
# Example: just fmt-all, or npm run format, etc.

# 2. Run your project's linter (as defined in CLAUDE.md)
# Example: just clippy-all, or npm run lint, etc.

# 3. Run your project's test suite (as defined in CLAUDE.md)
# Example: just test-all, or npm test, etc.
# OR for specific module as defined in your project
```

## Commit Message Generation (Delegated)

### Rule: Delegate to cheaper tools

**Do NOT** read `git diff` yourself and write commit message.

**Reason**: Reading large diffs wastes Opus tokens. Delegate to cheaper tools.

### Recommended Workflow

```
1. [Main] Stage changes: git add <files>
   ↓
2. [Main] Ensure no unstaged changes remain (git diff should be empty)
   If unstaged changes exist, either stage them or stash them first.
   ↓
3. [CSA:review] Review all uncommitted changes relative to HEAD
   csa review --diff
   (Note: --diff uses 'git diff HEAD', covering both staged and unstaged changes)
   ↓
4. [CSA:run] Generate commit message (if review passes)
   csa run "Run 'git diff --staged' and generate a Conventional Commits message"
   ↓
5. [Main] Commit with generated message
```

### CSA Review Output Format

csa review will return:
- ✅ List of issues found (if any)
- ✅ Change summary

**If csa review finds issues**:
1. Use `csa run --tool codex` in same session to fix
2. Run `csa review --diff` again (use `--session <ID>` to resume previous review session)
3. Loop until no issues
4. Generate commit message (see Step 3 above)

### Alternative: CSA

If csa review is not available:

```bash
csa run "Run 'git diff --staged' and generate a Conventional Commits message"
```

## Commit Granularity

### Atomic Commits Rule

**Each commit must be**:

1. **Self-contained**: One logical change
   - ✅ Good: `feat(auth): implement JWT validation`
   - ❌ Bad: `feat: add auth and refactor API and update docs`

2. **Buildable**: Code can compile/run
   - ✅ Code not broken after commit
   - ❌ Code won't compile after commit

3. **Reversible**: Can safely rollback
   - ✅ `git revert` won't break other features
   - ❌ Revert would break other features

### Commit Immediately After Logical Unit

**Workflow:**
```
Complete logical unit → Format → Lint → Test → Review → Commit → Next unit
```

**BAD** (❌):
```
Implement feature A
Implement feature B
Implement feature C
Commit all together  ← Wrong!
```

**GOOD** (✅):
```
Implement feature A → Review → Commit
Implement feature B → Review → Commit
Implement feature C → Review → Commit
```

### MUST NOT Start Next Work Before Commit

**After completing a logical unit, must commit first before starting next task.**

- ❌ NEVER start next feature while uncommitted changes exist
- ❌ NEVER have two logical units' changes in working tree simultaneously
- ❌ NEVER use git worktree for parallel development (storage constraints, merge conflicts waste tokens)
- ✅ Complete → Commit → ONLY THEN start next unit

**Why**: Pre-commit hooks validate the entire working tree. Uncommitted changes from unit A cause unit B's hooks to fail, and agents start "fighting" each other's intermediate states.

### Separate Concerns

**Keep these in SEPARATE commits:**

| Separate This | From This | Why |
|---------------|-----------|-----|
| Feature code | Test code | Easier to review |
| Refactoring | New features | Different change types |
| Dependency updates | Code changes | Isolate external changes |
| Formatting fixes | Logic changes | Avoid noise in diff |

## Special Cases

### Submodule Updates

For commits that only update submodule pointers:

```bash
git commit -m 'chore(submodules): bump submodules'
```

**No detailed body required.**

#### Mixed Visibility Repos (Security)

When parent repo is **public** but submodules are **private**:

**In private submodules:**
- ✅ Use full Conventional Commits with details

**In public parent repo:**
- ✅ Use generic message: `chore(submodules): bump submodules`
- ❌ NO details about private submodule changes

**Why**: Prevent leaking:
- Feature names or business logic
- Security fix details
- Internal project structure

### Merge Commits

**Prefer rebase over merge** for cleaner history.

If merge necessary:
```
merge: integrate feature/user-auth into main

[MOTIVATION]
Feature branch has diverged significantly from main.
Rebase would create too many conflicts.
```

### Reverts

Use `git revert` and include reason:

```
revert: feat(parser): add nested expressions

This reverts commit abc1234.

[MOTIVATION]
Introduced regression in production (#1234).
Reverting while investigating root cause.
```

## Complete Workflow Example

### Scenario: Implement JWT Validation

```bash
# 1. Write code
# (implement JWT validation in src/auth/jwt.rs)

# 2. Run your project's formatter
# Example: just fmt-all, npm run format, etc.

# 3. Run your project's linter
# Example: just clippy-all, npm run lint, etc.

# 4. Run your project's test suite
# Example: just test-auth, npm run test:auth, etc.

# 5. Stage changes
git add src/auth/jwt.rs tests/auth/jwt_test.rs

# 6. Pre-commit review (delegated to csa review)
# csa review --diff
#
# Output:
# ✅ No issues found
# Suggested message:
# feat(auth): implement JWT validation
#
# [MOTIVATION]
# Add JWT token validation to support authenticated API endpoints.
#
# [IMPLEMENTATION DETAILS]
# - Added JwtValidator struct with verify_token method
# - Implemented claims extraction and expiry check
# - Added 10 test cases for valid/invalid tokens

# 7. Commit with generated message
git commit -m "$(cat <<'EOF'
feat(auth): implement JWT validation

[MOTIVATION]
Add JWT token validation to support authenticated API endpoints.

[IMPLEMENTATION DETAILS]
- Added JwtValidator struct with verify_token method
- Implemented claims extraction and expiry check
- Added 10 test cases for valid/invalid tokens

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"

# 8. Verify commit
git log -1 --pretty=format:"%s%n%n%b"
```

### Scenario: Commit with Deferred Fixes (Summary)

```
1. Audit returns PASS_DEFERRED (current code OK, other modules have issues)
2. Task tools → TaskCreate for each deferred issue
3. Commit current changes
4. Push to origin, consider PR
5. Fix deferred issues by priority (Critical → High → Medium)
6. Each fix: full workflow (branch check → fmt → lint → test → audit → commit)
7. Result: multiple clean commits, all audited
```

> **Complete example**: See Appendix A below (includes TaskCreate invocation, commit message template, step-by-step bash).

## Anti-Patterns (FORBIDDEN)

### ❌ Common Mistakes

| Anti-Pattern | Why It's Bad | Fix |
|--------------|--------------|-----|
| Non-English commit message | Violates English-only rule | Use English |
| `git commit -m "fix"` | No context, unclear | Use Conventional Commits |
| Commit without review | May introduce bugs | Always use csa review |
| Batch multiple features | Not atomic | One commit per logical unit |
| Skip formatters/linters | Code quality issues | Always run before commit |
| Hardcoded secrets | Security vulnerability | Remove before commit |
| Debug code in commit | Pollutes codebase | Clean up before commit |
| `git commit --no-verify` | Bypasses hooks | Never skip verification |
| Ignore deferred issues | Security vulnerabilities accumulate | Fix immediately post-commit |
| "Fix later" deferred issues | Violates "Commit = Audited" | Fix in current session |
| Continue to new work with deferred issues | Technical debt compounds | Clear deferred queue first |

### ❌ Reading Diff Yourself (Token Waste)

**WRONG:**
```
1. [Main] Run: git diff --staged
2. [Main] Read 5000-line diff output
3. [Main] Write commit message
   → Wasted ~5,000 Opus tokens
```

**CORRECT:**
```
1. [CSA:review] Review uncommitted (csa review --diff)
   → Uses cheap Codex/Gemini tokens
2. [CSA:run] Generate commit message (csa run)
3. [Main] Use generated message
   → Saved ~4,500 tokens (90%)
```

## Integration with Task Tools

When using Task tools (TaskCreate/TaskUpdate), **reference this commit skill** for commit steps:

```python
# Using TaskCreate
TaskCreate(
    subject="[Sub:developer] Implement JWT validation",
    description="Implement JWT validation logic",
    activeForm="Implementing JWT validation"
)

TaskCreate(
    subject="[Skill:commit] Commit JWT validation changes",
    description="Run commit workflow",
    activeForm="Committing changes using commit skill"
)
```

**The `[Skill:commit]` executor means:**
1. Run your project's formatter (as defined in CLAUDE.md)
2. Run your project's linter (as defined in CLAUDE.md)
3. Run your project's test suite (as defined in CLAUDE.md)
4. Security scan
5. Pre-commit review (csa review)
6. Fix issues if any
7. Generate commit message (via csa run)
8. Commit

## ROI (Return on Investment)

| Benefit | Impact |
|---------|--------|
| **Consistent format** | All commits follow Conventional Commits |
| **Automated changelog** | Can generate changelog from commit messages |
| **Code quality** | Mandatory review catches bugs before commit |
| **Security** | Pre-commit scan prevents secret leaks |
| **Token efficiency** | Delegation saves 90% tokens on message generation |
| **Clear history** | Atomic commits make git history readable |
| **Reversibility** | Easy to revert individual changes |

## Summary

**Core philosophy**: **Commit = Audited** — Each commit represents code that has passed security audit.

**Workflow priority**:
```
0. Branch check (not on main)
1. Fix blocking issues → Commit → Push to origin
2. Fix deferred issues (Task tools → Critical → High → Medium)
3. Consider PR for LLM audit
4. ONLY THEN: Start new work
```

**Related Skills**: `security-audit` (audit), Task tools (deferred issue tracking), `csa review` (code review), `csa run` (commit message generation)

---

## Appendix A: Deferred Fixes Example

### Complete Workflow Example

```bash
# 1-4. Write code, format, lint, test (same as normal workflow)
# Implement feature in src/api/handler.rs

# 5. Stage changes
git add src/api/handler.rs tests/api/handler_test.rs

# 6. Security audit (the `security-audit` skill)
# [security-audit] Review uncommitted changes
#
# Output:
# PASS with deferred issues
#
# Current changes (src/api/handler.rs):
#   Test completeness: 100%
#   No security vulnerabilities
#
# Deferred issues (other modules):
#   src/auth/session.rs: Missing test for expired session (CRITICAL)
#   src/db/query.rs: No input size limit on query string (HIGH)

# 7. Invoke Task tools for deferred issues
# [Main agent uses TaskCreate]
TaskCreate(
    subject="[Post-commit fix] auth/session.rs: Add test for expired session (CRITICAL)",
    description="Security audit found missing test",
    activeForm="Adding expired session test"
)
TaskCreate(
    subject="[Post-commit fix] db/query.rs: Add input size limit (HIGH)",
    description="Security audit found no size limit",
    activeForm="Adding query size limit"
)
# Why Task tools: persist through auto-compact, explicit executors

# 8. Commit current changes
git commit -m "$(cat <<'EOF'
feat(api): implement user profile handler

[MOTIVATION]
Add endpoint to retrieve user profile data.

[IMPLEMENTATION DETAILS]
- Added ProfileHandler with get_profile method
- Implemented profile data serialization
- Added 8 test cases covering all edge cases

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"

# 9. Post-commit: push and consider PR
git push origin feat/api-profile-handler

# 10. Immediately fix deferred issues (CRITICAL first)

# Fix 1: auth/session.rs (CRITICAL)
git commit -m "$(cat <<'EOF'
test(auth): add expired session test

[MOTIVATION]
Security audit identified missing test for expired session handling.

[IMPLEMENTATION DETAILS]
- Added test_expired_session_rejected()
- Verifies session expiry check works correctly
- Covers edge case of exactly expired timestamp

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"

# Fix 2: db/query.rs (HIGH)
git commit -m "$(cat <<'EOF'
fix(db): add query size limit to prevent DoS

[MOTIVATION]
Security audit found no input size validation on query strings.

[IMPLEMENTATION DETAILS]
- Added MAX_QUERY_SIZE = 10KB constant
- Validate query length before processing
- Return error for oversized queries
- Added test for size limit enforcement

Co-Authored-By: Claude <noreply@anthropic.com>
EOF
)"

# 11. All deferred issues resolved
git log --oneline -3
# abc1235 fix(db): add query size limit to prevent DoS
# abc1234 test(auth): add expired session test
# abc1233 feat(api): implement user profile handler
```

### Key Takeaways

1. Current changes passed audit
2. Audit found issues in OTHER modules (not blocking current commit)
3. Committed current work first
4. **Used Task tools** (TaskCreate) to record deferred issues
5. Task tools ensured issues persist through auto-compact
6. Pushed to origin for LLM audit
7. Fixed deferred issues immediately by priority (Critical -> High)
8. Each fix went through full workflow (branch check -> fmt -> lint -> test -> audit -> commit)
9. Result: 3 clean commits, all audited

---

## Appendix B: Security Audit Integration

### Three-Phase Audit (from the `security-audit` skill)

#### Phase 1: Test Completeness — Most Critical

- Are all public functions tested?
- Are boundary conditions (empty, max, boundary) tested?
- Are error conditions tested?
- **Key question**: "Can you propose a test case that doesn't exist?"
  - If yes -> **FAIL, must add tests**
  - If no -> PASS, continue to Phase 2

#### Phase 2: Security Vulnerability Scan

- Does input validation exist?
- Are size/length limits enforced?
- Can malicious input trigger a panic?
- Is there resource exhaustion risk?

#### Phase 3: Code Quality Check

- No debug code (`println!`, `dbg!`, `console.log`)
- No hardcoded secrets
- No commented-out code
- No TODO/FIXME security items

**Audit failure = Commit rejected.** Must fix and re-audit.

### Verdict Handling

| Verdict | Meaning | Action |
|---------|---------|--------|
| **PASS** | No issues | Commit directly |
| **PASS with deferred** | Current changes OK, other modules have issues | Commit -> TaskCreate -> Fix immediately |
| **FAIL** | Current changes have issues | Fix -> Re-audit |

#### Key distinction

- **PASS with deferred** does NOT block current commit
- But deferred issues MUST be fixed **immediately** after commit
- "Record and fix later" is NOT allowed — must fix in **current session**
- Use **Task tools** (TaskCreate) to record deferred issues so they persist through auto-compact

#### FORBIDDEN behaviors

- Commit current changes and ignore deferred issues
- "Fix later" or "next time"
- Continue to new work before fixing deferred issues
- Let deferred issues accumulate across multiple commits

#### Why MUST fix immediately

- Deferred issues often indicate security vulnerabilities
- Delaying fixes leads to forgotten issues
- May affect other developers' work
- Violates "Commit = Audited" philosophy
