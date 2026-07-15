# CSA Prompt Crafting Guide

Distilled from Anthropic and OpenAI's 2025-2026 prompt engineering research.
Applies to writing prompts for `csa run`, `csa review`, and `csa debate` dispatch.

## Core Principle: Context Is a Finite, Degrading Resource

Every token in the prompt competes for attention budget. CSA sessions have a
fixed cold-start cost (~10-60K tokens for rules/context). Your prompt must
maximize signal-to-noise ratio within that budget.

**Rule of thumb**: If removing a sentence from the prompt wouldn't cause
a mistake, remove it.

## Prompt Structure (Recommended Order)

```
1. ROLE + OBJECTIVE   (1-2 sentences: who you are, what to accomplish)
2. CONTEXT            (files, prior findings, constraints — long content goes HERE)
3. INSTRUCTIONS       (step-by-step for GPT/Codex; outcome-oriented for Claude/reasoning)
4. BOUNDARIES         (what NOT to do — negative constraints)
5. OUTPUT FORMAT      (expected deliverables, sections, commit conventions)
6. VERIFICATION       (how to check your own work before finishing)
```

Long reference material (file contents, prior session output) goes in section 2,
NOT at the end. Queries and instructions after context improves quality by up to 30%.

## The Three Agentic Anchors

OpenAI's research shows three system-level instructions raise agent task
completion by ~20%. Include all three in every CSA dispatch prompt:

1. **Persistence**: "Keep working until the task is fully resolved. Do not
   stop to ask questions or hand back partial results."
2. **Tool-calling**: "Use tools to verify — do not guess file contents,
   test results, or git state. Read before writing."
3. **Planning**: "Plan before each action. After each tool result, reflect
   on whether the result advances the goal before proceeding."

## Positive Instructions > Negative Instructions

Positive instructions ("Write flowing prose") outperform negative ones
("Don't use bullet points") because they point the model toward a specific
region of output space rather than away from one.

**Pattern**: State the positive instruction first, then add the negative
constraint as a guardrail.

```
Good:  "Commit each logical milestone. Do not batch all changes to end."
Bad:   "Do not batch all changes to end."
```

However, negative constraints ARE valuable for eliminating high-probability
failure modes — use them as guardrails, not as primary direction.

## Match Prompting Style to Model

| Model Type | Prompting Style |
|------------|----------------|
| GPT-4.1/Codex | Explicit step-by-step; prescriptive; planning instructions |
| Claude 4.x | Outcome-oriented; explain WHY; fewer prescriptive steps |
| Reasoning (o1/o3) | High-level constraints only; DO NOT add "think step by step" |

**Reasoning models** already think internally. Adding chain-of-thought
instructions is redundant and may hurt performance. Instead, specify
constraints and success criteria; let the model figure out the approach.

**GPT/Codex** benefits from explicit planning prompts:
"Plan extensively before each function call. Reflect on outcomes of
previous calls before proceeding."

## Sub-Agent Prompt Requirements (The Four Essentials)

Every CSA dispatch MUST include:

1. **Specific objective**: What exactly to accomplish (not "fix bugs")
2. **Input context**: What the agent needs to know (issue description,
   prior findings, file paths — but do NOT pre-fetch file contents)
3. **Output format**: Expected deliverables (commits, report, test results)
4. **Scope boundaries**: What files/directories to touch, what to leave alone

```
Good prompt:
  "Fix GitHub issue #1234: bwrap fails on file paths.
   The bug is in crates/csa-resource/src/bwrap.rs — extra_writable
   paths that are files (not directories) cause 'Is a directory' error.
   Fix the path detection logic, add tests, run just pre-commit,
   commit with Conventional Commits scope."

Bad prompt:
  "Fix the bwrap bug."
```

## Defense in Depth for Constraints

Critical constraints need three layers:

1. **Prompt instruction** (probabilistic — shifts model behavior)
2. **Sandbox enforcement** (deterministic — blocks violations)
3. **Post-exec verification** (detective — catches what slipped through)

Example from RECON phases:
- Prompt: "READ-ONLY. Do NOT edit any files."
- Sandbox: `workspace_access = "read-only"`
- Post-exec: git diff check for unexpected changes

## Output Contracts

Define exactly what sections the response must contain. This creates
predictable, parseable output and reduces omissions.

```
"Your output MUST include:
 1. Root cause analysis (2-3 sentences)
 2. Fix description (what changed and why)
 3. Files modified (list)
 4. Test coverage (what tests added/modified)
 Commit with Conventional Commits format."
```

## Verification Loop (Highest-Leverage Pattern)

The single highest-leverage addition to any prompt is a verification step.
Without it, the model has no feedback loop except the caller's review.

```
"Before finishing:
 - Run cargo check to verify compilation
 - Run cargo test for affected crate
 - Run just pre-commit
 - Verify git status is clean
 Commit only after all checks pass."
```

