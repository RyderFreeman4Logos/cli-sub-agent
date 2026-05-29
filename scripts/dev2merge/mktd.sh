#!/usr/bin/env bash
#===============================================================================
# File: scripts/dev2merge/mktd.sh
#=======================================================================80090888
#
# Purpose: Main script for the mktd step in dev2merge workflow.
#   - Saves a TODO plan via `csa todo save <plan_name>`
#   - Immediately attests the plan via `csa todo attest <plan_name>`
#   - Provides structured logging (stderr or optional logfile)
#   - Exits with non-zero on any failure, preserving diagnostic output
#
# Security: Ensures that freshly generated plans have an up-to-date attestation
#   hash, preventing false-positive "[PLAN TAMPERED]" warnings.
#
# Usage:
#   ./mktd.sh <plan_name> [--log <logfile>] [--help]
#
# Arguments:
#   plan_name   - Must be a valid timestamp or identifier (e.g. 20260529T051251).
#                 Only alphanumeric characters and underscores are allowed.
#   --log       - (Optional) Path to a log file; if provided, log messages are
#                 appended there instead of written to stderr.
#   --help      - Show this help and exit.
#
# Dependencies:
#   - `csa` CLI tool must be available in $PATH.
#   - Standard POSIX utilities (date, printf, touch, dirname, mkdir, etc.)
#
# Exit codes:
#   0  - Success
#   1  - General error (save/attest failure)
#   2  - Usage error (invalid arguments)
#   3  - Input validation error (invalid plan name, unwritable logfile)
#   4  - Missing dependency (`csa` not found)
#
#===============================================================================

set -euo pipefail
IFS=$'\n\t'

#-------------------------------------------------------------------------------
# Global constants
#-------------------------------------------------------------------------------
readonly SCRIPT_NAME="$(basename "$0")"
readonly PLAN_NAME_REGEX='^[A-Za-z0-9_]+$'
readonly TIMESTAMP_FORMAT='%Y-%m-%dT%H:%M:%S%z'
readonly CSA_CMD='csa'
readonly MAX_PLAN_NAME_LENGTH=255

#-------------------------------------------------------------------------------
# Logging – write messages to stderr or a log file (prepend timestamp & severity)
#-------------------------------------------------------------------------------
log_msg() {
    local level="$1" msg="$2"
    local logline
    logline="$(date +"${TIMESTAMP_FORMAT}") [${level}] ${SCRIPT_NAME}: ${msg}"
    if [[ -n "${LOGFILE:-}" ]]; then
        printf '%s\n' "$logline" >> "$LOGFILE"
    else
        printf '%s\n' "$logline" >&2
    fi
}

#-------------------------------------------------------------------------------
# Trap handlers
#-------------------------------------------------------------------------------
# Cleanup on exit (normal or error)
cleanup() {
    local exit_code=$?
    if [[ $exit_code -ne 0 ]]; then
        log_msg "ERROR" "Terminated with exit code $exit_code"
    fi
}
trap cleanup EXIT ERR

# Intercept common signals for additional logging (optional)
trap 'log_msg "WARN" "Received SIGINT (Ctrl+C)"; exit 130' INT
trap 'log_msg "WARN" "Received SIGTERM"; exit 143' TERM

#-------------------------------------------------------------------------------
# Function: validate_logfile
#   Ensures that the log file path is writable (if provided).
#   Creates the parent directory if it does not exist.
#   Returns 0 on success, exits with code 3 on failure.
#-------------------------------------------------------------------------------
validate_logfile() {
    local path="$1"

    if [[ -z "$path" ]]; then
        log_msg "ERROR" "Internal: validate_logfile called with empty path"
        exit 3
    fi

    local dir
    dir="$(dirname "$path")"

    if [[ ! -d "$dir" ]]; then
        if ! mkdir -p "$dir" 2>/dev/null; then
            log_msg "ERROR" "Cannot create directory for log file: $dir"
            exit 3
        fi
    fi

    # Test write access by attempting to append/truncate (touch is fine)
    if ! touch "$path" 2>/dev/null; then
        log_msg "ERROR" "Log file is not writable or cannot be created: $path"
        exit 3
    fi

    log_msg "INFO" "Using log file: $path"
    return 0
}

#-------------------------------------------------------------------------------
# Function: usage
#   Prints usage information to stderr and exits with code 2.
#-------------------------------------------------------------------------------
usage() {
    cat >&2 <<EOF
Usage: ${SCRIPT_NAME} <plan_name> [--log <logfile>] [--help]

Save and attest a TODO plan.

Arguments:
  plan_name    - Identifier for the plan (alphanumeric, underscores only).
  --log FILE   - Optional log file location (default: stderr).
  --help       - Show this help and exit.

Example:
  ${SCRIPT_NAME} 20260529T051251
  ${SCRIPT_NAME} 20260529T051251 --log /var/log/mktd.log

EOF
    exit 2
}

