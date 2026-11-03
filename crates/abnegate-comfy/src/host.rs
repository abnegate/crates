//! The host-job contract: training jobs handed to a trainer that runs beside
//! ComfyUI on the host rather than inside it.
//!
//! Every job is a directory under the models root, named by its id inside
//! [`Contract::host_directory`](crate::train::Contract::host_directory), so a
//! week-long fine-tune outlives the process that queued it. The directory
//! holds:
//!
//! - [`JOB_FILE`]: the [`Job`], whose `status` the trainer moves from
//!   `queued` through `running` to `succeeded` or `failed`.
//! - [`CLAIM_FILE`]: the claim on a queued job, created with exclusive
//!   create (`O_CREAT | O_EXCL`) by whoever takes the job out of `queued`.
//!   The trainer creates it before it writes `running`, and a waiter that
//!   gives up on the job creates it before it writes `failed`; only the one
//!   that created it may move the job on, so a trainer that loses the claim
//!   leaves the job alone. A trainer that writes `running` without the claim
//!   can still race a waiter giving up.
//! - [`PROGRESS_FILE`]: one [`TrainProgress`] the trainer rewrites as it
//!   goes.
//! - [`PREVIEWS_DIRECTORY`]: the samples `progress.json` names in
//!   `previews`.
//! - [`DATASET_DIRECTORY`]: the images, `NNNN.png` captioned by `NNNN.txt`,
//!   with the companions a person set carries (`NNNN.kind`, `NNNN.pose`,
//!   `NNNN.mask.png`) and [`REBALANCE_FILE`].
//! - [`DATA_DIRECTORY`]: text data, [`TRAIN_DATA_FILE`] and
//!   [`VALID_DATA_FILE`] in the format [`FORMAT_FILE`] describes.
//! - The weight, written by the trainer as [`Job::filename`], which
//!   [`lora::train`](crate::lora::train) then publishes like any other. A
//!   `pivotal` job's trainer also writes the textual-inversion embedding it
//!   trains to the models directory's `embeddings/` under that same name,
//!   and the published weight's sidecar names it as its `embedding`.
//! - Any secret the host wrote with [`JobDirectory::write_secret`], removed
//!   once the job ends, such as the key a remote [`Compute`] reads, written
//!   with [`JobDirectory::write_compute_key`].
//! - [`DISMISSED_FILE`], once a host [dismissed](dismiss) the finished job.
//! - [`LOCK_FILE`], held by whoever publishes the job, from queueing it until
//!   its weight is published.
//!
//! The directory is readable only by its owner. Once a job has failed, or its
//! weight is published, `dataset/` and `data/` are removed; `job.json`,
//! `progress.json` and `previews/` stay for a host to show until
//! [`remove`] takes the job away.

mod compute;
mod job;
mod job_directory;
mod pending;
mod status;
mod trainer;

pub use compute::Compute;
pub use job::Job;
pub use job_directory::JobDirectory;
pub use status::Status;
pub use trainer::Trainer;

pub(crate) use compute::KEY_FILES;
pub(crate) use job::is_reserved;
pub(crate) use job_directory::is_secret_name;
pub(crate) use pending::Pending;

use crate::config::Config;
use crate::lora::TrainError;
use crate::train::Reporter;
use crate::train::TrainProgress;
use std::fs;
use std::fs::File;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::time::Instant;
use tokio::sync::watch;
use uuid::Uuid;

/// The `job.json` layout this version writes and reads.
pub const SCHEMA_VERSION: u32 = 1;
/// The job's state, a [`Job`].
pub const JOB_FILE: &str = "job.json";
/// The trainer's progress, a [`TrainProgress`].
pub const PROGRESS_FILE: &str = "progress.json";
/// The lock whoever publishes the job holds while it waits and publishes.
pub const LOCK_FILE: &str = "publish.lock";
/// The empty claim on a queued job: whoever creates it, the trainer or a
/// waiter giving up, alone moves the job out of `queued`.
pub const CLAIM_FILE: &str = "claimed";
/// The empty marker of a finished job a host no longer shows.
pub const DISMISSED_FILE: &str = "dismissed";
/// Samples the trainer renders while it trains.
pub const PREVIEWS_DIRECTORY: &str = "previews";
/// The image set and its captions and companions.
pub const DATASET_DIRECTORY: &str = "dataset";
/// How a person set's poses are reweighted, inside [`DATASET_DIRECTORY`].
pub const REBALANCE_FILE: &str = "rebalance.json";
/// Text data in JSON lines.
pub const DATA_DIRECTORY: &str = "data";
/// The training records, inside [`DATA_DIRECTORY`].
pub const TRAIN_DATA_FILE: &str = "train.jsonl";
/// The validation records, inside [`DATA_DIRECTORY`].
pub const VALID_DATA_FILE: &str = "valid.jsonl";
/// How the records are laid out, inside [`DATA_DIRECTORY`].
pub const FORMAT_FILE: &str = "format.json";

const NOT_PICKED_UP: &str = "the host trainer did not pick up the job";
const ANOTHER_JOB: &str = "job.json names another job";
const UNPUBLISHED: &str = "a job with no base is the host's to publish";
const PUBLISHED_OUTSIDE: &str = "a job's published weight is not a path under the models directory";
const STILL_BUSY: &str = "a job the host trainer may still hold is not removed";
const UNREADABLE: &str =
    "a job whose job.json cannot be read, which the host trainer may still hold, is not removed";
const BEING_PUBLISHED: &str = "a job that is being waited on or published is not removed";
const STILL_RUNNING: &str = "a training job is already running";

/// The directory every job lives in under `config`.
pub fn root(config: &Config) -> PathBuf {
    config
        .models_directory
        .join(&config.contract.host_directory)
}

/// Every job under `config` whose `job.json` reads, oldest first. An entry
/// that is not a directory of its own, such as a link to one, is no job.
pub fn jobs(config: &Config) -> Vec<Job> {
    jobs_in(&root(config))
}

/// The job started last, which is the one a host shows.
pub fn current(config: &Config) -> Option<Job> {
    jobs(config).pop()
}

/// [`current`], with its `progress.json` in [`Job::progress`], so a host that
/// restarted mid-run still shows how far it has got. `None` once the job is
/// [dismissed](dismiss), until another job starts.
pub fn current_with_progress(config: &Config) -> Option<Job> {
    let mut job = current(config)?;
    let directory = JobDirectory::new(config, job.id);
    if directory.dismissed() {
        return None;
    }
    job.progress = directory.read_progress();
    Some(job)
}

/// Hides the finished [`current`] job from [`current_with_progress`]
/// with [`DISMISSED_FILE`] and removes the secrets it lists, leaving the rest
/// of its directory as it is. Refuses a job still waiting for, or holding,
/// the trainer. With no job, or one already dismissed, there is nothing to
/// do.
pub fn dismiss(config: &Config) -> Result<(), TrainError> {
    let Some(job) = current(config) else {
        return Ok(());
    };
    if job.busy() {
        return Err(TrainError::Invalid(STILL_RUNNING));
    }
    let directory = JobDirectory::new(config, job.id);
    directory.remove_job_secrets(&job);
    directory.dismiss()
}

