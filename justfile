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
# Default Cargo/rustc fan-out for Just recipes is auto-detected from available
# memory; override with `CARGO_BUILD_JOBS=<n> just ...`.
# NEXTEST_TEST_THREADS defaults to 16 in the `test` recipe below (#2650);
# see comment there for rationale.
_auto_build_jobs := `scripts/detect-build-jobs.sh`
export CARGO_BUILD_JOBS := env_var_or_default("CARGO_BUILD_JOBS", _auto_build_jobs)
# Disable nextest's double-spawn test execution mode. With symlinked target/
# directories the double-spawn re-exec fails with "No such file or directory"
# (#1742). The `double-spawn` key was removed from nextest's TOML config schema;
# the NEXTEST_DOUBLE_SPAWN environment variable is the official replacement.
export NEXTEST_DOUBLE_SPAWN := "0"
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

# Verify exact-head release builds ignore live checkout and dotenv contamination.
exact-build-test:
    bash scripts/tests/build-exact-head-binaries-tests.sh

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

# Fast pre-commit (fmt/lint/static); tests run in pre-push (#1383).
pre-commit-fast:
    just find-monolith-files
    just monolith-test
    just exact-build-test
    bash scripts/tests/post-merge-rebuild-tests.sh
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

quality-gates:
    scripts/hooks/quality-gates.sh

pre-push:
    CSA_QUALITY_GATE_HOOK_MODE=1 scripts/hooks/quality-gates.sh

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

# Cap nextest parallelism to avoid exhausting memory and process/thread limits
# when running the full test suite under a CSA sandbox. With ~7000 tests,
# unlimited parallelism causes OOM SIGKILL and fork() EAGAIN (#2650).
# Override with NEXTEST_TEST_THREADS=<n> just test.
# Validate via shell to prevent shell injection from untrusted env values.
# Use case (not grep) to reject multiline values that contain embedded newlines.
_nextest_threads := `_v="${NEXTEST_TEST_THREADS:-16}"; case "$_v" in *[!0-9]*|'') echo 16 ;; *) echo "$_v" ;; esac`

# Run all tests in the workspace across default and feature builds.
# Env: CARGO_BUILD_JOBS defaults to auto-detected safe parallelism;
# NEXTEST_TEST_THREADS caps nextest parallelism (default 16, see #2650).
test:
    just check-cargo-target-writable
    just check-nextest-state-writable
    {{_cargo}} nextest run --workspace --test-threads {{_nextest_threads}}
    just check-nextest-state-writable
    {{_cargo}} nextest run --workspace --all-features --test-threads {{_nextest_threads}}

# Run e2e tests only.
# Env: CARGO_BUILD_JOBS defaults to auto-detected safe parallelism;
# NEXTEST_TEST_THREADS is caller-controlled.
test-e2e:
    just check-cargo-target-writable
    just check-nextest-state-writable
    {{_cargo}} nextest run --package cli-sub-agent --test e2e --all-features

# Run tests for a specific package.
# Env: CARGO_BUILD_JOBS defaults to auto-detected safe parallelism;
# NEXTEST_TEST_THREADS is caller-controlled.
# Usage: just test-p my-crate
test-p package:
    just check-cargo-target-writable
    just check-nextest-state-writable
    {{_cargo}} nextest run -p {{package}} --all-features

# Run tests matching a specific pattern/name.
# Env: CARGO_BUILD_JOBS defaults to auto-detected safe parallelism;
# NEXTEST_TEST_THREADS is caller-controlled.
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

# Reviewed push: run csa review against one captured commit, then push that exact
# commit, create/reuse PR, and synchronously trigger the post-create transaction.
push-reviewed base="main" expected_head="" expected_branch="":
    #!/usr/bin/env bash
    set -euo pipefail
    base={{quote(base)}}
    expected_head={{quote(expected_head)}}
    expected_branch={{quote(expected_branch)}}
    if [ "${base}" != "main" ]; then
        echo "ERROR: push-reviewed currently supports base=main only."
        exit 1
    fi
    review_head="$(git rev-parse HEAD)"
    current_branch="$(git symbolic-ref --quiet --short HEAD)" || {
        echo "ERROR: push-reviewed requires an attached feature branch." >&2
        exit 1
    }
    if [ -n "${expected_head}" ] && [ "${review_head}" != "${expected_head}" ]; then
        echo "ERROR: HEAD changed before review: expected ${expected_head}, found ${review_head}." >&2
        exit 1
    fi
    if [ -n "${expected_branch}" ] && [ "${current_branch}" != "${expected_branch}" ]; then
        echo "ERROR: branch changed before review: expected ${expected_branch}, found ${current_branch}." >&2
        exit 1
    fi
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "ERROR: push-reviewed requires a clean tracked worktree." >&2
        exit 1
    fi
    echo "=== Pre-push review: csa review --sa-mode false --range ${base}...${review_head} ==="
    csa review --sa-mode false --range "${base}...${review_head}"
    reviewed_branch="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
    if [ "$(git rev-parse HEAD)" != "${review_head}" ] \
        || [ "${reviewed_branch}" != "${current_branch}" ] \
        || ! git diff --quiet \
        || ! git diff --cached --quiet; then
        echo "ERROR: HEAD, branch, or tracked files changed during review; refusing to push." >&2
        exit 1
    fi
    echo "=== Review passed. Pushing captured commit ${review_head}... ==="
    git push origin "${review_head}:refs/heads/${current_branch}"
    git branch --set-upstream-to="origin/${current_branch}" "${current_branch}"
    echo "=== Creating or reusing PR targeting ${base}... ==="
    set +e
    CREATE_OUTPUT="$(gh pr create --base "${base}" --head "${current_branch}" 2>&1)"
    CREATE_RC=$?
    set -e
    if [ "${CREATE_RC}" -ne 0 ]; then
        if ! printf '%s\n' "${CREATE_OUTPUT}" | grep -Eiq 'already exists|a pull request already exists'; then
            echo "ERROR: gh pr create failed: ${CREATE_OUTPUT}"
            exit 1
        fi
        echo "PR already exists. Continuing with post-create helper."
    fi
    PR_JSON="$(gh pr list --state open --base "${base}" --head "${current_branch}" --json number,headRefName,headRefOid,baseRefName)"
    if [ "$(printf '%s' "${PR_JSON}" | jq 'length')" != "1" ]; then
        echo "ERROR: expected exactly one open PR for ${current_branch} -> ${base}." >&2
        exit 1
    fi
    PR_NUMBER="$(printf '%s' "${PR_JSON}" | jq -r '.[0].number')"
    REMOTE_HEAD="$(printf '%s' "${PR_JSON}" | jq -r '.[0].headRefOid')"
    if [ "${REMOTE_HEAD}" != "${review_head}" ]; then
        echo "ERROR: PR #${PR_NUMBER} points to ${REMOTE_HEAD}, expected reviewed commit ${review_head}." >&2
        exit 1
    fi
    scripts/hooks/post-pr-create.sh \
        --base "${base}" \
        --pr-number "${PR_NUMBER}" \
        --expected-branch "${current_branch}" \
        --expected-head "${review_head}"

