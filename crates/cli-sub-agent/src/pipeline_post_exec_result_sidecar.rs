use std::fs;
use std::path::Path;

pub(super) fn status_is_success(session_dir: &Path) -> bool {
    let path = session_dir.join(csa_session::CONTRACT_RESULT_ARTIFACT_PATH);
    let Ok(contents) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(toml::Value::Table(table)) = toml::from_str::<toml::Value>(&contents) else {
        return false;
    };

    let nested = table
        .get("result")
        .and_then(|value| value.as_table())
        .and_then(|table| table.get("status"));
    let flat = table.get("status");

    nested
        .or(flat)
        .and_then(|value| value.as_str())
        .is_some_and(|status| status.eq_ignore_ascii_case("success"))
}
