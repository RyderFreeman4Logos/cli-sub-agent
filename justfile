# Justfile for Rust Workspace
# Generic helpers for Rust monorepo management.
# ⚠️ AI AGENT: Do NOT modify this file or use `git commit -n`/`--no-verify` to bypass pre-commit. Fix the actual code.

set shell := ["bash", "-c"]
# Keep Just's transient scripts inside the repo so sandboxed commit paths
# do not depend on a writable XDG runtime dir such as /run/user/$UID.
# Use the repo root itself so the temp path exists in both normal clones and
# linked worktrees, where `.git` may be a file instead of a directory.
set tempdir := "."
# Automatically load .env file if present
set dotenv-load := true

# --- Environment Setup ---
# Calculate repo root (compatible with submodules)
_repo_root := `git rev-parse --show-superproject-working-tree 2>/dev/null | grep . || git rev-parse --show-toplevel`
# Just already executes repository-controlled code, so trust this checkout's
# mise config and avoid interactive trust prompts on sandboxed commit paths.
export MISE_TRUSTED_CONFIG_PATHS := _repo_root

# Keep cargo state local to avoid host pollution (Optional)
# export CARGO_HOME := _repo_root + "/.cargo-local"

# Fail fast when sandboxing blocks the default cargo or nextest write paths.
_check_writable path attempted:
    #!/usr/bin/env bash
    set -euo pipefail
    path="{{path}}"
    attempted="{{attempted}}"
    resolved_path="$(readlink -f "$path" 2>/dev/null || printf '%s' "$path")"
    mkdir -p "$path" 2>/dev/null || true
    probe="$path/.csa-write-probe.$$"
    if touch "$probe" >/dev/null 2>&1; then
        rm -f "$probe"
        exit 0
    fi
    echo >&2 "ERROR: attempted to write ${attempted} at ${path} (resolved: ${resolved_path}), but the path is not writable."
    echo >&2 "Adjust filesystem_sandbox.extra_writable in ~/.config/cli-sub-agent/config.toml to include ${resolved_path}."
    echo >&2 "If ${resolved_path} does not exist yet, create it first (for example: mkdir -p '${resolved_path}'). CSA only auto-creates the default cargo/rustup/tmp writable paths, not user-supplied extra_writable entries."
    echo >&2 "If this is unexpected, file an issue: GH_CONFIG_DIR=~/.config/gh-aider gh issue create --repo RyderFreeman4Logos/cli-sub-agent --title \"Sandbox write denial for ${attempted}\" --body \"Attempted to write ${attempted} at ${path} (resolved: ${resolved_path}), but the sandbox denied it. Please include your sandbox mode and the writable path you expected.\""
    exit 2

check-cargo-target-writable:
    #!/usr/bin/env bash
    set -euo pipefail
    target_dir="${CARGO_TARGET_DIR:-{{_repo_root}}/target}"
    just _check_writable "$target_dir" "cargo build artifacts"

check-nextest-state-writable:
    #!/usr/bin/env bash
    set -euo pipefail
    state_home="${XDG_STATE_HOME:-$HOME/.local/state}"
    just _check_writable "$state_home" "cargo nextest state files"

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
        */PATTERN.md|*/SKILL.md) exit 0 ;;         # workflow pattern definitions (single-purpose docs)
        */workflow.toml) exit 0 ;;                   # weave workflow definitions
        .test-target/*|.test-target/**) exit 0 ;;    # generated test target artifacts
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

# Fail if generated or scratch artifacts are staged for commit.
check-generated-artifacts:
    #!/usr/bin/env bash
    set -euo pipefail
    # Allow deletions so cleanup commits can remove previously tracked artifacts.
    blocked_paths="$(
        git diff --cached --name-only --diff-filter=ACMR \
            | rg '^([.]test-target/|[.]tmp/|target/|diff[.]txt$|test-write$)' || true
    )"
    if [ -n "${blocked_paths}" ]; then
        echo ""
        echo "=========================================="
        echo "ERROR: Generated or scratch artifacts are staged."
        echo "=========================================="
        printf '%s\n' "${blocked_paths}"
        echo ""
        echo "Remove these paths from the commit and keep them ignored."
        echo "=========================================="
        exit 1
    fi

# Verify workspace version has been bumped relative to main (prevents accidental unversioned PRs).
# Skipped on main itself and when CSA_SKIP_VERSION_CHECK=1 (e.g., during release automation).
check-version-bumped:
    #!/usr/bin/env bash
    set -euo pipefail
    branch=$(git symbolic-ref --short HEAD 2>/dev/null || echo "")
    if [ "$branch" = "main" ] || [ "$branch" = "" ]; then
        exit 0
    fi
    if [ "${CSA_SKIP_VERSION_CHECK:-0}" = "1" ]; then
        exit 0
    fi
    # Extract workspace version from Cargo.toml on current branch vs main.
    current=$(cargo metadata --no-deps --format-version 1 \
        | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')
    main_version=$(git show main:Cargo.toml 2>/dev/null \
        | grep -A1 '^\[workspace\.package\]' \
        | grep '^version' | head -1 \
        | sed 's/.*"\(.*\)".*/\1/' || echo "")
    if [ -z "$main_version" ]; then
        echo "WARNING: Could not read main branch version, skipping check."
        exit 0
    fi
    if [ "$current" = "$main_version" ]; then
        echo ""
        echo "=========================================="
        echo "ERROR: Workspace version ($current) matches main."
        echo "=========================================="
        echo ""
        echo "You must bump the version before committing on a feature branch."
        echo "Run:  just bump-patch"
        echo ""
        exit 1
    fi

