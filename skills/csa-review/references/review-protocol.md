# Review Protocol — Agent Instructions

> This file is loaded by the review agent as part of the csa-review skill.
> It defines the full review procedure the agent must follow autonomously.

> **CRITICAL**: You are the review agent. Your job is to review code, NOT to orchestrate.
> **Do NOT** run `csa run`, `csa review`, or spawn sub-agents. Follow the steps below directly.

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

## Step 3: Three-Pass Review

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