/// Removes the secrets of every job under `config` that has ended, such as
/// one that finished after the run waiting on it was dropped.
pub(crate) fn remove_ended_secrets(config: &Config) {
    for job in jobs(config).into_iter().filter(|job| !job.busy()) {
        JobDirectory::new(config, job.id).remove_job_secrets(&job);
    }
}

/// Whether the current job is still waiting for, or holding, the trainer.
pub fn busy(config: &Config) -> bool {
    current(config).is_some_and(|job| job.busy())
}

/// The preview `name` of the current job, as its `progress.json` lists it
/// under `previews/`.
pub fn preview(config: &Config, name: &str) -> Option<PathBuf> {
    let job = current(config)?;
    JobDirectory::new(config, job.id).preview(name)
}

/// Waits for the job in `directory` to finish and returns it once it
/// succeeded.
///
/// Fails with what the trainer wrote in `error` when the job fails, and when
/// no trainer started it within [`Trainer::pickup`] of the last moment
/// another job was running, in which case the job is marked failed so no
/// trainer starts it later. Fails with [`TrainError::Disabled`] when
/// [`Config::host_trainer`] is not set. A job that is running has no deadline
/// here; dropping the future leaves it to the trainer, and [`resume`] or
/// [`recover`] publishes it later. Once the job has ended, the secrets it
/// lists are removed, and so is what a failed job trained on.
pub async fn wait(config: &Config, directory: &JobDirectory) -> Result<Job, TrainError> {
    let trainer = config.host_trainer.as_ref().ok_or(TrainError::Disabled)?;
    wait_with(trainer, directory, &Reporter::default()).await
}

/// [`wait`], keeping the latest `progress.json` in `progress` the way
/// [`lora::train_reporting`](crate::lora::train_reporting) does.
pub async fn wait_reporting(
    config: &Config,
    directory: &JobDirectory,
    progress: watch::Sender<Option<TrainProgress>>,
) -> Result<Job, TrainError> {
    let trainer = config.host_trainer.as_ref().ok_or(TrainError::Disabled)?;
    wait_with(trainer, directory, &Reporter::new(progress)).await
}

/// Waits for the job `id`, queued by an earlier process, and publishes its
/// weight as [`lora::train`](crate::lora::train) would have. Returns where the
/// weight was published, at once for a job already published.
///
/// A job that names another id, has no [base](Job::base), or whose base,
/// method, subject or weight name the policy refuses fails before any wait.
/// While another waiter holds the job, this one waits for it to let go, then
/// returns what it published. Once its `job.json` reads, names `id` and
/// shows the job ended, the job loses its secrets, even when this then
/// fails.
pub async fn resume(config: &Config, id: Uuid) -> Result<PathBuf, TrainError> {
    resume_with(config, id, &Reporter::default()).await
}

/// [`resume`], keeping the latest progress in `progress` until the weight is
/// published, the way [`lora::train_reporting`](crate::lora::train_reporting)
/// does.
pub async fn resume_reporting(
    config: &Config,
    id: Uuid,
    progress: watch::Sender<Option<TrainProgress>>,
) -> Result<PathBuf, TrainError> {
    resume_with(config, id, &Reporter::new(progress)).await
}

/// Publishes every job that succeeded while nobody waited for it: one with a
/// [base](Job::base) whose weight is still in its directory and that is not
/// [published](Job::published) yet. A job with no base, which a host queued
/// for a trainer of its own, is left to the host, and a job another waiter
/// holds is left to that waiter. Every job that has ended loses its secrets,
/// and a failed one what it trained on. A job directory with no `job.json`
/// that nobody holds, which a crash left between writing its secrets and
/// queueing it, is removed. Returns where each weight went, or why it could
/// not be published.
///
/// A host calls it once at startup, after a restart that may have dropped
/// [`lora::train`](crate::lora::train) mid-run.
pub fn recover(config: &Config) -> Vec<Result<PathBuf, TrainError>> {
    discard_unqueued(&root(config));
    jobs(config)
        .into_iter()
        .filter(|job| !job.busy())
        .filter_map(|job| recover_job(config, job.id))
        .collect()
}

/// Removes the job `id` and everything in its directory. Refuses a job the
/// trainer may still hold, a job whose `job.json` cannot be read, and one
/// another waiter holds. A job that is not there is already removed.
pub fn remove(config: &Config, id: Uuid) -> Result<(), TrainError> {
    let directory = JobDirectory::new(config, id);
    match fs::symlink_metadata(directory.path()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(TrainError::Failed(error.to_string())),
        Ok(metadata) if !metadata.is_dir() => return directory.discard(),
        Ok(_) => {}
    }
    let unrecorded = matches!(
        fs::symlink_metadata(directory.path().join(JOB_FILE)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    );
    if !unrecorded {
        match directory.read_job() {
            Ok(job) if job.busy() => return Err(TrainError::Invalid(STILL_BUSY)),
            Ok(_) => {}
            Err(_) => return Err(TrainError::Invalid(UNREADABLE)),
        }
    }
    let _lock = directory
        .try_lock()?
        .ok_or(TrainError::Invalid(BEING_PUBLISHED))?;
    directory.discard()
}

/// Where `job`'s weight was published under `config`, once it was.
pub(crate) fn published(config: &Config, job: &Job) -> Result<Option<PathBuf>, TrainError> {
    let Some(published) = job.published.as_deref() else {
        return Ok(None);
    };
    let relative = Path::new(published);
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(TrainError::Invalid(PUBLISHED_OUTSIDE));
    }
    Ok(Some(config.models_directory.join(relative)))
}

/// Takes `directory`'s publication lock, waiting `interval` between tries
/// while another holds it.
pub(crate) async fn lock(
    directory: &JobDirectory,
    interval: std::time::Duration,
) -> Result<File, TrainError> {
    loop {
        if let Some(lock) = directory.try_lock()? {
            return Ok(lock);
        }
        tokio::time::sleep(interval).await;
    }
}

async fn resume_with(
    config: &Config,
    id: Uuid,
    progress: &Reporter,
) -> Result<PathBuf, TrainError> {
    let trainer = config.host_trainer.as_ref().ok_or(TrainError::Disabled)?;
    let directory = JobDirectory::new(config, id);
    let job = directory.read_job()?;
    if job.id != id {
        return Err(TrainError::Invalid(ANOTHER_JOB));
    }
    if !job.busy() {
        directory.remove_job_secrets(&job);
    }
    if let Some(path) = published(config, &job)? {
        return Ok(path);
    }
    if job.base.is_empty() {
        return Err(TrainError::Invalid(UNPUBLISHED));
    }
    crate::lora::validate_host_job(config, &job)?;
    let _lock = lock(&directory, trainer.poll_interval).await?;
    let job = wait_with(trainer, &directory, progress).await?;
    crate::lora::publish_host_job(config, &directory, job, progress)
}

