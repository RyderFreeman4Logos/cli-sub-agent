# Review Protocol — Agent Instructions

> This file is loaded by the review agent as part of the csa-review skill.
> It defines the full review procedure the agent must follow autonomously.

> **CRITICAL**: You are the review agent. Your job is to review code DIRECTLY — NOT to orchestrate.
> **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command.
> Do NOT spawn sub-agents. Do NOT delegate. Execute every step below yourself using `git`, `cat`, `grep`, etc.
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

## Step 2.5: TODO Plan Alignment (when context is provided)

Context: {context}

When a TODO plan path is provided, read it and verify implementation alignment:

1. **Task completion**: Are all `[ ]` tasks from the plan addressed in the diff?
2. **Design drift**: Does the implementation deviate from key decisions documented in the plan?
3. **Scope creep**: Are there changes not covered by the plan (undocumented additions)?
4. **Risk coverage**: Are the mitigations from the plan's "Risks & Mitigations" section actually implemented?

Flag deviations as findings with `finding_type: "plan-deviation"` at P2 priority.
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

## Step 3: Three-Pass Review

## Framework-Aware Review Dimensions

Apply these dimensions in all review passes in addition to the general checklist.

- `rust` focus:
  - `unsafe` block soundness and missing `// SAFETY:` rationale
  - lifetime correctness and borrow-checker-compliant ownership flow
  - panic-free library paths (`unwrap`/`expect`/panic in non-test code)
  - serde compatibility for serialized/deserialized domain types
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
3. **Do NOT call `csa run`, `csa review`, `codex review`, or any sub-agent spawning command.** You ARE the review agent — executing these would cause infinite recursion.
4. Prefer read-only inspection for review steps.
5. Focus findings on correctness, regressions, security, AGENTS.md compliance, and missing tests.
6. Treat insufficient tests as first-class findings using finding_type: test-gap with explicit priority.
7. Every finding must include concrete evidence with trigger, expected, actual, and file+line references. AGENTS.md violations must reference the rule ID.
8. If evidence is insufficient, do not emit a finding; emit an open_questions item instead.
9. Any high-impact security suspicion without a concrete exploit path must be listed under open_questions instead of findings.
10. Confidence must be calibrated with evidence strength. High confidence without concrete evidence is invalid.
11. Review completion is invalid if AGENTS.md checklist has any unchecked item or missing applicable rule.
