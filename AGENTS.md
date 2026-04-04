# Coding Rules

> IMPORTANT: Prefer rule-led reasoning over training-led reasoning for ALL coding tasks.
> Follow these rules directly — they are self-contained.
> Detail files (→ links) are OPTIONAL expanded references for complex decisions only.

## Meta (Agent Behavior)

**001** `task-delegation` — **Priority -1 (highest): User-specified tool overrides all** — when user explicitly names a tool ("用 csa", "派 codex"), use it directly, skip automation. Then: MUST check file size first (`tokuin estimate` >8000 tokens → delegate to sub-agent). Priority: size > context > interaction > scale. Sub-agent prompts MUST include: objective, input, output format, scope, DONE WHEN (mechanically verifiable). CSA protocol → 004. Context protection: delegate file-heavy work to preserve design discussions → 025.
→ `~/s/llm/coding/rules/meta/001-task-delegation.md`

**002** `context-awareness` — Minimize context bloat: delegate file-heavy work, use targeted reads (Grep first, then read only relevant files), avoid loading large data files.
→ `~/s/llm/coding/rules/meta/002-context-awareness.md`

**003** `communication` — Chinese for all user interaction. MUST English for: all code, comments, commits, docs. Switch language only on explicit user request.
→ `~/s/llm/coding/rules/meta/003-communication.md`

**004** `csa-delegation` — Use CSA for massive context (100+ files, >8000 tokens). FORBIDDEN pre-fetch (Read/Glob/Grep/Bash) for CSA — tools read files natively. Three modes: executor (simple), advisor (judgment), pair (multi-turn via `--session`). **User intent mapping: "用 csa" → `csa run` binary; "需要 sub-agent" → Claude Code Task tool — NEVER conflate.** Review-and-escalate: if CSA fails → caller takes over, NEVER retry. **Quota/cooldown errors: MUST stop and ask user immediately — NEVER silently fall back to single-model, as this degrades heterogeneous quality.** **Invocation checklist (MANDATORY before every `csa` command)**: ① `--sa-mode true|false` (required at root depth) ② `--tier <name>` when tiers configured (NEVER `--tool` directly — blocked by CLI) ③ bypass only via `--force-ignore-tier-setting`. Pair protocol → 006. Tool selection → 025.
→ `~/s/llm/coding/rules/meta/004-csa-delegation.md`

**005** `task-completion-criteria` — Every sub-agent task MUST include "DONE WHEN" with mechanically verifiable condition (exit code 0, empty output, file exists). NEVER mark complete until condition passes. Run verification command and report output.
→ `~/s/llm/coding/rules/meta/005-task-completion-criteria.md`

**006** `pair-programming` — Third CSA mode alongside advisor/executor. Use `--last` or `--session <ULID>` for multi-turn iteration. Trigger: complex design, disputed review, multi-step refactoring. Turn structure: propose → critique → refine → validate. Max rounds: 3-5. Convergence: both agree or caller decides after max rounds. NEVER pair for trivial tasks.
→ `~/s/llm/coding/rules/meta/006-pair-programming.md`

**020** `mcp-tools` — Minimize context pollution via strategic MCP tool selection. Search: `grep_repomix_output` not `read_file`. Large scope: `pack_codebase`. Analysis >8000 tokens: CSA (`csa run`). Memory: funnel filtering (search→timeline→get_observations, NEVER broad fetch). NEVER context dump (reading 20+ files).
→ `~/s/llm/coding/rules/all-lang/020-mcp-tools.md`

**023** `audit-oriented-programming` — Commit = Audited. Non-recursive audit: trust committed dependencies, only verify current module's API usage. Tests = contracts: auditor verifies sufficient tests AND implementation fulfills promises. Module MUST fit auditor sub-agent context (<200K — distinct from 001's 8000-token delegation threshold). Measure: `tokuin estimate`. Pre-commit audit mandatory.
→ `~/s/llm/coding/rules/all-lang/023-audit-oriented-programming.md`

