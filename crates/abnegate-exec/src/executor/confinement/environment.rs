use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use super::error::ConfinementError;
use super::path::executable_file;
use super::path::text;

const HOME: &str = "HOME";
const TMPDIR: &str = "TMPDIR";
const TMP: &str = "TMP";
const TEMP: &str = "TEMP";
const PATH: &str = "PATH";
const LANG: &str = "LANG";
const LC_ALL: &str = "LC_ALL";

const SYSTEM_PATH: &str = "/usr/bin:/bin";
const LOCALE: &str = "C.UTF-8";

pub(super) fn resolve_command(
    command: &str,
    environment: &BTreeMap<String, String>,
) -> Result<PathBuf, ConfinementError> {
    if command.contains('/') {
        return executable_file(Path::new(command))
            .ok_or_else(|| ConfinementError::CommandNotFound(command.to_string()))
            .and_then(|path| {
                text(&path)?;
                Ok(path)
            });
    }

    let search = environment
        .get(PATH)
        .cloned()
        .or_else(|| std::env::var(PATH).ok())
        .unwrap_or_else(|| SYSTEM_PATH.to_string());

    search
        .split(':')
        .filter(|directory| !directory.is_empty())
        .find_map(|directory| executable_file(&Path::new(directory).join(command)))
        .ok_or_else(|| ConfinementError::CommandNotFound(command.to_string()))
}

pub(super) fn complete_environment(
    requested: &BTreeMap<String, String>,
    command: &Path,
    writable: &Path,
) -> BTreeMap<String, String> {
    let mut environment = requested.clone();
    let writable = writable.display().to_string();
    for name in [HOME, TMPDIR, TMP, TEMP] {
        environment
            .entry(name.to_string())
            .or_insert_with(|| writable.clone());
    }
    environment
        .entry(PATH.to_string())
        .or_insert_with(|| command_path(command));
    for name in [LANG, LC_ALL] {
        environment
            .entry(name.to_string())
            .or_insert_with(|| LOCALE.to_string());
    }
    environment
}

fn command_path(command: &Path) -> String {
    let mut entries: Vec<&str> = Vec::with_capacity(3);
    if let Some(directory) = command.parent().and_then(Path::to_str) {
        entries.push(directory);
    }
    for entry in SYSTEM_PATH.split(':') {
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    entries.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_path_prefixes_the_command_directory() {
        assert_eq!(
            command_path(Path::new("/opt/tools/bin/run")),
            "/opt/tools/bin:/usr/bin:/bin"
        );
    }

    #[test]
    fn test_command_path_does_not_repeat_system_entries() {
        assert_eq!(command_path(Path::new("/bin/cat")), "/bin:/usr/bin");
    }

    #[test]
    fn test_complete_environment_fills_defaults() {
        let environment = complete_environment(
            &BTreeMap::new(),
            Path::new("/bin/cat"),
            Path::new("/tmp/writable"),
        );
        assert_eq!(environment.get(HOME).unwrap(), "/tmp/writable");
        assert_eq!(environment.get(TMPDIR).unwrap(), "/tmp/writable");
        assert_eq!(environment.get(LC_ALL).unwrap(), LOCALE);
    }

    #[test]
    fn test_complete_environment_keeps_requested_values() {
        let requested = BTreeMap::from([(HOME.to_string(), "/tmp/mine".to_string())]);
        let environment = complete_environment(
            &requested,
            Path::new("/bin/cat"),
            Path::new("/tmp/writable"),
        );
        assert_eq!(environment.get(HOME).unwrap(), "/tmp/mine");
    }

    #[test]
    fn test_resolve_command_finds_absolute_path() {
        let resolved = resolve_command("/bin/cat", &BTreeMap::new()).unwrap();
        assert!(resolved.ends_with("cat"));
    }

    #[test]
    fn test_resolve_command_rejects_missing_command() {
        assert_eq!(
            resolve_command("/bin/definitely-not-a-command", &BTreeMap::new()),
            Err(ConfinementError::CommandNotFound(
                "/bin/definitely-not-a-command".to_string()
            ))
        );
    }

    #[test]
    fn test_resolve_command_searches_the_requested_path() {
        let environment = BTreeMap::from([(PATH.to_string(), "/bin".to_string())]);
        assert!(resolve_command("cat", &environment).is_ok());
    }
}
