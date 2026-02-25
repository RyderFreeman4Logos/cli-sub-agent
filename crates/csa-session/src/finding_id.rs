//! Stable finding identifier contract (fid-v1).

use data_encoding::BASE32_NOPAD;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

const FINDING_ID_LENGTH: usize = 26;
const ANCHOR_HASH_LENGTH: usize = 8;

/// Stable identifier for findings used across sessions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FindingId(String);

impl FindingId {
    /// Compute a fid-v1 value from normalized finding fields.
    pub fn compute(
        engine: &str,
        rule_id: &str,
        path: &str,
        symbol: Option<&str>,
        span: Option<&str>,
        cwe: Option<&str>,
        anchor_hash: &str,
    ) -> Self {
        let normalized_path = normalize_path(path);
        let payload = format!(
            "engine={engine}\nrule_id={rule_id}\npath={normalized_path}\nsymbol={}\nspan={}\ncwe={}\nanchor_hash={anchor_hash}",
            symbol.unwrap_or_default(),
            span.unwrap_or_default(),
            cwe.unwrap_or_default()
        );

        let digest = Sha256::digest(payload.as_bytes());
        let encoded = BASE32_NOPAD.encode(&digest);
        Self(encoded[..FINDING_ID_LENGTH].to_string())
    }

    /// Return the identifier as a borrowed string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume this ID and return the inner string.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Display for FindingId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Normalize file paths for stable identifier generation.
///
/// - Strip leading `./` prefixes.
/// - Normalize path separators to `/`.
/// - Collapse repeated `/`.
/// - Remove trailing `/` (except root-like `/`).
pub fn normalize_path(path: &str) -> String {
    let mut normalized = path.replace('\\', "/");

    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }

    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }

    // Resolve internal `.` segments (e.g., "src/./lib.rs" → "src/lib.rs").
    normalized = normalized
        .split('/')
        .filter(|seg| *seg != ".")
        .collect::<Vec<_>>()
        .join("/");

    while normalized.ends_with('/') && normalized.len() > 1 {
        normalized.pop();
    }

    if normalized.is_empty() {
        ".".to_string()
    } else {
        normalized
    }
}

/// Compute an 8-char context anchor hash from trimmed lines.
pub fn anchor_hash(context_lines: &[&str]) -> String {
    let joined = context_lines
        .iter()
        .map(|line| line.trim())
        .collect::<Vec<_>>()
        .join("\n");
    let digest = Sha256::digest(joined.as_bytes());

    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to String cannot fail.
        let _ = write!(&mut hex, "{byte:02x}");
    }
    hex[..ANCHOR_HASH_LENGTH].to_string()
}

#[cfg(test)]
mod tests {
    use super::{FindingId, anchor_hash, normalize_path};

    #[test]
    fn test_compute_produces_consistent_26_char_base32_id() {
        let id = FindingId::compute(
            "semgrep",
            "rust.no-unwrap",
            "./src\\lib.rs/",
            Some("SessionManager"),
            Some("12:5-12:18"),
            Some("CWE-703"),
            "deadbeef",
        )
        .into_inner();

        assert_eq!(id.len(), 26);
        assert!(
            id.chars().all(|ch| matches!(ch, 'A'..='Z' | '2'..='7')),
            "id must be base32 characters only: {id}"
        );
    }

    #[test]
    fn test_compute_is_deterministic_for_same_inputs() {
        let id1 = FindingId::compute(
            "semgrep",
            "rust.no-unwrap",
            "src/lib.rs",
            Some("SessionManager"),
            Some("12:5-12:18"),
            Some("CWE-703"),
            "abcd1234",
        );
        let id2 = FindingId::compute(
            "semgrep",
            "rust.no-unwrap",
            "src/lib.rs",
            Some("SessionManager"),
            Some("12:5-12:18"),
            Some("CWE-703"),
            "abcd1234",
        );

        assert_eq!(id1, id2);
    }

    #[test]
    fn test_compute_changes_when_inputs_change() {
        let id1 = FindingId::compute(
            "semgrep",
            "rust.no-unwrap",
            "src/lib.rs",
            Some("SessionManager"),
            Some("12:5-12:18"),
            Some("CWE-703"),
            "abcd1234",
        );
        let id2 = FindingId::compute(
            "semgrep",
            "rust.no-panic",
            "src/lib.rs",
            Some("SessionManager"),
            Some("12:5-12:18"),
            Some("CWE-703"),
            "abcd1234",
        );

        assert_ne!(id1, id2);
    }

    #[test]
    fn test_normalize_path_handles_prefix_separators_and_trailing_slash() {
        assert_eq!(
            normalize_path("./src\\session\\manager.rs/"),
            "src/session/manager.rs"
        );
        assert_eq!(normalize_path("././src//lib.rs"), "src/lib.rs");
        assert_eq!(
            normalize_path("src\\nested\\\\file.rs"),
            "src/nested/file.rs"
        );
        // Internal dot segments are resolved.
        assert_eq!(normalize_path("src/./lib.rs"), "src/lib.rs");
        assert_eq!(normalize_path("a/./b/./c.rs"), "a/b/c.rs");
    }

    #[test]
    fn test_anchor_hash_stable_for_trimmed_context_lines() {
        let hash1 = anchor_hash(&["  let value = 1;  ", "", " println!(\"{value}\"); "]);
        let hash2 = anchor_hash(&["let value = 1;", "", "println!(\"{value}\");"]);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1, "96ca40d2");
    }

    #[test]
    fn test_path_rename_changes_finding_id() {
        let id1 = FindingId::compute(
            "semgrep",
            "rust.no-unwrap",
            "src/review.rs",
            Some("review_findings"),
            Some("90:3-90:30"),
            Some("CWE-703"),
            "f00dbabe",
        );
        let id2 = FindingId::compute(
            "semgrep",
            "rust.no-unwrap",
            "src/adjudication.rs",
            Some("review_findings"),
            Some("90:3-90:30"),
            Some("CWE-703"),
            "f00dbabe",
        );

        assert_ne!(id1, id2);
    }
}
