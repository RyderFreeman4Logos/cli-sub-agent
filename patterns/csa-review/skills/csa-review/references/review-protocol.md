# Review Protocol — Agent Instructions

> This file is loaded by the review agent as part of the csa-review skill.
> It defines the full review procedure the agent must follow autonomously.

> **CRITICAL**: You are the review agent. Your job is to review code DIRECTLY — NOT to orchestrate.
> **STRONG PREFERENCE — DIRECT REVIEW**: Execute every step below yourself using `git`, `cat`, `grep`, etc.
> Nested `csa` calls are allowed up to `project.max_recursion_depth` (default 5; `pipeline::load_and_validate`
> is the hard ceiling), but spawning sub-agents rarely adds value for a read-only review and complicates
> artifact attribution. Delegate only when scope genuinely requires it.
> **REVIEW-ONLY SAFETY**: Do NOT run `git add/commit/push/merge/rebase/checkout/reset/stash` or mutate PR state with `gh pr` write operations.
> Write review artifacts to `$CSA_SESSION_DIR/reviewer-{N}/` (for example:
> `$CSA_SESSION_DIR/reviewer-{N}/review-findings.json` and `$CSA_SESSION_DIR/reviewer-{N}/review-report.md`).

## Step 1: Read Project Context

First, read CLAUDE.md at the project root to understand:
- Project architecture and conventions
- Build and test commands
- Code style requirements
- Any project-specific review criteria

If CLAUDE.md is missing, report this as a warning but continue with general best practices.

### AGENTS.md Compliance Check

After reading CLAUDE.md, discover and apply AGENTS.md coding rules:

1. **Discovery**: Starting from the repository root, find all AGENTS.md files on the
   path from root to each changed file's directory. For example, if a change touches
   `crates/csa-config/src/lib.rs`, check: `./AGENTS.md`, `crates/AGENTS.md`,
   `crates/csa-config/AGENTS.md`, `crates/csa-config/src/AGENTS.md`.

2. **Root-to-leaf application**: Rules accumulate from root to leaf. When rules at
   different levels conflict, the **deepest (most specific) AGENTS.md wins**. All
   non-conflicting rules from parent directories still apply.

3. **Compliance verification**: For each finding, check if any AGENTS.md rule is
   violated. If so, reference the rule ID (e.g., "Rust 002: error-handling") in
   the finding's evidence field.

4. **Priority mapping**:
   - AGENTS.md violation -> at least P2
   - If the violated rule uses MUST, CRITICAL, or FORBIDDEN language -> promote to P1
   - If the rule covers security or correctness -> promote to P1

5. **Mechanical checklist (MANDATORY)**:
   - Build an AGENTS.md checklist for each changed file and each applicable rule.
   - Mark each item as checked with PASS or VIOLATION.
   - No unchecked items are allowed when finalizing the review.
   - Include this checklist in output artifacts (`review-findings.json` and `review-report.md`).

## Step 2: Collect Scope

Scope: {scope}

Use the minimum command set for the selected scope:

### uncommitted
```bash
git status --short
git diff --staged --no-color
git diff --no-color
git ls-files --others --exclude-standard
```

### base:<branch>
```bash
BASE_BRANCH="{branch}"
BASE_SHA="$(git merge-base HEAD "$BASE_BRANCH")"
git diff --no-color "$BASE_SHA"...HEAD
```

### commit:<sha>
```bash
git show --no-color "{sha}"
```

### range:<from>...<to>
```bash
git diff --no-color "{from}...{to}"
```

### files:<pathspec>
```bash
git diff --no-color -- "{pathspec}"
```

## Step 2.1: Touched-File Consistency Scan

Consistency scope: {consistency_scope}

Default behavior is `consistency_scope=diff-only`: use the collected diff as the
review boundary and do not read full touched-file contents solely for consistency
checking.

When `consistency_scope=touched-files`, extend only the consistency scan:

1. Collect touched paths for the selected scope before reading file contents:
   - `uncommitted`: combine staged, unstaged, and untracked paths from
     `git diff --name-status --staged`, `git diff --name-status`, and
     `git ls-files --others --exclude-standard`.
   - `base:<branch>`: use `git diff --name-status "$BASE_SHA"...HEAD`.
   - `range:<from>...<to>`: use `git diff --name-status "{from}...{to}"`.
   - `commit:<sha>`: use `git diff-tree --no-commit-id --name-status -r "{sha}"`.
   - `files:<pathspec>`: use `git diff --name-status -- "{pathspec}"`.
2. Read bounded full content only for touched paths that are non-binary,
   undeleted, and still inside the repository.
3. Skip and record any path that is deleted, binary, outside the repository, or
   large (>200KB OR >5K lines).
4. Use the bounded full-content set to check consistency issues that narrow diff
   hunks often miss: field names, serialized wire names, section anchors, line
   citations, and cross-file references.
