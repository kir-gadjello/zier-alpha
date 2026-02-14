use std::path::PathBuf;
use zier_alpha::config::SandboxPolicy;

/// Construct a SandboxPolicy for extension/script execution.
/// This policy is used by commands that load and run Deno-based extensions (e.g., Hive).
pub fn make_extension_policy(project_dir: &PathBuf, workspace: &PathBuf) -> SandboxPolicy {
    let mut policy = SandboxPolicy::default();

    // Extensions typically need to read environment variables (e.g., HOME, ZIER_*)
    policy.allow_env = true;

    // Allow access to temporary directory for IPC files, hydration temp files, etc.
    let temp_dir = std::env::temp_dir().to_string_lossy().to_string();
    policy.allow_read.push(temp_dir.clone());
    policy.allow_write.push(temp_dir);

    // Allow full access to workspace and project directories (cognitive routing applies)
    policy
        .allow_read
        .push(workspace.to_string_lossy().to_string());
    policy
        .allow_write
        .push(workspace.to_string_lossy().to_string());
    policy
        .allow_read
        .push(project_dir.to_string_lossy().to_string());
    policy
        .allow_write
        .push(project_dir.to_string_lossy().to_string());

    // If an extensions directory exists under home, allow read access
    if let Some(home) = directories::BaseDirs::new() {
        let ext_dir = home.home_dir().join(".zier-alpha/extensions");
        if ext_dir.exists() {
            policy
                .allow_read
                .push(ext_dir.to_string_lossy().to_string());
        }
    }

    policy
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_make_extension_policy() {
        // Basic sanity test
        let project = std::path::PathBuf::from("/project");
        let workspace = std::path::PathBuf::from("/workspace");
        let policy = super::make_extension_policy(&project, &workspace);
        assert!(policy.allow_env);
        assert!(policy.allow_read.contains(&"/workspace".to_string()));
        assert!(policy.allow_read.contains(&"/project".to_string()));
        assert!(policy
            .allow_read
            .contains(&std::env::temp_dir().to_string_lossy().to_string()));
    }
}
