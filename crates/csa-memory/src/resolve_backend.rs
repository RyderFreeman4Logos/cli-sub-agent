use csa_config::MemoryBackend;

use crate::detect_mempal;

pub fn resolve_backend(configured: MemoryBackend) -> MemoryBackend {
    match configured {
        MemoryBackend::Auto => {
            if detect_mempal().is_some() {
                MemoryBackend::Mempal
            } else {
                MemoryBackend::Legacy
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_backend;
    use csa_config::MemoryBackend;

    #[test]
    fn resolve_backend_preserves_explicit_backend() {
        assert_eq!(
            resolve_backend(MemoryBackend::Legacy),
            MemoryBackend::Legacy
        );
        assert_eq!(
            resolve_backend(MemoryBackend::Mempal),
            MemoryBackend::Mempal
        );
    }
}