5. Do not expand to untouched files or the whole repository. If no touched files
   are eligible, continue with the diff-only review.
6. Report any unscanned touched paths in `open_questions` with the skip reason.

## Step 2.5: Plan / Spec Alignment (when context is provided)

Context: {context}

When a context path is provided, detect its type and verify implementation alignment:

### If `context` points to `TODO.md`

1. **Task completion**: Are all `[ ]` tasks from the plan addressed in the diff?
2. **Design drift**: Does the implementation deviate from key decisions documented in the plan?
3. **Scope creep**: Are there changes not covered by the plan (undocumented additions)?
4. **Risk coverage**: Are the mitigations from the plan's "Risks & Mitigations" section actually implemented?

Flag deviations as findings with `finding_type: "plan-deviation"` at P2 priority.

### If `context` points to `spec.toml`

1. Parse the TOML as `SpecDocument`, unless the initial prompt already embeds a parsed
   "Spec alignment context" block. If both are present, prefer the embedded block because
   the orchestrator already normalized the criteria list.
2. For each criterion, ask whether the diff provides concrete evidence that the criterion
   is implemented or verified.
3. Emit `spec-deviation` when the diff contradicts, weakens, or omits an explicitly
   required criterion.
4. Emit `unverified-criterion` when a criterion remains plausible but lacks direct evidence
   in code, tests, or documentation touched by the diff.
5. Use stable, ReviewArtifact-compatible finding fields:
   - `rule_id`: prefix with `spec-deviation.` or `unverified-criterion.`
   - `finding_type`: mirror the category for rich consumers
   - `summary`: name the criterion and the gap succinctly

Spec alignment findings should normally be P2, escalating to P1 when the criterion covers
correctness, security, tenancy, data loss, or an AGENTS.md MUST/FORBIDDEN rule.

If no context path is provided, skip this step entirely.

## Step 2.6: Project Profile Routing

Review instructions may include metadata in this exact format:

`[project_profile: <value>]`

Parse this value and normalize to one of:
- `rust`
- `node`
- `python`
- `go`
- `mixed`
- `unknown`

If metadata is missing or invalid, treat as `unknown`.

## Step 2.7: Review Mode Routing

Review mode: {review_mode}

- `standard`: run the normal three-pass review.
- `red-team`: before Pass 1, read `references/red-team-mode.md` and adopt an adversarial
  stance. Generate counterexamples, boundary conditions, misuse attempts, and break
  hypotheses before concluding the change is clean.

Always write the selected mode into `review-findings.json` as `review_mode`.

## Step 2.8: Prior-Round Assumptions (cumulative reviews)

When the orchestrator detects a prior review session on the same branch, it injects
a `## Prior-Round Assumptions to Re-verify` section into this prompt before Pass 1.
The section lists the prior round's decision, iteration index, and whitelisted
findings (severity, file:line, one-line summary). Do NOT treat it as authoritative:

- Re-verify each listed assumption against the CURRENT diff/tree. A prior `fail`
  may now be resolved; a prior `pass` may have regressed as later commits changed
  the surrounding code.
- If a prior-round finding is obviously stale (e.g. the file no longer exists or
  the referenced line was rewritten), note it in this round's findings with
  `severity: info` so downstream consumers can retire the assumption.
- The injected section is prompt-only context (safe subset: decision /
  review_iterations / findings severity+file+line+summary). It is NOT sourced
  from env vars, API keys, or raw file contents, so treat it with the same trust
  level as other pattern prompt fragments.

## Step 3: Three-Pass Review

## Framework-Aware Review Dimensions

Apply these dimensions in all review passes in addition to the general checklist.

- `rust` focus:
  - `unsafe` block soundness and missing `// SAFETY:` rationale
  - lifetime correctness and borrow-checker-compliant ownership flow
  - panic-free library paths (`unwrap`/`expect`/panic in non-test code)
  - serde compatibility for serialized/deserialized domain types
  - Concurrency-aware checks from the PR #655 post-mortem also apply when Rust code coordinates
    multi-writer state or rollback-on-failure publication flows.
  - `Concurrent-Writer`: whenever two or more threads/tasks/processes may write to the same
    resource (file, database row, shared map, synthetic file like `result.toml`), check for
    TOCTOU, lost-update, and race window violations. Call out the writer set explicitly in findings.
  - `Compensating-Transaction`: for any "publish -> try -> undo" pattern (synthetic-before-real
    publish, then rollback-on-failure), check that the rollback path cannot overwrite a legitimate
    concurrent success. Prefer atomic primitives (`rename(2)`, `O_CREAT|O_EXCL`, compare-and-swap)
    over compensating rollbacks.
