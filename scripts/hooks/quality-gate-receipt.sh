#!/usr/bin/env bash
# Reuse a full quality gate only for an identical, clean acceptance manifest.
set -uo pipefail

readonly schema_version="1"
readonly implementation_version="1"

if [ "${1:-}" != "--" ] || [ "$#" -lt 2 ]; then
  echo 'usage: quality-gate-receipt.sh -- <quality-gate-command> [args...]' >&2
  exit 2
fi
shift

repo_root="$(git rev-parse --show-toplevel)" || exit 2
cd "$repo_root" || exit 2
state_dir="${repo_root}/.csa/state/quality-gate-receipts"
mkdir -p "$state_dir" || exit 2
manifest_file="$(mktemp "${state_dir}/manifest.XXXXXX")" || exit 2
receipt_temp=""
result_emitted=0
cleanup() {
  rm -f -- "$manifest_file"
  if [ -n "$receipt_temp" ]; then
    rm -f -- "$receipt_temp"
  fi
}
trap cleanup EXIT

sha256_text() {
  sha256sum | awk '{print $1}'
}

file_digest() {
  local path="$1"
  if [ -f "$path" ] && [ ! -L "$path" ]; then
    sha256sum "$path" | awk '{print $1}'
  else
    printf 'missing\n'
  fi
}

bounded_environment_digest() {
  local name value
  for name in \
    CARGO_BUILD_TARGET CARGO_INCREMENTAL CARGO_PROFILE CFLAGS CC \
    PKG_CONFIG_PATH RUSTDOCFLAGS RUSTFLAGS RUSTUP_TOOLCHAIN; do
    value="${!name-}"
    if [ -n "$value" ]; then
      printf '%s=%s\n' "$name" "$(printf '%s' "$value" | sha256_text)"
    else
      printf '%s=unset\n' "$name"
    fi
  done | sha256_text
}

rust_toolchain_digest() {
  local rustc_path
  rustc_path="$(command -v rustc)" || return 1
  rustc_path="$(realpath -e "$rustc_path")" || return 1
  {
    rustc -vV
    printf 'binary_sha256=%s\n' "$(file_digest "$rustc_path")"
    printf 'binary_realpath_sha256=%s\n' "$(printf '%s' "$rustc_path" | sha256_text)"
  } | sha256_text
}

target_provenance_digest() {
  local target="${CARGO_BUILD_TARGET-}" host
  host="$(rustc -vV | awk -F': ' '$1 == "host" {print $2}')" || return 1
  if [ -z "$target" ]; then
    target="$host"
  elif [ -f "$target" ]; then
    target="file:$(realpath -e "$target" | sha256_text):$(file_digest "$target")"
  fi
  printf 'host=%s\ntarget=%s\n' "$host" "$target" | sha256_text
}

gate_script_digest() {
  local command_name="$1" path
  path="$(command -v "$command_name" 2>/dev/null || true)"
  if [ -z "$path" ] && [ -f "$command_name" ]; then
    path="$command_name"
  fi
  if [ -n "$path" ] && [ -f "$path" ] && [ ! -L "$path" ]; then
    file_digest "$path"
  else
    printf '%s' "$command_name" | sha256_text
  fi
}

recipe_digest() {
  if [ -f justfile ] && just --show quality-gates >/dev/null 2>&1; then
    just --show quality-gates 2>/dev/null | sha256_text
  else
    printf 'quality-gates-recipe-missing' | sha256_text
  fi
}

repository_identity() {
  {
    git rev-list --max-parents=0 HEAD 2>/dev/null | sort
    git remote get-url origin 2>/dev/null | python3 -c '
import re, sys, urllib.parse
raw = sys.stdin.read().strip()
if not raw:
    print("no-origin")
elif "://" in raw:
    parsed = urllib.parse.urlsplit(raw)
    host = parsed.hostname or ""
    port = f":{parsed.port}" if parsed.port else ""
    print(f"{parsed.scheme}://{host}{port}{parsed.path}")
else:
    print(re.sub(r"^[^@/]+@", "", raw))
'
  } | sha256_text
}

