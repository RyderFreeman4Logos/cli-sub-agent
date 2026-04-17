use super::*;

#[test]
fn attach_primary_output_for_session_routes_expected_primary_log() {
    for tool in attach_tool_cases() {
        for runtime_binary in attach_runtime_binary_cases() {
            for output_log_exists in [false, true] {
                for stdout_log_exists in [false, true] {
                    let route = attach_route_for_session(
                        tool,
                        runtime_binary,
                        output_log_exists,
                        stdout_log_exists,
                        true,
                    );

                    if codex_routes_to_output_log(tool, runtime_binary) {
                        assert_eq!(
                            route,
                            AttachPrimaryOutput::OutputLog,
                            "codex ACP sessions must route to output.log: tool={tool} runtime_binary={runtime_binary:?} output_log_exists={output_log_exists} stdout_log_exists={stdout_log_exists}"
                        );
                    }

                    if routes_to_output_log_independent_of_files(tool, runtime_binary) {
                        assert_eq!(
                            route,
                            AttachPrimaryOutput::OutputLog,
                            "ACP sessions must not pin to stdout.log: tool={tool} runtime_binary={runtime_binary:?} output_log_exists={output_log_exists} stdout_log_exists={stdout_log_exists}"
                        );

                        let flipped_route = attach_route_for_session(
                            tool,
                            runtime_binary,
                            !output_log_exists,
                            !stdout_log_exists,
                            true,
                        );
                        assert_eq!(
                            flipped_route, route,
                            "ACP routing must not depend on log file existence: tool={tool} runtime_binary={runtime_binary:?}"
                        );
                    }

                    if matches!(tool, "gemini-cli" | "opencode") {
                        assert_eq!(
                            route,
                            AttachPrimaryOutput::StdoutLog,
                            "legacy non-ACP tools may fall back to stdout.log: tool={tool} runtime_binary={runtime_binary:?} output_log_exists={output_log_exists} stdout_log_exists={stdout_log_exists}"
                        );
                    }
                }
            }
        }
    }
}

fn attach_route_for_session(
    tool: &str,
    runtime_binary: Option<&str>,
    output_log_exists: bool,
    stdout_log_exists: bool,
    session_active: bool,
) -> AttachPrimaryOutput {
    let td = tempfile::tempdir().expect("tempdir");
    let metadata = csa_session::metadata::SessionMetadata {
        tool: tool.to_string(),
        tool_locked: true,
        runtime_binary: runtime_binary.map(std::string::ToString::to_string),
    };
    let metadata_toml = toml::to_string_pretty(&metadata).expect("metadata toml");
    std::fs::write(
        td.path().join(csa_session::metadata::METADATA_FILE_NAME),
        metadata_toml,
    )
    .expect("write metadata");

    if output_log_exists {
        std::fs::write(td.path().join("output.log"), "output").expect("write output log");
    }
    if stdout_log_exists {
        std::fs::write(td.path().join("stdout.log"), "stdout").expect("write stdout log");
    }

    if session_active {
        let lock_name = match tool {
            "codex" => "codex.lock",
            "claude-code" => "claude-code.lock",
            "gemini-cli" => "gemini-cli.lock",
            "opencode" => "opencode.lock",
            other => panic!("unsupported tool {other}"),
        };
        std::fs::create_dir_all(td.path().join("locks")).expect("create locks dir");
        std::fs::write(
            td.path().join("locks").join(lock_name),
            format!("{{\"pid\":{}}}", std::process::id()),
        )
        .expect("write lock");
    }

    attach_primary_output_for_session(td.path())
}

fn attach_runtime_binary_cases() -> [Option<&'static str>; 4] {
    [
        None,
        Some("/opt/csa/bin/codex"),
        Some("/opt/csa/bin/codex-acp"),
        Some("/opt/csa/bin/claude-code"),
    ]
}

fn attach_tool_cases() -> [&'static str; 4] {
    ["codex", "claude-code", "gemini-cli", "opencode"]
}

fn codex_routes_to_output_log(tool: &str, runtime_binary: Option<&str>) -> bool {
    tool == "codex" && runtime_binary.is_none_or(runtime_binary_indicates_codex_acp)
}

fn routes_to_output_log_independent_of_files(tool: &str, runtime_binary: Option<&str>) -> bool {
    tool == "claude-code" || codex_routes_to_output_log(tool, runtime_binary)
}

fn runtime_binary_indicates_codex_acp(runtime_binary: &str) -> bool {
    std::path::Path::new(runtime_binary)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("codex-acp"))
}
