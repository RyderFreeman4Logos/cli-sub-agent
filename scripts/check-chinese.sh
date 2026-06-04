#!/usr/bin/env bash
set -euo pipefail

rg "\p{Script=Han}" . --vimgrep \
  --glob '!target/**' \
  --glob '!.git/**' \
  --glob '!output/**' \
  --glob '!**/i18n/*.ftl' \
  --glob '!skills/mktd/**' \
  --glob '!tests/fixtures/**' \
  --glob '!.claude/rules/**' \
  --glob '!.agents/**' \
  --glob '!CLAUDE.md' \
  --glob '!GEMINI.md'
