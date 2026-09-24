mod value;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use crate::environment::value::is_key;
use crate::environment::value::quote;
use crate::environment::value::unquote;
use crate::error::Error;
use crate::private_file::PrivateFile;

const SEPARATOR: char = '=';
const COMMENT: char = '#';

/// A `.env` file whose keys can be upserted without disturbing the rest of it.
///
/// Comments, blank lines, and the order of existing keys survive an update;
/// keys that are not already there are appended. The file is replaced
/// atomically and is readable only by its owner, because these files hold
/// credentials; a symlink at the path is replaced, never written through.
///
/// Keys must be shell variable names. Every value reads back unchanged, and a
/// POSIX shell sourcing the file expands and executes nothing in it:
///
/// - letters, digits and `_-./:@%+,` alone are written bare;
/// - anything else without a `'` or a line break is single quoted, so `$`,
///   `` ` `` and `\` are literal;
/// - the rest are double quoted with `\`, `"`, `$` and `` ` `` escaped by a
///   backslash and line breaks written as `\n` and `\r`, so a value can never
///   spill onto a line of its own. A shell reads those two escapes as the
///   characters themselves; dotenv readers, and this one, as line breaks.
///
/// ```
/// use std::collections::BTreeMap;
///
/// use abnegate_config::EnvironmentFile;
///
/// let directory = tempfile::tempdir()?;
/// let file = EnvironmentFile::new(directory.path().join(".env"));
/// let values = BTreeMap::from([
///     ("PASSWORD".to_string(), "pa$$word".to_string()),
///     ("TOKEN".to_string(), "line\nADMIN=1".to_string()),
/// ]);
///
/// file.update(&values)?;
///
/// assert_eq!(
///     std::fs::read_to_string(file.path())?,
///     "PASSWORD='pa$$word'\nTOKEN=\"line\\nADMIN=1\"\n"
/// );
/// assert_eq!(file.read()?, values);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
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

    /// Every key the file defines. A file that is not there reads as empty,
    /// and a line that is not a `KEY=value` assignment is skipped.
    pub fn read(&self) -> Result<BTreeMap<String, String>, Error> {
        Ok(self
            .content()?
            .lines()
            .filter_map(parse)
            .collect::<BTreeMap<String, String>>())
    }

    /// Set every key in `values`, creating the file if it is not there.
    ///
    /// Fails with [`Error::InvalidKey`], writing nothing, when a key is
    /// not a shell variable name.
    pub fn update(&self, values: &BTreeMap<String, String>) -> Result<(), Error> {
        if let Some(key) = values.keys().find(|key| !is_key(key)) {
            return Err(Error::InvalidKey { key: key.clone() });
        }

        PrivateFile::new(&self.path).write(apply(&self.content()?, values).as_bytes())
    }

    fn content(&self) -> Result<String, Error> {
        match fs::read_to_string(&self.path) {
            Ok(content) => Ok(content),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
            Err(source) => Err(Error::Read {
                path: self.path.clone(),
                source,
            }),
        }
    }
}

fn apply(content: &str, values: &BTreeMap<String, String>) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut written: BTreeSet<&String> = BTreeSet::new();

    for line in content.lines() {
        match parse(line).and_then(|(key, _)| values.get_key_value(&key)) {
            Some((key, value)) => {
                lines.push(assignment(key, value));
                written.insert(key);
            }
            None => lines.push(line.to_string()),
        }
    }

    let appended: Vec<(&String, &String)> = values
        .iter()
        .filter(|(key, _)| !written.contains(key))
        .collect();

    if !appended.is_empty() {
        if let Some(last) = lines.last()
            && !last.trim().is_empty()
        {
            lines.push(String::new());
        }

        for (key, value) in appended {
            lines.push(assignment(key, value));
        }
    }

    let mut result = lines.join("\n");
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }

    result
}

fn assignment(key: &str, value: &str) -> String {
    format!("{key}{SEPARATOR}{}", quote(value))
}

