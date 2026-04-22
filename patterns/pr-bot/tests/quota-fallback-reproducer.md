# pr-bot quota fallback reproducer

This reproducer simulates gemini cloud-bot quota exhaustion without burning
real quota.

## Goal

Verify that `patterns/pr-bot/workflow.toml` routes to Step 6a when the latest
`gemini-code-assist[bot]` PR comment indicates quota exhaustion.

## Preconditions

- Work on a disposable test PR.
- `pr_review.cloud_bot=true`
- `pr_review.cloud_bot_name=gemini-code-assist`
- `pr_review.cloud_bot_login=gemini-code-assist[bot]`
- `gh` auth is already working for the repo.

## Simulation Steps

1. Create a temporary `gh` wrapper earlier on `PATH`.
2. Let the wrapper pass through every command except
   `gh api repos/<owner>/<repo>/issues/<PR>/comments?per_page=100`.
3. For that comments endpoint, return JSON whose newest
   `gemini-code-assist[bot]` comment body is one of:
   - `Daily quota limit reached for today.`
   - `Resource exhausted. Try again later.`
   - `Rate limit exceeded, try again later.`
4. Run `csa plan run patterns/pr-bot/workflow.toml` against the test PR.

## Expected Result

- Step 4a or Step 5 logs that the latest gemini comment indicates quota
  exhaustion.
- `CLOUD_BOT_SKIP_KIND=quota_exhausted`
- `MERGE_WITHOUT_BOT_REASON_KIND=cloud_bot_quota_exhausted`
- The workflow does not stop waiting for another bot response.
- The workflow follows the Step 6a merge-without-bot path.

## Control Check

Change the stubbed newest bot comment body to a normal non-quota message, then
rerun the workflow. The Step 5 trigger-and-wait path should execute normally.
