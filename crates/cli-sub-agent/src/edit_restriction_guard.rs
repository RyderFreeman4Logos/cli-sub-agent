use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};

fn git_binary() -> &'static Path {
    static GIT_BINARY: OnceLock<PathBuf> = OnceLock::new();
    GIT_BINARY.get_or_init(|| which::which("git").unwrap_or_else(|_| PathBuf::from("git")))
}

#[derive(Debug)]
pub(crate) struct TrackedFileEditGuard {
    project_root: PathBuf,
    pre_dirty_paths: BTreeSet<PathBuf>,
    pre_dirty_snapshots: HashMap<PathBuf, PathState>,
    pre_staged_patches: HashMap<PathBuf, Vec<u8>>,
}

#[derive(Debug, Clone)]
pub(crate) struct EditRestrictionViolation {
    pub(crate) modified_paths: Vec<PathBuf>,
    pub(crate) restored_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PathState {
    Missing,
    Symlink {
        target: PathBuf,
    },
    Regular {
        bytes: Vec<u8>,
        #[cfg(unix)]
        mode: u32,
        #[cfg(not(unix))]
        readonly: bool,
    },
    Other,
}

impl EditRestrictionViolation {
    pub(crate) fn summary(&self) -> String {
        format!(
            "Edit restriction violated: blocked modifications to {} existing tracked file(s)",
            self.modified_paths.len()
        )
    }

    pub(crate) fn detail_message(&self) -> String {
        let modified = self
            .modified_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let restored = self
            .restored_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "Edit restriction violated (allow_edit_existing_files=false). Modified tracked files: [{modified}]. Restored files: [{restored}]."
        )
    }
}

pub(crate) fn maybe_capture_tracked_file_guard(
    project_root: &Path,
) -> Result<Option<TrackedFileEditGuard>> {
    if !is_git_repo(project_root)? {
        return Ok(None);
    }

    Ok(Some(TrackedFileEditGuard::capture(project_root)?))
}

/// Guard that detects and removes new files created by a tool when
/// `allow_write_new_files = false`.
///
/// Covers three cases:
/// 1. New untracked files that didn't exist before
/// 2. New files that were staged via `git add` (no longer in `--others`)
/// 3. Pre-existing untracked files that were modified
#[derive(Debug)]
pub(crate) struct NewFileGuard {
    project_root: PathBuf,
    pre_untracked: BTreeSet<PathBuf>,
    pre_staged: BTreeSet<PathBuf>,
    pre_untracked_snapshots: HashMap<PathBuf, PathState>,
}

#[derive(Debug)]
pub(crate) struct NewFileViolation {
    pub(crate) new_paths: Vec<PathBuf>,
    pub(crate) removed_paths: Vec<PathBuf>,
}

impl NewFileViolation {
    pub(crate) fn summary(&self) -> String {
        format!(
            "Write restriction violated: blocked creation of {} new file(s)",
            self.new_paths.len()
        )
    }

    pub(crate) fn detail_message(&self) -> String {
        let created = self
            .new_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let removed = self
            .removed_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Write restriction violated (allow_write_new_files=false). \
             New files created: [{created}]. Removed files: [{removed}]."
        )
    }
}

pub(crate) fn maybe_capture_new_file_guard(project_root: &Path) -> Result<Option<NewFileGuard>> {
    if !is_git_repo(project_root)? {
        return Ok(None);
    }

    Ok(Some(NewFileGuard::capture(project_root)?))
}

impl NewFileGuard {
    fn capture(project_root: &Path) -> Result<Self> {
        let pre_untracked = git_untracked_set(project_root)?;
        let pre_staged =
            git_name_only_set(project_root, &["diff", "--name-only", "--cached", "-z"])?;

        // Snapshot pre-existing untracked files to detect modifications.
        let mut pre_untracked_snapshots = HashMap::new();
        for path in &pre_untracked {
            if is_internal_prompt_temp_path(path) {
                continue;
            }
            let snapshot = capture_path_state(&project_root.join(path)).with_context(|| {
                format!("failed to snapshot untracked file '{}'", path.display())
            })?;
            pre_untracked_snapshots.insert(path.clone(), snapshot);
        }

        Ok(Self {
            project_root: project_root.to_path_buf(),
            pre_untracked,
            pre_staged,
            pre_untracked_snapshots,
        })
    }

