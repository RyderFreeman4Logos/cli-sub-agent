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
Step 4: Submit PR + @codex review
       |
       v
Step 5: Poll for bot response (NEVER assume, ALWAYS poll)
       |
       v
Step 6: No issues? ──> Merge (remote + local) ✅ DONE
       |
       Has issues
       |
       v
Step 7: Evaluate each comment
       ├── False positive → Step 8: Debate (@codex in reply)
       └── Real issue ────→ Step 9: Fix + push + @codex review
                                    |
                                    v
                             Step 10: Poll → Has issues? → Back to Step 7
                                    |
                                    No issues
                                    |
                                    v
                             Step 11: Clean resubmission (new branch + new PR)
                                    |
                                    v
                             Step 12: Poll new PR → Has issues? → Back to Step 7
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

**If you believe a bot comment is wrong, you MUST:**
1. Queue it for Step 8 (local arbitration) — NOT reply directly
2. Get an independent verdict from a heterogeneous model via CSA
3. If the arbiter disagrees with you, debate adversarially (Step 8.3b)
4. Post the full audit trail (with model specs for BOTH sides) to the PR comment

**Any self-dismissal without arbitration is an SOP VIOLATION that undermines the entire review process. The point of heterogeneous review is that no single model — including you — gets to be judge of its own code.**

## Parameters

Extract from user message or PR context:

| Param | Source | Example |
|-------|--------|---------|
| `REPO` | PR URL or git remote | `user/repo` |
| `PR_NUM` | PR URL or `gh pr view` | `1` |
| `BRANCH` | Current git branch | `feat/hooks-system` |

### Temp File Naming Convention

All temp files are namespaced by `${REPO}` and `${PR_NUM}` to prevent
collisions when multiple projects use this skill concurrently:

```
/tmp/codex-bot-${REPO//\//-}-${PR_NUM}-baseline.json
/tmp/codex-bot-${REPO//\//-}-${PR_NUM}-poll-result.txt
/tmp/codex-bot-${REPO//\//-}-${PR_NUM}-watch.sh
```

Example for `user/repo` PR `3`: `/tmp/codex-bot-user-repo-3-baseline.json`

## Step 1: Commit Changes

Commit work using proper commit workflow. Ensure all changes are staged
and committed before proceeding.

## Step 2: Local Review

Run `csa-review` skill with `scope=base:main` to audit `origin/main...HEAD`
before submitting the PR. This catches issues early and reduces bot iterations.
Sessions are stored in `~/.local/state/csa/` (not `~/.codex/`).

## Step 3: Fix Local Review Issues

If the local review found issues:
1. Fix each issue
2. Commit fixes
3. Re-run local review
4. Repeat until clean

**Only proceed to Step 4 when local review passes.**

## Step 4: Submit PR

```bash
git push -u origin ${BRANCH}

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

# Trigger bot review
gh pr comment ${PR_NUM} --repo ${REPO} --body "@codex review"
```

## Step 5: Poll for Bot Response

**CRITICAL: NEVER assume the bot has replied. ALWAYS actively poll.**

```bash
# Record baseline
gh api "repos/${REPO}/pulls/${PR_NUM}/comments" \
  --jq '[.[] | select(.user.login == "chatgpt-codex-connector[bot]") | .id]' \
  > /tmp/codex-bot-${REPO//\//-}-${PR_NUM}-baseline.json

# Poll every 45-60s (within GitHub rate limits: 5000 req/hour authenticated)
while true; do
  sleep 45
  CURRENT=$(gh api "repos/${REPO}/pulls/${PR_NUM}/comments" \
    --jq '[.[] | select(.user.login == "chatgpt-codex-connector[bot]") | .id]')
  BASELINE=$(cat /tmp/codex-bot-${REPO//\//-}-${PR_NUM}-baseline.json)
  if [ "$CURRENT" != "$BASELINE" ]; then
    echo "NEW_COMMENTS_DETECTED"
    break
  fi
  # Check for review-only response (approved, no new inline comments)
  # IMPORTANT: Compare timestamps in UTC to avoid timezone bugs
  LATEST_REVIEW_UTC=$(gh api "repos/${REPO}/pulls/${PR_NUM}/reviews" \
    --jq '[.[] | select(.user.login == "chatgpt-codex-connector[bot]")] | last | .submitted_at')
  PUSH_UTC=$(git log -1 --format=%cI | TZ=UTC date -f - +%Y-%m-%dT%H:%M:%SZ 2>/dev/null)
  # If latest review is after push AND no new comments, bot approved
done
```

**Timeout**: If no response after 10 minutes, **notify the user** instead of
guessing or acting on stale data.

## Step 6: First-Pass Clean → Merge

If the bot's first review produces **no inline comments** (only a review-level
response), the PR is clean on first pass:

```bash
# Merge remotely
gh pr merge ${PR_NUM} --repo ${REPO} --squash --delete-branch

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

**Action**: React 👍 + reply acknowledging (**do NOT `@codex`** — avoid wasting bot tokens):
```bash
gh api "repos/${REPO}/pulls/comments/${COMMENT_ID}/reactions" \
  -X POST -f content='+1'