# Run all checks: monolith guard, env-dependent test lint, Chinese character detection, formatting, linting, and tests.
pre-commit:
    just find-monolith-files
    just check-generated-artifacts
    just check-version-bumped
    just check-chinese
    just fmt
    ./scripts/hooks/check-env-dependent-tests.sh
    just deny
    just clippy
    just test
    just test-e2e

# ==============================================================================

# Ensure no Chinese characters exist in source code (enforce English codebase).
# Requires: ripgrep (rg)
check-chinese:
    @echo "Checking for Chinese characters..."
    @! rg "\p{Script=Han}" . --vimgrep --glob '!target/**' --glob '!.git/**' --glob '!**/i18n/*.ftl' --glob '!skills/mktd/**' --glob '!tests/fixtures/**' --glob '!.claude/rules/**' --glob '!.agents/**' --glob '!CLAUDE.md' --glob '!GEMINI.md'

# Format code and auto-stage modified .rs files.
# This allows 'just fmt' to be run immediately before commit without manual 'git add'.
fmt:
    cargo fmt --all
    # Only stage tracked .rs files that were modified by fmt
    git diff --name-only | grep '\.rs$' | xargs -r git add

# Run clippy for the entire workspace (strict mode).
clippy:
    just check-cargo-target-writable
    cargo clippy --workspace --all-features -- -D warnings

# Run clippy for a specific package.
# Usage: just clippy-p my-crate
clippy-p package:
    just check-cargo-target-writable
    cargo clippy -p {{package}} --all-features -- -D warnings

# Security audit (requires cargo-deny)
deny:
    # Hide the inclusion graph to keep duplicate-crate warnings bounded.
    # The full graph is useful interactively, but in AI-driven commit workflows
    # it can explode into gigabytes of output and crash ACP-backed tools.
    cargo deny check --hide-inclusion-graph

# ==============================================================================
# 🧪 Testing
# ==============================================================================

# Run all tests in the workspace across default and feature builds.
test:
    just check-cargo-target-writable
    just check-nextest-state-writable
    cargo nextest run --workspace
    just check-nextest-state-writable
    cargo nextest run --workspace --all-features

# Run e2e tests only.
test-e2e:
    just check-cargo-target-writable
    just check-nextest-state-writable
    cargo nextest run --package cli-sub-agent --test e2e --all-features

# Run tests for a specific package.
# Usage: just test-p my-crate
test-p package:
    just check-cargo-target-writable
    just check-nextest-state-writable
    cargo nextest run -p {{package}} --all-features

# Run tests matching a specific pattern/name.
# Usage: just test-f login_validation
test-f pattern:
    just check-cargo-target-writable
    just check-nextest-state-writable
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

# Reviewed push: run csa review --range base...HEAD, then push, create/reuse PR,
# and synchronously trigger the post-create review transaction.
push-reviewed base="main":
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "{{base}}" != "main" ]; then
        echo "ERROR: push-reviewed currently supports base=main only."
        exit 1
    fi
    echo "=== Pre-push review: csa review --sa-mode false --range {{base}}...HEAD ==="
    csa review --sa-mode false --range "{{base}}...HEAD"
    echo "=== Review passed. Pushing... ==="
    git push -u origin HEAD
    echo "=== Creating or reusing PR targeting {{base}}... ==="
    set +e
    CREATE_OUTPUT="$(gh pr create --base "{{base}}" 2>&1)"
    CREATE_RC=$?
    set -e
    if [ "${CREATE_RC}" -ne 0 ]; then
        if ! printf '%s\n' "${CREATE_OUTPUT}" | grep -Eiq 'already exists|a pull request already exists'; then
            echo "ERROR: gh pr create failed: ${CREATE_OUTPUT}"
            exit 1
        fi
        echo "PR already exists. Continuing with post-create helper."
    fi
    scripts/hooks/post-pr-create.sh --base "{{base}}"

# Push to all submodules and the main repo (useful for monorepos).
git-push-all:
    git submodule foreach 'git push origin --all'
    git push origin --all

