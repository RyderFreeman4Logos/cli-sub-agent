---
name: rule-extractor
description: "Use when: merged PR had HIGH/CRITICAL findings that represent a bug class — extracts reusable coding rule"
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "rule-extractor"
  - "/rule-extractor"
  - "extract rule"
  - "extract rules from findings"
  - "closed-loop learning"
---

# Rule Extractor: Closed-Loop Learning from PR-Bot Findings

## Purpose

Transform verified HIGH/CRITICAL review findings into reusable coding rules.
When pr-bot identifies a structural bug class, this skill extracts the lesson
into a rule file so the bug class goes extinct — agents see the rule at
write-time and avoid the anti-pattern entirely.

Post-merge extraction (Option 2) ensures rules come from the FINAL fix state,
not intermediate review iterations.

## When to Activate

This skill activates as a post-merge step in pr-bot when:

1. A PR was merged successfully.
2. The review history contains HIGH/CRITICAL/P1 findings.
3. At least one finding was confirmed real (fixed, not dismissed as false positive).
4. The finding represents a bug class (structural, not isolated mistake).

The orchestrator invokes this skill via:
```bash
csa plan run patterns/rule-extractor/workflow.toml
```

## Execution Protocol (ORCHESTRATOR ONLY)

### SA Mode Propagation (MANDATORY)

When operating under SA mode, ALL `csa` invocations MUST include `--sa-mode true`.

### Step-by-Step

1. **Collect findings**: Read merged PR's review artifacts. Extract HIGH/CRITICAL
   findings from `findings.toml` (via `csa session result`) or PR comments
   (via `gh api`).

2. **Classify each finding** (bash step): Dispatch LLM classifier via `csa run`
   to determine BUG_CLASS vs ISOLATED_MISTAKE. Emits
   `CSA_VAR:HAS_BUG_CLASS_FINDINGS=yes` when bug classes found. Only bug classes proceed.
   - **BUG_CLASS**: Reproducible anti-pattern, structural fix, 2+ examples possible.
   - **ISOLATED_MISTAKE**: Single-line fix, unique to code path, no precedent.

3. **Deduplicate against existing rules**: Search project-local rules
   (`.agents/project-rules-ref/`). Use keyword grep + semantic LLM comparison
   (both in a single bash block).
   - EXACT_MATCH → skip.
   - PARTIAL_MATCH → propose update (add case study to existing rule).
   - NO_MATCH → proceed to draft.

4. **Generate rule draft**: Structure mirrors `rust/017-concurrent-file-primitives.md`:
   - Core Requirement
   - Why This Rule Exists (root cause + failure mode)
   - Anti-Patterns (Forbidden) — table format
   - Required Implementation Patterns — code examples
   - Decision Checklist — 2-4 yes/no items
   - Case Study — link to source PR

   Include frontmatter for traceability:
   ```yaml
   ---
   source: pr-bot-finding
   pr: "#<PR_NUM>"
   severity: HIGH|CRITICAL
   extracted-at: <ISO-8601>
   finding-ids: [<IDs>]
   ---
   ```

5. **Propose via PR**: NEVER auto-commit. Create branch
   `chore/rules-propose-<shortsha>`, commit rule file, push, open PR.
   Human review is mandatory before merge.

   On rule-proposal PR merge, update relevant AGENTS.md with one compact line:
   ```
   NNN|bug-class-slug|one-line summary
   ```

### Filter Criteria (Gate Before Classification)

All four must pass before a finding enters the pipeline:

1. Severity is HIGH/CRITICAL/P1 (MEDIUM/LOW/nit excluded).
2. False-positive check passed (finding was fixed, not debate-dismissed).
3. Finding is a bug CLASS (structural, not isolated — verified in Step 2).
4. Fix is not trivially single-line (structural change required).

### Deduplication Strategy

Two-layer dedup prevents rule proliferation:

1. **Keyword grep** (fast, exact): Search existing rule files for bug-class
   keywords. Catches obvious duplicates.
2. **Semantic LLM comparison** (slow, fuzzy): When grep finds potential matches,
   dispatch a tier-1-quick agent to compare the bug class description against
   the matched rule's content. Catches conceptual overlap that keyword grep misses.

## Integration

- **Invoked by**: pr-bot (post-merge, opt-in)
- **Depends on**: pr-bot review artifacts (findings, debate verdicts, fix commits)
- **Outputs to**: `.agents/project-rules-ref/<lang>/` (project-local, fork-only per rule 030)
- **Constraint**: NEVER auto-commits. Always proposes via PR.
- **Constraint**: AGENTS.md rule 030 (fork-only) — PRs target user's fork.

## Done Criteria

1. All HIGH/CRITICAL findings classified (bug class vs isolated).
2. Bug classes deduplicated against existing rules.
3. New rules drafted with correct structure.
4. Proposal PR(s) created with human review required.
5. No auto-commits to any rules repository.