fn parse(line: &str) -> Option<(String, String)> {
    let line = line.trim();

    if line.is_empty() || line.starts_with(COMMENT) {
        return None;
    }

    let (key, value) = line.split_once(SEPARATOR)?;
    let key = key.trim();

    is_key(key).then(|| (key.to_string(), unquote(value.trim())))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    const ALPHABET: [char; 16] = [
        'a', ' ', '\t', '\n', '\r', '"', '\'', '\\', '$', '`', '#', '=', 'n', 'é', '\u{2028}', '\0',
    ];
    const LONGEST: u32 = 4;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect()
    }

    fn every_value_up_to(longest: u32) -> impl Iterator<Item = String> {
        (0..=longest).flat_map(|length| {
            (0..ALPHABET.len().pow(length)).map(move |mut ordinal| {
                (0..length)
                    .map(|_| {
                        let character = ALPHABET[ordinal % ALPHABET.len()];
                        ordinal /= ALPHABET.len();
                        character
                    })
                    .collect()
            })
        })
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
    fn a_double_quoted_value_is_unescaped() {
        assert_eq!(
            parse("KEY=\"line\\nbreak \\\"quoted\\\" \\$HOME\""),
            Some((
                "KEY".to_string(),
                "line\nbreak \"quoted\" $HOME".to_string()
            ))
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
    fn a_line_whose_key_is_not_a_variable_name_is_not_an_assignment() {
        assert!(parse("1KEY=value").is_none());
        assert!(parse("KEY NAME=value").is_none());
        assert!(parse("=value").is_none());
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
    fn a_line_break_in_a_value_cannot_inject_a_key() {
        let updated = apply(
            "",
            &values(&[("TOKEN", "harmless\nADMIN_TOKEN=injected\r\nOTHER=x")]),
        );

        assert_eq!(updated.lines().count(), 1, "{updated:?}");
        let read: BTreeMap<String, String> = updated.lines().filter_map(parse).collect();
        assert_eq!(
            read,
            values(&[("TOKEN", "harmless\nADMIN_TOKEN=injected\r\nOTHER=x")])
        );
    }

    #[test]
    fn every_value_round_trips_on_a_line_of_its_own() {
        let mut checked = 0;

        for value in every_value_up_to(LONGEST) {
            let written = apply("", &values(&[("KEY", &value)]));

            assert_eq!(
                written.lines().count(),
                1,
                "{value:?} was written as {written:?}"
            );
            assert_eq!(
                written.lines().filter_map(parse).collect::<Vec<_>>(),
                [("KEY".to_string(), value.clone())],
                "{value:?} was written as {written:?}"
            );
            checked += 1;
        }

        assert_eq!(
            checked,
            (0..=LONGEST)
                .map(|length| ALPHABET.len().pow(length))
                .sum::<usize>()
        );
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
    fn a_tricky_value_survives_the_file() {
        let directory = TempDir::new().unwrap();
        let file = EnvironmentFile::new(directory.path().join(".env"));
        let tricky = values(&[
            ("NEWLINE", "one\ntwo"),
            ("DOLLAR", "$HOME and ${PATH}"),
            ("QUOTES", "it's \"quoted\""),
            ("BACKSLASH", "C:\\path\\n"),
        ]);

        file.update(&tricky).unwrap();

        assert_eq!(file.read().unwrap(), tricky);
    }

    #[test]
    fn an_invalid_key_is_refused_before_anything_is_written() {
        let directory = TempDir::new().unwrap();
        let file = EnvironmentFile::new(directory.path().join(".env"));

        for key in ["1KEY", "KEY-NAME", "KEY\nADMIN", "KEY=VALUE", ""] {
            let error = file
                .update(&values(&[("VALID", "value"), (key, "value")]))
                .unwrap_err();

            assert!(
                matches!(&error, Error::InvalidKey { key: refused } if refused == key),
                "{key:?}: {error:?}"
            );
            assert!(!file.path().exists());
        }
    }

    #[test]
    fn updating_twice_changes_nothing_the_second_time() {
        let directory = TempDir::new().unwrap();
        let file = EnvironmentFile::new(directory.path().join(".env"));
        let updates = values(&[
            ("KEY", "value"),
            ("OTHER", "with spaces"),
            ("MULTI", "line\nvalue"),
        ]);

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
        let path = directory.path().join(".env");
        fs::write(&path, "TOKEN=old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let file = EnvironmentFile::new(&path);

        file.update(&values(&[("TOKEN", "secret")])).unwrap();

        let mode = fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "environment file is world readable");
    }

    #[cfg(unix)]
    #[test]
    fn an_update_does_not_write_through_a_symlink() {
        let directory = TempDir::new().unwrap();
        let target = directory.path().join("authorized_keys");
        let path = directory.path().join(".env");
        fs::write(&target, "ssh-ed25519 AAAA person@example.com\n").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        EnvironmentFile::new(&path)
            .update(&values(&[("TOKEN", "secret")]))
            .unwrap();

        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "ssh-ed25519 AAAA person@example.com\n"
        );
        assert!(
            !fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_sourcing_the_file_reads_every_value_literally() {
        use std::process::Command;

        let directory = TempDir::new().unwrap();
        let path = directory.path().join(".env");
        let witness = directory.path().join("executed");
        let witness = witness.to_str().unwrap();
        let hostile = values(&[
            ("DOLLAR", "$HOME"),
            ("BRACED", "${HOME}"),
            ("SUBSTITUTION", &format!("$(touch {witness})")),
            ("BACKTICK", &format!("`touch {witness}`")),
            ("MIXED", &format!("it's $(touch {witness}) `id` \"$HOME\"")),
            ("BACKSLASH", "a\\b\\\\c"),
            ("HASH", "value # not a comment"),
            ("SPACES", "  padded  "),
        ]);
        EnvironmentFile::new(&path).update(&hostile).unwrap();

        let script = hostile
            .keys()
            .map(|key| format!("printf '%s\\0' \"${key}\""))
            .collect::<Vec<_>>()
            .join("; ");
        let output = Command::new("sh")
            .arg("-c")
            .arg(format!(". \"$0\"; {script}"))
            .arg(&path)
            .output()
            .unwrap();

        assert!(output.status.success(), "{output:?}");
        let sourced: Vec<&str> = std::str::from_utf8(&output.stdout)
            .unwrap()
            .split_terminator('\0')
            .collect();
        let expected: Vec<&str> = hostile.values().map(String::as_str).collect();
        assert_eq!(sourced, expected);
        assert!(
            !Path::new(witness).exists(),
            "sourcing the file ran a command"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_sourcing_the_file_sees_no_injected_key() {
        use std::process::Command;

        let directory = TempDir::new().unwrap();
        let path = directory.path().join(".env");
        EnvironmentFile::new(&path)
            .update(&values(&[("TOKEN", "harmless\nADMIN_TOKEN=injected")]))
            .unwrap();

        let output = Command::new("sh")
            .arg("-c")
            .arg(". \"$0\"; printf '%s' \"${ADMIN_TOKEN-unset}\"")
            .arg(&path)
            .output()
            .unwrap();

        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), "unset");
    }
}
