use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

pub(crate) const ACP_PAYLOAD_DEBUG_ENV: &str = "CSA_DEBUG_ACP_PAYLOAD";
const ACP_PAYLOAD_DEBUG_REL_PATH: &str = "output/acp-payload-debug.json";

pub(super) struct AcpPayloadDebugRequest<'a> {
    pub(super) env: &'a HashMap<String, String>,
    pub(super) session_dir: Option<&'a Path>,
    pub(super) tool_name: &'a str,
    pub(super) command: &'a str,
    pub(super) args: &'a [String],
    pub(super) working_dir: &'a Path,
    pub(super) resume_session_id: Option<&'a str>,
    pub(super) system_prompt: Option<&'a str>,
    pub(super) session_meta: Option<&'a serde_json::Map<String, serde_json::Value>>,
    pub(super) prompt: &'a str,
}

#[derive(Debug, Serialize)]
struct AcpPayloadDebug<'a> {
    tool_name: &'a str,
    command: &'a str,
    args: Vec<String>,
    working_dir: String,
    resume_session_id: Option<&'a str>,
    system_prompt: Option<&'a str>,
    session_meta: Option<serde_json::Map<String, Value>>,
    prompt_chars: usize,
    prompt_preview: String,
    prompt: &'a str,
}

fn debug_flag_enabled(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
}

fn acp_payload_debug_enabled(env: &HashMap<String, String>) -> bool {
    if let Some(value) = env.get(ACP_PAYLOAD_DEBUG_ENV) {
        return debug_flag_enabled(value);
    }

    std::env::var(ACP_PAYLOAD_DEBUG_ENV)
        .map(|value| debug_flag_enabled(&value))
        .unwrap_or(false)
}

fn redact_env_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for entry in map.values_mut() {
                *entry = Value::String("<redacted>".to_string());
            }
        }
        _ => *value = Value::String("<redacted>".to_string()),
    }
}

/// Short-form flags whose following argument is an HTTP header value and must
/// be checked for embedded credentials (e.g. `-H 'Authorization: Bearer ...'`).
/// Stored in ORIGINAL case — comparison is case-sensitive because `-h` (help)
/// and `-H` (header) are different flags.
const HEADER_SHORT_FLAGS: &[&str] = &["-H"];

