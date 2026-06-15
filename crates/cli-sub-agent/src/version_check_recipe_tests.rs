use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git should execute");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_workspace_version(repo: &Path, version: &str) {
    fs::write(
        repo.join("Cargo.toml"),
        format!("[workspace.package]\nversion = \"{version}\"\n"),
    )
    .expect("write Cargo.toml");
}

fn init_version_check_repo() -> TempDir {
    let td = TempDir::new().expect("create tempdir");
    run_git(td.path(), &["init", "-b", "main"]);
    run_git(td.path(), &["config", "user.email", "test@example.com"]);
    run_git(td.path(), &["config", "user.name", "Test User"]);

    write_workspace_version(td.path(), "0.1.0");
    run_git(td.path(), &["add", "Cargo.toml"]);
    run_git(td.path(), &["commit", "-m", "initial"]);
    run_git(td.path(), &["checkout", "-b", "feature/version-bumped"]);
    write_workspace_version(td.path(), "0.1.1");

    td
}

#[cfg(unix)]
fn install_executable(bin_dir: &Path, name: &str, contents: &str) {
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(bin_dir).expect("create fake bin dir");
    let path = bin_dir.join(name);
    fs::write(&path, contents).unwrap_or_else(|err| panic!("write fake {name}: {err}"));
    let mut perms = fs::metadata(&path)
        .unwrap_or_else(|err| panic!("fake {name} metadata: {err}"))
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms).unwrap_or_else(|err| panic!("chmod fake {name}: {err}"));
}

#[cfg(unix)]
fn install_fake_cargo(bin_dir: &Path) {
    install_executable(
        bin_dir,
        "cargo",
        r#"#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" != "metadata" ]; then
    echo "fake cargo only supports metadata; args=$*" >&2
    exit 43
fi
install_root="${CARGO_INSTALL_ROOT:-}"
printf '%s\n' "$install_root" >"${FAKE_CARGO_INSTALL_ROOT_CAPTURE:?}"
if [ -z "$install_root" ] || [ "$install_root" = "/usr/local" ]; then
    echo "fake cargo saw non-writable CARGO_INSTALL_ROOT=${install_root:-<unset>}" >&2
    exit 42
fi
if [ ! -d "$install_root" ]; then
    echo "fake cargo expected install root directory to exist: $install_root" >&2
    exit 44
fi
cat <<'JSON'
{"packages":[{"name":"cli-sub-agent","version":"0.1.1"}]}
JSON
"#,
    );
}

#[cfg(unix)]
fn install_fake_jq(bin_dir: &Path) {
    install_executable(
        bin_dir,
        "jq",
        r#"#!/usr/bin/env bash
set -euo pipefail
cat >/dev/null
if [ "${1:-}" != "-r" ]; then
    echo "fake jq only supports -r; args=$*" >&2
    exit 45
fi
expected='.packages[] | select(.name == "cli-sub-agent") | .version'
if [ "${2:-}" != "$expected" ]; then
    echo "fake jq saw unexpected filter: ${2:-<missing>}" >&2
    exit 46
fi
printf '0.1.1\n'
"#,
    );
}

#[cfg(unix)]
fn install_failing_just(bin_dir: &Path) {
    install_executable(
        bin_dir,
        "just",
        r#"#!/usr/bin/env bash
echo "fake just should not be invoked by version_check_recipe_tests" >&2
exit 127
"#,
    );
}

enum CargoInstallRoot {
    Unset,
    Value(&'static str),
    RepoLocal(&'static str),
}

#[cfg(unix)]
fn run_check_version_bumped(cargo_install_root: CargoInstallRoot) -> (String, String) {
    let repo = init_version_check_repo();
    let fake_bin = repo.path().join("fake-bin");
    install_fake_cargo(&fake_bin);
    install_fake_jq(&fake_bin);
    install_failing_just(&fake_bin);
    let capture_path = repo.path().join("fake-cargo-install-root.txt");
    let default_install_root = repo.path().join("target/cargo-install-root");

    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut command = Command::new("bash");
    command
        .arg(workspace_root().join("scripts/check-version-bumped.sh"))
        .current_dir(repo.path())
        .env("PATH", path)
        .env("FAKE_CARGO_INSTALL_ROOT_CAPTURE", &capture_path)
        .env_remove("CSA_SKIP_VERSION_CHECK");

    let expected_install_root = match cargo_install_root {
        CargoInstallRoot::Unset => {
            command.env_remove("CARGO_INSTALL_ROOT");
            default_install_root
        }
        CargoInstallRoot::Value(value) => {
            command.env("CARGO_INSTALL_ROOT", value);
            if value == "/usr/local" {
                default_install_root
            } else {
                PathBuf::from(value)
            }
        }
        CargoInstallRoot::RepoLocal(name) => {
            let path = repo.path().join(name);
            command.env("CARGO_INSTALL_ROOT", &path);
            path
        }
    };

    let output = command.output().expect("bash should execute");
    assert!(
        output.status.success(),
        "check-version-bumped script should invoke cargo with a writable install root without spawning just: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let observed_install_root =
        fs::read_to_string(&capture_path).expect("fake cargo should capture install root");
    (
        observed_install_root.trim().to_string(),
        expected_install_root.to_string_lossy().into_owned(),
    )
}

#[test]
#[cfg(unix)]
fn check_version_bumped_ignores_read_only_cargo_install_root() {
    let (observed, expected) = run_check_version_bumped(CargoInstallRoot::Value("/usr/local"));
    assert_eq!(observed, expected);
}

#[test]
#[cfg(unix)]
fn check_version_bumped_works_when_cargo_install_root_is_unset() {
    let (observed, expected) = run_check_version_bumped(CargoInstallRoot::Unset);
    assert_eq!(observed, expected);
}

#[test]
#[cfg(unix)]
fn check_version_bumped_preserves_explicit_cargo_install_root() {
    let (observed, expected) =
        run_check_version_bumped(CargoInstallRoot::RepoLocal("explicit-cargo-install-root"));
    assert_eq!(observed, expected);
}
