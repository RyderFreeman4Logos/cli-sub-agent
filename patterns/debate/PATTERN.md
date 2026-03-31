---
name = "debate"
description = "Adversarial multi-tool AI debate for strategy formulation with tier-based model escalation"
allowed-tools = "Bash, Read, Grep, Glob"
tier = "tier-2-standard"
version = "0.1.0"
---

# Debate: Adversarial Multi-Tool Strategy Formulation

Orchestrate adversarial debate using CSA's independent model routing.
Two modes: default workflow (single-call + session continuation) and
deep orchestrated debate (multi-round + tier escalation).

## Step 1: Role Detection

Determine if this agent is the orchestrator or a debate participant.
If initial prompt contains "Use the debate skill" → participant mode.
If invoked by user via /debate → orchestrator mode.
Participants MUST NOT run any csa commands (infinite recursion).

## Step 2: Discover Available Models

Tool: bash
OnFail: abort

List configured tiers and parse JSON to get available models.
At least one tier needs >= 2 models for debate. Record ordered
tier list for escalation.

```bash
csa --format json tiers list
```

## Step 3: Resolve Debate Tool

CSA auto-selects heterogeneous tool: claude-code caller → codex reviewer,
codex caller → claude-code reviewer. Override with explicit --tool if needed
(requires --force-ignore-tier-setting when tiers are configured).

## Step 4: Select Starting Tier

Use tier mapped to default in tier_mapping, or user-specified tier.
Filter models to those matching the debate tool.
Validate >= 2 models available. If < 2, try next higher tier.

### Fork From Research Session (Optional)

When a research session exists with relevant context (e.g., codebase exploration,
documentation gathering, prior analysis), fork debaters from it to avoid
redundant exploration. Note: `csa debate` does not yet support `--fork-from`
directly. Use `csa run --fork-from` to prepare a forked session, then pass
the research context into the debate prompt:

```bash
# Gather context via forked session, then feed into debate
SID=$(csa run --fork-from <research-session-id> "Summarize findings for debate context")
csa session wait --session "$SID"
csa debate "question (with research context above)"
```

**Benefits**: Debaters inherit the research session's context (files read,
patterns identified, domain knowledge gathered). This eliminates the warm-up
phase where each debater independently re-discovers the same information,
resulting in faster convergence and deeper arguments from round 1.

> **Planned**: Native `csa debate --fork-from` support is tracked for a future release.

## Step 5: Round 1 — Proposal

Tool: csa
Tier: ${CURRENT_TIER}

Proposer presents concrete, actionable strategy with:
1. Core Strategy (2-3 sentences)
2. Key Arguments (numbered, with evidence)
3. Implementation Steps (concrete actions)
4. Anticipated Weaknesses (honest limitations)

```bash
SID=$(csa run --model-spec "${PROPOSER_MODEL}" --ephemeral "${PROPOSAL_PROMPT}")
csa session wait --session "$SID"
```

## Step 6: Round 1 — Critique

Tool: csa
Tier: ${CURRENT_TIER}

Critic rigorously evaluates the proposal:
1. Logical Flaws
2. Missing Considerations
3. Better Alternatives
4. Strongest Counter-Arguments

```bash
SID=$(csa run --model-spec "${CRITIC_MODEL}" --ephemeral "${CRITIQUE_PROMPT}")
csa session wait --session "$SID"
```

## Step 7: Round 1 — Response

Tool: csa
Tier: ${CURRENT_TIER}

Proposer responds to each criticism:
1. Concede valid points and revise strategy
2. Refute invalid criticisms with evidence
3. Present revised strategy

```bash
SID=$(csa run --model-spec "${PROPOSER_MODEL}" --ephemeral "${RESPONSE_PROMPT}")
csa session wait --session "$SID"
```

## Step 8: Convergence Evaluation

Orchestrator evaluates after each critique-response pair:
- Both sides agree on core strategy → end debate
- Arguments repeat without novel insights → end debate
- Proposer cannot counter critique → escalate tier
- Arguments circular → escalate tier

## IF ${NEEDS_ESCALATION}

## Step 9: Tier Escalation

Find next higher tier. Summarize debate so far as context.
Restart debate loop with higher tier models.
Max 2 escalations.

```bash
SID=$(csa run --model-spec "${HIGHER_TIER_MODEL}" --ephemeral "${ESCALATION_PROMPT}")
csa session wait --session "$SID"
```

## ENDIF

## Step 10: Final Synthesis

Produce debate result document with:
- Final Strategy
- Key Insights from Debate
- Resolved Tensions
- Remaining Uncertainties
- Debate Trajectory (tiers, rounds, models used)
- Full model specs for ALL participants (audit trail)
