---
name = "rule-extractor"
description = "Auto-extract coding rules from HIGH/CRITICAL PR-bot findings (closed-loop learning)"
allowed-tools = "Bash, Read, Grep, Glob"
tier = "tier-2-standard"
version = "0.1.0"
---

# Rule Extractor: Closed-Loop Learning from PR-Bot Findings

Transforms verified HIGH/CRITICAL review findings into reusable coding rules.
When pr-bot identifies a structural bug class during review, this pattern
extracts the lesson into a rule file so the bug class goes extinct — future
agents see the rule at write-time and avoid the anti-pattern entirely.

**Design choice (Option 2)**: Runs post-merge on pr-bot, not on review
completion. This ensures rules are extracted from the FINAL fix state, not
intermediate review iterations. The merged code represents the authoritative
fix, and the full review history (rounds, false positives, debate results)
is available for case study generation.

## When to Use

Use this pattern when ALL of these conditions are met:
1. A PR was **merged successfully** via pr-bot.
2. The PR's review history contains **HIGH/CRITICAL/P1** severity findings.
3. The findings were **confirmed real** (not false positives — passed debate arbitration or were fixed).
4. At least one finding represents a **bug class** (structural pattern), not an isolated mistake.

## Step 1: Collect Findings

Tool: bash

Read the merged PR's review artifacts and extract HIGH/CRITICAL findings.

