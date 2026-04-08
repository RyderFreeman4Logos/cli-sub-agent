---
name: quality-gate
description: "Use when: creating, auditing, or optimizing repository quality gates (git hooks, pre-commit checks, CI guards, merge protection)"
allowed-tools: Bash, Read, Grep, Glob, Write, Edit
triggers:
  - "quality-gate"
  - "/quality-gate"
  - "create gates"
  - "add pre-commit"
  - "setup hooks"
  - "optimize gates"
  - "audit gates"
---

# Quality Gate: Repository Gate Infrastructure Builder

Create, audit, and optimize multi-layer quality gates for any repository.
Gates enforce code quality deterministically — through tooling, not instructions.

## Principles

1. **Deterministic over instructional** — enforce via exit codes, not README prose
2. **Fail-closed** — unknown state = blocked; only explicit pass allows through
3. **Layer defense** — pre-commit catches early, pre-push catches drift, pre-merge catches bypass
4. **Sandbox-aware** — skip gates that can't run in CI/sandbox (check writable paths)

## Gate Architecture (4 Layers)

```
Layer 1: PRE-COMMIT (developer machine, every commit)
  ├── Branch protection (block commits to protected branches)
  ├── Monolith guard (block oversized files by token/line count)
  ├── Artifact guard (block generated/scratch files from staging)
  ├── Version guard (version must differ from base branch)
  ├── Charset guard (enforce codebase language consistency)
  ├── Format (auto-format + auto-stage formatted files)
  ├── Lint (language-specific strict linting)
  ├── Dependency audit (license + vulnerability scanning)
  └── Test (unit + integration + e2e)

Layer 2: PRE-PUSH (before code reaches remote)
  ├── Version bump verification (redundant check, different timing)
  ├── Review verification (require recorded review session for HEAD)
  └── Advisory warnings (e.g., missing PR-bot marker for open PRs)

Layer 3: PRE-MERGE (before merging to protected branch)
  ├── Review completion marker verification
  ├── CI status check (all checks green)
  └── Merge command interception (PATH-injected wrapper)

Layer 4: POST-MERGE (after code lands on protected branch)
  ├── Auto-rebuild (recompile and install updated binaries)
  ├── Notification (audit log, team alerts)
  └── Cleanup (stale branch pruning, marker cleanup)
```

## Execution Protocol

### Phase 1 — DETECT (Audit Current State)

Analyze the repository to determine:

1. **Tech stack detection**:
   ```bash
   # Check for language indicators
   ls Cargo.toml package.json go.mod pyproject.toml Makefile justfile \
      CMakeLists.txt build.gradle pom.xml 2>/dev/null
   ```

2. **Hook manager detection**:
   ```bash
   # Lefthook (preferred)
   ls lefthook.yml lefthook.yaml .lefthook.yml 2>/dev/null
   # Husky (Node.js)
   ls .husky/_/husky.sh 2>/dev/null
   # pre-commit (Python)
   ls .pre-commit-config.yaml 2>/dev/null
   # Raw git hooks
   ls .git/hooks/pre-commit .git/hooks/pre-push 2>/dev/null
   ```

3. **Task runner detection**:
   ```bash
   ls justfile Justfile Makefile makefile package.json 2>/dev/null
   ```

4. **Existing gate inventory** — enumerate all active checks:
   - Parse lefthook.yml / .husky / .pre-commit-config.yaml
   - Parse justfile / Makefile / package.json scripts
   - List scripts/hooks/ directory
   - Check CI config (.github/workflows/, .gitlab-ci.yml, etc.)

5. **Coverage gap analysis** — compare against the 4-layer model above,
   report which gates exist and which are missing.

Output a structured audit report with coverage matrix:

```
GATE AUDIT REPORT
=================
Tech Stack: Rust (Cargo workspace)
Hook Manager: lefthook (v2.x)
Task Runner: just

Layer 1 (Pre-Commit):
  [x] Branch protection
  [x] Format (cargo fmt)
  [x] Lint (clippy)
  [x] Test
  [ ] Monolith guard        <- MISSING
  [ ] Artifact guard         <- MISSING
  [ ] Dependency audit       <- MISSING

Layer 2 (Pre-Push):
  [ ] Version check          <- MISSING
  ...
```

### Phase 2 — DESIGN (Plan Gates)

Based on the audit, design gates for each missing layer. Follow tech-stack-specific
best practices:

#### Tech Stack Reference

