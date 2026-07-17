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
    printf 'gate_command_sha256=%s\n' "$gate_command_digest"
    printf 'implementation_sha256=%s\n' "$implementation_digest"
  } >"$output"
}

validate_receipt() {
  local receipt="$1" identity="$2" manifest="$3"
  if [ ! -e "$receipt" ]; then
    printf 'receipt_missing\n'
    return 1
  fi
  if [ -L "$receipt" ]; then
    printf 'receipt_symlink\n'
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
}

collect_manifest "$manifest_file" "$@" || {
  echo 'ERROR: could not collect quality-gate acceptance manifest' >&2
  exit 2
}
identity="$(sha256sum "$manifest_file" | awk '{print $1}')"
receipt="${state_dir}/${identity}.json"
lock_file="${state_dir}/${identity}.lock"

exec 9>"$lock_file"
flock 9
reason="$(validate_receipt "$receipt" "$identity" "$manifest_file" 2>/dev/null)"
if [ "$reason" = valid ]; then
  emit_result reused "$identity" ""
  exit 0
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

if grep -qE '^(index_clean|tracked_worktree_clean)=false$' "$manifest_file" || \
   [ "$(grep '^untracked_worktree_digest=' "$manifest_file" | cut -d= -f2)" != "$(printf '' | sha256_text)" ]; then
  emit_result executed "$identity" dirty_state
  exit 0
fi

receipt_temp="$(mktemp "${state_dir}/receipt.${identity}.XXXXXX")" || exit 2
write_receipt "$receipt_temp" "$identity" "$manifest_file" || exit 2
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
