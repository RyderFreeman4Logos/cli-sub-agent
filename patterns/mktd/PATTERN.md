---
name = "mktd"
description = "Make TODO: CSA-powered reconnaissance, adversarial debate, and structured TODO plan generation"
allowed-tools = "TaskCreate, TaskUpdate, TaskList, TaskGet, Read, Grep, Glob, Bash, Write, Edit"
tier = "tier-2-standard"
version = "0.1.0"
---

# mktd: Make TODO — Debate-Enhanced Planning

Four-phase planning: RECON (CSA parallel exploration), DRAFT (synthesize TODO),
DEBATE (adversarial review), APPROVE (user gate).

Zero main-agent file reads during exploration. CSA sub-agents gather context.
Mandatory adversarial review catches blind spots.

## Step 1: Phase 1 — RECON Dimension 1 (Structure)

Tool: csa
Tier: tier-1-quick

Analyze codebase structure relevant to ${FEATURE}.
Report: relevant files (path + purpose, max 20), key types, module dependencies, entry points.
Working directory: ${CWD}

## Step 2: Phase 1 — RECON Dimension 2 (Patterns)

Tool: csa
Tier: tier-1-quick

Find existing patterns or similar features to ${FEATURE} in this codebase.
Report: file paths with approach, reusable components, conventions to follow.
Working directory: ${CWD}

## Step 3: Phase 1 — RECON Dimension 3 (Constraints)

Tool: csa
Tier: tier-1-quick

Identify constraints and risks for implementing ${FEATURE}.
Report: potential breaking changes, security considerations, performance, compatibility.
Working directory: ${CWD}

## Step 4: Phase 2 — DRAFT TODO

Synthesize CSA findings into a structured TODO plan.
Each item is a [ ] checkbox with executor tag.
Write all TODO descriptions, section headers, and task names in ${USER_LANGUAGE}.
Technical terms, code snippets, commit scope strings, and executor tags ([Main], [Sub:developer], [Skill:commit], [CSA:tool]) remain in English.
If USER_LANGUAGE is empty or unset, default to the language used in the FEATURE description.
Pre-assign executors: [Main], [Sub:developer], [Skill:commit], [CSA:tool].

**Output**: Print the COMPLETE TODO plan as text to stdout.
Do NOT write files to the project directory.
The output is captured as `${STEP_4_OUTPUT}` for subsequent steps.

## Step 5: Phase 2.5 — Threat Model

For each new API surface in the TODO plan (config fields, CLI inputs,
stored data, external interactions), enumerate:

- What sensitive data flows through this path?
- What happens with malformed/hostile input?
- What information is exposed in logs/display/persistence?
- What default behavior is safe vs unsafe?

Append threat findings as a "Security Considerations" section to the
TODO plan. Each finding becomes a checkbox item tagged [Security].

**Input**: `${STEP_4_OUTPUT}` (the draft TODO from Step 4).
**Output**: Print the COMPLETE threat analysis as text to stdout.

## Step 6: Phase 3 — Adversarial Debate

Tool: csa
Tier: tier-2-standard

## INCLUDE debate

Mandatory adversarial review of the TODO draft and threat model.
No exceptions — even "simple" plans benefit from challenge.

## Step 7: Revise TODO

Incorporate debate feedback and threat model findings. Update plan
based on valid criticisms. Concede valid points, defend sound decisions
with evidence.

**Output**: Print the COMPLETE revised TODO plan as text to stdout.
Do NOT write files to the project directory.
The output is captured as `${STEP_7_OUTPUT}` for the save step.

## Step 8: Save TODO

Tool: bash

Save finalized TODO using csa todo for git-tracked lifecycle.
Uses `${STEP_7_OUTPUT}` (the revised TODO from Step 7).

```bash
[[ -n "${STEP_7_OUTPUT:-}" ]] || { echo "STEP_7_OUTPUT is empty — Step 7 (revise) must output the finalized TODO as text" >&2; exit 1; }
TODO_TS=$(csa todo create --branch "$(git branch --show-current)" -- "${FEATURE}") || { echo "csa todo create failed" >&2; exit 1; }
TODO_PATH=$(csa todo show -t "${TODO_TS}" --path) || { echo "csa todo show failed" >&2; exit 1; }
printf '%s\n' "${STEP_7_OUTPUT}" > "${TODO_PATH}" || { echo "write TODO failed" >&2; exit 1; }
csa todo save -t "${TODO_TS}" "finalize: ${FEATURE}"
```

## Step 9: Phase 4 — User Approval

Present TODO to user for review in ${USER_LANGUAGE}.
User chooses: APPROVE → proceed to mktsk, MODIFY → revise, REJECT → abandon.
