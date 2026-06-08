use std::collections::HashMap;

pub(crate) const CARGO_BUILD_JOBS_ENV: &str = "CARGO_BUILD_JOBS";
pub(crate) const NEXTEST_TEST_THREADS_ENV: &str = "NEXTEST_TEST_THREADS";
const AUTO_BUILD_JOB_MEMORY_BUDGET_MB: u64 = 8192;
const LOW_FREE_MEMORY_MB: u64 = 2048;
const LOW_FREE_MEMORY_BUILD_JOBS: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BuildJobsHostMemory {
    pub(crate) available_mb: u64,
    pub(crate) free_mb: u64,
}

pub(crate) fn apply_build_jobs_env(
    extra_env: &mut Option<HashMap<String, String>>,
    build_jobs: Option<u32>,
) {
    apply_build_jobs_env_with_host(
        extra_env,
        build_jobs,
        |key| std::env::var(key).ok(),
        detect_build_jobs_host_memory(),
    );
}

pub(crate) fn build_jobs_env(build_jobs: Option<u32>) -> Option<HashMap<String, String>> {
    build_jobs_env_with_host(
        build_jobs,
        |key| std::env::var(key).ok(),
        detect_build_jobs_host_memory(),
    )
}

#[cfg(test)]
pub(crate) fn apply_build_jobs_env_with(
    extra_env: &mut Option<HashMap<String, String>>,
    build_jobs: Option<u32>,
    inherited_env: impl Fn(&str) -> Option<String>,
) {
    if let Some(updates) = build_jobs_env_with(build_jobs, inherited_env) {
        extra_env.get_or_insert_with(HashMap::new).extend(updates);
    }
}

#[cfg(test)]
pub(crate) fn build_jobs_env_with(
    build_jobs: Option<u32>,
    inherited_env: impl Fn(&str) -> Option<String>,
) -> Option<HashMap<String, String>> {
    build_jobs_env_with_host(build_jobs, inherited_env, None)
}

pub(crate) fn apply_build_jobs_env_with_host(
    extra_env: &mut Option<HashMap<String, String>>,
    build_jobs: Option<u32>,
    inherited_env: impl Fn(&str) -> Option<String>,
    host_memory: Option<BuildJobsHostMemory>,
) {
    if let Some(updates) = build_jobs_env_with_host(build_jobs, inherited_env, host_memory) {
        extra_env.get_or_insert_with(HashMap::new).extend(updates);
    }
}

pub(crate) fn build_jobs_env_with_host(
    build_jobs: Option<u32>,
    inherited_env: impl Fn(&str) -> Option<String>,
    host_memory: Option<BuildJobsHostMemory>,
) -> Option<HashMap<String, String>> {
    let mut updates = HashMap::new();
    if let Some(build_jobs) = build_jobs {
        let value = build_jobs.to_string();
        updates.insert(CARGO_BUILD_JOBS_ENV.to_string(), value.clone());
        updates.insert(NEXTEST_TEST_THREADS_ENV.to_string(), value);
    } else {
        let auto_jobs = host_memory.and_then(auto_build_jobs_cap);
        if let Some(value) =
            inherited_env(CARGO_BUILD_JOBS_ENV).or_else(|| auto_jobs.map(|jobs| jobs.to_string()))
        {
            updates.insert(CARGO_BUILD_JOBS_ENV.to_string(), value);
        }
        if let Some(value) = inherited_env(NEXTEST_TEST_THREADS_ENV)
            .or_else(|| auto_jobs.map(|jobs| jobs.to_string()))
        {
            updates.insert(NEXTEST_TEST_THREADS_ENV.to_string(), value);
        }
    }

    if updates.is_empty() {
        None
    } else {
        Some(updates)
    }
}

fn detect_build_jobs_host_memory() -> Option<BuildJobsHostMemory> {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    let available_mb = sys.available_memory() / 1024 / 1024;
    let free_mb = sys.free_memory() / 1024 / 1024;
    if available_mb == 0 {
        None
    } else {
        Some(BuildJobsHostMemory {
            available_mb,
            free_mb,
        })
    }
}

