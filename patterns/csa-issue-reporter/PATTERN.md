---
name = "csa-issue-reporter"
description = "File structured GitHub issues when CSA encounters errors during operation"
allowed-tools = "Bash, Read, Grep, Glob"
tier = "tier-1-quick"
version = "0.1.0"
---

# CSA Issue Reporter

File a structured GitHub issue on cli-sub-agent when CSA encounters an error.
Dispatched as a sub-agent with required inputs: error_summary, csa_command,
error_output, context.

## Step 1: Gather Environment Context

Tool: bash

Collect version and system information for the issue report.

```bash
csa --version 2>&1 || echo "csa not found"
uname -srm
rustc --version 2>/dev/null || echo "rustc not available"
git -C "$(git rev-parse --show-toplevel)" log -1 --format="%h %s" 2>/dev/null
```

## Step 2: Determine Labels

Map error type to GitHub labels:
- Quota / rate limit → bug, provider-quota
- Crash / panic → bug, crash
- Parse failure → bug, parsing
- Session state → bug, session
- Tool not found → bug, configuration
- Unexpected behavior → bug

If labels do not exist, fall back to just "bug".

## Step 3: Create Issue

Tool: bash
OnFail: abort

Create the GitHub issue with structured template using gh CLI.
Title follows Conventional Commits: type(scope): error_summary.

```bash
gh issue create \
  --repo RyderFreeman4Logos/cli-sub-agent \
  --title "${ISSUE_TITLE}" \
  --label "bug" \
  --body "${ISSUE_BODY}"
```

## Step 4: Report Back

Return the issue URL to the caller.

```bash
echo "Issue filed: ${ISSUE_URL}"
```
