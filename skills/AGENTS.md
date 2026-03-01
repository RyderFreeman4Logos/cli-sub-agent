# Skills & Patterns

> Global persona skills, compatibility workflow shims, and compiled workflow patterns.

Note: Workflow logic is defined in `./patterns/`. `skills/csa-review` and
`skills/debate` are kept as compatibility shims so `csa skill install` still
installs the workflow entrypoints required by `csa review` and `csa debate`.

## Global Persona Skills (6)

| Skill | Description |
|---|---|
| `csa` | Unified CLI sub-agent runtime with persistent sessions and recursive orchestration. |
| `csa-async-debug` | Expert diagnosis for Tokio/async Rust issues (deadlocks, leaks, cancellation safety). |
| `csa-doc-writing` | Practical technical writing for APIs, READMEs, architecture decisions, and changelogs. |
| `csa-rust-dev` | Comprehensive Rust development guidance for architecture, implementation, and standards. |
| `csa-security` | Adversarial security analysis to identify vulnerabilities before release. |
| `csa-test-gen` | Core-first test design and TDD guidance following the test pyramid. |

## Compatibility Workflow Skills (2)

| Skill | Description |
|---|---|
| `csa-review` | Compatibility shim for review command scaffolding; delegates behavior to workflow protocol. |
| `debate` | Compatibility shim for debate command scaffolding and continuation protocol. |

## Compiled Patterns (14)

| Pattern | Description |
|---|---|
| `ai-reviewed-commit` | AI-reviewed commit loop: review, fix, and re-review until clean before committing. |
| `code-review` | Scale-adaptive GitHub PR review workflow for small, medium, and large changes. |
| `commit` | Strict audited commit workflow with security, test, and review gates. |
| `csa-issue-reporter` | Structured GitHub issue filing workflow for CSA runtime/tool errors. |
| `csa-review` | Independent CSA-driven code review with session isolation and structured output. |
| `debate` | Adversarial multi-tool strategy debate with escalation and convergence checks. |
| `dev2merge` | End-to-end branch-to-merge workflow with mandatory mktd planning/debate gate. |
| `dev-to-merge` | End-to-end branch-to-merge workflow: implement, validate, PR, bot review, merge. |
| `file-audit` | Per-file AGENTS.md compliance audit with report generation workflow. |
| `mktd` | Make TODO workflow: reconnaissance, drafting, debate, and approval. |
| `mktsk` | Plan-to-execution workflow converting TODO plans into persistent serial tasks. |
| `pr-codex-bot` | Iterative PR review loop with Codex bot feedback and false-positive arbitration. |
| `sa` | Three-layer recursive sub-agent orchestration for dispatch, implementation, and fixes. |
| `security-audit` | Pre-commit adversarial security and test-completeness audit workflow. |
