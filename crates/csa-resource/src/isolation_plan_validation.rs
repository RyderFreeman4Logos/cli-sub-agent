use std::path::{Path, PathBuf};

use anyhow::Context;

use super::readable::ReadablePath;
use super::runtime_path::{
    canonicalize_or_fallback, home_dir, is_sensitive_system_path, is_xdg_runtime_child_path,
    normalize_path_components, xdg_runtime_root,
};

const SSD_MIRROR_ROOTS: [&str; 1] = ["/ssd/mirror-rootfs"];

/// Strictly validate writable sandbox paths against default safe roots.
/// # Errors
///
/// Returns an error for root, sensitive system paths, or paths outside
/// `project_root`, the user home directory, and `/tmp`.
pub fn validate_writable_paths(paths: &[PathBuf], project_root: &Path) -> anyhow::Result<()> {
    resolve_writable_paths_impl(paths, project_root, false).map(|_| ())
}

pub fn resolve_writable_paths(
    paths: &[PathBuf],
    project_root: &Path,
) -> anyhow::Result<Vec<PathBuf>> {
    resolve_writable_paths_impl(paths, project_root, true)
}

fn resolve_writable_paths_impl(
    paths: &[PathBuf],
    project_root: &Path,
    allow_outside_default_roots: bool,
) -> anyhow::Result<Vec<PathBuf>> {
    validate_sandbox_paths(
        paths,
        project_root,
        PathValidationOptions {
            kind: "writable_paths",
            require_absolute: false,
            require_exists: false,
            reject_tmp_root: false,
            canonicalize_for_allowlist: true,
            allow_requested_path_for_allowlist: true,
            allow_outside_default_roots,
        },
        &[],
    )
    .map(|paths| paths.into_iter().map(|path| path.resolved).collect())
}

/// Validate readable paths and pin each bind source at validation time.
///
/// Read-only binds are stricter than writable paths: every path must exist on
/// disk, `/tmp` itself is forbidden, and symlinked paths are validated against
/// the canonical target to prevent bind-mounting a safe-looking path that
/// resolves somewhere outside the allowlist.
///
/// Project-local relative paths are resolved against `project_root` before
/// validation, so every returned destination is absolute. The requested path
/// remains the mount destination so Bwrap can preserve logical `/tmp` paths
/// (#3074), while `bind_source` stays the validated target (#3102).
///
/// # Errors
///
/// Returns an error listing every rejected path when any path is outside the
/// allowed roots (project root, home dir, `/tmp`), fails to resolve, or is
/// sensitive.
pub fn validate_readable_paths(
    paths: &[PathBuf],
    project_root: &Path,
) -> anyhow::Result<Vec<ReadablePath>> {
    let mirror_roots = default_ssd_mirror_roots();
    validate_readable_paths_with_mirror_roots(paths, project_root, &mirror_roots)
}

pub(super) fn validate_readable_paths_with_mirror_roots(
    paths: &[PathBuf],
    project_root: &Path,
    mirror_roots: &[PathBuf],
) -> anyhow::Result<Vec<ReadablePath>> {
    validate_sandbox_paths(
        paths,
        project_root,
        readable_path_validation_options(),
        mirror_roots,
    )?
    .into_iter()
    .map(|path| {
        ReadablePath::try_pinned(path.requested, path.resolved)
            .with_context(|| "failed to pin validated readable path for race-free snapshots")
    })
    .collect::<anyhow::Result<Vec<_>>>()
}

fn readable_path_validation_options() -> PathValidationOptions<'static> {
    PathValidationOptions {
        kind: "readable_paths",
        require_absolute: true,
        require_exists: true,
        reject_tmp_root: true,
        canonicalize_for_allowlist: true,
        allow_requested_path_for_allowlist: false,
        allow_outside_default_roots: false,
    }
}

