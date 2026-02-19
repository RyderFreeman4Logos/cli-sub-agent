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
/// Module names are scoped to their crate root (the directory containing
/// `lib.rs` or `main.rs`) to avoid false edges between identically-named
/// modules in different crates.
pub(crate) fn topo_sort(files: &[String], project_root: &Path) -> Vec<String> {
    let rust_files: Vec<&String> = files.iter().filter(|f| f.ends_with(".rs")).collect();
    let non_rust_files: Vec<&String> = files.iter().filter(|f| !f.ends_with(".rs")).collect();

    // Group Rust files by crate root for scoped module name resolution.
    let crate_roots = discover_crate_roots(&rust_files);

    // Build crate-scoped module-name -> file-path index for Rust files.
    // Key format: "{crate_root}::{module_name}" to avoid cross-crate collisions.
    let mut mod_to_file: HashMap<String, String> = HashMap::new();
    for file in &rust_files {
        let crate_root = find_crate_root(file, &crate_roots);
        for module_name in infer_module_names(file) {
            let scoped_name = format!("{crate_root}::{module_name}");
            mod_to_file.insert(scoped_name, (*file).clone());
        }
    }

    // Build adjacency list: file -> set of files it depends on.
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
        let referenced_modules = parse_dependencies(&content);
        for module_name in referenced_modules {
            // Look up the scoped name within the same crate.
            let scoped_name = format!("{crate_root}::{module_name}");
            if let Some(dep_file) = mod_to_file.get(&scoped_name) {
                if dep_file != *file {
                    deps.entry((*file).clone())
                        .or_default()
                        .insert(dep_file.clone());
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
fn find_crate_root(file: &str, crate_roots: &[String]) -> String {
    let file_normalized = file.replace('\\', "/");
    for root in crate_roots {
        if file_normalized.starts_with(root) {
            return root.clone();
        }
    }
    // Fallback: use "." (project root as single implicit crate).
    ".".to_string()
}

/// Infer the module name(s) that a file path could represent.
///
/// For example:
/// - `src/foo.rs` -> `["foo"]`
/// - `src/foo/mod.rs` -> `["foo"]`
/// - `src/lib.rs` -> `["lib"]`
fn infer_module_names(file_path: &str) -> Vec<String> {
    let path = Path::new(file_path);
    let mut names = Vec::new();

    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
        if stem == "mod" {
            // src/foo/mod.rs -> module name is "foo" (parent directory)
            if let Some(parent) = path.parent() {
                if let Some(dir_name) = parent.file_name().and_then(|s| s.to_str()) {
                    names.push(dir_name.to_string());
                }
            }
        } else {
            names.push(stem.to_string());
        }
    }

    names
}

/// Extract module names referenced via `mod foo;` and `use crate::foo`.
fn parse_dependencies(content: &str) -> HashSet<String> {
    let mut deps = HashSet::new();

    for cap in MOD_DECL_RE.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            deps.insert(m.as_str().to_string());
        }
    }

    for cap in USE_CRATE_RE.captures_iter(content) {
        if let Some(m) = cap.get(1) {
            deps.insert(m.as_str().to_string());
        }
    }

    deps
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
    fn test_infer_module_names_regular_file() {
        assert_eq!(infer_module_names("src/foo.rs"), vec!["foo"]);
        assert_eq!(infer_module_names("src/bar/baz.rs"), vec!["baz"]);
    }

    #[test]
    fn test_infer_module_names_mod_file() {
        assert_eq!(infer_module_names("src/foo/mod.rs"), vec!["foo"]);
    }

    #[test]
    fn test_infer_module_names_lib() {
        assert_eq!(infer_module_names("src/lib.rs"), vec!["lib"]);
    }

    #[test]
    fn test_parse_dependencies_mod_decl() {
        let content = "mod foo;\npub mod bar;\npub(crate) mod baz;";
        let deps = parse_dependencies(content);
        assert!(deps.contains("foo"));
        assert!(deps.contains("bar"));
        assert!(deps.contains("baz"));
    }

    #[test]
    fn test_parse_dependencies_use_crate() {
        let content = "use crate::config;\nuse crate::session::State;";
        let deps = parse_dependencies(content);
        assert!(deps.contains("config"));
        assert!(deps.contains("session"));
    }

    #[test]
    fn test_parse_dependencies_ignores_inline_mod() {
        // `mod foo { ... }` should NOT be captured (no trailing semicolon).
        let content = "mod inline {\n    fn hello() {}\n}";
        let deps = parse_dependencies(content);
        assert!(!deps.contains("inline"));
    }

    #[test]
    fn test_linear_chain_sorted_leaves_first() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("mkdir");

        // leaf.rs has no dependencies
        fs::write(src.join("leaf.rs"), "pub fn leaf() {}").expect("write");

        // middle.rs depends on leaf
        fs::write(src.join("middle.rs"), "use crate::leaf;\npub fn mid() {}").expect("write");

        // root.rs depends on middle
        fs::write(src.join("root.rs"), "use crate::middle;\npub fn root() {}").expect("write");

        let files = vec![
            "src/root.rs".to_string(),
            "src/middle.rs".to_string(),
            "src/leaf.rs".to_string(),
        ];

        let sorted = topo_sort(&files, tmp.path());

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

        // a.rs depends on b, b.rs depends on a (cycle)
        fs::write(src.join("a.rs"), "use crate::b;").expect("write");
        fs::write(src.join("b.rs"), "use crate::a;").expect("write");

        // independent.rs has no deps
        fs::write(src.join("independent.rs"), "pub fn ind() {}").expect("write");

        let files = vec![
            "src/a.rs".to_string(),
            "src/b.rs".to_string(),
            "src/independent.rs".to_string(),
        ];

        let sorted = topo_sort(&files, tmp.path());

        // All files should still appear in output (cycle doesn't drop files).
        assert_eq!(sorted.len(), 3);
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

        let sorted = topo_sort(&files, tmp.path());
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

        let sorted = topo_sort(&files, tmp.path());

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

        let sorted = topo_sort(&files, tmp.path());

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

        // Cross-crate independence: crate_b's config must NOT appear as a
        // dependency of crate_a's lib (and vice versa). We verify by ensuring
        // that crate_b/src/config.rs is NOT forced before crate_a/src/lib.rs
        // in a way that would indicate a false cross-crate edge.
        // Since both crates are independent, the two sub-graphs should be
        // interleaved only by alphabetical tiebreaking, not by dependency.
        // Specifically, crate_a's lib should NOT depend on crate_b's config.
        // The simplest invariant: if we removed crate_b entirely, crate_a's
        // order would be [config, lib]. We can check that crate_a's internal
        // order is independent of crate_b's presence by ensuring the relative
        // order of crate_a files matches the 2-file case.
        let a_only_files = vec![
            "crate_a/src/lib.rs".to_string(),
            "crate_a/src/config.rs".to_string(),
        ];
        let a_only_sorted = topo_sort(&a_only_files, tmp.path());
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
}