# Show the release tag and commands without creating or pushing anything.
release-dry-run:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(cargo metadata --no-deps --format-version 1 \
        | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')
    tag="v${version}"
    echo "Release dry run only (no changes made)."
    echo "Version: ${version}"
    echo "Tag: ${tag}"
    echo "Would run:"
    echo "  git tag ${tag}"
    echo "  git push origin ${tag}"

# Create local release tag only (no push).
release-tag-local:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(cargo metadata --no-deps --format-version 1 \
        | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')
    tag="v${version}"

    if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
        echo "Local tag already exists: ${tag}"
        exit 1
    fi

    git tag "${tag}"
    echo "Created local tag: ${tag}"
    echo "To publish release artifacts, push manually after verification:"
    echo "  git push origin ${tag}"

# Create tag if missing, print push command, and require explicit confirmation before push.
release-tag:
    #!/usr/bin/env bash
    set -euo pipefail
    version=$(cargo metadata --no-deps --format-version 1 \
        | jq -r '.packages[] | select(.name == "cli-sub-agent") | .version')
    tag="v${version}"

    if git rev-parse -q --verify "refs/tags/${tag}" >/dev/null; then
        echo "Using existing local tag: ${tag}"
    else
        git tag "${tag}"
        echo "Created local tag: ${tag}"
    fi

    echo "About to run: git push origin ${tag}"
    read -r -p "Type 'yes' to confirm push: " confirm
    if [ "${confirm}" != "yes" ]; then
        echo "Push cancelled. Local tag remains: ${tag}"
        exit 1
    fi
    git push origin "${tag}"

# ==============================================================================
# 📦 Installation
# ==============================================================================

# Install git hooks via lefthook. Safe to run multiple times.
install-hooks:
    @git config --unset core.hooksPath 2>/dev/null || true
    lefthook install
    @echo "Lefthook hooks installed."

# Install latest local build to /usr/local/bin (reuses workspace target/ cache).
install:
    #!/usr/bin/env bash
    set -euo pipefail
    target_dir="${CARGO_TARGET_DIR:-{{_repo_root}}/target}"
    just check-cargo-target-writable
    cargo build --release --all-features -p cli-sub-agent -p weave
    install -m 755 "${target_dir}/release/csa" /usr/local/bin/csa
    install -m 755 "${target_dir}/release/weave" /usr/local/bin/weave
    @echo "Verifying installation..."
    @csa --version
    @weave --version

# Bump patch version of all workspace crates atomically.
# All crates inherit version.workspace, so a single workspace bump suffices.
# Requires: cargo-edit (cargo install cargo-edit)
bump-patch:
    cargo set-version --bump patch -p cli-sub-agent
    @echo "Bumped workspace version:"
    @cargo metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "cli-sub-agent" or .name == "weave") | "  \(.name) = \(.version)"'

# Generate CHANGELOG.md from Conventional Commits (requires git-cliff).
changelog:
    git cliff --output CHANGELOG.md
    @echo "CHANGELOG.md updated"

# Install pattern skills by creating symlinks to target directory
# Usage: just install-skills [target=".claude/skills"]
install-skills target=".claude/skills":
	#!/usr/bin/env bash
	set -euo pipefail
	mkdir -p "{{target}}"
	repo_root="$(git rev-parse --show-toplevel)"
	count=0
	for pattern_dir in "${repo_root}"/patterns/*/; do
		skills_dir="${pattern_dir}skills/"
		[ -d "$skills_dir" ] || continue
		for skill_dir in "$skills_dir"*/; do
			[ -d "$skill_dir" ] || continue
			skill_name=$(basename "$skill_dir")
			target_path="{{target}}/${skill_name}"
			if [ -L "$target_path" ]; then
				echo "  skip (symlink exists): ${skill_name}"
			elif [ -e "$target_path" ]; then
				echo "  WARN (non-symlink exists, skipping): ${skill_name}"
			else
				ln -sv "$(realpath "$skill_dir")" "$target_path"
				count=$((count + 1))
			fi
		done
	done
	# Also install independent skills from skills/ directory
	for skill_dir in "${repo_root}"/skills/*/; do
		[ -d "$skill_dir" ] || continue
		skill_name=$(basename "$skill_dir")
		target_path="{{target}}/${skill_name}"
		if [ -L "$target_path" ]; then
			echo "  skip (symlink exists): ${skill_name}"
		elif [ -e "$target_path" ]; then
			echo "  WARN (non-symlink exists, skipping): ${skill_name}"
		else
			ln -sv "$(realpath "$skill_dir")" "$target_path"
			count=$((count + 1))
		fi
	done
	echo "Installed ${count} skill(s) to {{target}}"

# Remove skill symlinks from target directory
# Usage: just uninstall-skills [target=".claude/skills"]
uninstall-skills target=".claude/skills":
	#!/usr/bin/env bash
	set -euo pipefail
	repo_root="$(git rev-parse --show-toplevel)"
	count=0
	for link in "{{target}}"/*/; do
		[ -L "${link%/}" ] || continue
		real=$(realpath "${link%/}" 2>/dev/null || true)
		if [[ "$real" == "${repo_root}/"* ]]; then
			rm -v "${link%/}"
			count=$((count + 1))
		fi
	done
	echo "Removed ${count} skill symlink(s) from {{target}}"
