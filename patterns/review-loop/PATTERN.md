# Review Loop

Bounded iterative review-fix loop for quality convergence.

## Flow

1. **Review**: Run `csa review --diff` on current changes.
2. **Evaluate**: Parse review output for issues.
   - If no issues found → set `${REVIEW_HAS_ISSUES}` to `"false"`, exit success.
   - If issues found → set `${REVIEW_HAS_ISSUES}` to `"true"`.
3. **Fix**: Apply fixes for reported issues.
4. **Round Check**: Increment `${ROUND}`.
   - If `${ROUND}` < `${MAX_ROUNDS}` (default: 2) → go to step 1.
   - If `${ROUND}` >= `${MAX_ROUNDS}` → set `${REMAINING_ISSUES}` with summary, exit.

## Variables

| Variable | Description | Default |
|----------|-------------|---------|
| `${REVIEW_HAS_ISSUES}` | `"true"` if review found issues | — |
| `${ROUND}` | Current round number (starts at 1) | `1` |
| `${MAX_ROUNDS}` | Maximum review-fix rounds | `2` |
| `${REMAINING_ISSUES}` | Summary of unfixed issues (if loop exhausted) | — |

## Constraints

- Maximum 2 rounds by default to prevent infinite loops.
- Uses `csa review --diff` for heterogeneous review (non-self model).
- Review output is parsed for issue markers; no issues = clean exit.
- If max rounds exhausted, remaining issues are reported but execution continues.
