use crate::dag::{DependencyGraph, DependencyNode};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

fn default_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpicPlan {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub epic: EpicMeta,
    #[serde(default)]
    pub stories: Vec<Story>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpicMeta {
    pub name: String,
    pub prefix: String,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Story {
    pub id: String,
    pub branch: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub summary: String,
    #[serde(default)]
    pub status: StoryStatus,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoryStatus {
    #[default]
    Pending,
    InProgress,
    Merged,
    Skipped,
}

impl std::fmt::Display for StoryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "inprogress"),
            Self::Merged => write!(f, "merged"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

impl EpicPlan {
    pub fn validate(&self) -> Result<()> {
        let mut ids = BTreeSet::new();
        for story in &self.stories {
            if !ids.insert(story.id.as_str()) {
                bail!("Duplicate story id: {}", story.id);
            }
        }

        for story in &self.stories {
            for dependency in &story.depends_on {
                if !ids.contains(dependency.as_str()) {
                    bail!(
                        "Story '{}' depends on unknown story id '{}'",
                        story.id,
                        dependency
                    );
                }
            }
        }

        let graph = self.to_dependency_graph();
        let _ = graph.topological_sort()?;
        Ok(())
    }

    pub fn to_dependency_graph(&self) -> DependencyGraph {
        let nodes: Vec<DependencyNode> = self
            .stories
            .iter()
            .map(|story| DependencyNode {
                title: story.id.clone(),
                is_done: story.status == StoryStatus::Merged,
            })
            .collect();

        let id_to_index: BTreeMap<&str, usize> = self
            .stories
            .iter()
            .enumerate()
            .map(|(index, story)| (story.id.as_str(), index))
            .collect();

        let edges = self
            .stories
            .iter()
            .enumerate()
            .flat_map(|(to_index, story)| {
                let id_to_index = &id_to_index;
                story
                    .depends_on
                    .iter()
                    .filter_map(move |dependency| id_to_index.get(dependency.as_str()))
                    .map(move |from_index| (*from_index, to_index))
            });

        DependencyGraph::from_trusted_nodes_and_edges(nodes, edges)
    }

    pub fn execution_order(&self) -> Result<Vec<&Story>> {
        self.validate()?;
        self.to_dependency_graph()
            .topological_sort()?
            .into_iter()
            .map(|index| {
                self.stories
                    .get(index)
                    .with_context(|| format!("Story index {index} missing from epic plan"))
            })
            .collect()
    }

    pub fn next_actionable(&self) -> Vec<&Story> {
        let status_by_id: BTreeMap<&str, StoryStatus> = self
            .stories
            .iter()
            .map(|story| (story.id.as_str(), story.status))
            .collect();

        self.stories
            .iter()
            .filter(|story| story.status == StoryStatus::Pending)
            .filter(|story| {
                story.depends_on.iter().all(|dependency| {
                    status_by_id.get(dependency.as_str()) == Some(&StoryStatus::Merged)
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaleSignals {
    pub new_modules: u32,
    pub new_pub_apis: u32,
    pub new_config_sections: u32,
    pub new_cli_subcommands: u32,
    pub cross_crate_deps: u32,
    pub design_doc_pages: u32,
}

impl ScaleSignals {
    pub fn signals_fired(&self) -> u32 {
        u32::from(self.new_modules > 2)
            + u32::from(self.new_pub_apis > 5)
            + u32::from(self.new_config_sections > 2)
            + u32::from(self.new_cli_subcommands > 1)
            + u32::from(self.cross_crate_deps > 3)
            + u32::from(self.design_doc_pages > 5)
    }

    pub fn is_epic(&self) -> bool {
        self.signals_fired() >= 3
    }
}

#[cfg(test)]
mod tests {
    use super::{EpicMeta, EpicPlan, ScaleSignals, Story, StoryStatus};

    fn sample_plan() -> EpicPlan {
        EpicPlan {
            schema_version: 1,
            epic: EpicMeta {
                name: "JJ sidecar".to_string(),
                prefix: "feat/jj-sidecar".to_string(),
                summary: "Split sidecar support into independent stories.".to_string(),
            },
            stories: vec![
                Story {
                    id: "phase-1a".to_string(),
                    branch: "feat/jj-sidecar/phase-1a-core-trait".to_string(),
                    depends_on: Vec::new(),
                    summary: "Add core trait.".to_string(),
                    status: StoryStatus::Merged,
                },
                Story {
                    id: "phase-1b".to_string(),
                    branch: "feat/jj-sidecar/phase-1b-cli".to_string(),
                    depends_on: vec!["phase-1a".to_string()],
                    summary: "Add CLI wiring.".to_string(),
                    status: StoryStatus::Pending,
                },
            ],
        }
    }

    #[test]
    fn epic_plan_toml_roundtrip() {
        let plan = sample_plan();

        let toml = toml::to_string_pretty(&plan).expect("epic plan should serialize to TOML");
        let decoded: EpicPlan =
            toml::from_str(&toml).expect("epic plan should deserialize from TOML");

        assert_eq!(decoded, plan);
    }

    #[test]
    fn story_status_defaults_to_pending() {
        let toml = r#"
id = "phase-1a"
branch = "feat/example/phase-1a"
summary = "Missing status should default to pending."
"#;

        let decoded: Story =
            toml::from_str(toml).expect("story should deserialize with default status");

        assert_eq!(decoded.status, StoryStatus::Pending);
    }

    #[test]
    fn validate_rejects_duplicate_story_ids() {
        let mut plan = sample_plan();
        plan.stories[1].id = "phase-1a".to_string();

        let err = plan.validate().expect_err("duplicate ids should fail");

        assert!(err.to_string().contains("Duplicate story id: phase-1a"));
    }

    #[test]
    fn validate_rejects_unknown_dependencies() {
        let mut plan = sample_plan();
        plan.stories[1].depends_on = vec!["missing".to_string()];

        let err = plan
            .validate()
            .expect_err("unknown dependencies should fail");

        assert!(
            err.to_string()
                .contains("Story 'phase-1b' depends on unknown story id 'missing'")
        );
    }

    #[test]
    fn validate_rejects_cycles() {
        let mut plan = sample_plan();
        plan.stories[0].depends_on = vec!["phase-1b".to_string()];

        let err = plan.validate().expect_err("cycles should fail");

        assert!(err.to_string().contains("Dependency cycle detected"));
    }

    #[test]
    fn execution_order_returns_dependencies_first() {
        let plan = sample_plan();

        let order = plan
            .execution_order()
            .expect("valid plan should have execution order");

        assert_eq!(order[0].id, "phase-1a");
        assert_eq!(order[1].id, "phase-1b");
    }

    #[test]
    fn next_actionable_returns_pending_stories_with_merged_dependencies() {
        let plan = sample_plan();

        let actionable = plan.next_actionable();

        assert_eq!(actionable.len(), 1);
        assert_eq!(actionable[0].id, "phase-1b");
    }

    #[test]
    fn scale_signals_require_three_thresholds() {
        let signals = ScaleSignals {
            new_modules: 3,
            new_pub_apis: 6,
            new_config_sections: 2,
            new_cli_subcommands: 1,
            cross_crate_deps: 4,
            design_doc_pages: 5,
        };

        assert_eq!(signals.signals_fired(), 3);
        assert!(signals.is_epic());
    }
}
