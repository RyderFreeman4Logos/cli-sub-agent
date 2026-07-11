use super::*;

pub(super) fn activation_reason(
    diff_size: Option<&ReviewDiffSize>,
    config: &ReviewChunkingConfig,
) -> Option<ReviewChunkActivationReason> {
    match config.mode {
        ReviewChunkingMode::Off => None,
        ReviewChunkingMode::Always => Some(ReviewChunkActivationReason::Always),
        ReviewChunkingMode::Auto => {
            let diff_size = diff_size?;
            if diff_size.files >= config.activate_files {
                Some(ReviewChunkActivationReason::FileCount)
            } else if diff_size.changed_lines > config.activate_changed_lines {
                Some(ReviewChunkActivationReason::ChangedLines)
            } else if diff_size.bytes > config.activate_diff_bytes {
                Some(ReviewChunkActivationReason::DiffBytes)
            } else {
                None
            }
        }
    }
}

pub(super) fn collect_review_chunk_files(
    project_root: &Path,
    scope: &str,
) -> Result<Vec<ReviewChunkFile>> {
    let mut files = collect_numstat_files(project_root, scope)?;
    apply_name_status(project_root, scope, &mut files)?;
    if scope == "uncommitted" {
        append_untracked_files(project_root, &mut files)?;
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files.dedup_by(|left, right| left.path == right.path);
    Ok(files)
}

pub(super) fn collect_numstat_files(
    project_root: &Path,
    scope: &str,
) -> Result<Vec<ReviewChunkFile>> {
    let output = run_git(project_root, &git_diff_args(scope, "--numstat"))?;
    Ok(parse_numstat_output(&output))
}

pub(super) fn apply_name_status(
    project_root: &Path,
    scope: &str,
    files: &mut [ReviewChunkFile],
) -> Result<()> {
    let output = run_git(project_root, &git_diff_args(scope, "--name-status"))?;
    let statuses = parse_name_status_output(&output);
    for file in files {
        if let Some(status) = statuses.get(&file.path) {
            file.status = status.clone();
        }
    }
    Ok(())
}

pub(super) fn append_untracked_files(
    project_root: &Path,
    files: &mut Vec<ReviewChunkFile>,
) -> Result<()> {
    let output = run_git(
        project_root,
        &["ls-files", "--others", "--exclude-standard"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>(),
    )?;
    let existing = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    for path in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !existing.contains(path) {
            files.push(ReviewChunkFile {
                path: path.to_string(),
                status: "A".to_string(),
                changed_lines: 1,
            });
        }
    }
    Ok(())
}

pub(super) fn run_git(project_root: &Path, args: &[String]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(project_root)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(super) fn git_diff_args(scope: &str, mode_flag: &str) -> Vec<String> {
    let mut args = match scope {
        "uncommitted" => vec!["diff".to_string(), "HEAD".to_string()],
        _ if scope.starts_with("range:") => vec![
            "diff".to_string(),
            scope.trim_start_matches("range:").to_string(),
        ],
        _ if scope.starts_with("base:") => vec![
            "diff".to_string(),
            scope.trim_start_matches("base:").to_string(),
        ],
        _ if scope.starts_with("commit:") => vec![
            "show".to_string(),
            "--format=".to_string(),
            scope.trim_start_matches("commit:").to_string(),
        ],
        _ if scope.starts_with("files:") => {
            let mut args = vec!["diff".to_string(), "HEAD".to_string(), "--".to_string()];
            args.extend(
                scope
                    .trim_start_matches("files:")
                    .split_whitespace()
                    .map(str::to_string),
            );
            args
        }
        _ => vec!["diff".to_string(), scope.to_string()],
    };
    let insert_at = 1;
    args.insert(insert_at, mode_flag.to_string());
    args.insert(insert_at + 1, "-M".to_string());
    args.insert(insert_at + 2, "--no-color".to_string());
    args
}

pub(super) fn parse_numstat_output(output: &str) -> Vec<ReviewChunkFile> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let added = fields.next()?;
            let deleted = fields.next()?;
            let path = fields.next()?;
            let changed_lines =
                parse_numstat_count(added).saturating_add(parse_numstat_count(deleted));
            Some(ReviewChunkFile {
                path: normalize_numstat_path(path),
                status: "M".to_string(),
                changed_lines,
            })
        })
        .collect()
}

pub(super) fn parse_numstat_count(raw: &str) -> usize {
    raw.parse::<usize>().unwrap_or(0)
}

pub(super) fn normalize_numstat_path(raw: &str) -> String {
    if let Some((prefix, rename)) = raw.split_once('{')
        && let Some((_, rest)) = rename.split_once("=>")
        && let Some((to, suffix)) = rest.split_once('}')
    {
        return format!("{}{}{}", prefix, to.trim(), suffix);
    }
    if let Some((_, to)) = raw.split_once("=>") {
        return to.trim().to_string();
    }
    raw.to_string()
}

pub(super) fn parse_name_status_output(output: &str) -> BTreeMap<String, String> {
    let mut statuses = BTreeMap::new();
    for line in output.lines() {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() < 2 {
            continue;
        }
        let status = fields[0].chars().next().unwrap_or('M').to_string();
        let path = if status == "R" || status == "C" {
            fields.get(2).copied().unwrap_or(fields[1])
        } else {
            fields[1]
        };
        statuses.insert(path.to_string(), status);
    }
    statuses
}

