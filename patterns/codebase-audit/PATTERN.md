---
name = "codebase-audit"
description = "Bottom-up per-crate deep code analysis with three-type Chinese documentation generation"
allowed-tools = "Bash, Read, Grep, Glob, Write"
tier = "tier-4-critical"
version = "0.2.0"
---

# Deep Codebase Audit & Documentation

Bottom-up per-crate code analysis generating three types of Chinese documentation
(README, review report, tech blog) plus a machine-readable facts.toml sidecar.
Crates are processed in topological order so downstream analysis inherits
upstream facts. Large crates are split into shards to stay within context budget.

## Step 1: Extract Crate Topology

Tool: bash
OnFail: abort

Run `cargo metadata` and extract workspace-internal crate dependencies in
topological order using Kahn's algorithm.

```bash
CRATE_LIST=$(bash scripts/crate-topo.sh)
if [ $? -ne 0 ]; then
  echo "ERROR: topological sort failed (circular dependency?)"
  exit 1
fi
echo "CSA_VAR:CRATE_LIST=${CRATE_LIST}"
echo "Crate processing order: ${CRATE_LIST}"
```

## Step 2: Prepare Output Structure

Tool: bash
OnFail: abort

Create the `drafts/crates/` mirror directory and initialize `progress.toml`.

```bash
IFS=',' read -ra CRATES <<< "${CRATE_LIST}"
for crate in "${CRATES[@]}"; do
  mkdir -p "drafts/crates/${crate}/chapters"
done

# Initialize progress.toml if not exists
if [ ! -f "drafts/crates/progress.toml" ]; then
  echo "# Codebase audit progress" > drafts/crates/progress.toml
  echo "# Auto-generated — do not edit manually" >> drafts/crates/progress.toml
  for crate in "${CRATES[@]}"; do
    echo "" >> drafts/crates/progress.toml
    echo "[${crate}]" >> drafts/crates/progress.toml
    echo 'status = "pending"' >> drafts/crates/progress.toml
    echo 'timestamp = ""' >> drafts/crates/progress.toml
    echo 'session_id = ""' >> drafts/crates/progress.toml
    echo 'shard_count = 1' >> drafts/crates/progress.toml
  done
fi
echo "Output structure ready: drafts/crates/"
```

## Step 3: Estimate Token Budgets

Tool: bash
OnFail: abort

For each crate, estimate total source token count and determine if sharding
is needed (threshold: 80K tokens per shard).

```bash
IFS=',' read -ra CRATES <<< "${CRATE_LIST}"
SHARD_MAP=""
for crate in "${CRATES[@]}"; do
  # Find crate source directory
  CRATE_DIR=$(cargo metadata --no-deps --format-version 1 2>/dev/null | \
    jq -r --arg name "$crate" '.packages[] | select(.name == $name) | .manifest_path' | \
    sed 's|/Cargo.toml$||')
  if [ -z "$CRATE_DIR" ] || [ ! -d "$CRATE_DIR/src" ]; then
    echo "WARN: Cannot find src/ for ${crate}, skipping shard estimation"
    SHARD_MAP="${SHARD_MAP}${crate}:1,"
    continue
  fi

  # Estimate total tokens (wc -l as proxy: ~1.5 lines per token)
  TOTAL_LINES=$(find "${CRATE_DIR}/src" -name "*.rs" -exec wc -l {} + 2>/dev/null | tail -1 | awk '{print $1}')
  TOTAL_LINES=${TOTAL_LINES:-0}
  EST_TOKENS=$(( TOTAL_LINES * 2 / 3 ))

  if [ "$EST_TOKENS" -gt 80000 ]; then
    SHARDS=$(( (EST_TOKENS + 79999) / 80000 ))
    echo "SHARD: ${crate} needs ${SHARDS} shards (est. ${EST_TOKENS} tokens, ${TOTAL_LINES} lines)"
  else
    SHARDS=1
    echo "OK: ${crate} fits single shard (est. ${EST_TOKENS} tokens, ${TOTAL_LINES} lines)"
  fi
  SHARD_MAP="${SHARD_MAP}${crate}:${SHARDS},"
done
# Remove trailing comma
SHARD_MAP="${SHARD_MAP%,}"
echo "CSA_VAR:SHARD_MAP=${SHARD_MAP}"
```

