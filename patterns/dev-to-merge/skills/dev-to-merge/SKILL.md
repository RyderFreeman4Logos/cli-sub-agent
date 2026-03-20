---
name: dev-to-merge
description: "Use when: legacy alias, redirects to dev2merge pipeline"
allowed-tools: Bash, Read, Grep, Glob, Edit, Write
triggers:
  - "dev-to-merge"
  - "/dev-to-merge"
---

# Dev-to-Merge (Alias for dev2merge)

This skill is a **legacy alias** for the `dev2merge` skill. All logic has been
consolidated into `dev2merge` as a deterministic weave workflow pipeline.

When invoked, redirect to `/dev2merge` or `csa plan run patterns/dev2merge/workflow.toml`.

See `.claude/skills/dev2merge/SKILL.md` for the full pipeline documentation.
