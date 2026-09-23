use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::ConfigError;

#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

/// A `.env` file whose keys can be upserted without disturbing the rest of it.
///
/// Comments, blank lines, and the order of existing keys survive an update;
/// keys that are not already there are appended. The file is written back
/// readable only by its owner, because these files hold credentials.
pub struct EnvironmentFile {
    path: PathBuf,
}

impl EnvironmentFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every key the file defines. A file that is not there reads as empty.
    pub fn read(&self) -> Result<BTreeMap<String, String>, ConfigError> {
        Ok(self
            .content()?
            .lines()
            .filter_map(parse)
            .collect::<BTreeMap<String, String>>())
    }

    /// Set every key in `values`, creating the file if it is not there.
    pub fn update(&self, values: &BTreeMap<String, String>) -> Result<(), ConfigError> {
        let updated = apply(&self.content()?, values);

        fs::write(&self.path, updated).map_err(|source| ConfigError::Write {
            path: self.path.clone(),
            source,
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&self.path, fs::Permissions::from_mode(FILE_MODE)).map_err(
                |source| ConfigError::Write {
                    path: self.path.clone(),
                    source,
                },
            )?;
        }

        Ok(())
    }

    fn content(&self) -> Result<String, ConfigError> {
        if !self.path.exists() {
            return Ok(String::new());
        }

        fs::read_to_string(&self.path).map_err(|source| ConfigError::Read {
            path: self.path.clone(),
            source,
        })
    }
}

fn apply(content: &str, values: &BTreeMap<String, String>) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut written: BTreeSet<String> = BTreeSet::new();

    for line in content.lines() {
        match parse(line).and_then(|(key, _)| values.get_key_value(&key)) {
            Some((key, value)) => {
                lines.push(format!("{key}={}", quote(value)));
                written.insert(key.clone());
            }
            None => lines.push(line.to_string()),
        }
    }

    let appended: Vec<(&String, &String)> = values
        .iter()
        .filter(|(key, _)| !written.contains(*key))
        .collect();

    if !appended.is_empty() {
        if let Some(last) = lines.last()
            && !last.trim().is_empty()
        {
            lines.push(String::new());
        }

        for (key, value) in appended {
            lines.push(format!("{key}={}", quote(value)));
        }
    }

    let mut result = lines.join("\n");
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    result
}

fn parse(line: &str) -> Option<(String, String)> {
    let line = line.trim();

    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let separator = line.find('=')?;
    let key = line[..separator].trim().to_string();
    let value = line[separator + 1..].trim();

    let quoted = (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''));
    let value = if quoted && value.len() >= 2 {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    };

    Some((key, value))
}