| Stack | Format | Lint | Type Check | Test | Dep Audit | Monolith |
|-------|--------|------|------------|------|-----------|----------|
| Rust | `cargo fmt` | `cargo clippy -- -D warnings` | (compiler) | `cargo nextest run` | `cargo deny check` | tokuin/wc -l |
| Go | `gofmt -l .` | `golangci-lint run` | (compiler) | `go test ./...` | `govulncheck ./...` | tokuin/wc -l |
| Python | `ruff format` | `ruff check` | `mypy --strict` | `pytest` | `pip-audit` | tokuin/wc -l |
| TypeScript | `biome format` | `biome check` | `tsc --noEmit` | `vitest run` | `npm audit` | tokuin/wc -l |
| Mixed | Per-language | Per-language | Per-language | Per-language | Per-language | tokuin/wc -l |

#### Gate Design Decisions

For each missing gate, decide:

1. **Hard vs soft fail**: Pre-commit checks are hard (exit 1). Advisories are soft (echo warning).
2. **Auto-fix capability**: Formatters can auto-fix + auto-stage. Linters generally cannot.
3. **Skip conditions**: When should the gate be skipped? (CI env, sandbox)
4. **Performance budget**: Pre-commit should complete in < 60s for good DX. Move slow checks to pre-push.

### Phase 3 — IMPLEMENT (Create Infrastructure)

Generate the following files based on design decisions:

#### 3a. Hook Manager Config

**Preferred: lefthook** (language-agnostic, fast, single binary).

```yaml
# lefthook.yml
pre-commit:
  commands:
    branch-protection:
      run: scripts/hooks/branch-protection.sh
    quality-gates:
      run: just pre-commit

pre-push:
  commands:
    version-check:
      run: scripts/hooks/version-check.sh
    # Add review-check if using csa:
    # review-check:
    #   run: scripts/hooks/review-check.sh

post-merge:
  commands:
    rebuild:
      run: scripts/hooks/post-merge-rebuild.sh
```

If the project already uses husky or pre-commit, adapt to that tool instead.

#### 3b. Task Runner (justfile or Makefile)

Organize recipes in this order:

```
1. default: pre-commit          (run all checks)
2. Individual gates:
   a. find-monolith-files       (token/line count guard)
   b. check-generated-artifacts (block generated files from staging)
   c. check-version-bumped      (version differs from base branch)
   d. check-charset             (enforce codebase language if needed)
   e. fmt                       (format + auto-stage)
   f. deny / audit              (dependency audit)
   g. lint / clippy             (strict linting)
   h. test                      (unit tests)
   i. test-e2e                  (end-to-end tests)
3. pre-commit: a b c d e f g h i (orchestration recipe)
```

**Critical patterns to include**:

**Monolith Guard** — block oversized files that degrade LLM/reviewer performance:
```bash
# Token-count check (requires tokuin, falls back to line count)
# Thresholds: MONOLITH_TOKEN_THRESHOLD (default 8000), MONOLITH_LINE_THRESHOLD (default 800)
# Process: git ls-files | parallel check_file {}
# Exclusions: *.lock, generated docs, workflow definitions
# Output: actionable error with stash-then-split instructions
```

**Artifact Guard** — block generated/scratch files from being committed:
```bash
# Check git diff --cached --name-only --diff-filter=ACMR against patterns:
# .test-target/, .tmp/, target/, dist/, node_modules/, __pycache__/
# Allow DELETIONS (cleanup commits should work)
```

**Version Guard** — enforce version bump on feature branches:
```bash
# Compare current version vs base branch version
# Skip on main/dev, skip if CSA_SKIP_VERSION_CHECK=1
# Error message includes the bump command (just bump-patch / npm version patch / etc.)
```

**Format + Auto-Stage** — format and re-add only modified tracked files:
```bash
# Run formatter
# git diff --name-only | grep '<ext>' | xargs -r git add
# This allows fmt to be part of pre-commit without manual re-staging
```

#### 3c. Hook Scripts (scripts/hooks/)

Each hook script MUST follow this template:

```bash
#!/usr/bin/env bash
# <Purpose>: <one-line description>
set -euo pipefail

# ── Skip conditions ─────────────────────────────────────────────
# Skip inside sandbox/CI environments
if [ -n "${CSA_SESSION_ID:-}" ]; then
    echo "[<hook>] Inside sandbox -- skipping."
    exit 0
fi

# ── Main logic ──────────────────────────────────────────────────
# ...

# ── Error output ────────────────────────────────────────────────
# MUST include:
# 1. What failed (exact condition)
# 2. How to fix it (exact command)
# 3. Why it matters (one line)
```

**Branch Protection** (`scripts/hooks/branch-protection.sh`):
```bash
#!/usr/bin/env bash
set -euo pipefail
branch=$(git symbolic-ref --short HEAD 2>/dev/null) || exit 0
[ -z "$branch" ] && exit 0
PROTECTED="main dev master"
for pb in $PROTECTED; do
  if [ "$branch" = "$pb" ]; then
    echo "BLOCKED: Cannot commit directly to '$branch'."
    echo "Create a feature branch: git checkout -b feat/<description>"
    exit 1
  fi
done
```