    /// Detect new files (untracked or staged) and modifications to
    /// pre-existing untracked files, then remove/restore them.
    pub(crate) fn enforce_and_remove(self) -> Result<Option<NewFileViolation>> {
        let post_untracked = git_untracked_set(&self.project_root)?;
        let post_staged = git_name_only_set(
            &self.project_root,
            &["diff", "--name-only", "--cached", "-z"],
        )?;

        let mut violating_paths: BTreeSet<PathBuf> = BTreeSet::new();

        // Case 1: New untracked files that didn't exist before.
        for path in post_untracked.difference(&self.pre_untracked) {
            if is_internal_prompt_temp_path(path) {
                continue;
            }
            violating_paths.insert(path.clone());
        }

        // Case 2: New staged files that weren't staged before (tool ran
        // `git add` on a newly created file, hiding it from --others).
        for path in post_staged.difference(&self.pre_staged) {
            if is_internal_prompt_temp_path(path) {
                continue;
            }
            // Only flag if it wasn't already a pre-existing untracked file
            // (modifications to pre-existing untracked are handled in case 3).
            if !self.pre_untracked.contains(path) {
                violating_paths.insert(path.clone());
            }
        }

        // Case 3: Pre-existing untracked files that were modified.
        for (path, previous_state) in &self.pre_untracked_snapshots {
            if is_internal_prompt_temp_path(path) {
                continue;
            }
            let current_state =
                capture_path_state(&self.project_root.join(path)).with_context(|| {
                    format!("failed to inspect untracked file '{}'", path.display())
                })?;
            if &current_state != previous_state {
                violating_paths.insert(path.clone());
            }
        }

        if violating_paths.is_empty() {
            return Ok(None);
        }

        let mut removed_paths = Vec::new();
        for path in &violating_paths {
            let full_path = self.project_root.join(path);

            // If this was a pre-existing untracked file, restore its snapshot
            // instead of deleting it.
            if let Some(previous_state) = self.pre_untracked_snapshots.get(path) {
                // Unstage if the tool staged it.
                if post_staged.contains(path) && !self.pre_staged.contains(path) {
                    git_restore_paths(&self.project_root, std::slice::from_ref(path), true, false)
                        .with_context(|| format!("failed to unstage file '{}'", path.display()))?;
                }
                restore_path_state(&full_path, previous_state).with_context(|| {
                    format!("failed to restore untracked file '{}'", path.display())
                })?;
                removed_paths.push(path.clone());
                continue;
            }

            // Unstage if the tool staged the new file.
            if post_staged.contains(path) {
                git_restore_paths(&self.project_root, std::slice::from_ref(path), true, false)
                    .with_context(|| format!("failed to unstage new file '{}'", path.display()))?;
            }

            // Remove the new file.
            if full_path.exists() {
                if full_path.is_dir() {
                    fs::remove_dir_all(&full_path).with_context(|| {
                        format!("failed to remove new directory '{}'", full_path.display())
                    })?;
                } else {
                    fs::remove_file(&full_path).with_context(|| {
                        format!("failed to remove new file '{}'", full_path.display())
                    })?;
                }
            }
            removed_paths.push(path.clone());
        }

        Ok(Some(NewFileViolation {
            new_paths: violating_paths.into_iter().collect(),
            removed_paths,
        }))
    }
}

fn git_untracked_set(project_root: &Path) -> Result<BTreeSet<PathBuf>> {
    git_name_only_set(
        project_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
}

fn is_internal_prompt_temp_path(path: &Path) -> bool {
    let mut components = path.components();
    let Some(first_component) = components.next() else {
        return false;
    };
    if first_component.as_os_str() != OsStr::new(".tmp") {
        return false;
    }

    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.ends_with(".prompt.md"))
}

impl TrackedFileEditGuard {
    fn capture(project_root: &Path) -> Result<Self> {
        let pre_staged_paths =
            git_name_only_set(project_root, &["diff", "--name-only", "--cached", "-z"])?;
        let pre_unstaged_paths = git_name_only_set(project_root, &["diff", "--name-only", "-z"])?;

        let pre_dirty_paths = pre_staged_paths
            .union(&pre_unstaged_paths)
            .cloned()
            .collect::<BTreeSet<_>>();

        let mut pre_dirty_snapshots = HashMap::new();
        let mut pre_staged_patches = HashMap::new();
        for path in &pre_dirty_paths {
            let snapshot = capture_path_state(&project_root.join(path))
                .with_context(|| format!("failed to snapshot dirty file '{}'", path.display()))?;
            pre_dirty_snapshots.insert(path.clone(), snapshot);
            let staged_patch =
                git_diff_cached_binary_for_path(project_root, path).with_context(|| {
                    format!("failed to snapshot staged diff for '{}'", path.display())
                })?;
            pre_staged_patches.insert(path.clone(), staged_patch);
        }

        Ok(Self {
            project_root: project_root.to_path_buf(),
            pre_dirty_paths,
            pre_dirty_snapshots,
            pre_staged_patches,
        })
    }

