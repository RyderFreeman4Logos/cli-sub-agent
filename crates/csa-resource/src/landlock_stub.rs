use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ABI {
    Unsupported,
}

pub fn apply_landlock_rules(_writable_paths: &[PathBuf]) -> anyhow::Result<()> {
    anyhow::bail!("Landlock is only supported on Linux")
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
}
