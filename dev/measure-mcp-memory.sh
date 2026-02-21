#!/usr/bin/env bash
# measure-mcp-memory.sh — Phase 0 baseline measurement for issue #191
#
# Measures RSS delta between lean-mode (settingSources: []) and full-mode
# (settingSources: ["user","project"]) ACP sessions to determine MCP server
# memory overhead per claude-code instance.
#
# Usage:
#   ./dev/measure-mcp-memory.sh [--instances N] [--settle-secs S] [--proxy] [--proxy-socket PATH]
#
# Requirements:
#   - claude-code-acp on PATH
#   - ANTHROPIC_API_KEY set (or equivalent auth)
#   - /proc filesystem (Linux)

set -euo pipefail

# --- Configuration -----------------------------------------------------------

INSTANCES=3                  # concurrent instances to project savings for
SETTLE_SECS=15               # seconds to wait for tool to fully initialize
SAMPLES=3                    # number of measurement samples per mode
PROXY_MODE=false             # when true, measure with mcp-hub proxy injection
if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
    PROXY_SOCKET="${XDG_RUNTIME_DIR}/csa/mcp-hub.sock"
else
    PROXY_SOCKET="/tmp/csa-${UID}/mcp-hub.sock"
fi

while [[ $# -gt 0 ]]; do
    case "$1" in
        --instances) INSTANCES="$2"; shift 2 ;;
        --settle-secs) SETTLE_SECS="$2"; shift 2 ;;
        --samples) SAMPLES="$2"; shift 2 ;;
        --proxy) PROXY_MODE=true; shift ;;
        --proxy-socket) PROXY_SOCKET="$2"; shift 2 ;;
        -h|--help)
            printf 'Usage: %s [--instances N] [--settle-secs S] [--samples N] [--proxy] [--proxy-socket PATH]\n' "$0"
            exit 0 ;;
        *) printf 'Unknown option: %s\n' "$1" >&2; exit 1 ;;
    esac
done

# --- Helpers -----------------------------------------------------------------

log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }

die() { log "FATAL: $*"; exit 1; }

# Get VmRSS in KB for a PID from /proc.
# Falls back to ps if /proc is unavailable.
get_rss_kb() {
    local pid="$1"
    if [[ -r "/proc/$pid/status" ]]; then
        awk '/^VmRSS:/ { print $2 }' "/proc/$pid/status"
    else
        ps -o rss= -p "$pid" 2>/dev/null | tr -d ' '
    fi
}

# Sum RSS of a process and all descendants (MCP servers are child processes).
get_tree_rss_kb() {
    local root_pid="$1"
    local total=0
    local pids

    # Collect process tree: root + all descendants
    pids=$(pstree -p "$root_pid" 2>/dev/null \
        | grep -oP '\(\K[0-9]+(?=\))' \
        || echo "$root_pid")

    for pid in $pids; do
        local rss
        rss=$(get_rss_kb "$pid" 2>/dev/null || echo 0)
        if [[ -n "$rss" && "$rss" -gt 0 ]]; then
            total=$((total + rss))
        fi
    done
    echo "$total"
}

# Launch claude-code via ACP with specified settingSources, measure RSS.
# Args: mode_label, json_meta
measure_mode() {
    local label="$1"
    local meta_json="$2"
    local total_rss=0
    local sample_count=0

    log "Measuring mode=$label (${SAMPLES} samples, settle=${SETTLE_SECS}s)"

    for i in $(seq 1 "$SAMPLES"); do
        log "  Sample $i/$SAMPLES ..."

        # Create a temp file for the ACP process to write to
        local tmpdir
        tmpdir=$(mktemp -d)
        local pid_file="$tmpdir/acp.pid"

        # Spawn claude-code-acp in background
        claude-code-acp &
        local acp_pid=$!
        echo "$acp_pid" > "$pid_file"

        # Wait for process to exist and stabilize
        local waited=0
        while ! kill -0 "$acp_pid" 2>/dev/null && [[ $waited -lt 5 ]]; do
            sleep 0.5
            waited=$((waited + 1))
        done

        if ! kill -0 "$acp_pid" 2>/dev/null; then
            log "  WARN: ACP process $acp_pid exited before measurement"
            rm -rf "$tmpdir"
            continue
        fi

        # Send initialize + new_session with meta via stdin (JSON-RPC)
        # The ACP protocol expects JSON-RPC 2.0 messages on stdin.
        local init_msg='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}'
        local session_msg
        session_msg=$(printf '{"jsonrpc":"2.0","id":2,"method":"session/new","params":{"workingDirectory":"%s","meta":%s}}' \
            "$(pwd)" "$meta_json")

        # Send init, then session/new
        {
            printf '%s\n' "$init_msg"
            sleep 2
            printf '%s\n' "$session_msg"
        } > "/proc/$acp_pid/fd/0" 2>/dev/null || true

        # Allow MCP servers to start and settle
        log "  Waiting ${SETTLE_SECS}s for initialization..."
        sleep "$SETTLE_SECS"

        if ! kill -0 "$acp_pid" 2>/dev/null; then
            log "  WARN: ACP process exited during settle period"
            rm -rf "$tmpdir"
            continue
        fi

        # Measure full process tree RSS
        local rss_kb
        rss_kb=$(get_tree_rss_kb "$acp_pid")
        local rss_mb=$(( rss_kb / 1024 ))
        log "  Sample $i: tree RSS = ${rss_kb} KB (${rss_mb} MB)"

        total_rss=$((total_rss + rss_kb))
        sample_count=$((sample_count + 1))

        # Cleanup: kill process tree
        kill -- -"$acp_pid" 2>/dev/null || kill "$acp_pid" 2>/dev/null || true
        wait "$acp_pid" 2>/dev/null || true
        rm -rf "$tmpdir"
    done

    if [[ $sample_count -eq 0 ]]; then
        die "No successful samples for mode=$label"
    fi

    local avg_rss_kb=$((total_rss / sample_count))
    echo "$avg_rss_kb"
}

