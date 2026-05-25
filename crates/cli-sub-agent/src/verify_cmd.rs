use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;
use serde::Serialize;

use crate::cli::{VerifyArgs, VerifyMethodArg};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum VerifyVerdict {
    Verified,
    NotVerified,
    Inconclusive,
}

impl VerifyVerdict {
    pub(crate) fn exit_code(self) -> i32 {
        match self {
            Self::Verified => 0,
            Self::NotVerified => 1,
            Self::Inconclusive => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum VerifyMethod {
    Test,
    Benchmark,
    TokenCount,
    Checklist,
}

#[derive(Debug, Serialize)]
pub(crate) struct VerifyResult {
    claim: String,
    baseline_ref: String,
    treatment_ref: String,
    method: VerifyMethod,
    verdict: VerifyVerdict,
    evidence: Vec<VerifyEvidence>,
}

#[derive(Debug, Serialize)]
pub(crate) struct VerifyEvidence {
    metric: String,
    baseline_value: serde_json::Value,
    treatment_value: serde_json::Value,
    delta: serde_json::Value,
}

#[derive(Debug)]
struct CommandMeasurement {
    passed: u64,
    failed: u64,
    exit_success: bool,
}

pub(crate) fn handle_verify(args: VerifyArgs) -> Result<i32> {
    let method = args
        .method
        .map(VerifyMethod::from_arg)
        .unwrap_or_else(|| VerifyMethod::detect(&args.claim));

    let result = match method {
        VerifyMethod::Test => verify_tests(&args)?,
        VerifyMethod::TokenCount => verify_token_count(&args)?,
        VerifyMethod::Benchmark => placeholder_result(
            &args,
            method,
            "Run Criterion (`cargo bench`) or hyperfine against exported baseline and treatment refs, then record the benchmark deltas.",
        ),
        VerifyMethod::Checklist => placeholder_result(
            &args,
            method,
            "Checklist: define the measurable claim, run the same command on baseline and treatment, compare the metric, attach raw output.",
        ),
    };

    let json = serde_json::to_string_pretty(&result)?;
    println!("{json}");

    if let Some(path) = &args.output {
        fs::write(path, format!("{json}\n"))
            .with_context(|| format!("Failed to write verify output: {}", path.display()))?;
    }

    Ok(result.verdict.exit_code())
}

impl VerifyMethod {
    fn from_arg(arg: VerifyMethodArg) -> Self {
        match arg {
            VerifyMethodArg::Test => Self::Test,
            VerifyMethodArg::Benchmark => Self::Benchmark,
            VerifyMethodArg::TokenCount => Self::TokenCount,
            VerifyMethodArg::Checklist => Self::Checklist,
        }
    }

    fn detect(claim: &str) -> Self {
        let normalized = claim.to_ascii_lowercase();
        if ["token", "size", "reduce"]
            .iter()
            .any(|needle| normalized.contains(needle))
        {
            Self::TokenCount
        } else if ["test", "pass", "fix"]
            .iter()
            .any(|needle| normalized.contains(needle))
        {
            Self::Test
        } else {
            Self::Checklist
        }
    }
}

fn verify_tests(args: &VerifyArgs) -> Result<VerifyResult> {
    let refs = ExportedRefs::new(&args.baseline, &args.treatment)?;
    let baseline = run_cargo_test(refs.baseline.path())?;
    let treatment = run_cargo_test(refs.treatment.path())?;

    let verdict = if treatment.exit_success && !baseline.exit_success {
        VerifyVerdict::Verified
    } else if !treatment.exit_success {
        VerifyVerdict::NotVerified
    } else {
        VerifyVerdict::Inconclusive
    };

    Ok(VerifyResult {
        claim: args.claim.clone(),
        baseline_ref: args.baseline.clone(),
        treatment_ref: args.treatment.clone(),
        method: VerifyMethod::Test,
        verdict,
        evidence: vec![
            evidence_u64(
                "cargo_test_passed",
                baseline.passed,
                treatment.passed,
                treatment.passed as i64 - baseline.passed as i64,
            ),
            evidence_u64(
                "cargo_test_failed",
                baseline.failed,
                treatment.failed,
                treatment.failed as i64 - baseline.failed as i64,
            ),
        ],
    })
}

fn verify_token_count(args: &VerifyArgs) -> Result<VerifyResult> {
    let refs = ExportedRefs::new(&args.baseline, &args.treatment)?;
    let changed_files = changed_files(&args.baseline, &args.treatment)?;
    if changed_files.is_empty() {
        return Ok(VerifyResult {
            claim: args.claim.clone(),
            baseline_ref: args.baseline.clone(),
            treatment_ref: args.treatment.clone(),
            method: VerifyMethod::TokenCount,
            verdict: VerifyVerdict::Inconclusive,
            evidence: vec![evidence_text(
                "changed_files",
                "0",
                "0",
                "no changed files between refs",
            )],
        });
    }

    let baseline_tokens = count_ref_tokens(refs.baseline.path(), &changed_files)?;
    let treatment_tokens = count_ref_tokens(refs.treatment.path(), &changed_files)?;
    let delta = treatment_tokens as i64 - baseline_tokens as i64;
    let verdict = if treatment_tokens < baseline_tokens {
        VerifyVerdict::Verified
    } else {
        VerifyVerdict::NotVerified
    };

    Ok(VerifyResult {
        claim: args.claim.clone(),
        baseline_ref: args.baseline.clone(),
        treatment_ref: args.treatment.clone(),
        method: VerifyMethod::TokenCount,
        verdict,
        evidence: vec![
            evidence_u64("tokens", baseline_tokens, treatment_tokens, delta),
            evidence_u64(
                "changed_files",
                changed_files.len() as u64,
                changed_files.len() as u64,
                0,
            ),
        ],
    })
}

fn placeholder_result(args: &VerifyArgs, method: VerifyMethod, instructions: &str) -> VerifyResult {
    VerifyResult {
        claim: args.claim.clone(),
        baseline_ref: args.baseline.clone(),
        treatment_ref: args.treatment.clone(),
        method,
        verdict: VerifyVerdict::Inconclusive,
        evidence: vec![evidence_text("instructions", "", "", instructions)],
    }
}

fn run_cargo_test(project_root: &Path) -> Result<CommandMeasurement> {
    let output = Command::new("cargo")
        .arg("test")
        .current_dir(project_root)
        .output()
        .with_context(|| format!("Failed to run cargo test in {}", project_root.display()))?;

    Ok(parse_cargo_test_output(
        &String::from_utf8_lossy(&output.stdout),
        &String::from_utf8_lossy(&output.stderr),
        output.status.success(),
    ))
}

fn parse_cargo_test_output(stdout: &str, stderr: &str, exit_success: bool) -> CommandMeasurement {
    let mut measurement = CommandMeasurement {
        passed: 0,
        failed: 0,
        exit_success,
    };
    let combined = format!("{stdout}\n{stderr}");
    let re = Regex::new(r"test result: \w+\.\s+(\d+) passed;\s+(\d+) failed").expect("valid regex");

    for capture in re.captures_iter(&combined) {
        measurement.passed += capture
            .get(1)
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .unwrap_or(0);
        measurement.failed += capture
            .get(2)
            .and_then(|m| m.as_str().parse::<u64>().ok())
            .unwrap_or(0);
    }

    if !exit_success && measurement.failed == 0 {
        measurement.failed = 1;
    }

    measurement
}

fn changed_files(baseline_ref: &str, treatment_ref: &str) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["diff", "--name-only", baseline_ref, treatment_ref])
        .output()
        .with_context(|| format!("Failed to diff refs {baseline_ref}..{treatment_ref}"))?;

    if !output.status.success() {
        bail!(
            "git diff failed for {baseline_ref}..{treatment_ref}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

fn count_ref_tokens(root: &Path, files: &[PathBuf]) -> Result<u64> {
    let mut paths = Vec::new();
    for relative in files {
        let path = root.join(relative);
        if !path.exists() {
            continue;
        }

        if fs::read_to_string(&path).is_err() {
            continue;
        }

        paths.push(path);
    }

    run_tokuin_estimate(&paths)
}

fn run_tokuin_estimate(files: &[PathBuf]) -> Result<u64> {
    if files.is_empty() {
        return Ok(0);
    }

    let exe = std::env::current_exe().context("Failed to resolve current csa executable")?;
    let output = Command::new(exe)
        .args(["tokuin", "estimate", "--json"])
        .args(files)
        .output()
        .context("Failed to run csa tokuin estimate")?;

    if !output.status.success() {
        bail!(
            "csa tokuin estimate failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("Failed to parse csa tokuin estimate JSON output")?;
    value
        .get("total")
        .or_else(|| value.get("tokens"))
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("csa tokuin estimate JSON did not include total or tokens"))
}

struct ExportedRefs {
    baseline: tempfile::TempDir,
    treatment: tempfile::TempDir,
}

impl ExportedRefs {
    fn new(baseline_ref: &str, treatment_ref: &str) -> Result<Self> {
        validate_ref(baseline_ref)?;
        validate_ref(treatment_ref)?;

        let baseline = tempfile::tempdir().context("Failed to create baseline export dir")?;
        let treatment = tempfile::tempdir().context("Failed to create treatment export dir")?;

        export_ref(baseline_ref, baseline.path())?;
        export_ref(treatment_ref, treatment.path())?;

        Ok(Self {
            baseline,
            treatment,
        })
    }
}

fn validate_ref(ref_name: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["rev-parse", "--verify", &format!("{ref_name}^{{tree}}")])
        .output()
        .with_context(|| format!("Failed to resolve git ref {ref_name}"))?;

    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "Invalid git ref {ref_name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

fn export_ref(ref_name: &str, destination: &Path) -> Result<()> {
    let mut archive = Command::new("git")
        .args(["archive", "--format=tar", ref_name])
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to start git archive for {ref_name}"))?;

    let archive_stdout = archive
        .stdout
        .take()
        .ok_or_else(|| anyhow!("git archive stdout was not captured"))?;

    let mut tar = Command::new("tar")
        .args([OsStr::new("-xf"), OsStr::new("-"), OsStr::new("-C")])
        .arg(destination)
        .stdin(Stdio::from(archive_stdout))
        .spawn()
        .with_context(|| format!("Failed to start tar for {}", destination.display()))?;

    let tar_status = tar.wait().context("Failed to wait for tar")?;
    let archive_status = archive.wait().context("Failed to wait for git archive")?;

    if !archive_status.success() {
        bail!("git archive failed for {ref_name}");
    }
    if !tar_status.success() {
        bail!("tar failed while exporting {ref_name}");
    }

    Ok(())
}

fn evidence_u64(metric: &str, baseline: u64, treatment: u64, delta: i64) -> VerifyEvidence {
    VerifyEvidence {
        metric: metric.to_string(),
        baseline_value: serde_json::json!(baseline),
        treatment_value: serde_json::json!(treatment),
        delta: serde_json::json!(delta),
    }
}

fn evidence_text(metric: &str, baseline: &str, treatment: &str, delta: &str) -> VerifyEvidence {
    VerifyEvidence {
        metric: metric.to_string(),
        baseline_value: serde_json::json!(baseline),
        treatment_value: serde_json::json!(treatment),
        delta: serde_json::json!(delta),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_detects_token_count_claims() {
        assert_eq!(
            VerifyMethod::detect("this reduces token size"),
            VerifyMethod::TokenCount
        );
    }

    #[test]
    fn auto_detects_test_claims() {
        assert_eq!(
            VerifyMethod::detect("fix makes tests pass"),
            VerifyMethod::Test
        );
    }

    #[test]
    fn auto_detects_checklist_for_unknown_claims() {
        assert_eq!(
            VerifyMethod::detect("the UX is clearer"),
            VerifyMethod::Checklist
        );
    }

    #[test]
    fn verdict_exit_codes_match_contract() {
        assert_eq!(VerifyVerdict::Verified.exit_code(), 0);
        assert_eq!(VerifyVerdict::NotVerified.exit_code(), 1);
        assert_eq!(VerifyVerdict::Inconclusive.exit_code(), 2);
    }

    #[test]
    fn parses_cargo_test_summary_counts() {
        let stdout = "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out";
        let stderr =
            "test result: FAILED. 1 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out";

        let measurement = parse_cargo_test_output(stdout, stderr, false);

        assert_eq!(measurement.passed, 4);
        assert_eq!(measurement.failed, 2);
        assert!(!measurement.exit_success);
    }
}
