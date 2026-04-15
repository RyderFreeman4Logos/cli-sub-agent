# Fix 791 Summary

- Renamed the consolidated review artifact from `review-consolidated.json` to
  `review-findings-consolidated.json` so the multi-review writer matches the
  reader's expected `review-findings-*` artifact family.
- Updated the bug-class loader path and all affected tests to use the new
  filename consistently.
- Added a regression test proving the consensus writer emits the exact path that
  the bug-class reader consumes.

## Verification

- `rg -n 'review-consolidated\.json' crates/`
- `cargo test -p cli-sub-agent review_consensus`
- `cargo test -p cli-sub-agent bug_class`
- `just pre-commit`
