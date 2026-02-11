---
name: csa-issue-reporter
description: File GitHub issues when CSA encounters errors during operation. Dispatched as sub-agent.
allowed-tools: Bash, Read, Grep, Glob
triggers:
  - "file csa issue"
  - "report csa issue"
  - "csa issue"
---

# CSA Issue Reporter

File a structured GitHub issue on the cli-sub-agent repository when CSA encounters an error during operation.

## When to Use

Dispatch this as a **sub-agent** whenever CSA encounters:
- Quota exhaustion or rate limit errors
- Unexpected crashes or panics
- Parse failures on CSA output
- Session corruption or state inconsistencies
- Tool binary not found or misconfigured
- Any behavior that deviates from expected CSA semantics

## Required Inputs

The caller MUST provide these in the sub-agent prompt:

| Field | Description |
|-------|-------------|
| `error_summary` | One-line description of the problem |
| `csa_command` | The exact `csa` command that was run (or the programmatic invocation) |
| `error_output` | Stderr/stdout from the failed command (truncate to ~200 lines if huge) |
| `context` | What the caller was trying to accomplish when the error occurred |

Optional:
| Field | Description |
|-------|-------------|
| `session_id` | CSA session ULID if applicable |
| `tool_name` | Which tool was being used (codex, claude-code, opencode, gemini-cli) |
| `workaround` | How the caller recovered (if at all) |

## Execution Protocol

### Step 1: Gather Environment Context

Run these commands to collect environment info:

```bash
csa --version 2>&1 || echo "csa not found"
uname -srm
rustc --version 2>/dev/null || echo "rustc not available"
git -C /home/obj/project/github/RyderFreeman4Logos/cli-sub-agent log -1 --format="%h %s" 2>/dev/null
```

### Step 2: Determine Labels

Map the error type to GitHub labels:

| Error Pattern | Label |
|---------------|-------|
| Quota / rate limit | `bug`, `provider-quota` |
| Crash / panic | `bug`, `crash` |
| Parse failure | `bug`, `parsing` |
| Session state | `bug`, `session` |
| Tool not found | `bug`, `configuration` |
| Unexpected behavior | `bug` |

If labels don't exist yet, use only `bug`.

### Step 3: Create Issue

```bash
gh issue create \
  --repo RyderFreeman4Logos/cli-sub-agent \
  --title "<type>(<scope>): <error_summary>" \
  --label "bug" \
  --body "$(cat <<'ISSUE_EOF'
## Environment

- **CSA version**: <version>
- **OS**: <uname output>
- **Rust**: <rustc version>
- **CSA commit**: <git log output>
- **Tool**: <tool_name or "unknown">

## What Happened

<error_summary expanded to 2-3 sentences>

## Reproduction

### Command
```
<csa_command>
```

### Error Output
```
<error_output, truncated to ~200 lines>
```

## Context

<What was being done when the error occurred. Include:
- Which skill/workflow triggered the CSA call
- What stage of the workflow (e.g., "during pre-PR review", "during debate round 2")
- Any relevant session IDs>

## Workaround

<How the caller recovered, or "None — task blocked">

## Expected Behavior

<What should have happened instead>
ISSUE_EOF
)"
```

### Step 4: Report Back

Return the issue URL to the caller:
```
Issue filed: https://github.com/RyderFreeman4Logos/cli-sub-agent/issues/<number>
```

## Commit Convention

Issue titles MUST follow Conventional Commits style:
- `bug(review): codex quota exhaustion during pre-PR review`
- `bug(session): session state.toml corrupted after SIGKILL`
- `bug(executor): gemini-cli binary not found despite config`

## Example Sub-Agent Dispatch

```
Dispatch a sub-agent (general-purpose) with this prompt:

"Use the csa-issue-reporter skill to file a GitHub issue.

error_summary: codex returned quota exhaustion during csa review --diff
csa_command: csa review --diff --tool codex
error_output: <paste stderr>
context: Running pre-commit review as part of /commit skill workflow.
  The review was on branch fix/csa-todo-bugs, reviewing changes to git.rs.
tool_name: codex
workaround: Fell back to local CSA review with claude-code as reviewer."
```