**026** `background-task-hygiene` — In long-running autonomous modes (sa/goo), `<task-notification>` is delayed until assistant turn ends — can be 30+ minutes. MUST use foreground `Bash(timeout:600000)` for critical-path commands (csa run/review/debate). NEVER `run_in_background` unless fire-and-forget with local fallback. All `<task-notification>` are informational-only — NEVER use as decision input if `TaskOutput` was already used. CSA `SessionComplete` hook writes breadcrumb to `{session_dir}/.complete` for context recovery after compaction.
→ `~/s/llm/coding/rules/meta/026-background-task-hygiene.md`

**027** `pattern-workflow-sync` — PATTERN.md and workflow.toml MUST stay synchronized. Change one → update the other in same commit. All `${VARIABLE}` references must exist in both files. Reviewer MUST verify 1:1 step correspondence. Anti-pattern: orphaned variables causing logic errors.
→ `~/s/llm/coding/rules/meta/027-pattern-workflow-sync.md`

**028** `no-proactive-worktree` — NEVER proactively create git worktrees. Only use `git worktree add` when the user **explicitly requests** worktree-based development. Worktrees introduce submodule URL override complexity (Libre-Vectis→fork), mise trust issues, and divergent build caches. The default workflow uses the main checkout directly.

**029** `hook-bypass-prevention` — **ABSOLUTE PROHIBITION on ALL hook bypass methods.** FORBIDDEN: `--no-verify`/`-n` on `git commit`/`git push`; `LEFTHOOK=0` env var; `LEFTHOOK_SKIP` env var; `export LEFTHOOK=0` before git commands; `env LEFTHOOK=0 git commit`; modifying `.git/hooks/*` files; setting `core.hooksPath` to bypass; any mechanism that prevents Lefthook/pre-commit hooks from running. **When `just pre-commit` fails**: (1) Code quality issues (clippy, fmt, test failures) → FIX the code, do NOT bypass. (2) Environment/sandbox limitations (permission denied, missing tool) → report `status="needs_clarification"` with exact error, do NOT bypass. (3) Pre-existing failures from unrelated workspace crates → report as blocker with exact error output, do NOT bypass. NEVER treat pre-existing failures as justification for `LEFTHOOK=0`. The correct response to ANY hook failure is to fix or report — NEVER to disable the hook.

## All Languages — Design

**001** `complexity` — Zero tolerance. Three symptoms: change amplification, cognitive load, unknown unknowns. Two root causes: dependencies, obscurity. Complexity accumulates incrementally — appears benign until irreversible. Every decision must answer: "Does this simplify?"
→ `~/s/llm/coding/rules/all-lang/001-complexity.md`

**002** `strategic-programming` — Allocate 10-20% time for design improvements. Small continuous investments > deferred refactoring. MUST reject AI's tactical bias ("make it work" vs "make it right"). Tactical tornado: high output + destroyed codebase.
→ `~/s/llm/coding/rules/all-lang/002-strategic-programming.md`

**003** `deep-modules` — Simple interface + complex implementation = deep module (good). Complex interface + simple implementation = shallow module (bad). AVOID classitis (many shallow modules). Module value = complexity it hides. Optimize interface for common cases, provide sensible defaults.
→ `~/s/llm/coding/rules/all-lang/003-deep-modules.md`

**004** `information-hiding` — MUST hide: data structures, algorithms, optimizations inside modules. Detect leakage: shared knowledge between modules, exposed internal structure, temporal decomposition. Provide specific access methods, not raw data structures.
→ `~/s/llm/coding/rules/all-lang/004-information-hiding.md`

**005** `generality` — General interface + specific implementation = deep module. Push specifics up (to caller) or down (to implementation). AVOID over-generalization (YAGNI). Three questions: meets future needs? convenient to use? simple implementation?
→ `~/s/llm/coding/rules/all-lang/005-generality.md`

**006** `abstraction-layers` — Each layer MUST provide DIFFERENT abstraction from adjacent layers. ELIMINATE pass-through methods (forwarding calls with no added value) and pass-through variables (threaded through unused). Use context objects or dependency injection instead.
→ `~/s/llm/coding/rules/all-lang/006-abstraction-layers.md`

**007** `pull-complexity-down` — Simple interface > simple implementation. 1 developer handles complexity so N users avoid it. MUST provide good defaults. Configuration SHOULD be optional. Expose advanced features only when genuinely needed.
→ `~/s/llm/coding/rules/all-lang/007-pull-complexity-down.md`

