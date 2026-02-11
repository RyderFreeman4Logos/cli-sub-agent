//! Genealogy tracking and tree building

use crate::manager::list_all_sessions_in;
use crate::state::MetaSessionState;
use anyhow::Result;
use std::path::Path;

/// Find all child sessions of a given session
pub fn find_children(project_path: &Path, session_id: &str) -> Result<Vec<String>> {
    use crate::manager::get_session_root;
    let base_dir = get_session_root(project_path)?;
    find_children_in(&base_dir, session_id)
}

/// Internal implementation: find children in explicit base directory
fn find_children_in(base_dir: &Path, session_id: &str) -> Result<Vec<String>> {
    let all_sessions = list_all_sessions_in(base_dir)?;

    let children: Vec<String> = all_sessions
        .into_iter()
        .filter_map(|session| {
            if session.genealogy.parent_session_id.as_deref() == Some(session_id) {
                Some(session.meta_session_id)
            } else {
                None
            }
        })
        .collect();

    Ok(children)
}

/// Build a tree representation of sessions
///
/// Format: `{prefix}{short_id}  {tools}  {description}`
/// where short_id is the first 11 characters of the ULID
pub fn list_sessions_tree(project_path: &Path, tool_filter: Option<&[&str]>) -> Result<String> {
    use crate::manager::get_session_root;
    let base_dir = get_session_root(project_path)?;
    list_sessions_tree_in(&base_dir, tool_filter)
}

/// Internal implementation: build tree from explicit base directory
fn list_sessions_tree_in(base_dir: &Path, tool_filter: Option<&[&str]>) -> Result<String> {
    let mut all_sessions = list_all_sessions_in(base_dir)?;

    // Apply tool filter if specified
    if let Some(tools) = tool_filter {
        all_sessions.retain(|session| tools.iter().any(|tool| session.tools.contains_key(*tool)));
    }

    // Sort by created_at for consistent ordering
    all_sessions.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    // Find root sessions (no parent)
    let roots: Vec<&MetaSessionState> = all_sessions
        .iter()
        .filter(|s| s.genealogy.parent_session_id.is_none())
        .collect();

    let mut output = String::new();

    for root in roots {
        output.push_str(&format_session_tree(root, &all_sessions, 0));
    }

    Ok(output)
}

