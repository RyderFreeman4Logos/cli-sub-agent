---
name: pr-codex-bot
description: "Iterative PR review loop with OpenAI Codex bot. Fetches bot comments, evaluates for false positives, fixes real issues, pushes, and triggers re-review until clean. Triggers on: codex bot, pr bot loop, bot review, codex review loop"
allowed-tools: Bash, Task, Read, Edit, Write, Grep, Glob
---

# PR Codex Bot Review Loop

Orchestrates an iterative fix-and-review loop with the OpenAI Codex bot on GitHub PRs.

## Workflow

```
Step 1: Commit changes
       |
       v
Step 2: Local review (codex-noninteractive-review-orchestrator)
       |
       v
Step 3: Fix local review issues (loop until clean)
       |
       v
Step 4: Submit PR + Review Trigger Procedure
       |                    |
       |        (UNAVAILABLE? → Merge directly — local review already covers main...HEAD)
       |
       v
Step 5: (Handled by Review Trigger Procedure — bounded poll + timeout)
       |
       v
Step 6: No issues? ──> Merge (remote + local) ✅ DONE
       |
       Has issues
       |
       v
Step 7: Evaluate each comment
       ├── False positive → Step 8: Debate (@codex in reply)
       └── Real issue ────→ Step 9: Fix + push + Review Trigger Procedure
                                    |
                                    v
                             Step 10: Review result → Has issues? → Back to Step 7
                                    |
                                    No issues
                                    |
                                    v
                             Step 11: Clean resubmission (new branch + new PR)
                                    |
                                    v
                             Step 12: Review Trigger Procedure → Has issues? → Back to Step 7
                                    |
                                    No issues
                                    |
                                    v
                             Step 13: Merge (remote + local) ✅ DONE
```

## FORBIDDEN Actions (VIOLATION = SOP breach)

- **NEVER dismiss a bot comment as "false positive" using your own reasoning alone** — you are the code author; your judgment is inherently biased
- **NEVER reply to a bot comment without completing Step 8 (local arbitration)** — even if the false positive seems "obvious", the arbiter will confirm instantly
- **NEVER skip the debate step for any reason** — "too simple", "clearly wrong", "design disagreement" are NOT valid excuses
- **NEVER post a dismissal comment without full model specs** (`tool/provider/model/thinking_budget`) for both debate participants
- **NEVER use the same model family for arbitration** as you (the main agent)
- **NEVER run Step 2 (local review) as a background task** — it MUST complete synchronously before Step 4 (push PR). Running it in background causes merge-before-review bugs
- **NEVER create or submit a PR without completing Step 2 (pre-PR local review)** — this is an ABSOLUTE, NON-NEGOTIABLE prerequisite. No local review = No PR. No exceptions, no "the changes are trivial", no "I'll review after". The local review (scope `main...HEAD`) is the FOUNDATION of the entire workflow — without it, cloud bot unavailability cannot safely fall through to merge.

**If you believe a bot comment is wrong, you MUST:**
1. Queue it for Step 8 (local arbitration) — NOT reply directly
2. Get an independent verdict from a different backend via CSA
3. If the arbiter disagrees with you, debate adversarially (Step 8.3b)
4. Post the full audit trail (with model specs for BOTH sides) to the PR comment

**Any self-dismissal without arbitration is an SOP VIOLATION that undermines the entire review process. The point of independent review is that no single model — including you — gets to be judge of its own code.**

## Parameters

Extract from user message or PR context:

| Param | Source | Example |
|-------|--------|---------|
| `REPO` | PR URL or git remote | `user/repo` |
| `PR_NUM` | PR URL or `gh pr view` | `1` |
| `BRANCH` | Current git branch | `feat/hooks-system` |
| `WORKFLOW_BRANCH` | Set once in Step 1 (= `BRANCH` at start) | `feat/hooks-system` |

**CRITICAL**: `WORKFLOW_BRANCH` is the original branch name set once at
workflow start. It MUST NOT be re-derived from `git branch --show-current`
after Step 11 branch switches (e.g., `${BRANCH}-clean`).

> **See**: [Baseline Capture Template](references/baseline-template.md) — run this before every `@codex review` trigger.

## Step 1: Commit Changes