pub(super) fn plan_review_chunks_from_files(
    scope: &str,
    diff_size: Option<&ReviewDiffSize>,
    files: Vec<ReviewChunkFile>,
    activation_reason: ReviewChunkActivationReason,
    config: &ReviewChunkingConfig,
) -> ReviewChunkPlan {
    let mut grouped = BTreeMap::<String, Vec<ReviewChunkFile>>::new();
    for file in files {
        grouped
            .entry(group_key_for_path(&file.path))
            .or_default()
            .push(file);
    }

    let mut chunks = Vec::new();
    let mut current = Vec::new();
    for (_, mut group_files) in grouped {
        group_files.sort_by(|left, right| left.path.cmp(&right.path));
        let group_chunks = split_group_files(group_files, config);
        for group_chunk in group_chunks {
            if should_start_new_chunk(&current, &group_chunk, config) {
                chunks.push(std::mem::take(&mut current));
            }
            current.extend(group_chunk);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks = cap_chunk_count(chunks, config.max_chunks);

    let chunks = chunks
        .into_iter()
        .enumerate()
        .map(|(idx, files)| build_chunk(idx + 1, files))
        .collect::<Vec<_>>();
    let total_changed_lines = chunks.iter().map(|chunk| chunk.changed_lines).sum();
    let total_files = chunks.iter().map(|chunk| chunk.files.len()).sum();
    let raw_diff_bytes = diff_size.map_or(0, |size| size.bytes);

    ReviewChunkPlan {
        scope: scope.to_string(),
        activation_reason,
        total_files,
        total_changed_lines,
        raw_diff_bytes,
        chunks,
    }
}

pub(super) fn split_group_files(
    files: Vec<ReviewChunkFile>,
    config: &ReviewChunkingConfig,
) -> Vec<Vec<ReviewChunkFile>> {
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    for file in files {
        let next_lines = changed_lines(&current).saturating_add(file.changed_lines);
        let next_files = current.len().saturating_add(1);
        if !current.is_empty()
            && (next_files > config.max_files_per_chunk
                || next_lines > config.max_changed_lines_per_chunk)
        {
            chunks.push(std::mem::take(&mut current));
        }
        current.push(file);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

pub(super) fn should_start_new_chunk(
    current: &[ReviewChunkFile],
    incoming: &[ReviewChunkFile],
    config: &ReviewChunkingConfig,
) -> bool {
    !current.is_empty()
        && (current.len().saturating_add(incoming.len()) > config.target_files_per_chunk
            || changed_lines(current).saturating_add(changed_lines(incoming))
                > config.target_changed_lines_per_chunk)
}

pub(super) fn cap_chunk_count(
    mut chunks: Vec<Vec<ReviewChunkFile>>,
    max_chunks: usize,
) -> Vec<Vec<ReviewChunkFile>> {
    let max_chunks = max_chunks.max(1);
    while chunks.len() > max_chunks {
        let merge_index = chunks
            .windows(2)
            .enumerate()
            .min_by_key(|(_, pair)| {
                pair[0]
                    .len()
                    .saturating_add(pair[1].len())
                    .saturating_add(changed_lines(&pair[0]))
                    .saturating_add(changed_lines(&pair[1]))
            })
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        let right = chunks.remove(merge_index + 1);
        chunks[merge_index].extend(right);
    }
    chunks
}

pub(super) fn build_chunk(id: usize, files: Vec<ReviewChunkFile>) -> ReviewChunk {
    let changed_lines = changed_lines(&files);
    let pathspecs = files.iter().map(|file| file.path.clone()).collect();
    let group = summarize_chunk_group(&files);
    ReviewChunk {
        id,
        group,
        estimated_tokens: estimate_tokens(files.len(), changed_lines),
        files,
        pathspecs,
        changed_lines,
    }
}

pub(super) fn changed_lines(files: &[ReviewChunkFile]) -> usize {
    files.iter().map(|file| file.changed_lines).sum()
}

pub(super) fn estimate_tokens(files: usize, changed_lines: usize) -> usize {
    files
        .saturating_mul(80)
        .saturating_add(changed_lines.saturating_mul(6))
}

pub(super) fn summarize_chunk_group(files: &[ReviewChunkFile]) -> String {
    let groups = files
        .iter()
        .map(|file| group_key_for_path(&file.path))
        .collect::<BTreeSet<_>>();
    if groups.len() == 1 {
        groups.into_iter().next().unwrap_or_else(|| ".".to_string())
    } else {
        "mixed".to_string()
    }
}

pub(super) fn group_key_for_path(path: &str) -> String {
    let path = Path::new(path);
    let mut components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.is_empty() {
        return ".".to_string();
    }
    if components.first() == Some(&"crates") && components.len() >= 2 {
        return format!("crates/{}", components[1]);
    }
    if components.first() == Some(&"src") && components.len() >= 2 {
        components.truncate(2);
        return components.join("/");
    }
    components[0].to_string()
}
