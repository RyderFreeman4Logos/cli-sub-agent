---
name = "mktd"
description = "Make TODO: CSA-powered reconnaissance, adversarial debate, and structured TODO plan generation"
allowed-tools = "TaskCreate, TaskUpdate, TaskList, TaskGet, Read, Grep, Glob, Bash, Write, Edit"
tier = "tier-2-standard"
version = "0.1.0"
---

# mktd: Make TODO — Debate-Enhanced Planning

Five-phase planning: RECON (CSA parallel exploration), DRAFT (synthesize TODO),
THREAT MODEL (security review), DEBATE (adversarial review), APPROVE (user gate).

Zero main-agent file reads during exploration. CSA sub-agents gather context.
Mandatory adversarial review catches blind spots.

## Step 0.6: Phase 1.5 — Language Detection

Tool: bash

Resolve planning language for TODO content with deterministic priority:
1) `${USER_LANGUAGE}` override
2) `${CSA_USER_LANGUAGE}` environment
3) detect from `${FEATURE}` text
4) fallback to English

```bash
if [[ -n "${USER_LANGUAGE:-}" ]]; then
  printf '%s\n' "${USER_LANGUAGE}"
  exit 0
fi
if [[ -n "${CSA_USER_LANGUAGE:-}" ]]; then
  printf '%s\n' "${CSA_USER_LANGUAGE}"
  exit 0
fi
if printf '%s' "${FEATURE:-}" | rg -q '[\p{Han}]'; then
  echo "Chinese (Simplified)"
  exit 0
fi
if printf '%s' "${FEATURE:-}" | rg -q '[\p{Hiragana}\p{Katakana}]'; then
  echo "Japanese"
  exit 0
fi
echo "English"
```

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
Write all TODO descriptions, section headers, and task names in `${STEP_50_OUTPUT}`.
Technical terms, code snippets, commit scope strings, and executor tags ([Main], [Sub:developer], [Skill:commit], [CSA:tool]) remain in English.
Pre-assign executors: [Main], [Sub:developer], [Skill:commit], [CSA:tool].
Every checkbox item MUST include a mechanically verifiable `DONE WHEN:` line.

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

Tool: bash
Tier: tier-2-standard

## INCLUDE debate

Mandatory adversarial review of the TODO draft and threat model.
No exceptions — even "simple" plans benefit from challenge.
Capture debate stdout, then normalize into a structured evidence packet
for mechanical validation.

```bash
LANGUAGE="${STEP_50_OUTPUT:-English}"
DEBATE_PROMPT="$(printf '%s\n' \
"Critically evaluate this draft TODO plan and threat model. Act as a devil's advocate." \
"" \
"## Draft TODO Plan" \
"${STEP_4_OUTPUT}" \
"" \
"## Threat Model (Step 5)" \
"${STEP_5_OUTPUT}" \
"" \
"## Output Requirements" \
"Provide explicit verdict and confidence in your conclusion." )"
DEBATE_SUMMARY="$(printf '%s\n' "${DEBATE_PROMPT}" | csa debate --rounds 3)" || { echo "csa debate failed" >&2; exit 1; }
[[ -n "${DEBATE_SUMMARY:-}" ]] || { echo "empty debate summary" >&2; exit 1; }
RAW_VERDICT="$(printf '%s\n' "${DEBATE_SUMMARY}" | grep '^Debate verdict:' | tail -n1 | sed -nE 's/^Debate verdict:[[:space:]]*([A-Z]+).*/\1/p')"
case "${RAW_VERDICT}" in
  APPROVE) MAPPED_VERDICT="READY" ;;
  REVISE|REJECT) MAPPED_VERDICT="REVISE" ;;
  *) MAPPED_VERDICT="REVISE" ;;
esac
CONFIDENCE="$(printf '%s\n' "${DEBATE_SUMMARY}" | grep '^Debate verdict:' | tail -n1 | sed -nE 's/^Debate verdict:[^\(]*\(confidence:[[:space:]]*([a-zA-Z]+)\).*/\1/p')"
printf '%s\n' "DEBATE_EVIDENCE:"
printf '%s\n' "- method: csa debate"
printf '%s\n' "- rounds: 3"
printf '%s\n' "- language: ${LANGUAGE}"
printf '%s\n' "- raw_verdict: ${RAW_VERDICT:-UNKNOWN}"
printf '%s\n' "- mapped_verdict: ${MAPPED_VERDICT}"
printf '%s\n' "- confidence: ${CONFIDENCE:-unknown}"
printf '%s\n' "VALID_CONCERNS:"
printf '%s\n' "- Review debate summary + threat model findings before approval."
printf '%s\n' "SUGGESTED_CHANGES:"
if [ "${MAPPED_VERDICT}" = "REVISE" ]; then
  printf '%s\n' "- Revise TODO tasks based on debate objections and risk findings."
else
  printf '%s\n' "- Keep current plan; no blocking objections detected by debate."
fi
printf '%s\n' "OVERALL_ASSESSMENT:"
printf '%s\n' "${DEBATE_SUMMARY}"
```

