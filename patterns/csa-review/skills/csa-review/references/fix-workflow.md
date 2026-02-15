# Fix Workflow — Review-and-Fix Mode

> This file describes the fix mode protocol for the csa-review skill.
> Fix mode is activated when `mode=review-and-fix` is specified.

## Step 5: Fix Mode (when mode=review-and-fix)

```bash
csa run --tool {review_tool} \
  --session {csa_session_id} \
  "Based on the review findings, fix all P0 and P1 issues:

1. Apply fixes for all P0 and P1 findings, including test-gap findings (add/update tests).
2. For security findings, verify exploit paths are closed and document residual risk.
3. Re-run targeted checks/tests for touched areas and record verification evidence.
4. Generate:
   - fix-summary.md (what was fixed and how)
   - post-fix-review-findings.json (remaining findings after fixes)
5. If any P0/P1 remains, explicitly mark as incomplete with explanation."
```

This resumes the same CSA session, preserving the review context.

## Step 6: Verification

After fixes, optionally run:
```bash
just pre-commit
```
or trigger another review round to verify fixes.
