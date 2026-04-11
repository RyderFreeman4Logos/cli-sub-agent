---
name: split-project-docs
description: Split monolith project documentation (CLAUDE.md, etc.) into condensed summary + reference detail files following AGENTS.md pattern
allowed-tools: Bash, Read, Write, Edit, Glob, Grep, Agent
---

# Split Project Docs: AGENTS.md-Style Progressive Disclosure

Split a monolith project documentation file into a condensed summary with
`→ path` links, plus individual detail files in a reference directory.

## When to Use

- Project doc file (CLAUDE.md, etc.) exceeds monolith token threshold (default 8000)
- `just find-monolith-files` flags a documentation file
- Manual request to reorganize project documentation

## Pattern

**Before:**
```
CLAUDE.md (8000+ tokens)
├── Section A (detailed)
├── Section B (detailed)
└── Section C (detailed)
```

**After:**
```
CLAUDE.md (~2000-4000 tokens, condensed summaries + → links)
drafts/project-rules-ref/
├── section-a.md (full detail)
├── section-b.md (full detail)
└── section-c.md (full detail)
.agents/project-rules-ref → ../drafts/project-rules-ref  (symlink)
```

## Workflow

### Phase 1: Analyze

**Executor: `csa run` (read-only advisor)**

The source file exceeds 8000 tokens. Do NOT read it in main agent context.

```
csa run --tool auto "Analyze @<file_path> for documentation splitting.
Report:
1. Total token count
2. All ## sections with approximate token counts
3. Which sections are already concise (keep inline)
4. Which sections are verbose (extract to detail files)
5. Suggested detail filenames (kebab-case.md)
6. Proposed one-liner summary for each extracted section"
```

**Keep inline** (do not extract):
- Title and project description (first paragraph)
- Sections that are already 1-3 lines
- MANDATORY/CRITICAL behavioral rules (Planning, Task Tracking)
- Command quick-reference tables

**Extract to detail files:**
- Sections with >500 tokens of prose
- Architecture descriptions with detailed type/struct listings
- Workflow descriptions with step-by-step procedures
- Configuration reference tables
- Implementation status inventories

### Phase 2: Prepare

1. **Ensure clean git state:**
   ```bash
   git status  # Should be clean
   ```

2. **Create reference directory:**
   ```bash
   mkdir -p drafts/project-rules-ref
   ```

3. **Create symlink** (relative, repo-portable):
   ```bash
   # From .agents/ directory, link to ../drafts/project-rules-ref
   ln -sfn ../drafts/project-rules-ref .agents/project-rules-ref
   ls -la .agents/project-rules-ref/  # Verify
   ```

### Phase 3: Extract + Condense

**Executor: Claude sub-agent** (needs Edit/Write tools, and the file is large)

Dispatch to sub-agent with this contract:

```
Read <file_path> and split it into condensed summary + detail files.

For each verbose section:
1. Write full content to drafts/project-rules-ref/<section-name>.md
2. Replace section in <file_path> with one-liner summary + link

Summary format (follow AGENTS.md convention):
  **<section-name>** — <one-line summary with key facts, constraints, defaults>.
  → `.agents/project-rules-ref/<section-name>.md`

Rules:
- ZERO information loss. Every detail must exist in either summary or detail file.
- Inline sections stay as-is (short sections, mandatory rules).
- Detail files get the FULL original content, not a rewrite.
- Summary captures the most important facts an agent needs for quick reference.
- Use kebab-case for filenames: architecture.md, git-workflow.md, etc.
```

### Phase 4: Verify

```bash
# Token count must be under threshold
tokuin estimate --model gpt-4 --format json <file_path> | jq '.tokens'
# Should be < 6000 (target 2000-4000)

# All detail files exist and are non-empty
for f in drafts/project-rules-ref/*.md; do
  [ -s "$f" ] && echo "OK: $f" || echo "EMPTY: $f"
done

# Symlink resolves
ls .agents/project-rules-ref/

# Links in condensed file point to real files
grep -oP '→ `\.agents/project-rules-ref/\K[^`]+' <file_path> | while read f; do
  [ -f ".agents/project-rules-ref/$f" ] && echo "OK: $f" || echo "MISSING: $f"
done
```

### Phase 5: Commit

Use plain `git commit` with hooks enabled. Suggested scope: `docs`.

Message pattern: `docs(<scope>): split <filename> into condensed summary + N reference files`

## Configuration

| Parameter | Default | Override |
|-----------|---------|----------|
| Token threshold | 8000 | `MONOLITH_TOKEN_THRESHOLD` env var |
| Target tokens | 2000-4000 | Adjust based on file complexity |
| Reference dir | `drafts/project-rules-ref` | Project convention |
| Symlink path | `.agents/project-rules-ref` | Must match → links |

## Integration

- **Triggered by**: `just find-monolith-files` flagging a .md file
- **Uses**: plain `git commit` for final commit
- **Complements**: `split-monolith-files` skill (for code files, not docs)
