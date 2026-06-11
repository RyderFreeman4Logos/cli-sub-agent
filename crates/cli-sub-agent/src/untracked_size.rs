//! Bounded sizing of untracked working-tree files.
//!
//! Untracked (never-staged) files never appear in `git diff HEAD`, so any size
//! measure derived solely from that diff is blind to brand-new files. Two call
//! sites need them counted: the review-aware writer guard (#1842) and the
//! `csa review` uncommitted diff-size report (#1818). Both share the same hazard
//! — an untracked set is attacker/accident-shaped (huge blobs, binaries, device
//! nodes, an unbounded number of files) — so the enumeration and the per-file
//! line counter live here once, with hard resource caps, rather than being
//! duplicated per call site.
//!
//! All work is bounded regardless of file size, type, or count: files are
//! enumerated with `git ls-files --others --exclude-standard` (so `.gitignore`d
//! build artifacts are never scanned), non-regular entries are skipped without
//! being opened, oversized files are recorded by size instead of read, line
//! counting streams through a fixed buffer and stops at a per-file cap, and the
//! number of files scanned is itself capped. Any single unreadable/race-deleted
//! file is tolerated (skipped), never fatal.

use std::collections::BTreeSet;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Per-file byte ceiling. A regular file larger than this is recorded as
/// "large, not line-counted" (its byte size only) instead of being read, so a
/// pathological multi-GiB untracked blob can never force an unbounded read.
const MAX_LINE_SCAN_BYTES: u64 = 1 << 20; // 1 MiB

/// Per-file line ceiling. Line counting stops here and the count is flagged
/// capped, bounding the work spent on an adversarial file that is mostly
/// newlines (e.g. millions of empty lines under the byte ceiling).
const MAX_LINES_PER_FILE: usize = 50_000;

/// Prefix window scanned for a NUL byte to classify a file as binary. Matches
/// git's own heuristic (a NUL in the first few KiB ⇒ binary) and keeps the
/// sniff bounded independent of file size.
const BINARY_SNIFF_BYTES: u64 = 8 * 1024; // 8 KiB

/// Streaming read buffer. Fixes peak per-file memory to a constant regardless
/// of file size — the file is never slurped whole.
const READ_BUF_BYTES: usize = 16 * 1024; // 16 KiB

/// Total untracked files line-counted before the scan is truncated. Beyond this
/// the scan stops and the report is marked truncated, so an untracked set with
/// an arbitrarily large number of files can never spin an unbounded loop.
pub(crate) const MAX_UNTRACKED_FILES: usize = 1_000;

/// Aggregate size contribution of the untracked working-tree files, ready to be
/// merged into a [`csa_session::ReviewDiffSize`]. `bytes` is the on-disk size of
/// the scanned regular files (including large/binary ones, whose lines are not
/// counted). `notes` is empty when every counted total is exact; otherwise it
/// carries human-readable markers stating which totals are a lower bound
/// (capped, binary/large, or truncated), so a reader is never misled into
/// treating an estimate as exact. `lower_bound` is the structured equivalent of
/// those notes for callers that need a boolean gate instead of report text.
pub(crate) struct UntrackedDiffSize {
    pub(crate) files: usize,
    pub(crate) lines: usize,
    pub(crate) bytes: u64,
    pub(crate) lower_bound: bool,
    pub(crate) notes: Vec<String>,
}

/// Size the untracked, non-ignored working-tree files under `project_root` with
/// every resource cap in this module enforced. Returns an all-zero, note-free
/// result when `project_root` is not a git worktree or has no untracked files
/// (git failure is fail-open, never fatal).
pub(crate) fn untracked_diff_size(project_root: &Path) -> UntrackedDiffSize {
    untracked_diff_size_with_filter(project_root, None)
}

/// Size only untracked files whose repo-relative path appears in `path_filter`.
pub(crate) fn untracked_diff_size_for_paths(
    project_root: &Path,
    path_filter: &BTreeSet<String>,
) -> UntrackedDiffSize {
    untracked_diff_size_with_filter(project_root, Some(path_filter))
}

fn untracked_diff_size_with_filter(
    project_root: &Path,
    path_filter: Option<&BTreeSet<String>>,
) -> UntrackedDiffSize {
    let listing = list_untracked(project_root);

    let mut out = UntrackedDiffSize {
        files: 0,
        lines: 0,
        bytes: 0,
        lower_bound: false,
        notes: Vec::new(),
    };
    let mut capped_files = 0usize;
    let mut uncounted_files = 0usize; // large + binary: sized but not line-counted

    for path in &listing.paths {
        if let Some(filter) = path_filter
            && !untracked_path_matches_filter(project_root, path, filter)
        {
            continue;
        }
        match classify_untracked_file(path) {
            FileClass::Text {
                lines,
                capped,
                bytes,
            } => {
                out.files += 1;
                out.lines = out.lines.saturating_add(lines);
                out.bytes = out.bytes.saturating_add(bytes);
                if capped {
                    capped_files += 1;
                }
            }
            FileClass::Large { bytes } | FileClass::Binary { bytes } => {
                out.files += 1;
                out.bytes = out.bytes.saturating_add(bytes);
                uncounted_files += 1;
            }
            FileClass::Skipped => {}
        }
    }

    if uncounted_files > 0 {
        out.lower_bound = true;
        out.notes.push(format!(
            "{uncounted_files} untracked file(s) not line-counted (binary or > {} MiB); changed-line total is a lower bound",
            MAX_LINE_SCAN_BYTES >> 20
        ));
    }
    if capped_files > 0 {
        out.lower_bound = true;
        out.notes.push(format!(
            "{capped_files} untracked file(s) line-counted up to {MAX_LINES_PER_FILE} lines (capped); changed-line total is a lower bound"
        ));
    }
    if listing.truncated {
        out.lower_bound = true;
        out.notes.push(format!(
            "untracked scan truncated: sized the first {MAX_UNTRACKED_FILES} untracked files (the working tree has more, not enumerated); totals are a lower bound"
        ));
    }
    out
}

