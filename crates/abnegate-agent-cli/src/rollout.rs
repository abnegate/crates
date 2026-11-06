//! Reading the window a Codex turn ran in from the rollout Codex keeps of
//! its session.
//!
//! `codex exec --json` reports a turn's tokens but not its model's context
//! window. Codex writes both, as `model_context_window` and
//! `last_token_usage` on each `token_count` event, to
//! `$CODEX_HOME/sessions/YYYY/MM/DD/rollout-<time>-<thread>.jsonl`, and
//! publishes each model's window in `$CODEX_HOME/models_cache.json`.

use std::ffi::OsString;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use nix::fcntl::OFlag;
use serde_json::Value;

use crate::environment::Environment;
use crate::occupancy::Occupancy;

/// The longest a finished Codex run waits on its rollout to learn its
/// window.
const ROLLOUT_TIMEOUT: Duration = Duration::from_secs(5);

const CODEX_HOME: &str = "CODEX_HOME";
const USER_HOME: &str = "HOME";
const DEFAULT_HOME: &str = ".codex";
const SESSIONS: &str = "sessions";
const CATALOGUE: &str = "models_cache.json";
const PREFIX: &str = "rollout-";
const EXTENSION: &str = ".jsonl";
const EVENT: &str = "event_msg";
const TOKEN_COUNT: &str = "token_count";
const PERCENT: u64 = 100;

/// How much of a rollout's end is read first. Codex writes a `token_count`
/// after every request, so the last one is among the final lines of a turn.
const TAIL: u64 = 64 * 1024;

/// The most of a rollout's end ever read, and the largest catalogue read. A
/// resumed session's rollout holds its whole history and only grows.
const LIMIT: u64 = 4 * 1024 * 1024;

/// Where one Codex run keeps its sessions, and the model it was asked for,
/// read once the run has exited by itself.
///
/// Nothing found under the home is trusted to be what its name says: the
/// agent can write there, and a link, device or pipe in a file's place would
/// hold the read, so links are never followed, the sessions folder included,
/// and a file is opened without following a link or waiting on a pipe and
/// read only when what was opened is a regular file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rollout {
    home: PathBuf,
    model: Option<String>,
}

impl Rollout {
    /// Where a child given `environment` keeps its sessions: its
    /// `CODEX_HOME`, or else `.codex` under its `HOME`, a relative one taken
    /// from the child's `working_directory` as the child would take it.
    /// `host` reads this process's variables as it did when the environment
    /// was built.
    pub(crate) fn locate(
        environment: &Environment,
        host: &dyn Fn(&str) -> Option<OsString>,
        working_directory: Option<&Path>,
        model: Option<&str>,
    ) -> Option<Self> {
        let given = |name: &str| {
            environment
                .variable(name, host)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home =
            given(CODEX_HOME).or_else(|| given(USER_HOME).map(|home| home.join(DEFAULT_HOME)))?;
        let home = match working_directory {
            Some(directory) if home.is_relative() => directory.join(home),
            _ => home,
        };
        Some(Self {
            home,
            model: model.map(str::to_string),
        })
    }

    /// The window and fill Codex last reported for `thread` in its rollout,
    /// or else the window it publishes for the run's model, with nothing
    /// said of its fill. The files are read off the runtime, and given up on
    /// past [`ROLLOUT_TIMEOUT`].
    pub(crate) async fn occupancy(self, thread: String) -> Option<Occupancy> {
        #[cfg(test)]
        read::record(&self.home);
        let home = self.home.clone();
        let reading = tokio::task::spawn_blocking(move || self.read(&thread));
        match tokio::time::timeout(ROLLOUT_TIMEOUT, reading).await {
            Ok(Ok(found)) => found,
            Ok(Err(error)) => {
                tracing::debug!(home = %home.display(), %error, "reading the codex rollout failed");
                None
            }
            Err(_) => {
                tracing::debug!(home = %home.display(), "gave up reading the codex rollout");
                None
            }
        }
    }

    fn read(&self, thread: &str) -> Option<Occupancy> {
        let sessions = self.home.join(SESSIONS);
        let reported = fs::symlink_metadata(&sessions)
            .is_ok_and(|metadata| metadata.is_dir())
            .then_some(&sessions)
            .and_then(|sessions| locate(sessions, thread))
            .and_then(|path| tail(&path));
        if reported.is_some() {
            return reported;
        }
        tracing::debug!(home = %self.home.display(), thread, "no codex rollout reports this thread's window");
        self.model
            .as_deref()
            .and_then(|model| published(&self.home.join(CATALOGUE), model))
    }
}

/// The rollout of `thread`, looked for in the newest day first: a resumed
/// session keeps appending to the file of the day it began.
fn locate(sessions: &Path, thread: &str) -> Option<PathBuf> {
    let suffix = format!("-{thread}{EXTENSION}");
    for year in directories(sessions) {
        for month in directories(&year) {
            for day in directories(&month) {
                let Ok(entries) = fs::read_dir(&day) else {
                    continue;
                };
                let found = entries.flatten().find(|entry| {
                    entry.file_type().is_ok_and(|kind| kind.is_file())
                        && entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.starts_with(PREFIX) && name.ends_with(&suffix))
                });
                if let Some(found) = found {
                    return Some(found.path());
                }
            }
        }
    }
    None
}

