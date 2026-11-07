use crate::config::Config;
use crate::host::CLAIM_FILE;
use crate::host::Compute;
use crate::host::DATA_DIRECTORY;
use crate::host::DATASET_DIRECTORY;
use crate::host::DISMISSED_FILE;
use crate::host::JOB_FILE;
use crate::host::Job;
use crate::host::LOCK_FILE;
use crate::host::PREVIEWS_DIRECTORY;
use crate::host::PROGRESS_FILE;
use crate::host::PUBLICATION_FILE;
use crate::host::SCHEMA_VERSION;
use crate::host::root;
use crate::inventory::WEIGHT_EXTENSION;
use crate::lora::TrainError;
use crate::train::Phase;
use crate::train::TrainProgress;
use abnegate_secret::SecretValue;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::fs::TryLockError;
use std::io::ErrorKind;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use uuid::Uuid;

const TEMPORARY_SUFFIX: &str = ".tmp";
/// The suffix of a job directory moved aside to be removed.
pub(crate) const DISCARDED_SUFFIX: &str = ".discarded";
const RESERVED: &[&str] = &[
    JOB_FILE,
    PROGRESS_FILE,
    LOCK_FILE,
    CLAIM_FILE,
    DISMISSED_FILE,
    PUBLICATION_FILE,
    PREVIEWS_DIRECTORY,
    DATASET_DIRECTORY,
    DATA_DIRECTORY,
    "pid",
];
const INPUT_DIRECTORIES: &[&str] = &[DATASET_DIRECTORY, DATA_DIRECTORY];
#[cfg(unix)]
const PRIVATE_DIRECTORY: u32 = 0o700;
const PRIVATE_FILE: u32 = 0o600;
const SHARED_FILE: u32 = 0o644;

/// The directory one host [`Job`] lives in, `<models>/<host directory>/<id>`,
/// and the files the crate and the host trainer share inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDirectory {
    path: PathBuf,
}

impl JobDirectory {
    /// The directory of the job `id` under `config`'s
    /// [`Contract::host_directory`](crate::train::Contract::host_directory).
    pub fn new(config: &Config, id: Uuid) -> Self {
        Self::at(root(config).join(id.to_string()))
    }

    pub(crate) fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// Where the directory is.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads `job.json`. Refuses one written for a newer
    /// [`SCHEMA_VERSION`], whose keys this version could misread.
    pub fn read_job(&self) -> Result<Job, TrainError> {
        let bytes = fs::read(self.path.join(JOB_FILE)).map_err(failed)?;
        let job: Job = serde_json::from_slice(&bytes)
            .map_err(|error| TrainError::Failed(error.to_string()))?;
        if job.schema_version > SCHEMA_VERSION {
            return Err(TrainError::Invalid(
                "job.json was written for a newer version of the host-job contract",
            ));
        }
        Ok(job)
    }

    /// Replaces `job.json` with `job` in one rename of a file written in
    /// full and flushed to disk, so the trainer never reads half of it and a
    /// crash never loses it.
    pub fn write_job(&self, job: &Job) -> Result<(), TrainError> {
        let encoded = serde_json::to_vec_pretty(job)
            .map_err(|error| TrainError::Failed(error.to_string()))?;
        self.replace(JOB_FILE, &encoded, SHARED_FILE)
    }

    /// Reads `progress.json`. `None` when there is none yet, or when it is
    /// caught halfway through being written.
    pub fn read_progress(&self) -> Option<TrainProgress> {
        TrainProgress::parse(&fs::read(self.path.join(PROGRESS_FILE)).ok()?)
    }

    /// Replaces `progress.json` with `progress` in one rename.
    pub fn write_progress(&self, progress: &TrainProgress) -> Result<(), TrainError> {
        let encoded =
            serde_json::to_vec(progress).map_err(|error| TrainError::Failed(error.to_string()))?;
        self.replace(PROGRESS_FILE, &encoded, SHARED_FILE)
    }

    /// Writes the progress of a job nobody has started: step 0 of `total`,
    /// [`Phase::Queued`], 0%. Returns what it wrote.
    pub fn write_queued_progress(&self, total: u32) -> Result<TrainProgress, TrainError> {
        let progress = TrainProgress::new(0, total.max(1))
            .with_phase(Phase::Queued)
            .with_message("Waiting for the host trainer")
            .with_percent(0);
        self.write_progress(&progress)?;
        Ok(progress)
    }

    /// Copies every regular file directly inside `source` into the job's
    /// subdirectory `directory`, such as [`DATA_DIRECTORY`] for a dataset
    /// the host built itself.
    pub fn stage(&self, source: &Path, directory: &str) -> Result<(), TrainError> {
        if !is_plain_name(directory) {
            return Err(TrainError::Invalid(
                "a job directory is one plain directory name",
            ));
        }
        self.create()?;
        let destination = self.path.join(directory);
        JobDirectory::at(destination.clone()).create()?;
        for entry in fs::read_dir(source).map_err(failed)? {
            let entry = entry.map_err(failed)?;
            if entry.file_type().map_err(failed)?.is_file() {
                fs::copy(entry.path(), destination.join(entry.file_name())).map_err(failed)?;
            }
        }
        Ok(())
    }