fn untracked_path_matches_filter(
    project_root: &Path,
    path: &Path,
    path_filter: &BTreeSet<String>,
) -> bool {
    let rel_path = path.strip_prefix(project_root).unwrap_or(path);
    let rel = rel_path.to_string_lossy();
    path_filter.contains(rel.as_ref())
}

/// A bounded enumeration of untracked, non-ignored working-tree paths.
///
/// `paths` holds at most [`MAX_UNTRACKED_FILES`] absolute paths; `truncated` is
/// set when the working tree held more and enumeration stopped early, so the
/// surplus files were never listed, allocated, or sized.
pub(crate) struct UntrackedListing {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) truncated: bool,
}

/// Untracked, non-ignored working-tree paths under `project_root`, absolute and
/// hard-capped at [`MAX_UNTRACKED_FILES`].
///
/// `git ls-files --others --exclude-standard` lists files git is neither
/// tracking nor ignoring, so `.gitignore`d paths and indexed entries (including
/// intent-to-add, which `git diff HEAD` already counts) are excluded — no
/// double-counting and no inflation from build artifacts. `-z` NUL-delimits the
/// paths so embedded newlines stay intact.
///
/// The enumeration is streamed, not buffered: git's stdout is read one
/// NUL-delimited path at a time and, once the cap is reached, the read handle is
/// dropped and the `git ls-files` child is killed and reaped so it stops walking
/// an arbitrarily large (attacker- or accident-shaped) tree. Peak memory and the
/// returned `Vec` are therefore bounded to ~the cap regardless of how many
/// untracked files exist — the cap is enforced *during* enumeration, never after
/// a full collection. Fail-open: returns an empty, untruncated listing when
/// `project_root` is not a git worktree or git cannot be spawned.
pub(crate) fn list_untracked(project_root: &Path) -> UntrackedListing {
    let Ok(mut child) = Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return UntrackedListing {
            paths: Vec::new(),
            truncated: false,
        };
    };
    let Some(stdout) = child.stdout.take() else {
        // Unreachable with `Stdio::piped()`, but never leave a child unreaped.
        let _ = child.kill();
        let _ = child.wait();
        return UntrackedListing {
            paths: Vec::new(),
            truncated: false,
        };
    };

    // Parse up to the cap, then drop the read end (closing the pipe) before
    // terminating git. Git's exit status is intentionally NOT inspected: on
    // truncation we kill it on purpose, so a "killed" status is expected and the
    // paths already collected stay valid.
    let (rels, truncated) =
        read_nul_delimited_capped(std::io::BufReader::new(stdout), MAX_UNTRACKED_FILES);
    let _ = child.kill(); // benign if git already exited on its own
    let _ = child.wait(); // reap to avoid a zombie

    UntrackedListing {
        paths: rels.into_iter().map(|rel| project_root.join(rel)).collect(),
        truncated,
    }
}

/// Read up to `cap` NUL-delimited entries from `reader`, one at a time, so a
/// caller streaming from a subprocess can stop the producer before it emits an
/// unbounded amount. Empty entries are skipped (defensive: `git ls-files -z`
/// does not emit them). Entries are decoded lossily because git can surface
/// non-UTF-8 paths — matching the previous whole-output `from_utf8_lossy`.
///
/// Returns the collected entries and whether the cap was reached with at least
/// one more entry pending (`truncated`). At most `cap + 1` entries are consumed:
/// the one-entry probe distinguishes "exactly `cap` entries" (not truncated)
/// from "more than `cap`" (truncated) without draining the remaining stream, so
/// an arbitrarily large input is never fully read. A mid-stream read error ends
/// the scan fail-open with whatever was collected.
fn read_nul_delimited_capped<R: BufRead>(mut reader: R, cap: usize) -> (Vec<String>, bool) {
    let mut entries = Vec::new();
    let mut segment = Vec::new();
    loop {
        segment.clear();
        match reader.read_until(0u8, &mut segment) {
            Ok(0) => return (entries, false), // EOF: at most `cap` entries ⇒ not truncated
            Ok(_) => {}
            Err(_) => return (entries, false), // fail-open with what we have
        }
        if segment.last() == Some(&0u8) {
            segment.pop(); // strip the NUL terminator
        }
        if segment.is_empty() {
            continue;
        }
        if entries.len() == cap {
            return (entries, true); // this is the (cap + 1)-th entry: truncate here
        }
        entries.push(String::from_utf8_lossy(&segment).into_owned());
    }
}

