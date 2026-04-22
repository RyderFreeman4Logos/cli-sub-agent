use csa_resource::filesystem_sandbox::FilesystemCapability;
use csa_resource::rlimit::current_rlimit_nproc;
use csa_resource::sandbox::{ResourceCapability, detect_resource_capability, systemd_version};
use std::path::Path;
use std::process::Command;

pub(super) fn print_sandbox_status() {
    let cap = detect_resource_capability();
    println!("Capability:  {cap}");

    match cap {
        ResourceCapability::CgroupV2 => {
            if let Some(ver) = systemd_version() {
                println!("Systemd:     {ver}");
            }
            println!("User scope:  supported");
        }
        ResourceCapability::Setrlimit => {
            println!("Enforces:    PID limit only (RLIMIT_NPROC)");
            match current_rlimit_nproc() {
                Some(n) => println!("RLIMIT_NPROC: {n}"),
                None => println!("RLIMIT_NPROC: unlimited"),
            }
            println!("Memory:      via MemoryBalloon (not setrlimit)");
        }
        ResourceCapability::None => {
            println!("Warning:     No sandbox isolation available.");
            println!("             Resource limits will not be enforced.");
        }
    }
}

pub(super) fn print_filesystem_sandbox_status() {
    let fs_cap = csa_resource::filesystem_sandbox::detect_filesystem_capability();
    println!("Capability:  {fs_cap}");

    match fs_cap {
        FilesystemCapability::Bwrap => {
            if let Some(ver) = bwrap_version() {
                println!("bwrap:       {ver}");
            }
            println!("User NS:     available");
        }
        FilesystemCapability::Landlock => {
            let abi = csa_resource::landlock::detect_abi();
            println!("Landlock ABI: {abi:?}");
        }
        FilesystemCapability::None => {
            println!("Warning:     No filesystem isolation available.");
            if let Some(ver) = bwrap_version() {
                println!("bwrap:       {ver} (installed but user namespaces blocked)");
            } else {
                println!("bwrap:       not installed");
            }
            if is_apparmor_userns_restricted() {
                println!("AppArmor:    restricts unprivileged user namespaces");
            }
            if !has_usable_user_namespaces() {
                println!("User NS:     unavailable");
            }
        }
    }
}

pub(super) fn build_filesystem_sandbox_json(fs_cap: FilesystemCapability) -> serde_json::Value {
    match fs_cap {
        FilesystemCapability::Bwrap => {
            serde_json::json!({
                "capability": "Bwrap",
                "bwrap_version": bwrap_version(),
                "user_namespaces": true,
                "apparmor_userns_restricted": is_apparmor_userns_restricted(),
            })
        }
        FilesystemCapability::Landlock => {
            let abi = csa_resource::landlock::detect_abi();
            serde_json::json!({
                "capability": "Landlock",
                "landlock_abi": format!("{abi:?}"),
                "user_namespaces": has_usable_user_namespaces(),
                "apparmor_userns_restricted": is_apparmor_userns_restricted(),
            })
        }
        FilesystemCapability::None => {
            serde_json::json!({
                "capability": "None",
                "bwrap_installed": bwrap_version().is_some(),
                "user_namespaces": has_usable_user_namespaces(),
                "apparmor_userns_restricted": is_apparmor_userns_restricted(),
            })
        }
    }
}

fn bwrap_version() -> Option<String> {
    let output = Command::new("bwrap").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|s| s.trim().to_string())
}

fn is_apparmor_userns_restricted() -> bool {
    let path = Path::new("/proc/sys/kernel/apparmor_restrict_unprivileged_userns");
    std::fs::read_to_string(path)
        .map(|content| content.trim() == "1")
        .unwrap_or(false)
}

fn has_usable_user_namespaces() -> bool {
    Command::new("unshare")
        .args(["-U", "true"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

pub(super) fn print_merge_guard_status() {
    match csa_hooks::detect_installed_guard() {
        Some(path) => {
            println!("merge guard: installed ({})", path.display());
        }
        None => {
            println!("merge guard: not installed");
            println!("  Hint: csa hooks install-merge-guard");
        }
    }
}

pub(super) fn print_git_hook_status(project_root: &Path) {
    let hooks = [("pre-push", "Blocks push without csa review session")];
    for (hook_name, description) in hooks {
        let hook_path = project_root.join(".git/hooks").join(hook_name);
        if hook_path.is_file() {
            println!("{hook_name}:  installed ({description})");
        } else {
            println!("{hook_name}:  NOT INSTALLED — {description}");
            println!("  Hint: ln -sf ../../scripts/hooks/{hook_name} .git/hooks/{hook_name}");
        }
    }
}
