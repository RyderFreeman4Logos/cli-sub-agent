---
name: codebase-audit
description: "Use when: deep per-crate code audit, Chinese documentation generation"
allowed-tools: Bash, Read, Grep, Glob, Write
triggers:
  - "codebase-audit"
  - "/codebase-audit"
  - "audit codebase"
  - "analyze codebase"
  - "deep code analysis"
---

# Codebase Audit: Deep Code Analysis & Documentation

## Role Detection (READ THIS FIRST -- MANDATORY)

Role MUST be determined by explicit mode marker, not fragile natural-language substring matching.
Treat the run as executor ONLY when initial prompt contains:
`<skill-mode>executor</skill-mode>`.

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `../../PATTERN.md` relative to this `SKILL.md`. Execute `Tool: bash`
   steps directly. Steps marked `Tool: csa` are dispatched by the orchestrator — skip them
   and report that they require orchestrator dispatch.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Perform a systematic, bottom-up deep analysis of an entire codebase (or scoped subset),
generating three types of Chinese documentation per crate:

1. **README.md** — Module overview with architecture, public API, key types
2. **review_report.md** — Code quality and security review report
3. **blog.md** — Technical deep-dive blog for intermediate developers

Plus a machine-readable **facts.toml** sidecar per crate containing exported APIs,
key types, constraints, risks, and dependency summaries.

Crates are processed in topological order (leaf dependencies first) so downstream
analysis inherits upstream facts. Large crates are automatically sharded to stay
within the 163,840 token context budget. A dual CSA Writer+Reviewer pipeline ensures
factual accuracy.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- Must be in a Cargo workspace with multiple crates
- `scripts/crate-topo.sh` must exist (generates topological crate order)

### Quick Start

```bash
csa run --sa-mode true --skill codebase-audit "Analyze all crates in the workspace"
```

### Known Limitation: loop_var

`csa plan run` currently skips `loop_var` steps (unsupported in the plan executor).
The orchestrator (this skill's executor) MUST implement the FOR loop manually:

```bash
# Step 1: Get crate list
CRATE_LIST=$(bash scripts/crate-topo.sh)
# Step 2-3: Prepare and estimate
# Step 4-7: FOR each crate — orchestrator iterates manually
IFS=',' read -ra CRATES <<< "$CRATE_LIST"
for crate in "${CRATES[@]}"; do
  # Run Writer CSA
  csa run --sa-mode true --tier tier-4-critical --timeout 3600 \
    "Analyze crate ${crate} at crates/${crate}/src/ ..."
  # Run Reviewer CSA
  csa run --sa-mode true --tier tier-4-critical --timeout 2400 \
    "Review drafts/crates/${crate}/ against crates/${crate}/src/ ..."
done
```

The workflow.toml remains the authoritative step definition; the orchestrator
translates loop steps into sequential CSA calls.

### SA Mode Propagation (MANDATORY)

When operating under SA mode, **ALL `csa` invocations MUST include `--sa-mode true`**.

### Step-by-Step

1. **Extract topology**: `scripts/crate-topo.sh` produces comma-separated crate list in dependency order.
2. **Prepare output**: Create `drafts/crates/{crate}/chapters/` directories and `progress.toml`.
3. **Estimate budgets**: `tokuin estimate` per crate, mark large crates for sharding (>80K tokens).
4. **Per-crate audit** (FOR loop, bottom-up):
   - Skip completed crates (check progress.toml)
   - Writer CSA (opus xhigh): generates README.md, review_report.md, blog.md, facts.toml
   - Reviewer CSA (opus xhigh): fact-checks all outputs against source code, fixes errors
   - Update progress.toml
5. **Global summary**: Read all facts.toml → generate SUMMARY.md with architecture diagram.
6. **Verify**: Check all crates completed, output statistics.

### Resumability

Fully resumable. If interrupted:
- `grep 'status = "pending"' drafts/crates/progress.toml` shows remaining work
- Re-run the same command to continue from where it left off

## Example Usage

| Command | Effect |
|---------|--------|
| `/codebase-audit` | Analyze all crates in topological order |

## Integration

- **Depends on**: `scripts/crate-topo.sh`, `cargo metadata`, `jq`
- **Related to**: `file-audit` (AGENTS.md compliance), `codebase-blog` (blog generation from audit)
- **Output**: `drafts/crates/` directory with per-crate documentation + global SUMMARY.md

## Done Criteria

1. Crate topology extracted via `cargo metadata` (not file-level topo).
2. All crates processed in topological order (leaves first).
3. Per-crate: facts.toml + README.md + review_report.md + blog.md generated.
4. Large crates (>80K tokens) sharded by module.
5. Reviewer verified factual accuracy of all outputs.
6. Reports in `drafts/crates/` mirroring crate structure.
7. `progress.toml` shows all crates completed.
8. `SUMMARY.md` generated with cross-crate analysis and Mermaid diagram.
