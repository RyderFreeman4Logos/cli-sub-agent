use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use csa_session::review_artifact::{Finding, ReviewArtifact};
use serde::{Deserialize, Serialize};

#[path = "bug_class_sanitization.rs"]
mod sanitization;

use sanitization::sanitize_candidate_for_skill;
#[cfg(test)]
pub(crate) use sanitization::{
    SANITIZED_CONTENT_PLACEHOLDER, sanitize_code_for_skill, sanitize_text_for_skill,
};

pub(crate) const CONSOLIDATED_REVIEW_ARTIFACT_FILE: &str = "review-findings-consolidated.json";
const SINGLE_REVIEW_ARTIFACT_FILE: &str = "review-findings.json";
const REVIEW_DETAILS_CONTEXT_FILE: &str = "output/details.md";
const BUG_CLASS_RECURRENCE_THRESHOLD: u32 = 2;
const SKILL_TYPE_CODE_QUALITY: &str = "code-quality";
const SKILL_REFERENCES_DIR: &str = "references";
const SKILL_CASE_STUDIES_FILE: &str = "case-studies.md";
const SKILL_DETAILED_PATTERNS_FILE: &str = "detailed-patterns.md";
const SKILL_MARKDOWN_FILE: &str = "SKILL.md";

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct CaseStudy {
    pub(crate) session_id: String,
    pub(crate) file_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) line_range: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) code_snippet: Option<String>,
    pub(crate) fix_description: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct BugClassCandidate {
    pub(crate) language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) domain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rule_id: Option<String>,
    pub(crate) anti_pattern_category: String,
    pub(crate) preferred_pattern: String,
    #[serde(default)]
    pub(crate) case_studies: Vec<CaseStudy>,
    #[serde(default)]
    pub(crate) recurrence_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CandidateKey {
    rule_id: String,
    language: String,
}

pub(crate) struct SkillExtractor {
    skill_root: PathBuf,
}

impl SkillExtractor {
    pub(crate) fn new(skill_root: PathBuf) -> Self {
        Self { skill_root }
    }

    pub(crate) fn from_global_config() -> Result<Self> {
        let skill_root = csa_config::paths::config_dir_write()
            .context("failed to resolve cli-sub-agent config directory")?
            .join("skills");
        Ok(Self::new(skill_root))
    }

    pub(crate) fn extract(&self, candidates: &[BugClassCandidate]) -> Result<Vec<PathBuf>> {
        let sanitized_candidates = candidates
            .iter()
            .map(sanitize_candidate_for_skill)
            .collect::<Vec<_>>();
        let mut grouped = group_candidates_by_language(&sanitized_candidates);
        let mut written = Vec::new();

        for (language, candidates) in &mut grouped {
            candidates.sort_by(|left, right| {
                left.anti_pattern_category
                    .cmp(&right.anti_pattern_category)
                    .then_with(|| left.rule_id.cmp(&right.rule_id))
                    .then_with(|| left.preferred_pattern.cmp(&right.preferred_pattern))
            });

            let skill_dir = self.skill_root.join(skill_directory_name(language));
            let references_dir = skill_dir.join(SKILL_REFERENCES_DIR);

            write_atomic_if_changed(
                &skill_dir.join(SKILL_MARKDOWN_FILE),
                &render_skill_markdown(language, candidates),
            )?;
            write_atomic_if_changed(
                &references_dir.join(SKILL_DETAILED_PATTERNS_FILE),
                &render_detailed_patterns_markdown(language, candidates),
            )?;
            write_atomic_if_changed(
                &references_dir.join(SKILL_CASE_STUDIES_FILE),
                &render_case_studies_markdown(language, candidates),
            )?;

            written.push(skill_dir);
        }

        Ok(written)
    }
}

