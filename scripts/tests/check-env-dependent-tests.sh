#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(git rev-parse --show-toplevel)"
CHECKER="${ROOT_DIR}/scripts/hooks/check-env-dependent-tests.sh"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

repo_dir="${TMP_ROOT}/fixture"
mkdir -p "${repo_dir}/crates/demo/src" "${repo_dir}/crates/demo/tests"
git init -q "${repo_dir}"

cat >"${repo_dir}/crates/demo/src/lib.rs" <<'RS'
fn production_only() {
    let home = home_dir();
    assert!(home.is_absolute());
    Command::new("which");
}

#[cfg(test)]
mod tests {
    #[test]
    fn detects_cfg_test_context() {
        let home = home_dir();
        assert!(home.is_absolute());
        Command::new("where");
    }

    #[test]
    fn allows_exists_guard() {
        let home = home_dir();
        assert!(home.exists());
    }
}
RS

cat >"${repo_dir}/crates/demo/tests/host_paths.rs" <<'RS'
#[test]
fn detects_test_named_file() {
    let home = env::var("HOME").unwrap();
    assert!(!home.is_empty());
    Command::new("which");
}
RS

git -C "${repo_dir}" add crates

output_file="${TMP_ROOT}/checker-output"
if (cd "${repo_dir}" && bash "${CHECKER}") >"${output_file}" 2>&1; then
    echo "expected checker to reject HOME/PATH-dependent test fixtures" >&2
    exit 1
fi

assert_contains() {
    local expected="$1"
    if ! grep -Fqx -- "$expected" "${output_file}"; then
        echo "missing expected checker violation: ${expected}" >&2
        cat "${output_file}" >&2
        exit 1
    fi
}

assert_contains "crates/demo/src/lib.rs:11: HOME-derived value used near assertion without '.exists()' guard"
assert_contains "crates/demo/tests/host_paths.rs:3: HOME-derived value used near assertion without '.exists()' guard"
assert_contains "crates/demo/src/lib.rs:13: test shells out to         Command::new(\"where\");; avoid host-specific binary discovery"
assert_contains "crates/demo/tests/host_paths.rs:5: test shells out to     Command::new(\"which\");; avoid host-specific binary discovery"

if grep -Fq "crates/demo/src/lib.rs:2:" "${output_file}" ||
    grep -Fq "crates/demo/src/lib.rs:4:" "${output_file}" ||
    grep -Fq "crates/demo/src/lib.rs:17:" "${output_file}"; then
    echo "checker scanned production code or rejected an .exists() guard" >&2
    cat "${output_file}" >&2
    exit 1
fi
