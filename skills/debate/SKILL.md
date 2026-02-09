---
name: debate
description: Adversarial multi-tool AI debate for strategy formulation with tier-based model escalation
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "debate"
  - "adversarial debate"
  - "multi-tool debate"
  - "AI debate"
  - "strategy debate"
---

# Debate: Adversarial Multi-Tool Strategy Formulation

## Purpose

Orchestrate an adversarial debate between heterogeneous AI tools to produce well-reasoned strategies. The `csa debate` command enforces model heterogeneity: when the caller is claude-code, debate runs codex (and vice versa), ensuring different model families stress-test each other's reasoning.

Two usage modes:
1. **Default Workflow** (single-call + session continuation): The caller proposes, `csa debate` critiques via heterogeneous model, caller rebuts, repeat until consensus.
2. **Deep Orchestrated Debate** (multi-round + tier escalation): The skill orchestrates multiple rounds with tier-based model selection for complex strategy questions.

## CLI Reference

```bash
# New debate — heterogeneous model analyzes the question
csa debate "Should we use gRPC or REST for our new microservice API?"

# Continue debate — pass counterargument in existing session
csa debate --session <SESSION_ID> "I disagree because gRPC adds complexity for our small team"

# Override tool selection (bypass auto-heterogeneous)
csa debate --tool codex "How should we handle distributed transactions?"

# Override model within selected tool
csa debate --model o3 "What caching strategy should we use?"

# Pipe long prompts via stdin
echo "Given this architecture: ... Should we refactor?" | csa debate
```

## Default Workflow Integration

This is the pattern for embedding `csa debate` as the standard decision-making step in AI coding tool system prompts (CLAUDE.md, AGENTS.md, etc.):

### Pattern: Propose → Counter → Arbitrate

```
1. You (the caller) have a question or proposal
2. Run: csa debate "your question/proposal"
3. Read the heterogeneous model's response
4. If you agree → adopt the strategy, note the consensus
5. If you disagree → counter-argue:
   csa debate --session <ID> "I disagree because X. My alternative is Y."
6. Read the response, repeat steps 4-5 until consensus
7. Document the final decision with the debate session ID for audit trail
```

### Example CLAUDE.md / AGENTS.md Snippet

```markdown
## Mandatory Debate Protocol

Before any non-trivial design decision (architecture, API design, data model, error handling strategy):

1. Run `csa debate "describe the decision and your initial leaning"`
2. Read the heterogeneous model's analysis
3. If the model raises valid concerns you hadn't considered, revise your approach
4. If you disagree, counter-argue: `csa debate --session <ID> "your counterpoint"`
5. Continue until consensus or 3 rounds (whichever comes first)
6. Document: "Decided X after debate (session: <ID>)"

Skip debate for: trivial changes, typo fixes, single-line edits, test additions.
```

## Required Inputs

- `question`: The strategy question or problem to debate (positional argument or stdin)
- `tool` (optional): Debate tool selection override (default: `auto`)
- `session` (optional): Resume existing debate session (ULID or prefix)
- `model` (optional): Override model within selected tool

## Prerequisites (MANDATORY — verify before ANY debate action)

1. `csa` binary MUST be in PATH. Verify: `which csa`
2. For orchestrated debates: project MUST have tiers configured. Verify: `csa --format json tiers list`
3. For default workflow: only `csa debate` needs to work (tiers not required)

If ANY prerequisite fails:
- **STOP IMMEDIATELY**
- Report the specific failure to the user
- Suggest remediation (e.g., "Run `csa init` to configure tiers")
- **DO NOT attempt to execute tools directly**

## FORBIDDEN Actions

- **NEVER** execute `gemini`, `opencode`, `codex`, or `claude` commands directly
- **NEVER** bypass CSA by constructing tool commands manually
- **NEVER** fall back to direct tool execution if CSA fails
- **NEVER** hardcode model names — ALL models come from `csa tiers list`
- **NEVER** guess model specs — if `csa tiers list` returns no usable models, ERROR

If CSA invocation fails (non-zero exit, timeout, parsing error):
- Report the exact error to the user
- Suggest `csa doctor` or manual investigation
- **STOP the debate** — do NOT retry with direct tool calls

## Execution Protocol (Orchestrated Deep Debate)

Use this protocol for complex strategy questions that benefit from multi-round, multi-model debate with tier escalation.

### Step 0: Discover Available Models (with validation)

```bash
csa --format json tiers list
```

**Validation (MANDATORY)**:
1. Command exits 0 → parse JSON. Continue.
2. Command exits non-zero → **STOP. Report error. Do NOT proceed.**
3. JSON `tiers` array is empty → **STOP. "No tiers configured. Run `csa init`."**
4. No tier has >= 2 models → **STOP. "At least one tier needs >= 2 models for debate."**
5. JSON parsing fails → **STOP. Report parsing error.**