/// The directories in `directory`, newest name first, never through a link.
fn directories(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    found.sort_unstable_by(|left, right| right.cmp(left));
    found
}

/// `path` opened only when it is still a regular file once open, with its
/// length. A link in its place is never followed, and a pipe is opened
/// without waiting for a writer, so neither holds the read.
fn regular(path: &Path) -> Option<(File, u64)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK).bits())
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    metadata.is_file().then_some((file, metadata.len()))
}

/// The last window `path` reports, read from its end alone: a tail of
/// [`TAIL`], doubled until a report turns up, the file's start is reached, or
/// the tail is [`LIMIT`] long.
fn tail(path: &Path) -> Option<Occupancy> {
    let (mut file, length) = regular(path)?;
    let mut span = TAIL;
    loop {
        let start = length.saturating_sub(span);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut bytes = Vec::new();
        file.by_ref().take(span).read_to_end(&mut bytes).ok()?;
        let text = String::from_utf8_lossy(&bytes);
        let whole = match start {
            0 => &text[..],
            _ => text.split_once('\n').map_or("", |(_, rest)| rest),
        };
        if let Some(reported) = last(whole) {
            return Some(reported);
        }
        if start == 0 || span >= LIMIT {
            return None;
        }
        span = (span * 2).min(LIMIT);
    }
}

/// The window the last `token_count` in `content` with a report of its own
/// names, filled as far as its last request filled it.
fn last(content: &str) -> Option<Occupancy> {
    content
        .lines()
        .rev()
        .filter(|line| line.contains(TOKEN_COUNT))
        .find_map(|line| {
            let line: Value = serde_json::from_str(line).ok()?;
            if line.get("type")?.as_str()? != EVENT
                || line.pointer("/payload/type")?.as_str()? != TOKEN_COUNT
            {
                return None;
            }
            let info = line.pointer("/payload/info")?;
            Occupancy::reported(
                info.get("model_context_window")?.as_u64()?,
                info.pointer("/last_token_usage/total_tokens")
                    .and_then(Value::as_u64),
            )
        })
}

/// The window Codex's catalogue at `path` publishes for `model`: its
/// `context_window`, cut to its `effective_context_window_percent` when it
/// names one.
fn published(path: &Path, model: &str) -> Option<Occupancy> {
    let (file, length) = regular(path)?;
    if length > LIMIT {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(LIMIT).read_to_end(&mut bytes).ok()?;
    let catalogue: Value = serde_json::from_slice(&bytes).ok()?;
    let entry = catalogue
        .get("models")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("slug").and_then(Value::as_str) == Some(model))?;
    let window = entry.get("context_window")?.as_u64()?;
    let effective = match entry
        .get("effective_context_window_percent")
        .and_then(Value::as_u64)
    {
        Some(percent) => window.saturating_mul(percent) / PERCENT,
        None => window,
    };
    Occupancy::reported(effective, None)
}

/// Every home a test's runs went to read, so a test can tell a run that
/// never read its rollout from one whose rollout told it nothing.
#[cfg(test)]
pub(crate) mod read {
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::PoisonError;

    static HOMES: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    pub(super) fn record(home: &Path) {
        HOMES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(home.to_path_buf());
    }

