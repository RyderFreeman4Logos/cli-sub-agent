#!/usr/bin/env bash
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT

step_eleven="$(python3 - "$repo_root/patterns/dev2merge/workflow.toml" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as source:
    workflow = tomllib.load(source)
for step in workflow["workflow"]["steps"]:
    if step["title"] == "Self-Review Gate":
        print(step["prompt"])
        break
PY
)"
grep -q 'just quality-gates' <<<"$step_eleven"
grep -q 'elif \[ -f Cargo.toml \]' <<<"$step_eleven"

fixture="$test_root/repo"
mkdir -p "$fixture/scripts/hooks" "$fixture/scripts" "$fixture/.csa/state"
git -C "$fixture" init -q
git -C "$fixture" config user.name "Dev2merge Tests"
git -C "$fixture" config user.email "dev2merge-tests@example.invalid"
git -C "$fixture" remote add origin https://example.invalid/dev2merge.git
cp "$repo_root/scripts/hooks/quality-gate-receipt.sh" "$fixture/scripts/hooks/"
cp "$repo_root/scripts/rename-no-replace.py" "$fixture/scripts/"
cp "$repo_root/rust-toolchain.toml" "$fixture/"
printf '[workspace]\n' >"$fixture/Cargo.toml"
printf '# lock\n' >"$fixture/Cargo.lock"
printf '# weave\n' >"$fixture/weave.lock"
printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>.csa/state/quality-counter\n' \
  >"$fixture/scripts/hooks/pre-push-quality-gates.sh"
for gate in branch-protection version-check review-check; do
  printf '#!/usr/bin/env bash\nset -euo pipefail\nprintf x >>.csa/state/%s-counter\n' "$gate" \
    >"$fixture/scripts/hooks/${gate}.sh"
done
chmod +x "$fixture/scripts/hooks/"*.sh
cat >"$fixture/justfile" <<'EOF'
quality-gates:
    scripts/hooks/quality-gate-receipt.sh -- scripts/hooks/pre-push-quality-gates.sh

pre-push: quality-gates
EOF
cp "$repo_root/lefthook.yml" "$fixture/lefthook.yml"
git -C "$fixture" add Cargo.toml Cargo.lock justfile lefthook.yml rust-toolchain.toml scripts
git -C "$fixture" commit -qm "test: initialize dev2merge fixture"

producer_started_ns="$(date +%s%N)"
producer="$(cd "$fixture" && just quality-gates)"
producer_elapsed_ms="$(( ($(date +%s%N) - producer_started_ns) / 1000000 ))"
producer_identity="$(printf '%s' "$producer" | python3 -c 'import json,sys; value=json.load(sys.stdin); assert value["status"] == "executed"; print(value["receipt_identity"])')"
(cd "$fixture" && scripts/hooks/review-check.sh)
consumer_started_ns="$(date +%s%N)"
consumer="$(cd "$fixture" && lefthook run pre-push 2>&1)"
consumer_elapsed_ms="$(( ($(date +%s%N) - consumer_started_ns) / 1000000 ))"
consumer_identity="$(python3 -c '
import json,sys
for line in sys.stdin:
    line=line.strip()
    if line.startswith("{"):
        try: value=json.loads(line)
        except json.JSONDecodeError: continue
        if value.get("status") == "reused":
            print(value["receipt_identity"]); break
' <<<"$consumer")"

test "$producer_identity" = "$consumer_identity"
test "$(wc -c <"$fixture/.csa/state/quality-counter")" -eq 1
test "$(wc -c <"$fixture/.csa/state/branch-protection-counter")" -eq 1
test "$(wc -c <"$fixture/.csa/state/version-check-counter")" -eq 1
test "$(wc -c <"$fixture/.csa/state/review-check-counter")" -eq 2
echo "PASS dev2merge-quality-gate-receipt identity=${producer_identity} quality_runs=1 executed_ms=${producer_elapsed_ms} reused_ms=${consumer_elapsed_ms}"