Commit work using proper commit workflow. Ensure all changes are staged
and committed before proceeding.

```bash
# Set WORKFLOW_BRANCH once — persists through Step 11 clean branch switches
WORKFLOW_BRANCH="$(git branch --show-current)"
```

## Step 2: Local Review (ABSOLUTE PREREQUISITE — MUST BLOCK)

**THIS IS THE MOST CRITICAL STEP IN THE ENTIRE WORKFLOW.**

The pre-PR local review (`main...HEAD`) is the FOUNDATION that makes the entire
workflow safe. It is what allows us to merge directly when the cloud bot is
unavailable. **Without this step, the PR MUST NOT be created — period.**

**CRITICAL: This step MUST run synchronously. NEVER launch the local review as a
background task.** The merge-before-review bug occurs when the local review runs in
the background while subsequent steps proceed without waiting for results.

Run `csa-review` skill with `scope=range:main...HEAD` to audit all changes since main
before submitting the PR. This catches cross-commit interaction issues early and reduces bot iterations.
Sessions are stored in `~/.local/state/csa/` (not `~/.codex/`).

**FORBIDDEN**: `run_in_background: true` for the local review command. You MUST wait
for the review output before proceeding.

After completing the local review:
```bash
# Use LOCAL_REVIEW_MARKER (not TMP_PREFIX) — PR_NUM not known until Step 4
LOCAL_REVIEW_MARKER="/tmp/codex-local-review-${REPO//\//-}-${BRANCH//\//-}.marker"
git rev-parse HEAD > "${LOCAL_REVIEW_MARKER}"
```

## Step 3: Fix Local Review Issues (GATE)

```bash
# Verify Step 2 was performed for current HEAD
LOCAL_REVIEW_MARKER="/tmp/codex-local-review-${REPO//\//-}-${BRANCH//\//-}.marker"
if [ ! -f "${LOCAL_REVIEW_MARKER}" ] || [ "$(cat "${LOCAL_REVIEW_MARKER}")" != "$(git rev-parse HEAD)" ]; then
  echo "ERROR: Local review marker missing or stale (HEAD changed). Re-run Step 2."
  exit 1
fi
```

If the local review found issues:
1. Fix each issue
2. Commit fixes
3. Re-run local review (synchronously — same blocking rule as Step 2),
   then update the marker: `git rev-parse HEAD > "${LOCAL_REVIEW_MARKER}"`
4. Repeat until clean

**HARD GATE: Only proceed to Step 4 when local review returns zero issues.
No exceptions. No "review is probably fine". No skipping. No "changes are trivial".
If the local review has not been completed for the current HEAD, the PR MUST NOT
be created. This gate is what makes UNAVAILABLE → direct merge safe.**

## Review Trigger Procedure (Single Entry Point)

**All review triggers (Steps 4, 9, 11/12) MUST use this unified procedure.**

> **See**: [Review Trigger Procedure](references/review-trigger-procedure.md) — complete procedure with baseline capture, bounded poll loop, quota detection, and direct merge on UNAVAILABLE.

**Quick reference** — Normalized review outcomes:

| Outcome | Meaning | Next Action |
|---------|---------|-------------|
| `CLEAN` | No issues found | Proceed to merge path |
| `HAS_ISSUES` | Reviewer found issues | Proceed to Step 7 (evaluate) |
| `UNAVAILABLE(quota)` | Cloud bot quota exhausted | **Merge directly** — local review already covers `main...HEAD` |
| `UNAVAILABLE(timeout)` | Cloud bot did not respond in 10 min | **Merge directly** — local review already covers `main...HEAD` |
| `ESCALATE(api_error)` | GitHub API failed (transient) | **Notify user** — bot may still be reviewing, retry or decide |

**Phases**: (1) Cloud path: baseline + `@codex review` + bounded poll (max 10 min, max 5 API failures) → (2) If `UNAVAILABLE(quota/timeout)`: verify LOCAL_REVIEW_MARKER exists and matches HEAD, then **merge directly**. If `ESCALATE(api_error)`: notify user (transient failure, not conclusive).

**Rationale for direct merge on UNAVAILABLE**: Step 2 (pre-PR local review) already
reviews the FULL `main...HEAD` range — the exact same scope the cloud bot would review.
When the cloud bot is deterministically unavailable (quota exhausted, timeout), the local
review has ALREADY provided equivalent independent coverage. No fallback review, no
user prompt, no fallback marker needed. Just merge.