# --- Alternative: csa-based measurement -------------------------------------
# If csa binary is available, use it for more accurate measurement since
# it handles the full ACP lifecycle properly.

measure_via_csa() {
    local label="$1"
    local lean_flag="$2"  # "" or "--lean"
    local total_rss=0
    local sample_count=0

    log "Measuring mode=$label via csa (${SAMPLES} samples, settle=${SETTLE_SECS}s)"

    for i in $(seq 1 "$SAMPLES"); do
        log "  Sample $i/$SAMPLES ..."

        # Launch csa in background with a simple prompt that keeps it alive
        local tmpdir
        tmpdir=$(mktemp -d)

        # shellcheck disable=SC2086
        env -u CLAUDECODE csa run --tool claude-code $lean_flag \
            --no-stream-stdout \
            "Respond with exactly: READY" \
            > "$tmpdir/stdout" 2>"$tmpdir/stderr" &
        local csa_pid=$!

        # Wait for the ACP subprocess to appear
        sleep "$SETTLE_SECS"

        # Find the claude-code-acp child process
        local acp_pid
        acp_pid=$(pgrep -P "$csa_pid" -f "claude-code-acp" 2>/dev/null | head -1 || true)

        if [[ -z "$acp_pid" ]]; then
            # Try finding any child of csa
            acp_pid=$(pgrep -P "$csa_pid" 2>/dev/null | head -1 || true)
        fi

        local rss_kb=0
        if [[ -n "$acp_pid" ]] && kill -0 "$acp_pid" 2>/dev/null; then
            rss_kb=$(get_tree_rss_kb "$acp_pid")
        elif kill -0 "$csa_pid" 2>/dev/null; then
            # Measure csa's full tree as fallback
            rss_kb=$(get_tree_rss_kb "$csa_pid")
        fi

        local rss_mb=$(( rss_kb / 1024 ))
        log "  Sample $i: tree RSS = ${rss_kb} KB (${rss_mb} MB)"

        if [[ $rss_kb -gt 0 ]]; then
            total_rss=$((total_rss + rss_kb))
            sample_count=$((sample_count + 1))
        fi

        # Cleanup
        kill -- -"$csa_pid" 2>/dev/null || kill "$csa_pid" 2>/dev/null || true
        wait "$csa_pid" 2>/dev/null || true
        rm -rf "$tmpdir"
    done

    if [[ $sample_count -eq 0 ]]; then
        log "WARN: No successful csa samples for mode=$label — returning 0"
        echo "0"
        return
    fi

    local avg_rss_kb=$((total_rss / sample_count))
    echo "$avg_rss_kb"
}

# --- Main --------------------------------------------------------------------

