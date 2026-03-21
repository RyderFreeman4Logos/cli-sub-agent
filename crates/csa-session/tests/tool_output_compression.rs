// Integration test for the tool output compression pipeline.
//
// Verifies the full cycle: store large output → update manifest → retrieve by index.
// This mirrors the flow in `csa session tool-output` (list and retrieve).

use csa_session::tool_output_store::ToolOutputStore;
use tempfile::TempDir;

/// Simulate a session where 3 tool calls are made, 2 exceed the threshold.
#[test]
fn compression_pipeline_store_manifest_retrieve() {
    let session_dir = TempDir::new().unwrap();
    let store = ToolOutputStore::new(session_dir.path()).unwrap();

    // Tool call 0: small output (not compressed, not stored).
    // Tool call 1: large output → compress and store.
    let large_1 = "x".repeat(10_000);
    let path_1 = store.store(1, large_1.as_bytes()).unwrap();
    store.append_manifest(1, large_1.len() as u64).unwrap();
    assert!(path_1.exists());

    // Tool call 2: another large output → compress and store.
    let large_2 = "y".repeat(20_000);
    let path_2 = store.store(2, large_2.as_bytes()).unwrap();
    store.append_manifest(2, large_2.len() as u64).unwrap();
    assert!(path_2.exists());

    // Verify manifest has exactly 2 entries.
    let manifest = store.read_manifest().unwrap();
    assert_eq!(manifest.entries.len(), 2);

    // Verify entry metadata.
    assert_eq!(manifest.entries[0].index, 1);
    assert_eq!(manifest.entries[0].original_bytes, 10_000);
    assert_eq!(manifest.entries[0].path, "tool_outputs/1.raw");

    assert_eq!(manifest.entries[1].index, 2);
    assert_eq!(manifest.entries[1].original_bytes, 20_000);
    assert_eq!(manifest.entries[1].path, "tool_outputs/2.raw");

    // Verify content retrieval by index.
    let loaded_1 = store.load(1).unwrap();
    assert_eq!(loaded_1, large_1.as_bytes());

    let loaded_2 = store.load(2).unwrap();
    assert_eq!(loaded_2, large_2.as_bytes());

    // Verify non-stored index returns error.
    assert!(store.load(0).is_err());
    assert!(store.load(99).is_err());
}

/// Verify that manifest is persistent across separate ToolOutputStore instances.
#[test]
fn manifest_persists_across_store_instances() {
    let session_dir = TempDir::new().unwrap();

    // First instance: store and record.
    {
        let store = ToolOutputStore::new(session_dir.path()).unwrap();
        store.store(0, b"first batch").unwrap();
        store.append_manifest(0, 11).unwrap();
    }

    // Second instance (write): add more.
    {
        let store = ToolOutputStore::new(session_dir.path()).unwrap();
        store.store(1, b"second batch").unwrap();
        store.append_manifest(1, 12).unwrap();
    }

    // Third instance (read-only): verify all entries visible without creating dirs.
    {
        let store = ToolOutputStore::open_readonly(session_dir.path());

        let manifest = store.read_manifest().unwrap();
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(manifest.entries[0].index, 0);
        assert_eq!(manifest.entries[1].index, 1);

        // Content from both instances is still accessible.
        assert_eq!(store.load(0).unwrap(), b"first batch");
        assert_eq!(store.load(1).unwrap(), b"second batch");
    }
}

/// Verify manifest.toml is valid TOML that can be parsed externally.
#[test]
fn manifest_is_valid_toml() {
    let session_dir = TempDir::new().unwrap();
    let store = ToolOutputStore::new(session_dir.path()).unwrap();

    store.store(0, b"content").unwrap();
    store.append_manifest(0, 7).unwrap();

    let raw = std::fs::read_to_string(store.manifest_path()).unwrap();
    let parsed: toml::Value = toml::from_str(&raw).unwrap();
    let entries = parsed.get("entries").unwrap().as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].get("index").unwrap().as_integer().unwrap(), 0);
    assert_eq!(
        entries[0]
            .get("original_bytes")
            .unwrap()
            .as_integer()
            .unwrap(),
        7
    );
}