**008** `together-or-apart` — Shared info → combine. Interface simplification → combine. Eliminate true duplication → combine. Independent change → separate. General vs specific → separate layers. Goal: minimize overall complexity.
→ `~/s/llm/coding/rules/all-lang/008-together-or-apart.md`

**018** `architecture` — Headless core pattern. Core MUST NOT import UI/framework. Core: pure functions, domain types, business rules (no IO). Side effects via trait injection (TimeProvider, Storage). Structure: `core/` (pure) + `adapters/` (infra) + `ui-*/` (presentation). 80%+ logic testable via fast unit tests.
→ `~/s/llm/coding/rules/all-lang/018-architecture.md`

## All Languages — Practice

**009** `error-handling` — Design errors out of existence (change semantics: delete non-existent file = no-op). Mask at lower level (retry transparently). Aggregate low-level into high-level errors. Crash on unrecoverable/programmer errors. Handle at level with enough context to decide correctly. Multi-path completeness: enumerate ALL paths, verify resource cleanup per path. Optimization degradation: MUST degrade silently to non-optimized path, NEVER propagate optimization errors.
→ `~/s/llm/coding/rules/all-lang/009-error-handling.md`

**010** `naming` — Precise, consistent, distinctive. Scope-proportional length. Functions: verb prefix, describe what not how. AVOID: vague (`data`/`info`/`manager`), redundant (`nameString`), abbreviations (`usrMgr`), numeric suffixes. Rename immediately when inaccurate.
→ `~/s/llm/coding/rules/all-lang/010-naming.md`

**011** `comments` — ALL comments MUST be in English. Four types: interface contracts (params/returns/throws), implementation rationale (why this approach), cross-module relationships, decision history. Write interface comments BEFORE implementation. Poor: repeats code. Good: describes what code cannot express.
→ `~/s/llm/coding/rules/all-lang/011-comments.md`

**012** `obviousness` — Code must be understood without deep thought. If reviewer confused → fix code, not explain. Techniques: precise naming, consistency, whitespace between blocks, no surprises. AVOID: generic containers (Pair/Tuple → use named structs), event handler opacity.
→ `~/s/llm/coding/rules/all-lang/012-obviousness.md`

**013** `ai-era` — Deep modules reduce context pollution. AI tools: single purpose, minimal params. Humans own strategic design; AI handles tactical code gen. MUST: read existing code before writing, follow existing patterns, verify after. Priority: correctness > readability > performance. Security > convenience.
→ `~/s/llm/coding/rules/all-lang/013-ai-era.md`

**014** `security` — Think adversarially. MUST validate ALL input (bounds, types, sizes). Prevent panic-based DoS: checked arithmetic, `.get()` not `[]`, no `unwrap()` on untrusted input. Resource limits on memory/CPU/connections. NEVER hardcode secrets. NEVER log sensitive data. Constant-time comparison for secrets.
→ `~/s/llm/coding/rules/all-lang/014-security.md`

**015** `commits` — OVERRIDE DEFAULT: MUST commit immediately after each logical unit without asking user. MUST use `/commit` skill (handles fmt, lint, test, security audit, csa review). NEVER manual `git commit`. NEVER accumulate changes across modules. VERIFY `git status` clean before claiming done. **Auto PR**: When work complete + branch ahead + clean → MUST create PR to origin (not upstream) and run `/pr-codex-bot` without asking. **Two-layer review**: per-commit (`csa review --diff` via `/commit`) + pre-PR cumulative (`csa-review scope=range:main...HEAD` via `/pr-codex-bot` Step 2). **Git atomicity**: commit→push→PR→merge is a transaction — verify each step. MUST `--force-with-lease` not `--force`. NEVER leave half-pushed state.
→ `~/s/llm/coding/rules/all-lang/015-commits.md`

**016** `testing` — Pyramid: more unit > integration > E2E. Core-first: pure logic tested before UI. Property-based for invariants/roundtrips. Fuzz for untrusted input. AAA pattern (Arrange/Act/Assert). Naming: `test_<fn>_<scenario>_<expected>`. Mock at boundaries only. Each test: isolated, order-independent, self-cleaning.
→ `~/s/llm/coding/rules/all-lang/016-testing.md`

