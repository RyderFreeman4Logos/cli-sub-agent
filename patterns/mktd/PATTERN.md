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

## Step 0: Phase 0.5 — Auto Session Discovery

Tool: bash

Find a reusable mktd session for the current project/branch.
Rules:

- Match branch + task_type=plan + phase=Available
- If found, output only the most recent session_id
- If not found (or not in git/detached HEAD), output empty string

```bash
command -v python3 >/dev/null 2>&1 || { echo ""; exit 0; }
python3 - <<'PY'
import json
import subprocess
import sys

def run(cmd):
    return subprocess.check_output(cmd, text=True).strip()

try:
    branch = run(["git", "rev-parse", "--abbrev-ref", "HEAD"])
except Exception:
    print("")
    sys.exit(0)

if not branch or branch == "HEAD":
    print("")
    sys.exit(0)

try:
    output = run(["csa", "session", "list", "--branch", branch, "--format", "json"])
    sessions = json.loads(output)
except Exception:
    print("")
    sys.exit(0)

for session in sessions:
    if session.get("task_type") == "plan" and session.get("phase") == "Available":
        print(session.get("session_id", ""))
        break
else:
    print("")
PY
```

## Step 1: Phase 1.5 — Language Detection

Tool: bash

Resolve planning language for TODO content.
Priority:
1) Explicit `${USER_LANGUAGE}`
2) Environment `${CSA_USER_LANGUAGE}`
3) Script-aware detection from `${FEATURE}`
4) Default to Chinese (Simplified) when script is mixed/unknown
5) Fallback to Chinese (Simplified) when `${FEATURE}` is empty

```bash
if [[ -n "${USER_LANGUAGE:-}" ]]; then
  printf '%s\n' "${USER_LANGUAGE}"
  exit 0
fi
if [[ -n "${CSA_USER_LANGUAGE:-}" ]]; then
  printf '%s\n' "${CSA_USER_LANGUAGE}"
  exit 0
fi
if printf '%s' "${FEATURE:-}" | rg -q '[\p{Hiragana}\p{Katakana}]'; then
  echo "Japanese"
  exit 0
fi
if printf '%s' "${FEATURE:-}" | rg -q '[\p{Hangul}]'; then
  echo "Korean"
  exit 0
fi
if printf '%s' "${FEATURE:-}" | rg -q '[\p{Arabic}]'; then
  echo "Arabic"
  exit 0
fi
if printf '%s' "${FEATURE:-}" | rg -q '[\p{Cyrillic}]'; then
  echo "Russian"
  exit 0
fi
if printf '%s' "${FEATURE:-}" | rg -q '[\p{Devanagari}]'; then
  echo "Hindi"
  exit 0
fi
if printf '%s' "${FEATURE:-}" | rg -q '[\p{Han}]'; then
  echo "Chinese (Simplified)"
  exit 0
fi
if [[ -n "${FEATURE:-}" ]]; then
  echo "Chinese (Simplified)"
  exit 0
fi
echo "Chinese (Simplified)"
```

## Step 2: Phase 1 — RECON Dimension 1 (Structure)

Tool: csa
Session: ${STEP_0_OUTPUT}
Tier: tier-1-quick

Analyze codebase structure relevant to ${FEATURE}.
Report: relevant files (path + purpose, max 20), key types, module dependencies, entry points.
Working directory: ${CWD}

## Step 3: Phase 1 — RECON Dimension 2 (Patterns)

Tool: csa
Session: ${STEP_0_OUTPUT}
Tier: tier-1-quick

Find existing patterns or similar features to ${FEATURE} in this codebase.
Report: file paths with approach, reusable components, conventions to follow.
Working directory: ${CWD}

## Step 4: Phase 1 — RECON Dimension 3 (Constraints)

Tool: csa
Session: ${STEP_0_OUTPUT}
Tier: tier-1-quick

Identify constraints and risks for implementing ${FEATURE}.
Report: potential breaking changes, security considerations, performance, compatibility.
Working directory: ${CWD}

## Step 5: Phase 2 — DRAFT TODO

Synthesize CSA findings into a structured TODO plan.

### RECON Findings (from prior steps)

### Structure (Step 2)
${STEP_2_OUTPUT}