#-------------------------------------------------------------------------------
# Function: validate_plan_name
#   Checks the plan name against the allowed pattern and length.
#   Exits with code 3 on any violation.
#-------------------------------------------------------------------------------
validate_plan_name() {
    local name="$1"

    if [[ -z "$name" ]]; then
        log_msg "ERROR" "Plan name must not be empty"
        exit 3
    fi

    if [[ ! "$name" =~ $PLAN_NAME_REGEX ]]; then
        log_msg "ERROR" "Invalid plan name '$name'. Must match regex: ${PLAN_NAME_REGEX}"
        exit 3
    fi

    if [[ ${#name} -gt $MAX_PLAN_NAME_LENGTH ]]; then
        log_msg "ERROR" "Plan name too long (${#name} chars). Maximum: $MAX_PLAN_NAME_LENGTH."
        exit 3
    fi

    return 0
}

#-------------------------------------------------------------------------------
# Function: run_csa_command
#   Wrapper to run a csa subcommand with proper error handling and logging.
#   Arguments:
#     1 - Subcommand (e.g., "todo save", "todo attest")
#     2 - Plan name
#   The function will exit the script on failure.
#-------------------------------------------------------------------------------
run_csa_command() {
    local subcommand="$1" plan="$2"
    log_msg "INFO" "Running: ${CSA_CMD} ${subcommand} '${plan}'"

    # Execute the command, capturing stderr to a variable if desired.
    # Since set -e is on, any failure will exit the script.
    # We use an explicit check to provide a specific error message.
    if ! "${CSA_CMD}" ${subcommand} "${plan}" 2>&1; then
        log_msg "ERROR" "'${CSA_CMD} ${subcommand} ${plan}' failed"
        exit 1
    fi

    log_msg "INFO" "'${CSA_CMD} ${subcommand} ${plan}' succeeded"
}

#-------------------------------------------------------------------------------
# Function: main
#   Entry point: parse arguments, validate, execute save & attest.
#-------------------------------------------------------------------------------
main() {
    local plan_name=""
    local logfile=""
    local args=("$@")

    # Parse arguments using a simple shift-based loop (POSIX-compatible)
    # This avoids the complexity of getopts with long options.
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --log)
                if [[ $# -lt 2 ]]; then
                    log_msg "ERROR" "Option '--log' requires an argument"
                    exit 2
                fi
                logfile="$2"
                shift 2
                ;;
            --help|-h)
                usage
                ;;
            --*)
                log_msg "ERROR" "Unknown long option: $1"
                exit 2
                ;;
            -?*)
                log_msg "ERROR" "Unknown short option: $1"
                exit 2
                ;;
            *)
                # Positional argument: we expect exactly one plan name
                if [[ -n "$plan_name" ]]; then
                    log_msg "ERROR" "Unexpected extra positional argument: $1"
                    exit 2
                fi
                plan_name="$1"
                shift
                ;;
        esac
    done

    # Ensure we have the required positional argument
    if [[ -z "$plan_name" ]]; then
        log_msg "ERROR" "Missing required positional argument: plan_name"
        usage
    fi

    # -- Validate plan name
    validate_plan_name "$plan_name"

    # -- If log file is specified, validate it and set global variable
    if [[ -n "$logfile" ]]; then
        validate_logfile "$logfile"
        # Export for use by log_msg
        export LOGFILE="$logfile"
    fi

    # -- Ensure csa is available and is executable
    if ! command -v "$CSA_CMD" &>/dev/null; then
        log_msg "ERROR" "Required command '$CSA_CMD' not found in PATH"
        exit 4
    fi

    #-----------------------------------------------------------------------
    # Step 1: Save the TODO plan
    #-----------------------------------------------------------------------
    log_msg "INFO" "Starting save and attest for plan: ${plan_name}"
    run_csa_command "todo save" "${plan_name}"

    #-----------------------------------------------------------------------
    # Step 2: Attest the plan (recompute and store hash)
    #-----------------------------------------------------------------------
    log_msg "INFO" "Attesting plan (computing and storing hash)..."
    run_csa_command "todo attest" "${plan_name}"

    #-----------------------------------------------------------------------
    # Success
    #-----------------------------------------------------------------------
    log_msg "INFO" "Plan '${plan_name}' saved and attested successfully."
}

# Execute main function with all passed arguments
main "$@"