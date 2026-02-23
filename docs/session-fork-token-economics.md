# Session Fork: Token Economics Analysis

## Overview

This document analyzes the token economics of session forking vs cold-start
sessions, covering both the existing soft-fork mechanism and the experimental
PTY-based native fork (`codex fork`).

## Cold Start Token Cost

A new CSA session incurs significant context loading overhead:

| Component | Estimated Tokens | Notes |
|-----------|----------------:|-------|
| CLAUDE.md | ~8,000 | Project rules and architecture |
| AGENTS.md rules | ~6,000 | Coding rules (injected via setting_sources) |
| System prompt | ~2,000 | Session metadata, tool instructions |
| Skill files | ~3,000-15,000 | Depends on loaded skills (0-5 skills) |
| MCP server definitions | ~1,000-3,000 | MCP hub routing guide |
| Project context files | ~5,000-10,000 | Config, patterns, READMEs |
| **Total cold start** | **~25,000-44,000** | Before any user prompt |

For Codex specifically (via ACP transport), the `setting_sources` mechanism
reduces this by selectively loading only relevant skills and rules. Even so,
a typical ACP cold start costs **~25,000-35,000 tokens** of context setup.

## Soft Fork Cost (Current Implementation)

The existing soft fork (`csa-session/soft_fork.rs`) injects a context summary
from the parent session:

| Component | Estimated Tokens | Notes |
|-----------|----------------:|-------|
| Context summary injection | ~2,000 | Capped at `SUMMARY_TOKEN_BUDGET` |
| New session cold start | ~25,000-35,000 | Same as above |
| **Total soft fork** | **~27,000-37,000** | Only ~2K more than cold start |

**Problem**: Soft fork does NOT reuse the parent's conversation history. It
creates a brand new session with a 2K summary prepended. The tool still pays
full cold-start context loading costs.

## Native Fork Cost (PTY Fork — `codex fork`)

Codex's native `fork` command copies the conversation history server-side:

| Component | Estimated Tokens | Notes |
|-----------|----------------:|-------|
| Fork API call | ~0 (server-side) | Conversation history copied by provider |
| New prompt | ~100-500 | Task-specific prompt only |
| PTY overhead | ~50 | `script(1)` wrapper output |
| **Total native fork** | **~150-550** | Dramatic reduction |

The key insight: `codex fork` operates at the **provider level** (OpenAI API),
copying the thread/conversation history without re-transmitting it. The forked
session inherits all prior context at zero additional input token cost.

## Savings Comparison

| Scenario | Input Tokens | Savings vs Cold Start |
|----------|------------:|---------------------:|
| Cold start | ~30,000 | baseline |
| Soft fork | ~32,000 | -2,000 (worse!) |
| Native fork | ~350 | **~29,650 (~99%)** |

### Per-Session Dollar Cost (at $15/M input tokens)

| Scenario | Cost per Session | Monthly (100 sessions) |
|----------|----------------:|-----------------------:|
| Cold start | $0.45 | $45.00 |
| Soft fork | $0.48 | $48.00 |
| Native fork | $0.005 | $0.53 |

## When Native Fork is Beneficial

Native fork delivers maximum value when:

1. **Iterative refinement**: Multiple prompts building on the same context
   (e.g., review → fix → re-review cycle)
2. **Branching experiments**: Try different approaches from the same point
3. **Multi-step workflows**: Pipeline steps that share conversation history
   (commit → review → fix pre-commit errors)

## When Native Fork is NOT Beneficial

1. **Cross-tool fork**: Forking from claude-code to codex (different providers)
   — soft fork is the only option
2. **Fresh context needed**: When the parent session's context is stale or
   irrelevant to the new task
3. **Different project**: Fork is session-local, not cross-project

## Implementation Status

| Feature | Status | Location |
|---------|--------|----------|
| Soft fork | Implemented | `csa-session/soft_fork.rs` |
| Genealogy tracking | Implemented | `csa-session/state.rs` (Genealogy) |
| PTY fork prototype | Experimental | `csa-process/pty_fork.rs` (feature-gated) |
| ACP fork integration | Not started | Would go in `csa-executor/transport.rs` |
| Token measurement tooling | Not started | Would use provider usage API |

## Limitations of PTY Fork Approach

1. **Interactive TUI**: `codex fork` is designed as an interactive command;
   PTY wrapping via `script(1)` may produce ANSI escape artifacts in output
2. **Output parsing**: TUI output contains control characters that need
   stripping before use as structured output
3. **No ACP equivalent**: The Codex ACP protocol does not currently expose
   a fork operation — only session resume
4. **Platform dependency**: `script(1)` behavior differs between GNU (Linux)
   and BSD (macOS)

## Recommendations

1. **Short-term**: Use soft fork for cross-tool scenarios; native fork for
   Codex-to-Codex session continuation
2. **Medium-term**: Request ACP protocol extension for fork support, which
   would eliminate the PTY dependency
3. **Long-term**: Each tool vendor should expose fork/branch in their API,
   enabling provider-level session branching without PTY hacks

## Measurement Approach

To validate these estimates empirically:

```bash
# 1. Run N cold-start sessions, capture token usage
for i in $(seq 1 10); do
  csa run --tool codex "echo hello" 2>&1 | grep -i token
done

# 2. Run N forked sessions from the same parent
PARENT_SESSION=$(csa session list --tool codex --json | jq -r '.[0].id')
for i in $(seq 1 10); do
  codex fork $PARENT_SESSION "echo hello" --dangerously-bypass-approvals-and-sandbox
done

# 3. Compare input token counts from provider billing
```

Token usage data should be captured from the provider's usage API (OpenAI
`usage` field in API responses) rather than estimated, once the PTY fork
integration is connected to the main execution pipeline.
