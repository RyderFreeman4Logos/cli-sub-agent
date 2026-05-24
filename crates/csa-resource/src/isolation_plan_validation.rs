use std::path::{Path, PathBuf};

use anyhow::Context;

use super::runtime_path::{
    canonicalize_or_fallback, home_dir, is_sensitive_system_path, is_xdg_runtime_child_path,
    normalize_path_components, xdg_runtime_root,
};

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
    )
}

/// Validate that readable paths are safe to expose into the sandbox.
///
/// Read-only binds are stricter than writable paths: every path must be
/// absolute, must exist on disk, `/tmp` itself is forbidden, and symlinked
/// paths are validated against the canonical target to prevent bind-mounting a
/// safe-looking path that resolves somewhere outside the allowlist.
pub fn validate_readable_paths(paths: &[PathBuf], project_root: &Path) -> anyhow::Result<()> {
    validate_sandbox_paths(
        paths,
        project_root,
        PathValidationOptions {
            kind: "readable_paths",
            require_absolute: true,
            require_exists: true,
            reject_tmp_root: true,
            canonicalize_for_allowlist: true,
            allow_requested_path_for_allowlist: false,
            allow_outside_default_roots: false,
        },
    )
    .map(|_| ())
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

fn validate_sandbox_paths(
    paths: &[PathBuf],
    project_root: &Path,
    options: PathValidationOptions<'_>,
) -> anyhow::Result<Vec<PathBuf>> {
    let home = home_dir().unwrap_or_else(|| PathBuf::from("/nonexistent"));
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

        let is_allowed = options.allow_outside_default_roots
            || allowed_parents
                .iter()
                .any(|parent| validated.resolved.starts_with(parent))
            || (options.allow_requested_path_for_allowlist
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
        resolved_paths.push(validated.resolved);
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
    if options.require_absolute && !path.is_absolute() {
        anyhow::bail!("path must be absolute");
    }
    let requested = normalize_path_components(if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    });
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
