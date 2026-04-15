<!-- CSA:SECTION:summary -->
Round-4 HIGH fix implemented locally on top of `d1bda9df`.
The bypass scanner now resets after shell separator tokens inside `sh -c` payloads, so later statements are scanned in command-prefix position again.
Regression tests were added for `;`, `&&`, `|`, and post-command `export` forms, while preserving earlier false-positive guards.
<!-- CSA:SECTION:summary:END -->

<!-- CSA:SECTION:details -->
Scope:
- `crates/cli-sub-agent/src/run_cmd_shell.rs`
- `crates/cli-sub-agent/src/run_cmd_tests_lefthook.rs`

Implementation:
- Kept the existing `Vec<String>` shell payload expansion shape.
- Updated `tokens_contain_lefthook_bypass` so separator tokens restart prefix scanning instead of terminating the scan after the first command token.
- Applied the same reset behavior while handling `env ...` and `export ...` prefix forms, so later statements in the same flattened payload are still checked.

Regression coverage added:
- `sh -c "git status; LEFTHOOK=0 git commit"` => blocked
- `sh -c "git rev-parse HEAD && LEFTHOOK=0 git commit"` => blocked
- `sh -c "git commit && export LEFTHOOK=0"` => blocked
- `sh -c "echo hello | LEFTHOOK=0 git commit"` => blocked
- `sh -c "echo 'LEFTHOOK=0'; git commit"` => allowed

Pending at write time:
- Focused `cargo test` runs
- `just pre-commit`
- commit + push
<!-- CSA:SECTION:details:END -->