## FOR crate IN ${CRATE_LIST}

## Step 4: Check Progress and Prepare Shard

Tool: bash
MaxIterations: 20

Check if this crate is already completed in progress.toml. If so, skip.
If the crate needs sharding, partition source files into shard groups by
top-level module directory.

```bash
# Ensure crate has an entry in progress.toml (handles new crates added after init)
if ! grep -q "^\[${crate}\]" drafts/crates/progress.toml 2>/dev/null; then
  echo "" >> drafts/crates/progress.toml
  echo "[${crate}]" >> drafts/crates/progress.toml
  echo 'status = "pending"' >> drafts/crates/progress.toml
  echo 'timestamp = ""' >> drafts/crates/progress.toml
  echo 'session_id = ""' >> drafts/crates/progress.toml
  echo 'shard_count = 1' >> drafts/crates/progress.toml
fi

# Check progress
STATUS=$(grep -A4 "^\[${crate}\]" drafts/crates/progress.toml 2>/dev/null | \
  grep 'status' | head -1 | sed 's/.*= *"\(.*\)"/\1/')
if [ "$STATUS" = "completed" ]; then
  echo "SKIP: ${crate} already completed"
  echo "CSA_VAR:CRATE_NEEDS_AUDIT=false"
  exit 0
fi

# Determine shard count from SHARD_MAP
SHARD_COUNT=$(echo "${SHARD_MAP}" | tr ',' '\n' | grep "^${crate}:" | cut -d: -f2)
SHARD_COUNT=${SHARD_COUNT:-1}

# Find crate source directory
CRATE_DIR=$(cargo metadata --no-deps --format-version 1 2>/dev/null | \
  jq -r --arg name "${crate}" '.packages[] | select(.name == $name) | .manifest_path' | \
  sed 's|/Cargo.toml$||')

# List source files
SOURCE_FILES=$(find "${CRATE_DIR}/src" -name "*.rs" | sort)

# Build dependency facts paths for context injection
DEPS=$(cargo metadata --no-deps --format-version 1 2>/dev/null | \
  jq -r --arg name "${crate}" '.packages[] | select(.name == $name) | .dependencies[] | select(.path != null) | .name')
DEP_FACTS=""
for dep in $DEPS; do
  FACTS_PATH="drafts/crates/${dep}/facts.toml"
  if [ -f "$FACTS_PATH" ]; then
    DEP_FACTS="${DEP_FACTS}${FACTS_PATH} "
  fi
done

echo "CSA_VAR:CRATE_NEEDS_AUDIT=true"
echo "CSA_VAR:CRATE_DIR=${CRATE_DIR}"
echo "CSA_VAR:DEPENDENCY_FACTS=${DEP_FACTS}"
echo "Auditing ${crate}: ${SHARD_COUNT} shard(s), dir=${CRATE_DIR}, dep_facts=[${DEP_FACTS}]"
```

## IF ${CRATE_NEEDS_AUDIT}

## Step 5: Writer Analysis

Tool: csa
Tier: tier-4-critical

You are the **Writer** for crate `${crate}`.

**Your task**: Read ALL source files in `${CRATE_DIR}/src/` and generate four outputs in `drafts/crates/${crate}/`:

1. **facts.toml** — Machine-readable sidecar with:
   - `exported_apis`: list of public functions/methods with signatures
   - `key_types`: list of public structs/enums/traits with brief descriptions
   - `constraints`: list of invariants and preconditions
   - `risks`: list of potential issues or technical debt
   - `dependency_summary`: what this crate depends on and why

2. **README.md** — Chinese module overview:
   - Architecture and design decisions
   - Public API index with usage examples
   - Key types and their relationships
   - If README exceeds 1000 lines, split into `chapters/01-xxx.md`, `chapters/02-xxx.md` etc.
     and keep README.md as a table of contents linking to chapters

3. **review_report.md** — Chinese code review report:
   - Code quality assessment (error handling, naming, module structure)
   - Security analysis (input validation, unsafe usage, resource limits)
   - Performance observations
   - Recommendations for improvement

