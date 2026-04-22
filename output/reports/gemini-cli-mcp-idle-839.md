# Gemini-cli 23min idle MCP failure — RECON (#839)

## Session verification

The session artifacts still exist at `/home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/cn-llm-censor-research/sessions/01KPDXJF68J3J7XY4B926GC293/`, so this report is based on direct artifact verification, not only the issue body.

Verified shape:

- `state.toml` shows `phase = "retired"`, `termination_reason = "completed"`, `turn_count = 1`, `last_exit_code = 1`, and tool `gemini-cli`.
- `result.toml` shows `status = "failure"`, `exit_code = 1`, `summary = "MCP issues detected. Run /mcp list for status."`.
- `output/full.md` is 46 bytes and contains only that one-line summary.
- `stdout.log` is 1732 bytes and consists of the SA-mode guard banner plus the final line `MCP issues detected. Run /mcp list for status.`.
- `stderr.log` is 8441 bytes and is dominated by repeated `[csa-heartbeat] tool still running: elapsed=... idle=... idle-timeout=1800s` lines, ending at about `elapsed=1394s`.
- `input/prompt.txt` is 45,740 bytes, consistent with a large multi-question research prompt.

One correction to the issue body: the preserved artifact says `detected`, not `detectd`.

## Root-cause analysis

### What most likely happened

The observed run used CSA's legacy stdout/stderr transport for `gemini-cli`, not ACP event streaming. Current transport routing still defaults `gemini-cli` to `Legacy`, and the heartbeat format in the artifact matches the legacy watchdog string (`tool still running`) rather than the ACP watchdog string (`ACP prompt still running`). That matters because the legacy path only knows whether the subprocess emitted any stdout/stderr bytes; it does not know whether Gemini made a tool call, opened an MCP connection, or produced any semantic progress.

The most likely sequence is:

1. CSA launched `gemini` in legacy mode with a large prompt.
2. Gemini stayed alive but produced no substantive output for ~23 minutes.
3. During that interval, CSA's watchdog emitted its own heartbeat lines every ~15s to stderr.
4. Gemini eventually emitted the line `MCP issues detected. Run /mcp list for status.` to stdout and exited with code `1`.
5. CSA then promoted that last non-empty stdout line into the session summary.

### Was the idle timer firing correctly?

Yes, based on the preserved artifacts and current watchdog code, the idle timer appears to have been armed and counting correctly.

- The heartbeat lines report `idle-timeout=1800s`, which matches the configured threshold.
- The last preserved heartbeat is at about `elapsed=1394s`, well below 1800s.
- The session ended around 1408s after start, so the child exited before CSA's idle kill threshold was reached.

So this was not an "idle timeout failed to kill" case. It was a "tool stayed silent for 23 minutes, then failed on its own before the watchdog deadline" case.

### Why did `[csa-heartbeat] tool still running` keep printing while nothing useful happened?

Because that line is emitted by CSA itself, not by gemini-cli, and it is deliberately non-semantic.

In the legacy process monitor, `maybe_emit_heartbeat()` prints a heartbeat whenever:

- enough wall-clock time has elapsed since last child activity, and
- enough time has elapsed since the previous heartbeat.

It updates `last_heartbeat`, but it does not update `last_activity`. So the heartbeat lines do not keep the child alive; they only prove that CSA's watchdog loop is still ticking.

This is why the session can look "busy" in `stderr.log` while making zero real progress. The heartbeat tells the operator "the wrapper is alive and still waiting", not "the tool is productively working".

### Did gemini-cli emit the MCP message, or did CSA synthesize it?

For this specific session, the evidence points to gemini-cli itself emitting the string, and CSA then reusing it as the summary.

Why this is high confidence:

- The exact string appears in the preserved `stdout.log`.
- `build_summary()` prefers the last non-empty stdout line for non-zero exits before falling back to stderr.
- `output/full.md` contains only that string, matching summary extraction behavior.

Current CSA code does also recognize this literal as a known gemini MCP failure marker, and newer paths may append a synthesized warning summary around it. But the preserved session does not show such augmentation. The artifact shape is more consistent with raw tool output being promoted into the summary.

### What failed between gemini-cli and MCP?

The artifacts prove "Gemini concluded there was an MCP problem" but do not prove which server, when it failed, or whether the failure was:

- startup-time MCP server launch failure,
- delayed disconnect after startup,
- a tool call blocked on an unhealthy server,
- a hub/proxy routing problem, or
- gemini-cli internally deciding MCP was degraded without surfacing per-server detail.

Current CSA code already has a preflight probe that can identify missing MCP server commands from mirrored runtime settings, and a degraded-MCP retry path that can disable unhealthy servers and retry. But none of that diagnostic detail is present in this preserved session. That means either:

- the session predates the richer degraded-MCP instrumentation path,
- the failure mode was not one of the preflight-detectable cases, or
- the diagnostic existed only in transient logs and was not persisted into session artifacts.

## Observability gaps

The missing data that would have materially improved diagnosis:

- MCP server identity at failure time: no record of which server Gemini considered unhealthy.
- MCP failure phase: no distinction between startup refusal, post-start disconnect, timeout during first tool call, or hub connectivity loss.
- Tool-call timeline: no evidence of whether Gemini attempted any tool call at all before failing.
- Progress classification: no separation between "silent reasoning" and "stuck before first useful action".
- Gemini-native diagnostics: CSA preserved only the user-facing one-liner, not the equivalent of `/mcp list` or any structured MCP health dump.
- CPU / process state snapshot near failure: nothing says whether `gemini` was runnable, sleeping on I/O, or effectively wedged.
- MCP hub reachability / ping timestamps: no per-server or per-hub health timeline was captured into the session directory.
- Child stderr/stdout semantic tagging: the legacy path records bytes, but not whether they were banner noise, tool progress, MCP warnings, or final fatal summary.
- Retry history: the preserved session does not show whether any degraded-MCP retry or selective server disablement was attempted.
- Structured failure reason in result metadata: the session has only `summary` and `exit_code`, not a typed reason such as `gemini_mcp_init_failure`, `gemini_mcp_runtime_disconnect`, or `gemini_no_progress_before_tool_call`.

## Proposed improvements (prioritized)

1. Persist structured MCP diagnostics into session artifacts when gemini reports MCP trouble.
Effort: M
Crate/module: `crates/csa-executor/src/transport_gemini_mcp_diagnostic.rs`, `crates/csa-executor/src/transport.rs`, `crates/csa-executor/src/transport_legacy_impl.rs`
Scope: gemini-cli-specific
Proposal: whenever `is_gemini_mcp_issue_result()` matches, persist a small JSON/TOML artifact under the session with `unhealthy_servers`, `probe_errors`, `hub_reachable`, retry/degrade decisions, and whether the failure came from preflight, first attempt, or retry.

2. Add a "no tool calls observed" fast-fail path for legacy gemini sessions.
Effort: M
Crate/module: `crates/csa-process/src/lib.rs`, `crates/csa-process/src/tool_liveness.rs`
Scope: tool-agnostic core with gemini benefit first
Proposal: track a second timer for "no substantive progress signal at all" and fail much earlier than the full idle timeout when there has been no stdout content, no spool growth, no ACP events, and no tool-call-like output markers.

3. Record subprocess CPU/process-state samples into session liveness snapshots.
Effort: M
Crate/module: `crates/csa-process/src/tool_liveness.rs`
Scope: tool-agnostic
Proposal: extend the liveness snapshot with sampled `/proc/<pid>/stat` state plus cumulative CPU time deltas so CSA can distinguish "alive but totally quiescent" from "alive and burning CPU".

4. Persist heartbeat context separately from child stderr.
Effort: S
Crate/module: `crates/csa-process/src/lib_output_helpers.rs`, `crates/csa-process/src/lib.rs`
Scope: tool-agnostic
Proposal: write CSA-generated heartbeats to a dedicated watcher log or structured heartbeat stream instead of mixing them into `stderr.log`, so artifact readers can instantly separate wrapper chatter from child output.

5. Preserve gemini-native MCP status text when available, not just the final one-line summary.
Effort: S
Crate/module: `crates/csa-executor/src/transport_gemini_helpers.rs`, `crates/csa-executor/src/transport_meta.rs`
Scope: gemini-cli-specific
Proposal: when the final output matches the MCP failure marker, retain the preceding N stdout/stderr lines as a focused "diagnostic tail" artifact instead of collapsing to a single summary line.

6. Emit typed failure reasons into `result.toml` / session state.
Effort: M
Crate/module: `crates/csa-executor/src/transport_gemini_helpers.rs`, `crates/csa-session/src/state.rs`
Scope: tool-agnostic schema with gemini-specific classifications
Proposal: store a machine-readable reason such as `gemini_mcp_issue_detected`, `idle_timeout`, `initial_response_timeout`, or `acp_init_failure`, plus optional detail fields.

7. Surface degraded-MCP retry decisions in user-visible summaries.
Effort: S
Crate/module: `crates/csa-executor/src/transport_legacy_impl.rs`, `crates/csa-executor/src/transport.rs`
Scope: gemini-cli-specific
Proposal: if CSA disables unhealthy servers or retries with degraded MCP, append a short structured note to the final summary and persist it in session metadata, so operators can tell whether the run failed before or after remediation.

8. Add session-level capture of "first substantive progress" timestamps.
Effort: M
Crate/module: `crates/csa-process/src/lib.rs`, `crates/csa-acp/src/connection.rs`
Scope: tool-agnostic
Proposal: persist timestamps for process spawn, first byte on stdout, first byte on stderr, first meaningful ACP event, first tool call, and first assistant text. That would make cases like this mechanically classifiable.

## Fast-fail heuristic

### Goal

Distinguish:

- genuinely idle / blocked sessions that have produced no meaningful work signals at all, from
- long silent reasoning sessions that are still making credible progress.

### Inputs

For legacy transport:

- wall-clock since spawn
- wall-clock since last child stdout/stderr byte
- whether any non-banner stdout content has appeared
- output spool growth
- stderr growth excluding CSA-generated heartbeat lines
- subprocess CPU-state samples from `/proc/<pid>/stat`
- optional lightweight MCP preflight result for gemini (`unhealthy_servers`, probe errors)

