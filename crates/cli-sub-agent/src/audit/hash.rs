use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::Path;

const BUFFER_SIZE: usize = 8 * 1024;

pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)
        .with_context(|| format!("Failed to open file for hashing: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; BUFFER_SIZE];

    loop {
        let bytes_read = file
            .read(&mut buffer)
            .with_context(|| format!("Failed while hashing file: {}", path.display()))?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(format!("sha256:{:x}", hasher.finalize()))
}