**Why api_error is ESCALATE, not UNAVAILABLE**: API errors are transient (network
issues, GitHub outages, permission problems). The bot may still be processing the
review — we just can't read the result. Unlike quota (bot explicitly says "can't
review") or timeout (bot didn't respond at all), api_error doesn't prove the bot
won't review. Escalate to user for retry/decision.

**PREREQUISITE**: Direct-merge-on-UNAVAILABLE is ONLY safe when LOCAL_REVIEW_MARKER
matches the current HEAD. Step 9 MUST refresh the marker after fixes (re-run local
review). If the marker is missing or stale, direct merge is FORBIDDEN.

## Step 4: Submit PR

```bash
# Final gate: ensure local review was performed for current HEAD
LOCAL_REVIEW_MARKER="/tmp/codex-local-review-${REPO//\//-}-${BRANCH//\//-}.marker"
if [ ! -f "${LOCAL_REVIEW_MARKER}" ] || [ "$(cat "${LOCAL_REVIEW_MARKER}")" != "$(git rev-parse HEAD)" ]; then
  echo "ERROR: Local review marker missing or stale. Re-run Step 2 before submitting PR."
  exit 1
fi

git push -u origin "${BRANCH}"

gh pr create --title "[type](scope): [description]" \
  --body "$(cat <<'PREOF'
## Summary
[bullet points]

## Test plan
- [ ] `cargo clippy -p [package] -- -D warnings`
- [ ] `cargo test -p [package]`
- [ ] @codex review

🤖 Generated with [Claude Code](https://claude.com/claude-code)
PREOF
)"

# Initialize TMP_PREFIX for this PR
TMP_PREFIX="/tmp/codex-bot-${REPO//\//-}-${PR_NUM}"
```

**Now follow the [Review Trigger Procedure](#review-trigger-procedure-single-entry-point)**
to trigger cloud review and wait for results.

## Step 5: Poll for Bot Response

Handled by the **Review Trigger Procedure** (Phase 2c). The procedure
implements a bounded poll loop (max 10 min) with API error retry limits.
See the procedure for the complete polling code with timeout.

## Step 6: First-Pass Clean → Merge

If the bot's first review produces **no inline comments** (only a review-level
response), the PR is clean on first pass:

```bash
# Merge remotely
gh pr merge "${PR_NUM}" --repo "${REPO}" --squash --delete-branch

# Update local main
git checkout main && git pull origin main
```

**This path only applies when the bot approves on the FIRST review with
zero fix iterations.** If any fixes were needed, go through Steps 7-13 instead.

## Step 7: Evaluate Bot Comments

For each new bot comment, classify:

### Category A: Already Fixed

The bot reviewed an older commit; the issue is already addressed.

**Detection**: Read the file at the path mentioned. If the code no longer
matches what the bot described, it's already fixed.

**Action**: React 👍 + reply acknowledging (**do NOT `@codex`** — avoid wasting bot tokens).
Use the correct API based on where the bot posted:

```bash
# For PR review comments (inline, found via pulls/{PR}/comments)
gh api "repos/${REPO}/pulls/comments/${COMMENT_ID}/reactions" \
  -X POST -f content='+1'
gh api "repos/${REPO}/pulls/${PR_NUM}/comments" \
  -X POST -f body="Fixed in ${COMMIT_SHA}. [brief explanation]." \
  -F "in_reply_to=${COMMENT_ID}"

# For issue-level comments (general, found via issues/{PR}/comments)
gh api "repos/${REPO}/issues/comments/${COMMENT_ID}/reactions" \
  -X POST -f content='+1'
gh api "repos/${REPO}/issues/${PR_NUM}/comments" \
  -X POST -f body="Fixed in ${COMMIT_SHA}. [brief explanation]."
```

### Category B: Suspected False Positive

The bot's suggestion appears incorrect or the code appears correct as-is.

**Common false positives**:
- Dead code: bot flags a reachable branch not exercised by the current caller
- Over-caution: type system already prevents the scenario
- Stale context: cross-file changes already resolve the issue
- Design disagreement: bot suggests a different but not better approach

