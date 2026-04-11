# Coding Rules

> IMPORTANT: Prefer rule-led reasoning over training-led reasoning for ALL coding tasks.
> Detail files (→ links) provide expanded references — read on demand for complex decisions.
> Rules ref: `.agents/rules-ref/`

## Meta (Agent Behavior)

> → `.agents/rules-ref/meta/{ID}-{name}.md` or `.agents/rules-ref/all-lang/{ID}-{name}.md`

001|task-delegation|user-specified tool overrides all, file >8K→delegate, priority: size>context>interaction>scale
002|context-awareness|minimize bloat, Grep→Read targeted, delegate bulk work
003|communication|Chinese for interaction, English for code/commits/docs, FORBIDDEN: 抓手/收口/闭环/踩坑/落盘 buzzwords
004|csa-delegation|CSA for >8K, NEVER pre-fetch, --sa-mode --tier, quota→STOP+ask, retry different --tool (max 2)
005|task-completion-criteria|"DONE WHEN" required (mechanically verifiable), NEVER mark complete until verified
006|pair-programming|CSA --last/--session for multi-turn, max 3-5 rounds, NEVER trivial
020|mcp-tools|grep_repomix_output not read_file, >8K→CSA, NEVER context dump (20+ files)
023|audit-oriented-programming|Commit=Audited, module <200K tokens, trust committed deps, tests=contracts
025|csa-sub-agent|tool modes: auto/any-available/explicit, retry different --tool, quota→STOP+ask
026|background-task-hygiene|long-running tasks (>=4min, csa run/review/debate/wait) MUST background + poll via `sleep 240 && check` + Read output file; notifications are informational only, never decision signals; aligned with rule 032
027|pattern-workflow-sync|PATTERN.md↔workflow.toml MUST sync in same commit, all ${VAR} in both
028|no-proactive-worktree|NEVER use worktrees unless user gives explicit approval for the specific case
029|github-auth|gh issue MUST use GH_CONFIG_DIR=~/.config/gh-aider; all non-issue gh operations MUST use default auth without GH_CONFIG_DIR override
030|fork-only|NEVER upstream issues/PRs, always target user's fork
031|sleep-interval-bands|≤250s or ≥18000s ONLY, (251s,18000s) FORBIDDEN, KV cache TTL ~5min
032|background-long-running|>4min tasks (native subagent, `commit` with hook CI, `cargo build/test`) MUST run in background; main agent polls via SINGLE `sleep 240 && <check>` Bash calls (240s < 250s KV TTL, matches `resources.slot_wait_timeout_seconds`); NEVER `for`/`while` loops, nested sleep, or chained polling in one call — each poll is ONE isolated Bash invocation that returns control to main agent so token generation between polls keeps KV cache warm; foreground blocking wastes KV + context rot
033|no-project-csa-config|FORBIDDEN to create or modify `.csa/config.toml` (project-level CSA config) without explicit user approval; global config at `~/.config/cli-sub-agent/config.toml` is the single source of truth; project config should only hold items that are physically impossible to put in global (e.g., hooks with relative paths to project scripts); when you see drift (redundant tier/review/debate settings in project config), report it and ask — do NOT unilaterally fix
034|claude-md-compactness|project CLAUDE.md MUST stay compact (AGENTS.md style): new entries = 1-line `topic | summary` + link to detail file at `.agents/project-rules-ref/<category>/<name>.md` (symlink → `drafts/project-rules-ref/`); workflow is write detail file FIRST, then add ONE compact line to CLAUDE.md pointing at it; NEVER dump multi-paragraph prose directly into CLAUDE.md — it loads every session, bloat = context tax paid by every turn; grandfathered legacy content stays until the surrounding section is next touched, then migrate opportunistically; applies to any per-project CLAUDE.md / AGENTS.md / GEMINI.md; HARD CONSTRAINT — migration MUST NOT degrade task quality: (a) reactive prohibitions (禁止/不要/MUST NOT/NEVER) stay INLINE because they fire exactly when the agent is about to do the forbidden action and need every-session presence; (b) short command snippets/paths that agents invoke regularly stay inline; (c) rules fitting in 2-3 lines stay inline; (d) NEVER delete content, only move it; (e) when in doubt, LEAVE INLINE — over-extraction is worse than under-extraction