    /// Writes `value` as the one line of the file `name` beside `job.json`,
    /// readable only by its owner, for a credential the trainer needs and
    /// `job.json` must never hold. The name is one plain file name the job
    /// does not use for anything else, and a link in its place is refused
    /// rather than followed, as is a value that holds a line break or any
    /// other control character once trimmed. List it with
    /// [`Job::with_secret`] so it is removed once the job ends.
    pub fn write_secret(&self, name: &str, value: &SecretValue) -> Result<(), TrainError> {
        if !is_secret_name(name) {
            return Err(TrainError::Invalid(
                "a job secret needs a plain file name the job does not use",
            ));
        }
        let value = value.expose().trim();
        if value.chars().any(is_line_break_or_control) {
            return Err(TrainError::Invalid(
                "a job secret is one line of printable text",
            ));
        }
        self.create()?;
        match fs::symlink_metadata(self.path.join(name)) {
            Ok(metadata) if !metadata.is_file() => {
                return Err(TrainError::Invalid(
                    "a job secret can only replace a regular file",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(failed(error)),
        }
        let line = format!("{value}\n");
        self.replace(name, line.as_bytes(), PRIVATE_FILE)
    }

    /// Writes `key` as the one line of `compute`'s
    /// [key file](Compute::key_file), the way [`JobDirectory::write_secret`]
    /// writes any secret: readable only by its owner, and never into
    /// `job.json`. Refuses a compute that reads no key, and a blank key.
    /// [`Job::with_compute`] lists the file so it is removed once the job
    /// ends.
    pub fn write_compute_key(&self, compute: Compute, key: &SecretValue) -> Result<(), TrainError> {
        let file = compute
            .key_file()
            .ok_or(TrainError::Invalid("this compute reads no key"))?;
        if key.expose().trim().is_empty() {
            return Err(TrainError::Invalid("a compute key cannot be blank"));
        }
        self.write_secret(file, key)
    }

    /// The preview `name` the trainer rendered under `previews/`, when it is
    /// a plain file name and the file is there.
    pub fn preview(&self, name: &str) -> Option<PathBuf> {
        if !is_plain_name(name) {
            return None;
        }
        let path = self.path.join(PREVIEWS_DIRECTORY).join(name);
        fs::symlink_metadata(&path)
            .is_ok_and(|metadata| metadata.is_file())
            .then_some(path)
    }

    /// Whether a host has [dismissed](crate::host::dismiss) the job.
    pub(crate) fn dismissed(&self) -> bool {
        fs::symlink_metadata(self.path.join(DISMISSED_FILE))
            .is_ok_and(|metadata| metadata.is_file())
    }

    /// Takes the job's [claim](CLAIM_FILE) by creating it exclusively.
    /// `false` when the trainer or another waiter holds it already.
    pub(crate) fn claim(&self) -> Result<bool, TrainError> {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path.join(CLAIM_FILE))
        {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(failed(error)),
        }
    }

    /// Notes `generation`, the training attempt whose sidecar the job's
    /// weight is about to be published with, before it is promoted.
    pub(crate) fn write_publication(&self, generation: &str) -> Result<(), TrainError> {
        self.replace(PUBLICATION_FILE, generation.as_bytes(), SHARED_FILE)
    }

    /// The generation [`JobDirectory::write_publication`] noted last.
    pub(crate) fn publication(&self) -> Option<String> {
        let generation = fs::read_to_string(self.path.join(PUBLICATION_FILE)).ok()?;
        Some(generation.trim().to_string()).filter(|generation| !generation.is_empty())
    }

    /// Lets the job's [claim](CLAIM_FILE) go, for a claimer that could not
    /// act on it.
    pub(crate) fn release_claim(&self) {
        match fs::remove_file(self.path.join(CLAIM_FILE)) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                directory = %self.path.display(),
                %error,
                "the job's claim could not be released, so no trainer can claim it"
            ),
        }
    }

    /// Marks the job dismissed, once.
    pub(crate) fn dismiss(&self) -> Result<(), TrainError> {
        if self.dismissed() {
            return Ok(());
        }
        self.replace(DISMISSED_FILE, b"", SHARED_FILE)
    }

