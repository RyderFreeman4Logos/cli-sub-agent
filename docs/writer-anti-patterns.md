# Writer Anti-Patterns (Learned from Review Failures)

> Load this document before implementing or fixing code. Each anti-pattern includes the review finding that triggered it, the correct pattern, and a self-check.

## 1. Shell-Safe Placeholders in Generated Commands

**Anti-pattern:** Generated shell commands contain `<placeholder>` with shell metacharacters (`<`, `>`, spaces).

**Review findings that triggered this:**
- #2440 R1: `--prompt-file <continuation prompt>` -- spaces and angle brackets break shell pasting
- #2440 R2: `--prompt-file <path>` -- same class, 4 sites in review_failure_context.rs and review_cmd_post_review.rs

**Correct pattern:** Use shell-safe filenames: `FIX_PROMPT.md`, `CONTINUATION_PROMPT.md`. Class-sweep ALL `--prompt-file`/`--prompt` arg templates.

**Self-check:** `grep -rn '<[a-z_ ]*>' crates/ --include='*.rs' | grep -i 'prompt\|file\|path'` -- any angle-bracket placeholders in generated commands?

## 2. Redact Reviewer-Controlled Text in Output

**Anti-pattern:** Output paths (terminal, JSON, TOML) render review finding metadata (title, category, location, primary_failure) raw without sanitization.

**Review findings that triggered this:**
- #2516 R3: `finding_category()` and `finding_location()` unsanitized
- #2512 R2: `primary_failure` raw in review_label.rs:50
- #2512 R3: `primary_failure` raw in wait_summary.rs:238 (class sweep)

**Correct pattern:** ALL output paths that render finding metadata must sanitize through `redact_text_content()` or equivalent. Class-sweep every output surface.

**Self-check:** Search for `format!` calls that interpolate finding fields directly into output strings.

## 3. Version Stamp Sync (weave.lock)

**Anti-pattern:** Cargo.toml version bump without syncing weave.lock.

**Review finding:** #2512 R1: weave.lock stale (0.1.1081 vs 0.1.1082)

**Correct pattern:** When bumping workspace version, ALWAYS update weave.lock in the same commit.

**Self-check:** After version bump, `grep '^csa = ' weave.lock` must match the version in `grep '^version = ' Cargo.toml`.

## 4. Regression Tests for Security/Redaction Fixes

**Anti-pattern:** Security/redaction/sanitization fix without test proving sensitive text is absent from output.

**Review finding:** #2512 R4: Missing regression coverage for primary_failure redaction

**Correct pattern:** Every redaction/sanitization fix MUST include a test that:
1. Constructs a finding with sensitive text
2. Renders it through the output path
3. Asserts the sensitive text does NOT appear in the output

**Self-check:** Does the fix have a test with `assert!(!output.contains(sensitive))`?

## 5. Negation-Aware Prose Resolution

**Anti-pattern:** Natural-language resolution heuristics classify active findings as resolved based on ambiguous words.

**Review findings that triggered this:**
- #2516 R4: "no longer" heuristic misidentifies active regression as resolved
- #2516 R6: Missing active-problem word guard in `review_signal_describes_resolved_issue`
- #2440 R3: `verified` word triggers false resolution

**Correct pattern:** Resolution heuristics must:
1. Check ACTIVE_PROBLEM_WORDS before classifying as resolved
2. Distinguish negation: "no blocking findings remain" (resolved) vs "still remains" (active)
3. Require EXPLICIT resolution language, not ambiguous single words
4. Treat mixed sentences (resolution word + active word) as NOT resolved

**Self-check:** Test with finding text containing `verified`, `confirmed`, `fixed` -- must NOT classify as resolved.

## 6. Fail-Closed on Empty/Missing Artifacts

**Anti-pattern:** Empty or missing artifact handling drops severity counts or findings instead of failing closed.

**Review finding:** #2516 R5: synthetic-empty path drops low-only JSON severity counts

**Correct pattern:** Empty/missing artifact handling must check ALL severity sources (JSON, TOML, counts) before treating as clean. ANY non-zero severity count (including LOW) = FAIL.

**Self-check:** Test with empty findings.toml but non-zero review-findings.json counts -- must FAIL.

## 7. Session Id Correctness in Continuation Guidance

**Anti-pattern:** Continuation guidance uses resume-wrapper session id instead of the worker session id that owns the failed result.

**Review finding:** #2440 R6: `--fork-from` uses wrapper id, not worker id

**Correct pattern:** Verify which session id flows into `--fork-from` commands -- must be the worker session id, not the resume-wrapper id.

**Self-check:** Test that the generated `--fork-from` value matches the actual worker session id.

## Usage

### For CSA-Codex writers (before commit):
1. Read this document
2. Run the self-checks for each anti-pattern
3. Fix any violations before `git commit --amend`

### For CSA-Codex reviewers:
1. Reference these anti-patterns when reviewing diffs
2. New anti-patterns discovered during review should be added to this document