4. **blog.md** — Chinese technical deep-dive blog:
   - Explain the crate's design philosophy and key decisions
   - Walk through interesting implementation details
   - Discuss tradeoffs and alternatives considered
   - Target audience: intermediate Rust developers

**Context injection**: Read facts.toml from these dependency crates (if they exist):
${DEPENDENCY_FACTS}

**Rules**:
- ALL prose in Chinese (Simplified). Code snippets, API names, technical terms in English.
- Reference specific line numbers when discussing code.
- Each output file MUST be self-contained and independently readable.
- If a single markdown file would exceed 1000 lines, split into chapters/ subdirectory.

## Step 6: Reviewer Verification

Tool: csa
Tier: tier-4-critical

You are the **Reviewer** for crate `${crate}`.

**Your task**: Fact-check ALL files generated by the Writer in `drafts/crates/${crate}/`.

For each file (README.md, review_report.md, blog.md, facts.toml, and any chapters/*.md):

1. **Source verification**: Every line number reference, function signature, type definition,
   and code snippet MUST exist in the actual source at `${CRATE_DIR}/src/`. Read the source
   files and verify.

2. **Consistency check**: facts.toml must be consistent with README.md and review_report.md.
   No contradictions between documents.

3. **Completeness check**: All public APIs listed in facts.toml must appear in README.md.
   No major public type or function should be missing.

4. **Accuracy check**: Technical claims in blog.md must be verifiable from source code.
   No hallucinated features or behaviors.

**Actions**:
- If you find errors: fix them directly in the output files.
- If facts.toml has wrong signatures: correct them.
- If line numbers are wrong: update them.
- Write a brief review summary to stdout listing what you checked and what you fixed.

## Step 7: Update Progress

Tool: bash
MaxIterations: 20

Update progress.toml for this crate after successful Writer + Reviewer pass.

```bash
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
# Update status in progress.toml
sed -i "/^\[${crate}\]/,/^$/{
  s/status = \".*\"/status = \"completed\"/
  s/timestamp = \".*\"/timestamp = \"${TIMESTAMP}\"/
}" drafts/crates/progress.toml
echo "Completed: ${crate} at ${TIMESTAMP}"
```

## ENDIF

## ENDFOR

## Step 8: Generate Global Summary

Tool: csa
Tier: tier-4-critical

Read ALL `facts.toml` files from `drafts/crates/*/facts.toml` and the first 50 lines
of each `README.md`.

Generate `drafts/crates/SUMMARY.md` (in Chinese) containing:

1. **Architecture Overview**: How the crates relate to each other, with a Mermaid dependency diagram.
2. **Cross-Crate API Consistency**: Are naming conventions consistent? Are error types compatible?
3. **Key Design Patterns**: Common patterns used across crates (e.g., RAII, typestate, builder).
4. **Risk Summary**: Aggregated risks from all crates' facts.toml.
5. **Statistics**: Total files analyzed, total lines, crate count, document count.

## Step 9: Verify Completion

Tool: bash
OnFail: abort

Verify all crates have been completed.

```bash
PENDING=$(grep 'status = "pending"' drafts/crates/progress.toml | wc -l)
if [ "$PENDING" -gt 0 ]; then
  echo "ERROR: ${PENDING} crate(s) still pending"
  grep -B1 'status = "pending"' drafts/crates/progress.toml
  exit 1
fi
echo "All crates completed successfully"
```

## Step 10: Final Statistics

Tool: bash

Output final audit statistics.

```bash
COMPLETED=$(grep 'status = "completed"' drafts/crates/progress.toml | wc -l)
TOTAL_DOCS=$(find drafts/crates/ -name "*.md" -o -name "*.toml" | grep -v progress.toml | wc -l)
TOTAL_LINES=$(find drafts/crates/ -name "*.md" | xargs wc -l 2>/dev/null | tail -1 | awk '{print $1}')
echo "=== Codebase Audit Complete ==="
echo "Crates analyzed: ${COMPLETED}"
echo "Documents generated: ${TOTAL_DOCS}"
echo "Total documentation lines: ${TOTAL_LINES}"
echo "Output directory: drafts/crates/"
```
