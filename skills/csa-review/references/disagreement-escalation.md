# Disagreement Escalation — Finding Dispute Resolution

> This file defines the protocol for handling contested review findings.
> Findings must never be silently dismissed without adversarial arbitration.

## Escalation Protocol

When the developer (or orchestrating agent) disagrees with a csa-review finding:

1. **NEVER silently dismiss findings.** Every finding was produced by an independent
   model with evidence — it deserves adversarial evaluation, not unilateral dismissal.

2. **Use the `debate` skill** to arbitrate contested findings:
   - The finding becomes the "question" for debate
   - The reviewer's evidence is the initial proposal
   - The developer's counter-argument is the critique
   - The debate MUST use independent models (CSA routes to a different backend from both the reviewer and developer)

3. **Record the outcome**: If a finding is dismissed after debate, document the
   debate verdict (with model specs) in the review report or PR comment.

4. **Escalate to user** if debate reaches deadlock (both sides have valid points).

**FORBIDDEN**: Dismissing a csa-review finding without adversarial arbitration.
The code author's confidence alone is NOT sufficient justification.
