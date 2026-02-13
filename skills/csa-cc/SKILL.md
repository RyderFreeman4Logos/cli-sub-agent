---
name: csa-cc
description: Thin routing skill that replaces all 14 Claude Code agents with csa claude-sub-agent
allowed-tools: Bash, Read, Grep, Glob, Task
---

# csa-cc: Claude Code Sub-Agent Router

Thin routing skill that replaces all 14 Claude Code agents with `csa claude-sub-agent`.

## When to Use

Use this skill when the main agent needs to delegate a task to a sub-agent.
Instead of choosing between 14+ agent types, use `csa-cc` as the single entry point.

## How It Works

1. Match task keywords to a domain skill (see mapping below)
2. Call `csa claude-sub-agent --skill <path>` with the matched skill
3. CSA handles tool selection internally (heterogeneous rules, config-driven fallback)

## Keyword → Domain Skill Mapping

| Keywords | Domain Skill | Path |
|----------|-------------|------|
| `rust`, `cargo`, `crate`, `clippy`, `rustfmt` | csa-rust-dev | `~/.claude/skills/csa-rust-dev` |
| `unsafe`, `ffi`, `concurrency`, `async`, `tokio`, `deadlock` | csa-async-debug | `~/.claude/skills/csa-async-debug` |
| `security`, `audit`, `vulnerability`, `injection`, `DoS` | csa-security | `~/.claude/skills/csa-security` |
| `test`, `coverage`, `property-based`, `fuzz`, `tdd` | csa-test-gen | `~/.claude/skills/csa-test-gen` |
| `doc`, `readme`, `api-doc`, `changelog`, `adr` | csa-doc-writing | `~/.claude/skills/csa-doc-writing` |

## Invocation Pattern

```bash
# With domain skill (keywords match)
csa claude-sub-agent --skill ~/.claude/skills/csa-rust-dev "implement the handler"

# Without domain skill (no keyword match)
csa claude-sub-agent "quick search for auth files"

# With explicit tool selection
csa claude-sub-agent --tool claude-code --skill ~/.claude/skills/csa-security "audit this module"
```

## Routing Decision Table

| Task Characteristic | Tool Selection | Domain Skill |
|--------------------|---------------|-------------|
| Quick lookup, summary, simple edit | auto (heterogeneous) | none |
| Standard implementation, write tests | auto (heterogeneous) | matched (if any) |
| Complex architecture, multi-file refactor | `--tool claude-code` | matched (if any) |
| Security-critical code | `--tool claude-code` | `csa-security` |
| Rust tasks | auto (heterogeneous) | `csa-rust-dev` |
| Async/concurrency debugging | `--tool claude-code` | `csa-async-debug` |

## What This Replaces

Previously: 14 agents in `~/.claude/agents/` (haiku/sonnet/opus-executor,
rust-sonnet/opus-developer, security-reviewer, async-debugger, test-generator,
doc-writer, etc.)

Now: This single routing skill + 5 domain skills + `csa claude-sub-agent` subcommand.

## Audit Trail

Every invocation logs: tool, skill path, exit code (via CSA's tracing).
