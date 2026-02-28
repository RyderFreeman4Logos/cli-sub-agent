---
name: mktd
description: Debate-enhanced TODO plan generation with CSA reconnaissance and adversarial review
allowed-tools: Bash, Read, Grep, Glob, Write, Edit
triggers:
  - "mktd"
  - "/mktd"
  - "make todo"
  - "create plan"
  - "plan feature"
---

# mktd: Make TODO -- Debate-Enhanced Planning

## Role Detection (READ THIS FIRST -- MANDATORY)

Role MUST be determined by explicit mode marker, not fragile natural-language substring matching.
Treat the run as executor ONLY when initial prompt contains:
`<skill-mode>executor</skill-mode>`.

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/mktd/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Generate a structured TODO plan for a feature through five phases: parallel CSA reconnaissance (structure, patterns, constraints), draft synthesis, security threat model, mandatory adversarial debate review, and user approval gate. The main agent performs zero file reads during exploration -- CSA sub-agents gather all context. Plans are saved via `csa todo` for git-tracked lifecycle management.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- Must be in a git repository with a feature branch checked out

### Quick Start

```bash
csa run --skill mktd "Plan the implementation of <feature description>"
```

### Step-by-Step

1. **Phase 1 -- RECON** (3 parallel CSA calls, tier-1):
   - **Dimension 1 (Structure)**: Analyze codebase structure relevant to the feature (files, types, dependencies, entry points).
   - **Dimension 2 (Patterns)**: Find existing similar features or reusable components.
   - **Dimension 3 (Constraints)**: Identify breaking changes, security risks, performance concerns.
   - Main agent MUST NOT use Read/Glob/Grep/Bash for exploration.
2. **Phase 1.5 -- LANGUAGE DETECTION**: Resolve language by priority: `${USER_LANGUAGE}` override -> `${CSA_USER_LANGUAGE}` env -> script-aware detect from `${FEATURE}` -> preserve feature language when script is unknown -> fallback English only when `${FEATURE}` is empty. This language is captured as `${STEP_50_OUTPUT}`.
3. **Phase 2 -- DRAFT**: Synthesize CSA findings into a structured TODO plan with checkbox items, executor tags ([Main], [Sub:developer], [Skill:commit], [CSA:tool]), and descriptions in `${STEP_50_OUTPUT}`. Every task MUST include a mechanically verifiable `DONE WHEN:` line. Technical terms, code snippets, commit scope strings, and executor tags remain in English.
4. **Phase 2.5 -- THREAT MODEL**: Review each new API surface for security concerns (sensitive data flows, hostile input, information exposure, safe defaults). Append findings as [Security] tagged items.
5. **Phase 3 -- DEBATE**: Run explicit `csa debate` (tier-2) via bash step, then normalize stdout into an evidence packet with headers: `DEBATE_EVIDENCE`, `VALID_CONCERNS`, `SUGGESTED_CHANGES`, `OVERALL_ASSESSMENT`.
6. **Phase 3.5 -- DEBATE VALIDATION**: Hard-fail if required evidence headers, mapped verdict (`READY|REVISE`), raw verdict (`APPROVE|REVISE|REJECT|UNKNOWN`), or confidence are missing.
7. **Phase 3b -- REVISE**: Incorporate debate feedback and threat model findings. Concede valid points, defend sound decisions. Output the complete revised plan as text (stdout).
8. **Phase 4 -- SAVE**: Save TODO via `csa todo create --branch <branch>`, write `${STEP_7_OUTPUT}` to TODO file, then `csa todo save`. Save step MUST validate non-empty checkbox tasks, `DONE WHEN` clauses, and language consistency (Chinese mode requires Chinese content).
9. **Phase 4b -- APPROVE**: Present to user in `${STEP_50_OUTPUT}` for APPROVE / MODIFY / REJECT.

## Example Usage

| Command | Effect |
|---------|--------|
| `/mktd global concurrency slots` | Plan implementation of global concurrency slot feature |
| `/mktd "ACP transport layer"` | Plan ACP transport implementation with debate review |

## Integration

- **Uses**: `debate` (Phase 3 adversarial review)
- **Feeds into**: `mktsk` (converts approved TODO into executable Task entries)
- **Lifecycle**: Plans managed by `csa todo` (create, show, save, find)

## Done Criteria

1. Three RECON dimensions completed via CSA (structure, patterns, constraints).
2. Main agent performed zero file reads during Phase 1.
3. TODO draft synthesized with executor tags and checkbox items.
4. Threat model completed for all new API surfaces.
5. Adversarial debate completed via explicit `csa debate`.
6. Debate evidence packet validated (includes mapped verdict, raw verdict, and confidence).
7. TODO revised to incorporate debate feedback and threat model findings.
8. TODO saved via `csa todo create` + `csa todo save` with branch association.
9. Save gate validated task completeness (`- [ ] ...`, `DONE WHEN`) and language consistency.
10. User presented with plan for approval decision in resolved language.