    pub(crate) fn enforce_and_restore(self) -> Result<Option<EditRestrictionViolation>> {
        let post_staged_paths = git_name_only_set(
            &self.project_root,
            &["diff", "--name-only", "--cached", "-z"],
        )?;
        let post_unstaged_paths =
            git_name_only_set(&self.project_root, &["diff", "--name-only", "-z"])?;

        let post_dirty_paths = post_staged_paths
            .union(&post_unstaged_paths)
            .cloned()
            .collect::<BTreeSet<_>>();

        let mut violating_paths = post_dirty_paths
            .difference(&self.pre_dirty_paths)
            .cloned()
            .collect::<BTreeSet<_>>();

        for path in &self.pre_dirty_paths {
            let Some(previous_state) = self.pre_dirty_snapshots.get(path) else {
                continue;
            };
            let current_state = capture_path_state(&self.project_root.join(path))
                .with_context(|| format!("failed to inspect dirty file '{}'", path.display()))?;
            let current_staged_patch = git_diff_cached_binary_for_path(&self.project_root, path)
                .with_context(|| {
                    format!("failed to inspect staged diff for '{}'", path.display())
                })?;
            let previous_staged_patch = self
                .pre_staged_patches
                .get(path)
                .map(Vec::as_slice)
                .unwrap_or_default();
            if &current_state != previous_state
                || current_staged_patch.as_slice() != previous_staged_patch
            {
                violating_paths.insert(path.clone());
            }
        }

        if violating_paths.is_empty() {
            return Ok(None);
        }

        let mut restored_paths = BTreeSet::new();

        let newly_dirty_paths = violating_paths
            .iter()
            .filter(|path| !self.pre_dirty_paths.contains(*path))
            .cloned()
            .collect::<Vec<_>>();

        if !newly_dirty_paths.is_empty() {
            git_restore_paths(&self.project_root, &newly_dirty_paths, true, true)
                .context("failed to restore newly modified tracked files")?;
            for path in &newly_dirty_paths {
                restored_paths.insert(path.clone());
            }
        }

        for path in violating_paths
            .iter()
            .filter(|path| self.pre_dirty_paths.contains(*path))
        {
            let Some(previous_state) = self.pre_dirty_snapshots.get(path) else {
                continue;
            };
            let previous_staged_patch = self
                .pre_staged_patches
                .get(path)
                .map(Vec::as_slice)
                .unwrap_or_default();

            restore_index_state(&self.project_root, path, previous_staged_patch)
                .with_context(|| format!("failed to restore index for '{}'", path.display()))?;
            restore_path_state(&self.project_root.join(path), previous_state)
                .with_context(|| format!("failed to restore dirty file '{}'", path.display()))?;
            restored_paths.insert(path.clone());
        }

        Ok(Some(EditRestrictionViolation {
            modified_paths: violating_paths.into_iter().collect(),
            restored_paths: restored_paths.into_iter().collect(),
        }))
    }
}

fn is_git_repo(project_root: &Path) -> Result<bool> {
    let output = Command::new(git_binary())
        .current_dir("/")
        .arg("-C")
        .arg(project_root)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output()
        .with_context(|| {
            format!(
                "failed to run git rev-parse in '{}'",
                project_root.display()
            )
        })?;

    if !output.status.success() {
        return Ok(false);
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
}

fn git_name_only_set(project_root: &Path, args: &[&str]) -> Result<BTreeSet<PathBuf>> {
    let output = Command::new(git_binary())
        .current_dir("/")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .with_context(|| {
            format!(
                "failed to run git command in '{}': git {}",
                project_root.display(),
                args.join(" ")
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "git {} failed in '{}': {}",
            args.join(" "),
            project_root.display(),
            stderr.trim()
        ));
    }

    Ok(parse_nul_paths(&output.stdout))
}

fn git_restore_paths(
    project_root: &Path,
    paths: &[PathBuf],
    staged: bool,
    worktree: bool,
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut command = Command::new(git_binary());
    command.arg("-C").arg(project_root).arg("restore");
    if staged {
        command.arg("--staged");
    }
    if worktree {
        command.arg("--worktree");
    }
    command.arg("--");
    for path in paths {
        command.arg(path);
    }

    let output = command.output().with_context(|| {
        format!(
            "failed to run git restore in '{}', path_count={}",
            project_root.display(),
            paths.len()
        )
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "git restore failed in '{}': {}",
            project_root.display(),
            stderr.trim()
        ));
    }

    Ok(())
}

