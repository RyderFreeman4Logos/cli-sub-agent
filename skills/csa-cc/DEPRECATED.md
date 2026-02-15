# REMOVED: csa-cc

This skill has been removed (SKILL.md deleted per pre-production versioning
policy). This file serves as migration reference only.

## Reason

The thin routing layer that `csa-cc` provides is now handled directly by
CSA's built-in tool routing and the pattern-based workflow system in
`drafts/patterns/`.

## Migration

- For code review delegation: use the `csa-review` pattern
- For security analysis: use the `security-audit` pattern
- For general CSA delegation: use `csa run --tool <tool>` directly
- For domain-specific skills (rust-dev, security, test-gen): these remain
  as global skills and do not need the csa-cc routing layer

## Timeline

- Deprecated: 2026-02-14
- SKILL.md removed: 2026-02-14 (pre-production, no compatibility shim needed)
