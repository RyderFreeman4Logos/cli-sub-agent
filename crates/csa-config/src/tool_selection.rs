//! Tool selection type for review/debate configuration.
//!
//! Supports both single tool string and whitelist array.

use serde::{Deserialize, Serialize};

/// Tool selection for review/debate: supports both single string and whitelist array.
///
/// TOML examples:
/// ```toml
/// tool = "codex"                      # Single: always use codex
/// tool = ["codex", "gemini-cli"]      # Whitelist: auto-select from these
/// tool = []                           # Empty: same as "auto"
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum ToolSelection {
    /// A single tool name or "auto".
    Single(String),
    /// Whitelist of allowed tool names. Empty = auto (all enabled tools).
    Whitelist(Vec<String>),
}

impl ToolSelection {
    /// Returns true if this is the default "auto" selection.
    pub fn is_auto(&self) -> bool {
        match self {
            Self::Single(s) => s == "auto",
            Self::Whitelist(v) => v.is_empty(),
        }
    }

    /// Returns the whitelist of allowed tools, if any.
    ///
    /// - `Single("auto")` → `None` (no restriction)
    /// - `Single("codex")` → `None` (direct selection, not a filter)
    /// - `Whitelist(["codex", "gemini-cli"])` → `Some(&["codex", "gemini-cli"])`
    /// - `Whitelist([])` → `None` (empty = no restriction)
    pub fn whitelist(&self) -> Option<&[String]> {
        match self {
            Self::Whitelist(v) if !v.is_empty() => Some(v),
            _ => None,
        }
    }

    /// Returns the single tool name if directly specified (not "auto").
    pub fn as_single(&self) -> Option<&str> {
        match self {
            Self::Single(s) if s != "auto" => Some(s),
            _ => None,
        }
    }

    /// Check if a tool name is allowed by this selection.
    ///
    /// Returns true when: no whitelist (Single or empty Whitelist), or tool is in whitelist.
    pub fn allows(&self, tool: &str) -> bool {
        match self.whitelist() {
            Some(list) => list.iter().any(|t| t == tool),
            None => true,
        }
    }
}

impl Default for ToolSelection {
    fn default() -> Self {
        Self::Single("auto".to_string())
    }
}

impl std::fmt::Display for ToolSelection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Single(s) => write!(f, "{s}"),
            Self::Whitelist(v) if v.is_empty() => write!(f, "auto"),
            Self::Whitelist(v) => write!(f, "[{}]", v.join(", ")),
        }
    }
}

impl Serialize for ToolSelection {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Single(s) => serializer.serialize_str(s),
            Self::Whitelist(v) => v.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ToolSelection {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de;

        struct ToolSelectionVisitor;

        impl<'de> de::Visitor<'de> for ToolSelectionVisitor {
            type Value = ToolSelection;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a tool name string or an array of tool names")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
                Ok(ToolSelection::Single(value.to_string()))
            }

            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut tools = Vec::new();
                while let Some(tool) = seq.next_element::<String>()? {
                    tools.push(tool);
                }
                Ok(ToolSelection::Whitelist(tools))
            }
        }

        deserializer.deserialize_any(ToolSelectionVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_auto_is_auto() {
        assert!(ToolSelection::default().is_auto());
        assert!(ToolSelection::Single("auto".to_string()).is_auto());
    }

    #[test]
    fn single_codex_is_not_auto() {
        assert!(!ToolSelection::Single("codex".to_string()).is_auto());
    }

    #[test]
    fn empty_whitelist_is_auto() {
        assert!(ToolSelection::Whitelist(vec![]).is_auto());
    }

    #[test]
    fn whitelist_returns_tools() {
        let sel = ToolSelection::Whitelist(vec!["codex".into(), "gemini-cli".into()]);
        assert!(!sel.is_auto());
        assert_eq!(sel.whitelist().unwrap().len(), 2);
    }

    #[test]
    fn as_single_returns_none_for_auto() {
        assert!(ToolSelection::default().as_single().is_none());
    }

    #[test]
    fn as_single_returns_tool_name() {
        assert_eq!(
            ToolSelection::Single("codex".to_string()).as_single(),
            Some("codex")
        );
    }

    #[test]
    fn allows_checks_whitelist() {
        let sel = ToolSelection::Whitelist(vec!["codex".into(), "gemini-cli".into()]);
        assert!(sel.allows("codex"));
        assert!(sel.allows("gemini-cli"));
        assert!(!sel.allows("claude-code"));
    }

    #[test]
    fn allows_auto_allows_everything() {
        assert!(ToolSelection::default().allows("anything"));
    }

    #[test]
    fn display_single() {
        assert_eq!(ToolSelection::Single("codex".into()).to_string(), "codex");
    }

    #[test]
    fn display_whitelist() {
        let sel = ToolSelection::Whitelist(vec!["codex".into(), "gemini-cli".into()]);
        assert_eq!(sel.to_string(), "[codex, gemini-cli]");
    }

    #[test]
    fn display_empty_whitelist() {
        assert_eq!(ToolSelection::Whitelist(vec![]).to_string(), "auto");
    }

    /// Helper wrapper for TOML serde tests.
    #[derive(Debug, Deserialize, Serialize)]
    struct Wrapper {
        tool: ToolSelection,
    }

    #[test]
    fn serde_roundtrip_single() {
        let w = Wrapper {
            tool: ToolSelection::Single("codex".into()),
        };
        let toml_str = toml::to_string(&w).unwrap();
        let parsed: Wrapper = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.tool, w.tool);
    }

    #[test]
    fn serde_roundtrip_whitelist() {
        let toml_str = r#"tool = ["codex", "gemini-cli"]"#;
        let parsed: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(
            parsed.tool,
            ToolSelection::Whitelist(vec!["codex".into(), "gemini-cli".into()])
        );
    }

    #[test]
    fn serde_empty_array() {
        let toml_str = r#"tool = []"#;
        let parsed: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(parsed.tool, ToolSelection::Whitelist(vec![]));
        assert!(parsed.tool.is_auto());
    }
}