## Early-Stop Criteria

Prevent over-searching by setting explicit discovery limits:

```
"Stop searching once you find the relevant function.
 Maximum 5 file reads for initial exploration.
 If the fix is clear after reading 2-3 files, proceed to implementation."
```

## Context Isolation via Sub-Agents

For research-heavy tasks, delegate exploration to sub-agents that return
condensed summaries (1-2K tokens). The main session stays clean.

**Anti-pattern**: One session that reads 50 files, fills its context, then
tries to implement a fix with degraded attention.

**Pattern**: Research sub-agent reads files → returns summary → implementation
sub-agent gets clean context + summary.

## Common Anti-Patterns

### Over-Prompting Newer Models
```
Bad:  "CRITICAL: You MUST use this tool. ALWAYS check. NEVER skip."
Good: "Use this tool when you need X."
```
Aggressive language causes overtriggering on Claude 4.5+ and GPT-5+.

### Laundry Lists of Edge Cases
```
Bad:  20 bullet points of edge cases
Good: 3 diverse examples showing the pattern
```
Examples communicate patterns more efficiently than exhaustive rules.

### Kitchen Sink Sessions
```
Bad:  One session doing research + implementation + review + commit
Good: Research session → Implementation session → Review session
```
Each session gets clean context for its phase.

### Contradictory Instructions
```
Bad:  "Be thorough. Also be concise. Cover every edge case. Keep it short."
Good: "Provide a concise fix with tests for the reported case and one edge case."
```
GPT-5/Claude 4.6+ spend reasoning tokens trying to reconcile contradictions
instead of working on the task.

## XML Tags for Disambiguation (Claude-Specific)

Claude models strongly prefer XML tags for separating content types:

```
<objective>Fix the authentication timeout bug</objective>
<context>
  Issue #456 reports that OAuth tokens expire during long operations.
  The timeout is hardcoded at 30s in auth_handler.rs:142.
</context>
<constraints>
  - Do not change the public API signature
  - Must remain backward-compatible with existing config files
</constraints>
```

## Tool Description Quality

Tool definitions steer agent behavior as much as system prompts.
When defining CSA-dispatched tools or writing prompts that reference tools:

- Make implicit context explicit (what the tool does AND what it doesn't)
- Clarify boundaries between similar tools
- Use semantically meaningful identifiers (not UUIDs)
- Include 1-3 realistic usage examples for complex tools

## Module Token Budget

Per-file token budgets are an engineering constraint, not a style preference.

### Quantitative Basis

| Fact | Value | Source |
|------|-------|--------|
| Context rot onset | ~50K tokens (200K window) | Chroma Research 2025 |
| Reliable utilization | 50-65% of window | Chroma Research 2025 |
| System overhead (rules/tools/memory/skills) | 50-70K tokens | Measured |
| 1000 lines of code | ~10,000 tokens | Wire Blog 2025 |
| AI defect risk increase (code health < 9.0) | >= 30% | Borg & Tornhill 2025 |
| CLAUDE.md compliance drop | > 200 lines | DEV Community 2026 |
| AI token waste on locating vs solving | 80% on locating | Nesler 2025 |

### The 8K Budget

CSA enforces a per-file target of 8K tokens (pre-commit gate). Rationale:
- 200K window - 60K overhead = 140K effective. Multi-turn tool calls halve
  that to ~70K working context. At 8K per file, an agent can hold ~8 files
  simultaneously with room for reasoning.
- 8K tokens ≈ 800 lines of Rust. Aligns with CSA's existing 800-line soft
  module limit and clippy cognitive_complexity thresholds.

Files exceeding 8K tokens are blocked by pre-commit. Exempted files (test,
generated, macro-heavy) still produce a WARNING.

### Interface-First Interaction

Interface documentation is 10x more token-efficient than source code.
When interacting with modules outside your current scope:

1. **First**: Use MCP/skills or LSP to understand the API
2. **Second**: Read doc comments and type signatures
3. **Last resort**: Read implementation source (only when docs are insufficient)

This is enforced by AGENTS.md Rule 063 (interface-first).

### When to Split vs Exempt

**Split when**: file has high token count AND high cognitive complexity.
Pure data definitions (structs, enums, match tables) with low complexity
are acceptable at higher token counts.

**Exempt when**: test files (`cfg(test)`), generated code, parser combinators,
macro definitions. These have inherently high token density but splitting
reduces readability.

## Sources

Anthropic: Claude 4 Best Practices, Context Engineering for Agents,
Building Effective Agents, Writing Tools for Agents, Agent Skills,
Reduce Hallucinations guide.

OpenAI: GPT-4.1 Prompting Guide, GPT-5 Prompting Guide, Prompt Guidance
(GPT-5.x), Reasoning Best Practices, Codex Prompting Guide, AGENTS.md Guide.
