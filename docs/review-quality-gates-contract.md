# Review Quality Gates Contract

## Purpose and Scope

Review Quality Gates define the blocking contract for review outputs before merge. This contract applies to review artifact identity, policy evaluation, schema evolution, KPI-based gate health, and runtime gate modes. It is normative for all implementation phases that produce or consume review findings.

## FindingId Specification (`fid-v1`)

### Canonical format

`finding_id = base32(sha256(engine + rule_id + path + symbol + span + cwe + anchor_hash))[:26]`

- Hash algorithm: SHA-256.
- Encoding: Base32 (RFC 4648 alphabet, no padding), lowercase in persisted output.
- Length: first 26 characters of encoded digest.
- Input concatenation order is fixed: `engine`, `rule_id`, `path`, `symbol`, `span`, `cwe`, `anchor_hash`.
- Empty fields are allowed and must be represented as empty strings in the same positional order.

### `normalize_path` rules

- Convert to repository-relative path.
- Normalize separators to `/`.
- Remove leading `./`.
- Collapse duplicate separators.
- Resolve `.` segments; preserve `..` segments if they cannot be safely resolved from repository root.
- On Windows paths, normalize drive letter to lowercase and strip drive prefix after repo-relative conversion.

### `anchor_hash` rules

- `anchor_hash = sha256(trim(line[i-1]) + "\n" + trim(line[i]) + "\n" + trim(line[i+1]))`
- Context window is 3 lines centered on the anchor line.
- `trim` means Unicode whitespace trim on both ends per line.
- Missing boundary lines (file start/end) are treated as empty strings.

### Stability guarantees and limitations

- IDs are stable for unchanged semantic location and unchanged normalized inputs.
- Cosmetic edits outside the 3-line anchor window should not change IDs.
- Known limitation: large refactors (file moves, symbol renames, span shifts, surrounding context rewrites) may legitimately change IDs.

## FailPolicy Specification

### Semantics

- `Open` (default): evaluation/logging continues on policy evaluation failure; do not block merge because of gate-internal failures.
- `Closed`: any policy evaluation failure is treated as blocking.

### Strategy priority (monotonically strict)

Policy resolution order is fixed:

1. Built-in defaults
2. Repository config
3. CI override

Later layers may keep or tighten effective strictness, but must not weaken a stricter earlier result.

### Waiver model

Each waiver must include:

- `scope`: what is waived (finding ID, rule, path, or gate mode target)
- `justification`: technical reason
- `ticket`: traceable work item/incident link
- `approver`: accountable reviewer identity
- `expires_at`: UTC expiration timestamp

Expired waivers are invalid and ignored by policy evaluation.

## Schema Versioning Strategy

- Every new artifact type MUST include a `schema_version` field.
- Forward compatibility: newly introduced fields MUST be optional (`Option<T>`) with `serde(default)` for deserialization safety.
- Renamed artifacts MUST support a dual-write period (old + new names) until all consumers migrate; removal is allowed only after migration completion.

## KPI Targets (Phase 2 Gate)

- Precision >= 85% (findings are actionable).
- False merge <= 2% (related findings incorrectly merged).
- Reopen rate <= 5% (rejected findings that recur).
- High-severity waiver ratio <= 10%.

## `gate_mode` Specification

- `monitor` (default): log and report only; no merge blocking.
- `critical_only`: block on Critical/High findings.
- `full`: block on P0-P2 findings.
