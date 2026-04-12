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

record_violation() {
    local file="$1"
    local line="$2"
    local message="$3"
    violations+=("$file:$line: $message")
}

# Given a file and the line number of a #[cfg(test)] attribute, extract
# the range of lines belonging to the cfg(test) block (the item that
# follows the attribute — typically `mod tests { ... }`).
# Outputs: "start_line end_line"
cfg_test_range() {
    local file="$1"
    local attr_line="$2"
    local total_lines
    total_lines="$(wc -l < "$file")"

    # Find the opening brace of the item following the attribute.
    local line_num="$attr_line"
    local depth=0
    local found_open=false
    local line=""
    local open_only=""
    local close_only=""

    while [ "$line_num" -le "$total_lines" ]; do
        line="$(sed -n "${line_num}p" "$file")"
        open_only="${line//[^\{]/}"
        close_only="${line//[^\}]/}"
        depth=$((depth + ${#open_only} - ${#close_only}))
        if [ ${#open_only} -gt 0 ] && [ "$found_open" = false ]; then
            found_open=true
        fi
        if [ "$found_open" = true ] && [ "$depth" -le 0 ]; then
            echo "$attr_line $line_num"
            return
        fi
        line_num=$((line_num + 1))
    done
    # If we couldn't find the end, return to end of file.
    echo "$attr_line $total_lines"
}

# Check whether a given line number falls inside any #[cfg(test)] block
# in the file.  Uses the pre-computed ranges passed via the array name.
# Arguments: line_number ranges_string
# ranges_string is space-separated pairs: "start1 end1 start2 end2 ..."
line_in_test_block() {
    local target="$1"
    shift
    local ranges=("$@")
    local i=0
    while [ "$i" -lt "${#ranges[@]}" ]; do
        local rstart="${ranges[$i]}"
        local rend="${ranges[$((i+1))]}"
        if [ "$target" -ge "$rstart" ] && [ "$target" -le "$rend" ]; then
            return 0
        fi
        i=$((i + 2))
    done
    return 1
}

# Returns true if the file name matches common Rust test file patterns.
is_test_named_file() {
    local file="$1"
    case "$file" in
        */tests/*.rs|*/tests.rs|*_tests.rs|*_tests_*.rs|*_test.rs|*_test_*.rs|*/test_*.rs)
            return 0 ;;
    esac
    return 1
}

# Scan a file for home_dir() / env::var("HOME") near assertion macros
# without a .exists() guard.
# Arguments: file [ranges...]
# If ranges is empty, scan the whole file; otherwise only lines in ranges.
check_home_sensitive_asserts() {
    local file="$1"
    shift
    local ranges=("$@")
    local scan_all=false
    if [ "${#ranges[@]}" -eq 0 ]; then
        scan_all=true
    fi

    local line_number=""
    while IFS=: read -r line_number _; do
        [ -n "$line_number" ] || continue

        # If we have ranges, skip lines outside test blocks.
        if [ "$scan_all" = false ]; then
            if ! line_in_test_block "$line_number" "${ranges[@]}"; then
                continue
            fi
        fi

        # Extract the enclosing block (brace-balanced) starting at this line.
        local line="" block="" open_only="" close_only="" depth=0
        while IFS= read -r line; do
            block+="$line"$'\n'
            open_only="${line//[^\{]/}"
            close_only="${line//[^\}]/}"
            depth=$((depth + ${#open_only} - ${#close_only}))
            if [ "$depth" -le 0 ]; then
                break
            fi
        done < <(tail -n +"$line_number" "$file")

        # Must contain an assertion macro (any of assert!, assert_eq!, etc.)
        if ! printf '%s' "$block" | grep -Eq 'assert(_eq|_ne|_matches)?!'; then
            continue
        fi

        # Skip if guarded: .exists() check, or if-let-Some wrapping home_dir().
        if printf '%s' "$block" | grep -Eq '\.exists\('; then
            continue
        fi
        if printf '%s' "$block" | grep -Eq 'if let Some\(.*\)\s*=\s*(home_dir|env::var)'; then
            continue
        fi

        record_violation \
            "$file" \
            "$line_number" \
            "HOME-derived value used near assertion without '.exists()' guard"
    done < <(grep -nE 'home_dir\(|env::var\([[:space:]]*"HOME"' "$file" 2>/dev/null || true)
}

# Scan a file for Command::new("which") or Command::new("where") in tests.
# Arguments: file [ranges...]
check_which_where_processes() {
    local file="$1"
    shift
    local ranges=("$@")
    local scan_all=false
    if [ "${#ranges[@]}" -eq 0 ]; then
        scan_all=true
    fi

    local line_number="" matched=""
    while IFS=: read -r line_number matched; do
        [ -n "$line_number" ] || continue

        if [ "$scan_all" = false ]; then
            if ! line_in_test_block "$line_number" "${ranges[@]}"; then
                continue
            fi
        fi

        record_violation \
            "$file" \
            "$line_number" \
            "test shells out to ${matched}; avoid host-specific binary discovery"
    done < <(grep -nE 'Command::new\([[:space:]]*"(which|where)"' "$file" 2>/dev/null || true)
}

# --- Main ---

while IFS= read -r file; do
    if is_test_named_file "$file"; then
        # Entire file is test context — scan everything.
        check_home_sensitive_asserts "$file"
        check_which_where_processes "$file"
    else
        # Only scan within #[cfg(test)] blocks.
        cfg_lines=()
        while IFS= read -r ln; do
            cfg_lines+=("$ln")
        done < <(grep -nE '^\s*#\[cfg\(test\)\]' "$file" 2>/dev/null | cut -d: -f1)

        if [ "${#cfg_lines[@]}" -eq 0 ]; then
            continue
        fi

        # Build ranges array.
        ranges=()
        for ln in "${cfg_lines[@]}"; do
            range_pair="$(cfg_test_range "$file" "$ln")"
            # shellcheck disable=SC2086
            ranges+=($range_pair)
        done

        check_home_sensitive_asserts "$file" "${ranges[@]}"
        check_which_where_processes "$file" "${ranges[@]}"
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