### Patterns (Step 3)
${STEP_3_OUTPUT}

### Constraints (Step 4)
${STEP_4_OUTPUT}

### Instructions

Each item is a [ ] checkbox with executor tag.
Write all TODO descriptions, section headers, and task names in `${STEP_1_OUTPUT}`.
Technical terms, code snippets, commit scope strings, and executor tags remain in English.
Pre-assign executors: [Main], [Sub:developer], [Skill:commit], [CSA:tool].
Every checkbox item MUST include a mechanically verifiable `DONE WHEN:` line.

### Output

Output the COMPLETE TODO plan as text to stdout.
Do NOT write files to the project directory.
The output is captured as `${STEP_5_OUTPUT}` for subsequent steps.

## Step 6: Phase 2.25 — Spec Generation

Extract verifiable acceptance criteria from the draft TODO plan.

### Draft TODO Plan
${STEP_5_OUTPUT}

### Instructions

Only keep criteria that can be mechanically checked after implementation.
Classify each criterion using `SpecCriterion.kind`:

- `scenario`: end-to-end behavior or user-visible flow
- `property`: invariant, guarantee, or rule that must always hold
- `check`: concrete validation command, artifact, or presence check

Write criterion descriptions in `${STEP_1_OUTPUT}`.
Write `summary` as exactly one Chinese line suitable for the `CSA-Criteria`
commit trailer, even when the TODO plan uses another language.
Output the COMPLETE `spec.toml` content as TOML using this shape:

```toml
schema_version = 1
plan_ulid = "__PLAN_ID__"
summary = "<one-line Chinese summary for commit trailer>"

[[criteria]]
kind = "scenario"
id = "S1"
description = "<verifiable acceptance criterion description>"
status = "pending"
```

Use `plan_ulid = "__PLAN_ID__"` as a placeholder. Step 11 must replace it with
the actual plan id returned by `csa todo create` before writing `spec.toml`.
Do NOT write files to the project directory.
The output is captured as `${STEP_6_OUTPUT}` for subsequent steps.

## Step 7: Phase 2.5 — Threat Model

Review the draft TODO plan for security and safety concerns.

### Draft TODO Plan
${STEP_5_OUTPUT}

### Instructions

For each new API surface in the plan (config fields, CLI inputs, stored data,
external interactions), enumerate:

- What sensitive data flows through this path?
- What happens with malformed/hostile input?
- What information is exposed in logs/display/persistence?
- What default behavior is safe vs unsafe?

Output a structured threat analysis. Each finding should specify:

1. **Surface** — what is being introduced
2. **Risk** — what could go wrong
3. **Mitigation** — how the TODO should address it

These findings will be incorporated into the TODO as [Security] tagged items.

### Output

Print the COMPLETE threat analysis as text to stdout.

## Step 8: Phase 3 — Adversarial Debate

Tool: bash
Tier: tier-2-standard

Run explicit adversarial debate via `csa debate`.
Capture debate stdout, then normalize into a structured evidence packet
for mechanical validation in Step 9.