## Step 6.5: Phase 3.5 — Validate Debate Evidence

Tool: bash

Debate output MUST contain required evidence markers.

```bash
[[ -n "${STEP_6_OUTPUT:-}" ]] || { echo "STEP_6_OUTPUT is empty — debate did not run" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^DEBATE_EVIDENCE:' || { echo "debate evidence header missing" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -Eq 'mapped_verdict:[[:space:]]*(READY|REVISE)' || { echo "mapped debate verdict missing" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -Eq 'raw_verdict:[[:space:]]*(APPROVE|REVISE|REJECT|UNKNOWN)' || { echo "raw debate verdict missing" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -Eq 'confidence:[[:space:]]*(high|medium|low|unknown)' || { echo "debate confidence missing" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^VALID_CONCERNS:' || { echo "valid concerns section missing" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^SUGGESTED_CHANGES:' || { echo "suggested changes section missing" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^OVERALL_ASSESSMENT:' || { echo "overall assessment section missing" >&2; exit 1; }
```

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
Execute ONLY the command block below.
FORBIDDEN: custom shell snippets, heredoc (`<<EOF`, `cat <<`), branch create/switch,
and writing intermediate files outside the TODO path.

```bash
[[ -n "${STEP_7_OUTPUT:-}" ]] || { echo "STEP_7_OUTPUT is empty — Step 7 (revise) must output the finalized TODO as text" >&2; exit 1; }
printf '%s\n' "${STEP_7_OUTPUT}" | grep -qE '^- \[ \] .+' || { echo "STEP_7_OUTPUT has no non-empty checkbox tasks" >&2; exit 1; }
printf '%s\n' "${STEP_7_OUTPUT}" | grep -q 'DONE WHEN:' || { echo "STEP_7_OUTPUT has no DONE WHEN clauses" >&2; exit 1; }
RESOLVED_LANGUAGE="${STEP_50_OUTPUT:-English}"
if printf '%s' "${RESOLVED_LANGUAGE}" | grep -qi 'chinese'; then
  HAN_COUNT_DRAFT=$(printf '%s\n' "${STEP_7_OUTPUT}" | rg -o '[\p{Han}]' | wc -l | tr -d '[:space:]')
  [[ "${HAN_COUNT_DRAFT:-0}" -ge 30 ]] || { echo "STEP_7_OUTPUT language mismatch: expected Chinese content" >&2; exit 1; }
fi
CURRENT_BRANCH=$(git branch --show-current) || { echo "detect branch failed" >&2; exit 1; }
TODO_TS=$(csa todo create --branch "${CURRENT_BRANCH}" -- "${FEATURE}") || { echo "csa todo create failed" >&2; exit 1; }
TODO_PATH=$(csa todo show -t "${TODO_TS}" --path) || { echo "csa todo show failed" >&2; exit 1; }
printf '%s\n' "${STEP_7_OUTPUT}" > "${TODO_PATH}" || { echo "write TODO failed" >&2; exit 1; }
[[ -s "${TODO_PATH}" ]] || { echo "saved TODO is empty" >&2; exit 1; }
grep -qE '^- \[ \] .+' "${TODO_PATH}" || { echo "saved TODO has no non-empty checkbox tasks" >&2; exit 1; }
grep -q 'DONE WHEN:' "${TODO_PATH}" || { echo "saved TODO has no DONE WHEN clauses" >&2; exit 1; }
if printf '%s' "${RESOLVED_LANGUAGE}" | grep -qi 'chinese'; then
  HAN_COUNT_SAVED=$(rg -o '[\p{Han}]' "${TODO_PATH}" | wc -l | tr -d '[:space:]')
  [[ "${HAN_COUNT_SAVED:-0}" -ge 30 ]] || { echo "saved TODO language mismatch: expected Chinese content" >&2; exit 1; }
fi
csa todo save -t "${TODO_TS}" "finalize: ${FEATURE}"
csa todo show -t "${TODO_TS}" --path
```

## Step 9: Phase 4 — User Approval

Present TODO to user for review in `${STEP_50_OUTPUT}`.
User chooses: APPROVE → proceed to mktsk, MODIFY → revise, REJECT → abandon.
