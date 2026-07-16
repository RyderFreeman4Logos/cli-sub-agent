pub(crate) fn default_template() -> String {
    r#"# CSA Global Configuration
# Location: ~/.config/cli-sub-agent/config.toml
#
# This file controls system-wide settings for all CSA projects.
# API keys and concurrency limits are configured here (not in project config).

[defaults]
max_concurrent = 3  # Default max parallel instances per tool
# tool = "codex"  # Default tool when auto-detection fails

# Per-tool host state directories exposed writable to sandboxed tool processes.
# Environment variables such as CODEX_HOME and CLAUDE_CONFIG_DIR still win.
[tool_state_dirs]
codex = "~/.codex"
claude = "~/.claude"

# Per-tool overrides. Uncomment and configure as needed.
#
# [tools.claude-code]
# max_concurrent = 1
# [tools.claude-code.env]
# ANTHROPIC_API_KEY = "sk-ant-..."
#
# [tools.codex]
# max_concurrent = 3
# fast_mode = true  # 2x cost, faster output
# [tools.codex.env]
# OPENAI_API_KEY = "sk-..."
#
# [tools.opencode]
# max_concurrent = 2
# [tools.opencode.env]
# ANTHROPIC_API_KEY = "sk-ant-..."

# Tool priority for auto-selection (heterogeneous routing, review, debate).
# First = most preferred. Tools not listed keep their default order.
# Optional: primary_writer_spec seeds `csa run` when no model-selecting
# flag is given. Format matches --model-spec: tool/provider/model/thinking_budget.
# Example: prefer Claude Code for worker tasks, then Codex.
# [preferences]
# primary_writer_spec = "codex/openai/gpt-5.4/high"
# tool_priority = ["claude-code", "codex", "opencode"]

# Optional GitHub CLI auth override for issue workflows.
# When unset, CSA falls back to ~/.config/gh-aider for issue reads/comments.
# [github]
# config_dir = "/home/you/.config/gh-aider"

# Review workflow: which tool to use for code review.
# "auto" selects the heterogeneous counterpart of the parent tool:
#   claude-code parent -> codex, codex parent -> claude-code.
# Set explicitly if auto-detection fails (e.g., parent is opencode).
# Optional: set `tier` to resolve the tool from a tier's models list
# with heterogeneous preference. `tier` takes priority over `tool`.
# Optional: set `thinking` for default thinking budget (low/medium/high/xhigh).
[review]
tool = "auto"
# batch_commits = 1
# tier = "tier-4-critical"
# thinking = "xhigh"

# Debate workflow: which tool to use for adversarial debate / arbitration.
# "auto" selects the heterogeneous counterpart of the parent tool:
#   claude-code parent -> codex, codex parent -> claude-code.
# Set explicitly if auto-detection fails (e.g., parent is opencode).
# Optional: set `tier` to resolve the tool from a tier's models list
# with heterogeneous preference. `tier` takes priority over `tool`.
[debate]
tool = "auto"
# Default wall-clock timeout for `csa debate` (30 minutes).
timeout_seconds = 1800
# Optional default thinking budget for `csa debate`.
# thinking = "high"
# tier = "tier-4-critical"
# Allow same-model adversarial fallback when heterogeneous models are unavailable.
# When true, `csa debate` runs two independent sub-agents of the same tool.
# Output is annotated with "same-model adversarial" to indicate degraded diversity.
same_model_fallback = true

# Fallback behavior when external services are unavailable.
# cloud_review_exhausted: what to do when cloud review bot is unavailable.
#   "auto-local" = automatically fall back to local CSA review (still reviews)
#   "ask-user"   = prompt user before falling back (default)
[fallback]
cloud_review_exhausted = "ask-user"

# Retry loops stop after this many total attempts.
[retry]
max_attempts = 3

# Issue/session token budget. New sessions inherit this unless a tighter tier
# token_budget applies.
[budget]
max_tokens_per_issue = 5000000

# Global-only emergency escape hatch for exact-model and force tier bypasses.
# Keep false by default: when a project has [tiers], use --tier <name>.
# Set true only in ~/.config/cli-sub-agent/config.toml when you need to allow
# --model-spec, --force, or --force-ignore-tier-setting under configured tiers.
[tier_policy]
allow_force_bypass = false

# Safety ceiling for the opt-in convergence completion path. Every field must be
# explicitly permitted here, a project may only tighten these limits, and the
# caller must still pass `csa review --converge --execute-completion`.
# [convergence_completion]
# allow_execution = false
# allow_provider_egress = false
# allow_shell_commands = false
# allow_credential_inheritance = false
# max_retention_days = 0

# Experimental feature flags. Disabled by default.
[experimental]
enable_prompt_caching = false
max_goal_loops = 3
max_goal_tokens = 500000
task_pool_workers = 1

# KV cache-aware polling defaults.
# `frequent_poll_seconds`: fast external state (GitHub API, bot responses).
# `default_ttl_seconds`: fallback when caller model provider detection is unavailable.
# `long_poll_seconds` is a deprecated alias for `default_ttl_seconds`.
[kv_cache]
frequent_poll_seconds = 60
default_ttl_seconds = 240

[kv_cache.provider_ttls]
claude = 3300
openai = 1700
glm = 540
xai = 1700
other = 270

# Parent-tool caller hints.
# Codex callers should prefer the CSA MCP `csa_session_wait` tool with a long
# tool timeout (default hint: 7200 seconds). This shell yield is kept for
# fallback `csa session wait` calls when MCP is unavailable.
# [caller_hints]
# codex_session_wait_yield_ms = 300000

# Optional early-exit warning for `csa session wait`.
# When the watched session's process tree RSS exceeds this threshold,
# `csa session wait` prints a CSA:MEMORY_WARN marker and exits 33.
# [session_wait]
# memory_warn_mb = 8192

# Display commands for `csa todo` subcommands.
# When set, output is piped through the specified command (only when stdout is a terminal).
# [todo]
# show_command = "bat -l md"   # Pipe `csa todo show` output through bat
# diff_command = "delta"       # Pipe `csa todo diff` output through delta

# Post-exec quality gate for successful `csa run` employee sessions.
# The merged project view inherits this section unless `.csa/config.toml`
# overrides it.
# [run]
# writer_must_commit = false
# [run.large_diff_warning]
# enabled = true
# changed_files = 5
# changed_lines = 500
# approx_diff_tokens = 8000
# mode = "warn"
# [run.post_exec_gate]
# enabled = true
# command = "just pre-commit"
# timeout_seconds = 1800  # Default 1800s (30 min). Increase for heavy Rust projects with long pre-commit hooks.
# skip_on_no_changes = true

# MCP (Model Context Protocol) servers injected into all tool sessions.
# Project-level .csa/mcp.toml servers override global ones with the same name.
#
# Stdio transport (local process, default):
# [[mcp.servers]]
# name = "repomix"
# type = "stdio"
# command = "npx"
# args = ["-y", "repomix", "--mcp"]
#
# HTTP transport (remote server, requires transport-http-client feature):
# [[mcp.servers]]
# name = "remote-mcp"
# type = "http"
# url = "https://mcp.example.com/mcp"
# # headers = { Authorization = "Bearer ..." }
# # allow_insecure = false  # Set true for http:// (not recommended)
#
# Legacy format (auto-detected as stdio, backward-compatible):
# [[mcp.servers]]
# name = "deepwiki"
# command = "npx"
# args = ["-y", "@anthropic/deepwiki-mcp"]
#
# Optional shared MCP hub socket path.
# mcp_proxy_socket = "/run/user/1000/cli-sub-agent/mcp-hub.sock"

# Execution tuning. Project-level [execution] overrides these values.
# [execution]
# min_timeout_seconds = 1800  # Floor for --timeout flag (seconds)
# auto_weave_upgrade = false  # Run `weave upgrade` before each CSA command
# ACP transport tuning. Project-level [acp] overrides these values.
# [acp]
# init_timeout_seconds = 120  # Timeout for ACP session creation (seconds)

# Session behaviour. Project-level [session] overrides these values.
# [session]
# fork_prefix_budget = 32768  # CSA-lite fork prefix token budget; clamped to [4096, 131072]
"#
    .to_string()
}
