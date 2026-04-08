use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ABI {
    Unsupported,
}

pub fn apply_landlock_rules(_writable_paths: &[PathBuf]) -> anyhow::Result<()> {
    // Non-Linux platforms do not support Landlock. Callers treat filesystem
    // isolation as best-effort, so degrade gracefully instead of failing
    // process startup.
    Ok(())
}

pub fn detect_abi() -> ABI {
    ABI::Unsupported
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_abi_reports_unsupported() {
        assert_eq!(detect_abi(), ABI::Unsupported);
    }

    #[test]
    fn apply_landlock_rules_is_noop() {
        apply_landlock_rules(&[]).expect("non-linux stub should be best-effort");
    }
}
