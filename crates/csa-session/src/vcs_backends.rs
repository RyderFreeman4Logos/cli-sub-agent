//! Concrete VCS backend implementations for git and jj.
//!
//! # Security Model
//!
//! All commands use `std::process::Command` with `.args()` for argument passing,
//! which prevents shell injection by design. Arguments are passed as separate
//! OS strings, never through shell expansion. User-provided inputs (commit
//! messages, paths) are passed via `.args()` or `.arg()`, not string interpolation.
//! Template strings for jj `-T` flags are hardcoded constants.

use csa_core::vcs::{VcsBackend, VcsIdentity, VcsKind};
use std::path::Path;
use std::process::{Command, Output};

/// Git VCS backend.
pub struct GitBackend;

impl VcsBackend for GitBackend {
    fn kind(&self) -> VcsKind {
        VcsKind::Git
    }

    fn current_branch(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git rev-parse: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let branch = parse_utf8_stdout(output.stdout, "git rev-parse --abbrev-ref HEAD")?;
        if branch.is_empty() || branch == "HEAD" {
            Ok(None)
        } else {
            Ok(Some(branch))
        }
    }

    fn default_branch(&self, project_root: &Path) -> Result<Option<String>, String> {
        if let Some(branch) = git_default_from_origin_head(project_root)? {
            return Ok(Some(branch));
        }
        if let Some(branch) = git_default_from_upstream(project_root)? {
            return Ok(Some(branch));
        }
        if let Some(branch) = git_default_from_init_config(project_root)? {
            return Ok(Some(branch));
        }
        git_default_from_local_heads(project_root)
    }

    fn head_id(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git rev-parse HEAD: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let head = parse_utf8_stdout(output.stdout, "git rev-parse HEAD")?;
        if head.is_empty() {
            Ok(None)
        } else {
            Ok(Some(head))
        }
    }

    fn head_short_id(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git rev-parse --short HEAD: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let short = parse_utf8_stdout(output.stdout, "git rev-parse --short HEAD")?;
        if short.is_empty() {
            Ok(None)
        } else {
            Ok(Some(short))
        }
    }

    fn identity(&self, project_root: &Path) -> Result<VcsIdentity, String> {
        // Single git call to get HEAD SHA, short SHA, and branch in one invocation
        // is not straightforward with git, so we make two calls (rev-parse for SHA+short,
        // rev-parse --abbrev-ref for branch). This is still better than 3 separate calls.
        let commit_id = self.head_id(project_root).ok().flatten();
        let short_id = self.head_short_id(project_root).ok().flatten();
        let ref_name = self.current_branch(project_root).ok().flatten();

        Ok(VcsIdentity {
            vcs_kind: VcsKind::Git,
            commit_id,
            change_id: None, // Git has no logical change identity
            short_id,
            ref_name,
            op_id: None, // Git has no operation log
        })
    }

