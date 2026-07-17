/// Batch configuration loaded from TOML file.
#[derive(Debug, Deserialize)]
struct BatchConfig {
    tasks: Vec<BatchTask>,
}

/// A single task in the batch.
#[derive(Debug, Clone, Deserialize)]
struct BatchTask {
    /// Task name (unique identifier)
    name: String,

    /// Tool to use (opencode, codex, claude-code)
    tool: String,

    /// Task prompt
    prompt: String,

    /// Execution mode: sequential (default) or parallel
    #[serde(default)]
    mode: TaskMode,

    /// Task dependencies (must complete before this task starts)
    #[serde(default)]
    depends_on: Vec<String>,

    /// Optional model override
    #[serde(default)]
    model: Option<String>,
}

/// Task execution mode.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum TaskMode {
    #[default]
    Sequential,
    Parallel,
}

/// Task execution result.
#[derive(Debug)]
struct TaskResult {
    name: String,
    exit_code: i32,
    duration_secs: f64,
    error: Option<String>,
}

struct BatchTaskExecutionContext<'a> {
    project_root: &'a Path,
    config: Option<&'a ProjectConfig>,
    global_config: &'a csa_config::GlobalConfig,
    model_catalog: &'a csa_config::EffectiveModelCatalog,
    resource_guard: &'a mut Option<ResourceGuard>,
    resource_overrides: crate::run_resource_overrides::RunResourceOverrides,
    level: usize,
    seq: usize,
    startup_env: &'a StartupSubtreeEnv,
}

struct BatchExecutionContext<'a> {
    project_root: &'a Path,
    config: Option<Arc<ProjectConfig>>,
    global_config: Arc<csa_config::GlobalConfig>,
    model_catalog: Arc<csa_config::EffectiveModelCatalog>,
    resource_overrides: crate::run_resource_overrides::RunResourceOverrides,
    startup_env: &'a StartupSubtreeEnv,
}