/// Canonicalize `path` through its deepest existing ancestor.
/// Missing tail components are re-attached, allowing writable directories that
/// may be pre-created later via `create_dir_all()`.
pub fn canonicalize_through_existing_ancestors(path: &Path) -> anyhow::Result<PathBuf> {
    let mut candidate = path.to_path_buf();
    let mut missing_suffix = Vec::new();

    loop {
        if candidate.as_os_str().is_empty() {
            let mut resolved = std::env::current_dir().with_context(|| {
                format!(
                    "failed to resolve current directory while canonicalizing {}",
                    path.display()
                )
            })?;
            for component in missing_suffix.iter().rev() {
                resolved.push(component);
            }
            return Ok(resolved);
        }

        match candidate.canonicalize() {
            Ok(mut resolved) => {
                for component in missing_suffix.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) => match candidate.try_exists() {
                Ok(true) => {
                    return Err(error).with_context(|| {
                        format!(
                            "failed to canonicalize existing path {} while resolving {}",
                            candidate.display(),
                            path.display()
                        )
                    });
                }
                Ok(false) => {
                    let component = candidate.file_name().with_context(|| {
                        format!(
                            "path {} has no existing ancestor to canonicalize through",
                            path.display()
                        )
                    })?;
                    missing_suffix.push(component.to_os_string());
                    candidate.pop();
                }
                Err(exists_error) => {
                    return Err(exists_error).with_context(|| {
                        format!(
                            "failed to probe path existence while resolving {}",
                            path.display()
                        )
                    });
                }
            },
        }
    }
}

struct PathValidationOptions<'a> {
    kind: &'a str,
    require_absolute: bool,
    require_exists: bool,
    reject_tmp_root: bool,
    canonicalize_for_allowlist: bool,
    allow_requested_path_for_allowlist: bool,
    allow_outside_default_roots: bool,
}

fn default_ssd_mirror_roots() -> [PathBuf; 1] {
    SSD_MIRROR_ROOTS.map(PathBuf::from)
}

fn validate_sandbox_paths(
    paths: &[PathBuf],
    project_root: &Path,
    options: PathValidationOptions<'_>,
    mirror_roots: &[PathBuf],
) -> anyhow::Result<Vec<ValidatedPath>> {
    let home = home_dir().unwrap_or_else(|| PathBuf::from("/nonexistent"));
    let lexical_allowed_roots = [
        normalize_path_components(project_root.to_path_buf()),
        normalize_path_components(home.clone()),
    ];
    let project_root = canonicalize_or_fallback(project_root);
    let project_root_for_join = project_root.clone();
    let home = canonicalize_or_fallback(home.as_path());
    let tmp_root = canonicalize_or_fallback(Path::new("/tmp"));
    let runtime_root = xdg_runtime_root();
    let mut allowed_parents = vec![project_root, home, tmp_root];
    if let Some(runtime_root) = runtime_root.clone() {
        allowed_parents.push(runtime_root);
    }
    let mut rejected = Vec::new();
    let mut resolved_paths = Vec::with_capacity(paths.len());

    for path in paths {
        let validated = match validate_single_path(path, &options, project_root_for_join.as_path())
        {
            Ok(candidate) => candidate,
            Err(reason) => {
                rejected.push(format!("{} ({reason})", path.display()));
                continue;
            }
        };

        if runtime_root
            .as_ref()
            .is_some_and(|root| validated.resolved == *root)
        {
            rejected.push(format!(
                "{} (resolved {}; XDG_RUNTIME_DIR root is too broad; expose a specific child directory such as {}/just)",
                path.display(),
                validated.resolved.display(),
                validated.resolved.display()
            ));
            continue;
        }

        let is_ssd_mirror_candidate = mirror_roots.iter().any(|mirror_root| {
            validated.requested.starts_with(mirror_root)
                || validated
                    .resolved
                    .starts_with(canonicalize_or_fallback(mirror_root))
        });
        let is_allowed = options.allow_outside_default_roots
            || (!is_ssd_mirror_candidate
                && allowed_parents
                    .iter()
                    .any(|parent| validated.resolved.starts_with(parent)))
            || is_ssd_mirror_path_for_allowed_root(
                &validated.requested,
                &validated.resolved,
                &lexical_allowed_roots,
                mirror_roots,
            )
            || (!is_ssd_mirror_candidate
                && options.allow_requested_path_for_allowlist
                && allowed_parents
                    .iter()
                    .any(|parent| validated.requested.starts_with(parent)));
        if !is_allowed {
            rejected.push(format!(
                "{} (resolved {}; outside allowed roots: home, /tmp, project root)",
                path.display(),
                validated.resolved.display()
            ));
            continue;
        }
        resolved_paths.push(validated);
    }

    if rejected.is_empty() {
        return Ok(resolved_paths);
    }

    anyhow::bail!(
        "{} validation failed: rejected paths [{}]. Allowed: subpaths of home dir, /tmp, or project root",
        options.kind,
        rejected.join(", ")
    );
}

