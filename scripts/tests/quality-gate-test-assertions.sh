#!/usr/bin/env bash
# Shared fail-loud assertions for quality-gate receipt shell contracts.

_receipt_test_evidence() {
  local value="$1" rendered digest
  rendered="${value//$'\n'/\\n}"
  rendered="${rendered//$'\r'/\\r}"
  rendered="${rendered//$'\t'/\\t}"
  case "$rendered" in
    *"/"* | *"@"* | *"://"* | \
      *[Aa][Uu][Tt][Hh]* | *[Cc][Rr][Ee][Dd][Ee][Nn][Tt][Ii][Aa][Ll]* | \
      *[Kk][Ee][Yy]* | *[Pp][Aa][Ss][Ss][Ww][Oo][Rr][Dd]* | \
      *[Ss][Ee][Cc][Rr][Ee][Tt]* | *[Tt][Oo][Kk][Ee][Nn]*)
      digest="$(printf '%s' "$value" | sha256sum)"
      printf 'sha256:%s,length:%s' "${digest%% *}" "${#value}"
      ;;
    *)
      if [ "${#rendered}" -le 160 ]; then
        printf '%q' "$rendered"
      else
        digest="$(printf '%s' "$value" | sha256sum)"
        printf 'sha256:%s,length:%s' "${digest%% *}" "${#value}"
      fi
      ;;
  esac
}

_receipt_test_fail() {
  local label="$1" expected="$2" actual="$3"
  printf 'FAIL %s expected=%s actual=%s\n' \
    "$label" "$(_receipt_test_evidence "$expected")" \
    "$(_receipt_test_evidence "$actual")" >&2
  return 1
}

assert_eq() {
  local label="$1" expected="$2" actual="$3"
  [ "$actual" = "$expected" ] || _receipt_test_fail "$label" "$expected" "$actual"
}

assert_ne() {
  local label="$1" unexpected="$2" actual="$3"
  [ "$actual" != "$unexpected" ] || \
    _receipt_test_fail "$label" "different-from:${unexpected}" "$actual"
}

assert_empty() {
  local label="$1" actual="$2"
  [ -z "$actual" ] || _receipt_test_fail "$label" empty "$actual"
}

assert_nonempty() {
  local label="$1" actual="$2"
  [ -n "$actual" ] || _receipt_test_fail "$label" nonempty empty
}

assert_num_lt() {
  local label="$1" limit="$2" actual="$3"
  if [[ ! "$actual" =~ ^[0-9]+$ ]]; then
    _receipt_test_fail "$label" "integer-less-than:${limit}" "non-integer:${actual}"
    return 1
  fi
  [ "$actual" -lt "$limit" ] || \
    _receipt_test_fail "$label" "integer-less-than:${limit}" "$actual"
}

assert_path_exists() {
  local label="$1" path="$2"
  [ -e "$path" ] || _receipt_test_fail "$label" path-exists path-missing
}

assert_path_absent() {
  local label="$1" path="$2"
  [ ! -e "$path" ] && [ ! -L "$path" ] || \
    _receipt_test_fail "$label" path-absent path-present
}

assert_executable() {
  local label="$1" path="$2"
  [ -f "$path" ] && [ -x "$path" ] || \
    _receipt_test_fail "$label" executable-file unavailable-or-nonexecutable
}

assert_contains() {
  local label="$1" needle="$2" actual="$3"
  [[ "$actual" == *"$needle"* ]] || \
    _receipt_test_fail "$label" "contains:${needle}" \
      "missing;content-$(_receipt_test_evidence "$actual")"
}

assert_not_matches() {
  local label="$1" pattern="$2" actual="$3" code
  set +e
  grep -Eq -- "$pattern" <<<"$actual"
  code=$?
  set -e
  case "$code" in
    0)
      _receipt_test_fail "$label" "no-match:${pattern}" \
        "matched;content-$(_receipt_test_evidence "$actual")"
      ;;
    1) return 0 ;;
    *) _receipt_test_fail "$label" matcher-succeeded "matcher-exit-${code}" ;;
  esac
}
