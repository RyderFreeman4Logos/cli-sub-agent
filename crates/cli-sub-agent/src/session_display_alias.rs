use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionAliasDisplay {
    pub(crate) requested_session_id: String,
    pub(crate) target_session_id: String,
}

impl SessionAliasDisplay {
    pub(crate) fn render_text_line(&self) -> String {
        format!(
            "Alias: kind=resume-wrapper requested_session_id={} target_session_id={}",
            self.requested_session_id, self.target_session_id
        )
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "kind": "resume-wrapper",
            "requested_session_id": self.requested_session_id,
            "target_session_id": self.target_session_id,
        })
    }
}

pub(crate) fn alias_for_display_session(
    session_dir: &Path,
    requested_session_id: &str,
) -> Option<SessionAliasDisplay> {
    let target_session_id = session_dir.file_name()?.to_str()?;
    if target_session_id == requested_session_id {
        return None;
    }
    if csa_session::validate_session_id(target_session_id).is_err()
        || csa_session::validate_session_id(requested_session_id).is_err()
    {
        return None;
    }
    Some(SessionAliasDisplay {
        requested_session_id: requested_session_id.to_string(),
        target_session_id: target_session_id.to_string(),
    })
}

pub(crate) fn text_lines(session_dir: &Path, requested_session_id: &str) -> Vec<String> {
    alias_for_display_session(session_dir, requested_session_id)
        .map(|alias| {
            vec![
                format!("Target session: {}", alias.target_session_id),
                alias.render_text_line(),
            ]
        })
        .unwrap_or_default()
}

pub(crate) fn apply_json_identity(
    payload: &mut serde_json::Value,
    session_dir: &Path,
    requested_session_id: &str,
) {
    payload["session_id"] = serde_json::Value::String(requested_session_id.to_string());
    if let Some(alias) = alias_for_display_session(session_dir, requested_session_id) {
        payload["target_session_id"] = serde_json::Value::String(alias.target_session_id.clone());
        payload["alias"] = alias.to_json();
    }
}
