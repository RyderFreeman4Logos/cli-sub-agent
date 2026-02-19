use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

/// Regex matching `mod foo;` declarations (not inline `mod foo { ... }`).
static MOD_DECL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:pub(?:\(crate\))?\s+)?mod\s+(\w+)\s*;").unwrap());

/// Regex matching `use crate::some::path` declarations.
static USE_CRATE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*use\s+crate::(\w+)").unwrap());

/// Sort files in topological order based on Rust `mod`/`use crate::` dependencies.
///
/// Returns files with leaves (no dependencies) first so auditors process
/// foundational modules before the code that depends on them.
///
/// Non-Rust files and files where dependency parsing fails fall back to
/// directory-depth ordering (deeper paths first, then alphabetical).
///
/// Dependencies are resolved by file path structure rather than module name
/// indexing, which correctly handles nested modules with the same basename
/// (e.g., `src/config.rs` and `src/foo/config.rs`).
///
/// `context_files` provides the full manifest file list for crate root
/// discovery. This ensures `lib.rs`/`main.rs` are found even when `files`
/// has been filtered (e.g., `--filter generated`). Pass `files` again when
/// no filtering was applied.
pub(crate) fn topo_sort(
    files: &[String],
    context_files: &[String],
    project_root: &Path,
) -> Vec<String> {
    let rust_files: Vec<&String> = files.iter().filter(|f| f.ends_with(".rs")).collect();
    let non_rust_files: Vec<&String> = files.iter().filter(|f| !f.ends_with(".rs")).collect();

    // Discover crate roots from the full (unfiltered) file set so that
    // `lib.rs`/`main.rs` are found even when the sort input was filtered.
    let context_rust: Vec<&String> = context_files
        .iter()
        .filter(|f| f.ends_with(".rs"))
        .collect();
    let crate_roots = discover_crate_roots(&context_rust);

    // Build file set for O(1) membership checks.
    let file_set: HashSet<&str> = rust_files.iter().map(|f| f.as_str()).collect();

    // Build adjacency list using path-based dependency resolution.
    let mut deps: HashMap<String, HashSet<String>> = HashMap::new();
    for file in &rust_files {
        deps.insert((*file).clone(), HashSet::new());
    }

    for file in &rust_files {
        let abs_path = project_root.join(file);
        let content = match fs::read_to_string(&abs_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let crate_root = find_crate_root(file, &crate_roots);

        // Resolve `mod foo;` declarations to child module file paths.
        for mod_name in parse_mod_declarations(&content) {
            for candidate in resolve_child_module(file, &mod_name) {
                if file_set.contains(candidate.as_str()) && candidate != **file {
                    deps.entry((*file).clone()).or_default().insert(candidate);
                }
            }
        }

        // Resolve `use crate::foo` to crate-root-level module file paths.
        for module_name in parse_use_crate_refs(&content) {
            for candidate in resolve_crate_use(&crate_root, &module_name) {
                if file_set.contains(candidate.as_str()) && candidate != **file {
                    deps.entry((*file).clone()).or_default().insert(candidate);
                }
            }
        }
    }

    // Kahn's algorithm for topological sort (leaves first).
    let sorted_rust = kahns_sort(&deps);

    // Non-Rust files sorted by depth (deeper first), then alphabetical.
    let mut non_rust_sorted: Vec<String> = non_rust_files.into_iter().cloned().collect();
    non_rust_sorted.sort_by(|a, b| {
        let depth_a = path_depth(a);
        let depth_b = path_depth(b);
        depth_b.cmp(&depth_a).then_with(|| a.cmp(b))
    });

    // Combine: Rust files in topo order first, then non-Rust files.
    let mut result = sorted_rust;
    result.extend(non_rust_sorted);
    result
}

/// Discover crate root directories from the file list.
///
/// A crate root is the directory containing `lib.rs` or `main.rs`.
/// Returns a sorted list of crate root paths (longest first for greedy matching).
fn discover_crate_roots(rust_files: &[&String]) -> Vec<String> {
    let mut roots: HashSet<String> = HashSet::new();
    for file in rust_files {
        let path = Path::new(file.as_str());
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if stem == "lib" || stem == "main" {
                if let Some(parent) = path.parent() {
                    roots.insert(parent.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    // Sort longest first so deeper crate roots match before shallower ones.
    let mut sorted: Vec<String> = roots.into_iter().collect();
    sorted.sort_by_key(|r| std::cmp::Reverse(r.len()));
    sorted
}

/// Find the crate root for a given file path.
///
/// Returns the longest matching crate root, or "." as fallback.
/// Matches on path-segment boundaries to avoid `crate_a/src` matching
/// `crate_a/src2/file.rs`.
fn find_crate_root(file: &str, crate_roots: &[String]) -> String {
    let file_normalized = file.replace('\\', "/");
    for root in crate_roots {
        if file_normalized.starts_with(root)
            && (file_normalized.len() == root.len()
                || file_normalized[root.len()..].starts_with('/'))
        {
            return root.clone();
        }
    }
    // Fallback: use "." (project root as single implicit crate).
    ".".to_string()
}

/// Extract `mod foo;` declaration names from source code.
///
/// Matches `mod foo;`, `pub mod foo;`, and `pub(crate) mod foo;` but NOT
/// inline `mod foo { ... }` blocks (no trailing semicolon).
fn parse_mod_declarations(content: &str) -> HashSet<String> {
    MOD_DECL_RE
        .captures_iter(content)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Extract top-level crate module names from `use crate::foo` declarations.
fn parse_use_crate_refs(content: &str) -> HashSet<String> {
    USE_CRATE_RE
        .captures_iter(content)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .collect()
}

/// Resolve `mod foo;` from a declaring file to candidate child module file paths.
///
/// Rust module resolution rules:
/// - `src/lib.rs` declaring `mod foo` -> `src/foo.rs` or `src/foo/mod.rs`
/// - `src/bar.rs` declaring `mod foo` -> `src/bar/foo.rs` or `src/bar/foo/mod.rs`
/// - `src/bar/mod.rs` declaring `mod foo` -> `src/bar/foo.rs` or `src/bar/foo/mod.rs`
fn resolve_child_module(declaring_file: &str, mod_name: &str) -> Vec<String> {
    let path = Path::new(declaring_file);
    let parent_dir = match path.file_stem().and_then(|s| s.to_str()) {
        Some("mod" | "lib" | "main") => {
            // mod.rs, lib.rs, main.rs -> child modules are siblings in the same dir
            path.parent().unwrap_or(Path::new("")).to_path_buf()
        }
        _ => {
            // foo.rs -> child modules are in foo/ directory
            path.with_extension("")
        }
    };

    let candidate_file = parent_dir.join(format!("{mod_name}.rs"));
    let candidate_mod = parent_dir.join(mod_name).join("mod.rs");

    vec![
        candidate_file.to_string_lossy().replace('\\', "/"),
        candidate_mod.to_string_lossy().replace('\\', "/"),
    ]
}

/// Resolve `use crate::foo` to candidate file paths within the crate root.
///
/// `use crate::foo` always references a top-level module of the crate,
/// regardless of where the `use` statement appears.
fn resolve_crate_use(crate_root: &str, module_name: &str) -> Vec<String> {
    if crate_root == "." {
        vec![format!("{module_name}.rs"), format!("{module_name}/mod.rs")]
    } else {
        vec![
            format!("{crate_root}/{module_name}.rs"),
            format!("{crate_root}/{module_name}/mod.rs"),
        ]
    }
}

/// Kahn's algorithm producing a topological ordering (leaves first).
///
/// `deps` maps each file to the set of files it depends on.  We want leaves
/// (files with zero dependencies) emitted first, so we run Kahn's on the
/// *forward* dependency graph using **out-degree** (number of unresolved deps).
///
/// If cycles are detected, cyclic components fall back to depth-based ordering
/// and are appended after the acyclic portion.
fn kahns_sort(deps: &HashMap<String, HashSet<String>>) -> Vec<String> {
    // Build reverse adjacency: for each node, which other nodes depend on it?
    let mut dependents: HashMap<&String, Vec<&String>> = HashMap::new();
    for node in deps.keys() {
        dependents.entry(node).or_default();
    }
    // out_degree[node] = how many (known) dependencies node still has.
    let mut out_degree: HashMap<&String, usize> = HashMap::new();
    for (node, node_deps) in deps {
        // Only count deps that are actually in our file set.
        let count = node_deps.iter().filter(|d| deps.contains_key(*d)).count();
        out_degree.insert(node, count);
        for dep in node_deps {
            if deps.contains_key(dep) {
                dependents.entry(dep).or_default().push(node);
            }
        }
    }

    // Start with nodes that have zero dependencies (leaves / foundations).
    let mut initial: Vec<&String> = out_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(node, _)| *node)
        .collect();
    initial.sort(); // deterministic
    let mut queue: VecDeque<&String> = initial.into_iter().collect();

    let mut result: Vec<String> = Vec::new();

    while let Some(node) = queue.pop_front() {
        result.push(node.clone());

        // For each node that depends on `node`, decrement its out-degree.
        if let Some(users) = dependents.get(node) {
            let mut next_ready: Vec<&String> = Vec::new();
            for user in users {
                if let Some(deg) = out_degree.get_mut(*user) {
                    *deg = deg.saturating_sub(1);
                    if *deg == 0 {
                        next_ready.push(user);
                    }
                }
            }
            next_ready.sort(); // deterministic
            for ready in next_ready {
                queue.push_back(ready);
            }
        }
    }

    // Handle cycles: any remaining nodes not in result are part of cycles.
    if result.len() < deps.len() {
        let result_set: HashSet<&str> = result.iter().map(|s| s.as_str()).collect();
        let mut cyclic: Vec<String> = deps
            .keys()
            .filter(|k| !result_set.contains(k.as_str()))
            .cloned()
            .collect();
        // Fall back to depth ordering for cyclic nodes.
        cyclic.sort_by(|a, b| {
            let depth_a = path_depth(a);
            let depth_b = path_depth(b);
            depth_b.cmp(&depth_a).then_with(|| a.cmp(b))
        });
        result.extend(cyclic);
    }

    result
}

fn path_depth(path: &str) -> usize {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_mod_declarations() {
        let content = "mod foo;\npub mod bar;\npub(crate) mod baz;";
        let decls = parse_mod_declarations(content);
        assert!(decls.contains("foo"));
        assert!(decls.contains("bar"));
        assert!(decls.contains("baz"));
    }

    #[test]
    fn test_parse_mod_declarations_ignores_inline() {
        // `mod foo { ... }` should NOT be captured (no trailing semicolon).
        let content = "mod inline {\n    fn hello() {}\n}";
        let decls = parse_mod_declarations(content);
        assert!(!decls.contains("inline"));
    }

    #[test]
    fn test_parse_use_crate_refs() {
        let content = "use crate::config;\nuse crate::session::State;";
        let refs = parse_use_crate_refs(content);
        assert!(refs.contains("config"));
        assert!(refs.contains("session"));
    }

    #[test]
    fn test_resolve_child_module_from_lib() {
        let candidates = resolve_child_module("src/lib.rs", "foo");
        assert!(candidates.contains(&"src/foo.rs".to_string()));
        assert!(candidates.contains(&"src/foo/mod.rs".to_string()));
    }

    #[test]
    fn test_resolve_child_module_from_regular_file() {
        let candidates = resolve_child_module("src/bar.rs", "foo");
        assert!(candidates.contains(&"src/bar/foo.rs".to_string()));
        assert!(candidates.contains(&"src/bar/foo/mod.rs".to_string()));
    }

    #[test]
    fn test_resolve_child_module_from_mod_rs() {
        let candidates = resolve_child_module("src/bar/mod.rs", "foo");
        assert!(candidates.contains(&"src/bar/foo.rs".to_string()));
        assert!(candidates.contains(&"src/bar/foo/mod.rs".to_string()));
    }

    #[test]
    fn test_resolve_crate_use() {
        let candidates = resolve_crate_use("crate_a/src", "config");
        assert!(candidates.contains(&"crate_a/src/config.rs".to_string()));
        assert!(candidates.contains(&"crate_a/src/config/mod.rs".to_string()));
    }

    #[test]
    fn test_resolve_crate_use_dot_root() {
        let candidates = resolve_crate_use(".", "config");
        assert!(candidates.contains(&"config.rs".to_string()));
        assert!(candidates.contains(&"config/mod.rs".to_string()));
    }

    #[test]
    fn test_linear_chain_sorted_leaves_first() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");

        // lib.rs establishes the crate root for `use crate::` resolution.
        fs::write(src.join("lib.rs"), "").expect("write");

        // leaf.rs has no dependencies
        fs::write(src.join("leaf.rs"), "pub fn leaf() {}").expect("write");

        // middle.rs depends on leaf
        fs::write(src.join("middle.rs"), "use crate::leaf;\npub fn mid() {}").expect("write");

        // root.rs depends on middle
        fs::write(src.join("root.rs"), "use crate::middle;\npub fn root() {}").expect("write");

        let files = vec![
            "src/lib.rs".to_string(),
            "src/root.rs".to_string(),
            "src/middle.rs".to_string(),
            "src/leaf.rs".to_string(),
        ];

        let sorted = topo_sort(&files, &files, tmp.path());

        let leaf_pos = sorted.iter().position(|f| f == "src/leaf.rs").unwrap();
        let mid_pos = sorted.iter().position(|f| f == "src/middle.rs").unwrap();
        let root_pos = sorted.iter().position(|f| f == "src/root.rs").unwrap();

        assert!(
            leaf_pos < mid_pos,
            "leaf should come before middle: {sorted:?}"
        );
        assert!(
            mid_pos < root_pos,
            "middle should come before root: {sorted:?}"
        );
    }

    #[test]
    fn test_cycle_falls_back_to_depth() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");

        // lib.rs establishes the crate root.
        fs::write(src.join("lib.rs"), "").expect("write");

        // a.rs depends on b, b.rs depends on a (cycle)
        fs::write(src.join("a.rs"), "use crate::b;").expect("write");
        fs::write(src.join("b.rs"), "use crate::a;").expect("write");

        // independent.rs has no deps
        fs::write(src.join("independent.rs"), "pub fn ind() {}").expect("write");

        let files = vec![
            "src/lib.rs".to_string(),
            "src/a.rs".to_string(),
            "src/b.rs".to_string(),
            "src/independent.rs".to_string(),
        ];

        let sorted = topo_sort(&files, &files, tmp.path());

        // All files should still appear in output (cycle doesn't drop files).
        assert_eq!(sorted.len(), 4);
        assert!(sorted.contains(&"src/a.rs".to_string()));
        assert!(sorted.contains(&"src/b.rs".to_string()));
        assert!(sorted.contains(&"src/independent.rs".to_string()));

        // Independent (no deps) should come before the cyclic pair.
        let ind_pos = sorted
            .iter()
            .position(|f| f == "src/independent.rs")
            .unwrap();
        let a_pos = sorted.iter().position(|f| f == "src/a.rs").unwrap();
        let b_pos = sorted.iter().position(|f| f == "src/b.rs").unwrap();
        assert!(
            ind_pos < a_pos && ind_pos < b_pos,
            "independent should come before cyclic nodes: {sorted:?}"
        );
    }

    #[test]
    fn test_mixed_rust_and_non_rust() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");

        fs::write(src.join("lib.rs"), "mod foo;").expect("write");
        fs::write(src.join("foo.rs"), "pub fn foo() {}").expect("write");

        let files = vec![
            "src/lib.rs".to_string(),
            "src/foo.rs".to_string(),
            "README.md".to_string(),
            "docs/guide.md".to_string(),
        ];

        let sorted = topo_sort(&files, &files, tmp.path());
        assert_eq!(sorted.len(), 4);

        // Rust files come first, non-Rust after.
        let lib_pos = sorted.iter().position(|f| f == "src/lib.rs").unwrap();
        let foo_pos = sorted.iter().position(|f| f == "src/foo.rs").unwrap();
        let readme_pos = sorted.iter().position(|f| f == "README.md").unwrap();
        let guide_pos = sorted.iter().position(|f| f == "docs/guide.md").unwrap();

        // foo (leaf) before lib (depends on foo)
        assert!(foo_pos < lib_pos, "foo should come before lib: {sorted:?}");

        // All Rust files before non-Rust
        assert!(
            lib_pos < readme_pos,
            "Rust files before non-Rust: {sorted:?}"
        );
        assert!(
            lib_pos < guide_pos,
            "Rust files before non-Rust: {sorted:?}"
        );

        // Non-Rust: docs/guide.md (depth 2) before README.md (depth 1)
        assert!(guide_pos < readme_pos, "deeper non-Rust first: {sorted:?}");
    }

    #[test]
    fn test_mod_declaration_creates_dependency() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");

        // main.rs declares mod config; (child module)
        fs::write(src.join("main.rs"), "mod config;\nfn main() {}").expect("write");
        fs::write(src.join("config.rs"), "pub fn load() {}").expect("write");

        let files = vec!["src/main.rs".to_string(), "src/config.rs".to_string()];

        let sorted = topo_sort(&files, &files, tmp.path());

        let config_pos = sorted.iter().position(|f| f == "src/config.rs").unwrap();
        let main_pos = sorted.iter().position(|f| f == "src/main.rs").unwrap();

        assert!(
            config_pos < main_pos,
            "config (leaf) should come before main: {sorted:?}"
        );
    }

    #[test]
    fn test_multi_crate_same_name_module_no_cross_crate_edge() {
        // Two crates each have a `config.rs`. The topo sort must NOT create a
        // dependency edge from crate_a/src/lib.rs to crate_b/src/config.rs
        // (or vice versa) just because the module names collide.
        let tmp = tempfile::tempdir().expect("tempdir");

        // crate_a: lib.rs declares `mod config;`
        let a_src = tmp.path().join("crate_a/src");
        fs::create_dir_all(&a_src).expect("mkdir crate_a");
        fs::write(a_src.join("lib.rs"), "mod config;\npub fn a() {}").expect("write");
        fs::write(a_src.join("config.rs"), "pub fn cfg_a() {}").expect("write");

        // crate_b: lib.rs declares `mod config;`
        let b_src = tmp.path().join("crate_b/src");
        fs::create_dir_all(&b_src).expect("mkdir crate_b");
        fs::write(b_src.join("lib.rs"), "mod config;\npub fn b() {}").expect("write");
        fs::write(b_src.join("config.rs"), "pub fn cfg_b() {}").expect("write");

        let files = vec![
            "crate_a/src/lib.rs".to_string(),
            "crate_a/src/config.rs".to_string(),
            "crate_b/src/lib.rs".to_string(),
            "crate_b/src/config.rs".to_string(),
        ];

        let sorted = topo_sort(&files, &files, tmp.path());

        // All 4 files must appear.
        assert_eq!(sorted.len(), 4, "all files present: {sorted:?}");

        // Within each crate: config (leaf) before lib (depends on config).
        let a_config = sorted
            .iter()
            .position(|f| f == "crate_a/src/config.rs")
            .unwrap();
        let a_lib = sorted
            .iter()
            .position(|f| f == "crate_a/src/lib.rs")
            .unwrap();
        assert!(a_config < a_lib, "crate_a: config before lib: {sorted:?}");

        let b_config = sorted
            .iter()
            .position(|f| f == "crate_b/src/config.rs")
            .unwrap();
        let b_lib = sorted
            .iter()
            .position(|f| f == "crate_b/src/lib.rs")
            .unwrap();
        assert!(b_config < b_lib, "crate_b: config before lib: {sorted:?}");

        // Cross-crate independence: verify crate_a order is preserved alone.
        let a_only_files = vec![
            "crate_a/src/lib.rs".to_string(),
            "crate_a/src/config.rs".to_string(),
        ];
        let a_only_sorted = topo_sort(&a_only_files, &a_only_files, tmp.path());
        let a_only_config = a_only_sorted
            .iter()
            .position(|f| f == "crate_a/src/config.rs")
            .unwrap();
        let a_only_lib = a_only_sorted
            .iter()
            .position(|f| f == "crate_a/src/lib.rs")
            .unwrap();
        assert!(
            a_only_config < a_only_lib,
            "isolated crate_a: config before lib: {a_only_sorted:?}"
        );
    }

    #[test]
    fn test_nested_same_name_modules_no_collision() {
        // Same crate has `src/config.rs` and `src/foo/config.rs`.
        // Path-based resolution must NOT confuse the two.
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let src_foo = src.join("foo");
        fs::create_dir_all(&src_foo).expect("mkdir");

        // lib.rs declares top-level modules
        fs::write(src.join("lib.rs"), "mod foo;\nmod config;").expect("write");

        // Top-level config module
        fs::write(src.join("config.rs"), "pub fn top_config() {}").expect("write");

        // foo.rs declares its own nested config
        fs::write(src.join("foo.rs"), "mod config;\npub fn foo() {}").expect("write");

        // Nested config under foo/
        fs::write(src_foo.join("config.rs"), "pub fn nested_config() {}").expect("write");

        let files = vec![
            "src/lib.rs".to_string(),
            "src/config.rs".to_string(),
            "src/foo.rs".to_string(),
            "src/foo/config.rs".to_string(),
        ];

        let sorted = topo_sort(&files, &files, tmp.path());
        assert_eq!(sorted.len(), 4, "all files present: {sorted:?}");

        // src/config.rs before src/lib.rs (lib depends on config)
        let config_pos = sorted.iter().position(|f| f == "src/config.rs").unwrap();
        let lib_pos = sorted.iter().position(|f| f == "src/lib.rs").unwrap();
        assert!(config_pos < lib_pos, "config before lib: {sorted:?}");

        // src/foo/config.rs before src/foo.rs (foo depends on its nested config)
        let nested_config_pos = sorted
            .iter()
            .position(|f| f == "src/foo/config.rs")
            .unwrap();
        let foo_pos = sorted.iter().position(|f| f == "src/foo.rs").unwrap();
        assert!(
            nested_config_pos < foo_pos,
            "nested config before foo: {sorted:?}"
        );

        // src/foo.rs before src/lib.rs (lib depends on foo)
        assert!(foo_pos < lib_pos, "foo before lib: {sorted:?}");
    }

    #[test]
    fn test_discover_crate_roots_multiple() {
        let files = vec![
            "crate_a/src/lib.rs".to_string(),
            "crate_a/src/config.rs".to_string(),
            "crate_b/src/main.rs".to_string(),
            "crate_b/src/util.rs".to_string(),
        ];
        let refs: Vec<&String> = files.iter().collect();
        let roots = discover_crate_roots(&refs);
        assert!(roots.contains(&"crate_a/src".to_string()));
        assert!(roots.contains(&"crate_b/src".to_string()));
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn test_find_crate_root_matches_longest() {
        let roots = vec!["crates/inner/src".to_string(), "crates".to_string()];
        assert_eq!(
            find_crate_root("crates/inner/src/foo.rs", &roots),
            "crates/inner/src"
        );
        assert_eq!(find_crate_root("crates/other/src/bar.rs", &roots), "crates");
    }

    #[test]
    fn test_find_crate_root_respects_path_boundary() {
        // "crate_a/src" must NOT match "crate_a/src2/file.rs".
        let roots = vec!["crate_a/src".to_string()];
        assert_eq!(find_crate_root("crate_a/src/foo.rs", &roots), "crate_a/src");
        // src2 is NOT a child of src, should fall back to "."
        assert_eq!(find_crate_root("crate_a/src2/foo.rs", &roots), ".");
    }

    #[test]
    fn test_topo_sort_with_filtered_files_uses_context() {
        // Simulates `--filter generated` where lib.rs (approved) is excluded
        // from `files` but present in `context_files`.
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");

        fs::write(src.join("lib.rs"), "mod config;").expect("write");
        fs::write(src.join("config.rs"), "pub fn load() {}").expect("write");
        fs::write(src.join("util.rs"), "use crate::config;\npub fn u() {}").expect("write");

        // Full manifest (context_files) includes lib.rs.
        let all_files = vec![
            "src/lib.rs".to_string(),
            "src/config.rs".to_string(),
            "src/util.rs".to_string(),
        ];
        // Filtered files exclude lib.rs (e.g., only "generated" status).
        let filtered = vec!["src/config.rs".to_string(), "src/util.rs".to_string()];

        let sorted = topo_sort(&filtered, &all_files, tmp.path());
        assert_eq!(sorted.len(), 2, "only filtered files: {sorted:?}");

        // config (leaf) before util (depends on config via `use crate::config`)
        let config_pos = sorted.iter().position(|f| f == "src/config.rs").unwrap();
        let util_pos = sorted.iter().position(|f| f == "src/util.rs").unwrap();
        assert!(
            config_pos < util_pos,
            "config before util with context: {sorted:?}"
        );
    }
}