impl BugClassCandidate {
    pub(crate) fn aggregate_from_review_artifacts(
        review_artifacts: &[ReviewArtifact],
    ) -> Vec<Self> {
        let mut grouped: BTreeMap<CandidateKey, Self> = BTreeMap::new();

        for artifact in review_artifacts {
            for finding in &artifact.findings {
                let Some(rule_id) = optional_string(&finding.rule_id) else {
                    continue;
                };
                let language = infer_language(&finding.file, &rule_id);
                let domain = infer_domain(&rule_id, &finding.summary);
                let case_study = CaseStudy {
                    session_id: artifact.session_id.clone(),
                    file_path: finding.file.clone(),
                    line_range: finding.line.map(|line| (line, line)),
                    code_snippet: None,
                    // Finding currently carries only the issue summary, so reuse it as the
                    // best available fix note until review artifacts expose structured remediation.
                    fix_description: finding.summary.clone(),
                };
                let key = CandidateKey {
                    rule_id: rule_id.clone(),
                    language: language.clone(),
                };

                match grouped.entry(key) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(Self {
                            language,
                            domain: domain.clone(),
                            rule_id: Some(rule_id.clone()),
                            anti_pattern_category: infer_anti_pattern_category(finding),
                            preferred_pattern: infer_preferred_pattern(finding, domain.as_deref()),
                            case_studies: vec![case_study],
                            recurrence_count: 1,
                        });
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry) => {
                        let candidate = entry.get_mut();
                        if candidate.domain.is_none() {
                            candidate.domain = domain;
                        }
                        candidate.case_studies.push(case_study);
                        candidate.recurrence_count += 1;
                    }
                }
            }
        }

        grouped.into_values().collect()
    }
}

/// Load persisted review artifacts for the given project from CSA session storage.
///
/// Session artifacts are read from `{state_root}/{project_key}/sessions/{session_id}`.
/// Consolidated review output takes precedence; single-reviewer artifacts are used
/// as a fallback for older or non-consolidated sessions. Missing artifacts are
/// skipped silently so partially-populated session directories do not abort mining.
pub(crate) fn load_review_artifacts_for_project(
    project_path: &Path,
) -> Result<Vec<ReviewArtifact>> {
    let mut review_artifacts = Vec::new();

    for session_dir in list_project_session_dirs(project_path)? {
        if let Some(artifact) = load_review_artifact_from_session_dir(&session_dir)? {
            review_artifacts.push(artifact);
        }
    }

    Ok(review_artifacts)
}

/// Promote only bug classes that recur across distinct review sessions.
///
/// `aggregate_from_review_artifacts()` counts raw findings, which can over-count
/// repeated mentions inside a single review. Bug-class promotion uses unique
/// session IDs instead so the recurrence threshold reflects independent reviews.
pub(crate) fn classify_recurring_bug_classes(
    review_artifacts: &[ReviewArtifact],
) -> Vec<BugClassCandidate> {
    let mut candidates = BugClassCandidate::aggregate_from_review_artifacts(review_artifacts);

    for candidate in &mut candidates {
        candidate.recurrence_count = unique_session_count(&candidate.case_studies);
    }

    candidates.retain(|candidate| candidate.recurrence_count >= BUG_CLASS_RECURRENCE_THRESHOLD);
    candidates
}

pub(crate) fn link_bug_class_pipeline_symbols() {
    let _ = load_review_artifacts_for_project as fn(&Path) -> Result<Vec<ReviewArtifact>>;
    let _ = classify_recurring_bug_classes as fn(&[ReviewArtifact]) -> Vec<BugClassCandidate>;
    let _ = SkillExtractor::new as fn(PathBuf) -> SkillExtractor;
    let _ = SkillExtractor::from_global_config as fn() -> Result<SkillExtractor>;
    let _ = SkillExtractor::extract
        as fn(&SkillExtractor, &[BugClassCandidate]) -> Result<Vec<PathBuf>>;
}

fn list_project_session_dirs(project_path: &Path) -> Result<Vec<PathBuf>> {
    let project_keys = project_path_keys(project_path);
    let mut session_dirs = BTreeMap::new();

    for (session_root, _) in csa_session::list_all_project_session_roots()? {
        for session in csa_session::list_sessions_from_root_readonly(&session_root)? {
            if !project_keys.contains(session.project_path.as_str()) {
                continue;
            }

            let session_dir = session_root.join("sessions").join(&session.meta_session_id);
            if session_dir.is_dir() {
                session_dirs
                    .entry(session.meta_session_id)
                    .or_insert(session_dir);
            }
        }
    }

    Ok(session_dirs.into_values().collect())
}

