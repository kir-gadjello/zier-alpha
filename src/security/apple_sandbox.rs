use crate::config::SandboxPolicy;

pub fn compile_profile(policy: &SandboxPolicy, executable_path: &str, script_path: &str) -> String {
    let mut sbpl = String::from("(version 1)\n(deny default)\n(debug deny)\n");

    // Base Allowances
    // Needed for the interpreter to run and for basic system functionality
    sbpl.push_str("(allow process-exec*)\n");
    sbpl.push_str("(allow sysctl-read)\n");
    sbpl.push_str("(allow signal)\n");

    // Allow reading basic system files needed for dynamic linking and locale
    sbpl.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
    sbpl.push_str("(allow file-read* (subpath \"/System/Library\"))\n");
    sbpl.push_str("(allow file-read* (subpath \"/private/var/db/timezone\"))\n"); // Common for time handling

    // Network
    if policy.allow_network {
        sbpl.push_str("(allow network*)\n");
        sbpl.push_str("(allow system-socket)\n");
    }

    // Always allow reading the executable itself and the script being executed
    sbpl.push_str(&format!("(allow file-read* (literal \"{}\"))\n", executable_path));
    sbpl.push_str(&format!("(allow file-read* (literal \"{}\"))\n", script_path));

    // User-defined Read Paths
    for path in &policy.allow_read {
        append_path_rule(&mut sbpl, "file-read*", path);
    }

    // User-defined Write Paths
    for path in &policy.allow_write {
        append_path_rule(&mut sbpl, "file-write*", path);
    }

    sbpl
}

fn append_path_rule(sbpl: &mut String, rule_type: &str, path: &str) {
    // Handle globs roughly
    let (directive, clean_path) = if path.ends_with("**") {
        ("subpath", path.trim_end_matches("**").trim_end_matches('/'))
    } else if path.ends_with("*") {
        // "subpath" covers everything under it, so * acts like ** in SBPL context usually
        ("subpath", path.trim_end_matches('*').trim_end_matches('/'))
    } else {
        ("literal", path)
    };

    // Expand tilde if present (simple expansion)
    let expanded_path = shellexpand::tilde(clean_path).to_string();

    sbpl.push_str(&format!("(allow {} ({} \"{}\"))\n", rule_type, directive, expanded_path));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_profile_basic() {
        let policy = SandboxPolicy {
            allow_network: false,
            allow_read: vec!["/tmp".to_string()],
            allow_write: vec![],
            allow_env: false,
        };
        let profile = compile_profile(&policy, "/usr/bin/python3", "/tmp/script.py");

        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(allow process-exec*)"));
        assert!(profile.contains("(allow file-read* (literal \"/usr/bin/python3\"))"));
        assert!(profile.contains("(allow file-read* (literal \"/tmp/script.py\"))"));
        assert!(profile.contains("(allow file-read* (literal \"/tmp\"))"));
        assert!(!profile.contains("(allow network*)"));
    }

    #[test]
    fn test_compile_profile_network() {
        let policy = SandboxPolicy {
            allow_network: true,
            allow_read: vec![],
            allow_write: vec![],
            allow_env: false,
        };
        let profile = compile_profile(&policy, "/bin/bash", "/tmp/script.sh");

        assert!(profile.contains("(allow network*)"));
        assert!(profile.contains("(allow system-socket)"));
    }
}
