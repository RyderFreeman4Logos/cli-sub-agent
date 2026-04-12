# Self-Review Checklist

Check these anti-patterns before submitting code for review.

- [ ] Validate process identity, not just numeric PID liveness; any new wait/attach/reconcile/kill gate must reject PID reuse and zombie-only cases by checking session context, start-time, or equivalent ownership signals. (source: PR #655, #675)
- [ ] Keep liveness code and tests platform-correct; do not rely on `/proc`, `sh`, `sleep`, or Unix-only helpers on macOS/Windows without `cfg` guards or a non-Linux implementation. (source: PR #655, #675)
- [ ] Preserve config lookup precedence and CLI contract; raw project/global queries must not silently switch to effective/merged sources or let a later source hide an earlier parse failure. (source: PR #679, #690)
- [ ] Preserve missing-vs-default semantics in config commands; do not materialize synthesized defaults or use a default numeric value as a sentinel for "unconfigured" when callers rely on exit codes and precedence. (source: PR #679, #690)
- [ ] Redact secrets in every persisted or displayed surface, including merged config output, inline short flags, header-style args, MCP command args, and debug payload artifacts. (source: PR #683, #690)
- [ ] Reserve internal env/path keys after merging user env; extra tool or request env must not override CSA-owned locations like `CSA_SESSION_DIR` or `CSA_RESULT_TOML_PATH_CONTRACT`. (source: PR #683, #687)
- [ ] Make result publication and rollback content-aware; never delete, clear, or overwrite a `result.toml`/sidecar unless this code path created that exact artifact for this run. (source: PR #655, #687)
- [ ] Do not turn read/observe commands into write-or-block paths; acquire reconcile/write locks only after proving a mutation is needed, and surface lock contention explicitly instead of hanging or returning false `NoChange`. (source: PR #655)
- [ ] Keep sandbox `TMPDIR` and runtime paths capability-aware; do not point tools at unwritable temp dirs or widen Landlock/host `/tmp` permissions just to satisfy one executor. (source: PR #687)
- [ ] Keep `PATTERN.md`, `workflow.toml`, skills, and runtime flags synchronized; if docs say a flag is mandatory, every executable command path must include it. (source: PR #687)