fn git_diff_cached_binary_for_path(project_root: &Path, path: &Path) -> Result<Vec<u8>> {
    let output = Command::new(git_binary())
        .current_dir("/")
        .arg("-C")
        .arg(project_root)
        .args(["diff", "--cached", "--binary", "--"])
        .arg(path)
        .output()
        .with_context(|| {
            format!(
                "failed to run git diff --cached in '{}', path='{}'",
                project_root.display(),
                path.display()
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "git diff --cached failed in '{}': {}",
            project_root.display(),
            stderr.trim()
        ));
    }

    Ok(output.stdout)
}

fn restore_index_state(project_root: &Path, path: &PathBuf, staged_patch: &[u8]) -> Result<()> {
    git_restore_paths(project_root, std::slice::from_ref(path), true, false)
        .with_context(|| format!("failed to reset index for '{}'", path.display()))?;
    if staged_patch.is_empty() {
        return Ok(());
    }
    git_apply_cached_patch(project_root, staged_patch)
}

fn git_apply_cached_patch(project_root: &Path, patch: &[u8]) -> Result<()> {
    let mut child = Command::new(git_binary())
        .current_dir("/")
        .arg("-C")
        .arg(project_root)
        .args(["apply", "--cached", "--binary", "--whitespace=nowarn"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn git apply in '{}'", project_root.display()))?;

    child
        .stdin
        .take()
        .context("git apply stdin unavailable")?
        .write_all(patch)
        .context("failed to write staged patch to git apply")?;

    let output = child
        .wait_with_output()
        .context("failed to wait for git apply")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "git apply --cached failed in '{}': {}",
            project_root.display(),
            stderr.trim()
        ));
    }
    Ok(())
}

fn parse_nul_paths(raw: &[u8]) -> BTreeSet<PathBuf> {
    raw.split(|byte| *byte == b'\0')
        .filter(|chunk| !chunk.is_empty())
        .map(|chunk| PathBuf::from(String::from_utf8_lossy(chunk).to_string()))
        .collect()
}

fn capture_path_state(path: &Path) -> Result<PathState> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(PathState::Missing),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to stat '{}'", path.display()));
        }
    };

    if metadata.file_type().is_symlink() {
        let target = fs::read_link(path)
            .with_context(|| format!("failed to read symlink target for '{}'", path.display()))?;
        return Ok(PathState::Symlink { target });
    }

    if metadata.is_file() {
        let bytes = fs::read(path)
            .with_context(|| format!("failed to read file bytes for '{}'", path.display()))?;
        return Ok(PathState::Regular {
            bytes,
            #[cfg(unix)]
            mode: {
                use std::os::unix::fs::PermissionsExt;
                metadata.permissions().mode()
            },
            #[cfg(not(unix))]
            readonly: metadata.permissions().readonly(),
        });
    }

    Ok(PathState::Other)
}

fn restore_path_state(path: &Path, state: &PathState) -> Result<()> {
    remove_existing_path(path)?;

    match state {
        PathState::Missing | PathState::Other => Ok(()),
        PathState::Symlink { target } => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent directory '{}'", parent.display())
                })?;
            }

            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, path).with_context(|| {
                    format!(
                        "failed to restore symlink '{}' -> '{}'",
                        path.display(),
                        target.display()
                    )
                })?;
                Ok(())
            }

            #[cfg(not(unix))]
            {
                let _ = target;
                Err(anyhow!(
                    "restoring symlinks for edit restriction guard is unsupported on this platform"
                ))
            }
        }
        PathState::Regular {
            bytes,
            #[cfg(unix)]
            mode,
            #[cfg(not(unix))]
            readonly,
        } => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent directory '{}'", parent.display())
                })?;
            }

            fs::write(path, bytes)
                .with_context(|| format!("failed to restore file '{}'", path.display()))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(*mode)).with_context(
                    || format!("failed to restore permissions for '{}'", path.display()),
                )?;
            }

            #[cfg(not(unix))]
            {
                let mut permissions = fs::metadata(path)
                    .with_context(|| format!("failed to stat '{}'", path.display()))?
                    .permissions();
                permissions.set_readonly(*readonly);
                fs::set_permissions(path, permissions).with_context(|| {
                    format!("failed to restore permissions for '{}'", path.display())
                })?;
            }

            Ok(())
        }
    }
}

fn remove_existing_path(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| format!("failed to stat '{}'", path.display()));
        }
    };

    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path)
            .with_context(|| format!("failed to remove existing file '{}'", path.display()))?;
        return Ok(());
    }

    if metadata.is_dir() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove existing directory '{}'", path.display()))?;
    }

    Ok(())
}

#[cfg(test)]
#[path = "edit_restriction_guard_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "edit_restriction_guard_tests_tail.rs"]
mod tests_tail;
