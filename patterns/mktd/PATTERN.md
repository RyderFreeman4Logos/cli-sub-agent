---
name = "mktd"
description = "Make TODO: CSA-powered reconnaissance, adversarial debate, and structured TODO plan generation"
allowed-tools = "TaskCreate, TaskUpdate, TaskList, TaskGet, Read, Grep, Glob, Bash, Write, Edit"
tier = "tier-2-standard"
version = "0.2.0"
---

# mktd: Make TODO — Debate-Enhanced Planning

Five-phase planning: RECON (CSA parallel exploration), DRAFT (synthesize TODO),
THREAT MODEL (security review), DEBATE (adversarial review), APPROVE (user gate).

Zero main-agent file reads during exploration. CSA sub-agents gather context.
Mandatory adversarial review catches blind spots.

### Intensity Modes

`INTENSITY` controls which phases run. Default: `full`.

| Mode | Phases | Use Case |
|------|--------|----------|
| `full` | RECON → LANGUAGE → DRAFT → SPEC → THREAT MODEL → DEBATE → VALIDATE → REVISE → SAVE → APPROVE | Default: all features, significant changes |
| `light` | RECON → LANGUAGE → DRAFT → SPEC → SAVE → APPROVE | Small changes (≤2 code files, <50 insertions) — skips threat model, debate, and revision |

Light mode flow: Phase 1 (RECON) → Phase 1.5 (LANGUAGE) → Phase 2 (DRAFT) →
Phase 2.25 (SPEC) → Phase 4 (SAVE) → Phase 4.5 (APPROVE).
The SAVE step uses the draft TODO directly instead of the revised version.

## Step 0: Phase 0.5 — Auto Session Discovery

Tool: bash

Find a reusable mktd session for the current project/branch.
Rules:

- Match branch + task_type=plan + phase=Available
- If found, output only the most recent session_id
- If not found (or not in git/detached HEAD), output empty string

