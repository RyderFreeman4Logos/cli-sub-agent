pub(crate) mod diff;
pub(crate) mod hash;
pub(crate) mod io;
pub(crate) mod scan;
pub(crate) mod security;

pub(crate) fn touch_symbols() {
    let _ = io::DEFAULT_MANIFEST_PATH;

    let _load: fn(&std::path::Path) -> anyhow::Result<csa_core::audit::AuditManifest> = io::load;
    let _save: fn(&std::path::Path, &csa_core::audit::AuditManifest) -> anyhow::Result<()> =
        io::save;
    let _hash: fn(&std::path::Path) -> anyhow::Result<String> = hash::hash_file;
    let _scan: fn(&std::path::Path, &[String]) -> anyhow::Result<Vec<std::path::PathBuf>> =
        scan::scan_directory;
    let _validate: fn(&std::path::Path, &std::path::Path) -> anyhow::Result<std::path::PathBuf> =
        security::validate_path;
    let _diff: fn(
        &csa_core::audit::AuditManifest,
        &std::collections::BTreeMap<String, String>,
    ) -> diff::ManifestDiff = diff::diff_manifest;

    let _ = diff::ManifestDiff::default().summary();
}