/// Recursively format a session and its children as a tree
fn format_session_tree(
    session: &MetaSessionState,
    all_sessions: &[MetaSessionState],
    indent: usize,
) -> String {
    let mut output = String::new();

    // Build prefix
    let prefix = if indent == 0 {
        String::new()
    } else {
        "  ".repeat(indent - 1) + "├─ "
    };

    // Short ID (first 11 chars)
    let short_id = &session.meta_session_id[..11.min(session.meta_session_id.len())];

    // Tools list
    let tools: Vec<&str> = session.tools.keys().map(|s| s.as_str()).collect();
    let tools_str = if tools.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", tools.join(", "))
    };

    // Description
    let description = session.description.as_deref().unwrap_or("<no description>");

    output.push_str(&format!(
        "{}{}  {}  {}\n",
        prefix, short_id, tools_str, description
    ));

    // Find and format children
    let children: Vec<&MetaSessionState> = all_sessions
        .iter()
        .filter(|s| s.genealogy.parent_session_id.as_deref() == Some(&session.meta_session_id))
        .collect();

    for child in children {
        output.push_str(&format_session_tree(child, all_sessions, indent + 1));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::create_session_in;
    use tempfile::tempdir;

    #[test]
    fn test_find_children() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let parent = create_session_in(temp_dir.path(), project_path, Some("Parent"), None, None)
            .expect("Failed to create parent");

        let child1 = create_session_in(
            temp_dir.path(),
            project_path,
            Some("Child 1"),
            Some(&parent.meta_session_id),
            None,
        )
        .expect("Failed to create child 1");

        let child2 = create_session_in(
            temp_dir.path(),
            project_path,
            Some("Child 2"),
            Some(&parent.meta_session_id),
            None,
        )
        .expect("Failed to create child 2");

        let children = find_children_in(temp_dir.path(), &parent.meta_session_id)
            .expect("Failed to find children");

        assert_eq!(children.len(), 2);
        assert!(children.contains(&child1.meta_session_id));
        assert!(children.contains(&child2.meta_session_id));
    }

    #[test]
    fn test_find_children_none() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let session = create_session_in(temp_dir.path(), project_path, Some("Lonely"), None, None)
            .expect("Failed to create session");

        let children = find_children_in(temp_dir.path(), &session.meta_session_id)
            .expect("Failed to find children");

        assert_eq!(children.len(), 0);
    }

    #[test]
    fn test_list_sessions_tree_single_root() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let root = create_session_in(
            temp_dir.path(),
            project_path,
            Some("Root session"),
            None,
            None,
        )
        .expect("Failed to create root");

        let tree = list_sessions_tree_in(temp_dir.path(), None).expect("Failed to build tree");

        assert!(tree.contains(&root.meta_session_id[..11]));
        assert!(tree.contains("Root session"));
        assert!(!tree.contains("├─")); // No children, no tree branches
    }

    #[test]
    fn test_list_sessions_tree_with_children() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let root = create_session_in(temp_dir.path(), project_path, Some("Root"), None, None)
            .expect("Failed to create root");

        let child = create_session_in(
            temp_dir.path(),
            project_path,
            Some("Child"),
            Some(&root.meta_session_id),
            None,
        )
        .expect("Failed to create child");

        let tree = list_sessions_tree_in(temp_dir.path(), None).expect("Failed to build tree");

        assert!(tree.contains(&root.meta_session_id[..11]));
        assert!(tree.contains(&child.meta_session_id[..11]));
        assert!(tree.contains("├─")); // Should have tree branch for child
    }

    #[test]
    fn test_list_sessions_tree_multiple_roots() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let root1 = create_session_in(temp_dir.path(), project_path, Some("Root 1"), None, None)
            .expect("Failed to create root 1");

        let root2 = create_session_in(temp_dir.path(), project_path, Some("Root 2"), None, None)
            .expect("Failed to create root 2");

        let tree = list_sessions_tree_in(temp_dir.path(), None).expect("Failed to build tree");

        assert!(tree.contains(&root1.meta_session_id[..11]));
        assert!(tree.contains(&root2.meta_session_id[..11]));
        assert!(tree.contains("Root 1"));
        assert!(tree.contains("Root 2"));
    }

    #[test]
    fn test_format_session_tree() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let session = create_session_in(temp_dir.path(), project_path, Some("Test"), None, None)
            .expect("Failed to create session");

        let all_sessions = vec![session.clone()];
        let formatted = format_session_tree(&session, &all_sessions, 0);

        assert!(formatted.contains(&session.meta_session_id[..11]));
        assert!(formatted.contains("Test"));
        assert!(formatted.contains("[]")); // No tools
    }

    #[test]
    fn test_root_sessions_no_parent() {
        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        let root = create_session_in(temp_dir.path(), project_path, Some("Root"), None, None)
            .expect("Failed to create root");

        assert!(root.genealogy.parent_session_id.is_none());
        assert_eq!(root.genealogy.depth, 0);
    }

    #[test]
    fn test_list_sessions_tree_public_api_with_project_path() {
        use crate::manager::{create_session, get_session_root};

        let temp_dir = tempdir().expect("Failed to create temp dir");
        let project_path = temp_dir.path();

        // Create session using public API (stores in proper location)
        let root = create_session(project_path, Some("Root session"), None, None)
            .expect("Failed to create root");

        // Use public API which should convert project_path to session root
        let tree = list_sessions_tree(project_path, None).expect("Failed to build tree");

        // Verify session appears in tree output
        assert!(tree.contains(&root.meta_session_id[..11]));
        assert!(tree.contains("Root session"));

        // Verify session is stored in correct location
        let session_root = get_session_root(project_path).expect("Failed to get session root");
        assert!(session_root
            .join("sessions")
            .join(&root.meta_session_id)
            .exists());
    }
}