# Exact-head reviewed push: build from an isolated archive of one captured SHA,
# force all nested workflow calls to resolve those binaries, then reuse
# push-reviewed.
push-reviewed-exact base="main":
    #!/usr/bin/env bash
    set -euo pipefail
    base={{quote(base)}}
    if [ "${base}" != "main" ]; then
        echo "ERROR: push-reviewed-exact currently supports base=main only." >&2
        exit 1
    fi
    if ! git diff --quiet || ! git diff --cached --quiet; then
        echo "ERROR: push-reviewed-exact requires a clean tracked worktree so release binaries match HEAD." >&2
        echo "Commit or stash tracked changes, then retry." >&2
        exit 1
    fi
    exact_head="$(git rev-parse HEAD)"
    exact_branch="$(git symbolic-ref --quiet --short HEAD)" || {
        echo "ERROR: push-reviewed-exact requires an attached feature branch." >&2
        exit 1
    }
    exact_bin_dir="{{_repo_root}}/target/exact-head/${exact_head}"
    "{{_repo_root}}/scripts/build-exact-head-binaries.sh" \
        --repo "{{_repo_root}}" \
        --head "${exact_head}" \
        --output-dir "${exact_bin_dir}"
    built_branch="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
    if [ "$(git rev-parse HEAD)" != "${exact_head}" ] \
        || [ "${built_branch}" != "${exact_branch}" ] \
        || ! git diff --quiet \
        || ! git diff --cached --quiet; then
        echo "ERROR: HEAD, branch, or tracked files changed during exact-head build; refusing to continue." >&2
        exit 1
    fi
    if [ "$(cat "${exact_bin_dir}/SOURCE_COMMIT")" != "${exact_head}" ]; then
        echo "ERROR: exact-head build provenance does not match ${exact_head}." >&2
        exit 1
    fi
    exact_csa="${exact_bin_dir}/csa"
    exact_weave="${exact_bin_dir}/weave"
    for binary in "${exact_csa}" "${exact_weave}"; do
        if [ ! -x "${binary}" ]; then
            echo "ERROR: exact-head binary was not produced at ${binary}." >&2
            exit 1
        fi
    done
    export PATH="${exact_bin_dir}:${PATH}"
    hash -r
    resolved_csa="$(command -v csa)"
    resolved_weave="$(command -v weave)"
    if [ "${resolved_csa}" != "${exact_csa}" ] || [ "${resolved_weave}" != "${exact_weave}" ]; then
        echo "ERROR: expected csa=${exact_csa} and weave=${exact_weave}; resolved csa=${resolved_csa}, weave=${resolved_weave}." >&2
        exit 1
    fi
    echo "=== Exact-head binaries for ${exact_head}: ${resolved_csa}, ${resolved_weave} ==="
    csa --version
    weave --version
    just push-reviewed "${base}" "${exact_head}" "${exact_branch}"

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

# Install release csa/weave (install_dir=/usr/local/bin).
install install_dir="/usr/local/bin":
    #!/usr/bin/env bash
    set -euo pipefail
    d={{quote(install_dir)}}; t="${CARGO_TARGET_DIR:-{{_repo_root}}/target}"
    just check-cargo-target-writable
    {{_cargo}} build --release --all-features -p cli-sub-agent -p weave
    install -m 755 "$t/release/csa" "$d/csa"
    install -m 755 "$t/release/weave" "$d/weave"
    "{{_repo_root}}/scripts/verify-csa-install-provenance.sh" "$t/release/csa" "$d/csa"
    "$t/release/weave" --version

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