```bash
LANGUAGE="${STEP_1_OUTPUT:-Chinese (Simplified)}"
DEBATE_PROMPT="$(printf '%s\n' \
"Critically evaluate this draft TODO plan, generated spec, and threat model. Act as a devil's advocate." \
"" \
"## Draft TODO Plan" \
"${STEP_5_OUTPUT}" \
"" \
"## Generated Spec (Step 6)" \
"${STEP_6_OUTPUT}" \
"" \
"## Threat Model (Step 7)" \
"${STEP_7_OUTPUT}" \
"" \
"## Output Requirements" \
"Provide explicit verdict and confidence in your conclusion." )"
DEBATE_JSON="$(printf '%s\n' "${DEBATE_PROMPT}" | csa debate --tool gemini-cli --rounds 3 --format json --timeout 240 --idle-timeout 120 --no-stream-stdout)" || { echo "csa debate failed" >&2; exit 1; }
[[ -n "${DEBATE_JSON:-}" ]] || { echo "empty debate json output" >&2; exit 1; }
RAW_VERDICT="$(printf '%s\n' "${DEBATE_JSON}" | jq -r '.verdict // "UNKNOWN"' | tr '[:lower:]' '[:upper:]')"
case "${RAW_VERDICT}" in
  APPROVE) MAPPED_VERDICT="READY" ;;
  REVISE|REJECT) MAPPED_VERDICT="REVISE" ;;
  *) MAPPED_VERDICT="REVISE" ;;
esac
CONFIDENCE="$(printf '%s\n' "${DEBATE_JSON}" | jq -r '.confidence // "unknown"' | tr '[:upper:]' '[:lower:]')"
SUMMARY_LINE="$(printf '%s\n' "${DEBATE_JSON}" | jq -r '.summary // empty')"
KEY_POINTS="$(printf '%s\n' "${DEBATE_JSON}" | jq -r '.key_points[]?')"
if [[ -z "${KEY_POINTS:-}" && -n "${SUMMARY_LINE:-}" ]]; then
  KEY_POINTS="${SUMMARY_LINE}"
fi
[[ -n "${KEY_POINTS:-}" ]] || { KEY_POINTS="No concrete concerns surfaced; keep current plan with careful verification."; }
printf '%s\n' "DEBATE_EVIDENCE:"
printf '%s\n' "- method: csa debate"
printf '%s\n' "- tool: gemini-cli"
printf '%s\n' "- rounds: 3"
printf '%s\n' "- language: ${LANGUAGE}"
printf '%s\n' "- raw_verdict: ${RAW_VERDICT:-UNKNOWN}"
printf '%s\n' "- mapped_verdict: ${MAPPED_VERDICT}"
printf '%s\n' "- confidence: ${CONFIDENCE:-unknown}"
printf '%s\n' "VALID_CONCERNS:"
printf '%s\n' "${KEY_POINTS}" | sed 's/^/- /'
printf '%s\n' "SUGGESTED_CHANGES:"
printf '%s\n' "${KEY_POINTS}" | sed 's/^/- Address: /'
printf '%s\n' "OVERALL_ASSESSMENT:"
if [[ -n "${SUMMARY_LINE:-}" ]]; then
  printf '%s\n' "${SUMMARY_LINE}"
else
  printf '%s\n' "Debate completed without summary; proceed conservatively with REVISE stance."
fi
```

## Step 9: Phase 3.5 — Validate Debate Evidence

Tool: bash

Validate that debate output exists and carries required evidence markers.

```bash
[[ -n "${STEP_8_OUTPUT:-}" ]] || { echo "STEP_8_OUTPUT is empty — debate did not run" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^DEBATE_EVIDENCE:' || { echo "debate evidence header missing" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -Eq 'mapped_verdict:[[:space:]]*(READY|REVISE)' || { echo "mapped debate verdict missing" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -Eq 'raw_verdict:[[:space:]]*(APPROVE|REVISE|REJECT|UNKNOWN)' || { echo "raw debate verdict missing" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -Eq 'confidence:[[:space:]]*(high|medium|low|unknown)' || { echo "debate confidence missing" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^VALID_CONCERNS:' || { echo "valid concerns section missing" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^SUGGESTED_CHANGES:' || { echo "suggested changes section missing" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^OVERALL_ASSESSMENT:' || { echo "overall assessment section missing" >&2; exit 1; }
```

## Step 10: Revise TODO

Incorporate debate feedback and threat model findings into the TODO plan.
Concede valid points and revise accordingly. Defend sound decisions with evidence.
Use the generated spec as an acceptance contract.
If the debate reveals missing coverage, add TODO work that restores alignment
instead of silently diverging from the spec.

### Prior Context

### Draft TODO (Step 5)
${STEP_5_OUTPUT}

### Generated Spec (Step 6)
${STEP_6_OUTPUT}

### Threat Model (Step 7)
${STEP_7_OUTPUT}

### Adversarial Critique (Step 8)
${STEP_8_OUTPUT}

### Output

Output the COMPLETE revised TODO plan as text to stdout.
Include threat model findings as [Security] tagged checkbox items.
Do NOT write files to the project directory.
The output is captured as `${STEP_10_OUTPUT}` for the save step.

## Step 11: Save TODO

Tool: bash

