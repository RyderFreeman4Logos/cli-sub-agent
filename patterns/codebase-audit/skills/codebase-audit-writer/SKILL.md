---
name: codebase-audit-writer
description: Writer CSA for deep crate analysis — generates README, review report, blog, and facts.toml
allowed-tools: Bash, Read, Grep, Glob, Write
---

# Codebase Audit Writer

## Role Detection (READ THIS FIRST -- MANDATORY)

Role MUST be determined by explicit mode marker, not fragile natural-language substring matching.
Treat the run as executor ONLY when initial prompt contains:
`<skill-mode>executor</skill-mode>`.

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. You MUST perform the analysis work DIRECTLY by reading source files and writing outputs.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command.

---

## Purpose

Analyze a single crate's source code and produce four outputs:
1. **facts.toml** — machine-readable API/type/constraint sidecar
2. **README.md** — Chinese module overview with architecture + API index
3. **review_report.md** — Chinese code quality and security review
4. **blog.md** — Chinese technical deep-dive blog

## Output Specifications

### 1. facts.toml Schema

```toml
# Machine-readable crate analysis sidecar
[metadata]
crate_name = "csa-core"
analyzed_at = "2026-03-19T05:00:00Z"
source_lines = 2500
source_files = 12

[[exported_apis]]
name = "SessionId::new"
signature = "pub fn new() -> Self"
module = "session"
description = "Create a new ULID-based session identifier"

[[key_types]]
name = "SessionPhase"
kind = "enum"
visibility = "pub"
description = "State machine: Active, Available, Retired"
variants = ["Active", "Available", "Retired"]

[[constraints]]
description = "SessionId must be a valid ULID"
enforced_by = "SessionId::new() uses ulid::Ulid::new()"
scope = "constructor"

[[risks]]
severity = "medium"
description = "No validation on deserialized SessionId strings"
location = "session.rs:42"
suggestion = "Add TryFrom<String> with ULID validation"

[dependency_summary]
direct_deps = ["ulid", "serde", "thiserror"]
workspace_deps = []
summary = "Core domain types with no workspace dependencies (L0 crate)"
```

### 2. README.md Format

```markdown
# {crate_name} — {one-line Chinese description}

## Architecture Overview
{Chinese prose: design philosophy, module structure, key decisions}

## Public API Index
| API | Module | Description |
|-----|--------|-------------|
| `fn_name(args) -> Ret` | module | Chinese description |

## Key Types
### TypeName
{Chinese description with code examples}

## Usage Examples
{Code snippets showing common usage patterns}

## Internal Structure
{Module dependency diagram if >5 modules}
```

**Chapter splitting rule**: If README.md would exceed 1000 lines, create:
- `README.md` as table of contents with links to chapters
- `chapters/01-architecture.md`, `chapters/02-public-api.md`, etc.
- Each chapter: 500-800 lines max

### 3. review_report.md Format

```markdown
# Code Review: {crate_name}

## Summary
{1-paragraph Chinese overview of code quality}

## Quality Assessment

### Error Handling
{Analysis with line references}

### Naming Conventions
{Analysis of identifier naming quality}

### Module Structure
{Is the module hierarchy clean? Pass-through methods? Shallow modules?}

## Security Analysis

### Input Validation
{Public API parameter validation}

### Unsafe Usage
{List all unsafe blocks, verify SAFETY comments}

### Resource Limits
{Unbounded allocations, missing timeouts}

## Performance Observations
{Hot paths, unnecessary allocations, iterator vs index patterns}

## Recommendations
1. {Priority-ordered improvement suggestions}
```

### 4. blog.md Format

```markdown
# {Chinese blog title about the crate}

{Technical deep-dive targeting intermediate Rust developers.
Cover design philosophy, interesting implementation patterns,
tradeoffs, and lessons. 800-1500 lines.}
```

## Writing Rules

1. **Language**: ALL prose in Chinese (Simplified). Code, API names, crate names, technical terms stay English.
2. **Line references**: Use `file.rs:42` format. MUST be accurate — verify by reading the actual file.
3. **Self-contained**: Each output file must be independently readable without the others.
4. **Dependency context**: If facts.toml from dependency crates is provided, use it to explain cross-crate relationships.
5. **No hallucination**: Only describe what exists in the source code. If uncertain, say so.
6. **Completeness**: Every `pub` item must appear in facts.toml. Major public APIs must appear in README.md.

## Execution Protocol (ORCHESTRATOR ONLY)

This skill is invoked by the codebase-audit workflow. It is not meant to be called directly.
The workflow provides `${CRATE_DIR}`, `${crate}`, and `${DEPENDENCY_FACTS}` variables.