fn project_path_keys(project_path: &Path) -> BTreeSet<String> {
    let mut project_keys = BTreeSet::from([project_path.to_string_lossy().to_string()]);
    if let Ok(canonical) = fs::canonicalize(project_path) {
        project_keys.insert(canonical.to_string_lossy().to_string());
    }
    project_keys
}

fn load_review_artifact_from_session_dir(session_dir: &Path) -> Result<Option<ReviewArtifact>> {
    let _details_context = read_optional_text_file(&session_dir.join(REVIEW_DETAILS_CONTEXT_FILE))?;

    let Some(artifact_path) = review_artifact_path(session_dir) else {
        return Ok(None);
    };

    let artifact_content = fs::read_to_string(&artifact_path)
        .with_context(|| format!("failed to read review artifact {}", artifact_path.display()))?;
    let artifact: ReviewArtifact = serde_json::from_str(&artifact_content).with_context(|| {
        format!(
            "failed to parse review artifact {}",
            artifact_path.display()
        )
    })?;

    Ok(Some(artifact))
}

fn review_artifact_path(session_dir: &Path) -> Option<PathBuf> {
    [
        session_dir.join(CONSOLIDATED_REVIEW_ARTIFACT_FILE),
        session_dir.join(SINGLE_REVIEW_ARTIFACT_FILE),
    ]
    .into_iter()
    .find(|artifact_path| artifact_path.is_file())
}

