---
name = "codebase-blog"
description = "Generate technical deep-dive blogs from audit results, bottom-up by module"
allowed-tools = "Bash, Read, Grep, Glob, Write"
tier = "tier-2-deep"
version = "0.1.0"
---

# Technical Blog Generation from Audit Results

Generate technical deep-dive blog posts from completed `codebase-audit` results.
Modules are processed in topological order (leaf dependencies first) so that
upstream module blogs can reference already-published dependency blogs. Each blog
covers what the module does, key design decisions, security considerations
extracted from audit reports, and interesting code patterns.

## Step 1: Ask User Preferences

Tool: (interactive -- orchestrator only)

Before any processing, ask the user for three configuration values:

1. **Preferred language** -- the human language for blog prose (English, Chinese,
   Japanese, etc.). Store as `${BLOG_LANGUAGE}`. Default: English.
2. **Mirror directory** -- the output directory for generated blog files. Store as
   `${MIRROR_DIR}`. Default: `./drafts/blog/`.
3. **Blog style** -- one of:
   - `technical-deep-dive` (detailed code walkthrough, design rationale)
   - `tutorial` (step-by-step explanation aimed at newcomers)
   - `overview` (high-level architecture summary, minimal code)

   Store as `${BLOG_STYLE}`. Default: `technical-deep-dive`.

If executing as a sub-agent with parameters already provided, parse them from the
prompt and skip the interactive prompt.

## Step 2: Validate Prerequisites

Tool: bash
OnFail: abort

Verify that a `codebase-audit` has been completed. At least some modules must
have audit status `generated` (meaning audit reports exist). If no audited
modules are found, abort with an error directing the user to run `/codebase-audit`
first.

```bash
csa audit status --format json --filter generated
```

Parse the output. If the resulting list is empty, report:

> "No audited modules found. Run `/codebase-audit` first to generate audit
> reports, then re-run `/codebase-blog`."

And abort.

## Step 3: Get Work Queue

Tool: bash
OnFail: abort

Retrieve audited modules that do NOT yet have a blog post, in topological order
(leaf dependencies first).

```bash
csa audit status --format json --order topo --filter generated
```

Parse the JSON output. Filter to entries where `blog_exists` is `false` or `null`.
Store the resulting ordered list as `${WORK_QUEUE}`.

If the work queue is empty (all audited modules already have blogs), report
"All audited modules already have blog posts" and skip to Step 7.

## Step 4: Prepare Output Directory

Tool: bash

Create the mirror directory structure for blog posts.

```bash
for file in ${WORK_QUEUE}; do
  mkdir -p "${MIRROR_DIR}/$(dirname "${file}")"
done
```

## FOR module IN ${WORK_QUEUE}

## Step 5: Generate Blog Post

Tool: read, grep, write

### 5a: Load Audit Report

Read the audit report from the audit mirror directory:
`./drafts/security-audit/${MODULE_PATH}.audit.md`

Extract:
- Verdict (PASS / PASS_WITH_NOTES / FAIL)
- Findings (Critical, Warning, Info)
- Cross-Module Notes
- Checklist results

### 5b: Read Source File

Read the source file at `${MODULE_PATH}`. Understand the module's purpose,
public API, internal structure, and key algorithms.

### 5c: Load Prior Blog Context

If any of this module's dependencies already have blog posts in `${MIRROR_DIR}`,
read their Overview sections as compressed context. Use these to:
- Link to dependency blogs where appropriate
- Avoid re-explaining concepts already covered in dependency blogs
- Build a narrative that flows bottom-up through the architecture

Skip this step for leaf modules with no blogged dependencies.

### 5d: Write Blog Post

Generate the blog post in `${BLOG_LANGUAGE}` using `${BLOG_STYLE}` conventions.

The blog MUST follow this structure:

```markdown
# {Module Name}: {Subtitle in BLOG_LANGUAGE}

## Overview
Brief description of what this module does, its role in the system, and why it
exists. If dependencies have blogs, link to them for background context.

## Architecture
How the module is structured internally. Key types, traits, and their
relationships. Data flow through the module. Diagrams if helpful.

## Key Implementation Details
Walkthrough of the most interesting or instructive code patterns in the module.
For `technical-deep-dive` style, include annotated code snippets. For `tutorial`
style, explain step-by-step. For `overview` style, summarize without code.

## Security Notes
Extracted from the audit report. Summarize:
- Audit verdict and what it means
- Any Warning or Critical findings (in accessible language)
- How the module handles trust boundaries with its dependencies

## Lessons Learned
Design decisions worth noting. Tradeoffs made. Patterns that could be reused
in other projects. Anti-patterns avoided and why.
```

Adapt section headers to `${BLOG_LANGUAGE}` (e.g., use Chinese headers if
`${BLOG_LANGUAGE}` is Chinese).

Write the blog to `${MIRROR_DIR}/${MODULE_PATH}.md`.

## Step 6: Update Manifest

Tool: bash

Record the blog path in the `csa audit` manifest so the module is tracked as
having a blog post.

```bash
csa audit update "${MODULE_PATH}" --blog-path "${MIRROR_DIR}/${MODULE_PATH}.md"
```

## ENDFOR

## Step 7: Generate Index

Tool: write

Create `${MIRROR_DIR}/INDEX.md` with a table of contents linking to all blog
posts. Organize by the topological order used during generation so readers can
follow the bottom-up narrative.

```markdown
# Codebase Technical Blog Index

## Overview
- Total modules covered: ${BLOG_COUNT}
- Language: ${BLOG_LANGUAGE}
- Style: ${BLOG_STYLE}
- Generated from: codebase-audit results

## Modules (Bottom-Up Order)

| # | Module | Subtitle | Audit Verdict |
|---|--------|----------|---------------|
| 1 | [module_a](./path/to/module_a.md) | ... | PASS |
| 2 | [module_b](./path/to/module_b.md) | ... | PASS_WITH_NOTES |
| ... | ... | ... | ... |

## Reading Guide
Start from the top (leaf modules) and work your way down. Each blog builds on
concepts introduced in its dependencies.
```

Adapt the index content to `${BLOG_LANGUAGE}`.
