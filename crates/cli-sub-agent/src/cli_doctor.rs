// NOTE #1858: #[path]-included by tests; no `crate::`, no binary-only methods (dead_code).
use clap::Subcommand;
use std::path::PathBuf;

fn default_install_target() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("csa")
            .join("csa.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/usr/local/bin/csa")
    }
}

#[derive(Debug, Subcommand)]
pub enum DoctorSubcommand {
    /// Verify that the PATH-resolved csa is the artifact installed at the intended target
    Install {
        /// Intended installation target (platform default: Unix `/usr/local/bin/csa`)
        #[arg(long, default_value_os_t = default_install_target())]
        target: PathBuf,
        /// Newly built artifact expected to be active; defaults to the intended target
        #[arg(long)]
        artifact: Option<PathBuf>,
    },
    /// Show the complete routing table for all operation types
    Routing {
        /// Filter by operation type (run, review, debate)
        #[arg(long)]
        operation: Option<String>,
        /// Filter by tier name
        #[arg(long)]
        tier: Option<String>,
    },
}
