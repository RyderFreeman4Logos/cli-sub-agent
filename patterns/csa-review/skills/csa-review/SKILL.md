---
name: csa-review
description: "Use when: running CSA-driven code review, independent model selection"
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "csa-review"
  - "csa review"
  - "CSA code review"
---

# CSA Review: Independent Code Review Orchestration

## Role Detection (READ THIS FIRST — MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the csa-review skill"`, then:

**YOU ARE THE REVIEW AGENT.** Follow these rules:
1. **SKIP the entire "Execution Protocol" section below** — it is for the orchestrator, not you.
2. **Read [Review Protocol](references/review-protocol.md)** and follow it step by step. That file tells you exactly how to perform the review.
3. **Read [Output Schema](references/output-schema.md)** for the required output format.
4. Your scope/mode/security_mode parameters are in your initial prompt. Parse them from there.
5. **STRONG PREFERENCE — DIRECT REVIEW**: Perform the review DIRECTLY by running `git diff`, reading files, and analyzing code yourself. Avoid spawning `csa run`/`csa review`/`csa debate` sub-agents unless the scope genuinely requires delegation (e.g., a 50K-line changeset that won't fit). Fractal recursion is allowed up to the configured ceiling (`project.max_recursion_depth`, default 5) and `pipeline::load_and_validate` enforces it, but a reviewer that nests more reviewers rarely adds value and complicates artifact attribution. When in doubt, read and analyze in-process.
6. **REVIEW-ONLY SAFETY**: Do NOT run `git add`, `git commit`, `git push`, `git merge`, `git rebase`, `git checkout`, `git reset`, `git stash`, or any `gh pr *` mutation command. Review mode must not mutate repo or PR state.
7. If the initial prompt contains `consistency_scope=touched-files`, extend consistency checks to bounded full content for touched files as defined in [Review Protocol](references/review-protocol.md). If it contains `consistency_scope=diff-only` or omits the parameter, keep consistency checks limited to the collected diff.

**Only if you are Claude Code and a human user typed `/csa-review` in the chat**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Run structured code reviews through CSA, ensuring:
- **Session isolation**: Review sessions stored in `~/.local/state/csa/`, not `~/.codex/`.
- **Independent model selection**: CSA automatically routes to an appropriate review tool based on configuration.
- **Self-contained review agent**: The review agent reads CLAUDE.md and builds project understanding autonomously.
- **Structured outputs**: JSON findings + Markdown report following a tested, optimized prompt.

## Required Inputs (Orchestrator)

- `scope`: one of:
  - `uncommitted` (default)
  - `base:<branch>` (e.g., `base:main`)
  - `commit:<sha>`
  - `range:<from>...<to>`
  - `files:<pathspec>`
- `mode` (optional): `review-only` (default) or `review-and-fix`
- `review_mode` (optional): `standard` (default) or `red-team`
- `security_mode` (optional): `auto` (default) | `on` | `off`
- `consistency_scope` (optional): `diff-only` (default) | `touched-files`
- `tool` (optional): override review tool (default: auto-detect independent reviewer)
- `context` (optional): path to `TODO.md` or `spec.toml` to check implementation alignment against the planned design

## SA Mode Propagation (MANDATORY)

When operating under SA mode (e.g., dispatched by `/sa` or any autonomous workflow),
**ALL `csa` invocations MUST include `--sa-mode true`**. This includes `csa run`,
`csa review`, `csa debate`, and any other execution commands. Omitting `--sa-mode`
at root depth causes a hard error; passing `false` when the caller is in SA mode
breaks prompt-guard propagation.

## Execution Protocol (ORCHESTRATOR ONLY — review agents MUST skip this section)

### Step 1: Determine Review Tool

The review tool is configured in `~/.config/cli-sub-agent/config.toml` under `[review]`:

```toml
[review]
tool = "auto"  # or "codex", "claude-code", "opencode"
```

**Auto mode** (default):
- Caller is `claude-code` -> review with `codex`
- Caller is `codex` -> review with `claude-code`
- Otherwise -> error with guidance to configure manually

Since this skill is designed to be invoked from Claude Code, the default auto behavior selects `codex` as the review tool.

If the user explicitly passes `tool`, use that instead.

### Step 1.5: Pre-PR Review Context Alignment

When the review scope covers `main...HEAD` (i.e., pre-PR review), the orchestrator MUST:

1. **Auto-detect associated context**: Run `csa todo find --branch $(git branch --show-current)` to find the plan for the current branch.
2. **Prefer spec.toml when available**: If the plan has `spec.toml`, pass it as `context` so the review agent can check explicit criteria; otherwise pass `TODO.md` when available.
3. **Continue when no context exists**: If no TODO/spec is found, continue the review normally. Alignment is best-effort, not a hard gate.

**Why**: Pre-PR reviews should align diff behavior with branch intent, but the branch may legitimately lack a stored TODO/spec artifact.

**Exception**: If the user explicitly provides `context=<path>`, skip auto-detection and use the provided path.

### Step 2: Build Review Prompt

Construct a comprehensive review prompt that the review agent will execute autonomously. The prompt includes all review instructions so the agent is fully self-contained.

**IMPORTANT**: The review agent reads CLAUDE.md itself. Do NOT read CLAUDE.md in the orchestrator and pass its content. The agent needs to build its own project understanding.

The review prompt instructs the agent to: read project context (CLAUDE.md + AGENTS.md), collect the diff for the given scope, perform a three-pass review (discovery, evidence filtering, adversarial security), apply Spec Alignment when `context` is `TODO.md` or `spec.toml`, switch to adversarial hypothesis generation when `review_mode=red-team`, and generate structured outputs.

> **See**: [Review Protocol](references/review-protocol.md) for the full agent instructions (scope commands, AGENTS.md compliance, three-pass review, non-negotiable rules).

> **See**: [Output Schema](references/output-schema.md) for the JSON findings schema and Markdown report template.

### Step 3: Execute Review via CSA

```bash
SID=$(csa run --sa-mode true --force-ignore-tier-setting --tool {review_tool} \
  --description "code-review: {scope}" \
  "{REVIEW_PROMPT}")
csa session wait --session "$SID"
```

Key behaviors:
- CSA manages the session in `~/.local/state/csa/` (not `~/.codex/`).
- The review agent has full autonomy: it reads CLAUDE.md, runs git commands, reads source files, and generates outputs.
- CSA handles concurrency control via global slots.
- The session is persistent and can be resumed for fixes.

### Step 4: Present Results

After CSA returns:
1. Read and display `$CSA_SESSION_DIR/reviewer-{N}/review-report.md` if generated.
2. Read and display `$CSA_SESSION_DIR/reviewer-{N}/review-findings.json` summary (finding count by priority).
3. Read and display AGENTS.md checklist summary (checked rule count and violation count).
4. Report the CSA session ID for potential follow-up.

### Step 5: Fix Mode (optional, when mode=review-and-fix)

If mode is `review-and-fix`, resume the same CSA session to fix all P0 and P1 issues, generate `$CSA_SESSION_DIR/reviewer-{N}/fix-summary.md` and `$CSA_SESSION_DIR/reviewer-{N}/post-fix-review-findings.json`, and mark any remaining P0/P1 as incomplete.

> **See**: [Fix Workflow](references/fix-workflow.md) for the full fix mode protocol and verification steps.

### Step 6: Verification (optional)

After fixes, optionally run:
```bash
just pre-commit
```
or trigger another review round to verify fixes.

## Example Usage

| Command | Effect |
|---------|--------|
| `/csa-review` | Auto-selects codex, reviews uncommitted changes, security_mode=auto |
| `/csa-review scope=base:main security_mode=on` | Reviews all changes since main with mandatory security pass |
| `/csa-review scope=uncommitted review_mode=red-team` | Reviews adversarially, focusing on breakage paths and counterexamples |
| `/csa-review scope=uncommitted mode=review-and-fix` | Reviews, then fixes P0/P1 in the same session |
| `/csa-review scope=uncommitted context=$(csa todo show -t <ts> --path)` | Reviews and checks alignment against a TODO plan |
| `/csa-review scope=uncommitted context=/abs/path/to/spec.toml` | Reviews against explicit criteria from `spec.toml` |
| `/csa-review tool=opencode scope=base:dev` | Uses opencode instead of auto-detected tool |

## Adjudication Protocol (Fix Mode Only)

When running in fix mode (`--fix` or `mode=review-and-fix`), the reviewer MUST attach explicit adjudication to each `Critical` and `High` severity finding using its stable `fid`.

Verdict options:
- `Accepted`: finding is valid and a fix is required.
- `Rejected`: finding is a false positive or otherwise invalid.
- `Deferred`: finding needs more context or human decision.

For each adjudicated finding, the reviewer MUST:
1. Provide exactly one verdict (`Accepted` | `Rejected` | `Deferred`).
2. Provide a rationale in 1-2 sentences that explains why the verdict is correct.
3. Emit the adjudication in the review output using machine-parseable markers so it can be persisted as `AdjudicationRecord`.

Required output block format:

```markdown
<!-- CSA:ADJUDICATION fid=<finding_id> verdict=accepted -->
Rationale: This unsafe block lacks a SAFETY comment and could cause UB.
<!-- CSA:ADJUDICATION:END -->
```

Notes:
- `verdict` values in markers MUST be lowercase: `accepted`, `rejected`, `deferred`.
- Non-fix mode behavior is unchanged; adjudication markers are required only in fix mode.

## Disagreement Escalation

When findings are contested, use the `debate` skill for adversarial arbitration. Findings must never be silently dismissed — every finding deserves independent evaluation.

Adjudication-specific escalation rule:
- If reviewer A marks a finding `Accepted` and reviewer B marks the same `fid` `Rejected`, that finding is automatically escalated and treated as `Deferred` until human review resolves it.
- This extends the existing consensus mechanism and prevents silent winner-takes-all resolution for disputed high-severity findings.

> **See**: [Disagreement Escalation](references/disagreement-escalation.md) for the full dispute resolution protocol.

## Review-then-Fix via `--fix` (CLI)

The `csa review` CLI has a built-in `--fix` flag that resumes the **same session**
to fix issues found during review. This is the recommended way to implement
the "reviewer fixes its own findings" pattern:

```bash
csa review --branch main --fix --max-rounds 3
```

**How it works:**
1. Review runs normally, identifies issues and emits a verdict.
2. If verdict is `HAS_ISSUES` and `--fix` is enabled, the reviewer session
   resumes with a fix prompt (same session, same context).
3. After each fix round, quality gates are re-evaluated.
4. Repeats up to `--max-rounds` (default: 3) or until gates pass.

**Key constraints:**
- `--fix` is **not supported** with `--reviewers > 1` (multi-reviewer consensus).
- The fix pass overrides `readonly_project_root` to `false` (the fix must write).
- The reviewer session is reused, preserving full context from the review phase.

### Review Session Metadata (`review_meta.json`)

After every `csa review` run, structured metadata is written to
`{session_dir}/review_meta.json` with the following fields:

```json
{
  "session_id": "01KM...",
  "head_sha": "abc123def456",
  "decision": "pass",
  "verdict": "CLEAN",
  "tool": "claude-code",
  "scope": "range:main...HEAD",
  "exit_code": 0,
  "fix_attempted": true,
  "fix_rounds": 1,
  "timestamp": "2026-03-22T05:30:00Z"
}
```

This metadata enables downstream consumers (pr-bot, commit skill,
orchestration scripts) to programmatically query review results without
parsing free-form text output. The `decision` field uses the five-value
`ReviewDecision` enum: `pass`, `fail`, `skip`, `uncertain`, `unavailable`.
`unavailable` means the reviewer infrastructure failed across all configured
tier models (for example quota/auth/network), while `uncertain` means the
reviewer ran but could not reach a confident conclusion. Legacy four-state
reviewer output (`CLEAN`, `HAS_ISSUES`, `SKIP`, `UNCERTAIN`) still parses for
backward compatibility.

When `--fix` is enabled, the metadata is updated after each fix round with
the latest verdict, exit code, and cumulative fix round count.

## References

| File | Purpose |
|------|---------|
| [references/review-protocol.md](references/review-protocol.md) | Full agent review instructions: project context, scope commands, AGENTS.md compliance, three-pass review, non-negotiable rules |
| [references/output-schema.md](references/output-schema.md) | JSON findings schema (`review-findings.json`) and Markdown report template (`review-report.md`) |
| [references/red-team-mode.md](references/red-team-mode.md) | Adversarial prompt fragment for `review_mode=red-team` |
| [references/fix-workflow.md](references/fix-workflow.md) | Fix mode protocol (Step 5) and verification (Step 6) for `review-and-fix` mode |
| [references/disagreement-escalation.md](references/disagreement-escalation.md) | Finding dispute resolution via `debate` skill with independent models |

## Done Criteria

1. Review prompt was sent to CSA with the correct tool.
2. CSA session was created in `~/.local/state/csa/` (verify with `csa session list`).
3. No sessions were created in `~/.codex/`.
4. **Recursion discipline**: nested `csa` calls from the review agent are permitted up to `project.max_recursion_depth` (default 5; enforced by `pipeline::load_and_validate`), but are unusual for a read-only review. If the review agent delegates, session tree depth should remain shallow and each nested call must justify itself (e.g., scope genuinely too large for a single agent).
5. Review agent read CLAUDE.md autonomously (not pre-fed by orchestrator).
6. Review agent discovered and applied AGENTS.md files (root-to-leaf) for all changed paths.
7. `$CSA_SESSION_DIR/reviewer-{N}/review-findings.json` and `$CSA_SESSION_DIR/reviewer-{N}/review-report.md` were generated.
8. Every finding has concrete evidence (trigger, expected, actual) and calibrated confidence. AGENTS.md violations reference rule IDs.
9. `review-findings.json` includes a complete `agents_md_checklist` with no missing applicable rules.
10. `review-report.md` includes AGENTS.md checklist section with all items checked.
11. If security_mode required pass 3, adversarial_pass_executed=true.
12. If `context` was `spec.toml`, every criterion is either supported by evidence or surfaced as `spec-deviation` / `unverified-criterion`.
13. If `review_mode=red-team`, `review-findings.json` contains `review_mode: "red-team"` and keeps the standard finding schema.
14. If mode=review-and-fix, fix artifacts exist and session was resumed (not new).
15. If mode=review-and-fix, every `Critical`/`High` finding includes one adjudication block with `fid`, `verdict`, and 1-2 sentence rationale.
16. CSA session ID was reported for potential follow-up.
17. **If any finding was contested**: debate skill was used with independent models, and outcome documented with model specs.
