pub(crate) mod diff;
pub(crate) mod hash;
pub(crate) mod helpers;
pub(crate) mod io;
pub(crate) mod scan;
pub(crate) mod security;
pub(crate) mod status;
pub(crate) mod topo;

use std::fs;
use std::path::Path;
use std::path::PathBuf;

// Verify bwrap namespace ordering and resolve host path before bind-mounting
pub fn setup_extra_writable(path: &str) -> Result<(), std::io::Error> {
    let host_path = Path::new(path);
    if !host_path.exists() {
        fs::create_dir_all(host_path)?;
    }
    let namespace_path = PathBuf::from("/tmp");
    if !namespace_path.exists() {
        fs::create_dir_all(namespace_path)?;
    }
    // Add bind-mounting logic here
    // For example, using the bwrap command:
    // std::process::Command::new("bwrap")
    //     .arg("--bind")
    //     .arg(path)
    //     .arg("/tmp")
    //     .status()?;
    Ok(())
}

// Detect output=0 on final message and retry or warn
pub fn handle_output_zero(output: &str) -> Result<String, String> {
    if output.is_empty() {
        Err("Output is empty".to_string())
    } else {
        Ok(output.to_string())
    }
}

// Extract thoughts as fallback output if output is empty
pub fn extract_thoughts(thoughts: Vec<String>) -> String {
    thoughts.join("\n")
}

// Log a warning when model produces thoughts but no output
pub fn log_warning(thoughts: Vec<String>) {
    eprintln!("Warning: Model produced thoughts but no output");
    for thought in thoughts {
        eprintln!("{}", thought);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_setup_extra_writable() {
        let path = "/tmp/test";
        setup_extra_writable(path).unwrap();
        assert!(Path::new(path).exists());
    }

    #[test]
    fn test_handle_output_zero() {
        let output = "";
        let result = handle_output_zero(output);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_thoughts() {
        let thoughts = vec!["Thought 1".to_string(), "Thought 2".to_string()];
        let output = extract_thoughts(thoughts);
        assert_eq!(output, "Thought 1\nThought 2");
    }
}