```bash
set -euo pipefail
# Collect review findings from the merged PR and parse findings.toml.
# Emits CSA_VAR:FINDING_DESCRIPTION, CSA_VAR:FINDING_SEVERITY, CSA_VAR:FINDING_FILE
# for the first HIGH/CRITICAL finding. Pattern processes one finding per invocation;
# invoke once per finding for multi-finding PRs.
FINDINGS_RAW=""
FINDING_EMITTED=""
if [ -n "${REVIEW_SESSION_ID:-}" ]; then
  # Read findings from the review session's structured output.
  # --section details contains the review findings (including findings.toml content
  # when the review pattern emits it). --full is a broader fallback.
  FINDINGS_RAW=$(csa session result --session "${REVIEW_SESSION_ID}" --section details 2>/dev/null || \
                 csa session result --session "${REVIEW_SESSION_ID}" --full 2>/dev/null || true)
else
  # Fall back to PR comment history for bot findings.
  # Extract severity + description + path from PR comment bodies and emit
  # CSA_VARs directly (comment JSON shape differs from findings.toml).
  PR_COMMENTS=$(gh api --paginate "repos/${REPO}/pulls/${PR_NUM}/comments" 2>/dev/null || true)
  if [ -n "$PR_COMMENTS" ]; then
    # Pick the first bot comment whose body mentions a HIGH/CRITICAL/P0/P1 severity.
    # Filter by known bot logins to avoid misinterpreting human comments.
    COMMENT_BODY=$(echo "$PR_COMMENTS" \
      | jq -r '.[] | select(.user.login | test("codex|gemini-pr-bot|coderabbit|github-actions"; "i")) | select(.body | test("P0|P1|HIGH|CRITICAL"; "i")) | .body' \
      | head -1)
    COMMENT_PATH=$(echo "$PR_COMMENTS" \
      | jq -r '.[] | select(.user.login | test("codex|gemini-pr-bot|coderabbit|github-actions"; "i")) | select(.body | test("P0|P1|HIGH|CRITICAL"; "i")) | .path // "unknown"' \
      | head -1)
    if [ -n "$COMMENT_BODY" ]; then
      # Infer severity from the comment text
      COMMENT_SEV="HIGH"
      if echo "$COMMENT_BODY" | grep -qiE "CRITICAL|P0"; then
        COMMENT_SEV="CRITICAL"
      fi
      # Use first line of comment body as description (strip markdown headers)
      COMMENT_DESC=$(echo "$COMMENT_BODY" | grep -v '^#' | grep -v '^[[:space:]]*$' | head -1 | sed 's/^[[:space:]]*//')
      echo "CSA_VAR:FINDING_DESCRIPTION=$COMMENT_DESC"
      echo "CSA_VAR:FINDING_SEVERITY=$COMMENT_SEV"
      echo "CSA_VAR:FINDING_FILE=${COMMENT_PATH:-unknown}"
      FINDING_EMITTED=1
      echo "Extracted finding from PR comment: severity=$COMMENT_SEV file=$COMMENT_PATH"
    else
      echo "WARNING: No HIGH/CRITICAL findings in PR comments"
      echo "CSA_VAR:FINDING_DESCRIPTION="
      echo "CSA_VAR:FINDING_SEVERITY="
      echo "CSA_VAR:FINDING_FILE="
      FINDING_EMITTED=1
    fi
  else
    echo "WARNING: Could not fetch PR comments"
    echo "CSA_VAR:FINDING_DESCRIPTION="
    echo "CSA_VAR:FINDING_SEVERITY="
    echo "CSA_VAR:FINDING_FILE="
    FINDING_EMITTED=1
  fi
  # Skip the TOML parser below — CSA_VARs already emitted
  FINDINGS_RAW=""
fi

# Parse findings.toml for first HIGH/CRITICAL finding and emit CSA_VARs.
# findings.toml uses [[findings]] sections with id, severity, description,
# and file_ranges (array of {path, start, end}).
if [ -n "$FINDINGS_RAW" ]; then
  # Extract severity, description, and file path from first high/critical finding.
  # The toml structure: severity = "high"|"critical", description = "...",
  # file_ranges has path = "..." entries.
  # Filter: only consider lines where severity is high or critical
  FIRST_SEV=$(echo "$FINDINGS_RAW" | grep -iE '^severity[[:space:]]*=[[:space:]]*"(high|critical)"' | head -1 | sed -E 's/.*= *"([^"]*)".*/\1/')
  # Find the description from the same [[findings]] block as the matched severity.
  # Since findings.toml is flat-ish (severity then description then file_ranges),
  # we take the description line following the first high/critical severity line.
  FIRST_DESC=$(echo "$FINDINGS_RAW" | grep -iEA 20 '^severity[[:space:]]*=[[:space:]]*".*(high|critical)"' | grep -i '^description' | head -1 | sed -E 's/.*= *"(.*)".*/\1/')
  FIRST_FILE=$(echo "$FINDINGS_RAW" | grep -iEA 30 '^severity[[:space:]]*=[[:space:]]*".*(high|critical)"' | grep -i '^path' | head -1 | sed -E 's/.*= *"([^"]*)".*/\1/')

  if [ -n "$FIRST_DESC" ]; then
    echo "CSA_VAR:FINDING_DESCRIPTION=$FIRST_DESC"
    echo "CSA_VAR:FINDING_SEVERITY=${FIRST_SEV:-HIGH}"
    echo "CSA_VAR:FINDING_FILE=${FIRST_FILE:-unknown}"
    FINDING_EMITTED=1
    echo "Extracted finding: severity=$FIRST_SEV file=$FIRST_FILE"
    echo "Description: $FIRST_DESC"
  else
    echo "WARNING: No parseable findings in review session output"
    echo "CSA_VAR:FINDING_DESCRIPTION="
    echo "CSA_VAR:FINDING_SEVERITY="
    echo "CSA_VAR:FINDING_FILE="
    FINDING_EMITTED=1
  fi
elif [ -z "$FINDING_EMITTED" ]; then
  echo "WARNING: No findings data available from session or PR comments"
  echo "CSA_VAR:FINDING_DESCRIPTION="
  echo "CSA_VAR:FINDING_SEVERITY="
  echo "CSA_VAR:FINDING_FILE="
fi
```

Output: CSA_VAR:FINDING_DESCRIPTION, CSA_VAR:FINDING_SEVERITY, CSA_VAR:FINDING_FILE
for the first HIGH/CRITICAL finding. Empty values if no findings found.