    /// Whether a run went to read the rollout under `home`.
    pub(crate) fn under(home: &Path) -> bool {
        HOMES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|read| read.starts_with(home))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Duration;

    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;
    use tempfile::TempDir;

    use super::LIMIT;
    use super::Rollout;
    use super::TAIL;
    use super::last;
    use super::published;
    use super::regular;
    use crate::environment::Environment;
    use crate::kind::AgentKind;
    use crate::occupancy::Occupancy;
    use crate::settings::CliSettings;

    const ROLLOUT: &str = include_str!("../tests/fixtures/codex/rollout.jsonl");
    const CATALOGUE: &str = include_str!("../tests/fixtures/codex/models_cache.json");
    const THREAD: &str = "019b2c41-0000-7000-8000-000000000001";
    const MODEL: &str = "gpt-6.1-sol";
    const FILLER: &str =
        r#"{"type":"response_item","payload":{"type":"message","content":"padding"}}"#;

    /// How long a read that must not wait on a pipe is given to return.
    const PROMPTLY: Duration = Duration::from_secs(10);

    fn reported() -> Option<Occupancy> {
        Occupancy::reported(475_000, Some(24_638))
    }

    /// What `work` returns, or a failure when it is still waiting after
    /// [`PROMPTLY`].
    fn promptly<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(work());
        });
        receiver
            .recv_timeout(PROMPTLY)
            .expect("the read waited on a pipe")
    }

    fn rollout(home: &Path) -> Rollout {
        Rollout {
            home: home.to_path_buf(),
            model: Some(MODEL.to_string()),
        }
    }

    fn day(home: &Path, day: &str) -> PathBuf {
        let day = home.join("sessions").join(day);
        fs::create_dir_all(&day).expect("a day folder");
        day
    }

    fn named(thread: &str) -> String {
        format!("rollout-2026-11-05T13-36-38-{thread}.jsonl")
    }

    fn padded(before: &str, after: &str) -> String {
        let mut rollout = before.to_string();
        while (rollout.len() as u64) < TAIL * 3 {
            rollout.push_str(FILLER);
            rollout.push('\n');
        }
        rollout.push_str(after);
        rollout
    }

    fn host(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let variables: BTreeMap<String, OsString> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), OsString::from(value)))
            .collect();
        move |name| variables.get(name).cloned()
    }

    fn located(settings: &CliSettings, pairs: &[(&str, &str)]) -> Option<PathBuf> {
        let host = host(pairs);
        let environment = Environment::new(AgentKind::Codex, settings, None, &host);
        Rollout::locate(
            &environment,
            &host,
            settings.working_directory.as_deref(),
            None,
        )
        .map(|rollout| rollout.home)
    }

    #[test]
    fn the_last_token_count_of_a_real_rollout_names_the_window_and_fill() {
        assert_eq!(last(ROLLOUT), reported());
    }

    #[test]
    fn a_rollout_without_a_reported_window_has_none() {
        let unreported: String = ROLLOUT
            .lines()
            .filter(|line| !line.contains("model_context_window"))
            .map(|line| format!("{line}\n"))
            .collect();
        assert_eq!(last(&unreported), None);
        assert_eq!(last("not json\n{}\n"), None);
        assert_eq!(
            last(
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"model_context_window":"wide"}}}"#
            ),
            None
        );
    }

    #[tokio::test]
    async fn the_rollout_is_found_by_its_thread_in_any_day() {
        let home = TempDir::new().expect("a home");
        let older = day(home.path(), "2026/10/31");
        let newer = day(home.path(), "2026/11/05");
        fs::write(
            newer.join(named("019b2c41-0000-7000-8000-000000000009")),
            "",
        )
        .expect("another thread");
        fs::write(older.join(named(THREAD)), ROLLOUT).expect("this thread");

        assert_eq!(
            rollout(home.path()).occupancy(THREAD.to_string()).await,
            reported()
        );
    }

    #[tokio::test]
    async fn the_end_of_a_rollout_longer_than_the_first_read_is_read() {
        let home = TempDir::new().expect("a home");
        let day = day(home.path(), "2026/11/05");
        fs::write(day.join(named(THREAD)), padded("", ROLLOUT)).expect("a long rollout");

        assert_eq!(
            rollout(home.path()).occupancy(THREAD.to_string()).await,
            reported()
        );
    }

    #[tokio::test]
    async fn a_report_further_back_than_the_first_read_is_still_found() {
        let home = TempDir::new().expect("a home");
        let day = day(home.path(), "2026/11/05");
        fs::write(day.join(named(THREAD)), padded(ROLLOUT, ""))
            .expect("a rollout ending in other events");

        assert_eq!(
            rollout(home.path()).occupancy(THREAD.to_string()).await,
            reported()
        );
    }

    #[tokio::test]
    async fn a_report_further_back_than_the_longest_read_is_never_found() {
        let home = TempDir::new().expect("a home");
        let day = day(home.path(), "2026/11/05");
        let mut rollout_text = ROLLOUT.to_string();
        while (rollout_text.len() as u64) <= LIMIT + TAIL {
            rollout_text.push_str(FILLER);
            rollout_text.push('\n');
        }
        fs::write(day.join(named(THREAD)), rollout_text).expect("a rollout ending in other events");
        let unpublished = Rollout {
            home: home.path().to_path_buf(),
            model: None,
        };

        assert_eq!(unpublished.occupancy(THREAD.to_string()).await, None);
    }

    #[tokio::test]
    async fn a_link_in_a_rollouts_place_is_never_read() {
        let home = TempDir::new().expect("a home");
        let day = day(home.path(), "2026/11/05");
        let elsewhere = home.path().join("elsewhere.jsonl");
        fs::write(&elsewhere, ROLLOUT).expect("a file outside the sessions");
        std::os::unix::fs::symlink(&elsewhere, day.join(named(THREAD)))
            .expect("a link named as the rollout");
        let unpublished = Rollout {
            home: home.path().to_path_buf(),
            model: None,
        };

        assert_eq!(unpublished.occupancy(THREAD.to_string()).await, None);
    }

    #[tokio::test]
    async fn a_link_in_a_day_folders_place_is_never_followed() {
        let home = TempDir::new().expect("a home");
        let elsewhere = home.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("a folder outside the sessions");
        fs::write(elsewhere.join(named(THREAD)), ROLLOUT).expect("a rollout outside the sessions");
        let month = home.path().join("sessions/2026/11");
        fs::create_dir_all(&month).expect("a month folder");
        std::os::unix::fs::symlink(&elsewhere, month.join("05")).expect("a link named as a day");
        let unpublished = Rollout {
            home: home.path().to_path_buf(),
            model: None,
        };

        assert_eq!(unpublished.occupancy(THREAD.to_string()).await, None);
    }

    #[test]
    fn a_pipe_swapped_into_a_files_place_is_never_waited_on() {
        let home = TempDir::new().expect("a home");
        let pipe = home.path().join("models_cache.json");
        mkfifo(&pipe, Mode::S_IRUSR | Mode::S_IWUSR).expect("a pipe named as the catalogue");

        let (opened, catalogue) = promptly(move || {
            let opened = regular(&pipe).is_some();
            (opened, published(&pipe, MODEL))
        });

        assert!(!opened, "a pipe was read as a file");
        assert_eq!(catalogue, None);
    }

    #[tokio::test]
    async fn a_linked_sessions_folder_is_never_followed() {
        let home = TempDir::new().expect("a home");
        let elsewhere = TempDir::new().expect("a folder outside the home");
        let day = elsewhere.path().join("2026/11/05");
        fs::create_dir_all(&day).expect("a day folder outside the home");
        fs::write(day.join(named(THREAD)), ROLLOUT).expect("a rollout outside the home");
        std::os::unix::fs::symlink(elsewhere.path(), home.path().join("sessions"))
            .expect("a link named as the sessions folder");
        let unpublished = Rollout {
            home: home.path().to_path_buf(),
            model: None,
        };

        assert_eq!(unpublished.occupancy(THREAD.to_string()).await, None);
    }

    #[tokio::test]
    async fn without_a_report_the_published_window_of_the_model_is_taken_unfilled() {
        let home = TempDir::new().expect("a home");
        let day = day(home.path(), "2026/11/05");
        let unreported: String = ROLLOUT
            .lines()
            .filter(|line| !line.contains("token_count"))
            .map(|line| format!("{line}\n"))
            .collect();
        fs::write(day.join(named(THREAD)), unreported).expect("a rollout without a report");
        fs::write(home.path().join("models_cache.json"), CATALOGUE).expect("the catalogue");

        assert_eq!(
            rollout(home.path()).occupancy(THREAD.to_string()).await,
            Occupancy::reported(500_000 * 95 / 100, None)
        );
    }

    #[test]
    fn a_published_window_without_its_effective_share_is_taken_whole() {
        let home = TempDir::new().expect("a home");
        let catalogue = home.path().join("models_cache.json");
        fs::write(&catalogue, CATALOGUE).expect("the catalogue");

        assert_eq!(
            published(&catalogue, "gpt-6.1-mini"),
            Occupancy::reported(272_000, None)
        );
        assert_eq!(published(&catalogue, "gpt-unknown"), None);
    }

    #[test]
    fn a_catalogue_that_is_a_link_or_of_another_shape_publishes_nothing() {
        let home = TempDir::new().expect("a home");
        let elsewhere = home.path().join("elsewhere.json");
        fs::write(&elsewhere, CATALOGUE).expect("a catalogue outside the home");
        let catalogue = home.path().join("models_cache.json");
        std::os::unix::fs::symlink(&elsewhere, &catalogue).expect("a link named as the catalogue");
        assert_eq!(published(&catalogue, MODEL), None);

        fs::remove_file(&catalogue).expect("the link removed");
        for shape in [
            r#"{"models":{}}"#,
            r#"[]"#,
            "not json",
            r#"{"models":[{"slug":"gpt-6.1-sol"}]}"#,
        ] {
            fs::write(&catalogue, shape).expect("a catalogue");
            assert_eq!(published(&catalogue, MODEL), None, "{shape}");
        }
    }

    #[tokio::test]
    async fn with_neither_a_report_nor_a_published_window_there_is_none() {
        let home = TempDir::new().expect("a home");

        assert_eq!(
            rollout(home.path()).occupancy(THREAD.to_string()).await,
            None
        );
        assert!(super::read::under(home.path()));
    }

    #[test]
    fn codex_home_beats_the_user_home() {
        let settings = CliSettings::default();
        assert_eq!(
            located(
                &settings,
                &[("HOME", "/home/agent"), ("CODEX_HOME", "/logins/7/codex")]
            ),
            Some(PathBuf::from("/logins/7/codex"))
        );
        assert_eq!(
            located(
                &settings
                    .clone()
                    .with_variable("CODEX_HOME", "/logins/8/codex"),
                &[("HOME", "/home/agent"), ("CODEX_HOME", "/logins/7/codex")]
            ),
            Some(PathBuf::from("/logins/8/codex"))
        );
        assert_eq!(
            located(&settings, &[("HOME", "/home/agent")]),
            Some(PathBuf::from("/home/agent/.codex"))
        );
        assert_eq!(
            located(&settings, &[("HOME", "/home/agent"), ("CODEX_HOME", "")]),
            Some(PathBuf::from("/home/agent/.codex"))
        );
        assert_eq!(located(&settings, &[]), None);
    }

    #[test]
    fn a_home_the_child_is_not_given_is_not_its_home() {
        let withheld = CliSettings::default().withhold_host_credentials();
        assert_eq!(
            located(
                &withheld,
                &[("HOME", "/home/agent"), ("CODEX_HOME", "/logins/7/codex")]
            ),
            Some(PathBuf::from("/home/agent/.codex"))
        );
    }

    #[test]
    fn a_relative_home_is_the_childs_working_directory_s() {
        let settings = CliSettings::default().with_working_directory("/w");
        assert_eq!(
            located(&settings, &[("CODEX_HOME", "logins/codex")]),
            Some(PathBuf::from("/w/logins/codex"))
        );
    }

    #[test]
    fn an_inherited_environment_lends_its_home_unless_it_is_removed() {
        let inherited = CliSettings::default().inherit_environment();
        assert_eq!(
            located(
                &inherited,
                &[("HOME", "/home/agent"), ("CODEX_HOME", "/logins/7/codex")]
            ),
            Some(PathBuf::from("/logins/7/codex"))
        );
        assert_eq!(
            located(
                &inherited.withhold_host_credentials(),
                &[("HOME", "/home/agent"), ("CODEX_HOME", "/logins/7/codex")]
            ),
            Some(PathBuf::from("/home/agent/.codex"))
        );
    }
}