fn quote(value: &str) -> String {
    let special = [' ', '"', '\'', '#', '$', '\n', '\\'];

    if value.contains(special) {
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_simple_line_parses() {
        assert_eq!(
            parse("KEY=value"),
            Some(("KEY".to_string(), "value".to_string()))
        );
    }

    #[test]
    fn surrounding_space_is_trimmed() {
        assert_eq!(
            parse("  KEY  =  value  "),
            Some(("KEY".to_string(), "value".to_string()))
        );
    }

    #[test]
    fn a_double_quoted_value_parses() {
        assert_eq!(
            parse("KEY=\"quoted value\""),
            Some(("KEY".to_string(), "quoted value".to_string()))
        );
    }

    #[test]
    fn a_single_quoted_value_parses() {
        assert_eq!(
            parse("KEY='quoted value'"),
            Some(("KEY".to_string(), "quoted value".to_string()))
        );
    }

    #[test]
    fn an_empty_value_parses() {
        assert_eq!(parse("KEY="), Some(("KEY".to_string(), String::new())));
    }

    #[test]
    fn a_lone_quote_is_not_a_quoted_value() {
        assert_eq!(parse("KEY=\""), Some(("KEY".to_string(), "\"".to_string())));
    }

    #[test]
    fn only_the_first_separator_splits_the_line() {
        assert_eq!(
            parse("KEY=value=with=equals"),
            Some(("KEY".to_string(), "value=with=equals".to_string()))
        );
    }

    #[test]
    fn comments_and_blank_lines_are_not_keys() {
        assert!(parse("# comment").is_none());
        assert!(parse("").is_none());
        assert!(parse("   ").is_none());
        assert!(parse("NOEQUALS").is_none());
    }

    #[test]
    fn a_plain_value_is_not_quoted() {
        assert_eq!(quote("simple"), "simple");
    }

    #[test]
    fn a_value_the_shell_would_read_is_quoted() {
        assert_eq!(quote("with spaces"), "\"with spaces\"");
        assert_eq!(quote("with#hash"), "\"with#hash\"");
        assert_eq!(quote("value$var"), "\"value$var\"");
    }

    #[test]
    fn quotes_and_backslashes_are_escaped() {
        assert_eq!(quote("with\"quote"), "\"with\\\"quote\"");
        assert_eq!(quote("path\\to\\file"), "\"path\\\\to\\\\file\"");
    }

    #[test]
    fn a_new_key_is_appended() {
        let updated = apply("EXISTING=value\n", &values(&[("NEW_KEY", "new_value")]));

        assert!(updated.contains("EXISTING=value"));
        assert!(updated.contains("NEW_KEY=new_value"));
    }

    #[test]
    fn an_existing_key_is_replaced() {
        let updated = apply("KEY=old_value\n", &values(&[("KEY", "new_value")]));

        assert!(updated.contains("KEY=new_value"));
        assert!(!updated.contains("old_value"));
    }

    #[test]
    fn comments_survive_an_update() {
        let updated = apply(
            "# This is a comment\nKEY=value\n",
            &values(&[("KEY", "new_value")]),
        );

        assert!(updated.contains("# This is a comment"));
        assert!(updated.contains("KEY=new_value"));
    }

    #[test]
    fn blank_lines_survive_an_update() {
        let updated = apply("KEY1=value1\n\nKEY2=value2\n", &values(&[("KEY1", "new1")]));

        assert!(updated.contains("KEY1=new1"));
        assert!(updated.contains("\n\n"));
    }

    #[test]
    fn an_empty_file_takes_the_new_keys() {
        let updated = apply("", &values(&[("NEW_KEY", "value")]));

        assert_eq!(updated, "NEW_KEY=value\n");
    }

    #[test]
    fn updates_and_appends_happen_together() {
        let updated = apply(
            "KEY1=old1\nKEY2=old2\n",
            &values(&[("KEY1", "new1"), ("KEY3", "new3")]),
        );

        assert!(updated.contains("KEY1=new1"));
        assert!(updated.contains("KEY2=old2"));
        assert!(updated.contains("KEY3=new3"));
    }

    #[test]
    fn existing_keys_keep_their_order() {
        let updated = apply(
            "FIRST=1\nSECOND=2\nTHIRD=3\n",
            &values(&[("SECOND", "updated")]),
        );

        let first = updated.find("FIRST").unwrap();
        let second = updated.find("SECOND").unwrap();
        let third = updated.find("THIRD").unwrap();

        assert!(first < second);
        assert!(second < third);
    }

    #[test]
    fn appended_keys_are_ordered() {
        let updated = apply("", &values(&[("BETA", "2"), ("ALPHA", "1")]));

        assert_eq!(updated, "ALPHA=1\nBETA=2\n");
    }

    #[test]
    fn the_file_ends_with_a_newline() {
        assert!(apply("KEY=value", &BTreeMap::new()).ends_with('\n'));
    }

    #[test]
    fn an_update_creates_the_file() {
        let directory = TempDir::new().unwrap();
        let file = EnvironmentFile::new(directory.path().join(".env"));

        file.update(&values(&[("KEY", "value")])).unwrap();

        assert_eq!(file.read().unwrap(), values(&[("KEY", "value")]));
    }

    #[test]
    fn an_update_rewrites_an_existing_file() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join(".env");
        fs::write(&path, "EXISTING=old\n").unwrap();
        let file = EnvironmentFile::new(&path);

        file.update(&values(&[("EXISTING", "new"), ("NEW_KEY", "value")]))
            .unwrap();

        assert_eq!(
            file.read().unwrap(),
            values(&[("EXISTING", "new"), ("NEW_KEY", "value")])
        );
    }

    #[test]
    fn updating_twice_changes_nothing_the_second_time() {
        let directory = TempDir::new().unwrap();
        let file = EnvironmentFile::new(directory.path().join(".env"));
        let updates = values(&[("KEY", "value"), ("OTHER", "with spaces")]);

        file.update(&updates).unwrap();
        let once = fs::read_to_string(file.path()).unwrap();
        file.update(&updates).unwrap();

        assert_eq!(fs::read_to_string(file.path()).unwrap(), once);
    }

    #[test]
    fn a_file_that_is_not_there_reads_as_empty() {
        let directory = TempDir::new().unwrap();
        let file = EnvironmentFile::new(directory.path().join(".env"));

        assert!(file.read().unwrap().is_empty());
    }

    #[test]
    fn reading_skips_comments() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join(".env");
        fs::write(&path, "# comment\nKEY=value\n\n").unwrap();

        assert_eq!(
            EnvironmentFile::new(&path).read().unwrap(),
            values(&[("KEY", "value")])
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let file = EnvironmentFile::new(directory.path().join(".env"));
        file.update(&values(&[("TOKEN", "secret")])).unwrap();

        let mode = fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            FILE_MODE,
            "environment file is world readable"
        );
    }
}