main() {
    log "=== MCP Memory Baseline Measurement (Issue #191) ==="
    log "Configuration: instances=$INSTANCES settle=${SETTLE_SECS}s samples=$SAMPLES proxy_mode=$PROXY_MODE"
    if [[ "$PROXY_MODE" == "true" ]]; then
        log "Proxy socket: $PROXY_SOCKET"
    fi
    log ""

    local use_csa=false
    if command -v csa >/dev/null 2>&1; then
        use_csa=true
        log "Using csa binary for measurement (preferred)"
    elif command -v claude-code-acp >/dev/null 2>&1; then
        log "Using claude-code-acp directly"
    else
        die "Neither 'csa' nor 'claude-code-acp' found on PATH.
Install ACP adapter: npm install -g @zed-industries/claude-code-acp
Or build csa: cargo build --release -p cli-sub-agent"
    fi

    log ""

    # --- Lean mode (settingSources: []) — no MCP servers loaded ---
    local lean_rss_kb
    if [[ "$use_csa" == "true" ]]; then
        lean_rss_kb=$(measure_via_csa "lean" "--lean")
    else
        lean_rss_kb=$(measure_mode "lean" '{"claudeCode":{"options":{"settingSources":[]}}}')
    fi
    local lean_rss_mb=$(( lean_rss_kb / 1024 ))
    log ""
    log "Lean mode avg RSS: ${lean_rss_kb} KB (${lean_rss_mb} MB)"

    # --- Full mode (settingSources: ["user","project"]) — all MCPs loaded ---
    local full_rss_kb
    if [[ "$PROXY_MODE" == "true" ]]; then
        # Proxy mode: inject a single mcp-hub endpoint to avoid per-tool MCP duplication.
        full_rss_kb=$(measure_mode "proxy" "{\"claudeCode\":{\"options\":{\"settingSources\":[\"user\",\"project\"],\"mcpServers\":{\"csa-mcp-hub\":{\"transport\":\"unix\",\"socketPath\":\"$PROXY_SOCKET\"}}}}}")
    elif [[ "$use_csa" == "true" ]]; then
        full_rss_kb=$(measure_via_csa "full" "")
    else
        full_rss_kb=$(measure_mode "full" '{"claudeCode":{"options":{"settingSources":["user","project"]}}}')
    fi
    local full_rss_mb=$(( full_rss_kb / 1024 ))
    log ""
    log "Full mode avg RSS: ${full_rss_kb} KB (${full_rss_mb} MB)"

    # --- Compute delta ---
    local delta_kb=$((full_rss_kb - lean_rss_kb))
    local delta_mb=$((delta_kb / 1024))
    local total_waste_mb=$((delta_mb * INSTANCES))
    local total_concurrent_rss_mb=$((full_rss_mb * INSTANCES))
    local under_4gb=false
    if [[ $total_concurrent_rss_mb -lt 4096 ]]; then
        under_4gb=true
    fi

    log ""
    log "================================================================"
    log "                   RESULTS SUMMARY"
    log "================================================================"
    log ""
    log "  Lean mode (no MCPs):    ${lean_rss_mb} MB"
    log "  Full mode (all MCPs):   ${full_rss_mb} MB"
    log "  Delta per instance:     ${delta_mb} MB"
    log "  Projected waste (×${INSTANCES}): ${total_waste_mb} MB"
    log "  Projected concurrent RSS (×${INSTANCES}): ${total_concurrent_rss_mb} MB"
    log "  Concurrent RSS < 4GB:   ${under_4gb}"
    log ""

    if [[ $delta_mb -ge 200 ]]; then
        log "  GATE: PASS — delta >= 200 MB. Proceed to Phase 1."
    else
        log "  GATE: FAIL — delta < 200 MB. Savings insufficient."
        log "  Recommendation: Close issue, daemon complexity not justified."
    fi
    log ""
    log "================================================================"

    # --- Output structured TOML report ---
    local report_file
    report_file="$(cd "$(dirname "$0")/.." && pwd)/dev/mcp-memory-result.toml"

    cat > "$report_file" <<EOF
# MCP Memory Baseline Measurement — Issue #191
# Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)

[measurement]
samples = $SAMPLES
settle_seconds = $SETTLE_SECS
projected_instances = $INSTANCES
proxy_mode = $PROXY_MODE
proxy_socket = "$PROXY_SOCKET"

[results]
lean_mode_rss_kb = $lean_rss_kb
lean_mode_rss_mb = $lean_rss_mb
full_mode_rss_kb = $full_rss_kb
full_mode_rss_mb = $full_rss_mb
delta_per_instance_kb = $delta_kb
delta_per_instance_mb = $delta_mb
total_projected_waste_mb = $total_waste_mb
total_projected_rss_mb = $total_concurrent_rss_mb
projected_under_4gb = $under_4gb

[gate]
threshold_mb = 200
passed = $(if [[ $delta_mb -ge 200 ]]; then echo "true"; else echo "false"; fi)
recommendation = "$(if [[ $delta_mb -ge 200 ]]; then echo "proceed_to_phase_1"; else echo "close_insufficient_savings"; fi)"
EOF

    log "Report written to: $report_file"
    echo "$report_file"
}

main "$@"
