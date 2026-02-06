# Justfile for Rust Workspace
# Generic helpers for Rust monorepo management.
# ⚠️ AI AGENT: Do NOT modify this file or use `git commit -n`/`--no-verify` to bypass pre-commit. Fix the actual code.

set shell := ["bash", "-c"]
# Automatically load .env file if present
set dotenv-load := true

# --- Environment Setup ---
# Calculate repo root (compatible with submodules)
_repo_root := `git rev-parse --show-superproject-working-tree 2>/dev/null | grep . || git rev-parse --show-toplevel`

# Keep cargo state local to avoid host pollution (Optional)
# export CARGO_HOME := _repo_root + "/.cargo-local"

# Default recipe
default: pre-commit

# ==============================================================================
# 🚀 Core Workflow
# ==============================================================================

# Detect monolith files by token/line count using tokuin + parallel
# Fails fast on first file exceeding threshold; blocks commit
# Env: MONOLITH_TOKEN_THRESHOLD (default 8000), MONOLITH_LINE_THRESHOLD (default 800), TOKUIN_MODEL (default gpt-4)
find-monolith-files:
    #!/usr/bin/env bash
    set -euo pipefail
    export MONOLITH_THRESHOLD_TOKENS="${MONOLITH_TOKEN_THRESHOLD:-8000}"
    export MONOLITH_THRESHOLD_LINES="${MONOLITH_LINE_THRESHOLD:-800}"
    export MONOLITH_MODEL="${TOKUIN_MODEL:-gpt-4}"

    # Write check script to temp file (avoids export -f which fails under zsh $SHELL)
    CHECKER=$(mktemp)
    trap 'rm -f "$CHECKER"' EXIT
    cat > "$CHECKER" << 'SCRIPT'
    #!/usr/bin/env bash
    file="$1"
    threshold_tokens="$MONOLITH_THRESHOLD_TOKENS"
    threshold_lines="$MONOLITH_THRESHOLD_LINES"
    model="$MONOLITH_MODEL"

    # --- Explicit excludes (customize per project) ---
    case "$file" in
        *.lock|*lock.json|*lock.yaml) exit 0 ;;  # package manager locks
        */AGENTS.md|*/FACTORY.md) exit 0 ;;        # auto-generated rule aggregation
    esac
    [ -f "$file" ] || exit 0
    grep -Iq '' "$file" 2>/dev/null || exit 0  # skip binary files

    monolith_error() {
        echo ""
        echo "=========================================="
        echo "ERROR: Monolith file detected! ($1, limit: $2)"
        echo "  File: $file"
        echo "=========================================="
        echo ""
        echo "REQUIRED ACTION:"
        echo "1. Stash your current work first:  git stash push -m 'pre-split'"
        echo "2. Split this file:                /split-monolith-files"
        echo "3. After splitting, retry your commit."
        echo ""
        echo "WHY: Large files cause context window bloat and degrade LLM performance."
        echo "IMPORTANT: Stash before splitting so you can recover via 'git stash pop' if splitting fails."
        echo "=========================================="
    }

    # Fast pre-filter: line count (zero-cost, no external tools)
    lines=$(wc -l < "$file" 2>/dev/null || echo 0)
    if [ "$lines" -gt "$threshold_lines" ]; then
        monolith_error "$lines lines" "$threshold_lines lines"
        exit 1
    fi

    # Accurate check: token count (requires tokuin; tolerates tokuin failures)
    tokens=$(tokuin estimate --model "$model" --format json "$file" 2>/dev/null \
        | jq -r '.tokens // 0' 2>/dev/null || echo 0)
    [ -z "$tokens" ] && tokens=0
    if [ "$tokens" -gt "$threshold_tokens" ]; then
        monolith_error "$tokens tokens" "$threshold_tokens tokens"
        exit 1
    fi
    SCRIPT
    chmod +x "$CHECKER"

    git ls-files --recurse-submodules \
        | parallel --halt now,fail=1 "$CHECKER" {}

# Run all checks: monolith guard, Chinese character detection, formatting, linting, and tests.
pre-commit:
    just find-monolith-files
    just check-chinese
    just fmt
    just deny
    just clippy
    just test
    just test-e2e

# ==============================================================================

# Ensure no Chinese characters exist in source code (enforce English codebase).
# Requires: ripgrep (rg)
check-chinese:
    @echo "Checking for Chinese characters..."
    @! rg "\p{Script=Han}" . --vimgrep --glob '!target/**' --glob '!.git/**' --glob '!**/i18n/*.ftl'

# Format code and auto-stage modified .rs files.
# This allows 'just fmt' to be run immediately before commit without manual 'git add'.
fmt:
    cargo fmt --all
    # Only stage tracked .rs files that were modified by fmt
    git diff --name-only | grep '\.rs$' | xargs -r git add

# Run clippy for the entire workspace (strict mode).
clippy:
    cargo clippy --workspace --all-features -- -D warnings

# Run clippy for a specific package.
# Usage: just clippy-p my-crate
clippy-p package:
    cargo clippy -p {{package}} --all-features -- -D warnings

# Security audit (requires cargo-deny)
deny:
    cargo deny check

# ==============================================================================
# 🧪 Testing
# ==============================================================================

# Run all tests in the workspace.
test:
    cargo nextest run --workspace --all-features

# Run e2e tests only.
test-e2e:
    cargo nextest run --package cli-sub-agent --test e2e --all-features

# Run tests for a specific package.
# Usage: just test-p my-crate
test-p package:
    cargo nextest run -p {{package}} --all-features

# Run tests matching a specific pattern/name.
# Usage: just test-f login_validation
test-f pattern:
    cargo nextest run --workspace --all-features -E 'test({{pattern}})'

# ==============================================================================
# 🛠 Git Helpers
# ==============================================================================

# Self-review helper: Show stats of staged vs unstaged changes.
review:
    @echo "=== Staged changes ==="
    git diff --cached --stat
    @echo ""
    @echo "=== Unstaged changes ==="
    git diff --stat
    @echo ""
    @echo "Review the above before committing."

# Push to all submodules and the main repo (useful for monorepos).
git-push-all:
    git submodule foreach 'git push origin --all'
    git push origin --all

# ==============================================================================
# 📦 Installation
# ==============================================================================

# Install latest local build to /usr/local/bin (requires cargo-auditable).
install:
    CARGO_HOME=/usr/local cargo auditable install --all-features --path crates/cli-sub-agent
    @echo "Verifying installation..."
    @csa --version