collect_manifest() {
  local output="$1" index_clean tracked_clean untracked_digest gate_command_digest
  local checkout_identity head_oid tree_oid index_oid repo_identity implementation_digest
  local rust_toolchain target_provenance feature_matrix environment_digest
  shift
  git diff --cached --quiet --ignore-submodules -- && index_clean=true || index_clean=false
  git diff --quiet --ignore-submodules -- && tracked_clean=true || tracked_clean=false
  untracked_digest="$(git ls-files --others --exclude-standard -z | sha256_text)"
  repo_identity="$(repository_identity)" || return 1
  checkout_identity="$(realpath -e "$repo_root" | sha256_text)" || return 1
  head_oid="$(git rev-parse HEAD)" || return 1
  tree_oid="$(git rev-parse 'HEAD^{tree}')" || return 1
  index_oid="$(git write-tree)" || return 1
  implementation_digest="$(file_digest "${repo_root}/scripts/hooks/quality-gate-receipt.sh")"
  rust_toolchain="$(rust_toolchain_digest)" || return 1
  target_provenance="$(target_provenance_digest)" || return 1
  feature_matrix="$(printf '%s' "${CSA_QUALITY_GATE_FEATURE_MATRIX-workspace-default,workspace-all-features,e2e}" | sha256_text)"
  environment_digest="$(bounded_environment_digest)"
  gate_command_digest="$(printf '%q\0' "$@" | sha256_text)"
  {
    printf 'schema_version=%s\n' "$schema_version"
    printf 'implementation_version=%s\n' "$implementation_version"
    printf 'repository_identity=%s\n' "$repo_identity"
    printf 'checkout_identity=%s\n' "$checkout_identity"
    printf 'head_oid=%s\n' "$head_oid"
    printf 'tree_oid=%s\n' "$tree_oid"
    printf 'index_oid=%s\n' "$index_oid"
    printf 'index_clean=%s\n' "$index_clean"
    printf 'tracked_worktree_clean=%s\n' "$tracked_clean"
    printf 'untracked_worktree_digest=%s\n' "$untracked_digest"
    printf 'cargo_lock_sha256=%s\n' "$(file_digest Cargo.lock)"
    printf 'weave_lock_sha256=%s\n' "$(file_digest weave.lock)"
    printf 'rust_toolchain_sha256=%s\n' "$rust_toolchain"
    printf 'target_provenance_sha256=%s\n' "$target_provenance"
    printf 'feature_matrix_sha256=%s\n' "$feature_matrix"
    printf 'environment_sha256=%s\n' "$environment_digest"
    printf 'justfile_sha256=%s\n' "$(file_digest justfile)"
    printf 'lefthook_sha256=%s\n' "$(file_digest lefthook.yml)"
    printf 'gate_script_sha256=%s\n' "$(gate_script_digest "$1")"
    printf 'quality_gate_entrypoint_sha256=%s\n' "$(file_digest scripts/hooks/quality-gates.sh)"
    printf 'recipe_sha256=%s\n' "$(recipe_digest)"
    printf 'gate_command_sha256=%s\n' "$gate_command_digest"
    printf 'implementation_sha256=%s\n' "$implementation_digest"
  } >"$output"
}

validate_receipt() {
  local receipt="$1" identity="$2" manifest="$3"
  if [ -L "$receipt" ]; then
    printf 'receipt_symlink\n'
    return 1
  fi
  if [ ! -e "$receipt" ]; then
    printf 'receipt_missing\n'
    return 1
  fi
  if [ ! -f "$receipt" ]; then
    printf 'receipt_not_file\n'
    return 1
  fi
  python3 - "$receipt" "$identity" "$manifest" <<'PY'
import hashlib
import json
import pathlib
import sys

receipt_path, expected_identity, manifest_path = sys.argv[1:]
try:
    receipt = json.loads(pathlib.Path(receipt_path).read_text(encoding="utf-8"))
except (OSError, UnicodeError, json.JSONDecodeError):
    print("receipt_malformed")
    raise SystemExit(1)
required = {
    "schema_version",
    "implementation_version",
    "status",
    "identity",
    "manifest_sha256",
    "manifest",
    "receipt_digest",
}
if set(receipt) != required:
    print("receipt_fields_invalid")
    raise SystemExit(1)
if receipt["schema_version"] != 1:
    print("receipt_schema_unknown")
    raise SystemExit(1)
if receipt["implementation_version"] != "1":
    print("receipt_implementation_stale")
    raise SystemExit(1)
if receipt["status"] != "PASS":
    print("receipt_status_not_pass")
    raise SystemExit(1)
manifest = pathlib.Path(manifest_path).read_text(encoding="utf-8")
manifest_digest = hashlib.sha256(manifest.encode()).hexdigest()
if receipt["identity"] != expected_identity or receipt["manifest_sha256"] != expected_identity:
    print("receipt_identity_mismatch")
    raise SystemExit(1)
if manifest_digest != expected_identity or receipt["manifest"] != manifest:
    print("receipt_manifest_mismatch")
    raise SystemExit(1)
payload = {key: receipt[key] for key in sorted(required - {"receipt_digest"})}
digest = hashlib.sha256(
    json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
).hexdigest()
if receipt["receipt_digest"] != digest:
    print("receipt_content_digest_mismatch")
    raise SystemExit(1)
print("valid")
PY
}

