#!/usr/bin/env bash
# Pre-commit: catch Rust tests that rely on host HOME/PATH behavior.
#
# Scans ALL *.rs files under crates/:
# - Test-named files (tests/*.rs, *_test*.rs, etc.) are scanned entirely.
# - Other files are scanned only within #[cfg(test)] blocks.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
cd "$repo_root"

declare -a violations=()

# Returns true if the file name matches common Rust test file patterns.
is_test_named_file() {
    local file="$1"
    case "$file" in
        */tests/*.rs|*/tests.rs|*_tests.rs|*_tests_*.rs|*_test.rs|*_test_*.rs|*/test_*.rs)
            return 0 ;;
    esac
    return 1
}

# Scan each file once. The awk state machine tracks #[cfg(test)] brace ranges
# and pending HOME-derived expressions while streaming the source. This avoids
# spawning sed once per source line for every cfg(test) block and match.
scan_file() {
    local file="$1"
    local scan_all="$2"

    while IFS= read -r violation; do
        violations+=("$violation")
    done < <(
        awk -v file="$file" -v scan_all="$scan_all" '
            function count_open_braces(line,    copy) {
                copy = line
                return gsub(/\{/, "", copy)
            }

            function count_close_braces(line,    copy) {
                copy = line
                return gsub(/\}/, "", copy)
            }

            function has_if_let_guard(block,    lines, line_count, i) {
                line_count = split(block, lines, "\n")
                for (i = 1; i <= line_count; i++) {
                    if (lines[i] ~ /if let Some\(.*\)[[:space:]]*=[[:space:]]*(home_dir|env::var)/) {
                        return 1
                    }
                }
                return 0
            }

            function finish_home(id,    block) {
                block = home_block[id]
                if (block ~ /assert(_eq|_ne|_matches)?!/ &&
                    block !~ /\.exists\(/ &&
                    !has_if_let_guard(block)) {
                    home_violations[++home_violation_count] = file ":" home_start[id] ": HOME-derived value used near assertion without '\''.exists()'\'' guard"
                }
                delete home_block[id]
                delete home_depth[id]
                delete home_found_open[id]
                delete home_limit[id]
                delete home_start[id]
            }

            function advance_homes(line, opens, brace_delta,    id) {
                for (id = 1; id <= home_count; id++) {
                    if (!(id in home_start)) {
                        continue
                    }
                    home_block[id] = home_block[id] line "\n"
                    if (opens > 0) {
                        home_found_open[id] = 1
                    }
                    home_depth[id] += brace_delta
                    if ((home_found_open[id] && home_depth[id] <= 0) ||
                        (!home_found_open[id] && NR >= home_limit[id])) {
                        finish_home(id)
                    }
                }
            }

            function start_home(line, opens, brace_delta,    id) {
                id = ++home_count
                home_start[id] = NR
                home_limit[id] = NR + 5
                home_block[id] = line "\n"
                home_depth[id] = brace_delta
                home_found_open[id] = (opens > 0)
                if (home_found_open[id] && home_depth[id] <= 0) {
                    finish_home(id)
                }
            }

            function discard_finished_cfg_blocks(    i, next_count) {
                next_count = 0
                for (i = 1; i <= cfg_count; i++) {
                    if (cfg_found_open[i] && cfg_depth[i] <= 0) {
                        continue
                    }
                    ++next_count
                    cfg_found_open[next_count] = cfg_found_open[i]
                    cfg_depth[next_count] = cfg_depth[i]
                }
                for (i = next_count + 1; i <= cfg_count; i++) {
                    delete cfg_found_open[i]
                    delete cfg_depth[i]
                }
                cfg_count = next_count
            }

            {
                opens = count_open_braces($0)
                closes = count_close_braces($0)
                brace_delta = opens - closes

                if ($0 ~ /^[[:space:]]*#\[cfg\(test\)\]/) {
                    ++cfg_count
                    cfg_found_open[cfg_count] = 0
                    cfg_depth[cfg_count] = 0
                }
                for (i = 1; i <= cfg_count; i++) {
                    if (!cfg_found_open[i] && opens > 0) {
                        cfg_found_open[i] = 1
                    }
                    cfg_depth[i] += brace_delta
                }

                in_test_context = (scan_all == "true" || cfg_count > 0)
                advance_homes($0, opens, brace_delta)

                if (in_test_context && $0 ~ /home_dir\(|env::var\([[:space:]]*"HOME"/) {
                    start_home($0, opens, brace_delta)
                }
                if (in_test_context && $0 ~ /Command::new\([[:space:]]*"(which|where)"/) {
                    process_violations[++process_violation_count] = file ":" NR ": test shells out to " $0 "; avoid host-specific binary discovery"
                }

                discard_finished_cfg_blocks()
            }

            END {
                for (id = 1; id <= home_count; id++) {
                    if (id in home_start) {
                        finish_home(id)
                    }
                }
                for (id = 1; id <= home_violation_count; id++) {
                    print home_violations[id]
                }
                for (id = 1; id <= process_violation_count; id++) {
                    print process_violations[id]
                }
            }
        ' "$file"
    )
}

# --- Main ---

while IFS= read -r file; do
    if is_test_named_file "$file"; then
        scan_file "$file" true
    else
        scan_file "$file" false
    fi
done < <(git ls-files 'crates/**.rs')

if [ "${#violations[@]}" -eq 0 ]; then
    exit 0
fi

echo ""
echo "=========================================="
echo "ERROR: Environment-dependent Rust tests detected."
echo "=========================================="
printf '%s\n' "${violations[@]}"
echo ""
echo "Actionable hints:"
echo "- Guard HOME-derived filesystem assertions with 'if path.exists()' or a parent '.exists()' check that matches production behavior."
echo "- Prefer explicit temp directories or injected XDG/HOME env vars in tests instead of reading the host environment."
echo "- Do not shell out to 'which' or 'where' in tests; build a fake PATH or create the binary path directly."
echo "=========================================="
exit 1
