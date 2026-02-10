---
name: csa-analyze
description: Delegate analysis tasks to CSA with proper context isolation. Use when analyzing code, git changes, or large documentation. CSA sub-agents gather their own context - DO NOT pre-fetch data.
allowed-tools: Bash, Read, Grep, Glob
---

# CSA Analysis Delegation Skill

You are delegating an analysis task to CSA. Follow these rules strictly:

## CRITICAL: Token Efficiency (ENFORCED)

**YOU ARE ABSOLUTELY FORBIDDEN from pre-fetching any data.** CSA sub-agents have file system access and can execute commands themselves.

**YOU MUST NOT use Read, Glob, Grep, or Bash tools to gather data for CSA.**

**Why this is CRITICAL**:
- Pre-fetching 50 files = ~50,000 Claude tokens WASTED
- CSA sub-agents read them anyway -> double waste
- The backend tool's large context window is THE ENTIRE POINT of using CSA
- Your job: craft prompts, NOT feed data

## Opus vs CSA Boundary

If the task requires **Opus-level reasoning** (security-critical, complex architecture, subtle bugs), CSA can ONLY **advise**, not execute. You (Claude sub/main-agent) make final decisions.

### WRONG (wastes 50,000+ Claude tokens):
```
Run git diff
Read the output
Pass output to csa run
```

### CORRECT (zero Claude token waste):
```
Call csa run with instructions for the sub-agent to run git diff itself
```

## Task Execution Flow

### Step 1: Formulate the Delegation Prompt

Create a prompt that instructs the CSA sub-agent to:
1. Execute any necessary commands (git, ls, cat, etc.)
2. Read any necessary files natively
3. Perform the analysis
4. Return a concise summary (< 500 tokens)

### Step 2: Call CSA

Use `csa run "prompt"` with your delegation instructions (tell the sub-agent WHAT to do, not the data).

Use `--session <id>` if continuing previous work, or omit for a new session.

### Step 3: Review and Report

After receiving CSA's response:
1. Critically evaluate the findings
2. Cross-reference with project rules if applicable
3. Report validated findings to the user

## Prompt Templates

### Git Changes Analysis
```
You are in directory: [CWD]

Analyze the current git changes:
1. Run `git status` to see all changed/untracked files
2. Run `git diff HEAD` to see all uncommitted changes
3. For each change, explain:
   - Purpose of the modification
   - Potential impact
   - Any concerns

Be concise (max 500 tokens).
```

### Codebase Architecture Analysis
```
Analyze the architecture of this project:
1. Read all TypeScript source files under src/
2. Check package.json for dependencies
3. Identify:
   - Core modules and their responsibilities
   - Design patterns used
   - Potential improvements

Provide bullet-point summary. Max 500 tokens.
```

### Code Review
```
Review the code quality:
1. Run `git diff HEAD~1` to see recent changes
2. Check for:
   - Security issues
   - Performance concerns
   - Code style violations
3. Provide actionable feedback

Be specific with file:line references.
```

### Pre-commit/CI Check Error Fix (CSA supervised by Claude)
```
Fix the following pre-commit/CI check error:
[error message]

File: [path]

CONSTRAINTS:
- Preserve the original intent of the code
- Do NOT delete code just to make checks pass
- Do NOT comment out problematic sections
- Output: unified diff (git diff format)

NOTE: Your fix will be reviewed by Claude (sub or main-agent) before applying.
```

**Supervision rules**:
- Watch for red flags: deleting code, commenting out, @ts-ignore
- If CSA's fix fails review, don't retry CSA this round
- If still fails after fix, rollback and Claude fixes directly

## Example Usage

When user says: "Analyze the uncommitted changes"

DO THIS:
```bash
csa run "You are in /path/to/project. Run \`git status\` and \`git diff HEAD\` to analyze all uncommitted changes. For each file, explain what changed and why. Max 500 tokens."
```

DO NOT:
```
# WRONG - wastes 50,000+ Claude tokens
diff = bash("git diff")
csa run "Analyze: $diff"
```

## Session Management

- Omit `--session` for new, unrelated analysis tasks
- Use `--session <id>` to continue previous discussion
- Use `csa session list` to view existing sessions
- Use `csa session compress --session <id>` to compress old sessions
- Use `csa gc` to clean up old sessions