/// Per-file outcome of [`classify_untracked_file`].
enum FileClass {
    /// Regular text file line-counted. `capped` is set when the count hit
    /// [`MAX_LINES_PER_FILE`] and is therefore a lower bound.
    Text {
        lines: usize,
        capped: bool,
        bytes: u64,
    },
    /// Regular file above [`MAX_LINE_SCAN_BYTES`]: sized but not read.
    Large { bytes: u64 },
    /// Regular file with a NUL byte in its prefix: sized but not line-counted.
    Binary { bytes: u64 },
    /// Non-regular (symlink/FIFO/socket/device/dir) or unreadable/race-deleted:
    /// contributes nothing and is tolerated, never fatal.
    Skipped,
}

/// Classify one untracked path for the review diff-size report, enforcing the
/// byte ceiling (large files are not read), binary detection (no bogus line
/// counts), and the per-file line cap. A stat/open/read error is folded into
/// [`FileClass::Skipped`] so a single unreadable file never aborts the scan.
fn classify_untracked_file(path: &Path) -> FileClass {
    let Some(meta) = regular_file_meta(path) else {
        return FileClass::Skipped; // non-regular, or stat error (race/EACCES)
    };
    let bytes = meta.len();
    if bytes > MAX_LINE_SCAN_BYTES {
        return FileClass::Large { bytes }; // never opened: bounded by skipping the read
    }
    let Ok(file) = std::fs::File::open(path) else {
        return FileClass::Skipped; // raced between stat and open
    };
    match scan_file_bounded(file, MAX_LINES_PER_FILE) {
        None => FileClass::Skipped, // read error mid-stream
        Some(scan) if scan.saw_nul => FileClass::Binary { bytes },
        Some(scan) => FileClass::Text {
            lines: scan.lines,
            capped: scan.hit_line_cap,
            bytes,
        },
    }
}

/// Metadata for `path` iff it is a regular file, classified WITHOUT following
/// symlinks (`symlink_metadata`) so a symlink is never followed to its target
/// and a non-regular type is never opened. `None` on a stat error or any
/// non-regular type.
fn regular_file_meta(path: &Path) -> Option<std::fs::Metadata> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.file_type().is_file())
}

/// Result of a bounded newline scan of a regular file.
struct BoundedScan {
    /// Newlines counted (plus one for a trailing partial line), saturating at
    /// `line_cap` when `hit_line_cap` is set.
    lines: usize,
    /// The scan stopped at `line_cap`; `lines` is a lower bound.
    hit_line_cap: bool,
    /// A NUL byte appeared within the first [`BINARY_SNIFF_BYTES`].
    saw_nul: bool,
}

/// Stream `file` counting `\n`, bounded by [`MAX_LINE_SCAN_BYTES`] and
/// `line_cap`, sniffing for a NUL within the first [`BINARY_SNIFF_BYTES`]. The
/// caller must have confirmed `file` is a regular file. Returns `None` on a read
/// error so the caller can skip the file.
fn scan_file_bounded(file: std::fs::File, line_cap: usize) -> Option<BoundedScan> {
    use std::io::Read;

    let mut reader = std::io::BufReader::new(file);
    let mut buf = [0u8; READ_BUF_BYTES];
    let mut lines = 0usize;
    let mut scanned: u64 = 0;
    let mut last_byte: Option<u8> = None;
    let mut saw_nul = false;
    let mut hit_line_cap = false;

    while scanned < MAX_LINE_SCAN_BYTES {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => return None,
        };
        let chunk = buf.get(..n)?;

        if !saw_nul && scanned < BINARY_SNIFF_BYTES {
            let window = ((BINARY_SNIFF_BYTES - scanned) as usize).min(n);
            if let Some(prefix) = chunk.get(..window) {
                saw_nul = prefix.contains(&0u8);
            }
        }

        for &byte in chunk {
            if byte == b'\n' {
                lines = lines.saturating_add(1);
                if lines >= line_cap {
                    hit_line_cap = true;
                    break;
                }
            }
        }
        last_byte = chunk.last().copied();
        scanned = scanned.saturating_add(n as u64);
        if hit_line_cap {
            break;
        }
    }

    // A non-empty file whose final scanned byte is not a newline has a trailing
    // partial line. Skip this when the line cap was hit: the count is already a
    // lower bound, so adding a partial line on top would be meaningless.
    if !hit_line_cap && matches!(last_byte, Some(byte) if byte != b'\n') {
        lines = lines.saturating_add(1);
    }

    Some(BoundedScan {
        lines,
        hit_line_cap,
        saw_nul,
    })
}

#[cfg(test)]
#[path = "untracked_size_tests.rs"]
mod tests;
