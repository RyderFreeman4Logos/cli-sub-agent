---
name: codebase-blog
description: Generate technical deep-dive blogs from audit results, bottom-up by module
allowed-tools: Bash, Read, Grep, Glob, Write
triggers:
  - "codebase-blog"
  - "/codebase-blog"
  - "blog from audit"
  - "generate blog"
  - "technical blog"
---

# Codebase Blog: Technical Blog Generation from Audit Results

## Role Detection (READ THIS FIRST -- MANDATORY)

**Check your initial prompt.** If it contains the literal string `"Use the codebase-blog skill"`, then:

**YOU ARE THE EXECUTOR.** Follow these rules:
1. **SKIP the "Execution Protocol" section below** -- it is for the orchestrator, not you.
2. **Read the pattern** at `patterns/codebase-blog/PATTERN.md` and follow it step by step.
3. **ABSOLUTE PROHIBITION**: Do NOT run `csa run`, `csa review`, `csa debate`, or ANY `csa` command. You must perform the work DIRECTLY. Running any `csa` command causes infinite recursion.

**Only if you are the main agent (Claude Code / human user)**:
- You are the **orchestrator**. Follow the "Execution Protocol" steps below.

---

## Purpose

Generate a series of technical blog posts from completed `codebase-audit` results.
Each audited module gets a blog post explaining what it does, how it works, key
design decisions, and security considerations drawn from its audit report.

Blogs are generated bottom-up (leaf dependencies first) so that each post can
reference and link to its dependency blogs, creating a cohesive reading path
through the codebase architecture.

Requires a prior `codebase-audit` run -- this pattern consumes audit results, it
does not perform audits.

## Execution Protocol (ORCHESTRATOR ONLY)

### Prerequisites

- `csa` binary MUST be in PATH: `which csa`
- A `codebase-audit` must have been completed (at least some modules with status `generated`)
- `csa audit status --filter generated` must return non-empty results

### Quick Start

```bash
csa run --skill codebase-blog "Generate technical blogs for all audited modules in English"
```

Or with specific parameters:

```bash
csa run --skill codebase-blog "Blog src/executor/ modules --language Chinese --style tutorial"
```

### Step-by-Step

1. **Check prerequisites**: Verify `csa audit status --filter generated` returns audited modules.
2. **Get work queue**: `csa audit status --format json --order topo --filter generated` -- filter to entries where `blog_exists` is false/null.
3. **Prepare output**: Create `${MIRROR_DIR}` directory (default: `./drafts/blog/`) mirroring source structure.
4. **Per-module blog** (bottom-up):
   - Read the audit report from `./drafts/security-audit/${path}.audit.md`
   - Read the source file
   - Load prior dependency blog posts as narrative context
   - Generate blog post in `${BLOG_LANGUAGE}` with `${BLOG_STYLE}` conventions
   - Write to `${MIRROR_DIR}/${path}.md`
   - Update manifest: `csa audit update <file> --blog-path <blog_path>`
5. **Generate index**: Create `${MIRROR_DIR}/INDEX.md` linking all blogs in topological order.

### Resumability

This pattern is fully resumable. If interrupted:
- `csa audit status --format json --filter generated` with `blog_exists=false` shows remaining work
- Already-blogged modules are skipped (manifest tracks blog paths)
- Re-run the same command to continue from where it left off

## Example Usage

| Command | Effect |
|---------|--------|
| `/codebase-blog` | Blog all audited modules (English, technical-deep-dive) |
| `/codebase-blog --language Chinese` | Blog all audited modules in Chinese |
| `/codebase-blog src/executor/ --style tutorial` | Tutorial-style blogs for executor modules |
| `/codebase-blog --resume` | Continue a previously interrupted blog generation |

## Integration

- **Depends on**: `codebase-audit` (must run first to produce audit reports)
- **Depends on**: `csa audit` CLI (status, update subcommands with `--blog-path`, `--format json`, `--order topo`, `--filter`)
- **Related to**: `codebase-audit` (produces the audit reports this pattern consumes)
- **Output**: `${MIRROR_DIR}` directory (default: `./drafts/blog/`) with per-module blog posts and `INDEX.md`

## Done Criteria

1. All audited modules in scope have blog posts generated in `${MIRROR_DIR}`.
2. Each blog post follows the structured format (Overview, Architecture, Key Implementation Details, Security Notes, Lessons Learned).
3. Blog prose is in `${BLOG_LANGUAGE}` with style matching `${BLOG_STYLE}`.
4. Manifest updated for each module with `--blog-path` (`csa audit status` shows `blog_exists=true` for all processed modules).
5. `${MIRROR_DIR}/INDEX.md` generated with links to all blog posts in topological order.
6. Dependency blogs are cross-referenced where applicable.