fn recover_job(config: &Config, id: Uuid) -> Option<Result<PathBuf, TrainError>> {
    let directory = JobDirectory::new(config, id);
    let _lock = match directory.try_lock() {
        Ok(Some(lock)) => lock,
        Ok(None) => return None,
        Err(error) => return Some(Err(error)),
    };
    let job = directory
        .read_job()
        .ok()
        .filter(|job| job.id == id && !job.busy())?;
    directory.remove_job_secrets(&job);
    if job.status == Status::Failed {
        directory.prune();
        return None;
    }
    if job.published.is_some() {
        directory.prune();
        return None;
    }
    if job.base.is_empty() {
        return None;
    }
    let weight = directory.path().join(job.filename.as_deref()?);
    if !fs::symlink_metadata(weight).is_ok_and(|metadata| metadata.is_file()) {
        return None;
    }
    Some(crate::lora::publish_host_job(
        config,
        &directory,
        job,
        &Reporter::default(),
    ))
}

/// Removes every job directory under `root` that holds no `job.json` and
/// whose lock nobody holds.
fn discard_unqueued(root: &Path) {
    for entry in fs::read_dir(root).into_iter().flatten().flatten() {
        let is_job = entry.file_type().is_ok_and(|kind| kind.is_dir())
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| Uuid::parse_str(name).is_ok());
        if !is_job {
            continue;
        }
        let directory = JobDirectory::at(entry.path());
        if fs::symlink_metadata(directory.path().join(JOB_FILE)).is_ok() {
            continue;
        }
        if let Ok(Some(_lock)) = directory.try_lock() {
            let _ = directory.discard();
        }
    }
}

fn jobs_in(root: &Path) -> Vec<Job> {
    let mut jobs: Vec<Job> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let id = Uuid::parse_str(entry.file_name().to_str()?).ok()?;
            JobDirectory::at(entry.path())
                .read_job()
                .ok()
                .filter(|job| job.id == id)
        })
        .collect();
    jobs.sort_by_key(|job| job.started_at);
    jobs
}

/// Whether a job other than `id` beside `directory` is running, which keeps
/// the trainer from picking `id` up however long it waits.
fn another_running(directory: &JobDirectory, id: Uuid) -> bool {
    directory.path().parent().is_some_and(|root| {
        jobs_in(root)
            .iter()
            .any(|job| job.id != id && job.status == Status::Running)
    })
}

/// Ends `job` on this side: its secrets go, and so does what it trained on
/// once it failed.
fn ended(directory: &JobDirectory, job: &Job) {
    directory.remove_job_secrets(job);
    if job.status == Status::Failed {
        directory.prune();
    }
}

fn failure(job: Job) -> TrainError {
    let reason = job
        .error
        .filter(|error| !error.trim().is_empty())
        .unwrap_or_else(|| "no reason given".to_string());
    TrainError::Failed(format!("host trainer failed: {reason}"))
}

pub(crate) async fn wait_with(
    trainer: &Trainer,
    directory: &JobDirectory,
    progress: &Reporter,
) -> Result<Job, TrainError> {
    let mut waiting = Instant::now();
    let mut running = false;
    loop {
        if let Some(update) = directory.read_progress() {
            progress.send(update);
        }
        let job = directory.read_job()?;
        match job.status {
            Status::Succeeded => {
                ended(directory, &job);
                return Ok(job);
            }
            Status::Failed => {
                ended(directory, &job);
                return Err(failure(job));
            }
            Status::Running => running = true,
            Status::Queued if running => {}
            Status::Queued if another_running(directory, job.id) => waiting = Instant::now(),
            Status::Queued if waiting.elapsed() > trainer.pickup => {
                if let Some(job) = give_up(directory)? {
                    ended(directory, &job);
                    return Err(TrainError::Failed(NOT_PICKED_UP.to_string()));
                }
            }
            Status::Queued => {}
        }
        tokio::time::sleep(trainer.poll_interval).await;
    }
}

