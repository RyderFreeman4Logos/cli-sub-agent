use crate::state::MetaSessionState;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};

pub(crate) fn load_session_with_created_at_fallback(
    contents: &str,
    session_id: &str,
) -> Result<MetaSessionState> {
    let mut value: toml::Value = toml::from_str(contents)?;
    let Some(table) = value.as_table_mut() else {
        bail!("Session state is not a TOML table");
    };

    if table.contains_key("created_at") {
        bail!("Session state already has created_at");
    }

    table.insert(
        "created_at".to_string(),
        toml::Value::String(decode_session_created_at(session_id)?.to_rfc3339()),
    );

    value
        .try_into::<MetaSessionState>()
        .context("Failed to parse session state after ULID created_at fallback")
}

pub fn decode_session_created_at(session_id: &str) -> Result<DateTime<Utc>> {
    let ulid = ulid::Ulid::from_string(session_id)
        .with_context(|| format!("Invalid session ULID: {session_id}"))?;
    let timestamp_ms = i64::try_from(ulid.timestamp_ms())
        .context("ULID timestamp exceeds supported chrono range")?;
    DateTime::from_timestamp_millis(timestamp_ms)
        .context("ULID timestamp is outside chrono supported range")
}