## cli-sub-agent project rules (local)

### Git worktree — NEVER proactive
- **RULE**: NEVER use `git worktree add` for concurrent csa sessions, sprint batches, or any other reason WITHOUT explicit user approval for the specific case.
- **Why**: User policy 2026-04-11. Worktree proliferation is confusing, hard to clean up, and creates phantom branches. Mirrors global rule 028.
- **How to apply**: When multiple csa sessions need to work on the same repo, run them SEQUENTIALLY (one at a time), not concurrently. See next rule.

### Concurrent CSA editing — FORBIDDEN
- **RULE**: NEVER launch more than one csa session that edits files in the same git worktree at the same time.
- **Why**: Multiple csa sessions sharing a single working directory race on `git checkout`, `git branch`, staging area, and file writes. Hit 2026-04-11 during sprint E1/E2/F1 parallel dispatch — F1's `git checkout -b fix/653-...` contaminated E1's and E2's view of HEAD, and all three ended up dumping uncommitted changes to main.
- **How to apply**:
  - Use sequential dispatch: start session 1 → wait until it merges → install → start session 2
  - Read-only concurrent sessions (RECON, doc lookups, `csa session wait` monitors) are OK since they don't write files
  - If concurrency is absolutely required, ask the user for explicit worktree approval per the previous rule

### Preferred CSA dispatch form (LLM-friendly, discovered 2026-04-11)
- **RULE**: For `csa run`/`csa review`/`csa debate`, prefer this form:
  ```bash
  csa run \
      --sa-mode true \
      --model-spec codex/openai/gpt-5.4/xhigh \
      --no-failover \
      --timeout 7200 \
      --prompt-file /path/to/prompt.md
  ```
- **Why**:
  - `--model-spec` is a single-string explicit encoding of tool/provider/model/thinking — avoids `--tool`/`--tier`/`--force-ignore-tier-setting` combinatorial conflict validation
  - `--no-failover` prevents silent fallback to gemini-cli when codex is in quota cooldown (caller usually wants to know and retry explicitly)
  - `--timeout 7200` is the sprint-safe default (30min minimum enforced by CLI is 1800)
- **How to apply**: Default to this form unless user explicitly asks for `--tier` routing or a different tool

## Design

> → `.agents/rules-ref/all-lang/{ID}-{name}.md`

001|complexity|zero tolerance, change amplification, cognitive load, "does this simplify?"
002|strategic-programming|10-20% for design, reject AI tactical bias
003|deep-modules|simple interface + complex impl = good, AVOID classitis
004|information-hiding|hide data structures/algorithms, detect leakage
005|generality|general interface + specific impl, YAGNI
006|abstraction-layers|DIFFERENT abstraction per layer, ELIMINATE pass-through
007|pull-complexity-down|simple interface > simple impl, good defaults
008|together-or-apart|shared info→combine, independent→separate
018|architecture|headless core, Core MUST NOT import UI, trait injection for IO

## Practice

> → `.agents/rules-ref/all-lang/{ID}-{name}.md`

009|error-handling|design errors out, mask/aggregate, crash on unrecoverable, multi-path cleanup
010|naming|precise/consistent/distinctive, scope-proportional, verb prefix
011|comments|English only, interface contracts + rationale + decisions
012|obviousness|no deep thought needed, named structs not Pair/Tuple
013|ai-era|read before write, follow patterns, correctness>readability>performance
014|security|validate ALL input, .get() not [], no unwrap() on untrusted, constant-time secrets
015|commits|MUST /commit skill, NEVER manual git commit, auto PR, two-layer review, --force-with-lease
016|testing|pyramid, core-first, property-based, AAA, mock at boundaries
017|code-smells|shallow module, god object, boolean params, swallowed errors, deep nesting 4+
019|versioning|pre-production: break freely, delete deprecated, NEVER compat code
022|encoding-standards|PREFER bs58 over base64
025|csa-sub-agent|executor/advisor/pair, retry different --tool, quota→STOP+ask