write_receipt() {
  local destination="$1" identity="$2" manifest="$3"
  python3 - "$destination" "$identity" "$manifest" <<'PY'
import hashlib
import json
import pathlib
import sys

destination, identity, manifest_path = sys.argv[1:]
manifest = pathlib.Path(manifest_path).read_text(encoding="utf-8")
payload = {
    "identity": identity,
    "implementation_version": "1",
    "manifest": manifest,
    "manifest_sha256": identity,
    "schema_version": 1,
    "status": "PASS",
}
payload["receipt_digest"] = hashlib.sha256(
    json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
).hexdigest()
pathlib.Path(destination).write_text(
    json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n",
    encoding="utf-8",
)
PY
}

emit_result() {
  local status="$1" identity="$2" reason="$3" exit_code="${4:-0}"
  python3 - "$status" "$identity" "$reason" "$exit_code" "$manifest_file" <<'PY'
import json
import pathlib
import sys

status, identity, reason, exit_code, manifest_path = sys.argv[1:]
manifest = dict(
    line.rstrip("\n").split("=", 1)
    for line in pathlib.Path(manifest_path).read_text(encoding="utf-8").splitlines()
)
print(json.dumps({
    "schema_version": 1,
    "status": status,
    "receipt_identity": identity,
    "rejection_reason": None if not reason else reason,
    "gate_exit_code": int(exit_code),
    "provenance": {
        "repository": manifest["repository_identity"],
        "checkout": manifest["checkout_identity"],
        "head": manifest["head_oid"],
    },
}, sort_keys=True, separators=(",", ":")))
PY
  result_emitted=1
}

collection_lock="${state_dir}/collection.lock"
exec 8>"$collection_lock"
flock 8
collect_manifest "$manifest_file" "$@" || {
  echo 'ERROR: could not collect quality-gate acceptance manifest' >&2
  exit 2
}
identity="$(sha256sum "$manifest_file" | awk '{print $1}')"
receipt="${state_dir}/${identity}.json"
lock_file="${state_dir}/${identity}.lock"
handle_signal() {
  local code="$1" reason="$2"
  if [ "$result_emitted" -eq 0 ]; then
    emit_result gate_failed "$identity" "$reason" "$code"
  fi
  exit "$code"
}
trap 'handle_signal 129 signal_hup' HUP
trap 'handle_signal 130 signal_int' INT
trap 'handle_signal 143 signal_term' TERM

exec 9>"$lock_file"
flock 9
flock -u 8
exec 8>&-
reason="$(validate_receipt "$receipt" "$identity" "$manifest_file" 2>/dev/null)"
if [ "$reason" = valid ]; then
  emit_result reused "$identity" ""
  exit 0
fi

quarantine_failed=0
if [ "$reason" != receipt_missing ]; then
  quarantine="${state_dir}/rejected.${identity}.$$.$RANDOM"
  "${repo_root}/scripts/rename-no-replace.py" "$receipt" "$quarantine" >/dev/null 2>&1 || \
    quarantine_failed=1
fi

"$@" >&2
gate_status=$?
if [ "$gate_status" -ne 0 ]; then
  emit_result gate_failed "$identity" gate_exit_nonzero "$gate_status"
  exit "$gate_status"
fi

post_manifest="$(mktemp "${state_dir}/manifest-post.XXXXXX")" || exit 2
if ! collect_manifest "$post_manifest" "$@" || ! cmp -s "$manifest_file" "$post_manifest"; then
  rm -f -- "$post_manifest"
  emit_result executed "$identity" input_drift
  exit 0
fi
rm -f -- "$post_manifest"

if [ "$quarantine_failed" -ne 0 ]; then
  emit_result gate_failed "$identity" publication_failed 1
  exit 1
fi

if grep -qE '^(index_clean|tracked_worktree_clean)=false$' "$manifest_file" || \
   [ "$(grep '^untracked_worktree_digest=' "$manifest_file" | cut -d= -f2)" != "$(printf '' | sha256_text)" ]; then
  emit_result executed "$identity" dirty_state
  exit 0
fi

receipt_temp="$(mktemp "${state_dir}/receipt.${identity}.XXXXXX")" || exit 2
write_receipt "$receipt_temp" "$identity" "$manifest_file" || exit 2
if [ "${CSA_QUALITY_GATE_TEST_FAULT-}" = crash-before-publish ]; then
  kill -KILL "$$"
fi
"${repo_root}/scripts/rename-no-replace.py" "$receipt_temp" "$receipt" >/dev/null 2>&1
rename_status=$?
if [ "$rename_status" -ne 0 ]; then
  if [ "$rename_status" -ne 3 ] || \
     [ "$(validate_receipt "$receipt" "$identity" "$manifest_file" 2>/dev/null)" != valid ]; then
    emit_result gate_failed "$identity" publication_failed 1
    exit 1
  fi
fi
receipt_temp=""
emit_result executed "$identity" "$reason"
