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
_cargo := _repo_root + "/scripts/cargo-env-normalize.sh cargo"
# Default Cargo/rustc fan-out for Just recipes; override with
# `CARGO_BUILD_JOBS=<n> just ...`. Do not default NEXTEST_TEST_THREADS here:
# full-suite runs need nextest's normal parallelism to avoid serializing
# thousands of tests.
export CARGO_BUILD_JOBS := env_var_or_default("CARGO_BUILD_JOBS", "1")
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
    # When path is a symlink, ensure the resolved destination directory exists
    if [ -L "$path" ]; then
        symlink_target="$(readlink "$path")"
        mkdir -p "$symlink_target" 2>/dev/null || true
    fi
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

# Detect monolith files by token/line count using the shared monolith checker.
# Env: MONOLITH_TOKEN_THRESHOLD (default 8000), MONOLITH_LINE_THRESHOLD (default 800), TOKUIN_MODEL (default gpt-4o)
find-monolith-files:
    scripts/monolith/check.sh --scope staged --baseline scripts/monolith/baseline.toml --report-all

# Test the shared monolith checker shell harness.
monolith-test:
    bash scripts/tests/monolith-check-tests.sh

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
    @bash "{{_repo_root}}/scripts/check-version-bumped.sh" "{{_repo_root}}"

# Fast pre-commit: formatting, linting, static analysis only (no tests).
# Tests run in pre-push hook instead, avoiding ~58min commit wait (#1383).
pre-commit-fast:
    just find-monolith-files
    just monolith-test
    just check-path-includes
    just check-generated-artifacts
    just check-version-bumped
    just check-chinese
    just fmt
    ./scripts/hooks/check-env-dependent-tests.sh
    just deny
    just clippy

# Run all checks: monolith guard, env-dependent test lint, Chinese character detection, formatting, linting, and tests.
pre-commit:
    just pre-commit-fast
    just test
    just test-e2e

# ==============================================================================

# Ensure src modules pulled into integration test crates stay crate-root agnostic.
check-path-includes:
    ./scripts/hooks/check-path-included-src.sh --self-test
    ./scripts/hooks/check-path-included-src.sh

# ==============================================================================

# Ensure no Chinese characters exist in source code (enforce English codebase).
# Requires: ripgrep (rg)
check-chinese:
    @echo "Checking for Chinese characters..."
    @! ./scripts/check-chinese.sh