    fn init(&self, path: &Path) -> Result<(), String> {
        let output = Command::new("git")
            .args(["init"])
            .current_dir(path)
            .output()
            .map_err(|e| format!("Failed to run git init: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "git init failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn add(&self, project_root: &Path, path: &Path) -> Result<(), String> {
        let output = Command::new("git")
            .args(["add", "--"])
            .arg(path)
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git add: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "git add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn commit(&self, project_root: &Path, message: &str) -> Result<(), String> {
        validate_commit_message(message)?;
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git commit: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "git commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn diff_uncommitted(&self, project_root: &Path) -> Result<String, String> {
        let output = Command::new("git")
            .args(["diff"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run git diff: {e}"))?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Jujutsu (jj) VCS backend.
pub struct JjBackend;

impl VcsBackend for JjBackend {
    fn kind(&self) -> VcsKind {
        VcsKind::Jj
    }

    fn current_branch(&self, project_root: &Path) -> Result<Option<String>, String> {
        // jj uses bookmarks instead of branches
        let output = Command::new("jj")
            .args(["bookmark", "list", "--no-pager", "-r", "@"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj bookmark list: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let bookmarks = parse_utf8_stdout(output.stdout, "jj bookmark list")?;
        // First line is the primary bookmark name (before any ':' or whitespace)
        bookmarks
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().next())
            .filter(|s| !s.is_empty())
            .map_or(Ok(None), |b| Ok(Some(b.to_string())))
    }

    fn default_branch(&self, project_root: &Path) -> Result<Option<String>, String> {
        jj_default_from_trunk_config(project_root)
    }

    fn head_id(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("jj")
            .args(["log", "--no-graph", "-r", "@", "-T", "change_id"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj log: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let id = parse_utf8_stdout(output.stdout, "jj log change_id")?;
        if id.is_empty() {
            Ok(None)
        } else {
            Ok(Some(id))
        }
    }

    fn head_short_id(&self, project_root: &Path) -> Result<Option<String>, String> {
        let output = Command::new("jj")
            .args([
                "log",
                "--no-graph",
                "-r",
                "@",
                "-T",
                "change_id.shortest(8)",
            ])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj log: {e}"))?;

        if !output.status.success() {
            return Ok(None);
        }

        let short = parse_utf8_stdout(output.stdout, "jj log short change_id")?;
        if short.is_empty() {
            Ok(None)
        } else {
            Ok(Some(short))
        }
    }

    fn identity(&self, project_root: &Path) -> Result<VcsIdentity, String> {
        // Get change_id, commit_id, and short change_id in one jj call
        let id_output = Command::new("jj")
            .args([
                "log",
                "--no-graph",
                "-r",
                "@",
                "-T",
                r#"change_id ++ "\n" ++ commit_id ++ "\n" ++ change_id.shortest(8)"#,
            ])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj log: {e}"))?;

        let (change_id, commit_id, short_id) = if id_output.status.success() {
            let text = String::from_utf8_lossy(&id_output.stdout);
            let mut lines = text.trim().lines();
            let cid = lines
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let cmid = lines
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let sid = lines
                .next()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            (cid, cmid, sid)
        } else {
            (None, None, None)
        };

        // Get bookmark (branch equivalent)
        let ref_name = self.current_branch(project_root).ok().flatten();

        // Get current operation ID
        let op_output = Command::new("jj")
            .args([
                "op",
                "log",
                "--no-graph",
                "-l",
                "1",
                "-T",
                "self.id().short(12)",
            ])
            .current_dir(project_root)
            .output()
            .ok();
        let op_id = op_output
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        Ok(VcsIdentity {
            vcs_kind: VcsKind::Jj,
            commit_id,
            change_id,
            short_id,
            ref_name,
            op_id,
        })
    }

    fn init(&self, path: &Path) -> Result<(), String> {
        let output = Command::new("jj")
            .args(["git", "init"])
            .current_dir(path)
            .output()
            .map_err(|e| format!("Failed to run jj git init: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "jj git init failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn add(&self, _project_root: &Path, _path: &Path) -> Result<(), String> {
        // jj auto-tracks all files; no explicit staging needed
        Ok(())
    }

    fn commit(&self, project_root: &Path, message: &str) -> Result<(), String> {
        validate_commit_message(message)?;
        let output = Command::new("jj")
            .args(["commit", "-m", message])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj commit: {e}"))?;

        if !output.status.success() {
            return Err(format!(
                "jj commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(())
    }

    fn diff_uncommitted(&self, project_root: &Path) -> Result<String, String> {
        let output = Command::new("jj")
            .args(["diff", "--no-pager"])
            .current_dir(project_root)
            .output()
            .map_err(|e| format!("Failed to run jj diff: {e}"))?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

fn parse_utf8_stdout(stdout: Vec<u8>, context: &str) -> Result<String, String> {
    String::from_utf8(stdout)
        .map(|text| text.trim().to_string())
        .map_err(|err| format!("{context} produced non-UTF-8 output: {err}"))
}

fn git_output(project_root: &Path, args: &[&str]) -> Result<Output, String> {
    Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .map_err(|err| format!("Failed to run git {}: {err}", args.join(" ")))
}

fn normalize_remote_branch_ref(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let branch = trimmed
        .strip_prefix("refs/remotes/origin/")
        .or_else(|| trimmed.strip_prefix("origin/"))
        .or_else(|| trimmed.strip_prefix("refs/heads/"))
        .unwrap_or(trimmed)
        .trim();
    (!branch.is_empty()).then(|| branch.to_string())
}

fn git_default_from_origin_head(project_root: &Path) -> Result<Option<String>, String> {
    let output = git_output(
        project_root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let raw = parse_utf8_stdout(output.stdout, "git symbolic-ref refs/remotes/origin/HEAD")?;
    Ok(normalize_remote_branch_ref(&raw))
}

fn git_default_from_upstream(project_root: &Path) -> Result<Option<String>, String> {
    let output = git_output(
        project_root,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let raw = parse_utf8_stdout(output.stdout, "git rev-parse @{upstream}")?;
    Ok(normalize_remote_branch_ref(&raw))
}

fn git_default_from_init_config(project_root: &Path) -> Result<Option<String>, String> {
    let output = git_output(project_root, &["config", "--get", "init.defaultBranch"])?;
    if !output.status.success() {
        return Ok(None);
    }
    let branch = parse_utf8_stdout(output.stdout, "git config init.defaultBranch")?;
    Ok((!branch.is_empty()).then_some(branch))
}

fn git_default_from_local_heads(project_root: &Path) -> Result<Option<String>, String> {
    for branch in ["main", "master"] {
        let ref_name = format!("refs/heads/{branch}");
        let output = git_output(
            project_root,
            &["show-ref", "--verify", "--quiet", &ref_name],
        )?;
        if output.status.success() {
            return Ok(Some(branch.to_string()));
        }
    }
    Ok(None)
}

fn jj_default_from_trunk_config(project_root: &Path) -> Result<Option<String>, String> {
    let output = match Command::new("jj")
        .args(["config", "get", "revset-aliases.trunk()"])
        .current_dir(project_root)
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };
    if !output.status.success() {
        return Ok(None);
    }
    let raw = parse_utf8_stdout(output.stdout, "jj config get revset-aliases.trunk()")?;
    let candidate = raw
        .trim()
        .trim_matches('"')
        .strip_prefix("bookmarks(")
        .and_then(|value| value.strip_suffix(')'))
        .map(|value| value.trim_matches('"').trim_matches('\'').to_string());
    Ok(candidate.filter(|value| !value.is_empty()))
}

/// Maximum commit message length (bytes). Prevents accidental or malicious oversized messages.
const MAX_COMMIT_MESSAGE_LEN: usize = 65536;

/// Validate a commit message for security and sanity.
fn validate_commit_message(message: &str) -> Result<(), String> {
    if message.contains('\0') {
        return Err("Commit message contains null byte: rejected for security".to_string());
    }
    if message.len() > MAX_COMMIT_MESSAGE_LEN {
        return Err(format!(
            "Commit message too long ({} bytes, max {})",
            message.len(),
            MAX_COMMIT_MESSAGE_LEN
        ));
    }
    Ok(())
}

/// Create the appropriate VcsBackend for the given project root.
///
/// Selection priority: explicit `backend` config > colocated_default > auto-detect.
/// For colocated repos (both `.jj/` and `.git/` present), `colocated_default`
/// overrides auto-detect's jj preference (defaults to Git when unset).
pub fn create_vcs_backend(project_root: &Path) -> Box<dyn VcsBackend> {
    create_vcs_backend_with_config(project_root, None, None)
}

/// Create a VcsBackend with explicit configuration overrides.
pub fn create_vcs_backend_with_config(
    project_root: &Path,
    backend_override: Option<VcsKind>,
    colocated_default: Option<VcsKind>,
) -> Box<dyn VcsBackend> {
    // Explicit override takes top priority
    if let Some(kind) = backend_override {
        return match kind {
            VcsKind::Jj => Box::new(JjBackend),
            VcsKind::Git => Box::new(GitBackend),
        };
    }

    let has_jj = project_root.join(".jj").is_dir();
    let has_git = project_root.join(".git").exists();

    match (has_jj, has_git) {
        // Colocated: both present — use colocated_default (defaults to Git)
        (true, true) => match colocated_default.unwrap_or(VcsKind::Git) {
            VcsKind::Jj => Box::new(JjBackend),
            VcsKind::Git => Box::new(GitBackend),
        },
        // jj only
        (true, false) => Box::new(JjBackend),
        // git only or neither (default to git)
        _ => Box::new(GitBackend),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn create_vcs_backend_defaults_to_git() {
        let temp =
            std::env::temp_dir().join(format!("csa-vcs-backend-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let backend = create_vcs_backend(&temp);
        assert_eq!(backend.kind(), VcsKind::Git);

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn validate_commit_message_rejects_null_byte() {
        let result = validate_commit_message("hello\0world");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("null byte"));
    }

    #[test]
    fn validate_commit_message_rejects_oversized() {
        let msg = "x".repeat(MAX_COMMIT_MESSAGE_LEN + 1);
        let result = validate_commit_message(&msg);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too long"));
    }

    #[test]
    fn validate_commit_message_accepts_normal() {
        assert!(validate_commit_message("feat: add VCS abstraction layer").is_ok());
    }

    #[test]
    fn create_vcs_backend_detects_jj() {
        let temp =
            std::env::temp_dir().join(format!("csa-vcs-backend-test-jj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join(".jj")).unwrap();

        let backend = create_vcs_backend(&temp);
        assert_eq!(backend.kind(), VcsKind::Jj);

        let _ = std::fs::remove_dir_all(&temp);
    }

    fn run_git(project_root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .expect("git should run");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn init_git_repo(project_root: &Path, default_branch: &str) {
        let init_default = format!("init.defaultBranch={default_branch}");
        let output = Command::new("git")
            .args(["-c", &init_default, "init"])
            .current_dir(project_root)
            .output()
            .expect("git init should run");
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        run_git(project_root, &["config", "user.email", "test@example.com"]);
        run_git(project_root, &["config", "user.name", "Test User"]);
    }

    fn commit_file(project_root: &Path) {
        fs::write(project_root.join("file.txt"), "content\n").expect("write test file");
        run_git(project_root, &["add", "file.txt"]);
        run_git(project_root, &["commit", "-m", "initial"]);
    }

    #[test]
    fn git_default_branch_prefers_origin_head() {
        let temp = tempfile::tempdir().expect("tempdir");
        init_git_repo(temp.path(), "main");
        commit_file(temp.path());
        run_git(
            temp.path(),
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
        run_git(
            temp.path(),
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );
        run_git(temp.path(), &["config", "init.defaultBranch", "trunk"]);

        let branch = GitBackend
            .default_branch(temp.path())
            .expect("default branch probe should succeed");

        assert_eq!(branch.as_deref(), Some("main"));
    }

    #[test]
    fn git_default_branch_falls_back_to_init_default_without_remote() {
        let temp = tempfile::tempdir().expect("tempdir");
        init_git_repo(temp.path(), "feature");
        run_git(temp.path(), &["config", "init.defaultBranch", "trunk"]);

        let branch = GitBackend
            .default_branch(temp.path())
            .expect("default branch probe should succeed");

        assert_eq!(branch.as_deref(), Some("trunk"));
    }

    #[test]
    fn git_default_branch_returns_none_when_unrecognized() {
        let temp = tempfile::tempdir().expect("tempdir");
        init_git_repo(temp.path(), "feature");

        let branch = GitBackend
            .default_branch(temp.path())
            .expect("default branch probe should not fail");

        assert_eq!(branch, None);
    }

    #[test]
    fn jj_default_branch_returns_none_when_uncertain() {
        let temp = tempfile::tempdir().expect("tempdir");

        let branch = JjBackend
            .default_branch(temp.path())
            .expect("jj default branch uncertainty should not hard fail");

        assert_eq!(branch, None);
    }
}
