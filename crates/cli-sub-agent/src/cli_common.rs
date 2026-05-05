use anyhow::Result;

/// Build version string combining Cargo.toml version and git describe.
pub(crate) fn build_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        let cargo_ver = env!("CARGO_PKG_VERSION");
        let git_desc = env!("CSA_GIT_DESCRIBE");
        if git_desc.is_empty() {
            cargo_ver.to_string()
        } else {
            format!("{cargo_ver} ({git_desc})")
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnTarget {
    Last,
    Auto,
    SessionId(String),
}

pub fn parse_return_to(value: &str) -> Result<ReturnTarget> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("return target cannot be empty");
    }
    match trimmed.to_ascii_lowercase().as_str() {
        "last" => Ok(ReturnTarget::Last),
        "auto" => Ok(ReturnTarget::Auto),
        _ => Ok(ReturnTarget::SessionId(trimmed.to_string())),
    }
}

pub(crate) fn validate_return_to(value: &str) -> std::result::Result<String, String> {
    parse_return_to(value)
        .map(|_| value.to_string())
        .map_err(|e| e.to_string())
}

pub(crate) fn parse_model_spec_arg(spec: &str) -> std::result::Result<String, String> {
    let known_tools: Vec<&'static str> = csa_config::global::all_known_tools()
        .iter()
        .map(|tool| tool.as_str())
        .collect();
    csa_executor::ModelSpec::parse_and_validate(spec, &known_tools)
        .map(|_| spec.to_string())
        .map_err(|e| e.to_string())
}

pub(crate) fn parse_spec_path_arg(spec: &str) -> std::result::Result<String, String> {
    csa_core::spec_validate::validate_spec(std::path::Path::new(spec))
        .map(|path| path.display().to_string())
        .map_err(|err| err.to_string())
}

pub(crate) fn validate_ulid(value: &str) -> std::result::Result<String, String> {
    csa_session::validate_session_id(value)
        .map(|_| value.to_string())
        .map_err(|e| e.to_string())
}
