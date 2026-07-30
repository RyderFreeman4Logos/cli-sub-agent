use super::soft_limit_usage_bytes;

#[test]
fn cgroup_page_cache_does_not_consume_soft_limit_working_set() {
    let memory_current = 10 * 1024 * 1024;
    let memory_stat = "anon 1048576\nfile 9437184\ninactive_file 8388608\n";

    assert_eq!(
        soft_limit_usage_bytes(memory_current, Some(memory_stat)),
        2 * 1024 * 1024,
        "reclaimable inactive file cache must not trigger a scope soft kill"
    );
}

#[test]
fn malformed_or_incoherent_cgroup_stat_keeps_the_full_soft_limit_charge() {
    let memory_current = 10 * 1024 * 1024;

    assert_eq!(soft_limit_usage_bytes(memory_current, None), memory_current);
    assert_eq!(
        soft_limit_usage_bytes(memory_current, Some("inactive_file not-a-number\n")),
        memory_current
    );
    assert_eq!(
        soft_limit_usage_bytes(memory_current, Some("inactive_file 11534336\n")),
        memory_current,
        "a raced or incoherent counter must not weaken the soft limit"
    );
}
