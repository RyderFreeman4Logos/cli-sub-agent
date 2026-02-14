---
name: csa-security
description: Adversarial security analysis expertise for identifying vulnerabilities before attackers do
allowed-tools: Bash, Read, Grep, Glob
---

# Security Audit & Code Review

Adversarial security analysis expertise for identifying vulnerabilities before attackers do.

## Core Principle: Assume All Input Is Malicious

For every code path:
1. If I were an attacker, how would I exploit this?
2. Can I exhaust system resources (memory/CPU/disk/connections)?
3. Can I bypass business logic constraints?

## Security Review Dimensions

| Dimension | Checks |
|-----------|--------|
| **Panic/DoS** | `unwrap()`, array bounds, division by zero, stack overflow, integer overflow |
| **Resource Exhaustion** | Unbounded loops, unlimited memory allocation, connection pool depletion |
| **Race Conditions** | TOCTOU, double-spend attacks, non-atomic operations, concurrent access bugs |
| **Injection Attacks** | SQL injection, command injection, XSS, path traversal, deserialization |
| **Auth/Authz** | Permission bypass, session fixation, credential leakage, privilege escalation |
| **Cryptography** | Weak randomness, timing attacks, plaintext storage, hardcoded secrets |
| **Business Logic** | Negative amounts, integer overflow, state machine bypass, double-processing |

## Domain-Specific Checklists

### Financial/Payment Systems
- [ ] Use fixed-point (NOT floating-point) for amounts
- [ ] All operations use database transactions
- [ ] Idempotency design (prevent replay attacks)
- [ ] Balance check and deduction atomic execution
- [ ] Decimal precision matches business rules

### User Input Processing
- [ ] Input length limits enforced
- [ ] Character set whitelist (reject unexpected chars)
- [ ] Parameterized queries (prevent SQL injection)
- [ ] Output encoding (prevent XSS)
- [ ] Path canonicalization (prevent traversal)

### Async/Concurrency
- [ ] No data races
- [ ] Deadlock risks documented and eliminated
- [ ] Memory ordering correct (Acquire/Release/SeqCst)
- [ ] Cancellation safety verified

## Report Format

```markdown
# Security Review: {Module Name}

**Risk Level**: [Critical / High / Medium / Low]

## Findings

### [Critical] SEC-001: {Title}
**Location**: `path/to/file.rs:123`
**Type**: {Panic DoS / Resource Exhaustion / Race Condition / ...}
**Description**: {Detailed vulnerability description}
**Suggested Fix**: {Code example}

## Priority Action Plan
1. **Immediate**: Critical/High issues
2. **Short-term**: Medium issues
3. **Long-term**: Low issues
```

## Key Rules

**FORBIDDEN PATTERNS**:
- Direct integer arithmetic for financial amounts
- `unwrap()` or `expect()` on untrusted input
- Unbounded allocations
- No validation of external input
- Hardcoded secrets or sensitive data in logs

**REQUIRED PATTERNS**:
- Checked arithmetic (`.checked_add()`, `.checked_mul()`)
- `.get()` instead of `[]` for collections
- Input size/type validation at boundaries
- Constant-time comparison for secrets
- Resource limits on memory/CPU/connections
