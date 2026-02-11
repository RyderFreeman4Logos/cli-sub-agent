# Output Schema — Review Artifacts

> This file defines the structured output formats for the csa-review skill.
> The review agent must generate both artifacts at the end of the review.

## review-findings.json

```json
{
  "findings": [
    {
      "id": "string",
      "priority": "P0|P1|P2|P3",
      "finding_type": "correctness|regression|security|test-gap|maintainability|agents-md-violation|plan-deviation",
      "file": "string",
      "line": 0,
      "summary": "string",
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
  "overall_risk": "low|medium|high|critical",
  "overall_summary": "string",
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
  "suggested_next_actions": ["string"]
}
```

## review-report.md

```markdown
# Code Review Report

## Scope
- Scope: {scope}
- Mode: {mode}
- Context source: CLAUDE.md
- Security mode: {security_mode}

## Findings (ordered by severity)
1. [P?][<finding_type>] <summary> (`<file>:<line>`, confidence=<0.00>)

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
```

Write both files to the current working directory (or a designated output location).
