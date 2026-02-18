use crate::config::SandboxPolicy;
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Read,
    Write,
}

/// Verify that a path is permitted by the sandbox policy.
///
/// The path is checked against:
/// 1. The workspace directory (always allowed)
/// 2. The project directory (always allowed)
/// 3. Explicitly allowed paths in the SandboxPolicy (allow_read/allow_write)
pub fn check_path_permitted(
    path: &Path,
    policy: &SandboxPolicy,
    workspace: &Path,
    project_dir: &Path,
    mode: PermissionMode,
) -> Result<()> {
    // Convert to absolute path if needed
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        // Fallback: join with project_dir (tools usually resolve paths before calling this)
        project_dir.join(path)
    };

    // Attempt to canonicalize to resolve symlinks
    // If file doesn't exist, we use the absolute path as-is, but we try to canonicalize parent
    let check_path = if let Ok(canon) = abs_path.canonicalize() {
        canon
    } else {
        // If path doesn't exist, try to canonicalize parent to resolve at least directory symlinks
        if let Some(parent) = abs_path.parent() {
            if let Ok(canon_parent) = parent.canonicalize() {
                canon_parent.join(abs_path.file_name().unwrap())
            } else {
                abs_path.clone()
            }
        } else {
            abs_path.clone()
        }
    };

    // Collect allowed roots
    let mut allowed_roots = Vec::new();

    // 1. Workspace
    if let Ok(p) = workspace.canonicalize() {
        allowed_roots.push(p);
    } else {
        allowed_roots.push(workspace.to_path_buf());
    }

    // 2. Project Dir
    if let Ok(p) = project_dir.canonicalize() {
        allowed_roots.push(p);
    } else {
        allowed_roots.push(project_dir.to_path_buf());
    }

    // 3. Policy paths
    let policy_paths = match mode {
        PermissionMode::Read => &policy.allow_read,
        PermissionMode::Write => &policy.allow_write,
    };

    for p_str in policy_paths {
        let expanded = shellexpand::tilde(p_str);
        let p_buf = PathBuf::from(expanded.to_string());

        let p_abs = if p_buf.is_absolute() {
            p_buf
        } else {
            project_dir.join(p_buf)
        };

        if let Ok(canon) = p_abs.canonicalize() {
            allowed_roots.push(canon);
        } else {
            allowed_roots.push(p_abs);
        }
    }

    // Check permissions
    for root in allowed_roots {
        if check_path.starts_with(&root) {
            return Ok(());
        }
    }

    Err(anyhow::anyhow!(
        "Path access denied: {}. Allowed roots: workspace, project, or configured policy.",
        path.display()
    ))
}