For ACP transport:

- total ACP event count
- timestamp of first meaningful ACP event
- timestamp of first tool call start/completion
- stderr growth
- subprocess CPU-state samples
- MCP preflight / degraded-retry metadata for gemini

### Heuristic

Declare a session "likely stuck before useful work" when all of the following hold:

1. No meaningful progress has ever been observed.
For legacy: no non-banner stdout payload and no spool growth attributable to assistant content.
For ACP: zero meaningful ACP events and zero tool calls.

2. The process is quiescent.
Example signal: sampled CPU delta stays near zero across at least 2 consecutive samples, and process state is repeatedly `S`/`D` rather than alternating with active work.

3. There is corroborating MCP risk.
Either gemini preflight already found unhealthy servers, or stderr/stdout contains MCP-warning markers, or the gemini runtime has configured MCP servers but none ever become active.

4. A shorter initial no-progress threshold has elapsed.
Suggested starting threshold:
- 90s for gemini legacy sessions with known MCP configuration and zero meaningful output
- 120s for gemini ACP sessions with zero meaningful ACP events
- otherwise fall back to existing initial-response / idle timeout behavior

### Action

On trigger:

- fail fast with a typed reason such as `no_progress_before_mcp_failure`,
- persist the evidence bundle (CPU samples, event counts, stderr tail, MCP preflight),
- include a remediation hint pointing to `csa doctor` and, for gemini, MCP diagnostics.

### Hook points

- Legacy path: `crates/csa-process/src/lib.rs` watchdog tick plus `crates/csa-process/src/tool_liveness.rs` snapshot probing.
- ACP path: `crates/csa-acp/src/connection.rs` prompt loop, using `processed_event_count`, meaningful-event timestamps, and stderr state.
- Gemini-specific enrichment: `crates/csa-executor/src/transport_gemini_mcp_diagnostic.rs`, `crates/csa-executor/src/transport_gemini_helpers.rs`, and the gemini branches in `crates/csa-executor/src/transport.rs` / `transport_legacy_impl.rs`.

## References

- Session artifact location and verified files:
  - `/home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/cn-llm-censor-research/sessions/01KPDXJF68J3J7XY4B926GC293/state.toml`
  - `/home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/cn-llm-censor-research/sessions/01KPDXJF68J3J7XY4B926GC293/result.toml`
  - `/home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/cn-llm-censor-research/sessions/01KPDXJF68J3J7XY4B926GC293/output/full.md`
  - `/home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/cn-llm-censor-research/sessions/01KPDXJF68J3J7XY4B926GC293/stdout.log`
  - `/home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/cn-llm-censor-research/sessions/01KPDXJF68J3J7XY4B926GC293/stderr.log`
  - `/home/obj/.local/state/cli-sub-agent/home/obj/project/github/RyderFreeman4Logos/cn-llm-censor-research/sessions/01KPDXJF68J3J7XY4B926GC293/input/prompt.txt`
- `gemini-cli` default transport routing to Legacy:
  - `crates/csa-executor/src/transport_factory.rs:70-77`
  - `crates/cli-sub-agent/src/review_cmd_tests_tail.rs:765-773`
- Legacy heartbeat emission and semantics:
  - `crates/csa-process/src/lib_output_helpers.rs:218-245`
  - `crates/csa-process/src/lib.rs:490-542`
- Legacy idle-timeout / liveness behavior:
  - `crates/csa-process/src/idle_watchdog.rs:16-60`
  - `crates/csa-process/src/tool_liveness.rs:25-48`
  - `crates/csa-process/src/tool_liveness.rs:67-82`
  - `crates/csa-process/src/tool_liveness.rs:147-160`
- Summary extraction choosing last non-empty stdout line on non-zero exit:
  - `crates/csa-executor/src/transport_meta.rs:523-539`
- Gemini MCP issue recognition and summary augmentation hooks:
  - `crates/csa-executor/src/transport_gemini_helpers.rs:15-18`
  - `crates/csa-executor/src/transport_gemini_helpers.rs:173-201`
  - `crates/csa-executor/src/transport.rs:530-577`
  - `crates/csa-executor/src/transport_legacy_impl.rs:41-127`
- Gemini MCP preflight / degraded-MCP diagnostics:
  - `crates/csa-executor/src/transport_gemini_mcp_diagnostic.rs:13-115`
  - `crates/csa-executor/src/transport_acp_spawn.rs:225-304`
- ACP watchdog contrast and meaningful-event tracking:
  - `crates/csa-acp/src/connection.rs:320-405`
  - `crates/csa-acp/src/connection.rs:598-631`
  - `crates/csa-acp/src/connection.rs:656-757`
  - `crates/csa-acp/src/client.rs:380-394`
- Review-path tool-diagnostic handling for MCP warnings:
  - `crates/cli-sub-agent/src/review_cmd_output_diagnostics.rs:8-36`
  - `crates/cli-sub-agent/src/review_cmd_result.rs:73-89`
- Session-summary noise filtering for heartbeat lines:
  - `crates/cli-sub-agent/src/session_observability.rs:328-349`
