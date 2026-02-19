# PATTERN.md Syntax Reference

## Frontmatter (TOML, between `---` delimiters)

```toml
---
name = "my-pattern"
description = "What this pattern does in one sentence"
allowed-tools = "Bash, Read, Grep, Glob, Edit, Task, TaskCreate, TaskUpdate, TaskList, TaskGet"
tier = "tier-2-standard"
version = "0.1.0"
---
```

| Field | Required | Values |
|-------|----------|--------|
| `name` | Yes | kebab-case, matches directory name |
| `description` | Yes | One sentence, used in listings |
| `allowed-tools` | Yes | Comma-separated tool names the executor may use |
| `tier` | Recommended | `tier-1-quick` / `tier-2-standard` / `tier-3-complex` |
| `version` | Recommended | SemVer string |

**Note**: PATTERN.md uses TOML frontmatter (`name = "value"`).
Companion SKILL.md uses YAML frontmatter (`name: value`). Do not mix them.

## Body Structure

### Title

```markdown
# Pattern Name: Human-Readable Title
```

### Description Paragraph

Brief explanation of what the pattern does and its key guarantees.

### Steps

Steps are `## Step N:` headings. Each step MAY include:

```markdown
## Step 3: Run Tests

Tool: bash
OnFail: abort

Description of what this step does.

` ` `bash
just test
` ` `
```

| Annotation | Values | Effect |
|------------|--------|--------|
| `Tool:` | `bash` / `csa` / `codex` / `claude-code` / any tool name / omit | What executes this step |
| `OnFail:` | `abort` / `skip` / `retry N` / `delegate [target]` | Error handling strategy |
| `Tier:` | `${VAR}` or literal | Tier override for this step |

**Note on OnFail formats**: In PATTERN.md, `OnFail: retry 2` is a plain string
parsed by `parse_fail_action`. In `workflow.toml`, serde deserializes `FailAction`
enum variants: `on_fail = "abort"` for unit variants, but `on_fail = { retry = 2 }`
(table form) for parameterized variants like `Retry(u32)` or `Delegate(String)`.

### Variables

Use `${VAR_NAME}` anywhere in the body. Variables are:
- Set by the orchestrator before dispatch
- Evaluated by the executor at runtime
- Listed in `workflow.toml` for tooling

Common variables:
- `${FILES}` — files to operate on
- `${BRANCH}` — current git branch
- `${COMMIT_MSG}` — generated commit message

### Control Flow

#### Conditional

```markdown
## IF ${CONDITION}

## Step Na: Only When Condition

Content here only runs when CONDITION is truthy.

## ELSE

## Step Nb: Otherwise

Alternative content.

## ENDIF
```

#### Loop

```markdown
## FOR item IN ${COLLECTION}

## Step Na: Process Each Item

Repeated for each item.

## ENDFOR
```

#### Composition (Include)

```markdown
## INCLUDE other-pattern
```

Inlines the referenced pattern's steps at this point.
The included pattern must exist in the same search paths.

**Example from commit pattern:**
```markdown
## Step 7: Security Audit

Tool: csa
Tier: tier-2-standard
OnFail: abort

## INCLUDE security-audit
```

### Sub-Steps

Use `## Step Na:`, `## Step Nb:` for conditional branches within a step:

```markdown
## IF ${AUDIT_FAIL}

## Step 7a: Fix Audit Issues

Fix blocking issues and re-run from Step 2.

## ENDIF

## IF ${AUDIT_PASS_DEFERRED}

## Step 7b: Record Deferred Issues

Record deferred issues via TaskCreate.

## ENDIF
```

## Tier Annotations

Tiers control which CSA tool/model is selected:

| Tier | Use When | Typical Tool |
|------|----------|-------------|
| `tier-1-quick` | Mechanical, low-judgment tasks | codex (low thinking) |
| `tier-2-standard` | Standard development tasks | auto (codex/claude-code) |
| `tier-3-complex` | Complex reasoning, architecture | claude-code (high thinking) |

Per-step tier override:
```markdown
## Step 10: Generate Commit Message

Tool: csa
Tier: tier-1-quick

Delegate message generation to cheaper tool.
```

## Code Blocks

Code blocks in steps serve as example commands. The executor interprets them
as the primary action for the step:

```markdown
## Step 2: Run Formatters

Tool: bash
OnFail: retry 2

` ` `bash
just fmt
` ` `
```

## Complete Minimal Example

```markdown
---
name = "hello-world"
description = "Minimal pattern demonstrating structure"
allowed-tools = "Bash, Read"
tier = "tier-1-quick"
version = "0.1.0"
---

# Hello World: Minimal Pattern Example

Demonstrates the basic structure of a CSA pattern.

## Step 1: Verify Environment

Tool: bash
OnFail: abort

` ` `bash
echo "Hello from pattern executor"
` ` `

## Step 2: Report

Print results to the user.
```
