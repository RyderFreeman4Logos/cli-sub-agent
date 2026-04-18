# Output Schema — Review Artifacts

> This file defines the structured output formats for the csa-review skill.
> The review agent must generate both artifacts at the end of the review.

## review-findings.json

```json
{
  "findings": [
    {
      "fid": "string",
      "severity": "critical|high|medium|low|info",
      "file": "string",
      "line": 0,
      "rule_id": "string",
      "summary": "string",
      "engine": "reviewer|semgrep|clippy|custom",
      "finding_type": "correctness|regression|security|test-gap|maintainability|agents-md-violation|plan-deviation|spec-deviation|unverified-criterion",
      "trigger": "string",
      "expected": "string",
      "actual": "string",
      "impact": "string",
      "evidence": "string",
      "verification": "string",
      "attack_path": "string",
      "preconditions": "string",
      "exploit_steps": "string",
      "blast_radius": "string",
      "mitigation": "string",
      "cwe": "string",
      "fix_hint": "string",
      "test_case_hint": "string",
      "confidence": 0.0
    }
  ],
  "severity_summary": {
    "critical": 0,
    "high": 0,
    "medium": 0,
    "low": 0,
    "info": 0
  },
  "review_mode": "standard|red-team",
  "schema_version": "1.0",
  "session_id": "string",
  "timestamp": "RFC3339 string",
  "overall_risk": "low|medium|high|critical",
  "overall_summary": "string",
  "agents_md_checklist": [
    {
      "file": "string",
      "agents_chain": ["string"],
      "rule_id": "string",
      "source_agents_md": "string",
      "result": "pass|violation",
      "evidence": "string"
    }
  ],
  "test_gaps": ["string"],
  "open_questions": [
    {
      "id": "string",
      "question": "string",
      "needed_evidence": "string"
    }
  ],
  "security_review": {
    "security_mode": "auto|on|off",
    "adversarial_pass_executed": true,
    "triggered_by": ["string"]
  },
  "suggested_next_actions": ["string"],
  "generated_outputs": {
    "commit_message": "string or null",
    "pr_body": "string or null"
  }
}
```

### Aggregation Compatibility

`review-findings.json` is consumed by `review_consensus.rs`, so the following fields are
mandatory and must remain ReviewArtifact-compatible:

- Top level: `findings`, `severity_summary`, `review_mode`, `schema_version`, `session_id`, `timestamp`
- Per finding: `fid`, `severity`, `file`, `line`, `rule_id`, `summary`, `engine`

Additional rich fields are allowed and will be ignored by the consolidator when not needed.

## $CSA_SESSION_DIR/output/review-verdict.json

```json
{
  "schema_version": 1,
  "session_id": "string",
  "timestamp": "RFC3339 string",
  "decision": "pass|fail|skip|uncertain",
  "verdict_legacy": "CLEAN|HAS_ISSUES",
  "severity_counts": {
    "critical": 0,
    "high": 0,
    "medium": 0,
    "low": 0,
    "info": 0
  },
  "prior_round_refs": ["01KM..."]
}
```

- `schema_version` is fixed at `1`.
- `decision` uses the structured four-value review verdict.
- `verdict_legacy` preserves the legacy binary token for compatibility.
- `severity_counts` is a map keyed by severity name with finding counts.
- `prior_round_refs` lists earlier related review session IDs when available.

Whitelist constraints:

- NO file contents
- NO diff
- NO env
- NO api_key
- NO user TOML

### generated_outputs

Optional outputs generated when the review finds **no P0 or P1 issues**:

- **commit_message**: A Conventional Commits message (English) summarizing the changes. Only generated for per-commit reviews (`uncommitted` scope). Set to `null` if P0/P1 issues exist or scope is not per-commit.
- **pr_body**: A PR description with `## Summary` (bullet points) and `## Test plan` (checklist). Only generated for pre-PR reviews (`base:<branch>` or `range:` scope). Set to `null` if P0/P1 issues exist or scope is not pre-PR.

### agents_md_checklist

Mandatory AGENTS.md compliance evidence:

- One entry per `(changed file, applicable AGENTS.md rule)` pair.
- `result` must be `pass` or `violation` (no third state).
- Missing entries mean review is incomplete.

## review-report.md

```markdown
# Code Review Report

## Scope
- Scope: {scope}
- Mode: {mode}
- Review mode: {review_mode}
- Context source: CLAUDE.md
- Security mode: {security_mode}

## Findings (ordered by severity)
1. [P?][<finding_type>] <summary> (`<file>:<line>`, confidence=<0.00>)

## AGENTS.md Checklist
- [x] `<file>` | `<rule-id>` | `<source AGENTS.md>` | PASS
- [x] `<file>` | `<rule-id>` | `<source AGENTS.md>` | VIOLATION (finding: `<id>`)

## Security Findings (attacker perspective)
1. [P?][security] <summary> (`<file>:<line>`)
- Attack path: <...>
- Preconditions: <...>
- Exploit steps: <...>
- Blast radius: <...>
- Mitigation: <...>

## Test Coverage Findings
1. [P?][test-gap] <summary> (`<file>:<line>`)

## Test Gaps
- <gap>

## Open Questions
- <question + needed evidence>

## Overall Risk
- <risk>

## Recommended Actions
1. <action>

## Suggested Commit Message
<!-- Only present when no P0/P1 findings and scope is per-commit -->
<type>(<scope>): <description>

## Suggested PR Body
<!-- Only present when no P0/P1 findings and scope is pre-PR -->
## Summary
- <bullet points>

## Test plan
- [ ] <checklist items>
```

Write `review-findings.json` to `$CSA_SESSION_DIR/review-findings.json` and
`review-verdict.json` to `$CSA_SESSION_DIR/output/review-verdict.json`.