## Step 2: Classify Bug Class vs Isolated Mistake

Tool: bash

For each HIGH/CRITICAL finding, dispatch an LLM classifier to determine
whether the finding represents a **bug class** (structural anti-pattern
that can recur) or an **isolated mistake** (one-off typo, copy-paste error,
unique to this specific code path).

Classification criteria:
- **Bug class**: The anti-pattern is reproducible in other code. Two or more
  examples exist (either in this PR's findings or in historical PRs). The fix
  required a structural change (new abstraction, pattern switch), not just a
  line edit.
- **Isolated mistake**: The fix was a single-line correction. The mistake
  cannot generalize to other code paths. No historical precedent exists.

Only bug classes proceed to Step 3. Isolated mistakes are logged and skipped.

```bash
set -euo pipefail
CLASSIFY_SID=$(csa run --sa-mode true --tier tier-2-standard \
  --description "classify-finding: bug-class-or-isolated" \
  "Classify the following review finding as either BUG_CLASS or ISOLATED_MISTAKE.

   Finding: ${FINDING_DESCRIPTION}
   Severity: ${FINDING_SEVERITY}
   File: ${FINDING_FILE}
   Fix commit: ${FIX_COMMIT_SHA}
   Fix diff summary: ${FIX_DIFF_SUMMARY}

   Criteria for BUG_CLASS:
   - The anti-pattern is reproducible in other code
   - The fix required a structural change, not just a line edit
   - Two or more examples exist or could exist in a codebase of this size

   Criteria for ISOLATED_MISTAKE:
   - Single-line fix, unique to this code path
   - Cannot generalize to other locations
   - No historical precedent

   Output EXACTLY these lines (one per line, no quoting, no extra whitespace):
   CLASSIFICATION=BUG_CLASS or CLASSIFICATION=ISOLATED_MISTAKE
   RATIONALE=<one paragraph explaining why>

   If CLASSIFICATION=BUG_CLASS, also output these ADDITIONAL lines:
   BUG_CLASS_NAME=<human-readable name, e.g. Unbound Shell Variable>
   BUG_CLASS_SLUG=<kebab-case slug, e.g. unbound-shell-variable>
   BUG_CLASS_KEYWORDS=<space-separated grep keywords, e.g. unbound variable set -u>
   BUG_CLASS_DESCRIPTION=<single-line paragraph describing the bug class>
   ANTI_PATTERN_EXAMPLES_B64=<base64-encoded multi-line anti-pattern code examples>
   CORRECT_PATTERN_B64=<base64-encoded multi-line correct pattern code examples>

   If CLASSIFICATION=ISOLATED_MISTAKE, do NOT output the BUG_CLASS_* lines.")
csa session wait --session "$CLASSIFY_SID"

# Read classifier result and emit CSA_VAR for condition gating.
# When classification is BUG_CLASS, also parse and emit the 6 BUG_CLASS_*
# fields that Steps 3-5 reference.
CLASSIFY_OUTPUT=$(csa session result --session "$CLASSIFY_SID" --section summary 2>/dev/null || \
                  csa session result --session "$CLASSIFY_SID" --full 2>/dev/null || true)
if echo "$CLASSIFY_OUTPUT" | grep -q "CLASSIFICATION=BUG_CLASS"; then
  echo "CSA_VAR:HAS_BUG_CLASS_FINDINGS=yes"

  # Extract the 6 BUG_CLASS fields from LLM output (one per line, KEY=VALUE format)
  BC_NAME=$(echo "$CLASSIFY_OUTPUT" | grep -E '^BUG_CLASS_NAME=' | head -1 | sed 's/^BUG_CLASS_NAME=//')
  BC_SLUG=$(echo "$CLASSIFY_OUTPUT" | grep -E '^BUG_CLASS_SLUG=' | head -1 | sed 's/^BUG_CLASS_SLUG=//')
  BC_KEYWORDS=$(echo "$CLASSIFY_OUTPUT" | grep -E '^BUG_CLASS_KEYWORDS=' | head -1 | sed 's/^BUG_CLASS_KEYWORDS=//')
  BC_DESC=$(echo "$CLASSIFY_OUTPUT" | grep -E '^BUG_CLASS_DESCRIPTION=' | head -1 | sed 's/^BUG_CLASS_DESCRIPTION=//')
  BC_ANTI_B64=$(echo "$CLASSIFY_OUTPUT" | grep -E '^ANTI_PATTERN_EXAMPLES_B64=' | head -1 | sed 's/^ANTI_PATTERN_EXAMPLES_B64=//')
  BC_CORRECT_B64=$(echo "$CLASSIFY_OUTPUT" | grep -E '^CORRECT_PATTERN_B64=' | head -1 | sed 's/^CORRECT_PATTERN_B64=//')

  # Fallback defaults when LLM omits optional fields
  : "${BC_NAME:=unknown-bug-class}"
  : "${BC_SLUG:=unknown-bug-class}"
  : "${BC_KEYWORDS:=bug class}"
  : "${BC_DESC:=${FINDING_DESCRIPTION}}"

  echo "CSA_VAR:BUG_CLASS_NAME=$BC_NAME"
  echo "CSA_VAR:BUG_CLASS_SLUG=$BC_SLUG"
  echo "CSA_VAR:BUG_CLASS_KEYWORDS=$BC_KEYWORDS"
  echo "CSA_VAR:BUG_CLASS_DESCRIPTION=$BC_DESC"
  echo "CSA_VAR:ANTI_PATTERN_EXAMPLES_B64=${BC_ANTI_B64:-}"
  echo "CSA_VAR:CORRECT_PATTERN_B64=${BC_CORRECT_B64:-}"
else
  echo "CSA_VAR:HAS_BUG_CLASS_FINDINGS="
fi
```

## Step 3: Deduplicate Against Existing Rules

Tool: bash

Check whether an existing rule already covers this bug class. Search
project-local rules (`docs/rules-proposed/`). Emits
`CSA_VAR:SHOULD_DRAFT=yes` when Step 4 should run, and
`CSA_VAR:DEDUPE_RESULT` with the semantic comparison outcome.

```bash
set -euo pipefail
PROJECT_RULES_DIR="docs/rules-proposed"

# Keyword grep across project-local rules
PROJECT_MATCHES=""
if [ -d "${PROJECT_RULES_DIR}" ]; then
  PROJECT_MATCHES=$(grep -rl "${BUG_CLASS_KEYWORDS}" "${PROJECT_RULES_DIR}/" 2>/dev/null || true)
fi

if [ -z "${PROJECT_MATCHES}" ]; then
  echo "EXISTING_RULE_MATCH=none"
  echo "CSA_VAR:EXISTING_RULE_MATCH=none"
  echo "CSA_VAR:DEDUPE_RESULT=NO_MATCH"
  echo "CSA_VAR:SHOULD_DRAFT=yes"
else
  echo "Potential matches found:"
  echo "${PROJECT_MATCHES}"
  echo "EXISTING_RULE_MATCH=potential"
  echo "CSA_VAR:EXISTING_RULE_MATCH=potential"

  # Read actual content from matched rule files for semantic comparison
  MATCHED_CONTENT=""
  while IFS= read -r match_file; do
    [ -z "$match_file" ] && continue
    MATCHED_CONTENT="${MATCHED_CONTENT}
--- ${match_file} ---
$(cat "$match_file")
"
  done <<< "${PROJECT_MATCHES}"

  # Dispatch semantic deduplication for potential matches
  DEDUPE_SID=$(csa run --sa-mode true --tier tier-1-quick \
    --description "dedupe-check: ${BUG_CLASS_NAME}" \
    "Compare this bug class against the existing rule(s).
     Bug class: ${BUG_CLASS_DESCRIPTION}
     Existing rule content:
${MATCHED_CONTENT}
     Output: DEDUPE_RESULT=EXACT_MATCH|PARTIAL_MATCH|NO_MATCH
     If PARTIAL_MATCH: UPDATE_SUGGESTION: <what to add>")
  csa session wait --session "$DEDUPE_SID"

  # Read dedupe result and decide whether to draft
  DEDUPE_OUTPUT=$(csa session result --session "$DEDUPE_SID" --section summary 2>/dev/null || true)
  if echo "$DEDUPE_OUTPUT" | grep -q "DEDUPE_RESULT=EXACT_MATCH"; then
    echo "CSA_VAR:DEDUPE_RESULT=EXACT_MATCH"
    echo "CSA_VAR:SHOULD_DRAFT="
  elif echo "$DEDUPE_OUTPUT" | grep -q "DEDUPE_RESULT=PARTIAL_MATCH"; then
    echo "CSA_VAR:DEDUPE_RESULT=PARTIAL_MATCH"
    echo "CSA_VAR:SHOULD_DRAFT=yes"
  else
    echo "CSA_VAR:DEDUPE_RESULT=NO_MATCH"
    echo "CSA_VAR:SHOULD_DRAFT=yes"
  fi
fi
```

## Step 4: Generate Rule Draft

Tool: bash (dispatches csa run, captures draft into DRAFT_FILE)
Tier: tier-2-standard

Generate a rule file following the structure of existing rules
(e.g., `rust/017-concurrent-file-primitives.md`). The rule must contain:

1. **Core Requirement**: One-paragraph summary of what the rule requires.
2. **Why This Rule Exists**: Root cause explanation with the concrete failure mode.
3. **Anti-Patterns (Forbidden)**: Table of code shapes that cause this bug.
4. **Required Implementation Patterns**: Structurally-safe alternatives with code examples.
5. **Decision Checklist**: 2-4 yes/no checks an agent can apply at write-time.
6. **Case Study**: Link to the PR/commit that surfaced this bug class.

The rule file includes frontmatter for traceability:

```yaml
---
source: pr-bot-finding
pr: "#<PR_NUM>"
severity: <HIGH|CRITICAL>
extracted-at: <ISO-8601 date>
finding-ids: [<list of finding IDs>]
---
```

```bash
set -euo pipefail

# Decode base64-encoded multi-line fields from Step 2
ANTI_PATTERN_DECODED=""
if [ -n "${ANTI_PATTERN_EXAMPLES_B64:-}" ]; then
  ANTI_PATTERN_DECODED=$(echo "${ANTI_PATTERN_EXAMPLES_B64}" | base64 -d 2>/dev/null || true)
fi
: "${ANTI_PATTERN_DECODED:=No anti-pattern examples provided.}"

CORRECT_PATTERN_DECODED=""
if [ -n "${CORRECT_PATTERN_B64:-}" ]; then
  CORRECT_PATTERN_DECODED=$(echo "${CORRECT_PATTERN_B64}" | base64 -d 2>/dev/null || true)
fi
: "${CORRECT_PATTERN_DECODED:=No correct pattern examples provided.}"

DRAFT_SID=$(csa run --sa-mode true --tier tier-2-standard \
  --description "draft-rule: ${BUG_CLASS_NAME}" \
  "Generate a coding rule file for the following bug class.

   Bug class: ${BUG_CLASS_DESCRIPTION}
   Language: ${LANG}
   Anti-pattern examples:
${ANTI_PATTERN_DECODED}
   Correct pattern:
${CORRECT_PATTERN_DECODED}
   PR: #${PR_NUM}
   Fix commit: ${FIX_COMMIT_SHA}

   Use this structure (mirrors rust/017-concurrent-file-primitives.md):
   1. Core Requirement (one paragraph)
   2. Why This Rule Exists (failure mode + root cause)
   3. Anti-Patterns table (| Anti-pattern | Consequence | Fix |)
   4. Required Implementation Patterns (code examples)
   5. Decision Checklist (2-4 yes/no items)
   6. Case Study: PR #${PR_NUM}

   Add frontmatter: source: pr-bot-finding, pr: #${PR_NUM}, severity: ${FINDING_SEVERITY},
   extracted-at: $(date -u +%Y-%m-%d)

   Output the complete rule file content between RULE_DRAFT_START and RULE_DRAFT_END markers.")
csa session wait --session "$DRAFT_SID"

# Extract rule content from session output between markers
DRAFT_RAW=$(csa session result --session "$DRAFT_SID" --section details 2>/dev/null || \
            csa session result --session "$DRAFT_SID" 2>/dev/null || true)
DRAFT_CONTENT=$(echo "$DRAFT_RAW" | sed -n '/RULE_DRAFT_START/,/RULE_DRAFT_END/{/RULE_DRAFT_START/d;/RULE_DRAFT_END/d;p}')

if [ -z "$DRAFT_CONTENT" ]; then
  echo "ERROR: Could not extract rule draft from session $DRAFT_SID"
  exit 1
fi

# Write draft to a temp file and emit its path
DRAFT_FILE="docs/rules-proposed/.draft-${BUG_CLASS_SLUG}.md"
mkdir -p "$(dirname "$DRAFT_FILE")"
echo "$DRAFT_CONTENT" > "$DRAFT_FILE"
echo "CSA_VAR:DRAFT_FILE=$DRAFT_FILE"
echo "CSA_VAR:STEP4_SESSION_ID=$DRAFT_SID"
```

## Step 5: Propose via PR

Tool: bash

Create a proposal PR with the rule draft. NEVER auto-commit rules to
the main rules repository. Human review is mandatory. Reads draft
content from `DRAFT_FILE` produced by Step 4.

```bash
set -euo pipefail

# Read draft content from Step 4 output file
if [ ! -f "${DRAFT_FILE}" ]; then
  echo "ERROR: DRAFT_FILE not found at ${DRAFT_FILE}"
  exit 1
fi

# Determine target directory (project-local, fork-only per rule 030)
RULE_DIR="docs/rules-proposed/${LANG}"
mkdir -p "${RULE_DIR}"

# Determine next rule number (safe for empty directory)
EXISTING=$(ls "${RULE_DIR}/" 2>/dev/null | grep -E '^[0-9]{3}-' || true)
if [ -n "${EXISTING}" ]; then
  LAST_NUM=$(echo "${EXISTING}" | sort -n | tail -1 | cut -c1-3)
else
  LAST_NUM="000"
fi
NEXT_NUM=$(printf '%03d' $((10#${LAST_NUM} + 1)))
RULE_FILE="${NEXT_NUM}-${BUG_CLASS_SLUG}.md"

# Create proposal branch (include BUG_CLASS_SLUG for uniqueness across findings)
SHORT_SHA=$(echo "${FIX_COMMIT_SHA}" | cut -c1-8)
PROPOSAL_BRANCH="chore/rules-propose-${SHORT_SHA}-${BUG_CLASS_SLUG}"
git checkout -b "${PROPOSAL_BRANCH}"

# Copy draft to final rule location
cp "${DRAFT_FILE}" "${RULE_DIR}/${RULE_FILE}"
# Clean up draft file
rm -f "${DRAFT_FILE}"

git add "${RULE_DIR}/${RULE_FILE}"
git commit -m "chore(rules): propose ${LANG}/${RULE_FILE} from PR #${PR_NUM}

Extracted from HIGH/CRITICAL finding in PR #${PR_NUM}.
Bug class: ${BUG_CLASS_NAME}
Source: pr-bot-finding auto-extraction (issue #661)"

git push -u origin "${PROPOSAL_BRANCH}"
gh pr create \
  --title "chore(rules): propose ${LANG}/${RULE_FILE} — ${BUG_CLASS_NAME}" \
  --body "## Rule Proposal (auto-extracted)

Source PR: #${PR_NUM}
Severity: ${FINDING_SEVERITY}
Bug class: ${BUG_CLASS_NAME}

Auto-extracted by rule-extractor pattern (issue #661). Human review required.

### Checklist
- [ ] Rule accurately describes the bug class
- [ ] Anti-patterns are correct and actionable
- [ ] Preferred patterns are structurally safe
- [ ] Decision checklist is clear for agents
- [ ] No overlap with existing rules"
```

On merge of the proposal PR, the relevant AGENTS.md index is updated
with one compact line per rule 034:

```
NNN|bug-class-slug|one-line summary of the rule
```

## Variables

### Workflow template variables (declared in workflow.toml)

- `${REPO}`: GitHub repository slug (owner/repo).
- `${PR_NUM}`: Merged PR number.
- `${REVIEW_SESSION_ID}`: CSA review session ID (optional, for csa review findings).
- `${FINDING_DESCRIPTION}`: Description of the current finding being processed.
- `${FINDING_SEVERITY}`: Finding severity (HIGH/CRITICAL/P1), emitted by Step 1, used in frontmatter and PR body.
- `${FINDING_FILE}`: File path of the finding.
- `${FIX_COMMIT_SHA}`: Commit SHA of the fix.
- `${FIX_DIFF_SUMMARY}`: Summary of the fix diff.
- `${BUG_CLASS_NAME}`: Human-readable name of the classified bug class.
- `${BUG_CLASS_DESCRIPTION}`: Detailed description of the bug class.
- `${BUG_CLASS_KEYWORDS}`: Keywords for grep-based deduplication.
- `${BUG_CLASS_SLUG}`: URL-safe slug for file naming.
- `${LANG}`: Target language directory (rust, go, py, ts, all-lang).
- `${ANTI_PATTERN_EXAMPLES}`: Code examples of the anti-pattern (deprecated — use B64 variant).
- `${ANTI_PATTERN_EXAMPLES_B64}`: Base64-encoded multi-line anti-pattern code examples (emitted by Step 2, decoded in Step 4).
- `${CORRECT_PATTERN}`: Code examples of the correct pattern (deprecated — use B64 variant).
- `${CORRECT_PATTERN_B64}`: Base64-encoded multi-line correct pattern code examples (emitted by Step 2, decoded in Step 4).
- `${RULE_CONTENT}`: Generated rule file content (deprecated — use DRAFT_FILE).
- `${SHOULD_DRAFT}`: Set to "yes" by Step 3 when Step 4 should run (empty on EXACT_MATCH).
- `${DEDUPE_RESULT}`: Deduplication outcome from Step 3 (EXACT_MATCH|PARTIAL_MATCH|NO_MATCH).
- `${DRAFT_FILE}`: Path to draft rule file written by Step 4 (read by Step 5).
- `${STEP4_SESSION_ID}`: Session ID of the Step 4 CSA run (for audit).

### Filter criteria (enforced before Step 2)

1. Finding severity is HIGH/CRITICAL/P1.
2. False-positive check passed (finding was fixed, not dismissed via debate).
3. Finding is a bug CLASS (Step 2 classification).
4. Fix is not trivially single-line (structural change required).

## Integration

- **Invoked by**: pr-bot (post-merge, opt-in) via `csa plan run --sa-mode true patterns/rule-extractor/workflow.toml`
- **Depends on**: pr-bot review artifacts (findings, debate verdicts, fix commits)
- **Outputs to**: `docs/rules-proposed/<lang>/` (project-local, fork-only per rule 030)
- **Constraint**: NEVER auto-commits. Always proposes via PR for human review.
- **Constraint**: AGENTS.md rule 030 (fork-only) — PRs target user's fork.

## Done Criteria

1. All HIGH/CRITICAL findings from the merged PR classified (bug class vs isolated).
2. Bug classes deduplicated against existing rules.
3. New rules drafted with correct structure (anti-patterns, preferred patterns, decision checklist, case study).
4. Proposal PR(s) created with human review required.
5. No auto-commits to rules repositories.
