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

**Check your initial prompt.** If it contains the literal string `"Use the mktd skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/mktd/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Generate a structured TODO plan for a feature through four phases: parallel CSA reconnaissance (structure, patterns, constraints), draft synthesis, mandatory adversarial debate review, and user approval gate. The main agent performs zero file reads during exploration -- CSA sub-agents gather all context. Plans are saved via `csa todo` for git-tracked lifecycle management.

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
2. **Phase 1.5 -- LANGUAGE DETECTION**: Detect the primary language used in conversation with the user. Set USER_LANGUAGE accordingly (e.g., "Chinese (Simplified)", "English", "Japanese"). If unclear, default to the language used in the FEATURE description. This language will be used for all TODO descriptions, section headers, and task names.
3. **Phase 2 -- DRAFT**: Synthesize CSA findings into a structured TODO plan with checkbox items, executor tags ([Main], [Sub:developer], [Skill:commit], [CSA:tool]), and descriptions in USER_LANGUAGE. Technical terms, code snippets, commit scope strings, and executor tags remain in English. Output the complete plan as text (stdout) -- do NOT write files to the project directory.
4. **Phase 3 -- DEBATE**: Run `csa debate` (tier-2) to adversarially review the TODO draft. Mandatory -- no exceptions.
5. **Phase 3b -- REVISE**: Incorporate debate feedback. Concede valid points, defend sound decisions. Output the complete revised plan as text (stdout).
6. **Phase 4 -- SAVE**: Save TODO via `csa todo create --branch <branch>`, pipe `${STEP_6_OUTPUT}` to the TODO file, `csa todo save`.
7. **Phase 4b -- APPROVE**: Present to user for APPROVE / MODIFY / REJECT.

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
4. Adversarial debate completed with at least one exchange.
5. TODO revised to incorporate valid debate feedback.
6. TODO saved via `csa todo create` + `csa todo save` with branch association.
7. User presented with plan for approval decision.
