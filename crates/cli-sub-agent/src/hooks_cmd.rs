//! CLI handler for `csa hooks` subcommands.

use anyhow::Result;

use crate::cli::HooksCommands;

/// Dispatch `csa hooks <subcommand>`.
pub fn handle_hooks(cmd: HooksCommands) -> Result<()> {
    match cmd {
        HooksCommands::InstallMergeGuard { path } => {
            let install_dir = path.unwrap_or_else(csa_hooks::default_install_dir);
            let wrapper_path = csa_hooks::install_merge_guard(&install_dir)?;

            println!("Merge guard installed: {}", wrapper_path.display());
            println!();
            println!("Add the following to your shell profile (~/.bashrc, ~/.zshrc, etc.):");
            println!();
            println!("  export PATH=\"{}:$PATH\"", install_dir.display());
            println!();
            println!("Then reload your shell or run:");
            println!();
            println!("  export PATH=\"{}:$PATH\"", install_dir.display());
            println!();
            println!("Verify with: which gh");
            println!("  Expected: {}", wrapper_path.display());

            Ok(())
        }
    }
}
