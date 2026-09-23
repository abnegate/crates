//! Scores a trained adapter against its own base and promotes the best checkpoint.

use crate::config::Config;
use crate::http::CANCEL_TIMEOUT;
use crate::http::POLL_TIMEOUT;
use crate::recipe::TrainingModel;
use crate::train::MIN_WEIGHT_BYTES;
use crate::train::PACKAGED_TRAIN_CONFIG;
use crate::train::{Contract, Run};
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

const RANK_PERCENT: &str = "0.5";
const RANK_IMAGES: usize = 4;
const MEASURE_PERCENTS: &str = "0.2,0.6,0.9";
const PROBE_SEED: u64 = 1234;
const FINAL: &str = "final";

#[derive(Debug, Deserialize)]
struct Settings {
    resolution: u32,
    #[serde(default)]
    checkpoint_every: u32,
    #[serde(default = "default_checkpoints_per_run")]
    checkpoints_per_run: u32,
}

fn default_checkpoints_per_run() -> u32 {
    8
}

/// How much better an adapter fits its own training images than its base does.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Quality {
    pub improvement: f32,
    pub checkpoint: String,
    pub measured: bool,
    pub calibration: QualityCalibration,
}

/// Whether callers may compare the score with the FLUX health thresholds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum QualityCalibration {
    FluxHealthBands,
    Uncalibrated,
}

struct Candidate {
    label: String,
    lora: String,
    staged: Option<PathBuf>,
    mean: f64,
}

struct Sample {
    folder: String,
    manifest: String,
}

/// Ranks every checkpoint the run left behind, promotes the winner over
/// `output`, and deletes only this run's temporary namespace. `None` leaves the
/// trained adapter exactly where the trainer wrote it.
pub async fn select(
    config: &Config,
    model: &TrainingModel,
    run: &Run,
    output: &Path,
    captions: &HashMap<String, String>,
) -> Option<Quality> {
    match crate::http::client(config) {
        Ok(client) => select_with(&client, config, model, run, output, captions).await,
        Err(error) => {
            tracing::warn!(%error, "could not reach ComfyUI; the adapter stands unscored");
            None
        }
    }
}

/// [`select`] on a client the caller already holds for this run.
pub(crate) async fn select_with(
    client: &reqwest::Client,
    config: &Config,
    model: &TrainingModel,
    run: &Run,
    output: &Path,
    captions: &HashMap<String, String>,
) -> Option<Quality> {
    let selection = match Selection::new(client, config, model, run, output, captions) {
        Some(selection) => selection,
        None => {
            // Silence here reads to the caller as "the trainer produced
            // nothing", which is what the error it raises next says. Name the
            // step that refused instead.
            tracing::warn!(
                artifact = %run.artifact,
                models_directory = %config.models_directory.display(),
                images = captions.len(),
                loras = models_loras(config).is_some(),
                produced = produced(config).is_some(),
                manifest = crate::train::manifest(model, captions).is_some(),
                "quality selection could not start; the adapter stands unscored"
            );
            crate::train::cleanup_with(client, config, run).await;
            return None;
        }
    };
    let deadline = Instant::now() + Duration::from_secs(config.train_timeout_seconds);
    let sample = subsample(config, model, &run.folder, captions, RANK_IMAGES);
    let quality = selection.choose(sample.as_ref(), deadline).await;
    if selection.probe.cleanup.load(Ordering::Acquire) {
        discard(config, sample.as_ref());
        selection.sweep();
        crate::train::cleanup_with(client, config, run).await;
    } else {
        tracing::warn!(
            prompt_namespace = %run.folder,
            "retaining LoRA inputs because an abandoned probe did not reach terminal history"
        );
    }
    quality
}

struct Selection<'a> {
    probe: Probe<'a>,
    settings: Settings,
    folder: String,
    output: PathBuf,
    adapter: String,
    stem: String,
    loras: PathBuf,
    images: usize,
}

impl<'a> Selection<'a> {
    fn new(
        client: &reqwest::Client,
        config: &'a Config,
        model: &'a TrainingModel,
        run: &Run,
        output: &Path,
        captions: &HashMap<String, String>,
    ) -> Option<Self> {
        config.validate().ok()?;
        run.validate(&config.contract).ok()?;

        let settings: Settings = serde_json::from_str(PACKAGED_TRAIN_CONFIG).ok()?;
        let adapter = format!("{}.safetensors", run.artifact);
        artifact(&adapter, &run.artifact, &config.contract.artifact_prefix)?;
        Some(Self {
            probe: Probe::new(client, config, model, run, captions, settings.resolution)?,
            settings,
            folder: run.folder.clone(),
            output: output.to_path_buf(),
            adapter,
            stem: run.artifact.clone(),
            loras: models_loras(config)?,
            images: captions.len(),
        })
    }

