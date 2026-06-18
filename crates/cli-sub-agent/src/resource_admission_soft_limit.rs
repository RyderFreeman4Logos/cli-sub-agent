use csa_resource::{IsolationPlan, ResourceCapability, memory_policy};

const REVIEWER_SUB_SESSION_TASK_TYPE: &str = "reviewer_sub_session";
// Codex's default profile is 12288MB because the old 8192MB cap still failed
// large Rust workspaces. For reviewer sessions, the monitor threshold itself
// must stay at least at that old cap; otherwise 8192MB at the default 70%
// soft limit recreates #2254's 5734MB no-verdict kill window.
const CODEX_REVIEW_MIN_SOFT_LIMIT_MB: u64 = 8192;
pub(crate) const MEMORY_SOFT_LIMIT_ADMISSION_REASON: &str = "memory_soft_limit_admission";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemorySoftLimitAdmissionError {
    message: String,
}

impl MemorySoftLimitAdmissionError {
    pub(crate) const TERMINATION_REASON: &'static str = MEMORY_SOFT_LIMIT_ADMISSION_REASON;
}

impl std::fmt::Display for MemorySoftLimitAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MemorySoftLimitAdmissionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReviewSoftLimitAdmissionDenial {
    memory_max_mb: u64,
    soft_limit_percent: u8,
    threshold_mb: u64,
    required_threshold_mb: u64,
    required_memory_max_mb: u64,
}

impl ReviewSoftLimitAdmissionDenial {
    fn message(self, tool_name: &str) -> String {
        format!(
            "CSA: {reason} denied -- {tool_name} reviewer soft memory threshold is \
             {threshold}MB, below required={required}MB \
             (memory_max_mb={memory_max}MB, soft_limit_percent={percent}). \
             This review/fix session is likely to be terminated by CSA's memory monitor before \
             producing a verdict. Raise --memory-max-mb, resources.memory_max_mb, or \
             tools.{tool_name}.memory_max_mb to at least {required_memory_max}MB, remove a lower \
             memory override so Codex can use its 12288MB default, or raise \
             resources.soft_limit_percent only when host RAM makes that safe.",
            reason = MEMORY_SOFT_LIMIT_ADMISSION_REASON,
            threshold = self.threshold_mb,
            required = self.required_threshold_mb,
            memory_max = self.memory_max_mb,
            percent = self.soft_limit_percent,
            required_memory_max = self.required_memory_max_mb,
        )
    }
}

pub(crate) fn ensure_review_soft_limit_admission(
    task_type: Option<&str>,
    tool_name: &str,
    isolation_plan: Option<&IsolationPlan>,
) -> Result<(), MemorySoftLimitAdmissionError> {
    let Some(denial) = review_soft_limit_admission_denial(task_type, tool_name, isolation_plan)
    else {
        return Ok(());
    };

    Err(MemorySoftLimitAdmissionError {
        message: denial.message(tool_name),
    })
}

fn review_soft_limit_admission_denial(
    task_type: Option<&str>,
    tool_name: &str,
    isolation_plan: Option<&IsolationPlan>,
) -> Option<ReviewSoftLimitAdmissionDenial> {
    if task_type != Some(REVIEWER_SUB_SESSION_TASK_TYPE) || tool_name != "codex" {
        return None;
    }

    let plan = isolation_plan?;
    if plan.resource != ResourceCapability::CgroupV2 {
        return None;
    }

    let memory_max_mb = plan.memory_max_mb?;
    let soft_limit_percent = plan
        .soft_limit_percent
        .unwrap_or(memory_policy::DEFAULT_SOFT_LIMIT_PERCENT);
    let threshold_mb = memory_policy::soft_limit_threshold_mb(memory_max_mb, soft_limit_percent)?;
    if threshold_mb >= CODEX_REVIEW_MIN_SOFT_LIMIT_MB {
        return None;
    }

    Some(ReviewSoftLimitAdmissionDenial {
        memory_max_mb,
        soft_limit_percent,
        threshold_mb,
        required_threshold_mb: CODEX_REVIEW_MIN_SOFT_LIMIT_MB,
        required_memory_max_mb: memory_policy::required_memory_max_for_soft_limit_mb(
            CODEX_REVIEW_MIN_SOFT_LIMIT_MB,
            soft_limit_percent,
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use csa_resource::FilesystemCapability;

    fn isolation_plan(
        tool_resource: ResourceCapability,
        memory_max_mb: Option<u64>,
        soft_limit_percent: Option<u8>,
    ) -> IsolationPlan {
        IsolationPlan {
            resource: tool_resource,
            filesystem: FilesystemCapability::Bwrap,
            writable_paths: Vec::new(),
            readable_paths: Vec::new(),
            env_overrides: std::collections::HashMap::new(),
            degraded_reasons: Vec::new(),
            memory_max_mb,
            memory_swap_max_mb: None,
            pids_max: None,
            readonly_project_root: true,
            project_root: None,
            soft_limit_percent,
            memory_monitor_interval_seconds: None,
        }
    }

    #[test]
    fn codex_review_soft_limit_admission_denies_8192_at_default_percent() {
        let plan = isolation_plan(ResourceCapability::CgroupV2, Some(8192), None);

        let denial = review_soft_limit_admission_denial(
            Some(REVIEWER_SUB_SESSION_TASK_TYPE),
            "codex",
            Some(&plan),
        )
        .expect("codex reviewer should be denied");

        assert_eq!(denial.threshold_mb, 5734);
        assert_eq!(denial.required_threshold_mb, CODEX_REVIEW_MIN_SOFT_LIMIT_MB);
        assert_eq!(denial.required_memory_max_mb, 11_703);
        let err = ensure_review_soft_limit_admission(
            Some(REVIEWER_SUB_SESSION_TASK_TYPE),
            "codex",
            Some(&plan),
        )
        .expect_err("admission should fail");
        assert_eq!(
            MemorySoftLimitAdmissionError::TERMINATION_REASON,
            MEMORY_SOFT_LIMIT_ADMISSION_REASON
        );
        assert!(err.to_string().contains("--memory-max-mb"));
        assert!(err.to_string().contains("tools.codex.memory_max_mb"));
    }

    #[test]
    fn codex_review_soft_limit_admission_allows_codex_default_limit() {
        let plan = isolation_plan(ResourceCapability::CgroupV2, Some(12_288), None);

        assert_eq!(
            review_soft_limit_admission_denial(
                Some(REVIEWER_SUB_SESSION_TASK_TYPE),
                "codex",
                Some(&plan),
            ),
            None
        );
    }

    #[test]
    fn codex_review_soft_limit_admission_allows_non_review_tasks() {
        let plan = isolation_plan(ResourceCapability::CgroupV2, Some(8192), None);

        assert_eq!(
            review_soft_limit_admission_denial(Some("run"), "codex", Some(&plan)),
            None
        );
    }

    #[test]
    fn codex_review_soft_limit_admission_allows_non_cgroup_plans() {
        let plan = isolation_plan(ResourceCapability::Setrlimit, Some(8192), None);

        assert_eq!(
            review_soft_limit_admission_denial(
                Some(REVIEWER_SUB_SESSION_TASK_TYPE),
                "codex",
                Some(&plan),
            ),
            None
        );
    }

    #[test]
    fn codex_review_soft_limit_admission_honors_custom_soft_limit_percent() {
        let plan = isolation_plan(ResourceCapability::CgroupV2, Some(8192), Some(100));

        assert_eq!(
            review_soft_limit_admission_denial(
                Some(REVIEWER_SUB_SESSION_TASK_TYPE),
                "codex",
                Some(&plan),
            ),
            None
        );
    }
}