Save finalized TODO and `spec.toml` using csa todo for git-tracked lifecycle.
Uses `${STEP_10_OUTPUT}` (the revised TODO from the revise step) and
`${STEP_6_OUTPUT}` (the generated spec from Step 6).
Execute ONLY the command block below.
FORBIDDEN: custom shell snippets, heredoc (`<<EOF`, `cat <<`), branch create/switch,
and writing intermediate files outside the TODO path.

```bash
[[ -n "${STEP_10_OUTPUT:-}" ]] || { echo "STEP_10_OUTPUT is empty — Step 10 (revise) must output the finalized TODO as text" >&2; exit 1; }
printf '%s\n' "${STEP_10_OUTPUT}" | grep -qE '^- \[ \] .+' || { echo "STEP_10_OUTPUT has no non-empty checkbox tasks" >&2; exit 1; }
printf '%s\n' "${STEP_10_OUTPUT}" | grep -q 'DONE WHEN:' || { echo "STEP_10_OUTPUT has no DONE WHEN clauses" >&2; exit 1; }
[[ -n "${STEP_6_OUTPUT:-}" ]] || { echo "STEP_6_OUTPUT is empty — Step 6 must output spec.toml content" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^schema_version = 1$' || { echo "STEP_6_OUTPUT missing schema_version = 1" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^plan_ulid = "__PLAN_ID__"$' || { echo "STEP_6_OUTPUT missing __PLAN_ID__ placeholder" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^summary = "' || { echo "STEP_6_OUTPUT missing summary" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^\[\[criteria\]\]$' || { echo "STEP_6_OUTPUT has no criteria entries" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | rg -q '^kind = "(scenario|property|check)"$' || { echo "STEP_6_OUTPUT has invalid criterion kinds" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^id = "' || { echo "STEP_6_OUTPUT missing criterion id" >&2; exit 1; }
printf '%s\n' "${STEP_6_OUTPUT}" | grep -q '^description = "' || { echo "STEP_6_OUTPUT missing criterion description" >&2; exit 1; }
SUMMARY_LINE=$(printf '%s\n' "${STEP_6_OUTPUT}" | sed -n 's/^summary = "\(.*\)"$/\1/p' | head -n1)
printf '%s' "${SUMMARY_LINE}" | rg -q '[\p{Han}]' || { echo "STEP_6_OUTPUT summary must be one Chinese line" >&2; exit 1; }
RESOLVED_LANGUAGE="${STEP_1_OUTPUT:-Chinese (Simplified)}"
if printf '%s' "${RESOLVED_LANGUAGE}" | grep -qi 'chinese'; then
  TASK_COUNT=$(printf '%s\n' "${STEP_10_OUTPUT}" | grep -cE '^- \[ \] .+')
  MIN_HAN="${TASK_COUNT}"
  if [ "${MIN_HAN}" -lt 2 ]; then MIN_HAN=2; fi
  if [ "${MIN_HAN}" -gt 30 ]; then MIN_HAN=30; fi
  HAN_COUNT_DRAFT=$(printf '%s\n' "${STEP_10_OUTPUT}" | rg -o '[\p{Han}]' | wc -l | tr -d '[:space:]')
  [[ "${HAN_COUNT_DRAFT:-0}" -ge "${MIN_HAN}" ]] || { echo "STEP_10_OUTPUT language mismatch: expected Han-script content (Han chars >= ${MIN_HAN})" >&2; exit 1; }
elif printf '%s' "${RESOLVED_LANGUAGE}" | grep -qi 'han script'; then
  TASK_COUNT=$(printf '%s\n' "${STEP_10_OUTPUT}" | grep -cE '^- \[ \] .+')
  MIN_CJK="${TASK_COUNT}"
  if [ "${MIN_CJK}" -lt 2 ]; then MIN_CJK=2; fi
  if [ "${MIN_CJK}" -gt 30 ]; then MIN_CJK=30; fi
  CJK_COUNT_DRAFT=$(printf '%s\n' "${STEP_10_OUTPUT}" | rg -o '[\p{Han}\p{Hiragana}\p{Katakana}]' | wc -l | tr -d '[:space:]')
  [[ "${CJK_COUNT_DRAFT:-0}" -ge "${MIN_CJK}" ]] || { echo "STEP_10_OUTPUT language mismatch: expected CJK-script content (CJK chars >= ${MIN_CJK})" >&2; exit 1; }
fi
CURRENT_BRANCH=$(git branch --show-current) || { echo "detect branch failed" >&2; exit 1; }
TODO_TS=$(csa todo create --branch "${CURRENT_BRANCH}" -- "${FEATURE}") || { echo "csa todo create failed" >&2; exit 1; }
TODO_PATH=$(csa todo show -t "${TODO_TS}" --path) || { echo "csa todo show failed" >&2; exit 1; }
SPEC_PATH="$(dirname "${TODO_PATH}")/spec.toml"
printf '%s\n' "${STEP_10_OUTPUT}" > "${TODO_PATH}" || { echo "write TODO failed" >&2; exit 1; }
SPEC_CONTENT="${STEP_6_OUTPUT//__PLAN_ID__/${TODO_TS}}"
printf '%s\n' "${SPEC_CONTENT}" > "${SPEC_PATH}" || { echo "write spec failed" >&2; exit 1; }
[[ -s "${TODO_PATH}" ]] || { echo "saved TODO is empty" >&2; exit 1; }
[[ -s "${SPEC_PATH}" ]] || { echo "saved spec is empty" >&2; exit 1; }
grep -qE '^- \[ \] .+' "${TODO_PATH}" || { echo "saved TODO has no non-empty checkbox tasks" >&2; exit 1; }
grep -q 'DONE WHEN:' "${TODO_PATH}" || { echo "saved TODO has no DONE WHEN clauses" >&2; exit 1; }
grep -q '^schema_version = 1$' "${SPEC_PATH}" || { echo "saved spec missing schema_version = 1" >&2; exit 1; }
grep -q "^plan_ulid = \"${TODO_TS}\"$" "${SPEC_PATH}" || { echo "saved spec plan_ulid mismatch" >&2; exit 1; }
grep -q '^summary = "' "${SPEC_PATH}" || { echo "saved spec missing summary" >&2; exit 1; }
grep -q '^\[\[criteria\]\]$' "${SPEC_PATH}" || { echo "saved spec has no criteria entries" >&2; exit 1; }
if printf '%s' "${RESOLVED_LANGUAGE}" | grep -qi 'chinese'; then
  HAN_COUNT_SAVED=$(rg -o '[\p{Han}]' "${TODO_PATH}" | wc -l | tr -d '[:space:]')
  [[ "${HAN_COUNT_SAVED:-0}" -ge "${MIN_HAN:-2}" ]] || { echo "saved TODO language mismatch: expected Han-script content (Han chars >= ${MIN_HAN:-2})" >&2; exit 1; }
elif printf '%s' "${RESOLVED_LANGUAGE}" | grep -qi 'han script'; then
  CJK_COUNT_SAVED=$(rg -o '[\p{Han}\p{Hiragana}\p{Katakana}]' "${TODO_PATH}" | wc -l | tr -d '[:space:]')
  [[ "${CJK_COUNT_SAVED:-0}" -ge "${MIN_CJK:-2}" ]] || { echo "saved TODO language mismatch: expected CJK-script content (CJK chars >= ${MIN_CJK:-2})" >&2; exit 1; }
fi
csa todo save -t "${TODO_TS}" "finalize: ${FEATURE}" || { echo "csa todo save failed" >&2; exit 1; }
SPEC_RENDERED=$(csa todo show -t "${TODO_TS}" --spec) || { echo "csa todo show --spec failed" >&2; exit 1; }
[[ "${SPEC_RENDERED}" != "No spec found for this plan" ]] || { echo "spec.toml was not persisted" >&2; exit 1; }
printf '%s\n' "${SPEC_RENDERED}" | grep -q '^Criteria:$' || { echo "csa todo show --spec missing criteria section" >&2; exit 1; }
printf '%s\n' "${SPEC_RENDERED}" | grep -q '^- \[pending\] ' || { echo "csa todo show --spec did not render pending criteria" >&2; exit 1; }
csa todo show -t "${TODO_TS}" --path
```

## Step 12: Phase 4 — User Approval

Present TODO to user for review in `${STEP_1_OUTPUT}`.
User chooses: APPROVE → proceed to mktsk, MODIFY → revise, REJECT → abandon.