    /// Creates the directory readable only by its owner, or checks that the
    /// one already there is a directory rather than a link to one.
    pub(crate) fn create(&self) -> Result<(), TrainError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(failed)?;
        }
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(PRIVATE_DIRECTORY);
        }
        match builder.create(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&self.path).map_err(failed)?;
                if metadata.is_dir() {
                    Ok(())
                } else {
                    Err(TrainError::Invalid(
                        "a job directory is not a plain directory",
                    ))
                }
            }
            Err(error) => Err(failed(error)),
        }
    }

    /// Creates the directory for a job nobody has queued yet and takes its
    /// publication lock, waiting for whoever holds it. A sweep that
    /// [discards](JobDirectory::discard) the directory while holding the lock
    /// moves it away first, so when nothing is left at the path once the lock
    /// is taken, the directory is made again and locked afresh.
    pub(crate) fn create_locked(&self) -> Result<File, TrainError> {
        loop {
            self.create()?;
            let Some(lock) = self.open_lock()? else {
                continue;
            };
            lock.lock().map_err(failed)?;
            if fs::symlink_metadata(self.path.join(LOCK_FILE)).is_ok() {
                return Ok(lock);
            }
        }
    }

    /// Takes the job's publication lock, which whoever publishes the job
    /// holds from queueing it until its weight is published. `None` when
    /// another holds it.
    pub(crate) fn try_lock(&self) -> Result<Option<File>, TrainError> {
        let file = self
            .open_lock()?
            .ok_or_else(|| failed(std::io::Error::from(ErrorKind::NotFound)))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(error)) => Err(failed(error)),
        }
    }

    /// Opens the job's lock file, creating it when it is missing. `None` when
    /// the directory is no longer there, which macOS can report as `EINVAL`
    /// when a [discard](JobDirectory::discard) moves it during the open.
    fn open_lock(&self) -> Result<Option<File>, TrainError> {
        let path = self.path.join(LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(PRIVATE_FILE);
        }
        let opened = match options.open(&path) {
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                match fs::symlink_metadata(&path) {
                    Ok(metadata) if !metadata.is_file() => {
                        return Err(TrainError::Invalid("a job lock is not a regular file"));
                    }
                    Ok(_) => OpenOptions::new().read(true).write(true).open(&path),
                    Err(error) => Err(error),
                }
            }
            opened => opened,
        };
        match opened {
            Ok(file) => Ok(Some(file)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error)
                if error.kind() == ErrorKind::InvalidInput
                    && fs::symlink_metadata(&self.path).is_err() =>
            {
                Ok(None)
            }
            Err(error) => Err(failed(error)),
        }
    }

    /// Removes the secrets `job` lists, and every compute's key file whether
    /// or not the list names it.
    pub(crate) fn remove_job_secrets(&self, job: &Job) {
        self.remove_secrets(&job.secrets);
        for file in crate::host::KEY_FILES {
            let _ = fs::remove_file(self.path.join(file));
        }
    }

    /// Removes the secrets `names` lists, skipping any name that could not
    /// be one.
    pub(crate) fn remove_secrets(&self, names: &[String]) {
        for name in names {
            if is_secret_name(name) {
                let _ = fs::remove_file(self.path.join(name));
            }
        }
    }

    /// Removes what the job trained on, keeping `job.json`, `progress.json`
    /// and `previews/` for a host to show.
    pub(crate) fn prune(&self) {
        for name in INPUT_DIRECTORIES {
            remove_entry(&self.path.join(name));
        }
    }

    /// Removes the whole directory, first moving it aside under a name that
    /// is no job's, so whoever reaches the job's path afterwards finds nothing
    /// there rather than part of what is being removed.
    pub(crate) fn discard(&self) -> Result<(), TrainError> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.is_dir() => {
                let discarded = self.path.with_file_name(format!(
                    ".{}.{}{DISCARDED_SUFFIX}",
                    self.path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default(),
                    Uuid::new_v4()
                ));
                fs::rename(&self.path, &discarded).map_err(failed)?;
                fs::remove_dir_all(&discarded).map_err(failed)
            }
            Ok(_) => Err(TrainError::Invalid(
                "a job directory is not a plain directory",
            )),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(failed(error)),
        }
    }

    fn replace(&self, name: &str, bytes: &[u8], mode: u32) -> Result<(), TrainError> {
        self.create()?;
        let temporary = self
            .path
            .join(format!("{name}.{}{TEMPORARY_SUFFIX}", Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode);
        }
        #[cfg(not(unix))]
        let _ = mode;
        let written = options
            .open(&temporary)
            .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
            .and_then(|()| fs::rename(&temporary, self.path.join(name)));
        if let Err(error) = written {
            let _ = fs::remove_file(&temporary);
            return Err(failed(error));
        }
        let _ = File::open(&self.path).and_then(|directory| directory.sync_all());
        Ok(())
    }
}

/// Whether `character` breaks a line or is any other control character,
/// counting the Unicode line and paragraph separators.
fn is_line_break_or_control(character: char) -> bool {
    character.is_control() || matches!(character, '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// Whether `name` can be a secret file beside `job.json`. A name that
/// differs from a reserved one only in case is refused too, since it is the
/// same file on a case-insensitive filesystem.
pub(crate) fn is_secret_name(name: &str) -> bool {
    let lowercase = name.to_ascii_lowercase();
    is_plain_name(name)
        && !RESERVED
            .iter()
            .any(|reserved| reserved.eq_ignore_ascii_case(name))
        && !lowercase.ends_with(TEMPORARY_SUFFIX)
        && !lowercase.ends_with(WEIGHT_EXTENSION)
}

fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

fn remove_entry(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

fn failed(error: std::io::Error) -> TrainError {
    TrainError::Failed(error.to_string())
}