# Format code and re-stage only .rs files that were already staged before fmt.
# Abort first when any staged Rust file also has unstaged hunks.
fmt:
    #!/usr/bin/env bash
    set -euo pipefail
    staged_rs=()
    while IFS= read -r -d '' path; do
        staged_rs+=("$path")
    done < <(git diff --cached --name-only -z -- '*.rs')
    unstaged_rs=()
    while IFS= read -r -d '' path; do
        unstaged_rs+=("$path")
    done < <(git diff --name-only -z -- '*.rs')
    partial=()
    for staged in "${staged_rs[@]}"; do
        for unstaged in "${unstaged_rs[@]}"; do
            if [[ "$staged" == "$unstaged" ]]; then
                partial+=("$staged")
                break
            fi
        done
    done
    if (( ${#partial[@]} > 0 )); then
        printf 'just fmt: refusing to format -- these Rust files are partially staged (mixed staged/unstaged hunks); stage or stash the remaining hunks first:\n' >&2
        printf '  %q\n' "${partial[@]}" >&2
        exit 1
    fi
    if (( ${#staged_rs[@]} == 0 )); then
        exit 0
    fi
    {{_cargo}} fmt --all
    printf '%s\0' "${staged_rs[@]}" | xargs -0 git add --

# Run clippy for the entire workspace (strict mode).
clippy:
    just check-cargo-target-writable
    {{_cargo}} clippy --workspace --all-features -- -D warnings

# Run clippy for a specific package.
# Usage: just clippy-p my-crate
clippy-p package:
    just check-cargo-target-writable
    {{_cargo}} clippy -p {{package}} --all-features -- -D warnings

# Security audit (requires cargo-deny)
deny:
    @# Hide the inclusion graph to keep duplicate-crate warnings bounded.
    @# The full graph is useful interactively, but in AI-driven commit workflows
    @# it can explode into gigabytes of output and crash ACP-backed tools.
    @deny_args="--hide-inclusion-graph"; \
    if [ "${CARGO_DENY_DISABLE_FETCH:-}" = "1" ] || [ "${CARGO_DENY_OFFLINE:-}" = "1" ]; then \
        deny_args="$deny_args --disable-fetch"; \
    fi; \
    deny_output="$(mktemp "${TMPDIR:-/tmp}/csa-deny-output.XXXXXX")"; \
    trap 'rm -f "$deny_output"' EXIT; \
    if {{_cargo}} deny check $deny_args >"$deny_output" 2>&1; then \
        echo "cargo deny check passed (success output suppressed)"; \
    else \
        status=$?; \
        cat "$deny_output"; \
        exit "$status"; \
    fi

# ==============================================================================
# 🧪 Testing
# ==============================================================================

# Run all tests in the workspace across default and feature builds.
# Env: CARGO_BUILD_JOBS defaults to 1; NEXTEST_TEST_THREADS is caller-controlled.
test:
    just check-cargo-target-writable
    just check-nextest-state-writable
    {{_cargo}} nextest run --workspace
    just check-nextest-state-writable
    {{_cargo}} nextest run --workspace --all-features

# Run e2e tests only.
# Env: CARGO_BUILD_JOBS defaults to 1; NEXTEST_TEST_THREADS is caller-controlled.
test-e2e:
    just check-cargo-target-writable
    just check-nextest-state-writable
    {{_cargo}} nextest run --package cli-sub-agent --test e2e --all-features

# Run tests for a specific package.
# Env: CARGO_BUILD_JOBS defaults to 1; NEXTEST_TEST_THREADS is caller-controlled.
# Usage: just test-p my-crate
test-p package:
    just check-cargo-target-writable
    just check-nextest-state-writable
    {{_cargo}} nextest run -p {{package}} --all-features

# Run tests matching a specific pattern/name.
# Env: CARGO_BUILD_JOBS defaults to 1; NEXTEST_TEST_THREADS is caller-controlled.
# Usage: just test-f login_validation
test-f pattern:
    just check-cargo-target-writable
    just check-nextest-state-writable
    {{_cargo}} nextest run --workspace --all-features -E 'test({{pattern}})'

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
    version=$({{_cargo}} metadata --no-deps --format-version 1 \
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
    version=$({{_cargo}} metadata --no-deps --format-version 1 \
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
    version=$({{_cargo}} metadata --no-deps --format-version 1 \
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
    {{_cargo}} build --release --all-features -p cli-sub-agent -p weave
    install -m 755 "${target_dir}/release/csa" /usr/local/bin/csa
    install -m 755 "${target_dir}/release/weave" /usr/local/bin/weave
    echo "Verifying installation..."
    csa --version
    weave --version

# Bump patch version of all workspace crates atomically.
# All crates inherit version.workspace, so a single workspace bump suffices.
# Requires: cargo-edit (cargo install cargo-edit)
bump-patch:
    {{_cargo}} set-version --bump patch -p cli-sub-agent
    @{{_cargo}} run --quiet -p cli-sub-agent -- migrate
    git add Cargo.toml Cargo.lock weave.lock
    @echo "Bumped workspace version:"
    @{{_cargo}} metadata --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "cli-sub-agent" or .name == "weave") | "  \(.name) = \(.version)"'

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
	# Also install independent skills from skills/ directory.
	# Skills listed in MANAGED_SKILLS are inactive (user-triggered only via
	# `csa skill run`) and should NOT be symlinked into .claude/skills/
	# to avoid consuming context-window tokens.
	MANAGED_SKILLS="nohup-poll pattern-creator quality-gate split-project-docs"
	for skill_dir in "${repo_root}"/skills/*/; do
		[ -d "$skill_dir" ] || continue
		skill_name=$(basename "$skill_dir")
		if echo " ${MANAGED_SKILLS} " | grep -q " ${skill_name} "; then
			echo "  skip (managed): ${skill_name}"
			continue
		fi
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