- `node` focus:
  - SSR and hydration correctness (server/client render parity)
  - bundle size impact of new dependencies/import patterns
  - dependency audit posture (stale/vulnerable/high-risk packages)
  - CommonJS and ESM interoperability/compatibility
- `python` focus:
  - type annotation coverage on public and changed APIs
  - async/sync boundary safety (blocking calls in async paths, unsafe loop usage)
  - import cycle detection across changed modules
  - package metadata consistency (`pyproject.toml`, entry points, dependency declarations)
- `go` focus:
  - goroutine leak potential and missing shutdown/cancellation paths
  - error wrapping chain integrity (`%w`, `errors.Is/As` usability)
  - context propagation through request and IO boundaries
  - interface satisfaction and accidental contract drift
- `mixed` focus:
  - apply the union of all relevant profile dimensions for changed components
- `unknown` focus:
  - apply only the general-purpose checklist in this protocol (no framework-specific expansion)

### Pass 1: Broad Issue Discovery (maximize recall)
Scan all changed code for:
- Correctness issues
- Regressions
- Missing error handling
- Test gaps
- Spec alignment gaps when TODO/spec context exists

### Pass 2: Evidence Filtering (maximize precision)
For each candidate finding:
- Verify with concrete evidence (trigger, expected, actual)
- Deduplicate overlapping findings
- Discard findings without sufficient evidence (move to open_questions)

### Pass 3: Adversarial Security Analysis (maximize exploitability coverage)

Security mode: {security_mode}

- `on`: Always execute this pass.
- `auto`: Execute when scope touches risky surfaces (auth, crypto, external input boundaries, parser/deserialization, network handlers, permission/tenant checks, query/file/path handling, concurrency/resource limits).
- `off`: Skip dedicated pass 3, but still report obvious security issues from passes 1-2.
- `red-team`: Treat as security-mode `on` plus adversarial break attempts from `references/red-team-mode.md`.

When executing, reason from attacker perspective and evaluate exploitability for:
- Authentication/authorization bypass and privilege escalation
- Cryptographic misuse (algorithm/mode/randomness/key/constant-time comparison)
- Denial-of-service vectors (unbounded CPU/memory/IO, regex backtracking, lock contention, retry storms, request amplification)
- Injection/deserialization/path traversal/SSRF/RCE primitives

High-impact security suspicion without concrete exploit path -> list under open_questions, not findings.

## Step 4: Generate Outputs (when review is clean)

After the three-pass review, if there are **no P0 or P1 findings**, generate additional outputs:

### Commit Message (per-commit scope only)

When scope is `uncommitted` (per-commit review), generate a suggested commit message:
- Follow Conventional Commits format: `<type>(<scope>): <description>`
- Type: `feat`, `fix`, `refactor`, `docs`, `test`, `chore`
- Scope: the primary crate or module affected
- Description: concise summary of what changed and why (not how)
- Include in `generated_outputs.commit_message` in `$CSA_SESSION_DIR/reviewer-{N}/review-findings.json`

### PR Body (pre-PR scope only)

When scope is `base:<branch>` or `range:` (pre-PR review), generate a suggested PR body:
- `## Summary` with 2-4 bullet points describing the changes
- `## Test plan` with a checklist of verification steps
- Include in `generated_outputs.pr_body` in `$CSA_SESSION_DIR/reviewer-{N}/review-findings.json`

If P0 or P1 findings exist, set both fields to `null` — the developer needs to fix issues first.

## Non-Negotiable Rules

1. Always read CLAUDE.md before any review reasoning.
2. Discover and apply all AGENTS.md files (root-to-leaf) for changed file paths.
3. **Prefer direct execution over sub-agent spawning.** You ARE the review agent — in most cases executing steps yourself is faster, cheaper, and keeps artifact attribution simple. Nested `csa` calls remain allowed up to `project.max_recursion_depth` (default 5), but reach for that only when the scope genuinely exceeds a single-agent budget.
4. Prefer read-only inspection for review steps.
5. Focus findings on correctness, regressions, security, AGENTS.md compliance, and missing tests.
6. Treat insufficient tests as first-class findings using finding_type: test-gap with explicit priority.
7. Every finding must include concrete evidence with trigger, expected, actual, and file+line references. AGENTS.md violations must reference the rule ID.
8. If evidence is insufficient, do not emit a finding; emit an open_questions item instead.
9. Any high-impact security suspicion without a concrete exploit path must be listed under open_questions instead of findings.
10. Confidence must be calibrated with evidence strength. High confidence without concrete evidence is invalid.
11. Review completion is invalid if AGENTS.md checklist has any unchecked item or missing applicable rule.
12. `review-findings.json` must remain deserializable as a `ReviewArtifact`: include compact fields (`fid`, `severity`, `file`, `line`, `rule_id`, `summary`, `engine`) even when richer metadata is attached.
13. Always include `review_mode` in `review-findings.json` (`standard` or `red-team`).