fn read_optional_text_file(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn unique_session_count(case_studies: &[CaseStudy]) -> u32 {
    case_studies
        .iter()
        .map(|case_study| case_study.session_id.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        .min(u32::MAX as usize) as u32
}

fn optional_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn infer_language(file_path: &str, rule_id: &str) -> String {
    if let Some(ext) = Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        && let Some(language) = canonical_language(ext)
    {
        return language.to_string();
    }

    for token in rule_id.split(['/', '.', ':', '-']) {
        if let Some(language) = canonical_language(token) {
            return language.to_string();
        }
    }

    "unknown".to_string()
}

fn canonical_language(token: &str) -> Option<&'static str> {
    match token.trim().to_ascii_lowercase().as_str() {
        "rs" | "rust" => Some("rust"),
        "go" => Some("go"),
        "py" | "python" => Some("python"),
        "js" | "jsx" | "javascript" => Some("javascript"),
        "ts" | "tsx" | "typescript" => Some("typescript"),
        "java" => Some("java"),
        "kt" | "kts" | "kotlin" => Some("kotlin"),
        "rb" | "ruby" => Some("ruby"),
        "php" => Some("php"),
        "c" | "h" => Some("c"),
        "cc" | "cpp" | "cxx" | "hpp" => Some("cpp"),
        "cs" | "csharp" => Some("csharp"),
        "swift" => Some("swift"),
        "scala" => Some("scala"),
        "sh" | "bash" | "zsh" | "shell" => Some("shell"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "proto" => Some("proto"),
        "sql" => Some("sql"),
        _ => None,
    }
}

fn infer_domain(rule_id: &str, summary: &str) -> Option<String> {
    let lower = format!("{rule_id} {summary}").to_ascii_lowercase();

    for (domain, keywords) in [
        (
            "concurrency",
            &[
                "async",
                "await",
                "mutex",
                "deadlock",
                "thread",
                "race condition",
                "tokio",
            ][..],
        ),
        (
            "error-handling",
            &[
                "unwrap",
                "expect",
                "panic",
                "result",
                "error handling",
                "propagate",
            ][..],
        ),
        (
            "ownership",
            &[
                "ownership",
                "borrow",
                "borrowed",
                "lifetime",
                "move",
                "clone",
            ][..],
        ),
        (
            "resource-management",
            &["drop", "raii", "cleanup", "resource leak", "close handle"][..],
        ),
    ] {
        if keywords.iter().any(|keyword| lower.contains(keyword)) {
            return Some(domain.to_string());
        }
    }

    None
}

fn infer_anti_pattern_category(finding: &Finding) -> String {
    let rule_tail = finding
        .rule_id
        .rsplit(|ch| ['/', '.'].contains(&ch))
        .find(|segment| !segment.trim().is_empty())
        .unwrap_or_default();
    let rule_slug = slugify(rule_tail);
    if !rule_slug.is_empty() && !rule_slug.chars().all(|ch| ch.is_ascii_digit()) {
        return rule_slug;
    }

    let summary_slug = slugify(&finding.summary);
    if !summary_slug.is_empty() {
        return summary_slug;
    }

    "uncategorized".to_string()
}

fn infer_preferred_pattern(finding: &Finding, domain: Option<&str>) -> String {
    let lower = format!("{} {}", finding.rule_id, finding.summary).to_ascii_lowercase();

    if ["unwrap", "expect", "panic"]
        .iter()
        .any(|keyword| lower.contains(keyword))
    {
        return "Return Result and propagate recoverable failures with ? instead of panicking."
            .to_string();
    }

    if lower.contains("mutex") && lower.contains("await") {
        return "Avoid holding synchronous locks across .await; narrow the critical section or redesign ownership."
            .to_string();
    }

    if lower.contains("clone") {
        return "Prefer borrowing existing data when shared ownership is unnecessary.".to_string();
    }

    match domain {
        Some("error-handling") => {
            "Preserve error context and propagate recoverable failures through Result.".to_string()
        }
        Some("concurrency") => {
            "Prefer synchronization patterns that make ownership and lock scope explicit."
                .to_string()
        }
        _ => finding.summary.clone(),
    }
}

fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;

    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

fn group_candidates_by_language(
    candidates: &[BugClassCandidate],
) -> BTreeMap<String, Vec<&BugClassCandidate>> {
    let mut grouped: BTreeMap<String, Vec<&BugClassCandidate>> = BTreeMap::new();

    for candidate in candidates {
        grouped
            .entry(candidate.language.clone())
            .or_default()
            .push(candidate);
    }

    grouped
}

fn skill_directory_name(language: &str) -> String {
    format!("code-quality-{}", slugify(language))
}

fn skill_name(language: &str) -> String {
    skill_directory_name(language)
}

fn render_skill_markdown(language: &str, candidates: &[&BugClassCandidate]) -> String {
    let language_name = display_language_name(language);
    let language_keyword = language.trim().to_ascii_lowercase();
    let extension_hint = language_extension_hint(language);
    let mut markdown = format!(
        "---\nname: {}\ndescription: \"{}\"\ntype: {}\n---\n\n# {} Code Quality Guide\n\nUse this thin routing skill to catch recurring {} issues before they reappear.\n\n## Quick Check Rules\n",
        skill_name(language),
        yaml_escape(&format!(
            "Use when reviewing {language_name} ({language_keyword}) code quality, recurring {language_keyword} bug classes, or {extension_hint} changes."
        )),
        SKILL_TYPE_CODE_QUALITY,
        language_name,
        language_name
    );

    for candidate in candidates {
        let rule = candidate.rule_id.as_deref().unwrap_or("unlabeled rule");
        markdown.push_str(&format!(
            "- Look for {} (rule `{rule}`), recurring across {} review session(s).\n",
            humanize_slug(&candidate.anti_pattern_category),
            candidate.recurrence_count
        ));
    }

    markdown.push_str("\n## Preferred Patterns\n");
    for candidate in candidates {
        markdown.push_str(&format!(
            "- {}: {}\n",
            humanize_slug(&candidate.anti_pattern_category),
            candidate.preferred_pattern
        ));
    }

    markdown.push_str(
        "\n## Progressive Disclosure\n- Open `references/detailed-patterns.md` for recurrence data and remediation context.\n- Open `references/case-studies.md` for concrete examples from prior reviews.\n",
    );

    markdown
}

fn render_detailed_patterns_markdown(language: &str, candidates: &[&BugClassCandidate]) -> String {
    let language_name = display_language_name(language);
    let mut markdown = format!(
        "# {} Detailed Patterns\n\nGenerated from recurring review findings for {} code.\n",
        language_name, language_name
    );

    for candidate in candidates {
        let rule = candidate.rule_id.as_deref().unwrap_or("unlabeled");
        let domain = candidate.domain.as_deref().unwrap_or("general");
        markdown.push_str(&format!(
            "\n## {}\n- Rule: `{rule}`\n- Domain: `{domain}`\n- Recurrence: {} review session(s)\n- Preferred pattern: {}\n\n### Case Study Index\n",
            humanize_slug(&candidate.anti_pattern_category),
            candidate.recurrence_count,
            candidate.preferred_pattern
        ));

        for case_study in &candidate.case_studies {
            markdown.push_str(&format!(
                "1. Session `{}` at `{}`{}: {}\n",
                case_study.session_id,
                case_study.file_path,
                format_line_range(case_study.line_range),
                case_study.fix_description
            ));
        }
    }

    markdown
}

fn render_case_studies_markdown(language: &str, candidates: &[&BugClassCandidate]) -> String {
    let language_name = display_language_name(language);
    let code_fence = code_fence_language(language);
    let mut markdown = format!(
        "# {} Case Studies\n\nConcrete examples mined from recurring review artifacts.\n",
        language_name
    );

    for candidate in candidates {
        markdown.push_str(&format!(
            "\n## {}\n",
            humanize_slug(&candidate.anti_pattern_category)
        ));

        for case_study in &candidate.case_studies {
            markdown.push_str(&format!(
                "\n### Session `{}`\n- File: `{}`{}\n- Fix: {}\n",
                case_study.session_id,
                case_study.file_path,
                format_line_range(case_study.line_range),
                case_study.fix_description
            ));

            if let Some(snippet) = &case_study.code_snippet {
                markdown.push_str(&format!("\n```{code_fence}\n{snippet}\n```\n"));
            } else {
                markdown.push_str("\nCode snippet unavailable in the persisted review artifact.\n");
            }
        }
    }

    markdown
}

fn display_language_name(language: &str) -> String {
    match language.trim().to_ascii_lowercase().as_str() {
        "csharp" => "C#".to_string(),
        "cpp" => "C++".to_string(),
        "javascript" => "JavaScript".to_string(),
        "typescript" => "TypeScript".to_string(),
        "proto" => "Proto".to_string(),
        "sql" => "SQL".to_string(),
        "toml" => "TOML".to_string(),
        "yaml" => "YAML".to_string(),
        "unknown" | "" => "Unknown".to_string(),
        token => {
            let mut chars = token.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => "Unknown".to_string(),
            }
        }
    }
}

fn language_extension_hint(language: &str) -> String {
    match language.trim().to_ascii_lowercase().as_str() {
        "rust" => "`.rs`".to_string(),
        "go" => "`*.go`".to_string(),
        "python" => "`*.py`".to_string(),
        "javascript" => "`*.js` or `*.jsx`".to_string(),
        "typescript" => "`*.ts` or `*.tsx`".to_string(),
        "shell" => "shell script".to_string(),
        "unknown" | "" => "source file".to_string(),
        token => format!("`*.{token}`"),
    }
}

fn code_fence_language(language: &str) -> String {
    match language.trim().to_ascii_lowercase().as_str() {
        "unknown" | "" => "text".to_string(),
        "shell" => "bash".to_string(),
        other => other.to_string(),
    }
}

fn humanize_slug(slug: &str) -> String {
    let words = slug
        .split('-')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>();

    if words.is_empty() {
        "Uncategorized".to_string()
    } else {
        words.join(" ")
    }
}

fn format_line_range(line_range: Option<(u32, u32)>) -> String {
    match line_range {
        Some((start, end)) if start == end => format!(":{start}"),
        Some((start, end)) => format!(":{start}-{end}"),
        None => String::new(),
    }
}

fn yaml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_atomic_if_changed(path: &Path, content: &str) -> Result<bool> {
    match fs::read_to_string(path) {
        Ok(existing) if existing == content => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read existing file {}", path.display()));
        }
    }

    write_atomic(path, content)?;
    Ok(true)
}

fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("missing parent directory for {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create parent directory {}", parent.display()))?;

    let mut temp_file = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    temp_file
        .write_all(content.as_bytes())
        .with_context(|| format!("failed to write temp file for {}", path.display()))?;
    temp_file
        .persist(path)
        .map_err(|error| error.error)
        .map(|_| ())
        .with_context(|| format!("failed to atomically replace {}", path.display()))
}

#[cfg(test)]
#[path = "bug_class_tests.rs"]
mod tests;
