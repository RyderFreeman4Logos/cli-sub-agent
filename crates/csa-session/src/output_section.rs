//! Structured output section types for progressive loading.
//!
//! Sessions emit structured output via marker-delimited sections.
//! An index file (`output/index.toml`) lists sections with metadata,
//! and each section is stored as a separate file (`output/<id>.md`).

use serde::{Deserialize, Serialize};

/// A single section of structured session output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputSection {
    /// Section identifier (e.g., "summary", "details", "implementation").
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Start line in output.log (inclusive, 1-based).
    pub line_start: usize,
    /// End line in output.log (inclusive, 1-based).
    pub line_end: usize,
    /// Approximate token count for this section.
    pub token_estimate: usize,
    /// Relative path in the output/ directory (e.g., "summary.md").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
}

/// Index of all structured sections in a session's output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputIndex {
    /// Ordered list of output sections.
    #[serde(default)]
    pub sections: Vec<OutputSection>,
    /// Total estimated tokens across all sections.
    pub total_tokens: usize,
    /// Total lines in output.log.
    pub total_lines: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_section(id: &str, title: &str) -> OutputSection {
        OutputSection {
            id: id.to_string(),
            title: title.to_string(),
            line_start: 1,
            line_end: 50,
            token_estimate: 1200,
            file_path: Some(format!("{id}.md")),
        }
    }

    #[test]
    fn test_output_section_toml_round_trip() {
        let section = sample_section("summary", "Executive Summary");
        let toml_str = toml::to_string(&section).expect("serialize");
        let restored: OutputSection = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(section, restored);
    }

    #[test]
    fn test_output_section_without_file_path() {
        let section = OutputSection {
            id: "inline".to_string(),
            title: "Inline Section".to_string(),
            line_start: 10,
            line_end: 20,
            token_estimate: 300,
            file_path: None,
        };
        let toml_str = toml::to_string(&section).expect("serialize");
        assert!(
            !toml_str.contains("file_path"),
            "None file_path should be skipped"
        );
        let restored: OutputSection = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(section, restored);
    }

    #[test]
    fn test_output_index_toml_round_trip() {
        let index = OutputIndex {
            sections: vec![
                sample_section("summary", "Summary"),
                sample_section("details", "Implementation Details"),
            ],
            total_tokens: 2400,
            total_lines: 100,
        };
        let toml_str = toml::to_string(&index).expect("serialize");
        let restored: OutputIndex = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(index, restored);
    }

    #[test]
    fn test_output_index_empty_sections() {
        let index = OutputIndex {
            sections: vec![],
            total_tokens: 0,
            total_lines: 0,
        };
        let toml_str = toml::to_string(&index).expect("serialize");
        let restored: OutputIndex = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(index, restored);
        assert!(restored.sections.is_empty());
    }
}