fn is_ssd_mirror_path_for_allowed_root(
    requested: &Path,
    resolved: &Path,
    lexical_allowed_roots: &[PathBuf],
    mirror_roots: &[PathBuf],
) -> bool {
    mirror_roots.iter().any(|mirror_root| {
        let canonical_mirror_root = canonicalize_or_fallback(mirror_root);
        let canonical_mirror_suffix = resolved
            .strip_prefix(&canonical_mirror_root)
            .ok()
            .map(|suffix| Path::new("/").join(suffix));
        let maps_to_same_authorized_root = |logical_requested: &Path| {
            lexical_allowed_roots
                .iter()
                .find(|allowed_root| logical_requested.starts_with(allowed_root))
                .is_some_and(|allowed_root| {
                    canonical_mirror_suffix
                        .as_ref()
                        .is_some_and(|logical_resolved| logical_resolved.starts_with(allowed_root))
                })
        };
        let requested_mirror_path_is_allowed = requested
            .strip_prefix(mirror_root)
            .ok()
            .map(|suffix| Path::new("/").join(suffix))
            .is_some_and(|logical_requested| maps_to_same_authorized_root(&logical_requested));
        let allowed_path_resolves_in_mirror = maps_to_same_authorized_root(requested);

        requested_mirror_path_is_allowed || allowed_path_resolves_in_mirror
    })
}

struct ValidatedPath {
    requested: PathBuf,
    resolved: PathBuf,
}

fn validate_single_path(
    path: &Path,
    options: &PathValidationOptions<'_>,
    project_root: &Path,
) -> anyhow::Result<ValidatedPath> {
    if path == Path::new("/") {
        anyhow::bail!("root path is forbidden");
    }
    if options.reject_tmp_root && path == Path::new("/tmp") {
        anyhow::bail!("/tmp itself is forbidden; expose a specific sub-path instead");
    }
    // Resolve project-local relative paths against the project root before the
    // absolute-path validator, so relative `--extra-readable` / `--context`
    // paths under an allowed root are accepted (#3074).
    let requested = normalize_path_components(if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    });
    if options.require_absolute && !requested.is_absolute() {
        anyhow::bail!("path must be absolute");
    }
    if requested == Path::new("/") {
        anyhow::bail!("root path is forbidden");
    }
    if options.reject_tmp_root && requested == Path::new("/tmp") {
        anyhow::bail!("/tmp itself is forbidden; expose a specific sub-path instead");
    }
    let path_exists = !options.require_exists
        || requested.try_exists().with_context(|| {
            format!(
                "failed to probe path '{}' before sandbox launch",
                path.display()
            )
        })?;
    if !path_exists {
        anyhow::bail!(
            "path '{}' does not exist. Create it first or remove the flag.",
            path.display()
        );
    }

    if !options.canonicalize_for_allowlist {
        return Ok(ValidatedPath {
            requested: requested.clone(),
            resolved: requested,
        });
    }

    let resolved = canonicalize_through_existing_ancestors(&requested)?;
    if xdg_runtime_root()
        .as_ref()
        .is_some_and(|root| resolved == *root)
    {
        anyhow::bail!(
            "resolved path {} is forbidden; expose a specific child directory instead",
            resolved.display()
        );
    }
    if is_sensitive_system_path(&resolved) && !is_xdg_runtime_child_path(&resolved) {
        anyhow::bail!("resolved path {} is forbidden", resolved.display());
    }
    Ok(ValidatedPath {
        requested,
        resolved,
    })
}