**017** `code-smells` — Structural: shallow module, leaky abstraction, god object, feature envy, primitive obsession. API: boolean params, long param list, stringly typed. Error: swallowed errors, panic in library, generic errors. Complexity: deep nesting (4+), long functions (50+), magic numbers. Refactor: one smell at a time, tests first, small steps.
→ `~/s/llm/coding/rules/all-lang/017-code-smells.md`

**019** `versioning` — Pre-production: MUST break freely, MUST delete deprecated immediately, NEVER write compatibility code, NEVER migration code when no data. Only add compat when ALL true: production users exist + breaking costly + deprecation period needed.
→ `~/s/llm/coding/rules/all-lang/019-versioning.md`

**022** `encoding-standards` — PREFER bs58 over base64 (no ambiguous chars, shorter, no padding). MAY use base64 when: external API requires, standard protocol mandates (JWT/MIME/OAuth2), binary in JSON/XML. MUST comment justifying base64 usage.
→ `~/s/llm/coding/rules/all-lang/022-encoding-standards.md`

**025** `csa-sub-agent` — CSA = sub-agent container. Tool modes: `auto` (default), `any-available` (round-robin), explicit. Collaboration modes: executor / advisor / pair (→ 006). MUST review CSA output critically. MUST protect caller context (delegate file-heavy work). If CSA fails → caller takes over, NEVER retry. **Quota/cooldown: MUST stop and ask user — NEVER silently degrade to single-model.**
→ `~/s/llm/coding/rules/all-lang/025-csa-sub-agent.md`

## Rust

**001** `type-system` — MUST use Newtype for semantic clarity (UserId, PostId). MUST use enums for mutually exclusive states instead of booleans. Encode invariants in type system with builder/typestate patterns. Prefer compile-time checks over runtime.
→ `~/s/llm/coding/rules/rust/001-type-system.md`

**002** `error-handling` — NEVER `unwrap()` or `expect()` in library code. MUST use `thiserror` for libraries, `anyhow` for apps, `color-eyre` for top-level. Propagate with `?`. Panics only in tests, prototypes, truly unrecoverable.
→ `~/s/llm/coding/rules/rust/002-error-handling.md`

**003** `traits` — MUST program to traits, not concrete types. Split large traits into small, focused, composable ones. Native `async fn` in traits (Rust 1.75+); `async-trait` only for `dyn Trait`. Make traits object-safe when appropriate.
→ `~/s/llm/coding/rules/rust/003-traits.md`

**004** `modules` — MUST default to `pub(crate)`. `lib.rs` as facade re-exporting only public API. AVOID glob re-exports. Hide implementation details. Prelude pattern for core types.
→ `~/s/llm/coding/rules/rust/004-modules.md`

**005** `ownership` — MUST prefer borrowing over cloning. Accept `&str` over `&String`. Move ownership for long-term responsibility. Hide lifetimes in public APIs using owned values. `Cow<T>` for conditional cloning, `Arc`/`Rc` only when sharing necessary.
→ `~/s/llm/coding/rules/rust/005-ownership.md`

**006** `concurrency` — Default `std::sync::Mutex` (futex-based since 1.62). `parking_lot::Mutex` when fairness/anti-starvation critical. `tokio::sync::Mutex` ONLY for holding across `.await`. `DashMap` for concurrent maps. Channels: `tokio::sync::mpsc`, `crossbeam-channel`, `flume`. NEVER hold sync locks across `.await`. Prefer lock-free when possible.
→ `~/s/llm/coding/rules/rust/006-concurrency.md`

**007** `ecosystem` — MUST: `thiserror`/`anyhow` (errors), `serde` (serialization), `tokio` (async), `tracing` (logging), `clap` (CLI). Check library health before adding deps.
→ `~/s/llm/coding/rules/rust/007-ecosystem.md`

**008** `testing` — MUST: `proptest` (property-based), `cargo-fuzz` (fuzzing), `mockall` (mocking), `rstest` (parameterized), `tokio::test` (async). Structure: `src/` unit, `tests/` integration, `fuzz/` fuzz targets.
→ `~/s/llm/coding/rules/rust/008-testing.md`