fn auto_build_jobs_cap(memory: BuildJobsHostMemory) -> Option<u32> {
    if memory.available_mb == 0 {
        return None;
    }

    let jobs_from_available = (memory.available_mb / AUTO_BUILD_JOB_MEMORY_BUDGET_MB)
        .max(1)
        .min(u64::from(u32::MAX)) as u32;

    if memory.free_mb > 0 && memory.free_mb < LOW_FREE_MEMORY_MB {
        Some(jobs_from_available.clamp(1, LOW_FREE_MEMORY_BUILD_JOBS))
    } else {
        Some(jobs_from_available)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inherited_env(key: &str) -> Option<String> {
        match key {
            CARGO_BUILD_JOBS_ENV => Some("3".to_string()),
            NEXTEST_TEST_THREADS_ENV => Some("5".to_string()),
            _ => None,
        }
    }

    #[test]
    fn explicit_build_jobs_overrides_build_and_nextest_env() {
        let mut env = Some(HashMap::from([
            (CARGO_BUILD_JOBS_ENV.to_string(), "99".to_string()),
            (NEXTEST_TEST_THREADS_ENV.to_string(), "99".to_string()),
        ]));

        apply_build_jobs_env_with(&mut env, Some(4), inherited_env);
        let env = env.expect("explicit build jobs should keep env map");

        assert_eq!(env.get(CARGO_BUILD_JOBS_ENV).map(String::as_str), Some("4"));
        assert_eq!(
            env.get(NEXTEST_TEST_THREADS_ENV).map(String::as_str),
            Some("4")
        );
    }

    #[test]
    fn absent_build_jobs_propagates_inherited_env_without_synthesizing() {
        let mut env = None;

        apply_build_jobs_env_with(&mut env, None, |key| match key {
            CARGO_BUILD_JOBS_ENV => Some("3".to_string()),
            _ => None,
        });
        let env = env.expect("inherited build jobs should create env map");

        assert_eq!(env.get(CARGO_BUILD_JOBS_ENV).map(String::as_str), Some("3"));
        assert!(!env.contains_key(NEXTEST_TEST_THREADS_ENV));
    }

    #[test]
    fn absent_build_jobs_preserves_absent_env() {
        let mut env = None;

        apply_build_jobs_env_with(&mut env, None, |_| None);

        assert!(env.is_none());
    }

    #[test]
    fn absent_build_jobs_propagates_inherited_nextest_threads_when_set() {
        let mut env = None;

        apply_build_jobs_env_with(&mut env, None, inherited_env);
        let env = env.expect("inherited build jobs should create env map");

        assert_eq!(env.get(CARGO_BUILD_JOBS_ENV).map(String::as_str), Some("3"));
        assert_eq!(
            env.get(NEXTEST_TEST_THREADS_ENV).map(String::as_str),
            Some("5")
        );
    }

    #[test]
    fn build_jobs_env_returns_explicit_gate_env() {
        let env = build_jobs_env_with(Some(2), inherited_env).expect("explicit env");

        assert_eq!(env.get(CARGO_BUILD_JOBS_ENV).map(String::as_str), Some("2"));
        assert_eq!(
            env.get(NEXTEST_TEST_THREADS_ENV).map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn build_jobs_env_returns_inherited_gate_env() {
        let env = build_jobs_env_with(None, |key| match key {
            CARGO_BUILD_JOBS_ENV => Some("3".to_string()),
            _ => None,
        })
        .expect("inherited env");

        assert_eq!(env.get(CARGO_BUILD_JOBS_ENV).map(String::as_str), Some("3"));
        assert!(!env.contains_key(NEXTEST_TEST_THREADS_ENV));
    }

    #[test]
    fn build_jobs_env_returns_none_when_absent() {
        let env = build_jobs_env_with(None, |_| None);

        assert!(env.is_none());
    }

    #[test]
    fn auto_build_jobs_caps_from_available_memory() {
        let jobs = auto_build_jobs_cap(BuildJobsHostMemory {
            available_mb: 18_000,
            free_mb: 900,
        });

        assert_eq!(jobs, Some(2));
    }

    #[test]
    fn auto_build_jobs_applies_when_env_is_absent() {
        let env = build_jobs_env_with_host(
            None,
            |_| None,
            Some(BuildJobsHostMemory {
                available_mb: 18_000,
                free_mb: 900,
            }),
        )
        .expect("auto env");

        assert_eq!(env.get(CARGO_BUILD_JOBS_ENV).map(String::as_str), Some("2"));
        assert_eq!(
            env.get(NEXTEST_TEST_THREADS_ENV).map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn inherited_env_takes_precedence_over_auto_cap_per_key() {
        let env = build_jobs_env_with_host(
            None,
            |key| match key {
                CARGO_BUILD_JOBS_ENV => Some("6".to_string()),
                _ => None,
            },
            Some(BuildJobsHostMemory {
                available_mb: 18_000,
                free_mb: 900,
            }),
        )
        .expect("mixed env");

        assert_eq!(env.get(CARGO_BUILD_JOBS_ENV).map(String::as_str), Some("6"));
        assert_eq!(
            env.get(NEXTEST_TEST_THREADS_ENV).map(String::as_str),
            Some("2")
        );
    }
}