Parse the JSON output to get:
- `tiers[].name`: tier name (sorted by name, lower = simpler)
- `tiers[].models[]`: model specs in `tool/provider/model/thinking_budget` format
- `tiers[].description`: human-readable tier description
- `tier_mapping.default`: default tier for general tasks

### Step 1: Resolve Debate Tool (Heterogeneous Auto)

`csa debate` handles tool resolution automatically:
- If parent is `claude-code` → debate uses `codex`
- If parent is `codex` → debate uses `claude-code`
- Otherwise → error with guidance to configure manually

Configuration in `~/.config/cli-sub-agent/config.toml` or `.csa/config.toml`:

```toml
[debate]
tool = "auto"  # or "codex", "claude-code", "opencode", "gemini-cli"
```

### Step 2: Select Starting Tier (and Filter Models)

- If `tier` is specified, use that tier.
- Otherwise, use the tier mapped to `default` in `tier_mapping`.
- Record the ordered list of all tier names for potential escalation.

For the selected tier, build the `models[]` list:

- Start with `tiers[chosen_tier].models`
- If a debate tool is resolved (Step 1), **filter** to only model specs whose tool prefix matches the debate tool
- Validate the filtered `models[]` has >= 2 entries (proposal + critique)
- If it has < 2 entries, try the next higher tier; if no tier works, **STOP** and instruct the user to add >=2 models for the debate tool to a tier

### Step 3: Debate Loop

Within the selected tier, models alternate via round-robin:
- `models[0]` = Proposer (Round 1), Responder (Round 2), ...
- `models[1]` = Critic (Round 1), Proposer (Round 2), ...
- `models[2]` = next in rotation if available

**Round N (Proposal)**:
```bash
csa run --model-spec "{models[proposer_index]}" --ephemeral \
  "Question: {question}

You are the PROPOSER in an adversarial debate. {context_from_previous_rounds}

Provide a concrete, actionable strategy. Structure your response as:
1. Core Strategy (2-3 sentences)
2. Key Arguments (numbered, with evidence/reasoning)
3. Implementation Steps (concrete actions)
4. Anticipated Weaknesses (acknowledge limitations honestly)"
```

**Round N (Critique)**:
```bash
csa run --model-spec "{models[critic_index]}" --ephemeral \
  "Question: {question}

You are the CRITIC in an adversarial debate.

PROPOSAL:
{proposal_text}

Rigorously critique this proposal:
1. Logical Flaws (identify reasoning errors)
2. Missing Considerations (what was overlooked)
3. Better Alternatives (if any exist, be specific)
4. Strongest Counter-Arguments (the best case AGAINST this proposal)

Be intellectually honest: acknowledge strengths before attacking weaknesses."
```

**Round N (Response)**:
```bash
csa run --model-spec "{models[responder_index]}" --ephemeral \
  "Question: {question}

You are the PROPOSER responding to criticism.

ORIGINAL PROPOSAL:
{proposal_text}

CRITIQUE:
{critique_text}

Respond to each criticism:
1. Concede valid points and revise your strategy
2. Refute invalid criticisms with evidence
3. Present your REVISED STRATEGY incorporating lessons learned

If the critique fundamentally undermines your approach, propose a new strategy."
```

### Step 4: Convergence Evaluation

After each critique-response pair, YOU (the orchestrator) evaluate:

**Convergence criteria** (debate should end):
- Both sides agree on core strategy with minor differences
- New rounds repeat previous arguments without novel insights
- Revised strategy addresses all major criticisms

**Escalation criteria** (move to next tier):
- Proposer cannot effectively counter the critique
- Arguments are circular without resolution
- The question's complexity exceeds the current tier's reasoning capability
- Both sides acknowledge the need for deeper analysis

### Step 5: Escalation (if needed)

When escalation is triggered:
1. Find the next higher tier (by sorted tier name order).
2. If no higher tier exists, end the debate with current best result.
3. Summarize the debate so far as context for the new tier.
4. Restart the debate loop with the higher tier's models.

```bash
# Example: escalating from tier-1-quick to tier-2-standard
csa run --model-spec "{higher_tier_models[0]}" --ephemeral \
  "Question: {question}

PREVIOUS DEBATE SUMMARY (lower-tier models could not resolve):
{debate_summary}

You have been escalated to provide deeper analysis. Build on the previous debate:
1. Identify what the previous debaters missed
2. Propose a superior strategy with stronger reasoning
3. Address all unresolved criticisms"
```

### Step 6: Final Synthesis

After the debate concludes (convergence or max rounds/escalations reached), YOU synthesize:

```markdown
# Debate Result: {question}

## Final Strategy
{synthesized_strategy}

## Key Insights from Debate
- {insight_1}
- {insight_2}

## Resolved Tensions
- {tension_1}: resolved by {resolution}

## Remaining Uncertainties
- {uncertainty_1}

## Debate Trajectory
- Tier: {starting_tier} -> {final_tier} ({n} escalations)
- Rounds: {total_rounds}
- Models used: {model_list}
```

## Audit Trail Requirements (MANDATORY)

Every debate result MUST include:
1. **Full model specs** for ALL participants in `tool/provider/model/thinking_budget` format
2. **Round-by-round transcript** (at minimum: position summaries per round)
3. **Final verdict** with which side prevailed and rationale
4. **Escalation history** if tier escalation occurred

**Why**: Debate results are used as evidence in code review arbitration (pr-codex-bot),
security audits, and design decisions. Without model specs, future reviewers cannot
assess the quality or heterogeneity of the arbitration.

## PR Integration (when used for code review arbitration)

When the debate skill is invoked from `pr-codex-bot` Step 8 (false positive arbitration)
or any code review context where results will be posted to a PR:

### MANDATORY: Post Results to PR

The debate result MUST be posted as a PR comment for audit trail. The caller
(typically pr-codex-bot) is responsible for posting, but the debate output MUST
include all information needed:

1. **Participants section** with full model specs (both sides)
2. **Bot's original concern** (what was being debated)
3. **Round-by-round summary** (not full transcript — keep PR comments readable)
4. **Conclusion** with verdict (DISMISSED / CONFIRMED / ESCALATED)
5. **CSA session ID** (if applicable, for full transcript retrieval)

### Template for PR Comment

```markdown
**Local arbitration result: [DISMISSED|CONFIRMED|ESCALATED].**

## Participants
- **Author**: `{tool}/{provider}/{model}/{thinking_budget}`
- **Arbiter**: `{tool}/{provider}/{model}/{thinking_budget}`

## Debate Summary
### Round 1
- **Proposer** (`{model}`): [position summary]
- **Critic** (`{model}`): [counter-argument summary]
### Round N...

## Conclusion
[verdict, rationale, which side prevailed]

## Audit
- Rounds: {N}, Escalations: {N}
- CSA session: `{session_id}`
```

**FORBIDDEN**: Posting a debate result without model specs. If model specs cannot be
determined (e.g., CSA returned no metadata), report this explicitly in the comment
rather than omitting the section.

## Constraints

- **No hardcoded models**: All models come from `csa tiers list`.
- **Ephemeral sessions**: Deep debate rounds use `--ephemeral` (no persistent sessions needed).
- **Persistent sessions**: Default workflow uses sessions for multi-turn continuation.
- **Round limit**: max 3 rounds per tier (configurable).
- **Escalation limit**: max 2 escalations (configurable).
- **Total budget**: max 4 rounds * 3 tiers = 12 CSA invocations worst case.
- **Orchestrator role**: The calling agent evaluates convergence/escalation; CSA tools only debate.

## Example Usage

### Default Workflow (Propose → Counter → Arbitrate)

```
User: "Should we refactor the auth module to use JWT instead of sessions?"

Agent runs: csa debate "We're considering replacing server-side sessions with JWT for our auth module. Current system uses Redis-backed sessions. Team size: 3 developers. Scale: ~10k DAU."

# Heterogeneous model responds with analysis, arguments for/against

Agent disagrees with a point:
csa debate --session 01JK... "Your concern about token revocation is valid, but we plan to use short-lived tokens (15min) with refresh rotation, which mitigates this."

# Model responds to counterpoint

Agent reaches consensus → documents decision
```

### Deep Orchestrated Debate

```
User: /debate "How should we handle distributed transactions across 5 microservices?"
```

Debate flow:
1. Tier-2 model A proposes saga pattern
2. Tier-2 model B critiques (compensation complexity, partial failures)
3. Tier-2 model A responds but cannot address all concerns
4. Orchestrator: arguments weak -> escalate to Tier-3
5. Tier-3 model A proposes event sourcing + saga hybrid
6. Tier-3 model B refines with specific failure scenarios
7. Orchestrator: converging -> synthesize result

## Done Criteria

1. At least 1 debate exchange (proposal + critique/counterpoint) completed.
2. Final decision or synthesis document produced with clear strategy.
3. All CSA invocations used `csa debate` or `csa run` (no direct tool calls).
4. No hardcoded model names in any invocation.
5. Zero direct tool invocations (all through CSA).
6. If any CSA command failed, debate was stopped and error reported.
7. **All participant model specs listed in `tool/provider/model/thinking_budget` format** (audit trail).
8. **If used for PR arbitration**: debate result posted to PR comment with full model specs and round summaries (see PR Integration section above).