    async fn choose(&self, sample: Option<&Sample>, deadline: Instant) -> Option<Quality> {
        let (folder, manifest) = sample
            .map(|sample| (sample.folder.as_str(), sample.manifest.as_str()))
            .unwrap_or((&self.folder, &self.probe.manifest));
        if self
            .probe
            .stage_remote(&self.adapter, deadline)
            .await
            .is_none()
        {
            tracing::warn!(
                adapter = %self.adapter,
                "could not stage the trained adapter for probing; the adapter stands unscored"
            );
            return None;
        }
        let base = match self
            .probe
            .mean(folder, manifest, None, RANK_PERCENT, deadline)
            .await
        {
            Some(base) if base > 0.0 => base,
            other => {
                tracing::warn!(
                    base = ?other,
                    "the base model did not produce a usable loss; the adapter stands unscored"
                );
                return None;
            }
        };
        let mut best = Candidate {
            label: FINAL.to_string(),
            lora: self.adapter.clone(),
            staged: None,
            mean: self
                .probe
                .mean(
                    folder,
                    manifest,
                    Some(&self.adapter),
                    RANK_PERCENT,
                    deadline,
                )
                .await?,
        };
        for step in self.candidates() {
            if Instant::now() >= deadline {
                break;
            }
            let name = format!("{}-step{step}.safetensors", self.stem);
            let Some(staged) = destination(&self.loras, &name, &self.stem, self.prefix()) else {
                continue;
            };
            if !self.probe.stage(&name, &staged, deadline).await {
                continue;
            }
            match self
                .probe
                .mean(folder, manifest, Some(&name), RANK_PERCENT, deadline)
                .await
            {
                Some(mean) if mean < best.mean => {
                    if let Some(beaten) = best.staged.take() {
                        let _ = fs::remove_file(beaten);
                    }
                    best = Candidate {
                        label: format!("step{step}"),
                        lora: name,
                        staged: Some(staged),
                        mean,
                    };
                }
                _ => {
                    if self.probe.cleanup.load(Ordering::Acquire) {
                        let _ = fs::remove_file(staged);
                    }
                }
            }
        }
        let (base, winner, measured) = match self.measure(&best, deadline).await {
            Some((base, winner)) if base > 0.0 => (base, winner, true),
            _ => (base, best.mean, false),
        };
        if let Some(staged) = &best.staged {
            let bytes = read_regular(staged)?;
            crate::train::atomic_write(&self.output, &bytes).ok()?;
        }
        Some(Quality {
            improvement: ((base - winner) / base) as f32,
            checkpoint: best.label,
            measured,
            calibration: match self.probe.model {
                TrainingModel::Flux { .. } => QualityCalibration::FluxHealthBands,
                TrainingModel::QwenEdit { .. } => QualityCalibration::Uncalibrated,
            },
        })
    }

    fn candidates(&self) -> Vec<u32> {
        if let Some(written) = self.written()
            && !written.is_empty()
        {
            return written;
        }
        let Some(steps) = steps(self.images) else {
            return Vec::new();
        };
        let interval = interval(&self.settings, steps);
        if interval == 0 {
            return Vec::new();
        }
        (1..)
            .map(|multiple| multiple * interval)
            .take_while(|step| *step < steps)
            .collect()
    }

