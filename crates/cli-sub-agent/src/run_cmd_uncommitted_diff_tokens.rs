use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use csa_config::RunLargeDiffWarningConfig;

pub(super) const DIFF_BYTES_PER_TOKEN: usize = 4;
const DIFF_TOKEN_READ_BUF_BYTES: usize = 16 * 1024;

pub(super) fn estimate_changed_surface_tokens(
    project_root: &Path,
    path_filter: Option<&BTreeSet<String>>,
    untracked: &crate::untracked_size::UntrackedDiffSize,
    tracked_token_threshold: Option<usize>,
) -> usize {
    let untracked_bytes = usize::try_from(untracked.bytes).unwrap_or(usize::MAX);
    let untracked_tokens = untracked_bytes / DIFF_BYTES_PER_TOKEN;
    let Some(token_threshold) = tracked_token_threshold else {
        return untracked_tokens;
    };
    if untracked_tokens > token_threshold {
        return untracked_tokens;
    }

    let remaining_threshold = token_threshold.saturating_sub(untracked_tokens);
    let tracked_tokens =
        estimate_tracked_diff_tokens(project_root, path_filter, remaining_threshold)
            .unwrap_or_default();
    untracked_tokens.saturating_add(tracked_tokens)
}

pub(super) fn tracked_diff_token_threshold(config: &RunLargeDiffWarningConfig) -> usize {
    if config.approx_diff_tokens > 0 {
        config.approx_diff_tokens
    } else {
        default_tracked_diff_token_threshold()
    }
}

pub(super) fn default_tracked_diff_token_threshold() -> usize {
    RunLargeDiffWarningConfig::default().approx_diff_tokens
}

pub(super) struct TrackedDiffTokenEstimate {
    pub(super) tokens: usize,
    pub(super) cap_reached: bool,
}

fn estimate_tracked_diff_tokens(
    project_root: &Path,
    path_filter: Option<&BTreeSet<String>>,
    token_threshold: usize,
) -> Option<usize> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(project_root)
        .args(["diff", "--no-ext-diff", "--no-color", "HEAD"])
        .arg("--")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(filter) = path_filter {
        command.env("GIT_LITERAL_PATHSPECS", "1");
        command.args(filter);
    }

    let mut child = command.spawn().ok()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };

    let estimate = estimate_diff_stream_tokens(stdout, token_threshold)?;
    if estimate.cap_reached {
        let _ = child.kill();
        let _ = child.wait();
        return Some(token_threshold.saturating_add(1));
    }

    let status = child.wait().ok()?;
    status.success().then_some(estimate.tokens)
}

pub(super) fn estimate_diff_stream_tokens<R: Read>(
    mut reader: R,
    token_threshold: usize,
) -> Option<TrackedDiffTokenEstimate> {
    let byte_limit = tracked_diff_byte_limit(token_threshold);
    let mut bytes_read = 0usize;
    let mut buffer = [0u8; DIFF_TOKEN_READ_BUF_BYTES];

    while bytes_read < byte_limit {
        let remaining = byte_limit.saturating_sub(bytes_read);
        let read_len = remaining.min(buffer.len());
        let n = reader.read(&mut buffer[..read_len]).ok()?;
        if n == 0 {
            return Some(TrackedDiffTokenEstimate {
                tokens: diff_bytes_to_approx_tokens(bytes_read),
                cap_reached: false,
            });
        }
        bytes_read = bytes_read.saturating_add(n);
    }

    Some(TrackedDiffTokenEstimate {
        tokens: token_threshold.saturating_add(1),
        cap_reached: true,
    })
}

pub(super) fn tracked_diff_byte_limit(token_threshold: usize) -> usize {
    token_threshold
        .saturating_mul(DIFF_BYTES_PER_TOKEN)
        .saturating_add(1)
}

fn diff_bytes_to_approx_tokens(bytes: usize) -> usize {
    bytes.saturating_add(DIFF_BYTES_PER_TOKEN - 1) / DIFF_BYTES_PER_TOKEN
}