fn arg_name_is_sensitive(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    [
        "api-key",
        "api_key",
        "apikey",
        "auth",
        "authorization",
        "bearer",
        "header",
        "password",
        "secret",
        "token",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

/// Returns `true` when `flag` is a short alias for a header flag (e.g. `-H`).
/// Case-sensitive: `-h` (help) must NOT match.
fn is_header_short_flag(flag: &str) -> bool {
    let trimmed = flag.trim();
    HEADER_SHORT_FLAGS.contains(&trimmed)
}

fn arg_value_is_sensitive(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized.contains("authorization:")
        || normalized.contains("bearer ")
        || normalized.contains("token=")
        || normalized.contains("token:")
        || normalized.contains("api-key=")
        || normalized.contains("api-key:")
        || normalized.contains("api_key=")
        || normalized.contains("api_key:")
        || normalized.contains("apikey=")
        || normalized.contains("apikey:")
        || normalized.contains("secret=")
        || normalized.contains("secret:")
        || normalized.contains("password=")
        || normalized.contains("password:")
}

fn arg_has_inline_sensitive_value(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    normalized.starts_with('-')
        && arg_name_is_sensitive(&normalized)
        && (normalized.contains('=') || normalized.contains(':'))
}

fn redact_args(args: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(args.len());
    let mut redact_next = false;

    for arg in args {
        if redact_next {
            redacted.push("<redacted>".to_string());
            redact_next = false;
            continue;
        }

        if let Some((name, _value)) = arg.split_once('=')
            && arg_name_is_sensitive(name)
        {
            redacted.push(format!("{name}=<redacted>"));
            continue;
        }

        if arg_has_inline_sensitive_value(arg) {
            redacted.push("<redacted>".to_string());
            continue;
        }

        if arg.starts_with('-') && arg_name_is_sensitive(arg) {
            redacted.push(arg.clone());
            redact_next = true;
            continue;
        }

        // Short header flags like `-H` take the next arg as an HTTP header
        // value that may embed credentials (e.g. `-H 'x-api-key: secret'`).
        if is_header_short_flag(arg) {
            redacted.push(arg.clone());
            redact_next = true;
            continue;
        }

        if arg_value_is_sensitive(arg) {
            redacted.push("<redacted>".to_string());
            continue;
        }

        redacted.push(arg.clone());
    }

    redacted
}

fn redact_session_meta(
    session_meta: Option<&serde_json::Map<String, Value>>,
) -> Option<serde_json::Map<String, Value>> {
    fn redact_nested_env_maps(value: &mut Value) {
        match value {
            Value::Object(map) => {
                for (key, entry) in map.iter_mut() {
                    if key == "env" {
                        redact_env_value(entry);
                    } else if key == "args" {
                        if let Value::Array(items) = entry {
                            let args = items
                                .iter()
                                .filter_map(Value::as_str)
                                .map(ToString::to_string)
                                .collect::<Vec<_>>();
                            *entry = Value::Array(
                                redact_args(&args).into_iter().map(Value::String).collect(),
                            );
                        }
                    } else {
                        redact_nested_env_maps(entry);
                    }
                }
            }
            Value::Array(items) => {
                for item in items {
                    redact_nested_env_maps(item);
                }
            }
            _ => {}
        }
    }

    let mut value = Value::Object(session_meta?.clone());
    redact_nested_env_maps(&mut value);
    value.as_object().cloned()
}

pub(super) fn maybe_write_acp_payload_debug(
    request: AcpPayloadDebugRequest<'_>,
) -> Option<PathBuf> {
    if !acp_payload_debug_enabled(request.env) {
        return None;
    }

    let debug_path = request.session_dir?.join(ACP_PAYLOAD_DEBUG_REL_PATH);
    if let Some(parent) = debug_path.parent()
        && fs::create_dir_all(parent).is_err()
    {
        return None;
    }

    let payload = AcpPayloadDebug {
        tool_name: request.tool_name,
        command: request.command,
        args: redact_args(request.args),
        working_dir: request.working_dir.display().to_string(),
        resume_session_id: request.resume_session_id,
        system_prompt: request.system_prompt,
        session_meta: redact_session_meta(request.session_meta),
        prompt_chars: request.prompt.chars().count(),
        prompt_preview: request.prompt.chars().take(2000).collect(),
        prompt: request.prompt,
    };
    let serialized = serde_json::to_string_pretty(&payload).ok()?;
    fs::write(&debug_path, format!("{serialized}\n")).ok()?;
    Some(debug_path)
}

#[cfg(test)]
mod tests {
    use super::redact_args;

    #[test]
    fn redact_args_redacts_inline_sensitive_values_without_consuming_following_args() {
        let args = vec![
            "-HAuthorization:Bearer abc123".to_string(),
            "--safe".to_string(),
            "--api-key=value-1".to_string(),
            "--api_key=value-2".to_string(),
            "--header".to_string(),
            "Authorization: Bearer xyz987".to_string(),
        ];

        assert_eq!(
            redact_args(&args),
            vec![
                "<redacted>".to_string(),
                "--safe".to_string(),
                "--api-key=<redacted>".to_string(),
                "--api_key=<redacted>".to_string(),
                "--header".to_string(),
                "<redacted>".to_string(),
            ]
        );
    }

    #[test]
    fn redact_args_redacts_short_header_flag_values() {
        let args = vec![
            "-H".to_string(),
            "x-api-key: secret".to_string(),
            "--verbose".to_string(),
            "-H".to_string(),
            "Content-Type: application/json".to_string(),
        ];

        assert_eq!(
            redact_args(&args),
            vec![
                "-H".to_string(),
                "<redacted>".to_string(),
                "--verbose".to_string(),
                "-H".to_string(),
                "<redacted>".to_string(),
            ]
        );
    }

    #[test]
    fn redact_args_does_not_treat_lowercase_h_as_header_flag() {
        // -h is typically "help", not "header" — must NOT arm redact_next
        let args = vec!["-h".to_string(), "some-value".to_string()];
        assert_eq!(
            redact_args(&args),
            vec!["-h".to_string(), "some-value".to_string()]
        );
    }
}