    fn written(&self) -> Option<Vec<u32>> {
        let mut steps: Vec<u32> = fs::read_dir(produced(self.probe.config)?)
            .ok()?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let kind = entry.file_type().ok()?;
                if kind.is_symlink() || !kind.is_file() {
                    return None;
                }
                let name = entry.file_name().into_string().ok()?;
                artifact(&name, &self.stem, self.prefix()).flatten()
            })
            .collect();
        steps.sort_unstable();
        Some(steps)
    }

    async fn measure(&self, best: &Candidate, deadline: Instant) -> Option<(f64, f64)> {
        let base = self
            .probe
            .mean(
                &self.folder,
                &self.probe.manifest,
                None,
                MEASURE_PERCENTS,
                deadline,
            )
            .await?;
        let winner = self
            .probe
            .mean(
                &self.folder,
                &self.probe.manifest,
                Some(&best.lora),
                MEASURE_PERCENTS,
                deadline,
            )
            .await?;
        Some((base, winner))
    }

    fn prefix(&self) -> &str {
        self.probe.prefix()
    }

    fn sweep(&self) {
        let directories = [Some(self.loras.clone()), produced(self.probe.config)];
        for directory in directories.into_iter().flatten() {
            let Ok(entries) = fs::read_dir(directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if entry.file_type().is_ok_and(|kind| kind.is_file())
                    && artifact(&name, &self.stem, self.prefix()).is_some_and(|step| step.is_some())
                {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

struct Probe<'a> {
    config: &'a Config,
    model: &'a TrainingModel,
    client: reqwest::Client,
    manifest: String,
    resolution: u32,
    stem: String,
    cleanup: AtomicBool,
    cancel_timeout: Duration,
}

impl<'a> Probe<'a> {
    fn new(
        client: &reqwest::Client,
        config: &'a Config,
        model: &'a TrainingModel,
        run: &Run,
        captions: &HashMap<String, String>,
        resolution: u32,
    ) -> Option<Self> {
        Some(Self {
            client: client.clone(),
            config,
            model,
            manifest: crate::train::manifest(model, captions)?,
            resolution,
            stem: run.artifact.clone(),
            cleanup: AtomicBool::new(true),
            cancel_timeout: CANCEL_TIMEOUT,
        })
    }

    fn prefix(&self) -> &str {
        &self.config.contract.artifact_prefix
    }

    async fn mean(
        &self,
        folder: &str,
        manifest: &str,
        lora: Option<&str>,
        percents: &str,
        deadline: Instant,
    ) -> Option<f64> {
        let graph = graph(
            self.model,
            folder,
            manifest,
            self.resolution,
            lora,
            percents,
            &self.config.contract,
        );
        let queued = self.queue(graph).await?;
        if queued.error.as_ref().is_some_and(|error| !error.is_null())
            || !queued.node_errors.is_empty()
            || !queued.number.is_finite()
            || queued.number < 0.0
        {
            self.abandon(&queued.prompt_id, false).await;
            return None;
        }
        let entry = self.wait(&queued.prompt_id, deadline).await?;
        let mean = report_mean(&entry);
        if mean.is_none() {
            self.abandon(&queued.prompt_id, true).await;
        }
        mean
    }

    async fn wait(&self, prompt: &Uuid, deadline: Instant) -> Option<Value> {
        loop {
            let entry = match self.history(prompt, deadline).await {
                Some(entry) => entry,
                None => {
                    self.abandon(prompt, false).await;
                    return None;
                }
            };
            let Some(entry) = entry else {
                tokio::time::sleep(Duration::from_millis(
                    self.config.poll_interval_milliseconds,
                ))
                .await;
                continue;
            };
            if failed(&entry) {
                self.abandon(prompt, true).await;
                return None;
            }
            if completed(&entry) {
                return Some(entry);
            }
            tokio::time::sleep(Duration::from_millis(
                self.config.poll_interval_milliseconds,
            ))
            .await;
        }
    }

    async fn stage(&self, name: &str, target: &Path, deadline: Instant) -> bool {
        if artifact(name, &self.stem, self.prefix()).is_none()
            || self.stage_remote(name, deadline).await.is_none()
        {
            return false;
        }
        let Some(bytes) = self.fetch(name).await else {
            return false;
        };
        crate::train::atomic_write(target, &bytes).is_ok()
    }

    async fn stage_remote(&self, name: &str, deadline: Instant) -> Option<()> {
        artifact(name, &self.stem, self.prefix())?;
        let queued = self
            .queue(json!({
                "1": {
                    "class_type": self.config.contract.stage_training_artifact_node,
                    "inputs": { "artifact": name }
                }
            }))
            .await?;
        if queued.error.as_ref().is_some_and(|error| !error.is_null())
            || !queued.node_errors.is_empty()
            || !queued.number.is_finite()
            || queued.number < 0.0
        {
            self.abandon(&queued.prompt_id, false).await;
            return None;
        }
        self.wait(&queued.prompt_id, deadline).await.map(|_| ())
    }

    async fn queue(&self, graph: Value) -> Option<PromptResponse> {
        let response = match self
            .authorize(self.client.post(format!("{}/prompt", self.config.base_url)))
            .json(&json!({ "prompt": graph }))
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => {
                self.cleanup.store(false, Ordering::Release);
                return None;
            }
        };
        let response = match response.error_for_status() {
            Ok(response) => response,
            Err(_) => {
                self.cleanup.store(false, Ordering::Release);
                return None;
            }
        };
        let response: Value = match response.json().await {
            Ok(response) => response,
            Err(_) => {
                self.cleanup.store(false, Ordering::Release);
                return None;
            }
        };
        match serde_json::from_value(response.clone()) {
            Ok(queued) => Some(queued),
            Err(_) => {
                if let Some(prompt) = response
                    .get("prompt_id")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                {
                    self.abandon(&prompt, false).await;
                } else {
                    self.cleanup.store(false, Ordering::Release);
                }
                None
            }
        }
    }

    async fn history(&self, prompt: &Uuid, deadline: Instant) -> Option<Option<Value>> {
        if Instant::now() >= deadline {
            return None;
        }
        let history: Value = self
            .authorize(
                self.client
                    .get(format!("{}/history/{prompt}", self.config.base_url))
                    .timeout(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .min(POLL_TIMEOUT),
                    ),
            )
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()?;
        let entries = history.as_object()?;
        if entries.is_empty() {
            return Some(None);
        }
        if entries.len() != 1 {
            return None;
        }
        entries.get(&prompt.to_string()).cloned().map(Some)
    }

    async fn abandon(&self, prompt: &Uuid, terminal: bool) {
        let _ = self
            .authorize(
                self.client
                    .post(format!("{}/api/jobs/{prompt}/cancel", self.config.base_url))
                    .timeout(POLL_TIMEOUT),
            )
            .send()
            .await;
        if terminal {
            return;
        }
        let deadline = Instant::now() + self.cancel_timeout;
        loop {
            if Instant::now() >= deadline {
                self.cleanup.store(false, Ordering::Release);
                return;
            }
            if let Some(Some(entry)) = self.history(prompt, deadline).await
                && terminal_history(&entry)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(
                self.config.poll_interval_milliseconds,
            ))
            .await;
        }
    }

    async fn fetch(&self, name: &str) -> Option<Vec<u8>> {
        artifact(name, &self.stem, self.prefix())?;
        let response = self
            .authorize(self.client.get(format!(
                "{}/view?filename={}&subfolder=loras&type=output",
                self.config.base_url,
                urlencoding::encode(name)
            )))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        let expected = format!("filename=\"{name}\"");
        if response
            .headers()
            .get(CONTENT_DISPOSITION)
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
            || !response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .is_some_and(crate::train::is_weight_payload)
        {
            return None;
        }
        let bytes = response.bytes().await.ok()?;
        (bytes.len() >= MIN_WEIGHT_BYTES).then(|| bytes.to_vec())
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        crate::http::authorize(self.config, request)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptResponse {
    prompt_id: Uuid,
    number: f64,
    #[serde(default)]
    error: Option<Value>,
    #[serde(default)]
    node_errors: Map<String, Value>,
}

fn failed(entry: &Value) -> bool {
    entry
        .pointer("/status/status_str")
        .and_then(Value::as_str)
        .is_some_and(|state| state.eq_ignore_ascii_case("error"))
}

fn completed(entry: &Value) -> bool {
    entry.pointer("/status/completed").and_then(Value::as_bool) == Some(true)
        || entry
            .pointer("/status/status_str")
            .and_then(Value::as_str)
            .is_some_and(|state| state.eq_ignore_ascii_case("success"))
}

fn terminal_history(entry: &Value) -> bool {
    completed(entry) || failed(entry)
}

/// The adapter loader carries a fresh node id on every probe because ComfyUI
/// caches one node instance per id and remembers the last weights it read.
pub(crate) fn graph(
    model: &TrainingModel,
    folder: &str,
    manifest: &str,
    resolution: u32,
    lora: Option<&str>,
    percents: &str,
    contract: &Contract,
) -> Value {
    match model {
        TrainingModel::Flux { checkpoint } => flux_graph(
            checkpoint, folder, manifest, resolution, lora, percents, contract,
        ),
        TrainingModel::QwenEdit { unet, clip, vae } => qwen_graph(
            unet, clip, vae, folder, manifest, resolution, lora, percents, contract,
        ),
    }
}

fn flux_graph(
    checkpoint: &str,
    folder: &str,
    manifest: &str,
    resolution: u32,
    lora: Option<&str>,
    percents: &str,
    contract: &Contract,
) -> Value {
    let mut nodes = json!({
        "1": { "class_type": "CheckpointLoaderSimple", "inputs": { "ckpt_name": checkpoint } },
        "2": {
            "class_type": contract.load_train_dataset_node,
            "inputs": { "folder": folder, "manifest_json": manifest, "resolution": resolution }
        },
        "3": { "class_type": "VAEEncode", "inputs": { "pixels": ["2", 0], "vae": ["1", 2] } },
        "4": { "class_type": "CLIPTextEncode", "inputs": { "text": ["2", 2], "clip": ["1", 1] } },
        "5": {
            "class_type": contract.probe_loss_node,
            "inputs": {
                "model": ["1", 0], "latents": ["3", 0], "positive": ["4", 0],
                "percents": percents, "seed": PROBE_SEED
            }
        },
        "6": { "class_type": "PreviewAny", "inputs": { "source": ["5", 0] } }
    });
    attach_lora(&mut nodes, "1", "5", lora);
    nodes
}

fn qwen_graph(
    unet: &str,
    clip: &str,
    vae: &str,
    folder: &str,
    manifest: &str,
    resolution: u32,
    lora: Option<&str>,
    percents: &str,
    contract: &Contract,
) -> Value {
    let mut nodes = json!({
        "1": { "class_type": "UNETLoader", "inputs": { "unet_name": unet, "weight_dtype": "default" } },
        "2": { "class_type": "CLIPLoader", "inputs": { "clip_name": clip, "type": "qwen_image" } },
        "3": { "class_type": "VAELoader", "inputs": { "vae_name": vae } },
        "4": {
            "class_type": contract.load_train_dataset_node,
            "inputs": { "folder": folder, "manifest_json": manifest, "resolution": resolution }
        },
        "5": { "class_type": "VAEEncode", "inputs": { "pixels": ["4", 0], "vae": ["3", 0] } },
        "6": {
            "class_type": "TextEncodeQwenImageEditPlus",
            "inputs": { "clip": ["2", 0], "prompt": ["4", 2], "vae": ["3", 0], "image1": ["4", 1] }
        },
        "7": {
            "class_type": contract.probe_loss_node,
            "inputs": {
                "model": ["1", 0], "latents": ["5", 0], "positive": ["6", 0],
                "percents": percents, "seed": PROBE_SEED
            }
        },
        "8": { "class_type": "PreviewAny", "inputs": { "source": ["7", 0] } }
    });
    attach_lora(&mut nodes, "1", "7", lora);
    nodes
}

fn attach_lora(nodes: &mut Value, model: &str, probe: &str, lora: Option<&str>) {
    if let Some(lora) = lora {
        let loader = format!("lora-{}", Uuid::new_v4());
        nodes[loader.as_str()] = json!({
            "class_type": "LoraLoaderModelOnly",
            "inputs": { "model": [model, 0], "lora_name": lora, "strength_model": 1.0 }
        });
        nodes[probe]["inputs"]["model"] = json!([loader, 0]);
    }
}

fn report_mean(entry: &Value) -> Option<f64> {
    entry
        .get("outputs")?
        .as_object()?
        .values()
        .filter_map(Value::as_object)
        .flat_map(|node| node.values())
        .filter_map(|value| match value {
            Value::Array(items) => items.first().and_then(Value::as_str),
            Value::String(text) => Some(text.as_str()),
            _ => None,
        })
        .filter_map(|text| serde_json::from_str::<Value>(text).ok())
        .find_map(|report| report.get("mean").and_then(Value::as_f64))
}

fn steps(images: usize) -> Option<u32> {
    Some(crate::train::packaged_config().ok()?.steps(images))
}

fn interval(settings: &Settings, steps: u32) -> u32 {
    if settings.checkpoint_every == 0 {
        return 0;
    }
    if settings.checkpoints_per_run < 1 {
        return settings.checkpoint_every;
    }
    settings
        .checkpoint_every
        .max(steps.div_ceil(settings.checkpoints_per_run))
}

fn artifact(name: &str, stem: &str, prefix: &str) -> Option<Option<u32>> {
    validate_uuid_name(stem, prefix)?;
    if name == format!("{stem}.safetensors") {
        return Some(None);
    }
    let step = name
        .strip_prefix(&format!("{stem}-step"))?
        .strip_suffix(".safetensors")?
        .parse::<u32>()
        .ok()?;
    (step > 0).then_some(Some(step))
}

fn validate_uuid_name(name: &str, prefix: &str) -> Option<()> {
    let id = name.strip_prefix(prefix)?;
    let uuid = Uuid::parse_str(id).ok()?;
    (uuid.get_version_num() == 4 && uuid.to_string() == id).then_some(())
}

fn destination(root: &Path, name: &str, stem: &str, prefix: &str) -> Option<PathBuf> {
    artifact(name, stem, prefix)?;
    let root = real_directory(root)?;
    let path = root.join(name);
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return None;
    }
    Some(path)
}

fn read_regular(path: &Path) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    fs::read(path).ok()
}

fn produced(config: &Config) -> Option<PathBuf> {
    let root = real_directory(config.models_directory.parent()?)?;
    let output = child_directory(&root, "output")?;
    child_directory(&output, "loras")
}

fn input(config: &Config) -> Option<PathBuf> {
    let root = real_directory(config.models_directory.parent()?)?;
    child_directory(&root, "input")
}

fn models_loras(config: &Config) -> Option<PathBuf> {
    let root = real_directory(config.models_directory.parent()?)?;
    let models = real_directory(&config.models_directory)?;
    if models.parent() != Some(root.as_path()) {
        return None;
    }
    child_directory(&models, "loras")
}

fn real_directory(path: &Path) -> Option<PathBuf> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return None;
    }
    path.canonicalize().ok()
}