**Post-Merge Rebuild** (`scripts/hooks/post-merge-rebuild.sh`):
```bash
#!/usr/bin/env bash
# Skip sandbox, check writable target, rebuild
set -euo pipefail
if [ -n "${CSA_SESSION_ID:-}" ]; then exit 0; fi
if [ ! -w /usr/local/bin ]; then
    echo "[post-merge] Install target not writable -- skipping."
    exit 0
fi
echo "[post-merge] Rebuilding..."
if just install; then
    echo "[post-merge] Installed successfully."
else
    echo "[post-merge] WARNING: build failed (exit $?)." >&2
fi
```

#### 3d. Review Checklist (`.csa/review-checklist.md`)

Project-specific review items that encode hard-won lessons:

```markdown
# Project Review Checklist

Common pitfalls and patterns to verify during code review:

- [ ] <Domain-specific check 1>
- [ ] <Domain-specific check 2>
...
```

Derive items from:
- Recurring review findings (if review history exists)
- Language-specific pitfalls (e.g., Rust: RAII guards + process::exit, Go: goroutine leaks)
- Project architecture constraints (e.g., sandbox writable paths, FFI boundaries)
- Security patterns (e.g., config structs with serde(default) need is_default())

#### 3e. Installation Recipe

Add to justfile:
```
install-hooks:
    @git config --unset core.hooksPath 2>/dev/null || true
    lefthook install
    @echo "Hooks installed."
```

### Phase 4 — VERIFY

1. **Dry-run each gate**: Run `just pre-commit` on the current codebase
2. **Test failure modes**: Verify each gate produces clear, actionable error messages
3. **Performance check**: Time `just pre-commit` — should be < 60s for good DX
4. **Sandbox test**: Verify hooks skip gracefully when `CSA_SESSION_ID` is set

## Gate Catalog (Complete Reference)

### Layer 1: Pre-Commit Gates

#### 1.1 Branch Protection
- **Purpose**: Prevent direct commits to protected branches
- **Fail mode**: Hard (exit 1)
- **Skip**: Detached HEAD
- **Action**: Suggest feature branch naming convention
- **Applies to**: All projects

#### 1.2 Monolith File Guard
- **Purpose**: Block oversized files that degrade LLM and reviewer performance
- **Fail mode**: Hard (exit 1)
- **Thresholds**: 8000 tokens / 800 lines (configurable via env)
- **Tool**: `tokuin estimate` (fallback: `wc -l`)
- **Exclusions**: Lock files, generated docs, workflow definitions, test fixtures
- **Action**: Stash, split file, retry commit
- **Applies to**: All projects with LLM-assisted workflows

#### 1.3 Generated Artifact Guard
- **Purpose**: Block staging of generated/scratch files
- **Fail mode**: Hard (exit 1)
- **Patterns**: `.test-target/`, `.tmp/`, `target/`, `dist/`, `node_modules/`, `__pycache__/`, `*.pyc`
- **Exception**: Deletions allowed (cleanup commits)
- **Applies to**: All projects

#### 1.4 Version Bump Guard
- **Purpose**: Ensure version changes on feature branches
- **Fail mode**: Hard (exit 1)
- **Skip**: Main/dev branches, `CSA_SKIP_VERSION_CHECK=1`
- **Action**: Show bump command for the project's version tool
- **Applies to**: Projects with release versioning (libraries, CLIs, services)

#### 1.5 Charset/Encoding Guard
- **Purpose**: Enforce codebase language consistency
- **Fail mode**: Hard (exit 1)
- **Tool**: `rg "\p{Script=Han}"` (or equivalent Unicode script check)
- **Exclusions**: i18n files, test fixtures, rule/doc files
- **Applies to**: Projects with explicit language policy

#### 1.6 Formatting
- **Purpose**: Consistent code style
- **Fail mode**: Hard (exit 1 if unformatted files remain)
- **Auto-fix**: YES — format then `git add` modified tracked files
- **Tools**: `cargo fmt`, `gofmt`, `ruff format`, `biome format`, `prettier`
- **Applies to**: All projects

#### 1.7 Dependency Audit
- **Purpose**: License compliance and vulnerability scanning
- **Fail mode**: Hard (exit 1)
- **Tools**: `cargo deny check`, `npm audit`, `pip-audit`, `govulncheck`
- **Applies to**: All projects with external dependencies

#### 1.8 Linting
- **Purpose**: Catch bugs, enforce idioms, prevent anti-patterns
- **Fail mode**: Hard (exit 1 with `-D warnings`)
- **Tools**: `cargo clippy`, `golangci-lint run`, `ruff check`, `biome check`, `eslint`
- **Applies to**: All projects

