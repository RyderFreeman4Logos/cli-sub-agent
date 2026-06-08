pub(super) fn is_global_only_key(key: &str) -> bool {
    has_section(
        key,
        &["caller_hints", "experimental", "kv_cache", "state_dir"],
    )
}

pub(super) fn global_key_prefers_raw_lookup(key: &str) -> bool {
    has_section(key, &["caller_hints", "kv_cache", "state_dir"])
}

fn has_section(key: &str, sections: &[&str]) -> bool {
    key.split('.')
        .next()
        .is_some_and(|section| sections.contains(&section))
}