fn child_directory(root: &Path, name: &str) -> Option<PathBuf> {
    if Path::new(name).components().count() != 1 {
        return None;
    }
    let root = real_directory(root)?;
    let child = real_directory(&root.join(name))?;
    (child.parent() == Some(root.as_path())).then_some(child)
}

fn subsample(
    config: &Config,
    model: &TrainingModel,
    folder: &str,
    captions: &HashMap<String, String>,
    limit: usize,
) -> Option<Sample> {
    let contract = &config.contract;
    validate_uuid_name(folder, &contract.folder_prefix)?;
    if captions.len() <= limit {
        return None;
    }
    let input = input(config)?;
    let source = child_directory(&input, folder)?;
    let name = format!("{}{}", contract.probe_prefix, Uuid::new_v4());
    if !crate::train::is_single_component(&name) {
        return None;
    }
    let destination = input.join(&name);

    if fs::symlink_metadata(&destination).is_ok() {
        return None;
    }
    fs::create_dir(&destination).ok()?;
    let destination = real_directory(&destination)?;
    if destination.parent() != Some(input.as_path()) {
        return None;
    }
    let result = (|| {
        let mut selected = HashMap::new();
        for directory in ["targets", "control_1"] {
            if directory == "control_1" && matches!(model, TrainingModel::Flux { .. }) {
                continue;
            }
            let source_directory = child_directory(&source, directory)?;
            let target_directory = destination.join(directory);
            fs::create_dir(&target_directory).ok()?;
            let target_directory = real_directory(&target_directory)?;
            if target_directory.parent() != Some(destination.as_path()) {
                return None;
            }
            for index in 0..limit {
                let filename = format!("{index:04}.png");
                let source_file = source_directory.join(&filename);
                let bytes = read_regular(&source_file)?;
                write_new(&target_directory.join(&filename), &bytes)?;
                if directory == "targets" {
                    selected.insert(filename.clone(), captions.get(&filename)?.clone());
                }
            }
        }
        Some(Sample {
            folder: name.clone(),
            manifest: crate::train::manifest(model, &selected)?,
        })
    })();
    if result.is_none() {
        remove_namespace(&input, &name, &contract.probe_prefix);
    }
    result
}

