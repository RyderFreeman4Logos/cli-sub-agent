//! Transport failover override helpers applied to session execution.

use csa_core::env::NO_FAILOVER_ENV_KEY;

pub(super) fn apply_transport_failover_overrides(
    execute_options: &mut csa_executor::ExecuteOptions,
    merged_env: Option<&std::collections::HashMap<String, String>>,
) {
    if merged_env.is_some_and(|env| env.contains_key(NO_FAILOVER_ENV_KEY)) {
        execute_options.acp_crash_max_attempts = 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_transport_failover_overrides_disables_acp_crash_retry() {
        let mut execute_options =
            csa_executor::ExecuteOptions::new(csa_process::StreamMode::BufferOnly, 60);
        let env =
            std::collections::HashMap::from([(NO_FAILOVER_ENV_KEY.to_string(), "1".to_string())]);

        apply_transport_failover_overrides(&mut execute_options, Some(&env));

        assert_eq!(execute_options.acp_crash_max_attempts, 1);
    }

    #[test]
    fn apply_transport_failover_overrides_preserves_default_without_flag() {
        let mut execute_options =
            csa_executor::ExecuteOptions::new(csa_process::StreamMode::BufferOnly, 60);

        apply_transport_failover_overrides(&mut execute_options, None);

        assert_eq!(execute_options.acp_crash_max_attempts, 2);
    }
}