```bash
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
# --- Intensity detection (CSA_VAR side-effect, stripped from STEP_OUTPUT) ---
if [[ "${INTENSITY:-full}" == "light" ]]; then
  echo "Planning intensity: light (skipping threat model, debate, revision)" >&2
  echo "CSA_VAR:INTENSITY_IS_LIGHT=true"
else
  echo "Planning intensity: full" >&2
  echo "CSA_VAR:INTENSITY_IS_LIGHT=false"
fi

# --- Language detection (result is the step's logical output) ---
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
Session: ${STEP_1_OUTPUT}
Tier: tier-1-quick

Analyze codebase structure relevant to ${FEATURE}.

CONSTRAINT ANCHOR: The user prompt above may specify target crates, key files, integration points, or architectural approach. If so, these are HARD CONSTRAINTS — start exploration from the specified files/modules and expand outward only as needed. Do NOT explore unrelated crates as primary targets. If the user specified key files, those MUST appear in your report.

Report: relevant files (path + purpose, max 20), key types, module dependencies, entry points.
Working directory: ${CWD}

## Step 3: Phase 1 — RECON Dimension 2 (Patterns)

Tool: csa
Session: ${STEP_1_OUTPUT}
Tier: tier-1-quick

Find existing patterns or similar features to ${FEATURE} in this codebase.

CONSTRAINT ANCHOR: If the user prompt specifies an architectural approach or target crate, pattern discovery MUST be scoped to that context first. Do NOT let codebase pattern matching override the user's explicit requirements. Report patterns that SUPPORT the user's specified approach, not patterns that suggest a different approach.

Report: file paths with approach, reusable components, conventions to follow.
Working directory: ${CWD}

## Step 4: Phase 1 — RECON Dimension 3 (Constraints)

Tool: csa
Session: ${STEP_1_OUTPUT}
Tier: tier-1-quick

Identify constraints and risks for implementing ${FEATURE}.

CONSTRAINT ANCHOR: If the user prompt specifies scope boundaries (target crate, specific modules), evaluate constraints WITHIN that scope. Flag risks that affect the user's specified approach, not risks that argue for a different approach.

Report: potential breaking changes, security considerations, performance, compatibility.
Working directory: ${CWD}

## Step 4a: Phase 1 — RECON Dimension 4 (Semantic Invariants)

Tool: csa
Session: ${STEP_1_OUTPUT}
Tier: tier-1-quick

Identify the semantic invariants and concurrency assumptions for ${FEATURE}.

CONSTRAINT ANCHOR: If the user prompt specifies scope boundaries (target crate, specific modules), evaluate invariants and concurrency semantics WITHIN that scope. Do NOT invent a different design just to simplify the invariant story.

Report using these keys:
- `invariant_list`: what invariants must hold during this module's lifetime
- `assumption_list`: what assumptions this module makes about external state, other writers, or process lifetime
- `concurrency_model`: who else can write to the files/state this touches, plus the failure / rollback model for each important step

Working directory: ${CWD}

## Step 4b: Phase 1 — Persist RECON References

Tool: bash

Save RECON findings as TODO references for progressive disclosure.
Each dimension's output is stored as a reference file so the full plan
can link to detailed findings without bloating TODO.md itself.

```bash
[[ -n "${STEP_13_OUTPUT:-}" ]] || { echo "no TODO path yet — skip ref persistence" >&2; exit 0; }
TODO_DIR="$(dirname "${STEP_13_OUTPUT}")"
TODO_TS="$(basename "${TODO_DIR}")"
if [[ -n "${STEP_3_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_3_OUTPUT}" recon-structure.md 2>/dev/null || true
fi
if [[ -n "${STEP_4_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_4_OUTPUT}" recon-patterns.md 2>/dev/null || true
fi
if [[ -n "${STEP_5_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_5_OUTPUT}" recon-constraints.md 2>/dev/null || true
fi
if [[ -n "${STEP_6_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_6_OUTPUT}" recon-invariants.md 2>/dev/null || true
fi
echo "RECON references persisted"
```

> **Note**: This step runs best AFTER Step 13 (Save TODO) creates the todo
> directory. When run before save, the step is a no-op. The orchestrator MAY
> re-invoke this step after save to persist references retroactively, or agents
> can call `csa todo ref add` directly during RECON to attach findings as they
> become available.

## Step 7: Phase 2 — DRAFT TODO

Synthesize CSA findings into a structured TODO plan.

### RECON Findings (from prior steps)

### Structure (Step 3)
${STEP_3_OUTPUT}

### Patterns (Step 4)
${STEP_4_OUTPUT}

### Constraints (Step 5)
${STEP_5_OUTPUT}

### Semantic Invariants (Step 6)
${STEP_6_OUTPUT}

### Instructions

CONSTRAINT VERIFICATION: Before drafting, check that RECON findings align with the user's original feature request (${FEATURE}). If the user specified a target crate, architecture, or key files, the plan MUST target those — not alternatives suggested by codebase pattern matching. If RECON findings contradict user constraints, note the conflict and follow the user's intent.

#### TODO Structure Requirements

The TODO plan MUST include the following sections for context recovery:

a) **"## Design Overview" section** at the top with:
   - Problem statement (1-2 sentences)
   - Key design decisions with rationale
   - Architecture constraints discovered during recon

b) Each task item MUST include:
   - A descriptive title (not just "Implement X")
   - Context sub-bullet explaining WHY this task exists and HOW it relates to the design
   - Dependencies on other tasks with specific details (not just "depends on task 1")
   - Each task description MUST be >= 20 words to provide sufficient context

c) **"## Debate Findings" section** (left empty in draft, populated after Phase 3):
   - Which debate points were adopted
   - Which were deferred and why

#### Formatting Rules

Each item is a [ ] checkbox with executor tag.
Write all TODO descriptions, section headers, and task names in `${STEP_2_OUTPUT}`.
Technical terms, code snippets, commit scope strings, and executor tags remain in English.
Pre-assign executors: [Main], [Sub:developer], [Skill:commit], [CSA:tool].
Every checkbox item MUST include a mechanically verifiable `DONE WHEN:` line.

### Output

Output the COMPLETE TODO plan as text to stdout.
Do NOT write files to the project directory.
Do NOT manually create `output/summary.md` or other `output/*` files in the repo.
CSA captures stdout and persists it under `$CSA_SESSION_DIR/output/summary.md`.
The output is captured as `${STEP_7_OUTPUT}` for subsequent steps.

## Step 8: Phase 2.25 — Spec Generation

Extract verifiable acceptance criteria from the draft TODO plan.

### Draft TODO Plan
${STEP_7_OUTPUT}

### Instructions

Only keep criteria that can be mechanically checked after implementation.
Classify each criterion using `SpecCriterion.kind`:

- `scenario`: end-to-end behavior or user-visible flow
- `property`: invariant, guarantee, or rule that must always hold
- `check`: concrete validation command, artifact, or presence check

Write criterion descriptions in `${STEP_2_OUTPUT}`.
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

Use `plan_ulid = "__PLAN_ID__"` as a placeholder. Step 13 must replace it with
the actual plan id returned by `csa todo create` before writing `spec.toml`.
Do NOT write files to the project directory.
Do NOT manually create `output/summary.md` or other `output/*` files in the repo.
CSA captures stdout and persists it under `$CSA_SESSION_DIR/output/summary.md`.
The output is captured as `${STEP_8_OUTPUT}` for subsequent steps.

## Step 9: Phase 2.5 — Threat Model

**Condition**: Skip if `INTENSITY=light`.

Review the draft TODO plan for security and safety concerns.

### Draft TODO Plan
${STEP_7_OUTPUT}

### Instructions

For each new API surface in the plan (config fields, CLI inputs, stored data,
external interactions), enumerate:

- What sensitive data flows through this path?
- What happens with malformed/hostile input?
- What information is exposed in logs/display/persistence?
- What default behavior is safe vs unsafe?
- What is the concurrent writer model for every external state this touches?
- Where are the TOCTOU / rollback / cleanup races?
- Which state-machine invariants must hold under concurrent transitions?
- Is each file write atomic (rename-into-place) or vulnerable to write-then-maybe-delete races?

If this is a **default-change task** (changes a default value, default
transport, default tool, default feature gate, or default config field),
also produce this matrix as part of the threat model output:

| Existing user state                  | Pre-change behavior | Post-change behavior | Gap? |
|--------------------------------------|---------------------|----------------------|------|
| Only legacy binary installed         | Works               | May fail (how?)      | Y/N  |
| Config explicitly set to old default | Works               | Honored?             | Y/N  |
| Config empty (uses default)          | Works               | Works                | expected |
| Config via project override          | Works               | Works                | Y/N  |

For each row where `Gap = Yes`, add a Phase-2.5-generated TODO item tagged
`[Security]` or `[Compat]`. Non-default-change tasks skip the matrix.

Output a structured threat analysis. Each finding should specify:

1. **Surface** — what is being introduced
2. **Risk** — what could go wrong
3. **Mitigation** — how the TODO should address it

These findings will be incorporated into the TODO as `[Security]` and, for
default-change compatibility gaps, `[Compat]` tagged items.

### Output

Print the COMPLETE threat analysis as text to stdout.

## Step 10: Phase 3 — Adversarial Debate

**Condition**: Skip if `INTENSITY=light`.

Tool: bash
Tier: tier-2-standard

Run explicit adversarial debate via `csa debate`.
Capture debate stdout, then normalize into a structured evidence packet
for mechanical validation in Step 11.

The debate prompt is now written to a temporary file created with `mktemp`,
then passed through `csa debate --prompt-file "$PROMPT_FILE"` instead of
embedding the full markdown payload as a single shell argument. This avoids
bash variable-expansion and shell-quoting hazards when the generated TODO,
spec, or threat model contains markdown with backticks, dollar-prefixed
identifiers, or other shell-sensitive content.

```bash
LANGUAGE="${STEP_2_OUTPUT:-Chinese (Simplified)}"
PROMPT_FILE="$(mktemp)"
trap 'rm -f "$PROMPT_FILE"' EXIT
{
  printf '%s\n' "Critically evaluate this draft TODO plan, generated spec, and threat model. Act as a devil's advocate."
  echo ""
  printf '%s\n' "## Draft TODO Plan"
  printf '%s\n' "${STEP_7_OUTPUT}"
  echo ""
  printf '%s\n' "## Generated Spec (Step 8)"
  printf '%s\n' "${STEP_8_OUTPUT}"
  echo ""
  printf '%s\n' "## Threat Model (Step 9)"
  printf '%s\n' "${STEP_9_OUTPUT}"
  echo ""
  printf '%s\n' "## Required Red-Team Coverage"
  printf '%s\n' "Enumerate every assumption made in the TODO plan."
  printf '%s\n' "For each assumption, construct at least one scenario where it is false and explain the resulting failure mode or missing mitigation."
  echo ""
  printf '%s\n' "## Output Requirements"
  printf '%s\n' "Provide explicit verdict and confidence in your conclusion."
} > "$PROMPT_FILE"
SID=$(csa debate --sa-mode true --rounds 3 --format json --idle-timeout 600 --no-stream-stdout --prompt-file "$PROMPT_FILE") || { echo "csa debate failed" >&2; exit 1; }
DEBATE_JSON="$(csa session wait --session "$SID" 2>&1)" || { echo "csa debate failed" >&2; exit 1; }
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
printf '%s\n' "- tool: config:[debate].tool"
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

## Step 11: Phase 3.5 — Validate Debate Evidence

**Condition**: Skip if `INTENSITY=light`.

Tool: bash

Validate that debate output exists and carries required evidence markers.

```bash
[[ -n "${STEP_10_OUTPUT:-}" ]] || { echo "STEP_10_OUTPUT is empty — debate did not run" >&2; exit 1; }
printf '%s\n' "${STEP_10_OUTPUT}" | grep -q '^DEBATE_EVIDENCE:' || { echo "debate evidence header missing" >&2; exit 1; }
printf '%s\n' "${STEP_10_OUTPUT}" | grep -Eq 'mapped_verdict:[[:space:]]*(READY|REVISE)' || { echo "mapped debate verdict missing" >&2; exit 1; }
printf '%s\n' "${STEP_10_OUTPUT}" | grep -Eq 'raw_verdict:[[:space:]]*(APPROVE|REVISE|REJECT|UNKNOWN)' || { echo "raw debate verdict missing" >&2; exit 1; }
printf '%s\n' "${STEP_10_OUTPUT}" | grep -Eq 'confidence:[[:space:]]*(high|medium|low|unknown)' || { echo "debate confidence missing" >&2; exit 1; }
printf '%s\n' "${STEP_10_OUTPUT}" | grep -q '^VALID_CONCERNS:' || { echo "valid concerns section missing" >&2; exit 1; }
printf '%s\n' "${STEP_10_OUTPUT}" | grep -q '^SUGGESTED_CHANGES:' || { echo "suggested changes section missing" >&2; exit 1; }
printf '%s\n' "${STEP_10_OUTPUT}" | grep -q '^OVERALL_ASSESSMENT:' || { echo "overall assessment section missing" >&2; exit 1; }
```

## Step 12: Revise TODO

**Condition**: Skip if `INTENSITY=light`.

Incorporate debate feedback and threat model findings into the TODO plan.
Concede valid points and revise accordingly. Defend sound decisions with evidence.
Use the generated spec as an acceptance contract.
If the debate reveals missing coverage, add TODO work that restores alignment
instead of silently diverging from the spec.

Enrich task descriptions with debate rationale. Each task MUST have enough context
that a fresh agent (post-compaction) can understand the full design intent without
any prior conversation history.

Populate the "## Debate Findings" section with:
- Which debate points were adopted and how they changed the plan
- Which were deferred and why (with brief justification)

### Prior Context

### Draft TODO (Step 7)
${STEP_7_OUTPUT}

### Generated Spec (Step 8)
${STEP_8_OUTPUT}

### Threat Model (Step 9)
${STEP_9_OUTPUT}

### Adversarial Critique (Step 10)
${STEP_10_OUTPUT}

### Output

Output the COMPLETE revised TODO plan as text to stdout.
Include threat model findings as [Security] tagged checkbox items.
Do NOT write files to the project directory.
Do NOT manually create `output/summary.md` or other `output/*` files in the repo.
CSA captures stdout and persists it under `$CSA_SESSION_DIR/output/summary.md`.
The output is captured as `${STEP_12_OUTPUT}` for the save step.

## Step 13: Save TODO

Tool: bash

Save finalized TODO and `spec.toml` using csa todo for git-tracked lifecycle.
In full mode, uses `${STEP_12_OUTPUT}` (revised TODO). In light mode, falls back
to `${STEP_7_OUTPUT}` (draft TODO) since threat model/debate/revise are skipped.
Spec comes from `${STEP_8_OUTPUT}` in both modes.
Execute ONLY the command block below.
FORBIDDEN: custom shell snippets, heredoc (`<<EOF`, `cat <<`), branch create/switch,
and writing intermediate files outside the TODO path.

```bash
# Resolve TODO content: revised (full) or draft (light)
if [[ -n "${STEP_12_OUTPUT:-}" ]]; then
  FINAL_TODO="${STEP_12_OUTPUT}"
elif [[ -n "${STEP_7_OUTPUT:-}" ]]; then
  FINAL_TODO="${STEP_7_OUTPUT}"
else
  echo "Neither STEP_12_OUTPUT (revised) nor STEP_7_OUTPUT (draft) is available" >&2; exit 1
fi
printf '%s\n' "${FINAL_TODO}" | grep -qE '^- \[ \] .+' || { echo "TODO has no non-empty checkbox tasks" >&2; exit 1; }
printf '%s\n' "${FINAL_TODO}" | grep -q 'DONE WHEN:' || { echo "TODO has no DONE WHEN clauses" >&2; exit 1; }
[[ -n "${STEP_8_OUTPUT:-}" ]] || { echo "STEP_8_OUTPUT is empty — Step 8 must output spec.toml content" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^schema_version = 1$' || { echo "STEP_8_OUTPUT missing schema_version = 1" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^plan_ulid = "__PLAN_ID__"$' || { echo "STEP_8_OUTPUT missing __PLAN_ID__ placeholder" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^summary = "' || { echo "STEP_8_OUTPUT missing summary" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^\[\[criteria\]\]$' || { echo "STEP_8_OUTPUT has no criteria entries" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | rg -q '^kind = "(scenario|property|check)"$' || { echo "STEP_8_OUTPUT has invalid criterion kinds" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^id = "' || { echo "STEP_8_OUTPUT missing criterion id" >&2; exit 1; }
printf '%s\n' "${STEP_8_OUTPUT}" | grep -q '^description = "' || { echo "STEP_8_OUTPUT missing criterion description" >&2; exit 1; }
SUMMARY_LINE=$(printf '%s\n' "${STEP_8_OUTPUT}" | sed -n 's/^summary = "\(.*\)"$/\1/p' | head -n1)
printf '%s' "${SUMMARY_LINE}" | rg -q '[\p{Han}]' || { echo "STEP_8_OUTPUT summary must be one Chinese line" >&2; exit 1; }
RESOLVED_LANGUAGE="${STEP_2_OUTPUT:-Chinese (Simplified)}"
if printf '%s' "${RESOLVED_LANGUAGE}" | grep -qi 'chinese'; then
  TASK_COUNT=$(printf '%s\n' "${FINAL_TODO}" | grep -cE '^- \[ \] .+')
  MIN_HAN="${TASK_COUNT}"
  if [ "${MIN_HAN}" -lt 2 ]; then MIN_HAN=2; fi
  if [ "${MIN_HAN}" -gt 30 ]; then MIN_HAN=30; fi
  HAN_COUNT_DRAFT=$(printf '%s\n' "${FINAL_TODO}" | rg -o '[\p{Han}]' | wc -l | tr -d '[:space:]')
  [[ "${HAN_COUNT_DRAFT:-0}" -ge "${MIN_HAN}" ]] || { echo "TODO language mismatch: expected Han-script content (Han chars >= ${MIN_HAN})" >&2; exit 1; }
elif printf '%s' "${RESOLVED_LANGUAGE}" | grep -qi 'han script'; then
  TASK_COUNT=$(printf '%s\n' "${FINAL_TODO}" | grep -cE '^- \[ \] .+')
  MIN_CJK="${TASK_COUNT}"
  if [ "${MIN_CJK}" -lt 2 ]; then MIN_CJK=2; fi
  if [ "${MIN_CJK}" -gt 30 ]; then MIN_CJK=30; fi
  CJK_COUNT_DRAFT=$(printf '%s\n' "${FINAL_TODO}" | rg -o '[\p{Han}\p{Hiragana}\p{Katakana}]' | wc -l | tr -d '[:space:]')
  [[ "${CJK_COUNT_DRAFT:-0}" -ge "${MIN_CJK}" ]] || { echo "TODO language mismatch: expected CJK-script content (CJK chars >= ${MIN_CJK})" >&2; exit 1; }
fi
CURRENT_BRANCH=$(git branch --show-current) || { echo "detect branch failed" >&2; exit 1; }
LANG_ARGS=()
if [[ -n "${RESOLVED_LANGUAGE:-}" ]]; then
  LANG_ARGS+=("--language" "${RESOLVED_LANGUAGE}")
fi
TODO_TS=$(csa todo create --branch "${CURRENT_BRANCH}" "${LANG_ARGS[@]}" -- "${FEATURE}") || { echo "csa todo create failed" >&2; exit 1; }
TODO_PATH=$(csa todo show -t "${TODO_TS}" --path) || { echo "csa todo show failed" >&2; exit 1; }
SPEC_PATH="$(dirname "${TODO_PATH}")/spec.toml"
printf '%s\n' "${FINAL_TODO}" > "${TODO_PATH}" || { echo "write TODO failed" >&2; exit 1; }
SPEC_CONTENT="${STEP_8_OUTPUT//__PLAN_ID__/${TODO_TS}}"
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
# Validate language metadata consistency: if metadata.language is set,
# verify TODO content matches (e.g., Chinese plan should have Chinese descriptions)
if [[ -n "${RESOLVED_LANGUAGE:-}" ]]; then
  if printf '%s' "${RESOLVED_LANGUAGE}" | grep -qi 'chinese'; then
    HAN_META_CHECK=$(rg -o '[\p{Han}]' "${TODO_PATH}" | wc -l | tr -d '[:space:]')
    [[ "${HAN_META_CHECK:-0}" -ge 2 ]] || { echo "language metadata mismatch: plan language is ${RESOLVED_LANGUAGE} but content lacks Han characters" >&2; exit 1; }
  fi
fi
csa todo save -t "${TODO_TS}" "finalize: ${FEATURE}" || { echo "csa todo save failed" >&2; exit 1; }
SPEC_RENDERED=$(csa todo show -t "${TODO_TS}" --spec) || { echo "csa todo show --spec failed" >&2; exit 1; }
[[ "${SPEC_RENDERED}" != "No spec found for this plan" ]] || { echo "spec.toml was not persisted" >&2; exit 1; }
printf '%s\n' "${SPEC_RENDERED}" | grep -q '^Criteria:$' || { echo "csa todo show --spec missing criteria section" >&2; exit 1; }
printf '%s\n' "${SPEC_RENDERED}" | grep -q '^- \[pending\] ' || { echo "csa todo show --spec did not render pending criteria" >&2; exit 1; }
csa todo show -t "${TODO_TS}" --path
```

## Step 14: Persist References & Design Document

Tool: bash

After TODO is saved, persist RECON, threat model, debate findings, and a
consolidated design document as references for progressive disclosure.
The design document aggregates all RECON findings into a single reference
stored in `~/.local/state/cli-sub-agent/` (not git-tracked). Agents
executing the plan can retrieve it via `csa todo ref show design.md`
without loading the full plan into their context window.

```bash
TODO_PATH="${STEP_13_OUTPUT}"
[[ -n "${TODO_PATH:-}" ]] || { echo "STEP_13_OUTPUT empty — cannot persist refs" >&2; exit 1; }
TODO_DIR="$(dirname "${TODO_PATH}")"
TODO_TS="$(basename "${TODO_DIR}")"
if [[ -n "${STEP_3_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_3_OUTPUT}" recon-structure.md 2>/dev/null || true
fi
if [[ -n "${STEP_4_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_4_OUTPUT}" recon-patterns.md 2>/dev/null || true
fi
if [[ -n "${STEP_5_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_5_OUTPUT}" recon-constraints.md 2>/dev/null || true
fi
if [[ -n "${STEP_6_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_6_OUTPUT}" recon-invariants.md 2>/dev/null || true
fi
if [[ -n "${STEP_9_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_9_OUTPUT}" threat-model.md 2>/dev/null || true
fi
if [[ -n "${STEP_10_OUTPUT:-}" ]]; then
  csa todo ref add -t "${TODO_TS}" --content "${STEP_10_OUTPUT}" debate-evidence.md 2>/dev/null || true
fi

# Generate consolidated design document (not git-tracked)
DESIGN_DOC=$(printf '%s\n\n' \
  "# Design Document: ${FEATURE}" \
  "> Auto-generated by mktd RECON phase. Not git-tracked." \
  "> Stored in ~/.local/state/cli-sub-agent/ via csa todo ref." \
  "> View with: csa todo ref show design.md" \
  "" \
  "## Codebase Structure" \
  "${STEP_3_OUTPUT}" \
  "## Existing Patterns & Conventions" \
  "${STEP_4_OUTPUT}" \
  "## Constraints & Risks" \
  "${STEP_5_OUTPUT}" \
  "## Semantic Invariants" \
  "${STEP_6_OUTPUT}" \
  "## Threat Model" \
  "${STEP_9_OUTPUT}" \
  "## Debate Evidence" \
  "${STEP_10_OUTPUT}")
csa todo ref add -t "${TODO_TS}" --content "${DESIGN_DOC}" design.md 2>/dev/null || true

csa todo ref list -t "${TODO_TS}" 2>/dev/null || echo "(no refs persisted)"
```

## Step 15: Phase 4 — User Approval

Present TODO to user for review in `${STEP_2_OUTPUT}`.
User chooses: APPROVE → proceed to mktsk, MODIFY → revise, REJECT → abandon.