**009** `long-running-commands` — Verification commands (`cargo clippy`, `cargo test`, `just pre-commit`) MUST block: run in background for token efficiency but MUST wait for result before writing any code. NEVER write/edit files while verification is pending. NEVER bypass pre-commit hooks by ANY method — see Meta 029 `hook-bypass-prevention` for comprehensive list. Read-only research may proceed in parallel.
→ `~/s/llm/coding/rules/rust/009-long-running-commands.md`

**010** `build-cache` — FORBIDDEN: NEVER set `CARGO_HOME` (destroys cache). MUST use `just` commands. Preserve `target/`. MUST NOT disable incremental compilation or override `RUSTC_WRAPPER` if sccache configured.
→ `~/s/llm/coding/rules/rust/010-build-cache.md`

**011** `code-quality` — FORBIDDEN: `#[allow(...)]` in production. Fix warnings by refactoring/better types/design. Enable `-D warnings` in `.cargo/config.toml`. Only exceptions: generated code, FFI boundaries, with justification.
→ `~/s/llm/coding/rules/rust/011-code-quality.md`

**012** `unsafe` — MUST document every `unsafe` with `// SAFETY:` comment. Public `unsafe fn` needs `# Safety` doc. `CString`/`CStr` for FFI strings. `catch_unwind` at FFI boundaries. `#[repr(C)]` for FFI types. NEVER cast `*const` to `*mut` and write.
→ `~/s/llm/coding/rules/rust/012-unsafe.md`

**013** `design-patterns` — Typestate for state machines. Newtype with validation for domain types. Smart pointers: `Box` (single owner), `Rc`/`Arc` (shared), `RefCell`/`Mutex` (interior mutability). RAII/Drop for cleanup. Builders for complex construction. `OnceLock`/`LazyLock` for lazy init.
→ `~/s/llm/coding/rules/rust/013-design-patterns.md`

**014** `performance` — MUST profile before optimizing (`flamegraph`, `heaptrack`, `criterion`). Priority: algorithm > data structure > allocations > cache locality > SIMD. Pre-allocate known-size collections. Iterators over index loops. NEVER `LinkedList` without proof.
→ `~/s/llm/coding/rules/rust/014-performance.md`

**015** `subprocess-lifecycle` — Every `Command::spawn()` MUST have RAII cleanup guard (kill + wait on Drop). Timeout MUST kill entire process group (`setsid` + negative-PID kill). PID liveness: cross-verify 2+ signals. After kill: close stdin, drain stdout/stderr, `wait()` to reap. NEVER leave zombies.
→ `~/s/llm/coding/rules/rust/015-subprocess-lifecycle.md`

**016** `serde-default` — When a struct uses `#[serde(default)]`, every code path loading this struct MUST check `is_default()` or equivalent before using it as an override. Default values are indistinguishable from 'not set' — using them as overrides silently masks higher-priority config sources (e.g., global config masked by project default). MUST implement `is_default()` method on any config struct with serde(default). Discovered via PR #264 R10 review.
→ (no detail file yet)

## Go

**001** `error-handling` — MUST check errors immediately. Wrap with `fmt.Errorf` + `%w`. NEVER ignore without explicit `_`. NEVER `panic()` for business logic. `errors.Is()` for sentinels, `errors.As()` for types. `defer` for cleanup.
→ `~/s/llm/coding/rules/go/001-error-handling.md`

**002** `interfaces` — Small interfaces (single-method preferred), defined at consumer side. MUST NOT use `interface{}` — prefer generics (Go 1.18+). Comma-ok for type assertions. Return concrete types, accept interfaces.
→ `~/s/llm/coding/rules/go/002-interfaces.md`

**003** `packages` — Short, lowercase names. NEVER `common`/`util`/`helper`. `internal/` for private, `pkg/` for exportable. Import groups: stdlib → third-party → project. No dot imports. No circular imports.
→ `~/s/llm/coding/rules/go/003-packages.md`

**004** `ecosystem` — MUST run `go fmt`, `go vet`, `golangci-lint run` before commit. `go mod tidy` for deps. Table-driven tests. `slog` (1.21+) or `zap` for logging.
→ `~/s/llm/coding/rules/go/004-ecosystem.md`