fn write_new(path: &Path, bytes: &[u8]) -> Option<()> {
    use std::io::Write;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .ok()?;
    file.write_all(bytes).and_then(|()| file.sync_all()).ok()
}

fn discard(config: &Config, sample: Option<&Sample>) {
    let Some(sample) = sample else { return };
    let Some(input) = input(config) else { return };
    remove_namespace(&input, &sample.folder, &config.contract.probe_prefix);
}

fn remove_namespace(root: &Path, name: &str, prefix: &str) {
    if validate_uuid_name(name, prefix).is_none() {
        return;
    }
    let Some(root) = real_directory(root) else {
        return;
    };
    let path = root.join(name);
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return;
    }
    let Ok(path) = path.canonicalize() else {
        return;
    };
    if path.parent() == Some(root.as_path()) {
        let _ = fs::remove_dir_all(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::train::{ARTIFACT_PREFIX, FOLDER_PREFIX, PROBE_PREFIX};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    fn flux() -> TrainingModel {
        TrainingModel::Flux {
            checkpoint: "flux.safetensors".into(),
        }
    }

    fn qwen() -> TrainingModel {
        TrainingModel::QwenEdit {
            unet: "qwen-unet.safetensors".into(),
            clip: "qwen-clip.safetensors".into(),
            vae: "qwen-vae.safetensors".into(),
        }
    }

    fn run() -> Run {
        let id = Uuid::new_v4();
        Run {
            folder: format!("{FOLDER_PREFIX}{id}"),
            artifact: format!("{ARTIFACT_PREFIX}{id}"),
        }
    }

    fn probe<'a>(config: &'a Config, model: &'a TrainingModel, run: &Run) -> Probe<'a> {
        Probe {
            config,
            model,
            client: crate::http::client(config).unwrap(),
            manifest: "{}".into(),
            resolution: 512,
            stem: run.artifact.clone(),
            cleanup: AtomicBool::new(true),
            cancel_timeout: Duration::from_millis(30),
        }
    }

    #[test]
    fn probe_graphs_use_the_explicit_model_family_and_native_encoders() {
        let contract = Contract::default();
        let flux = graph(&flux(), "folder", "{}", 512, None, RANK_PERCENT, &contract);
        let qwen = graph(&qwen(), "folder", "{}", 512, None, RANK_PERCENT, &contract);
        assert_eq!(flux["2"]["class_type"], contract.load_train_dataset_node);
        assert_eq!(flux["3"]["class_type"], "VAEEncode");
        assert_eq!(flux["5"]["class_type"], contract.probe_loss_node);
        assert_eq!(qwen["4"]["class_type"], contract.load_train_dataset_node);
        assert_eq!(qwen["7"]["class_type"], contract.probe_loss_node);
        assert_eq!(flux["1"]["class_type"], "CheckpointLoaderSimple");

        assert_eq!(flux["5"]["inputs"]["positive"], json!(["4", 0]));
        assert_eq!(qwen["1"]["class_type"], "UNETLoader");
        assert_eq!(qwen["2"]["inputs"]["type"], "qwen_image");
        assert_eq!(qwen["6"]["class_type"], "TextEncodeQwenImageEditPlus");
        assert_eq!(qwen["5"]["inputs"]["pixels"], json!(["4", 0]));
        assert_eq!(qwen["6"]["inputs"]["image1"], json!(["4", 1]));
        assert_eq!(qwen["6"]["inputs"]["prompt"], json!(["4", 2]));
        assert_eq!(qwen["7"]["inputs"]["positive"], json!(["6", 0]));
    }

    #[test]
    fn adapter_names_are_bound_to_the_run_uuid() {
        let run = run();
        assert_eq!(
            artifact(
                &format!("{}.safetensors", run.artifact),
                &run.artifact,
                ARTIFACT_PREFIX
            ),
            Some(None)
        );
        assert_eq!(
            artifact(
                &format!("{}-step42.safetensors", run.artifact),
                &run.artifact,
                ARTIFACT_PREFIX
            ),
            Some(Some(42))
        );
        assert_eq!(
            artifact("../../victim", &run.artifact, ARTIFACT_PREFIX),
            None
        );
        assert_eq!(
            artifact("other-step42.safetensors", &run.artifact, ARTIFACT_PREFIX),
            None
        );
        assert_eq!(
            artifact(
                &format!("{}.safetensors", run.artifact),
                &run.artifact,
                "adapter-"
            ),
            None,
            "a stem outside the configured namespace names nothing"
        );
    }

    #[test]
    fn probe_graphs_address_a_renamed_node_pack() {
        let contract = Contract {
            load_train_dataset_node: "LoadDataset".into(),
            probe_loss_node: "MeasureLoss".into(),
            ..Contract::default()
        };
        let flux = graph(&flux(), "folder", "{}", 512, None, RANK_PERCENT, &contract);
        let qwen = graph(&qwen(), "folder", "{}", 512, None, RANK_PERCENT, &contract);
        assert_eq!(flux["2"]["class_type"], "LoadDataset");
        assert_eq!(flux["5"]["class_type"], "MeasureLoss");
        assert_eq!(qwen["4"]["class_type"], "LoadDataset");
        assert_eq!(qwen["7"]["class_type"], "MeasureLoss");
    }

    #[test]
    fn quality_serializes_an_explicit_calibration_contract() {
        let flux = serde_json::to_value(Quality {
            improvement: 0.34,
            checkpoint: "step400".into(),
            measured: true,
            calibration: QualityCalibration::FluxHealthBands,
        })
        .unwrap();
        let qwen = serde_json::to_value(Quality {
            improvement: 0.12,
            checkpoint: FINAL.into(),
            measured: true,
            calibration: QualityCalibration::Uncalibrated,
        })
        .unwrap();
        assert_eq!(flux["calibration"], "flux_health_bands");
        assert_eq!(qwen["calibration"], "uncalibrated");
        assert_eq!(flux["measured"], true);
    }

    #[test]
    fn checkpoint_interval_is_bounded_by_the_packaged_budget() {
        let settings: Settings = serde_json::from_str(PACKAGED_TRAIN_CONFIG).unwrap();
        for total in [300, 401, 800, steps(usize::MAX).unwrap()] {
            let gap = interval(&settings, total);
            assert!(gap >= settings.checkpoint_every);
            assert!(total.div_ceil(gap) <= settings.checkpoints_per_run);
        }
    }

    #[test]
    fn checkpoint_discovery_ignores_symlinks_and_other_namespaces() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = tempfile::tempdir().unwrap();
            let models = root.path().join("models");
            let output = root.path().join("output/loras");
            fs::create_dir_all(models.join("loras")).unwrap();
            fs::create_dir_all(&output).unwrap();
            let run = run();
            fs::write(
                output.join(format!("{}-step12.safetensors", run.artifact)),
                b"weights",
            )
            .unwrap();
            fs::write(output.join("victim"), b"safe").unwrap();
            symlink(
                output.join("victim"),
                output.join(format!("{}-step13.safetensors", run.artifact)),
            )
            .unwrap();
            let foreign = format!("{ARTIFACT_PREFIX}other-step99.safetensors");
            fs::write(output.join(&foreign), b"other").unwrap();
            let config = Config {
                models_directory: models,
                ..Default::default()
            };
            let model = flux();
            let selection = Selection::new(
                &crate::http::client(&config).unwrap(),
                &config,
                &model,
                &run,
                &root.path().join("attempt/adapter.safetensors"),
                &HashMap::from([("0000.png".into(), "portrait".into())]),
            )
            .unwrap();
            assert_eq!(selection.written(), Some(vec![12]));
            selection.sweep();
            assert!(output.join("victim").is_file());
            assert!(
                output
                    .join(format!("{}-step13.safetensors", run.artifact))
                    .is_symlink()
            );
            assert!(output.join(&foreign).is_file());
        }
    }

    #[test]
    fn staging_destination_refuses_a_symlink() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = tempfile::tempdir().unwrap();
            let run = run();
            let name = format!("{}-step12.safetensors", run.artifact);
            let victim = root.path().join("victim");
            fs::write(&victim, b"safe").unwrap();
            symlink(&victim, root.path().join(&name)).unwrap();
            assert!(destination(root.path(), &name, &run.artifact, ARTIFACT_PREFIX).is_none());
            assert_eq!(fs::read(victim).unwrap(), b"safe");
        }
    }

    #[test]
    fn sample_cleanup_refuses_a_symlinked_namespace() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = tempfile::tempdir().unwrap();
            let input = root.path().join("input");
            let models = root.path().join("models");
            let victim = root.path().join("victim");
            fs::create_dir_all(&input).unwrap();
            fs::create_dir_all(&models).unwrap();
            fs::create_dir_all(&victim).unwrap();
            fs::write(victim.join("kept"), b"safe").unwrap();
            let sample = Sample {
                folder: format!("{PROBE_PREFIX}{}", Uuid::new_v4()),
                manifest: "{}".into(),
            };
            symlink(&victim, input.join(&sample.folder)).unwrap();
            discard(
                &Config {
                    models_directory: models,
                    ..Default::default()
                },
                Some(&sample),
            );
            assert_eq!(fs::read(victim.join("kept")).unwrap(), b"safe");
            assert!(input.join(sample.folder).is_symlink());
        }
    }

    #[cfg(unix)]
    #[test]
    fn probe_paths_reject_symlinked_root_and_intermediate_directories() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let actual = root.path().join("actual");
        fs::create_dir_all(actual.join("models/loras")).unwrap();
        fs::create_dir(actual.join("input")).unwrap();
        let linked = root.path().join("comfy");
        symlink(&actual, &linked).unwrap();
        let linked_config = Config {
            models_directory: linked.join("models"),
            ..Default::default()
        };
        assert!(input(&linked_config).is_none());
        assert!(produced(&linked_config).is_none());
        assert!(models_loras(&linked_config).is_none());

        let comfy = root.path().join("safe");
        let outside = root.path().join("outside");
        fs::create_dir_all(comfy.join("models/loras")).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, comfy.join("input")).unwrap();
        fs::create_dir(comfy.join("output")).unwrap();
        symlink(&outside, comfy.join("output/loras")).unwrap();
        let config = Config {
            models_directory: comfy.join("models"),
            ..Default::default()
        };
        assert!(input(&config).is_none());
        assert!(produced(&config).is_none());
    }

    #[test]
    fn a_sample_is_never_staged_outside_the_input_directory() {
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        let input = root.path().join("input");
        fs::create_dir(&models).unwrap();
        fs::create_dir(&input).unwrap();
        let run = run();
        let source = input.join(&run.folder).join("targets");
        fs::create_dir_all(&source).unwrap();
        for index in 0..5u8 {
            fs::write(source.join(format!("{index:04}.png")), [index]).unwrap();
        }
        let captions = (0..5)
            .map(|index| (format!("{index:04}.png"), format!("instruction {index}")))
            .collect();
        let config = Config {
            models_directory: models,
            contract: Contract {
                probe_prefix: "../outside-".into(),
                ..Contract::default()
            },
            ..Default::default()
        };
        assert!(subsample(&config, &flux(), &run.folder, &captions, 4).is_none());
        let stray: Vec<_> = fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("outside-"))
            .collect();
        assert!(stray.is_empty(), "a sample was staged at {stray:?}");
    }

    #[cfg(unix)]
    #[test]
    fn subsample_rejects_a_symlinked_training_image_directory() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        let input = root.path().join("input");
        let victim = root.path().join("victim");
        fs::create_dir(&models).unwrap();
        fs::create_dir(&input).unwrap();
        fs::create_dir(&victim).unwrap();
        let run = run();
        let source = input.join(&run.folder);
        fs::create_dir(&source).unwrap();
        for index in 0..5 {
            fs::write(victim.join(format!("{index:04}.png")), [index]).unwrap();
        }
        symlink(&victim, source.join("targets")).unwrap();
        let captions = (0..5)
            .map(|index| (format!("{index:04}.png"), format!("instruction {index}")))
            .collect();
        let config = Config {
            models_directory: models,
            ..Default::default()
        };
        assert!(subsample(&config, &flux(), &run.folder, &captions, 4).is_none());
        assert_eq!(fs::read_dir(&input).unwrap().count(), 1);
        assert_eq!(fs::read(victim.join("0000.png")).unwrap(), [0]);
    }

    /// `fetch` had no test at all, so the header it demands was never compared
    /// with the header ComfyUI sends. Requiring `application/octet-stream` is
    /// what refused every trained adapter in `train::download`; the same check
    /// guards every candidate checkpoint here.
    #[tokio::test]
    async fn a_candidate_checkpoint_is_fetched_from_the_header_comfyui_sends() {
        for served in ["application/safetensors", "application/octet-stream"] {
            let server = MockServer::start().await;
            let run = run();
            let name = format!("{}.safetensors", run.artifact);
            Mock::given(method("GET"))
                .and(path("/view"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header(
                            "content-disposition",
                            format!("filename=\"{name}\"").as_str(),
                        )
                        .insert_header("content-type", served)
                        .set_body_bytes(vec![3u8; MIN_WEIGHT_BYTES + 1]),
                )
                .mount(&server)
                .await;
            let config = Config {
                base_url: server.uri(),
                poll_interval_milliseconds: 1,
                ..Default::default()
            };
            let model = flux();

            let bytes = probe(&config, &model, &run).fetch(&name).await;

            assert_eq!(
                bytes.map(|bytes| bytes.len()),
                Some(MIN_WEIGHT_BYTES + 1),
                "{served} must be fetched"
            );
        }
    }

    #[tokio::test]
    async fn a_candidate_checkpoint_that_is_an_error_page_is_refused() {
        let server = MockServer::start().await;
        let run = run();
        let name = format!("{}.safetensors", run.artifact);
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header(
                        "content-disposition",
                        format!("filename=\"{name}\"").as_str(),
                    )
                    .insert_header("content-type", "text/html")
                    .set_body_bytes(vec![3u8; MIN_WEIGHT_BYTES + 1]),
            )
            .mount(&server)
            .await;
        let config = Config {
            base_url: server.uri(),
            poll_interval_milliseconds: 1,
            ..Default::default()
        };
        let model = flux();

        assert!(
            probe(&config, &model, &run).fetch(&name).await.is_none(),
            "an HTML body must not be taken for a checkpoint"
        );
    }

    #[tokio::test]
    async fn malformed_probe_history_cancels_exact_job_and_waits_for_terminal_history() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        let unrelated = Uuid::new_v4();
        let calls = Arc::new(AtomicUsize::new(0));
        let sequence = calls.clone();
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(move |_request: &Request| {
                if sequence.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
                    ResponseTemplate::new(200).set_body_json(json!({ unrelated.to_string(): {} }))
                } else {
                    ResponseTemplate::new(200).set_body_json(json!({
                        prompt.to_string(): {"status": {"status_str": "error"}}
                    }))
                }
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/jobs/{prompt}/cancel")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"cancelled": true})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "prompt_id": prompt,
                "number": 0,
                "node_errors": {}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let config = Config {
            base_url: server.uri(),
            poll_interval_milliseconds: 1,
            ..Default::default()
        };
        let model = flux();
        let run = run();
        let probe = probe(&config, &model, &run);
        assert!(
            probe
                .mean(
                    "folder",
                    "{}",
                    None,
                    RANK_PERCENT,
                    Instant::now() + Duration::from_secs(1)
                )
                .await
                .is_none()
        );
        assert!(probe.cleanup.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn malformed_probe_queue_response_cancels_its_recoverable_prompt() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "prompt_id": prompt,
                "number": "not-a-number",
                "node_errors": {}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/jobs/{prompt}/cancel")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"cancelled": true})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                prompt.to_string(): {"status": {"status_str": "error"}}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let config = Config {
            base_url: server.uri(),
            poll_interval_milliseconds: 1,
            ..Default::default()
        };
        let model = flux();
        let run = run();
        let probe = probe(&config, &model, &run);
        assert!(probe.queue(json!({})).await.is_none());
        assert!(probe.cleanup.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn unverified_cancel_failure_marks_probe_cleanup_unsafe() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/jobs/{prompt}/cancel")))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let config = Config {
            base_url: server.uri(),
            poll_interval_milliseconds: 1,
            ..Default::default()
        };
        let model = flux();
        let run = run();
        let probe = probe(&config, &model, &run);
        probe.abandon(&prompt, false).await;
        assert!(!probe.cleanup.load(Ordering::Acquire));
        let expected = format!("/api/jobs/{prompt}/cancel");
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|request| request.url.path().contains("/api/jobs/"))
                .all(|request| request.url.path() == expected)
        );
    }

    #[tokio::test]
    async fn terminal_probe_error_still_sends_targeted_cancel() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                prompt.to_string(): {"status": {"status_str": "error"}}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/jobs/{prompt}/cancel")))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let config = Config {
            base_url: server.uri(),
            poll_interval_milliseconds: 1,
            ..Default::default()
        };
        let model = flux();
        let run = run();
        let probe = probe(&config, &model, &run);
        assert!(
            probe
                .wait(&prompt, Instant::now() + Duration::from_secs(1))
                .await
                .is_none()
        );
        assert!(probe.cleanup.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn abandoned_stage_job_uses_the_same_targeted_cancellation_gate() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "prompt_id": prompt,
                "number": 0,
                "node_errors": {}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!("/api/jobs/{prompt}/cancel")))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        let config = Config {
            base_url: server.uri(),
            poll_interval_milliseconds: 1,
            ..Default::default()
        };
        let model = flux();
        let run = run();
        let probe = probe(&config, &model, &run);
        assert!(
            probe
                .stage_remote(&format!("{}.safetensors", run.artifact), Instant::now(),)
                .await
                .is_none()
        );
        assert!(!probe.cleanup.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn staging_asks_a_renamed_node_pack_for_its_own_node() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("StageWeights"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "prompt_id": prompt,
                "number": 0,
                "node_errors": {}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                prompt.to_string(): {"status": {"completed": true, "status_str": "success"}}
            })))
            .mount(&server)
            .await;
        let config = Config {
            base_url: server.uri(),
            poll_interval_milliseconds: 1,
            contract: Contract {
                stage_training_artifact_node: "StageWeights".into(),
                artifact_prefix: "adapter-".into(),
                ..Contract::default()
            },
            ..Default::default()
        };
        let model = flux();
        let run = Run::new(&config.contract);
        let probe = probe(&config, &model, &run);

        assert!(
            probe
                .stage_remote(
                    &format!("{}.safetensors", run.artifact),
                    Instant::now() + Duration::from_secs(5)
                )
                .await
                .is_some(),
            "an adapter named under the configured namespace must stage"
        );
    }

    #[test]
    fn qwen_sample_preserves_zipped_target_reference_instructions() {
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        fs::create_dir_all(&models).unwrap();
        fs::create_dir_all(root.path().join("input")).unwrap();
        let run = run();
        let source = root.path().join("input").join(&run.folder);
        fs::create_dir_all(source.join("targets")).unwrap();
        fs::create_dir_all(source.join("control_1")).unwrap();
        let captions: HashMap<String, String> = (0..6)
            .map(|index| (format!("{index:04}.png"), format!("instruction {index}")))
            .collect();
        for index in 0..6 {
            fs::write(source.join(format!("targets/{index:04}.png")), [index]).unwrap();
            fs::write(
                source.join(format!("control_1/{index:04}.png")),
                [index + 10],
            )
            .unwrap();
        }
        let config = Config {
            models_directory: models,
            ..Default::default()
        };
        let sample = subsample(&config, &qwen(), &run.folder, &captions, 4).unwrap();
        let manifest: Value = serde_json::from_str(&sample.manifest).unwrap();
        assert_eq!(manifest["pairs"].as_array().unwrap().len(), 4);
        for index in 0..4 {
            assert_eq!(manifest["pairs"][index]["index"], index);
            assert_eq!(
                manifest["pairs"][index]["target"],
                format!("targets/{index:04}.png")
            );
            assert_eq!(
                manifest["pairs"][index]["reference"],
                format!("control_1/{index:04}.png")
            );
            assert_eq!(
                manifest["pairs"][index]["instruction"],
                format!("instruction {index}")
            );
        }
        discard(&config, Some(&sample));
        assert!(!root.path().join("input").join(sample.folder).exists());
    }
}
