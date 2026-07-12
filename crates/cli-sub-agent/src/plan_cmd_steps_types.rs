/// Step result.
#[derive(Serialize, Deserialize)]
pub(crate) struct StepResult {
    pub(crate) step_id: usize,
    pub(crate) title: String,
    pub(crate) exit_code: i32,
    pub(crate) duration_secs: f64,
    pub(crate) skipped: bool,
    pub(crate) error: Option<String>,
    /// Output as `${STEP_<id>_OUTPUT}`.
    pub(crate) output: Option<String>,
    /// CSA session exposed as `${STEP_<id>_SESSION}`.
    pub(crate) session_id: Option<String>,
    /// Command description for failures.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    /// Final stderr kept out of step output variables.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stderr: Option<String>,
}

pub(super) struct PlanRunContext<'a> {
    pub(super) project_root: &'a Path,
    pub(super) workflow_path: &'a Path,
    pub(super) config: Option<&'a ProjectConfig>,
    pub(super) global_config: &'a csa_config::GlobalConfig,
    pub(super) model_catalog: &'a csa_config::EffectiveModelCatalog,
    pub(super) tool_override: Option<&'a ToolName>,
    pub(super) model_spec_override: Option<&'a String>,
    pub(super) journal: &'a mut PlanRunJournal,
    pub(super) journal_path: Option<&'a Path>,
    pub(super) resume_completed_steps: &'a HashSet<usize>,
    pub(super) chunked: bool,
    pub(super) no_fs_sandbox: bool,
    pub(super) resources: RunResourceOverrides,
    pub(super) startup_env: &'a StartupSubtreeEnv,
}

pub(crate) struct StepExecutionContext<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) workflow_path: &'a Path,
    pub(crate) config: Option<&'a ProjectConfig>,
    pub(crate) global_config: &'a csa_config::GlobalConfig,
    pub(crate) model_catalog: &'a csa_config::EffectiveModelCatalog,
    pub(crate) tool_override: Option<&'a ToolName>,
    pub(crate) model_spec_override: Option<&'a String>,
    pub(crate) no_fs_sandbox: bool,
    pub(crate) resources: RunResourceOverrides,
    pub(crate) startup_env: &'a StartupSubtreeEnv,
}
