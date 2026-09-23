use super::error::ConfinementError;
use super::path::text;
use super::resolved::Resolved;

const READ_TREES: [&str; 4] = ["/usr", "/bin", "/lib", "/lib64"];
const LOADER_FILES: [&str; 3] = ["/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/localtime"];

pub(super) fn arguments(resolved: &Resolved) -> Result<Vec<String>, ConfinementError> {
    let mut arguments: Vec<String> = [
        "--die-with-parent",
        "--new-session",
        "--unshare-all",
        "--unshare-net",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]
    .iter()
    .map(|value| value.to_string())
    .collect();

    for tree in READ_TREES {
        arguments.extend([
            "--ro-bind-try".to_string(),
            tree.to_string(),
            tree.to_string(),
        ]);
    }
    arguments.extend(["--dir".to_string(), "/etc".to_string()]);
    for file in LOADER_FILES {
        arguments.extend([
            "--ro-bind-try".to_string(),
            file.to_string(),
            file.to_string(),
        ]);
    }
    for root in &resolved.read_roots {
        let path = text(root)?.to_string();
        arguments.extend(["--ro-bind".to_string(), path.clone(), path]);
    }
    for root in &resolved.write_roots {
        let path = text(root)?.to_string();
        arguments.extend(["--bind".to_string(), path.clone(), path]);
    }
    for root in &resolved.execute_roots {
        let path = text(root)?.to_string();
        arguments.extend(["--ro-bind".to_string(), path.clone(), path]);
    }
    arguments.extend([
        "--chdir".to_string(),
        text(&resolved.working_dir)?.to_string(),
        "--".to_string(),
        text(&resolved.command)?.to_string(),
    ]);
    arguments.extend(resolved.arguments.iter().cloned());

    Ok(arguments)
}
