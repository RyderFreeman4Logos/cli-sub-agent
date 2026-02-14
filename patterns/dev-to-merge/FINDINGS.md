# Dev-to-Merge Pattern: Compilation Findings

## Summary

The skill-lang format successfully expressed a 24-step development workflow
covering branch validation, quality gates, commit, PR creation, codex-bot
review loop with false-positive arbitration, and merge. The weave compiler
produced a valid TOML execution plan.

## What Worked Well

1. **Variable extraction**: The compiler correctly identified all 10 variables
   (`BRANCH`, `REPO`, `SCOPE`, `COMMIT_MSG`, `REVIEW_HAS_ISSUES`,
   `BOT_HAS_ISSUES`, `BOT_COMMENTS`, `COMMENT_IS_FALSE_POSITIVE`,
   `COMMENT_TEXT`, `PR_NUM`) from `${VAR}` placeholders across the document.

2. **Control flow compilation**: `## IF` / `## ELSE` / `## ENDIF` blocks
   compiled correctly. Conditional steps received `condition` fields
   (`${REVIEW_HAS_ISSUES}`, `${BOT_HAS_ISSUES}`) and ELSE branches received
   negated conditions (`!(${BOT_HAS_ISSUES})`).

3. **FOR loop compilation**: The `## FOR comment IN ${BOT_COMMENTS}` block
   produced steps with `loop_var` containing both `variable` and `collection`,
   correctly tagging all steps inside the loop body.

4. **Nested control flow**: IF inside FOR compiled correctly — the comment
   processing loop contains a conditional arbitration/fix branch.

5. **Hint extraction**: `Tool:`, `Tier:`, and `OnFail:` metadata lines were
   correctly extracted from step bodies and promoted to structured fields.

6. **FailAction variants**: `abort`, `skip`, and `retry N` all serialized
   correctly with the appropriate TOML representation.

## Limitations Found

### 1. Markdown Headers Inside Code Blocks Are Parsed as Steps

**Severity**: High

The `## Summary` and `## Test plan` headers inside a heredoc/code fence in
Step 15 (Create PR) were misinterpreted as new step headers. This split the
PR creation step into 3 separate steps (15, 16, 17) with broken content.

**Root cause**: The parser classifies lines by regex before considering code
fence context. Lines starting with `## ` inside triple-backtick blocks are
treated as step headers.

**Fix needed**: Track fenced code block state (```` ``` ````) in the line
classifier and skip structural parsing inside fences.

### 2. No Step Dependency Graph

**Severity**: Medium

The compiled plan has `depends_on: []` for every step. Sequential ordering
is implicit (by `id`) but the compiler does not infer dependencies from
variable flow (e.g., Step 13 depends on Step 12 producing `COMMIT_MSG`).

**Fix needed**: Data-flow analysis to populate `depends_on` from variable
producers/consumers, enabling parallel execution of independent steps.

### 3. No Loop-Back / Retry-Loop Semantics

**Severity**: Medium

The review-fix-re-review cycle (Steps 9-11) is expressed as a flat IF block,
but the semantic intent is a bounded retry loop: "loop back to Step 9 if
issues persist (max 3 rounds)." The skill-lang has no `## WHILE` or
`## LOOP ... UNTIL` construct.

**Workaround**: Express as `## FOR round IN [1,2,3]` with an IF guard, but
this is awkward and does not support early exit on success.

**Suggestion**: Add `## WHILE condition` / `## ENDWHILE` with max-iteration
guard, or `## RETRY max_count` block.

### 4. No Step Output Binding

**Severity**: Medium

Step 12 (Generate Commit Message) should produce a value that Step 13
consumes as `${COMMIT_MSG}`. The current format has no way to declare that
a step *produces* a variable value — variables are only consumed.

**Fix needed**: Add `Output: ${VAR_NAME}` hint (similar to `Tool:`/`Tier:`)
so the runtime knows which step populates which variable.

### 5. No INCLUDE Resolution at Compile Time

**Severity**: Low

`## INCLUDE` blocks produce a placeholder step with `tool = "weave"`. The
compiler does not resolve includes transitively. This is fine for now (the
runtime would handle it) but means the compiled plan may have unresolved
references.

### 6. Condition Expressions Are Opaque Strings

**Severity**: Low

Conditions like `${REVIEW_HAS_ISSUES}` and `${COMMENT_IS_FALSE_POSITIVE}`
are stored as raw strings. The compiler performs no validation on whether
these are boolean-evaluable or reference defined variables. A typo like
`${REVEW_HAS_ISSUES}` would silently compile.

**Fix needed**: Validate that all variables in conditions appear in the
extracted variable set. Optionally support simple expressions (`==`, `!=`,
`&&`, `||`).

## Suggestions for Runtime Executor Design

1. **Variable resolution**: The executor needs a variable store. Steps with
   `Output: ${VAR}` hints populate the store; `${VAR}` in prompts are
   template-substituted before execution.

2. **Condition evaluation**: A minimal expression evaluator for conditions.
   At minimum: truthy/falsy (non-empty = true), negation (`!(...)`), and
   equality (`== "value"`).

3. **Loop execution**: FOR blocks re-execute their child steps once per
   collection element, binding the loop variable.

4. **Failure handling**: `on_fail` determines behavior on step failure:
   - `abort` → halt the plan
   - `skip` → continue to next step
   - `retry N` → re-execute up to N times
   - `delegate target` → hand off to a different tool/tier

5. **INCLUDE resolution**: At runtime, resolve `## INCLUDE` by loading and
   compiling the referenced PATTERN.md, then splicing its steps into the
   current plan.

6. **Code fence awareness**: The parser should track fenced code blocks to
   avoid misinterpreting headers inside code as structural elements.

## Post-Review Fixes

After local review (csa review --branch main) flagged 3 P1 and 1 P2 issues:

1. **CR-001 (P1)**: Restructured Step 15 (Create PR) to avoid `##` headers
   inside the step body. Replaced the embedded heredoc with a `${PR_BODY}`
   variable reference. Compiled plan now has exactly 24 steps (was 26).

2. **CR-002 (P1)**: Quoted all `${BRANCH}` references in shell push commands
   (`"${BRANCH}"`) to prevent command injection via branch names with
   shell metacharacters.

3. **CR-003 (P1)**: Added explicit NOTE comments to Steps 13 and 16 stating
   that production usage should invoke `/commit` and `/pr-codex-bot` skills
   per AGENTS.md rule 015. The raw commands here demonstrate skill-lang only.

4. **CR-004 (P2)**: Swapped Step 5 (Security Scan) and Step 6 (Stage Changes)
   so staging occurs first, ensuring `git diff --cached` has content to scan.

## Statistics

| Metric | Value |
|--------|-------|
| Source lines (PATTERN.md) | ~190 |
| Compiled steps | 24 |
| Variables extracted | 11 |
| IF blocks | 3 (review issues, bot issues, false positive) |
| FOR blocks | 1 (iterate bot comments) |
| Nested IF-in-FOR | 1 |
| Unique tools referenced | 4 (bash, csa, claude-code, weave) |
| Tiers referenced | 3 (tier-1-quick, tier-2-standard, tier-3-complex) |