#### 1.9 Testing
- **Purpose**: Verify correctness
- **Fail mode**: Hard (exit 1)
- **Tools**: `cargo nextest run`, `go test`, `pytest`, `vitest run`
- **Split**: Unit tests in pre-commit, slow E2E in pre-push (if > 60s total)
- **Applies to**: All projects

### Layer 2: Pre-Push Gates

#### 2.1 Version Re-verification
- **Purpose**: Redundant check (catches amend/rebase that reverted version)
- **Fail mode**: Hard (exit 1)
- **Applies to**: Same as 1.4

#### 2.2 Review Session Verification
- **Purpose**: Ensure code has been reviewed before reaching remote
- **Fail mode**: Hard (exit 1)
- **Check**: Query `csa session list --format json` for review session matching branch + HEAD
- **VCS support**: Git commit_id + jj change_id
- **Applies to**: Projects using CSA review workflow

#### 2.3 PR-Bot Marker Advisory
- **Purpose**: Warn about open PRs missing pr-bot verification
- **Fail mode**: Soft (echo warning, exit 0)
- **Check**: Look for `.done` marker file for the PR + current HEAD SHA
- **Applies to**: Projects using pr-bot workflow

### Layer 3: Pre-Merge Gates

#### 3.1 Review Completion Marker
- **Purpose**: Verify automated review completed for this PR
- **Fail mode**: Hard (exit 1)
- **Check**: Marker file exists at `~/.local/state/cli-sub-agent/pr-bot-markers/{REPO}/{PR}-{SHA}.done`
- **Applies to**: Projects using pr-bot

#### 3.2 Merge Command Interception
- **Purpose**: Intercept `gh pr merge` to enforce review gate
- **Mechanism**: PATH-injected shell wrapper that validates marker before forwarding to real `gh`
- **Fail mode**: Hard (wrapper exits 1 if marker missing)
- **Features**: --auto blocking, cross-repo rejection, --help passthrough
- **Applies to**: Projects using CSA merge guard (`csa hooks install-merge-guard`)

### Layer 4: Post-Merge Hooks

#### 4.1 Auto-Rebuild
- **Purpose**: Keep local binaries in sync with merged code
- **Skip**: Sandbox (`CSA_SESSION_ID`), read-only install target
- **Action**: Run build + install, warn on failure
- **Applies to**: Projects that install local binaries

#### 4.2 Audit Logging
- **Purpose**: Record merge events for compliance trail
- **Format**: JSONL append to `~/.local/state/cli-sub-agent/audit/merge-events.jsonl`
- **Fields**: timestamp, repo, branch, PR number, HEAD SHA, merger identity
- **Applies to**: Projects with audit requirements

## Anti-Patterns (NEVER Do These)

| Anti-Pattern | Why It's Wrong | Correct Approach |
|--------------|----------------|------------------|
| `--no-verify` / `-n` | Bypasses ALL hooks | Fix the failing check |
| `LEFTHOOK=0` env var | Disables hook manager | Fix the failing check |
| `git config core.hooksPath /dev/null` | Redirects hooks to void | Use `lefthook install` |
| Catch-all `\|\| true` in gates | Swallows real failures | Only on skip conditions |
| Hard-coding paths in hooks | Breaks portability | Use `git rev-parse --show-toplevel` |
| Running slow tests in pre-commit | Ruins DX (> 60s) | Move to pre-push or CI |
| Advisory-only for critical gates | Gets ignored | Hard fail with clear error |
| Checking tool version in gate | Fragile, over-specified | Check behavior, not version |

## Example Usage

| Command | Effect |
|---------|--------|
| `/quality-gate` | Full audit of current repository's gate infrastructure |
| `/quality-gate audit` | Audit-only (no changes) |
| `/quality-gate create` | Create gate infrastructure from scratch |
| `/quality-gate optimize` | Analyze and improve existing gates |
| `/quality-gate add monolith-guard` | Add specific gate to existing setup |

## Done Criteria

1. Audit report generated with coverage matrix for all 4 layers.
2. Hook manager config created or updated (lefthook.yml preferred).
3. Task runner recipes cover all applicable gates for the tech stack.
4. Hook scripts follow the standard template (skip conditions, clear errors).
5. Each gate tested: runs successfully on clean codebase.
6. `just pre-commit` (or equivalent) exits 0 on current codebase.
7. Performance: pre-commit completes in < 60s.
8. Sandbox safety: hooks skip gracefully when `CSA_SESSION_ID` is set.
9. Review checklist created with project-specific items (if applicable).
10. `lefthook install` (or equivalent) documented in setup recipe.