gh api "repos/${REPO}/pulls/${PR_NUM}/comments" \
  -X POST -f body="Fixed in ${COMMIT_SHA}. [brief explanation]." \
  -F in_reply_to=${COMMENT_ID}
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
heterogeneous model verdict. Your confidence as the code author is irrelevant —
the entire point of this process is that **no single model judges its own code**.

Replying directly with your own reasoning (e.g., "This is a design choice,
dismissing.") without completing Step 8 is a **FORBIDDEN action** (see above).

### Category C: Real Issue

The bot found a genuine bug or improvement.

**Action**: React 👍 → **queue for Step 9** (**do NOT `@codex`** — avoid wasting
bot tokens; the fix + `@codex review` in Step 9 will trigger re-evaluation):
```bash
gh api "repos/${REPO}/pulls/comments/${COMMENT_ID}/reactions" \
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
arbitration process with an **independent, heterogeneous model**.

### Model Heterogeneity Requirement

**CRITICAL**: The arbiter MUST be a **different model family** from both:
- The code author (you, the main agent)
- The cloud bot reviewer

This is the core value of arbitration — different training data, architectures,
and biases create "cognitive friction" that catches issues a single model would
miss. Using the same model family as the reviewer defeats the purpose.

| You (main agent) | Cloud Bot | Local Arbiter (MUST differ) |
|-------------------|-----------|---------------------------|
| Claude Opus 4.6 | GPT-5.3-Codex | `csa debate` → auto-selects **codex** |
| GPT-based agent | GPT-5.3-Codex | `csa debate` → auto-selects **claude-code** |
| Gemini-based agent | GPT-5.3-Codex | `csa debate --tool codex` or `--tool claude-code` |

**Preferred arbiter**: Use `csa debate` which auto-selects a heterogeneous
counterpart (if you are Claude, it picks codex; if you are codex, it picks
claude-code). No manual model-spec selection needed.

### Step 8.1: Get Independent Local Opinion

Use `csa debate` to spawn a heterogeneous arbiter. The command automatically
selects a different model family from you (the caller):

```bash
csa debate "A code reviewer flagged the following issue in [file:line]:

[paste bot's comment verbatim]

The relevant code is at [file path]. Please read the code and
evaluate whether this is a genuine issue or a false positive.
Do NOT assume the reviewer is right or wrong — form your own
independent assessment based on the actual code."
```

**NOTE**: `csa debate` reads `[debate]` config for tool selection. If `tool = "auto"`
(default), it auto-detects your tool from `CSA_PARENT_TOOL` and picks the
heterogeneous counterpart. Auto mode only maps `claude-code <-> codex`. If you
are a **gemini-cli** or **opencode** caller, auto will error — you must pass
`--tool codex` or `--tool claude-code` explicitly (or set `[debate].tool` in config).

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
(**do NOT `@codex`**):

```bash
gh api "repos/${REPO}/pulls/comments/${COMMENT_ID}/reactions" \
  -X POST -f content='-1'
gh api "repos/${REPO}/pulls/${PR_NUM}/comments" \
  -X POST \
  -f body="**Dismissed after local arbitration.**

**Participants:**
- Author: \`{your_tool}/{your_provider}/{your_model}/{your_thinking_budget}\`
- Arbiter: \`{arbiter_tool}/{arbiter_provider}/{arbiter_model}/{arbiter_thinking_budget}\`

**Reasoning:** [summary of arbiter reasoning]. [cite file:line evidence]." \
  -F in_reply_to=${COMMENT_ID}
```

**MANDATORY**: Model specs MUST use the `tool/provider/model/thinking_budget` format
(matching CSA tiers). This enables future reviewers to verify heterogeneous arbitration.

### Step 8.3b: Arbiter Confirms Real Issue or Uncertain → YOU Debate

**CRITICAL: YOU (the main agent / code author) MUST debate with the arbiter.**
Do NOT just accept the arbiter's verdict. You wrote the code — defend your
design decisions. The heterogeneous debate IS the value of this step.

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

**The debate is between two DIFFERENT model families (via `csa debate`):**
```
YOU (Claude Opus 4.6, code author)
    ↕  adversarial debate via csa debate  ↕
Arbiter (auto-selected heterogeneous model, independent evaluator)
```

This is where model heterogeneity creates real value — if both a Claude
and a GPT independently conclude the same thing after debate, the
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
(**do NOT `@codex`**):

```bash
gh api "repos/${REPO}/pulls/${PR_NUM}/comments" \
  -X POST \
  -f body="**Local arbitration result: [DISMISSED|CONFIRMED|ESCALATED].**

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
- Debate skill used: [yes/no — if complex, the \`debate\` skill provides structured multi-round debate]" \
  -F in_reply_to=${COMMENT_ID}
```

**MANDATORY**: Both model specs MUST use the `tool/provider/model/thinking_budget` format.
The audit section enables future reviewers (human or AI) to verify that heterogeneous
models were used and assess the quality of the arbitration.

### `@codex` Tagging Rules

- **NEVER `@codex` in false positive replies** — the bot ignores threaded
  debates anyway, and tagging wastes its tokens
- **NEVER `@codex` in real issue / already-fixed replies** — the fix commit
  + `@codex review` in Step 9 will trigger re-evaluation
- **The ONLY place to `@codex`** is `gh pr comment` to trigger a full
  re-review (Steps 4, 9, 11)

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
git push origin ${BRANCH}
gh pr comment ${PR_NUM} --repo ${REPO} --body "@codex review"
```

**Then poll (Step 5) and re-evaluate (Step 7).**

## Step 10: Re-Review Loop

After pushing fixes, poll for bot response (Step 5):
- **Has issues** → back to Step 7 (evaluate, debate/fix)
- **No issues** → proceed to Step 11 (clean resubmission)

## Step 11: Clean Resubmission

When the bot converges after fix iterations, the PR has accumulated
incremental fix commits. Create a clean PR for audit-friendly history.

```bash
# 1. Create new branch from main
git checkout -b ${BRANCH}-clean main

# 2. Squash merge all changes
git merge --squash ${BRANCH}

# 3. Unstage for selective re-commit
git reset HEAD

# 4. Recommit in logical groups by concern
#    Use `git add <specific files>` to stage by group
#    Each commit = one logical concern (not one file)

# 5. Push new branch
git push -u origin ${BRANCH}-clean

# 6. Create new PR linking to old one
gh pr create --title "[type](scope): [description]" \
  --body "$(cat <<'PREOF'
## Summary
[description]

## Background
Clean resubmission of #${OLD_PR_NUM}. The original PR went through
N rounds of iterative review with @codex. Fix commits have been
consolidated into logical groups here.

See #${OLD_PR_NUM} for the full review discussion.

## Test plan
- [ ] `cargo clippy -p [package] -- -D warnings`
- [ ] `cargo test -p [package]`
- [ ] @codex review
PREOF
)"

# 7. Close old PR
gh pr comment ${OLD_PR_NUM} --repo ${REPO} \
  --body "Superseded by #${NEW_PR_NUM}. Preserved for review discussion reference."
gh pr close ${OLD_PR_NUM} --repo ${REPO}

# 8. Trigger review on new PR
gh pr comment ${NEW_PR_NUM} --repo ${REPO} --body "@codex review"
```

### Commit Grouping Strategy

Group by **concern**, not by chronology or file:

| Concern | Typical Files | Commit Convention |
|---------|--------------|-------------------|
| Core abstractions | types, mod, registry | `feat(scope): [what the types enable]` |
| Implementation | executor, engine | `feat(scope): [what the engine does]` |
| Configuration | config, schema | `feat(scope): [what becomes configurable]` |
| Integration | router, dispatch | `feat(scope): [where it's wired in]` |
| Tests | test modules | `test(scope): [what is verified]` |
| Formatting | (if needed) | `style(scope): apply cargo fmt` |

**Number of commits is flexible** — use as many as needed for logical separation.

### Preservation Policy

| Artifact | Action | Reason |
|----------|--------|--------|
| Old branch | Keep | Audit trail |
| Old commits | Keep | Shows iterative development |
| Old PR | Close with comment | Links to new PR, preserves discussion |
| New branch | Active | Clean history for merge |
| New PR | Active | Fresh review with coherent diff |

## Step 12: Review New PR

Poll for bot response on the new clean PR (Step 5):
- **No issues** → Step 13 (merge)
- **Has issues** → back to Step 7 (evaluate each comment, debate/fix)
  - If fixes are needed again, repeat the full cycle including
    another clean resubmission (Step 11) with incrementing branch
    names: `${BRANCH}-clean-2`, `${BRANCH}-clean-3`, etc.

## Step 13: Merge

```bash
# Merge remotely
gh pr merge ${NEW_PR_NUM} --repo ${REPO} --squash --delete-branch

# Update local main
git checkout main && git pull origin main
```

## Loop Control

| Control | Value | Rationale |
|---------|-------|-----------|
| Max iterations per PR | 5 | Prevents infinite loop |
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
| Poll timeout (10 min) | **Notify user** - don't act on stale data |

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

**IMPORTANT**: PR review comment APIs have subtle path differences:

| Operation | Endpoint | Notes |
|-----------|----------|-------|
| List comments | `GET repos/{REPO}/pulls/{PR}/comments` | Includes `in_reply_to_id` |
| Get one comment | `GET repos/{REPO}/pulls/comments/{ID}` | No PR number! |
| Reply to comment | `POST repos/{REPO}/pulls/{PR}/comments` with `-F in_reply_to={ID}` | Uses PR number |
| Add reaction | `POST repos/{REPO}/pulls/comments/{ID}/reactions` with `-f content='+1'` or `'-1'` | No PR number! |

Bot user login: `chatgpt-codex-connector[bot]`

## Integration

| Skill | When to Use |
|-------|-------------|
| `csa-review` | Step 2: local review before PR (sessions in `~/.local/state/csa/`) |
| `debate` | Step 8: adversarial arbitration for suspected false positives |
| `commit` | After fixing issues |
| `csa run --tier tier-4-critical` | If bot flags security issue (deep analysis) |
