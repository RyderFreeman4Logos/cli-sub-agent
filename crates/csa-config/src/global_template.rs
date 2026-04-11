pub(crate) fn default_template() -> String {
    r#"# CSA Global Configuration
# Location: ~/.config/cli-sub-agent/config.toml
#
# This file controls system-wide settings for all CSA projects.
# API keys and concurrency limits are configured here (not in project config).

[defaults]
max_concurrent = 3  # Default max parallel instances per tool
# tool = "codex"  # Default tool when auto-detection fails

# Per-tool overrides. Uncomment and configure as needed.
#
# [tools.gemini-cli]
# max_concurrent = 5
# api_key = "AI..."  # Fallback only after quota exhaustion; fresh invocations stay OAuth-first.
# [tools.gemini-cli.env]
# CSA_GEMINI_INCLUDE_DIRECTORIES = "/abs/path/one,/abs/path/two"
#
# [tools.claude-code]
# max_concurrent = 1
# [tools.claude-code.env]
# ANTHROPIC_API_KEY = "sk-ant-..."
#
# [tools.codex]
# max_concurrent = 3
# [tools.codex.env]
# OPENAI_API_KEY = "sk-..."
#
# [tools.opencode]
# max_concurrent = 2
# [tools.opencode.env]
# ANTHROPIC_API_KEY = "sk-ant-..."

# Tool priority for auto-selection (heterogeneous routing, review, debate).
# First = most preferred. Tools not listed keep their default order.
# Example: prefer Claude Code for worker tasks, then Codex.
# [preferences]
# tool_priority = ["claude-code", "codex", "gemini-cli", "opencode"]

# Review workflow: which tool to use for code review.
# "auto" selects the heterogeneous counterpart of the parent tool:
#   claude-code parent -> codex, codex parent -> claude-code.
# Set explicitly if auto-detection fails (e.g., parent is opencode).
# Optional: set `tier` to resolve the tool from a tier's models list
# with heterogeneous preference. `tier` takes priority over `tool`.
# Optional: set `thinking` for default thinking budget (low/medium/high/xhigh).
[review]
tool = "auto"
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

# KV cache-aware polling defaults.
# `frequent_poll_seconds`: fast external state (GitHub API, bot responses).
# `long_poll_seconds`: long waits that return control before the caller cache goes cold.
# Max-tier Opus users can raise `long_poll_seconds` to 3000 (~50 min on a 1h TTL).
[kv_cache]
frequent_poll_seconds = 60
long_poll_seconds = 240

# Display commands for `csa todo` subcommands.
# When set, output is piped through the specified command (only when stdout is a terminal).
# [todo]
# show_command = "bat -l md"   # Pipe `csa todo show` output through bat
# diff_command = "delta"       # Pipe `csa todo diff` output through delta

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
"#
    .to_string()
}