/// Marks the job failed when it is still queued and this waiter wins its
/// [claim](CLAIM_FILE), and returns it when the mark held. A trainer that
/// claimed the job first keeps it, even before it writes `running`.
fn give_up(directory: &JobDirectory) -> Result<Option<Job>, TrainError> {
    if directory.read_job()?.status != Status::Queued || !directory.claim()? {
        return Ok(None);
    }
    let mut job = directory.read_job()?;
    if job.status != Status::Queued {
        return Ok(None);
    }
    job.status = Status::Failed;
    job.error = Some(NOT_PICKED_UP.to_string());
    directory.write_job(&job)?;
    let marked = directory.read_job()?;
    Ok((marked.status == Status::Failed).then_some(marked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::train::Phase;
    use abnegate_secret::SecretValue;
    use serde_json::json;
    use std::time::Duration;

    fn models() -> (tempfile::TempDir, Config) {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            models_directory: root.path().to_path_buf(),
            host_trainer: Some(
                Trainer::new()
                    .with_poll_interval(Duration::from_millis(5))
                    .with_pickup(Duration::from_secs(5)),
            ),
            ..Config::default()
        };
        (root, config)
    }

    fn queued(config: &Config, job: &Job) -> JobDirectory {
        let directory = JobDirectory::new(config, job.id);
        directory.write_job(job).unwrap();
        directory
    }

    #[test]
    fn the_job_file_keeps_its_wire_names() {
        let job = Job::new("jerry", "finetune", "base.safetensors")
            .with_subject("person")
            .with_trigger("ohwx")
            .with_architecture("sdxl")
            .with_base("sdxl")
            .with_filename("jerry.safetensors")
            .with_recipe_id("sdxl")
            .with_huggingface_base("SG161222/RealVisXL_V5.0")
            .with_image_count(40)
            .with_compute(Compute::Runpod)
            .with_secret("runpod.key");
        let mut job = job;
        job.gpu = Some("A40".into());
        job.pod_id = Some("pod-1".into());
        job.process_id = Some(4242);
        job.published = Some("checkpoints/jerry.safetensors".into());
        let encoded = serde_json::to_value(&job).unwrap();
        assert_eq!(
            encoded,
            json!({
                "schema_version": 1,
                "id": job.id,
                "name": "jerry",
                "method": "finetune",
                "subject": "person",
                "trigger": "ohwx",
                "architecture": "sdxl",
                "checkpoint": "base.safetensors",
                "base": "sdxl",
                "provider": "runpod",
                "gpu": "A40",
                "pod_id": "pod-1",
                "status": "queued",
                "filename": "jerry.safetensors",
                "recipe_id": "sdxl",
                "hf_base": "SG161222/RealVisXL_V5.0",
                "image_count": 40,
                "pid": 4242,
                "started_at": encoded["started_at"],
                "published": "checkpoints/jerry.safetensors",
                "secrets": ["runpod.key"],
            })
        );
        assert_eq!(serde_json::from_value::<Job>(encoded).unwrap(), job);
        for (status, name) in [
            (Status::Queued, "queued"),
            (Status::Running, "running"),
            (Status::Succeeded, "succeeded"),
            (Status::Failed, "failed"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), json!(name));
            assert_eq!(status.as_str(), name);
        }
        assert_eq!(
            (
                JOB_FILE,
                PROGRESS_FILE,
                LOCK_FILE,
                DISMISSED_FILE,
                PREVIEWS_DIRECTORY,
                DATASET_DIRECTORY,
                REBALANCE_FILE,
                DATA_DIRECTORY,
                TRAIN_DATA_FILE,
                VALID_DATA_FILE,
                FORMAT_FILE,
            ),
            (
                "job.json",
                "progress.json",
                "publish.lock",
                "dismissed",
                "previews",
                "dataset",
                "rebalance.json",
                "data",
                "train.jsonl",
                "valid.jsonl",
                "format.json",
            )
        );
    }

    #[test]
    fn a_job_the_trainer_rewrote_keeps_its_own_keys_and_reads_without_the_optional_ones() {
        let written = json!({
            "schema_version": 1,
            "id": "6f1c9a2e-5d4b-4c3a-9e8f-7a6b5c4d3e2f",
            "name": "yvonne",
            "method": "finetune",
            "checkpoint": "base.safetensors",
            "status": "running",
            "error": null,
            "pid": 4242,
            "step": 120,
            "total": 8000,
            "gpu": "A40",
            "pod_id": "pod-1",
            "started_at": "2026-10-05T01:02:03Z",
        });
        let job: Job = serde_json::from_value(written).unwrap();
        assert_eq!(job.subject, "");
        assert_eq!(job.trigger, "");
        assert_eq!(job.base, "");
        assert_eq!(job.status, Status::Running);
        assert_eq!(job.process_id, Some(4242));
        assert_eq!(job.error, None);
        assert_eq!(job.compute, Compute::Local);
        assert_eq!(job.gpu.as_deref(), Some("A40"));
        assert_eq!(job.pod_id.as_deref(), Some("pod-1"));
        assert_eq!(job.fields["step"], 120);
        let rewritten = serde_json::to_value(&job).unwrap();
        assert_eq!(rewritten["provider"], "local");
        assert_eq!(rewritten["gpu"], "A40");
        assert_eq!(rewritten["pod_id"], "pod-1");
        assert_eq!(rewritten["step"], 120);
        assert!(rewritten.get("progress").is_none());
    }

    #[test]
    fn a_host_field_cannot_replace_a_key_the_job_gives_a_meaning() {
        let job = Job::new("jerry", "lora", "base.safetensors")
            .with_field("status", "succeeded")
            .with_field("hf_base", "elsewhere")
            .with_field("provider", "runpod")
            .with_field("gpu", "H100")
            .with_field("tier", "overnight");
        assert_eq!(job.status, Status::Queued);
        assert_eq!(job.compute, Compute::Local);
        assert_eq!(job.gpu, None);
        for key in ["status", "hf_base", "provider", "gpu"] {
            assert!(!job.fields.contains_key(key), "{key} became a host field");
        }
        assert_eq!(job.fields["tier"], "overnight");
    }

    #[test]
    fn host_job_defaults_provider_to_local_and_round_trips_runpod_without_a_key() {
        let job = Job::new("jerry", "lora", "base.safetensors").with_trigger("ohwx");
        assert_eq!(job.compute, Compute::Local);
        assert!(job.secrets.is_empty());
        let mut value = serde_json::to_value(&job).unwrap();
        assert_eq!(value["provider"], "local");
        value.as_object_mut().unwrap().remove("provider");
        let loaded: Job = serde_json::from_value(value).unwrap();
        assert_eq!(loaded.compute, Compute::Local);

        let mut runpod = Job::new("jerry", "finetune", "base.safetensors")
            .with_trigger("ohwx")
            .with_compute(Compute::Runpod);
        runpod.gpu = Some("A40".into());
        runpod.pod_id = Some("pod-1".into());
        let encoded = serde_json::to_value(&runpod).unwrap();
        assert_eq!(encoded["provider"], "runpod");
        assert_eq!(encoded["gpu"], "A40");
        assert_eq!(encoded["pod_id"], "pod-1");
        assert_eq!(encoded["secrets"], json!(["runpod.key"]));
        assert!(encoded.get("runpod_api_key").is_none());
        assert!(encoded.get("api_key").is_none());
        let loaded: Job = serde_json::from_value(encoded).unwrap();
        assert_eq!(loaded.compute, Compute::Runpod);
        assert_eq!(loaded.gpu.as_deref(), Some("A40"));
        assert_eq!(loaded.pod_id.as_deref(), Some("pod-1"));
        for (compute, name, key_file) in [
            (Compute::Local, "local", None),
            (Compute::Runpod, "runpod", Some("runpod.key")),
        ] {
            assert_eq!(serde_json::to_value(compute).unwrap(), json!(name));
            assert_eq!(compute.as_str(), name);
            assert_eq!(compute.key_file(), key_file);
            assert_eq!(compute.is_remote(), key_file.is_some());
        }
    }

    #[test]
    fn a_job_json_a_host_writes_today_still_reads_and_keeps_its_keys() {
        let written = json!({
            "schema_version": 1,
            "id": "6f1c9a2e-5d4b-4c3a-9e8f-7a6b5c4d3e2f",
            "name": "jerry",
            "method": "finetune",
            "subject": "person",
            "trigger": "ohwx",
            "checkpoint": "lustifySDXLNSFW_ggwpV7.safetensors",
            "provider": "runpod",
            "gpu": "A40",
            "pod_id": "pod-1",
            "status": "running",
            "filename": "jerry.safetensors",
            "recipe_id": "sdxl-people",
            "hf_base": "SG161222/RealVisXL_V5.0",
            "image_count": 40,
            "step": 120,
            "total": 8000,
            "phase": "training",
            "message": "Training",
            "percent": 2,
            "loss": 0.21,
            "eta_seconds": 14400,
            "pid": 4242,
            "previews": ["previews/step-100-0.png"],
            "started_at": "2026-10-05T01:02:03.456789Z",
        });
        let job: Job = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(job.id.to_string(), "6f1c9a2e-5d4b-4c3a-9e8f-7a6b5c4d3e2f");
        assert_eq!(
            (job.name.as_str(), job.method.as_str(), job.subject.as_str()),
            ("jerry", "finetune", "person")
        );
        assert_eq!(job.trigger, "ohwx");
        assert_eq!(job.checkpoint, "lustifySDXLNSFW_ggwpV7.safetensors");
        assert_eq!(job.compute, Compute::Runpod);
        assert_eq!(job.gpu.as_deref(), Some("A40"));
        assert_eq!(job.pod_id.as_deref(), Some("pod-1"));
        assert_eq!(job.status, Status::Running);
        assert_eq!(job.filename.as_deref(), Some("jerry.safetensors"));
        assert_eq!(job.recipe_id, "sdxl-people");
        assert_eq!(job.huggingface_base, "SG161222/RealVisXL_V5.0");
        assert_eq!(job.image_count, 40);
        assert_eq!(job.process_id, Some(4242));
        assert_eq!(job.base, "");
        assert!(job.secrets.is_empty());
        assert!(job.busy());

        let rewritten = serde_json::to_value(&job).unwrap();
        for (key, value) in written.as_object().unwrap() {
            if key == "started_at" {
                let started: chrono::DateTime<chrono::Utc> =
                    serde_json::from_value(rewritten[key].clone()).unwrap();
                assert_eq!(started, job.started_at);
                continue;
            }
            assert_eq!(&rewritten[key], value, "{key} changed on a rewrite");
        }
    }

    #[test]
    fn a_language_job_round_trips_the_subject_its_host_names() {
        let job = Job::new("support-bot", "lora", "qwen2.5:7b")
            .with_subject("language")
            .with_recipe_id("chat")
            .with_filename("support-bot");
        let loaded: Job = serde_json::from_slice(&serde_json::to_vec(&job).unwrap()).unwrap();
        assert_eq!(loaded.subject, "language");
        assert_eq!(loaded.recipe_id, "chat");
        assert_eq!(loaded.checkpoint, "qwen2.5:7b");
        assert_eq!(loaded.filename.as_deref(), Some("support-bot"));
        assert_eq!(loaded.trigger, "");
        assert_eq!(loaded.base, "");
    }

    #[test]
    fn current_picks_the_latest_job() {
        let (_root, config) = models();
        let older = Job::new("first", "lora", "base.safetensors");
        let mut newer = Job::new("second", "finetune", "base.safetensors");
        newer.started_at = older.started_at + chrono::Duration::seconds(5);
        queued(&config, &older);
        let directory = queued(&config, &newer);
        fs::write(root(&config).join("not-a-job"), b"").unwrap();
        fs::create_dir(root(&config).join(Uuid::new_v4().to_string())).unwrap();

        assert_eq!(current(&config).unwrap().name, "second");
        assert_eq!(jobs(&config).len(), 2);
        assert!(busy(&config));
        newer.status = Status::Succeeded;
        directory.write_job(&newer).unwrap();
        assert!(!busy(&config));
        assert!(current(&Config::default()).is_none());
    }

    #[test]
    fn current_with_progress_reads_the_sidecar() {
        let (_root, config) = models();
        let job = Job::new("jerry", "lora", "base.safetensors");
        let directory = queued(&config, &job);
        fs::write(
            directory.path().join(PROGRESS_FILE),
            br#"{"step":0,"total":8000,"phase":"class_images","message":"Generating class image 12 of 2000","percent":8,"eta_seconds":14400,"loss":0.21}"#,
        )
        .unwrap();

        let viewed = current_with_progress(&config).unwrap();
        assert_eq!(viewed.name, "jerry");
        let progress = viewed.progress.unwrap();
        assert_eq!((progress.step, progress.total), (0, 8000));
        assert_eq!(progress.phase, Some(Phase::ClassImages));
        assert_eq!(
            progress.message.as_deref(),
            Some("Generating class image 12 of 2000")
        );
        assert_eq!(progress.percent, Some(8));
        assert_eq!(progress.eta, Some(Duration::from_secs(14400)));
        assert_eq!(progress.loss, Some(0.21));
        assert!(progress.previews.is_empty());

        fs::write(
            directory.path().join(PROGRESS_FILE),
            br#"{"step":250,"total":8000,"previews":["previews/step-250-0.png"]}"#,
        )
        .unwrap();
        let viewed = current_with_progress(&config).unwrap();
        assert_eq!(
            viewed.progress.unwrap().previews,
            vec!["previews/step-250-0.png".to_string()]
        );
        assert!(current(&config).unwrap().progress.is_none());
    }

    #[test]
    fn preview_resolves_a_basename_under_the_current_job() {
        let (_root, config) = models();
        let job = Job::new("jerry", "lora", "base.safetensors");
        let directory = queued(&config, &job);
        let previews = directory.path().join(PREVIEWS_DIRECTORY);
        fs::create_dir_all(&previews).unwrap();
        let png = previews.join("step-250-0.png");
        fs::write(&png, b"png").unwrap();

        assert_eq!(preview(&config, "step-250-0.png"), Some(png));
        for name in ["../job.json", "..", "", "nested/step.png", "missing.png"] {
            assert!(preview(&config, name).is_none(), "{name:?} resolved");
        }
        let (_empty, nothing) = models();
        assert!(preview(&nothing, "step-250-0.png").is_none());
    }

    #[test]
    fn queued_progress_keeps_parse_progress_alive() {
        let (_root, config) = models();
        let directory = JobDirectory::new(&config, Uuid::new_v4());
        let trainer = config.host_trainer.clone().unwrap();
        let written = directory
            .write_queued_progress(trainer.steps_for(40))
            .unwrap();

        let progress =
            TrainProgress::parse(&fs::read(directory.path().join(PROGRESS_FILE)).unwrap()).unwrap();
        assert_eq!(progress, written);
        assert_eq!(progress.step, 0);
        assert_eq!(progress.total, 800);
        assert_eq!(progress.phase, Some(Phase::Queued));
        assert_eq!(progress.percent, Some(0));
        assert!(!directory.path().join("progress.json.tmp").exists());
    }

    #[test]
    fn person_step_budget_matches_the_host_config() {
        let trainer = Trainer::default();
        assert_eq!(trainer.steps_for(1), 500);
        assert_eq!(trainer.steps_for(25), 500);
        assert_eq!(trainer.steps_for(26), 520);
        assert_eq!(trainer.steps_for(400), 8000);
        assert_eq!(trainer.steps_for(500), 10000);
        assert_eq!(trainer.steps_for(2000), 40000);
        assert_eq!(trainer.steps_for(2500), 40000);
        assert_eq!(trainer.steps_for(usize::MAX), 40000);
        let tuned = Trainer::default().with_steps(20, 150, 2000);
        assert_eq!(tuned.steps_for(7), 150);
        assert_eq!(tuned.steps_for(8), 160);
        assert_eq!(tuned.steps_for(200), 2000);
    }

    #[test]
    fn a_trainer_that_could_never_report_progress_is_refused() {
        assert!(Trainer::default().validate().is_ok());
        for trainer in [
            Trainer::default().with_pickup(Duration::ZERO),
            Trainer::default().with_poll_interval(Duration::ZERO),
            Trainer::default().with_steps(20, 10, 0),
            Trainer::default().with_steps(20, 900, 800),
        ] {
            assert!(trainer.validate().is_err(), "{trainer:?} was accepted");
        }
    }

    #[test]
    fn stage_copies_a_dataset_with_every_companion() {
        let (root, config) = models();
        let source = root.path().join("attempt");
        fs::create_dir_all(source.join("nested")).unwrap();
        for (name, bytes) in [
            ("0000.png", b"png".as_slice()),
            ("0000.txt", b"ohwx person"),
            ("0000.mask.png", b"mask"),
            ("0000.kind", b"body"),
            ("0000.pose", b"standing"),
            (REBALANCE_FILE, b"{}"),
        ] {
            fs::write(source.join(name), bytes).unwrap();
        }
        let directory = JobDirectory::new(&config, Uuid::new_v4());

        directory.stage(&source, DATASET_DIRECTORY).unwrap();

        let dataset = directory.path().join(DATASET_DIRECTORY);
        assert_eq!(fs::read(dataset.join("0000.png")).unwrap(), b"png");
        assert_eq!(fs::read(dataset.join("0000.txt")).unwrap(), b"ohwx person");
        assert_eq!(fs::read(dataset.join("0000.mask.png")).unwrap(), b"mask");
        assert_eq!(fs::read(dataset.join("0000.kind")).unwrap(), b"body");
        assert_eq!(fs::read(dataset.join("0000.pose")).unwrap(), b"standing");
        assert_eq!(fs::read(dataset.join(REBALANCE_FILE)).unwrap(), b"{}");
        assert!(!dataset.join("nested").exists());
        for directory_name in ["", "..", "../escape", "nested/data"] {
            assert!(directory.stage(&source, directory_name).is_err());
        }
    }

    #[test]
    fn stage_copies_text_data_without_a_dataset() {
        let (root, config) = models();
        let source = root.path().join("data");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join(TRAIN_DATA_FILE), b"{\"text\":\"hi\"}\n").unwrap();
        fs::write(source.join(FORMAT_FILE), b"{\"mask_prompt\":false}").unwrap();
        let directory = JobDirectory::new(&config, Uuid::new_v4());

        directory.stage(&source, DATA_DIRECTORY).unwrap();

        let data = directory.path().join(DATA_DIRECTORY);
        assert_eq!(
            fs::read(data.join(TRAIN_DATA_FILE)).unwrap(),
            b"{\"text\":\"hi\"}\n"
        );
        assert_eq!(
            fs::read(data.join(FORMAT_FILE)).unwrap(),
            b"{\"mask_prompt\":false}"
        );
        assert!(!directory.path().join(DATASET_DIRECTORY).exists());
    }

    #[test]
    fn a_secret_is_one_line_its_owner_alone_reads() {
        let (_root, config) = models();
        let directory = JobDirectory::new(&config, Uuid::new_v4());
        directory
            .write_secret("runpod.key", &SecretValue::new(" rp-secret \n"))
            .unwrap();
        let path = directory.path().join("runpod.key");
        assert_eq!(fs::read_to_string(&path).unwrap(), "rp-secret\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        for name in [
            JOB_FILE,
            PROGRESS_FILE,
            DATASET_DIRECTORY,
            "pid",
            "jerry.safetensors",
            "job.json.tmp",
            ".hidden",
            "../escape",
            "",
            "JOB.JSON",
            "Progress.Json",
            "publish.lock",
            "dismissed",
            "Jerry.SafeTensors",
            "key.TMP",
        ] {
            assert!(
                directory
                    .write_secret(name, &SecretValue::new("x"))
                    .is_err(),
                "{name:?} was written"
            );
        }
        directory.remove_secrets(&["runpod.key".to_string(), "../job.json".to_string()]);
        assert!(!path.exists());
        assert_eq!(
            fs::read_dir(directory.path()).unwrap().count(),
            0,
            "no temporary file is left behind"
        );
    }

    #[test]
    fn write_runpod_key_is_one_line_and_mode_600() {
        let (_root, config) = models();
        let directory = JobDirectory::new(&config, Uuid::new_v4());
        directory
            .write_compute_key(Compute::Runpod, &SecretValue::new(" rp-secret \n"))
            .unwrap();
        let path = directory.path().join("runpod.key");
        assert_eq!(fs::read_to_string(&path).unwrap(), "rp-secret\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(!directory.path().join(JOB_FILE).exists());

        for (compute, key) in [(Compute::Local, "rp-secret"), (Compute::Runpod, " \n")] {
            let elsewhere = JobDirectory::new(&config, Uuid::new_v4());
            let error = elsewhere
                .write_compute_key(compute, &SecretValue::new(key))
                .unwrap_err();
            assert!(matches!(error, TrainError::Invalid(_)), "{error:?}");
            assert!(!elsewhere.path().exists(), "{compute:?} wrote a key");
        }
    }

    #[test]
    fn dismiss_hides_a_finished_job_and_refuses_a_running_one() {
        let (_root, config) = models();
        dismiss(&config).expect("with no job there is nothing to dismiss");
        let mut job = Job::new("yvonne", "finetune", "base.safetensors").with_trigger("ohwx");
        job.status = Status::Failed;
        job.error = Some("cancelled".into());
        let directory = queued(&config, &job);
        assert_eq!(
            current_with_progress(&config).unwrap().error.as_deref(),
            Some("cancelled")
        );

        dismiss(&config).unwrap();
        assert!(current_with_progress(&config).is_none());
        assert_eq!(current(&config).unwrap().name, "yvonne");
        assert!(directory.path().join(DISMISSED_FILE).is_file());
        assert!(directory.path().join(JOB_FILE).is_file());
        dismiss(&config).unwrap();

        let mut running = Job::new("jerry", "lora", "base.safetensors").with_trigger("ohwx");
        running.started_at = job.started_at + chrono::Duration::seconds(5);
        let running_directory = queued(&config, &running);
        let error = dismiss(&config).unwrap_err();
        assert!(
            matches!(error, TrainError::Invalid(message) if message == STILL_RUNNING),
            "{error:?}"
        );
        assert!(!running_directory.path().join(DISMISSED_FILE).exists());
        assert_eq!(current_with_progress(&config).unwrap().name, "jerry");
    }

    fn ended_runpod_job(config: &Config, status: Status) -> (Job, PathBuf) {
        let mut job = Job::new("jerry", "lora", "base.safetensors").with_compute(Compute::Runpod);
        job.status = status;
        let directory = queued(config, &job);
        directory
            .write_compute_key(Compute::Runpod, &SecretValue::new("rp-secret"))
            .unwrap();
        (job, directory.path().join("runpod.key"))
    }

    #[test]
    fn dismissing_a_finished_job_removes_its_key() {
        let (_root, config) = models();
        let (_job, key) = ended_runpod_job(&config, Status::Succeeded);

        dismiss(&config).unwrap();

        assert!(!key.exists(), "the key outlived its dismissed job");
        assert!(current_with_progress(&config).is_none());
    }

    #[tokio::test]
    async fn resuming_an_ended_job_that_cannot_publish_removes_its_key() {
        let (_root, config) = models();
        let (job, key) = ended_runpod_job(&config, Status::Failed);

        let error = resume(&config, job.id).await.unwrap_err();

        assert!(
            matches!(error, TrainError::Invalid(message) if message == UNPUBLISHED),
            "{error:?}"
        );
        assert!(!key.exists(), "the key outlived its refused job");
    }

    #[test]
    fn a_job_that_lists_no_secrets_still_loses_its_compute_key() {
        let (_root, config) = models();
        let mut job = Job::new("jerry", "lora", "base.safetensors").with_compute(Compute::Runpod);
        job.status = Status::Succeeded;
        job.secrets.clear();
        let directory = queued(&config, &job);
        directory
            .write_compute_key(Compute::Runpod, &SecretValue::new("rp-secret"))
            .unwrap();

        dismiss(&config).unwrap();

        assert!(
            !directory.path().join("runpod.key").exists(),
            "a job.json rewritten without its secrets kept the key"
        );
    }

    #[test]
    fn recovery_removes_an_unqueued_job_nobody_holds() {
        let (_root, config) = models();
        let crashed = JobDirectory::new(&config, Uuid::new_v4());
        crashed
            .write_compute_key(Compute::Runpod, &SecretValue::new("rp-secret"))
            .unwrap();
        let staging = JobDirectory::new(&config, Uuid::new_v4());
        staging
            .write_compute_key(Compute::Runpod, &SecretValue::new("rp-secret"))
            .unwrap();
        let held = staging.try_lock().unwrap().expect("nobody holds the job");
        let stranger = root(&config).join("not-a-job");
        fs::create_dir_all(&stranger).unwrap();

        assert!(recover(&config).is_empty());

        assert!(!crashed.path().exists(), "the crashed job's key was left");
        assert!(
            staging.path().join("runpod.key").is_file(),
            "a job being staged lost its key"
        );
        assert!(stranger.is_dir(), "a directory that is no job was removed");
        drop(held);
    }

    #[cfg(unix)]
    #[test]
    fn a_secret_replaces_a_file_others_could_read_with_one_they_cannot() {
        use std::os::unix::fs::PermissionsExt;
        let (_root, config) = models();
        let directory = JobDirectory::new(&config, Uuid::new_v4());
        directory.create().unwrap();
        let path = directory.path().join("runpod.key");
        fs::write(&path, b"stale\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        directory
            .write_secret("runpod.key", &SecretValue::new("rp-secret"))
            .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "rp-secret\n");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(directory.path()).unwrap().permissions().mode() & 0o777,
            0o700,
            "the job directory is its owner's alone"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_secret_never_follows_a_link_in_its_place() {
        let (root, config) = models();
        let directory = JobDirectory::new(&config, Uuid::new_v4());
        directory.create().unwrap();
        let target = root.path().join("elsewhere");
        fs::write(&target, b"untouched").unwrap();
        let path = directory.path().join("runpod.key");
        std::os::unix::fs::symlink(&target, &path).unwrap();

        let error = directory
            .write_secret("runpod.key", &SecretValue::new("rp-secret"))
            .unwrap_err();

        assert!(matches!(error, TrainError::Invalid(_)), "{error:?}");
        assert_eq!(fs::read(&target).unwrap(), b"untouched");
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_job_written_for_a_newer_contract_is_refused() {
        let (_root, config) = models();
        let mut job = Job::new("jerry", "lora", "base.safetensors");
        job.schema_version = SCHEMA_VERSION + 1;
        let directory = queued(&config, &job);

        let error = directory.read_job().unwrap_err();

        assert!(matches!(error, TrainError::Invalid(_)), "{error:?}");
        assert!(jobs(&config).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_job_directory_is_no_job() {
        let (root, config) = models();
        let job = Job::new("jerry", "lora", "base.safetensors");
        let elsewhere = JobDirectory::at(root.path().join("elsewhere"));
        elsewhere.write_job(&job).unwrap();
        fs::create_dir_all(super::root(&config)).unwrap();
        std::os::unix::fs::symlink(
            elsewhere.path(),
            super::root(&config).join(job.id.to_string()),
        )
        .unwrap();

        assert!(jobs(&config).is_empty());
    }

    #[tokio::test]
    async fn a_job_the_trainer_claimed_before_running_keeps_its_inputs() {
        let (root, config) = models();
        let job = Job::new("jerry", "lora", "base.safetensors");
        let directory = queued(&config, &job);
        let source = root.path().join("set");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("0000.png"), b"png").unwrap();
        directory.stage(&source, DATASET_DIRECTORY).unwrap();
        fs::write(directory.path().join(CLAIM_FILE), b"").unwrap();
        let trainer = Trainer::new()
            .with_poll_interval(Duration::from_millis(5))
            .with_pickup(Duration::from_millis(10));

        let waited = tokio::time::timeout(
            Duration::from_millis(300),
            wait_with(&trainer, &directory, &Reporter::default()),
        )
        .await;

        assert!(
            waited.is_err(),
            "the waiter gave up a claimed job: {waited:?}"
        );
        assert_eq!(directory.read_job().unwrap().status, Status::Queued);
        assert!(
            directory
                .path()
                .join(DATASET_DIRECTORY)
                .join("0000.png")
                .is_file()
        );
    }

    #[test]
    fn a_job_the_trainer_claimed_is_never_given_up() {
        let (_root, config) = models();
        let mut job = Job::new("jerry", "lora", "base.safetensors");
        job.status = Status::Running;
        let directory = queued(&config, &job);

        assert_eq!(give_up(&directory).unwrap(), None);
        assert_eq!(directory.read_job().unwrap().status, Status::Running);
    }

    #[tokio::test]
    async fn a_job_queued_behind_a_running_one_is_not_given_up() {
        let (_root, mut config) = models();
        config.host_trainer = Some(
            Trainer::new()
                .with_poll_interval(Duration::from_millis(5))
                .with_pickup(Duration::from_millis(20)),
        );
        let mut ahead = Job::new("first", "lora", "base.safetensors");
        ahead.status = Status::Running;
        queued(&config, &ahead);
        let behind = Job::new("second", "lora", "base.safetensors");
        let directory = queued(&config, &behind);

        let waited =
            tokio::time::timeout(Duration::from_millis(200), wait(&config, &directory)).await;

        assert!(waited.is_err(), "the job was given up: {waited:?}");
        assert_eq!(directory.read_job().unwrap().status, Status::Queued);
    }

    #[tokio::test]
    async fn a_failed_job_loses_its_secrets_and_what_it_trained_on() {
        let (root, config) = models();
        let mut job = Job::new("jerry", "lora", "base.safetensors").with_secret("runpod.key");
        job.status = Status::Failed;
        let directory = queued(&config, &job);
        directory
            .write_secret("runpod.key", &SecretValue::new("rp"))
            .unwrap();
        let source = root.path().join("set");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("0000.png"), b"png").unwrap();
        directory.stage(&source, DATASET_DIRECTORY).unwrap();
        fs::create_dir_all(directory.path().join(PREVIEWS_DIRECTORY)).unwrap();

        wait(&config, &directory).await.unwrap_err();

        assert!(!directory.path().join("runpod.key").exists());
        assert!(!directory.path().join(DATASET_DIRECTORY).exists());
        assert!(directory.path().join(JOB_FILE).is_file());
        assert!(directory.path().join(PREVIEWS_DIRECTORY).is_dir());
    }

    #[test]
    fn remove_refuses_a_job_whose_record_cannot_be_read() {
        let (_root, config) = models();
        let id = Uuid::new_v4();
        let directory = JobDirectory::new(&config, id);
        directory.create().unwrap();
        fs::write(directory.path().join(JOB_FILE), b"{ half written").unwrap();

        assert!(
            matches!(remove(&config, id), Err(TrainError::Invalid(message)) if message == UNREADABLE)
        );
        assert!(directory.path().is_dir());

        fs::remove_file(directory.path().join(JOB_FILE)).unwrap();
        remove(&config, id).expect("a directory with no record is removable");
        assert!(!directory.path().exists());
    }

    #[test]
    fn a_secret_of_more_than_one_line_is_refused() {
        let (_root, config) = models();
        for value in [
            " rp-secret\nPATH=/tmp \n",
            "rp\rsecret",
            "rp\tsecret",
            "rp\u{85}secret",
            "rp\u{2028}secret",
            "rp\u{2029}secret",
        ] {
            let directory = JobDirectory::new(&config, Uuid::new_v4());
            let error = directory
                .write_secret("runpod.key", &SecretValue::new(value))
                .unwrap_err();
            assert!(
                matches!(error, TrainError::Invalid(_)),
                "{value:?}: {error:?}"
            );
            assert!(
                !directory.path().exists(),
                "{value:?} created the job directory before it was refused"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn staging_never_follows_a_link_in_place_of_its_directory() {
        let (root, config) = models();
        let directory = JobDirectory::new(&config, Uuid::new_v4());
        directory.create().unwrap();
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, directory.path().join(DATASET_DIRECTORY)).unwrap();
        let source = root.path().join("set");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("0000.png"), b"png").unwrap();

        let error = directory.stage(&source, DATASET_DIRECTORY).unwrap_err();

        assert!(matches!(error, TrainError::Invalid(_)), "{error:?}");
        assert!(
            !elsewhere.join("0000.png").exists(),
            "staging wrote through a link"
        );
    }

    #[test]
    fn remove_takes_away_an_ended_job_nobody_holds() {
        let (_root, config) = models();
        let mut job = Job::new("jerry", "lora", "base.safetensors");
        let directory = queued(&config, &job);
        assert!(
            matches!(remove(&config, job.id), Err(TrainError::Invalid(message)) if message == STILL_BUSY)
        );

        job.status = Status::Succeeded;
        directory.write_job(&job).unwrap();
        let held = directory.try_lock().unwrap().expect("nobody holds the job");
        assert!(
            matches!(remove(&config, job.id), Err(TrainError::Invalid(message)) if message == BEING_PUBLISHED)
        );
        drop(held);

        remove(&config, job.id).unwrap();
        assert!(!directory.path().exists());
        remove(&config, job.id).expect("a removed job is already removed");
    }

    fn run_then_succeed(directory: &JobDirectory) {
        let trainer = directory.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            let mut running = trainer.read_job().unwrap();
            running.status = Status::Running;
            trainer.write_job(&running).unwrap();
            trainer
                .write_progress(&TrainProgress::new(3, 10).with_phase(Phase::Training))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            running.status = Status::Succeeded;
            trainer.write_job(&running).unwrap();
        });
    }

    #[tokio::test]
    async fn wait_returns_when_the_host_job_succeeds_and_keeps_its_latest_progress() {
        let (_root, config) = models();
        let job = Job::new("jerry", "lora", "base.safetensors");
        let directory = queued(&config, &job);
        run_then_succeed(&directory);
        let (sender, receiver) = watch::channel(None);

        let finished = wait_reporting(&config, &directory, sender).await.unwrap();

        assert_eq!(finished.status, Status::Succeeded);
        assert_eq!(
            *receiver.borrow(),
            Some(TrainProgress::new(3, 10).with_phase(Phase::Training))
        );
    }

    #[tokio::test]
    async fn wait_forwards_each_new_progress_once() {
        let (_root, config) = models();
        let job = Job::new("jerry", "lora", "base.safetensors");
        let directory = queued(&config, &job);
        run_then_succeed(&directory);
        let (reporter, published) = Reporter::recording();

        wait_with(config.host_trainer.as_ref().unwrap(), &directory, &reporter)
            .await
            .unwrap();

        assert_eq!(
            *published.lock().unwrap(),
            [TrainProgress::new(3, 10).with_phase(Phase::Training)],
            "the same progress is sent once"
        );
    }

    #[tokio::test]
    async fn a_failed_job_fails_with_what_the_trainer_wrote() {
        let (_root, config) = models();
        let mut job = Job::new("jerry", "lora", "base.safetensors");
        job.status = Status::Failed;
        job.error = Some("CUDA out of memory".into());
        let directory = queued(&config, &job);

        let error = wait(&config, &directory).await.unwrap_err();

        assert!(
            matches!(&error, TrainError::Failed(message) if message.contains("CUDA out of memory")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_job_nobody_picks_up_fails_and_stays_failed() {
        let (_root, mut config) = models();
        config.host_trainer = Some(
            Trainer::new()
                .with_poll_interval(Duration::from_millis(5))
                .with_pickup(Duration::from_millis(40)),
        );
        let job = Job::new("jerry", "lora", "base.safetensors");
        let directory = queued(&config, &job);

        let error = wait(&config, &directory).await.unwrap_err();

        assert!(
            matches!(&error, TrainError::Failed(message) if message == NOT_PICKED_UP),
            "{error}"
        );
        let marked = directory.read_job().unwrap();
        assert_eq!(marked.status, Status::Failed);
        assert_eq!(marked.error.as_deref(), Some(NOT_PICKED_UP));
    }

    #[tokio::test]
    async fn waiting_without_a_host_trainer_is_disabled() {
        let (_root, mut config) = models();
        config.host_trainer = None;
        let job = Job::new("jerry", "lora", "base.safetensors");
        let directory = queued(&config, &job);

        let error = wait(&config, &directory).await.unwrap_err();

        assert!(matches!(error, TrainError::Disabled), "{error}");
    }
}