**Action**: **Do NOT react or reply yet.** Queue for Step 8 (local arbitration).

The cloud bot has limited context and cannot execute commands to verify its
claims. Do not debate with it directly — instead, get an independent local
second opinion first.

**SOP VIOLATION WARNING**: You MUST NOT skip Step 8 for ANY Category B comment.
Even if you are "99% sure" it is a false positive, you MUST get an independent
model verdict via CSA. Your confidence as the code author is irrelevant —
the entire point of this process is that **no single model judges its own code**.

Replying directly with your own reasoning (e.g., "This is a design choice,
dismissing.") without completing Step 8 is a **FORBIDDEN action** (see above).

### Category C: Real Issue

The bot found a genuine bug or improvement.

**Action**: React 👍 → **queue for Step 9** (**do NOT `@codex`** — avoid wasting
bot tokens; the fix + `@codex review` in Step 9 will trigger re-evaluation).
Use the correct API based on where the bot posted:
```bash
# For PR review comments (inline)
gh api "repos/${REPO}/pulls/comments/${COMMENT_ID}/reactions" \
  -X POST -f content='+1'

# For issue-level comments (general)
gh api "repos/${REPO}/issues/comments/${COMMENT_ID}/reactions" \
  -X POST -f content='+1'
```

### Evaluation Priority

**MUST treat P1 as real unless proven false.** P2/P3 get more scrutiny:

| Priority | Default Assumption | Override When |
|----------|-------------------|---------------|
| P1 | Real issue | Type system prevents it OR code path unreachable |
| P2 | Needs verification | Often valid but sometimes over-cautious |
| P3 | Low priority | Consider deferring if non-critical |

## Step 8: Local Arbitration for Suspected False Positives

**Do NOT debate with the cloud bot directly.** The bot cannot execute commands,
trace cross-module dependencies, or verify its claims. Instead, use a local
arbitration process with an **independent model via CSA**.

### Independent Model Requirement

**CRITICAL**: The arbiter MUST be routed through CSA to ensure independent evaluation from:
- The code author (you, the main agent)
- The cloud bot reviewer

This is the core value of arbitration — different reasoning systems create "cognitive friction" that catches issues a single model would miss. CSA handles tool routing internally to ensure independence.

| You (main agent) | Cloud Bot | Local Arbiter (CSA routes) |
|-------------------|-----------|---------------------------|
| Claude Opus 4.6 | GPT-5.3-Codex | `csa debate` → CSA auto-routes |
| GPT-based agent | GPT-5.3-Codex | `csa debate` → CSA auto-routes |
| Gemini-based agent | GPT-5.3-Codex | `csa debate` → CSA auto-routes |

**Preferred arbiter**: Use `csa debate` which automatically selects an appropriate backend based on configuration. CSA's internal routing ensures independent evaluation.

### Step 8.1: Get Independent Local Opinion

Use `csa debate` to spawn an independent arbiter. CSA automatically routes to an appropriate backend:

```bash
csa debate "A code reviewer flagged the following issue in [file:line]:

[paste bot's comment verbatim]

The relevant code is at [file path]. Please read the code and
evaluate whether this is a genuine issue or a false positive.
Do NOT assume the reviewer is right or wrong — form your own
independent assessment based on the actual code."
```

**NOTE**: `csa debate` reads `[debate]` config for tool selection. If `tool = "auto"`
(default), CSA automatically routes to an appropriate backend based on configuration.
Auto mode ensures independent evaluation. If auto routing cannot determine an appropriate
backend, you may need to specify `--tool` explicitly or configure `[debate].tool` in config.

**CRITICAL**: Do NOT tell the arbiter your own opinion. Let it form
an independent judgment.

### Step 8.2: Branch on Arbiter Verdict

```
Arbiter says...
    │
    ├── "False positive" ──→ Step 8.3a (dismiss)
    │
    ├── "Real issue" ──→ Step 8.3b (YOU debate with arbiter)
    │
    └── "Uncertain" ──→ Step 8.3b (YOU debate with arbiter)
```

### Step 8.3a: Arbiter Confirms False Positive

React 👎 on PR and post the arbitration result as audit trail with **full model specs**
(**do NOT `@codex`**). Use the correct API based on where the bot posted:

```bash
# For PR review comments (inline)
gh api "repos/${REPO}/pulls/comments/${COMMENT_ID}/reactions" \
  -X POST -f content='-1'
gh api "repos/${REPO}/pulls/${PR_NUM}/comments" \
  -X POST \
  -f body="**Dismissed after local arbitration.**

**Participants:**
- Author: \`{your_tool}/{your_provider}/{your_model}/{your_thinking_budget}\`
- Arbiter: \`{arbiter_tool}/{arbiter_provider}/{arbiter_model}/{arbiter_thinking_budget}\`

**Reasoning:** [summary of arbiter reasoning]. [cite file:line evidence]." \
  -F "in_reply_to=${COMMENT_ID}"

# For issue-level comments (general)
gh api "repos/${REPO}/issues/comments/${COMMENT_ID}/reactions" \
  -X POST -f content='-1'
gh api "repos/${REPO}/issues/${PR_NUM}/comments" \
  -X POST \
  -f body="**Dismissed after local arbitration.**

**Participants:**
- Author: \`{your_tool}/{your_provider}/{your_model}/{your_thinking_budget}\`
- Arbiter: \`{arbiter_tool}/{arbiter_provider}/{arbiter_model}/{arbiter_thinking_budget}\`

**Reasoning:** [summary of arbiter reasoning]. [cite file:line evidence]."
```

**MANDATORY**: Model specs MUST use the `tool/provider/model/thinking_budget` format
(matching CSA tiers). This enables future reviewers to verify independent arbitration.

### Step 8.3b: Arbiter Confirms Real Issue or Uncertain → YOU Debate

**CRITICAL: YOU (the main agent / code author) MUST debate with the arbiter.**
Do NOT just accept the arbiter's verdict. You wrote the code — defend your
design decisions. The independent adversarial debate IS the value of this step.

**Resume the same debate session** (via `csa debate --session <id>`) and engage in
adversarial debate:

1. **YOU present counter-arguments** — why your design choice was intentional,
   what context the arbiter may have missed, what tradeoffs you considered
2. **Arbiter responds** with evidence for/against
3. **YOU rebut** with code references, cross-module context, or design rationale
4. **Iterate** until consensus or deadlock (max 3 rounds)

```bash
# Continue the debate with your counter-argument
csa debate --session <SESSION_ID> "I disagree because [your reasoning].
The code is intentionally designed this way because [rationale]."
```

**The debate is routed through CSA for independent evaluation:**
```
YOU (Claude Opus 4.6, code author)
    ↕  adversarial debate via csa debate  ↕
Arbiter (CSA-routed backend, independent evaluator)
```

This is where independent evaluation creates real value — if multiple different
reasoning systems independently conclude the same thing after debate, the
confidence is much higher than a single model's judgment.

**After debate concludes**:

| Outcome | Action |
|---------|--------|
| Both agree: real issue | React 👍 + queue for Step 9 (fix) |
| Both agree: false positive | React 👎 + post reasoning (Step 8.3a) |
| You convinced arbiter | React 👎 + post reasoning with debate trail |
| Arbiter convinced you | React 👍 + queue for Step 9 (fix) |
| Deadlock (each side has valid points) | **Escalate to user** |

Post the full debate summary as a PR comment for audit trail with **full model specs**
(**do NOT `@codex`**). Use the correct reply API based on where the bot posted:

```bash
# For PR review comments (inline) — use pulls/{PR}/comments with in_reply_to
gh api "repos/${REPO}/pulls/${PR_NUM}/comments" \
  -X POST \
  -f body="**Local arbitration result: [DISMISSED|CONFIRMED|ESCALATED].**
...
[debate body omitted for brevity — see template below]" \
  -F "in_reply_to=${COMMENT_ID}"

# For issue-level comments (general) — use issues/{PR}/comments
gh api "repos/${REPO}/issues/${PR_NUM}/comments" \
  -X POST \
  -f body="**Local arbitration result: [DISMISSED|CONFIRMED|ESCALATED].**
...
[debate body — same content as above]"
```

**Debate body template** (use in both API variants above):
```
**Local arbitration result: [DISMISSED|CONFIRMED|ESCALATED].**

## Participants (MANDATORY for auditability)
- **Author**: \`{your_tool}/{your_provider}/{your_model}/{your_thinking_budget}\`
- **Arbiter**: \`{arbiter_tool}/{arbiter_provider}/{arbiter_model}/{arbiter_thinking_budget}\`

## Bot's concern
[bot comment summary]

## Arbiter's independent assessment
[arbiter's initial verdict and reasoning]

## Debate (Author vs Arbiter)
### Round 1
- **Author** (\`{your_model}\`): [your counter-argument]
- **Arbiter** (\`{arbiter_model}\`): [arbiter's response]
### Round 2 (if needed)
- **Author** (\`{your_model}\`): [your rebuttal]
- **Arbiter** (\`{arbiter_model}\`): [arbiter's response]

## Conclusion
[final verdict, which side prevailed, and rationale]

## Audit
- Debate rounds: {N}
- CSA session: \`{session_id}\` (if applicable)
- Debate skill used: [yes/no — if complex, the \`debate\` skill provides structured multi-round debate]
```

**MANDATORY**: Both model specs MUST use the `tool/provider/model/thinking_budget` format.
The audit section enables future reviewers (human or AI) to verify that independent
models were used and assess the quality of the arbitration.

### `@codex` Tagging Rules

- **NEVER `@codex` in false positive replies** — the bot ignores threaded
  debates anyway, and tagging wastes its tokens
- **NEVER `@codex` in real issue / already-fixed replies** — the fix commit
  + `@codex review` in Step 9 will trigger re-evaluation
- **The ONLY place to `@codex`** is `gh pr comment` to trigger a full
  re-review (Steps 4, 9, 11) — handled by the Review Trigger Procedure

## Step 9: Fix Real Issues

Delegate via CSA tier-based model selection or fix directly:

| Issue Complexity | Approach | Notes |
|-----------------|----------|-------|
| Simple (1-3 lines) | Main agent directly | Just fix it (delegation overhead > cost) |
| Moderate (logic change) | `csa run --tier tier-2-standard` | Provide file path + description |
| Complex (architectural) | `csa run --tier tier-3-complex` or escalate to user | May need user input |

```bash
# After fixing
git add [fixed files]
git commit -m "fix(scope): [description]

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

**MANDATORY: Re-run local review before push** — the fix has moved HEAD, so the
LOCAL_REVIEW_MARKER from Step 2 is now stale. The direct-merge-on-UNAVAILABLE
path requires a fresh marker matching the current HEAD.

```bash
# Re-run local review for updated HEAD (same blocking rule as Step 2)
csa review --branch main
LOCAL_REVIEW_MARKER="/tmp/codex-local-review-${REPO//\//-}-${BRANCH//\//-}.marker"
git rev-parse HEAD > "${LOCAL_REVIEW_MARKER}"

git push origin "${BRANCH}"
```

**Now follow the [Review Trigger Procedure](#review-trigger-procedure-single-entry-point)**
to trigger cloud review and wait for results.
Then re-evaluate (Step 7).

## Step 10: Re-Review Loop

After pushing fixes, follow the **[Review Trigger Procedure](#review-trigger-procedure-single-entry-point)**:
- Result is `HAS_ISSUES` → back to Step 7 (evaluate, debate/fix)
- Result is `CLEAN` → proceed to Step 11 (clean resubmission)
- Result is `UNAVAILABLE(quota/timeout)` → merge directly (local review refreshed in Step 9)
- Result is `ESCALATE(api_error)` → notify user (transient failure, retry or decide)

## Step 11: Clean Resubmission

When the bot converges after fix iterations, the PR has accumulated
incremental fix commits. Create a clean PR for audit-friendly history.

> **See**: [Clean Resubmission Flow](references/clean-resubmission-flow.md) — detailed procedure for creating clean PRs.

## Step 12: Review New PR

Follow the **[Review Trigger Procedure](#review-trigger-procedure-single-entry-point)**
on the new clean PR (update `PR_NUM` and `TMP_PREFIX` for the new PR first):
- Result is `CLEAN` → Step 13 (merge)
- Result is `HAS_ISSUES` → back to Step 7 (evaluate each comment, debate/fix)
  - If fixes are needed again, repeat the full cycle including
    another clean resubmission (Step 11) with incrementing branch
    names: `${BRANCH}-clean-2`, `${BRANCH}-clean-3`, etc.
- Result is `UNAVAILABLE(quota/timeout)` → merge directly (local review covers `main...HEAD`)
- Result is `ESCALATE(api_error)` → notify user (transient failure, retry or decide)

## Step 13: Merge

```bash
# Merge remotely
gh pr merge "${NEW_PR_NUM}" --repo "${REPO}" --squash --delete-branch

# Update local main
git checkout main && git pull origin main
```

## Loop Control

| Control | Value | Rationale |
|---------|-------|-----------|
| Max iterations per PR | 10 | Prevents infinite loop |
| Same issue threshold | 2 | If bot flags same issue twice, escalate |
| Poll interval | 45-60s | Within GitHub rate limits |
| Poll timeout | 10 min | Notify user if exceeded |

### Exit Conditions

| Condition | Action |
|-----------|--------|
| No issues on first review | **Step 6** - direct merge |
| No issues after fix iterations | **Step 11** - clean resubmission |
| No issues on clean PR | **Step 13** - merge |
| Max iterations reached | **Escalate** - report to user |
| Same issue re-flagged after fix | **Escalate** - root cause missed |
| Bot flags architectural issue | **Escalate** - needs human decision |
| Poll timeout (10 min) | **UNAVAILABLE(timeout)** — merge directly (local review covers `main...HEAD`) |
| Bot replies with quota message | **UNAVAILABLE(quota)** — merge directly (local review covers `main...HEAD`) |
| API errors (5 consecutive) | **ESCALATE(api_error)** — notify user (transient failure, bot may still be reviewing) |

## Anti-Trust Protocol

**NEVER blindly fix everything the bot suggests.** The bot:

- Has limited context (reviews diff, not full codebase)
- Cannot trace cross-module dependencies
- May flag defensive code as unnecessary
- May suggest changes that break callers

**For each fix, verify**:
1. Does the fix compile? (`cargo clippy`)
2. Do existing tests pass? (`cargo test`)
3. Does the fix preserve the original design intent?
4. Could the fix introduce a regression?

## GitHub API Reference

**IMPORTANT**: GitHub has THREE different comment APIs for PRs:

| Type | Endpoint | When Bot Uses It |
|------|----------|------------------|
| **Issue comments** (general) | `GET/POST repos/{REPO}/issues/{PR}/comments` | **Primary** — bot posts here |
| **Review comments** (inline) | `GET repos/{REPO}/pulls/{PR}/comments` | When bot has line-specific feedback |
| **Reviews** (approve/reject) | `GET repos/{REPO}/pulls/{PR}/reviews` | Formal review submissions |

**CRITICAL**: The bot (`chatgpt-codex-connector[bot]`) primarily posts to
**issue-level comments** (`issues/{PR}/comments`), but may also use PR review
comments (`pulls/{PR}/comments`) for inline feedback. Polling MUST check all
three endpoints (issue comments, review comments, reviews).

| Operation | Endpoint | Notes |
|-----------|----------|-------|
| List issue comments | `GET repos/{REPO}/issues/{PR}/comments` | Bot's primary channel |
| List review comments | `GET repos/{REPO}/pulls/{PR}/comments` | Inline code comments |
| Get one review comment | `GET repos/{REPO}/pulls/comments/{ID}` | No PR number! |
| Reply to review comment | `POST repos/{REPO}/pulls/{PR}/comments` with `-F in_reply_to={ID}` | Uses PR number |
| Add reaction (review) | `POST repos/{REPO}/pulls/comments/{ID}/reactions` with `-f content='+1'` or `'-1'` | No PR number! |
| Add reaction (issue) | `POST repos/{REPO}/issues/comments/{ID}/reactions` with `-f content='+1'` or `'-1'` | No PR number! |

Bot user login: `chatgpt-codex-connector[bot]`

## Integration

| Skill | When to Use |
|-------|-------------|
| `csa-review` | Step 2: local review before PR (sessions in `~/.local/state/csa/`) |
| `debate` | Step 8: adversarial arbitration for suspected false positives |
| `commit` | After fixing issues |
| `csa run --tier tier-4-critical` | If bot flags security issue (deep analysis) |
