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

## Step 2: Verify Prerequisites

Tool: bash
OnFail: abort

Verify csa binary is available and tiers are configured.

```bash
which csa && csa --format json tiers list
```

## Step 3: Discover Available Models

Tool: bash
OnFail: abort

Parse tiers JSON to get available models. At least one tier needs
>= 2 models for debate. Record ordered tier list for escalation.

```bash
csa --format json tiers list
```

## Step 4: Resolve Debate Tool

CSA auto-selects heterogeneous tool: claude-code caller → codex reviewer,
codex caller → claude-code reviewer. Override with explicit --tool if needed.

## Step 5: Select Starting Tier

Use tier mapped to default in tier_mapping, or user-specified tier.
Filter models to those matching the debate tool.
Validate >= 2 models available. If < 2, try next higher tier.

## Step 6: Round 1 — Proposal

Tool: csa
Tier: ${CURRENT_TIER}

Proposer presents concrete, actionable strategy with:
1. Core Strategy (2-3 sentences)
2. Key Arguments (numbered, with evidence)
3. Implementation Steps (concrete actions)
4. Anticipated Weaknesses (honest limitations)

```bash
csa run --model-spec "${PROPOSER_MODEL}" --ephemeral "${PROPOSAL_PROMPT}"
```

## Step 7: Round 1 — Critique

Tool: csa
Tier: ${CURRENT_TIER}

Critic rigorously evaluates the proposal:
1. Logical Flaws
2. Missing Considerations
3. Better Alternatives
4. Strongest Counter-Arguments

```bash
csa run --model-spec "${CRITIC_MODEL}" --ephemeral "${CRITIQUE_PROMPT}"
```

## Step 8: Round 1 — Response

Tool: csa
Tier: ${CURRENT_TIER}

Proposer responds to each criticism:
1. Concede valid points and revise strategy
2. Refute invalid criticisms with evidence
3. Present revised strategy

```bash
csa run --model-spec "${PROPOSER_MODEL}" --ephemeral "${RESPONSE_PROMPT}"
```

## Step 9: Convergence Evaluation

Orchestrator evaluates after each critique-response pair:
- Both sides agree on core strategy → end debate
- Arguments repeat without novel insights → end debate
- Proposer cannot counter critique → escalate tier
- Arguments circular → escalate tier

## IF ${NEEDS_ESCALATION}

## Step 10: Tier Escalation

Find next higher tier. Summarize debate so far as context.
Restart debate loop with higher tier models.
Max 2 escalations.

```bash
csa run --model-spec "${HIGHER_TIER_MODEL}" --ephemeral "${ESCALATION_PROMPT}"
```

## ENDIF

## Step 11: Final Synthesis

Produce debate result document with:
- Final Strategy
- Key Insights from Debate
- Resolved Tensions
- Remaining Uncertainties
- Debate Trajectory (tiers, rounds, models used)
- Full model specs for ALL participants (audit trail)