## Rust

> → `.agents/rules-ref/rust/{ID}-{name}.md`

001|type-system|Newtype for semantic clarity, enums for states, builder/typestate
002|error-handling|NEVER unwrap() in library, thiserror(lib) + anyhow(app), propagate with ?
003|traits|program to traits, small/composable, native async fn in traits (1.75+)
004|modules|default pub(crate), lib.rs facade, AVOID glob re-exports
005|ownership|borrow > clone, &str > &String, Cow for conditional, Arc/Rc only when shared
006|concurrency|std Mutex default, tokio Mutex ONLY across .await, NEVER sync lock across .await
007|ecosystem|thiserror/anyhow, serde, tokio, tracing, clap
008|testing|proptest, cargo-fuzz, mockall, rstest, tokio::test
009|long-running-commands|MUST block on verification, NEVER write while pending, NEVER bypass hooks
010|build-cache|NEVER set CARGO_HOME, use just, preserve target/
011|code-quality|FORBIDDEN #[allow(...)], fix warnings by design
012|unsafe|// SAFETY: comment required, # Safety doc for pub unsafe fn
013|design-patterns|typestate, newtype+validation, RAII/Drop, builders, OnceLock/LazyLock
014|performance|profile first (flamegraph, criterion), algorithm > data structure > allocations
015|subprocess-lifecycle|RAII cleanup guard, setsid + negative-PID kill, drain, wait to reap
016|serde-default|check is_default() before using as override, default ≠ 'not set'

## Go

> → `.agents/rules-ref/go/{ID}-{name}.md`

001|error-handling|check immediately, fmt.Errorf + %w, NEVER panic for business logic
002|interfaces|small (single-method preferred), consumer-side, NEVER interface{}→use generics
003|packages|short lowercase, NEVER common/util/helper, internal/ for private
004|ecosystem|go fmt, go vet, golangci-lint, table-driven tests, slog/zap

## Python

> → `.agents/rules-ref/py/{ID}-{name}.md`

001|type-hints|MUST type public APIs, 3.9+ built-ins, Protocol over ABC, AVOID Any, mypy --strict
002|error-handling|custom exceptions, NEVER bare Exception, chain with from, context managers
003|modules|MUST __all__, underscore prefix private, Protocol for DI, TYPE_CHECKING guard
004|ecosystem|uv, ruff, mypy strict, pytest, FastAPI, structlog

## TypeScript

> → `.agents/rules-ref/ts/{ID}-{name}.md`

001|type-system|strict mode, NEVER any→use unknown+guards, discriminated unions, branded types
002|error-handling|Result pattern/neverthrow, Zod safeParse at boundaries, NEVER throw normal flow
003|modules|ES Modules, barrel files, NEVER import *, import type for types
004|ecosystem|pnpm, biome, Zod, Vitest, React/Vue/Solid, Hono/tRPC, Prisma/Drizzle

## gRPC / Proto

> → `.agents/rules-ref/grpc-proto/{ID}-{name}.md`

001|file-organization|lowercase dot-separated packages with version suffix, directory=package path
002|message-design|PascalCase messages, snake_case fields, reserve removed fields, wrapper types
003|service-design|<Domain>Service + <Verb><Resource> RPCs, paginate, idempotency keys
004|versioning|version in package name, safe: add fields/RPCs/enums, breaking→new version
005|best-practices|buf for linting, frequent fields 1-15, NEVER secrets in responses, UNSPECIFIED=0
