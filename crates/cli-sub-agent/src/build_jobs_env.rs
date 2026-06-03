use std::collections::HashMap;

pub(crate) const CARGO_BUILD_JOBS_ENV: &str = "CARGO_BUILD_JOBS";
pub(crate) const NEXTEST_TEST_THREADS_ENV: &str = "NEXTEST_TEST_THREADS";

pub(crate) fn apply_build_jobs_env(
    extra_env: &mut Option<HashMap<String, String>>,
    build_jobs: Option<u32>,
) {
    apply_build_jobs_env_with(extra_env, build_jobs, |key| std::env::var(key).ok());
}

pub(crate) fn apply_build_jobs_env_with(
    extra_env: &mut Option<HashMap<String, String>>,
    build_jobs: Option<u32>,
    inherited_env: impl Fn(&str) -> Option<String>,
) {
    let mut updates = HashMap::new();
    if let Some(build_jobs) = build_jobs {
        let value = build_jobs.to_string();
        updates.insert(CARGO_BUILD_JOBS_ENV.to_string(), value.clone());
        updates.insert(NEXTEST_TEST_THREADS_ENV.to_string(), value);
    } else {
        if let Some(value) = inherited_env(CARGO_BUILD_JOBS_ENV) {
            updates.insert(CARGO_BUILD_JOBS_ENV.to_string(), value);
        }
        if let Some(value) = inherited_env(NEXTEST_TEST_THREADS_ENV) {
            updates.insert(NEXTEST_TEST_THREADS_ENV.to_string(), value);
        }
    }

    if !updates.is_empty() {
        extra_env.get_or_insert_with(HashMap::new).extend(updates);
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
}