## Python

**001** `type-hints` — MUST type all public APIs. Python 3.9+ built-ins (`list[str]`) not `typing.List`. `Protocol` over ABC. AVOID `Any`. Strict: `mypy --strict` or `pyright strict`.
→ `~/s/llm/coding/rules/py/001-type-hints.md`

**002** `error-handling` — Custom exception hierarchies. NEVER catch bare `Exception`. Chain with `from`. Context managers for cleanup. `Optional` for expected absence. `logger.exception()` for traces. Document raises.
→ `~/s/llm/coding/rules/py/002-error-handling.md`

**003** `modules` — MUST `__all__` in `__init__.py`. Underscore prefix for private. AVOID double underscore mangling. `Protocol` for DI. `TYPE_CHECKING` guard for circular imports. Flat packages unless justified.
→ `~/s/llm/coding/rules/py/003-modules.md`

**004** `ecosystem` — MUST: `uv` (project mgmt), `ruff` (format+lint), `mypy` (strict), `pytest` (testing). `FastAPI` for APIs, `structlog` for logging. CI: `uv run ruff check . && uv run mypy src && uv run pytest`.
→ `~/s/llm/coding/rules/py/004-ecosystem.md`

## TypeScript

**001** `type-system` — MUST strict mode (`strict`, `noImplicitAny`, `strictNullChecks`, `noUncheckedIndexedAccess`). NEVER `any` — use `unknown` + guards. Discriminated unions. `interface` for shapes, `type` for unions. Branded types for ID safety.
→ `~/s/llm/coding/rules/ts/001-type-system.md`

**002** `error-handling` — MUST Result pattern or `neverthrow`. Discriminated error unions for exhaustive matching. Zod `safeParse()` at boundaries. NEVER throw in normal flow. `Result<T, E>` for explicit errors.
→ `~/s/llm/coding/rules/ts/002-error-handling.md`

**003** `modules` — ES Modules. Barrel files for imports. NEVER `import * as`. Underscore prefix for internal. `import type` for types. Extract shared types to third module for circular deps.
→ `~/s/llm/coding/rules/ts/003-modules.md`

**004** `ecosystem` — MUST: `pnpm`, `biome` (lint+format), TS `ES2022`/`ESNext`/`bundler` strict. `Zod` validation. `Vitest` testing. Frontend: React/Vue/Solid. Backend: Hono/tRPC. ORM: Prisma/Drizzle.
→ `~/s/llm/coding/rules/ts/004-ecosystem.md`

## gRPC / Proto

**001** `file-organization` — Lowercase dot-separated packages with version suffix (v1, v2). Directory MUST match package path. Separate services and messages. Specify language-specific options.
→ `~/s/llm/coding/rules/grpc-proto/001-file-organization.md`

**002** `message-design` — PascalCase messages, snake_case fields, SCREAMING_SNAKE_CASE enums with prefix. Fields 1-15 for frequent access. MUST reserve removed fields. Wrapper types for nullable. FORBIDDEN: reusing field numbers, generic catch-all fields.
→ `~/s/llm/coding/rules/grpc-proto/002-message-design.md`

**003** `service-design` — `<Domain>Service` + `<Verb><Resource>` RPCs. Dedicated Request/Response per RPC. Paginate with `page_token`. Streaming for real-time/large data. Idempotency keys for financial ops. FORBIDDEN: "God services."
→ `~/s/llm/coding/rules/grpc-proto/003-service-design.md`

**004** `versioning` — Version in package name (v1, v2). Safe: add fields/RPCs/enum values. Breaking → new version. MUST reserve removed field numbers/names. Deprecate before removal (`[deprecated = true]`). Run both versions during transition.
→ `~/s/llm/coding/rules/grpc-proto/004-versioning.md`

**005** `best-practices` — MUST document all contracts. MUST use `buf` for linting + breaking change detection. Frequent fields 1-15, rare at 100+. NEVER secrets in responses. Separate internal/external messages. Enums: `UNSPECIFIED = 0`.
→ `~/s/llm/coding/rules/grpc-proto/005-best-practices.md`
