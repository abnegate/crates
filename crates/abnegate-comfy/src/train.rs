//! HTTP client that runs packaged LoRA training graphs on ComfyUI.

mod contract;

pub use contract::Contract;

use crate::config::Config;
use crate::excerpt;
use crate::http::CANCEL_TIMEOUT;
use crate::http::POLL_TIMEOUT;
use crate::http::authorize;
use crate::lora::TrainError;
use crate::recipe::TrainingModel;
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use std::time::Duration;
use uuid::Uuid;

pub(crate) const PACKAGED_TRAIN_CONFIG: &str = include_str!("../comfyui/train_config.json");
/// Smallest body taken for trained weights. ComfyUI answers a missing file
/// with a 200 error page, so size is what tells the two apart.
pub(crate) const MIN_WEIGHT_BYTES: usize = 10_000;
const MANIFEST_VERSION: u32 = 1;
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(30);
const TRAIN_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Default `class_type` of the node that trains the adapter.
pub const TRAIN_LORA_NODE: &str = "ZoneTrainLoRA";
/// Default `class_type` of the node that deletes a run's dataset and weights.
pub const CLEANUP_TRAINING_RUN_NODE: &str = "ZoneCleanupTrainingRun";
/// Default `class_type` of the node that loads a staged dataset and its manifest.
pub const LOAD_TRAIN_DATASET_NODE: &str = "ZoneLoadTrainDataset";
/// Default `class_type` of the node that measures a model's loss on a dataset.
pub const PROBE_LOSS_NODE: &str = "ZoneProbeLoss";
/// Default `class_type` of the node that moves a trained checkpoint to where a
/// LoRA loader finds it.
pub const STAGE_TRAINING_ARTIFACT_NODE: &str = "ZoneStageTrainingArtifact";
/// Default prefix of the variables handed to an external training command.
pub const ENVIRONMENT_PREFIX: &str = "ZONE_TRAIN";
/// Default prefix of the variable naming ComfyUI's input directory for an
/// external training command.
pub const INPUT_ENVIRONMENT_PREFIX: &str = "ZONE_COMFY";
/// Default namespace of the input folder a run stages its dataset in.
pub const FOLDER_PREFIX: &str = "zone-train-";
/// Default namespace of the weights a run writes.
pub const ARTIFACT_PREFIX: &str = "zone-lora-";
/// Default namespace of the input folder a quality probe stages its sample in.
pub const PROBE_PREFIX: &str = "zone-probe-";

#[derive(Debug, Deserialize)]
pub struct TrainConfig {
    passes_per_image: u32,
    min_steps: u32,
    max_steps: u32,
    rank: u32,
    learning_rate: f64,
    lora_dtype: String,
    training_dtype: String,
    resolution: u32,
    bypass_mode: bool,
    gradient_checkpointing: bool,
    checkpoint_depth: u32,
    seed: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub folder: String,
    pub artifact: String,
}

impl Run {
    /// A fresh pair of names under `contract`'s namespaces.
    pub fn new(contract: &Contract) -> Self {
        Self {
            folder: format!("{}{}", contract.folder_prefix, Uuid::new_v4()),
            artifact: format!("{}{}", contract.artifact_prefix, Uuid::new_v4()),
        }
    }

    /// Refuses names that are not a v4 UUID under `contract`'s namespaces,
    /// and any name at all under a contract that fails [`Contract::validate`].
    pub fn validate(&self, contract: &Contract) -> Result<(), TrainError> {
        contract.validate()?;
        validate_run_name(&self.folder, &contract.folder_prefix)?;
        validate_run_name(&self.artifact, &contract.artifact_prefix)
    }
}

impl Default for Run {
    fn default() -> Self {
        Self::new(&Contract::default())
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UploadResponse {
    name: String,
    subfolder: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Default, Deserialize)]
struct HistoryStatus {
    #[serde(default)]
    status_str: String,
    #[serde(default)]
    completed: Option<bool>,
}

#[derive(Debug, Serialize)]
struct Manifest {
    schema_version: u32,
    architecture: &'static str,
    pairs: Vec<Pair>,
}

#[derive(Debug, Serialize)]
struct Pair {
    index: usize,
    target: String,
    reference: Option<String>,
    instruction: String,
}

#[derive(Debug)]
struct Failure {
    error: TrainError,
    cleanup: bool,
}

#[derive(Debug)]
struct WaitFailure {
    error: TrainError,
    cleanup: bool,
}

impl From<TrainError> for Failure {
    fn from(error: TrainError) -> Self {
        Self {
            error,
            cleanup: true,
        }
    }
}

pub fn packaged_config() -> Result<TrainConfig, TrainError> {
    serde_json::from_str(PACKAGED_TRAIN_CONFIG)
        .map_err(|error| TrainError::Failed(format!("train config: {error}")))
}

impl TrainConfig {
    /// Side of the square every training image is read back at.
    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// Steps for a dataset of this size, clamped to the configured bounds.
    pub fn steps(&self, image_count: usize) -> u32 {
        u32::try_from(image_count.max(1))
            .unwrap_or(u32::MAX)
            .saturating_mul(self.passes_per_image)
            .clamp(self.min_steps, self.max_steps)
    }
}

pub async fn run(
    config: &Config,
    model: &TrainingModel,
    work: &Path,
    output: &Path,
    image_count: usize,
) -> Result<Run, TrainError> {
    if !config.enabled {
        return Err(TrainError::Disabled);
    }
    let client = crate::http::client(config).map_err(request_failed)?;
    run_with(&client, config, model, work, output, image_count).await
}

/// [`run`] on a client the caller already holds for this run.
pub(crate) async fn run_with(
    client: &reqwest::Client,
    config: &Config,
    model: &TrainingModel,
    work: &Path,
    output: &Path,
    image_count: usize,
) -> Result<Run, TrainError> {
    if !config.enabled {
        return Err(TrainError::Disabled);
    }
    config.validate()?;
    let run = Run::new(&config.contract);
    match execute(client, config, model, work, output, image_count, &run).await {
        Ok(()) => Ok(run),
        Err(failure) => {
            if failure.cleanup {
                cleanup_with(client, config, &run).await;
            }
            Err(failure.error)
        }
    }
}

fn request_failed(error: reqwest::Error) -> TrainError {
    TrainError::Failed(error.to_string())
}

async fn execute(
    client: &reqwest::Client,
    config: &Config,
    model: &TrainingModel,
    work: &Path,
    output: &Path,
    image_count: usize,
    run: &Run,
) -> Result<(), Failure> {
    run.validate(&config.contract)?;
    let settings = packaged_config()?;
    let manifest = stage_or_upload(client, config, model, work, run).await?;
    let graph = train_graph(
        model,
        &run.folder,
        &manifest,
        &run.artifact,
        &settings,
        settings.steps(image_count),
        &config.contract,
    );
    let prompt = queue(client, config, graph)
        .await
        .map_err(|failure| Failure {
            error: failure.error,
            cleanup: failure.cleanup,
        })?;
    if let Err(failure) = wait_prompt(
        client,
        config,
        prompt,
        Duration::from_secs(config.train_timeout_seconds),
    )
    .await
    {
        return Err(Failure {
            error: failure.error,
            cleanup: failure.cleanup,
        });
    }
    download(client, config, run, output)
        .await
        .map_err(Failure::from)
}

async fn queue(
    client: &reqwest::Client,
    config: &Config,
    graph: Value,
) -> Result<Uuid, WaitFailure> {
    let response: Value = authorize(
        config,
        client
            .post(format!("{}/prompt", config.base_url))
            .json(&json!({ "prompt": graph })),
    )
    .send()
    .await
    .map_err(|error| WaitFailure {
        error: TrainError::Failed(error.to_string()),
        cleanup: false,
    })?
    .error_for_status()
    .map_err(|error| WaitFailure {
        error: TrainError::Failed(error.to_string()),
        cleanup: false,
    })?
    .json()
    .await
    .map_err(|error| WaitFailure {
        error: TrainError::Failed(format!("invalid ComfyUI prompt response: {error}")),
        cleanup: false,
    })?;
    let response: PromptResponse = match serde_json::from_value(response.clone()) {
        Ok(response) => response,
        Err(error) => {
            let cleanup = if let Some(prompt) = response
                .get("prompt_id")
                .and_then(Value::as_str)
                .and_then(|value| Uuid::parse_str(value).ok())
            {
                cancel_and_wait(client, config, prompt, false).await
            } else {
                false
            };
            return Err(WaitFailure {
                error: TrainError::Failed(format!(
                    "invalid ComfyUI prompt response: {}",
                    excerpt::head(&error.to_string())
                )),
                cleanup,
            });
        }
    };
    if response
        .error
        .as_ref()
        .is_some_and(|error| !error.is_null())
        || !response.node_errors.is_empty()
    {
        let prompt = response.prompt_id;
        let detail = response
            .error
            .unwrap_or_else(|| json!(response.node_errors));
        let cleanup = cancel_and_wait(client, config, prompt, false).await;
        return Err(WaitFailure {
            error: TrainError::Failed(format!(
                "ComfyUI rejected train graph: {}",
                excerpt::head(&detail.to_string())
            )),
            cleanup,
        });
    }
    if !response.number.is_finite() || response.number < 0.0 {
        let cleanup = cancel_and_wait(client, config, response.prompt_id, false).await;
        return Err(WaitFailure {
            error: TrainError::Failed("ComfyUI returned an invalid queue number".into()),
            cleanup,
        });
    }
    Ok(response.prompt_id)
}

async fn download(
    client: &reqwest::Client,
    config: &Config,
    run: &Run,
    output: &Path,
) -> Result<(), TrainError> {
    let filename = format!("{}.safetensors", run.artifact);
    let response = authorize(
        config,
        client.get(format!(
            "{}/view?filename={}&subfolder=loras&type=output",
            config.base_url,
            urlencoding::encode(&filename)
        )),
    )
    .send()
    .await
    .map_err(|error| TrainError::Failed(error.to_string()))?
    .error_for_status()
    .map_err(|error| TrainError::Failed(error.to_string()))?;
    let disposition = response
        .headers()
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if disposition != format!("filename=\"{filename}\"") {
        return Err(TrainError::Failed(
            "ComfyUI view response named a different artifact".into(),
        ));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !is_weight_payload(content_type) {
        return Err(TrainError::Failed(format!(
            "ComfyUI view response is not safetensors data: {}",
            excerpt::head(content_type)
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| TrainError::Failed(error.to_string()))?;
    if bytes.len() < MIN_WEIGHT_BYTES {
        return Err(TrainError::Failed(
            "ComfyUI returned a LoRA that is too small to be trained weights".into(),
        ));
    }
    atomic_write(output, &bytes)
}

/// Whether a `/view` response body is weights rather than an error page.
///
/// The pinned ComfyUI serves a `.safetensors` artifact as
/// `application/safetensors`; older builds served the generic
/// `application/octet-stream`. Accepting only the latter rejected every
/// trained adapter at the download step, and the run was then swept by the
/// error path, so a completed training run produced nothing. The check still
/// has to be narrow: its job is to refuse an HTML or JSON error page before it
/// is written out as weights.
pub(crate) fn is_weight_payload(content_type: &str) -> bool {
    matches!(
        content_type
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "application/octet-stream" | "application/safetensors"
    )
}

pub(crate) fn atomic_write(output: &Path, bytes: &[u8]) -> Result<(), TrainError> {
    let parent = output
        .parent()
        .ok_or_else(|| TrainError::Failed("training output has no parent".into()))?;
    fs::create_dir_all(parent).map_err(|error| TrainError::Failed(error.to_string()))?;
    if fs::symlink_metadata(output).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(TrainError::Failed(
            "training output cannot be a symlink".into(),
        ));
    }
    let name = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| TrainError::Failed("training output filename is invalid".into()))?;
    let temporary = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| TrainError::Failed(error.to_string()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| TrainError::Failed(error.to_string()))?;
        fs::rename(&temporary, output).map_err(|error| TrainError::Failed(error.to_string()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn train_graph(
    model: &TrainingModel,
    folder: &str,
    manifest: &str,
    artifact: &str,
    settings: &TrainConfig,
    steps: u32,
    contract: &Contract,
) -> Value {
    match model {
        TrainingModel::Flux { checkpoint } => flux_graph(
            checkpoint, folder, manifest, artifact, settings, steps, contract,
        ),
        TrainingModel::QwenEdit { unet, clip, vae } => qwen_graph(
            unet, clip, vae, folder, manifest, artifact, settings, steps, contract,
        ),
    }
}

fn trainer(
    model: Value,
    latents: Value,
    positive: Value,
    artifact: &str,
    settings: &TrainConfig,
    steps: u32,
    contract: &Contract,
) -> Value {
    json!({
        "class_type": contract.train_lora_node,
        "inputs": {
            "model": model,
            "latents": latents,
            "positive": positive,
            "steps": steps,
            "learning_rate": settings.learning_rate,
            "rank": settings.rank,
            "seed": settings.seed,
            "training_dtype": settings.training_dtype,
            "lora_dtype": settings.lora_dtype,
            "gradient_checkpointing": settings.gradient_checkpointing,
            "checkpoint_depth": settings.checkpoint_depth,
            "bypass_mode": settings.bypass_mode,
            "save_name": artifact
        }
    })
}

fn flux_graph(
    checkpoint: &str,
    folder: &str,
    manifest: &str,
    artifact: &str,
    settings: &TrainConfig,
    steps: u32,
    contract: &Contract,
) -> Value {
    let mut graph = json!({
        "1": {
            "class_type": "CheckpointLoaderSimple",
            "inputs": { "ckpt_name": checkpoint }
        },
        "2": {
            "class_type": contract.load_train_dataset_node,
            "inputs": {
                "folder": folder,
                "manifest_json": manifest,
                "resolution": settings.resolution
            }
        },
        "3": {
            "class_type": "VAEEncode",
            "inputs": { "pixels": ["2", 0], "vae": ["1", 2] }
        },
        "4": {
            "class_type": "CLIPTextEncode",
            "inputs": { "text": ["2", 2], "clip": ["1", 1] }
        }
    });
    graph["5"] = trainer(
        json!(["1", 0]),
        json!(["3", 0]),
        json!(["4", 0]),
        artifact,
        settings,
        steps,
        contract,
    );
    graph
}

fn qwen_graph(
    unet: &str,
    clip: &str,
    vae: &str,
    folder: &str,
    manifest: &str,
    artifact: &str,
    settings: &TrainConfig,
    steps: u32,
    contract: &Contract,
) -> Value {
    let mut graph = json!({
        "1": {
            "class_type": "UNETLoader",
            "inputs": { "unet_name": unet, "weight_dtype": "default" }
        },
        "2": {
            "class_type": "CLIPLoader",
            "inputs": { "clip_name": clip, "type": "qwen_image" }
        },
        "3": {
            "class_type": "VAELoader",
            "inputs": { "vae_name": vae }
        },
        "4": {
            "class_type": contract.load_train_dataset_node,
            "inputs": {
                "folder": folder,
                "manifest_json": manifest,
                "resolution": settings.resolution
            }
        },
        "5": {
            "class_type": "VAEEncode",
            "inputs": { "pixels": ["4", 0], "vae": ["3", 0] }
        },
        "6": {
            "class_type": "TextEncodeQwenImageEditPlus",
            "inputs": {
                "clip": ["2", 0],
                "prompt": ["4", 2],
                "vae": ["3", 0],
                "image1": ["4", 1]
            }
        }
    });
    graph["7"] = trainer(
        json!(["1", 0]),
        json!(["5", 0]),
        json!(["6", 0]),
        artifact,
        settings,
        steps,
        contract,
    );
    graph
}

async fn stage_or_upload(
    client: &reqwest::Client,
    config: &Config,
    model: &TrainingModel,
    work: &Path,
    run: &Run,
) -> Result<String, TrainError> {
    let pairs = pairs(model, work)?;
    let manifest = serde_json::to_string(&Manifest {
        schema_version: MANIFEST_VERSION,
        architecture: architecture(model),
        pairs,
    })
    .map_err(|error| TrainError::Failed(error.to_string()))?;
    if let Some(input) = local_input(config)? {
        stage_local(work, &input, run)?;
    } else {
        stage_remote(client, config, model, work, run).await?;
    }
    Ok(manifest)
}

fn pairs(model: &TrainingModel, work: &Path) -> Result<Vec<Pair>, TrainError> {
    let targets = pngs(&work.join("targets"))?;
    let controls = pngs(&work.join("control_1")).unwrap_or_default();
    if matches!(model, TrainingModel::QwenEdit { .. }) && targets.len() != controls.len() {
        return Err(TrainError::Invalid(
            "Qwen edit training needs one reference for every target",
        ));
    }
    let mut pairs = Vec::with_capacity(targets.len());
    for (index, target) in targets.iter().enumerate() {
        let expected = format!("{index:04}.png");
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(TrainError::Invalid("training image name is invalid"))?;
        if name != expected {
            return Err(TrainError::Invalid(
                "training image names must be contiguous indices",
            ));
        }
        let instruction_path = target.with_extension("txt");
        if fs::symlink_metadata(&instruction_path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
        {
            return Err(TrainError::Invalid(
                "training instructions must be regular files",
            ));
        }
        let instruction = fs::read_to_string(instruction_path)
            .map_err(|_| TrainError::Invalid("every training image needs an instruction"))?;
        if instruction.trim().is_empty() {
            return Err(TrainError::Invalid(
                "every training image needs an instruction",
            ));
        }
        let reference = match model {
            TrainingModel::Flux { .. } => None,
            TrainingModel::QwenEdit { .. } => {
                let control = controls.get(index).ok_or(TrainError::Invalid(
                    "Qwen edit training needs one reference for every target",
                ))?;
                if control.file_name() != target.file_name() {
                    return Err(TrainError::Invalid(
                        "Qwen edit references must use the target index",
                    ));
                }
                Some(format!("control_1/{expected}"))
            }
        };
        pairs.push(Pair {
            index,
            target: format!("targets/{expected}"),
            reference,
            instruction: instruction.trim().to_string(),
        });
    }
    if pairs.is_empty() {
        return Err(TrainError::Invalid("training needs images"));
    }
    Ok(pairs)
}

fn stage_local(work: &Path, input: &Path, run: &Run) -> Result<(), TrainError> {
    let input = require_directory(input, "ComfyUI input directory")?;
    if !is_single_component(&run.folder) {
        return Err(TrainError::Invalid(
            "training namespace is not a single directory name",
        ));
    }
    let destination = input.join(&run.folder);
    if fs::symlink_metadata(&destination).is_ok() {
        return Err(TrainError::Failed(
            "training namespace already exists".into(),
        ));
    }
    fs::create_dir(&destination).map_err(|error| TrainError::Failed(error.to_string()))?;
    let destination = require_child_directory(&input, &destination, "training namespace")?;
    let result = (|| {
        for directory in ["targets", "control_1"] {
            let source = work.join(directory);
            if !source.is_dir() {
                continue;
            }
            let target = destination.join(directory);
            fs::create_dir(&target).map_err(|error| TrainError::Failed(error.to_string()))?;
            let target =
                require_child_directory(&destination, &target, "training image directory")?;
            for source in pngs(&source)? {
                let name = source
                    .file_name()
                    .ok_or(TrainError::Invalid("training image name is invalid"))?;
                copy_new(&source, &target.join(name))?;
            }
        }
        Ok(())
    })();
    if result.is_err() {
        remove_child_directory(&input, &destination);
    }
    result
}

async fn stage_remote(
    client: &reqwest::Client,
    config: &Config,
    model: &TrainingModel,
    work: &Path,
    run: &Run,
) -> Result<(), TrainError> {
    for directory in ["targets", "control_1"] {
        if directory == "control_1" && matches!(model, TrainingModel::Flux { .. }) {
            continue;
        }
        for png in pngs(&work.join(directory))? {
            let name = png
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or(TrainError::Invalid("training image name is invalid"))?;
            upload_png(
                client,
                config,
                &format!("{}/{directory}", run.folder),
                name,
                &fs::read(&png).map_err(|error| TrainError::Failed(error.to_string()))?,
            )
            .await?;
        }
    }
    Ok(())
}

fn pngs(directory: &Path) -> Result<Vec<PathBuf>, TrainError> {
    let directory = require_directory(directory, "training image directory")?;
    let mut files = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| TrainError::Failed(error.to_string()))? {
        let entry = entry.map_err(|error| TrainError::Failed(error.to_string()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("png") {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| TrainError::Failed(error.to_string()))?;
        if kind.is_symlink() || !kind.is_file() {
            return Err(TrainError::Invalid("training images must be regular files"));
        }
        files.push(path);
    }
    files.sort();
    Ok(files)
}

async fn upload_png(
    client: &reqwest::Client,
    config: &Config,
    subfolder: &str,
    filename: &str,
    bytes: &[u8],
) -> Result<(), TrainError> {
    let part = reqwest::multipart::Part::bytes(bytes.to_vec())
        .file_name(filename.to_string())
        .mime_str("image/png")
        .map_err(|error| TrainError::Failed(error.to_string()))?;
    let form = reqwest::multipart::Form::new()
        .part("image", part)
        .text("overwrite", "false")
        .text("type", "input")
        .text("subfolder", subfolder.to_string());
    let uploaded = authorize(
        config,
        client
            .post(format!("{}/upload/image", config.base_url))
            .multipart(form),
    )
    .send()
    .await
    .map_err(|error| TrainError::Failed(error.to_string()))?
    .error_for_status()
    .map_err(|error| TrainError::Failed(error.to_string()))?
    .json::<UploadResponse>()
    .await
    .map_err(|error| TrainError::Failed(format!("invalid ComfyUI upload response: {error}")))?;
    if uploaded.name != filename || uploaded.subfolder != subfolder || uploaded.kind != "input" {
        return Err(TrainError::Failed(
            "ComfyUI staged a training image under a different name".into(),
        ));
    }
    Ok(())
}

async fn wait_prompt(
    client: &reqwest::Client,
    config: &Config,
    prompt: Uuid,
    timeout: Duration,
) -> Result<(), WaitFailure> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::time::Instant::now() >= deadline {
            let cleanup = cancel_and_wait(client, config, prompt, false).await;
            return Err(WaitFailure {
                error: TrainError::Failed("training timed out".into()),
                cleanup,
            });
        }
        let history = match history(client, config, prompt, deadline).await {
            Ok(history) => history,
            Err(error) => {
                let cleanup = cancel_and_wait(client, config, prompt, false).await;
                return Err(WaitFailure { error, cleanup });
            }
        };
        let entry = match exact_history(&history, prompt) {
            Ok(entry) => entry,
            Err(error) => {
                let cleanup = cancel_and_wait(client, config, prompt, false).await;
                return Err(WaitFailure { error, cleanup });
            }
        };
        if let Some(entry) = entry {
            match train_prompt_complete(entry) {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => {
                    cancel_and_wait(client, config, prompt, true).await;
                    return Err(WaitFailure {
                        error,
                        cleanup: true,
                    });
                }
            }
        }
        tokio::time::sleep(TRAIN_POLL_INTERVAL).await;
    }
}

async fn history(
    client: &reqwest::Client,
    config: &Config,
    prompt: Uuid,
    deadline: tokio::time::Instant,
) -> Result<Value, TrainError> {
    let timeout = deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .min(POLL_TIMEOUT);
    authorize(
        config,
        client
            .get(format!("{}/history/{prompt}", config.base_url))
            .timeout(timeout),
    )
    .send()
    .await
    .map_err(|error| TrainError::Failed(error.to_string()))?
    .error_for_status()
    .map_err(|error| TrainError::Failed(error.to_string()))?
    .json()
    .await
    .map_err(|error| TrainError::Failed(error.to_string()))
}

fn exact_history(history: &Value, prompt: Uuid) -> Result<Option<&Value>, TrainError> {
    let entries = history
        .as_object()
        .ok_or_else(|| TrainError::Failed("ComfyUI history is not an object".into()))?;
    if entries.is_empty() {
        return Ok(None);
    }
    if entries.len() != 1 {
        return Err(TrainError::Failed(
            "ComfyUI history returned an unexpected prompt".into(),
        ));
    }
    entries
        .get(&prompt.to_string())
        .map(Some)
        .ok_or_else(|| TrainError::Failed("ComfyUI history prompt id changed".into()))
}

fn train_prompt_complete(entry: &Value) -> Result<bool, TrainError> {
    let status: HistoryStatus =
        serde_json::from_value(entry.get("status").cloned().unwrap_or(json!({})))
            .map_err(|_| TrainError::Failed("ComfyUI history status is invalid".into()))?;
    if status.status_str.eq_ignore_ascii_case("error") {
        return Err(TrainError::Failed(format!(
            "ComfyUI train failed: {}",
            excerpt::head(
                &entry
                    .get("status")
                    .cloned()
                    .unwrap_or(json!({}))
                    .to_string()
            )
        )));
    }
    Ok(status.completed == Some(true) || status.status_str.eq_ignore_ascii_case("success"))
}

async fn cancel_and_wait(
    client: &reqwest::Client,
    config: &Config,
    prompt: Uuid,
    terminal: bool,
) -> bool {
    let _ = authorize(
        config,
        client
            .post(format!("{}/api/jobs/{prompt}/cancel", config.base_url))
            .timeout(POLL_TIMEOUT),
    )
    .send()
    .await;
    if terminal {
        return true;
    }
    let deadline = tokio::time::Instant::now() + CANCEL_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        if let Ok(history) = history(client, config, prompt, deadline).await
            && let Ok(Some(entry)) = exact_history(&history, prompt)
            && train_prompt_terminal(entry)
        {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(config.poll_interval_milliseconds)).await;
    }
}

fn train_prompt_terminal(entry: &Value) -> bool {
    let Ok(status) =
        serde_json::from_value::<HistoryStatus>(entry.get("status").cloned().unwrap_or(json!({})))
    else {
        return false;
    };
    status.completed == Some(true)
        || status.status_str.eq_ignore_ascii_case("success")
        || status.status_str.eq_ignore_ascii_case("error")
}

pub async fn cleanup(config: &Config, run: &Run) {
    match crate::http::client(config) {
        Ok(client) => cleanup_with(&client, config, run).await,
        Err(error) => {
            tracing::warn!(%error, "could not reach ComfyUI to clean up a training run");
            if run.validate(&config.contract).is_ok() {
                cleanup_local(config, run);
            }
        }
    }
}

/// [`cleanup`] on a client the caller already holds for this run.
pub(crate) async fn cleanup_with(client: &reqwest::Client, config: &Config, run: &Run) {
    if run.validate(&config.contract).is_err() {
        return;
    }
    cleanup_local(config, run);
    if config.validate().is_err() {
        return;
    }
    let graph = json!({

        "1": {
            "class_type": config.contract.cleanup_training_run_node,
            "inputs": { "folder": run.folder, "artifact": run.artifact }
        }
    });
    let Ok(prompt) = queue(client, config, graph).await else {
        return;
    };
    let _ = wait_prompt(client, config, prompt, CLEANUP_TIMEOUT).await;
}

fn cleanup_local(config: &Config, run: &Run) {
    if let Ok(Some(input)) = local_input(config) {
        let folder = input.join(&run.folder);
        remove_child_directory(&input, &folder);
    }
    if let Some(output) = local_output(config) {
        let prefix = format!("{}-step", run.artifact);
        let final_name = format!("{}.safetensors", run.artifact);
        let Ok(entries) = fs::read_dir(output) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if (name == final_name || name.starts_with(&prefix) && name.ends_with(".safetensors"))
                && !entry
                    .file_type()
                    .is_ok_and(|file_type| file_type.is_symlink())
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

fn local_input(config: &Config) -> Result<Option<PathBuf>, TrainError> {
    let Some(root) = config.models_directory.parent() else {
        return Ok(None);
    };
    let root = match fs::symlink_metadata(root) {
        Ok(_) => require_directory(root, "ComfyUI root")?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(TrainError::Failed(error.to_string())),
    };
    let input = root.join("input");
    match fs::symlink_metadata(&input) {
        Ok(_) => require_child_directory(&root, &input, "ComfyUI input directory").map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(TrainError::Failed(error.to_string())),
    }
}

fn local_output(config: &Config) -> Option<PathBuf> {
    let root = require_directory(config.models_directory.parent()?, "ComfyUI root").ok()?;
    let output = require_child_directory(&root, &root.join("output"), "ComfyUI output").ok()?;
    require_child_directory(&output, &output.join("loras"), "ComfyUI LoRA output").ok()
}

fn require_directory(path: &Path, label: &'static str) -> Result<PathBuf, TrainError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| TrainError::Failed(format!("{label}: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TrainError::Invalid(label));
    }
    path.canonicalize()
        .map_err(|error| TrainError::Failed(format!("{label}: {error}")))
}

fn require_child_directory(
    root: &Path,
    path: &Path,
    label: &'static str,
) -> Result<PathBuf, TrainError> {
    let root = require_directory(root, label)?;
    let path = require_directory(path, label)?;
    if path.parent() != Some(root.as_path()) {
        return Err(TrainError::Invalid(label));
    }
    Ok(path)
}

/// Whether `name` joins onto a directory as exactly one entry inside it.
pub(crate) fn is_single_component(name: &str) -> bool {
    let mut components = Path::new(name).components();
    !name.contains(['/', '\\'])
        && matches!(
            (components.next(), components.next()),
            (Some(Component::Normal(_)), None)
        )
}

fn copy_new(source: &Path, destination: &Path) -> Result<(), TrainError> {
    let metadata =
        fs::symlink_metadata(source).map_err(|error| TrainError::Failed(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(TrainError::Invalid("training images must be regular files"));
    }
    let mut source = File::open(source).map_err(|error| TrainError::Failed(error.to_string()))?;
    if !source.metadata().is_ok_and(|metadata| metadata.is_file()) {
        return Err(TrainError::Invalid("training images must be regular files"));
    }
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|error| TrainError::Failed(error.to_string()))?;
    io::copy(&mut source, &mut destination)
        .and_then(|_| destination.sync_all())
        .map_err(|error| TrainError::Failed(error.to_string()))?;
    Ok(())
}

fn remove_child_directory(root: &Path, path: &Path) {
    let Ok(root) = require_directory(root, "cleanup root") else {
        return;
    };
    let Ok(metadata) = fs::symlink_metadata(path) else {
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

pub(crate) fn manifest(
    model: &TrainingModel,
    captions: &HashMap<String, String>,
) -> Option<String> {
    if captions.is_empty() {
        return None;
    }
    let mut entries: Vec<(&String, &String)> = captions.iter().collect();
    entries.sort_by_key(|(name, _)| *name);
    let pairs = entries
        .into_iter()
        .enumerate()
        .map(|(index, (name, instruction))| {
            if name != &format!("{index:04}.png") || instruction.trim().is_empty() {
                return None;
            }
            Some(Pair {
                index,
                target: format!("targets/{name}"),
                reference: matches!(model, TrainingModel::QwenEdit { .. })
                    .then(|| format!("control_1/{name}")),
                instruction: instruction.trim().to_string(),
            })
        })
        .collect::<Option<Vec<Pair>>>()?;
    serde_json::to_string(&Manifest {
        schema_version: MANIFEST_VERSION,
        architecture: architecture(model),
        pairs,
    })
    .ok()
}

pub(crate) fn architecture(model: &TrainingModel) -> &'static str {
    match model {
        TrainingModel::Flux { .. } => "flux",
        TrainingModel::QwenEdit { .. } => "qwen_edit",
    }
}

fn validate_run_name(name: &str, prefix: &str) -> Result<(), TrainError> {
    let Some(id) = name.strip_prefix(prefix) else {
        return Err(TrainError::Invalid("invalid training run namespace"));
    };
    let uuid = Uuid::parse_str(id).map_err(|_| TrainError::Invalid("invalid training run UUID"))?;
    if uuid.get_version_num() != 4 || uuid.to_string() != id {
        return Err(TrainError::Invalid("invalid training run UUID"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{Recipe, RecipeCatalog};
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const DATASETS: [usize; 4] = [8, 24, 100, 300];
    const HEALTHY_PASSES: std::ops::RangeInclusive<f64> = 17.0..=19.0;

    fn passes(config: &TrainConfig, images: usize) -> f64 {
        f64::from(config.steps(images)) / images as f64
    }

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

    /// A dataset on disk, as `lora::train` leaves it for the graph runner.
    fn dataset() -> tempfile::TempDir {
        let work = tempfile::tempdir().unwrap();
        let targets = work.path().join("targets");
        fs::create_dir_all(&targets).unwrap();
        let mut png = Vec::new();
        {
            use image::ImageEncoder;
            image::codecs::png::PngEncoder::new(&mut png)
                .write_image(&[10, 20, 30], 1, 1, image::ExtendedColorType::Rgb8)
                .unwrap();
        }
        fs::write(targets.join("0000.png"), &png).unwrap();
        fs::write(targets.join("0000.txt"), "ohwx, a portrait").unwrap();
        fs::write(targets.join("0001.png"), &png).unwrap();
        fs::write(targets.join("0001.txt"), "ohwx, from behind").unwrap();
        work
    }

    fn config(server: &MockServer) -> Config {
        Config {
            enabled: true,
            base_url: server.uri(),
            api_token: Some("secret".into()),
            train_timeout_seconds: 60,
            poll_interval_milliseconds: 50,
            // No sibling input/ directory, so staging falls through to upload.
            models_directory: std::env::temp_dir().join(format!("comfy-models-{}", Uuid::new_v4())),
            ..Default::default()
        }
    }

    fn recipe() -> Recipe {
        RecipeCatalog::packaged()
            .unwrap()
            .get("flux-schnell")
            .expect("the packaged catalog ships flux-schnell")
            .clone()
    }

    fn base() -> TrainingModel {
        recipe()
            .training_model()
            .expect("flux-schnell is a trainable base")
    }

    async fn queues(server: &MockServer, body: serde_json::Value) {
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    /// ComfyUI echoes back where it put the file, and the uploader checks that
    /// what came back is what it sent.
    struct Stage;

    impl wiremock::Respond for Stage {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            let body = String::from_utf8_lossy(&request.body);
            let field = |name: &str| {
                body.split(&format!("name=\"{name}\""))
                    .nth(1)
                    .and_then(|rest| rest.split("\r\n\r\n").nth(1))
                    .and_then(|rest| rest.split("\r\n").next())
                    .unwrap_or_default()
                    .to_string()
            };
            let filename = body
                .split("filename=\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_default()
                .to_string();
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": filename,
                "subfolder": field("subfolder"),
                "type": field("type"),
            }))
        }
    }

    async fn uploads(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(Stage)
            .mount(server)
            .await;
    }

    struct Delayed<R>(R, Duration);

    impl<R: wiremock::Respond> wiremock::Respond for Delayed<R> {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            self.0.respond(request).set_delay(self.1)
        }
    }

    #[tokio::test]
    async fn a_redirect_never_carries_the_token_to_another_host() {
        let server = MockServer::start().await;
        let elsewhere = MockServer::start().await;
        let prompt = Uuid::new_v4();
        uploads(&server).await;
        queues(&server, json!({"prompt_id": prompt, "number": 1})).await;
        finishes(&server, prompt).await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/view", elsewhere.uri()).as_str()),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(Serve(vec![7u8; 20_000], "application/safetensors"))
            .mount(&elsewhere)
            .await;

        let work = dataset();
        let output = work.path().join("out.safetensors");
        run(&config(&server), &base(), work.path(), &output, 2)
            .await
            .expect_err("a redirected download is not the artifact that was asked for");

        assert!(
            elsewhere.received_requests().await.unwrap().is_empty(),
            "the token header followed a redirect to another host"
        );
        assert!(!output.exists());
    }

    #[tokio::test]
    async fn a_short_training_deadline_does_not_cut_off_a_slow_upload() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(Delayed(Stage, Duration::from_millis(1_200)))
            .mount(&server)
            .await;
        queues(&server, json!({"prompt_id": prompt, "number": 1})).await;
        finishes(&server, prompt).await;
        serves(&server, vec![7u8; 20_000]).await;
        let config = Config {
            train_timeout_seconds: 1,
            request_timeout_seconds: 30,
            ..config(&server)
        };

        let work = dataset();
        let output = work.path().join("out.safetensors");
        run(&config, &base(), work.path(), &output, 2)
            .await
            .expect("the training deadline bounds the graph, not each request");
        assert_eq!(fs::read(&output).unwrap(), vec![7u8; 20_000]);
    }

    async fn finishes(server: &MockServer, prompt: Uuid) {
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({prompt.to_string(): {"status": {"completed": true, "status_str": "success"}}}),
            ))
            .mount(server)
            .await;
    }

    /// ComfyUI names the artifact it is serving, and the download checks that
    /// the file it gets back is the one it asked for.
    struct Serve(Vec<u8>, &'static str);

    impl wiremock::Respond for Serve {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            let filename = request
                .url
                .query_pairs()
                .find(|(key, _)| key == "filename")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            ResponseTemplate::new(200)
                .insert_header(
                    "content-disposition",
                    format!("filename=\"{filename}\"").as_str(),
                )
                .insert_header("content-type", self.1)
                .set_body_bytes(self.0.clone())
        }
    }

    /// The pinned ComfyUI serves a `.safetensors` artifact as
    /// `application/safetensors`, so that is what the default mock sends.
    async fn serves(server: &MockServer, weights: Vec<u8>) {
        serves_as(server, weights, "application/safetensors").await;
    }

    async fn serves_as(server: &MockServer, weights: Vec<u8>, content_type: &'static str) {
        Mock::given(method("GET"))
            .and(path("/view"))
            .and(query_param("subfolder", "loras"))
            .respond_with(Serve(weights, content_type))
            .mount(server)
            .await;
    }

    /// The download refused everything but `application/octet-stream`, and the
    /// pinned ComfyUI serves `application/safetensors`, so every trained
    /// adapter was rejected and then swept by the error path: a completed run
    /// produced nothing. The mocks had always sent what the code expected.
    #[test]
    fn the_content_types_comfyui_serves_weights_as_are_accepted() {
        for served in [
            "application/safetensors",
            "application/octet-stream",
            "application/safetensors; charset=binary",
            "Application/SafeTensors",
        ] {
            assert!(is_weight_payload(served), "{served} must be accepted");
        }
    }

    /// The check exists to stop an error page being written out as weights.
    #[test]
    fn an_error_page_is_not_mistaken_for_weights() {
        for served in [
            "text/html",
            "text/html; charset=utf-8",
            "application/json",
            "image/png",
            "",
        ] {
            assert!(!is_weight_payload(served), "{served} must be refused");
        }
    }

    #[tokio::test]
    async fn a_view_response_that_is_not_weights_is_refused() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        uploads(&server).await;
        queues(&server, json!({"prompt_id": prompt, "number": 1})).await;
        finishes(&server, prompt).await;
        serves_as(&server, vec![7u8; 20_000], "text/html").await;

        let work = dataset();
        let output = work.path().join("my-style.safetensors");
        let error = run(&config(&server), &base(), work.path(), &output, 2)
            .await
            .expect_err("an HTML body must not be written out as weights");

        assert!(
            format!("{error}").contains("not safetensors data"),
            "unexpected error: {error}"
        );
        assert!(!output.exists(), "a refused download must write nothing");
    }

    #[tokio::test]
    async fn a_finished_graph_writes_the_weights_comfyui_produced() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        uploads(&server).await;
        queues(&server, json!({"prompt_id": prompt, "number": 1})).await;
        finishes(&server, prompt).await;
        serves(&server, vec![7u8; 20_000]).await;

        let work = dataset();
        let output = work.path().join("my-style.safetensors");
        let started = run(&config(&server), &base(), work.path(), &output, 2)
            .await
            .unwrap();
        assert_eq!(fs::read(&output).unwrap(), vec![7u8; 20_000]);

        let posted = server.received_requests().await.unwrap();
        let graph = posted
            .iter()
            .find(|request| request.url.path() == "/prompt")
            .expect("the graph was never submitted");
        assert_eq!(
            graph.headers.get(crate::config::TOKEN_HEADER).unwrap(),
            "secret",
            "a token-protected ComfyUI has to be told who is asking"
        );
        let body: Value = serde_json::from_slice(&graph.body).unwrap();
        let inputs = &body["prompt"]["5"]["inputs"];
        assert_eq!(
            inputs["steps"],
            packaged_config().unwrap().min_steps,
            "two images clamp up to min_steps"
        );
        assert_eq!(inputs["save_name"], started.artifact);
        assert_eq!(
            body["prompt"]["1"]["inputs"]["ckpt_name"],
            recipe().defaults["checkpoint"]
        );
        assert_eq!(
            body["prompt"]["2"]["inputs"]["resolution"],
            packaged_config().unwrap().resolution()
        );
        let manifest: Value = serde_json::from_str(
            body["prompt"]["2"]["inputs"]["manifest_json"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["architecture"], "flux");
        assert_eq!(manifest["pairs"][0]["target"], "targets/0000.png");
        assert_eq!(manifest["pairs"][0]["instruction"], "ohwx, a portrait");
        assert_eq!(manifest["pairs"][1]["target"], "targets/0001.png");
        assert_eq!(manifest["pairs"][1]["instruction"], "ohwx, from behind");
        assert!(
            manifest["pairs"][0]["reference"].is_null(),
            "an identity run trains on the target alone"
        );
    }

    #[tokio::test]
    async fn a_target_with_no_caption_beside_it_is_refused() {
        let work = dataset();
        fs::remove_file(work.path().join("targets/0001.txt")).unwrap();
        let error = run(
            &Config {
                enabled: true,
                base_url: "http://127.0.0.1:9".into(),
                ..Default::default()
            },
            &base(),
            work.path(),
            &work.path().join("out.safetensors"),
            2,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&error, TrainError::Invalid(message) if message.contains("instruction")),
            "a dataset the writer could not have produced must not reach the trainer: {error}"
        );
    }

    #[tokio::test]
    async fn a_graph_comfyui_will_not_accept_fails_the_job() {
        let server = MockServer::start().await;
        uploads(&server).await;
        queues(
            &server,
            json!({
                "prompt_id": Uuid::new_v4(),
                "number": 1,
                "error": {"type": "prompt_outputs_failed_validation"}
            }),
        )
        .await;

        let work = dataset();
        let error = run(
            &config(&server),
            &base(),
            work.path(),
            &work.path().join("out.safetensors"),
            2,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&error, TrainError::Failed(message) if message.contains("rejected")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn weights_too_small_to_be_trained_are_not_accepted() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        uploads(&server).await;
        queues(&server, json!({"prompt_id": prompt, "number": 1})).await;
        finishes(&server, prompt).await;
        // ComfyUI serves its error pages with a 200, so size is the only tell.
        serves(&server, b"<html>not found</html>".to_vec()).await;

        let work = dataset();
        let output = work.path().join("out.safetensors");
        let error = run(&config(&server), &base(), work.path(), &output, 2)
            .await
            .unwrap_err();
        assert!(
            matches!(&error, TrainError::Failed(message) if message.contains("too small")),
            "{error}"
        );
        assert!(
            !output.exists(),
            "a rejected download leaves nothing behind"
        );
    }

    #[tokio::test]
    async fn a_graph_that_never_finishes_times_out() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        uploads(&server).await;
        queues(&server, json!({"prompt_id": prompt, "number": 1})).await;
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({prompt.to_string(): {"status": {"status_str": "running"}}}),
                ),
            )
            .mount(&server)
            .await;

        let work = dataset();
        let client = reqwest::Client::new();
        let failure = wait_prompt(
            &client,
            &config(&server),
            prompt,
            Duration::from_millis(120),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&failure.error, TrainError::Failed(message) if message.contains("timed out")),
            "{}",
            failure.error
        );
        drop(work);
    }

    #[tokio::test]
    async fn a_graph_that_errors_is_reported_rather_than_polled_forever() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({prompt.to_string(): {"status": {"status_str": "error"}}}),
                ),
            )
            .mount(&server)
            .await;

        let failure = wait_prompt(
            &reqwest::Client::new(),
            &config(&server),
            prompt,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&failure.error, TrainError::Failed(message) if message.contains("train failed")),
            "{}",
            failure.error
        );
    }

    fn escaping() -> Contract {
        Contract {
            folder_prefix: "../escape-".into(),
            ..Contract::default()
        }
    }

    #[tokio::test]
    async fn a_contract_that_names_a_path_outside_comfyui_is_refused_before_anything_is_sent() {
        let server = MockServer::start().await;
        let config = Config {
            contract: escaping(),
            ..config(&server)
        };
        let work = dataset();
        let error = run(
            &config,
            &base(),
            work.path(),
            &work.path().join("out.safetensors"),
            2,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TrainError::Configuration(_)), "{error}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn cleanup_under_an_invalid_contract_touches_nothing() {
        let server = MockServer::start().await;
        let config = Config {
            contract: escaping(),
            ..config(&server)
        };
        cleanup(&config, &Run::new(&config.contract)).await;
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[test]
    fn local_staging_never_creates_a_directory_outside_the_input_directory() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("input");
        fs::create_dir(&input).unwrap();
        let escaped = format!("escaped-{}", Uuid::new_v4());
        let run = Run {
            folder: format!("../{escaped}"),
            artifact: format!("{ARTIFACT_PREFIX}{}", Uuid::new_v4()),
        };
        let work = dataset();
        assert!(stage_local(work.path(), &input, &run).is_err());
        assert!(
            !root.path().join(&escaped).exists(),
            "staging created a directory beside the input directory"
        );
    }

    #[test]
    fn only_a_plain_name_is_a_single_component() {
        assert!(is_single_component("zone-run"));
        for name in [
            "",
            ".",
            "..",
            "../run",
            "nested/run",
            "run/",
            "back\\slash",
            "/run",
        ] {
            assert!(!is_single_component(name), "{name:?} was accepted");
        }
    }

    #[tokio::test]
    async fn a_zero_training_budget_is_refused_before_anything_is_sent() {
        let server = MockServer::start().await;
        let config = Config {
            train_timeout_seconds: 0,
            ..config(&server)
        };
        let work = dataset();
        let error = run(
            &config,
            &base(),
            work.path(),
            &work.path().join("out.safetensors"),
            2,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TrainError::Configuration(_)), "{error}");
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn training_against_a_disabled_comfyui_does_not_reach_the_network() {
        let work = dataset();
        let error = run(
            &Config::default(),
            &base(),
            work.path(),
            &work.path().join("out.safetensors"),
            2,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TrainError::Disabled), "{error}");
    }

    #[tokio::test]
    async fn a_dataset_beside_comfyui_is_staged_rather_than_uploaded() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        queues(&server, json!({"prompt_id": prompt, "number": 1})).await;
        finishes(&server, prompt).await;
        serves(&server, vec![7u8; 20_000]).await;

        // models_directory with a sibling input/ is the shared-volume deployment,
        // where the dataset can simply be copied into place.
        let comfy = tempfile::tempdir().unwrap();
        let input = comfy.path().join("input");
        fs::create_dir_all(&input).unwrap();
        let mut settings = config(&server);
        settings.models_directory = comfy.path().join("models");
        fs::create_dir_all(&settings.models_directory).unwrap();

        let work = dataset();
        run(
            &settings,
            &base(),
            work.path(),
            &work.path().join("out.safetensors"),
            2,
        )
        .await
        .unwrap();

        let staged: Vec<PathBuf> = fs::read_dir(&input)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|item| item.path()))
            .collect();
        assert_eq!(staged.len(), 1, "one folder per training run");
        // Captions ride in the graph's captions_json, so only the images stage.
        assert!(staged[0].join("targets/0000.png").is_file());
        assert!(staged[0].join("targets/0001.png").is_file());
        assert!(
            !server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|request| request.url.path() == "/upload/image"),
            "a staged dataset must not also be uploaded"
        );
    }

    #[test]
    fn train_graphs_use_native_encoders_instead_of_an_optional_dataset_node() {
        let settings = packaged_config().unwrap();
        let contract = Contract::default();
        let flux = train_graph(
            &flux(),
            "folder",
            "{}",
            "artifact",
            &settings,
            12,
            &contract,
        );
        let qwen = train_graph(
            &qwen(),
            "folder",
            "{}",
            "artifact",
            &settings,
            12,
            &contract,
        );
        for graph in [&flux, &qwen] {
            assert!(graph.to_string().contains("VAEEncode"));
            assert!(graph.to_string().contains(LOAD_TRAIN_DATASET_NODE));
        }
        assert_eq!(flux["4"]["class_type"], "CLIPTextEncode");
        assert_eq!(qwen["1"]["class_type"], "UNETLoader");
        assert_eq!(qwen["1"]["inputs"]["unet_name"], "qwen-unet.safetensors");
        assert_eq!(qwen["2"]["class_type"], "CLIPLoader");
        assert_eq!(qwen["2"]["inputs"]["type"], "qwen_image");
        assert_eq!(qwen["2"]["inputs"]["clip_name"], "qwen-clip.safetensors");
        assert_eq!(qwen["3"]["class_type"], "VAELoader");
        assert_eq!(qwen["3"]["inputs"]["vae_name"], "qwen-vae.safetensors");
        assert_eq!(qwen["5"]["class_type"], "VAEEncode");
        assert_eq!(qwen["6"]["class_type"], "TextEncodeQwenImageEditPlus");
        assert_eq!(qwen["5"]["inputs"]["pixels"], json!(["4", 0]));
        assert_eq!(qwen["6"]["inputs"]["image1"], json!(["4", 1]));
        assert_eq!(qwen["6"]["inputs"]["prompt"], json!(["4", 2]));
        assert!(!qwen.to_string().contains("CheckpointLoaderSimple"));
    }

    #[test]
    fn qwen_manifest_keeps_same_index_target_reference_and_instruction() {
        let raw: Value = serde_json::from_str(
            &manifest(
                &qwen(),
                &HashMap::from([("0000.png".into(), "change the colour".into())]),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(raw["architecture"], "qwen_edit");
        assert_eq!(raw["pairs"][0]["target"], "targets/0000.png");
        assert_eq!(raw["pairs"][0]["reference"], "control_1/0000.png");
        assert_eq!(raw["pairs"][0]["instruction"], "change the colour");
    }

    #[test]
    fn run_names_are_server_generated_v4_uuids() {
        let contract = Contract::default();
        let run = Run::new(&contract);
        run.validate(&contract).unwrap();
        assert_ne!(run, Run::new(&contract));
        assert_ne!(
            run.folder.trim_start_matches(FOLDER_PREFIX),
            run.artifact.trim_start_matches(ARTIFACT_PREFIX)
        );
        assert!(
            Run {
                folder: format!("{FOLDER_PREFIX}user-name"),
                artifact: format!("{ARTIFACT_PREFIX}user-name"),
            }
            .validate(&contract)
            .is_err()
        );
    }

    fn renamed() -> Contract {
        Contract {
            train_lora_node: "TrainAdapter".into(),
            cleanup_training_run_node: "SweepRun".into(),
            load_train_dataset_node: "LoadDataset".into(),
            probe_loss_node: "MeasureLoss".into(),
            stage_training_artifact_node: "StageWeights".into(),
            folder_prefix: "dataset-".into(),
            artifact_prefix: "adapter-".into(),
            probe_prefix: "sample-".into(),
            environment_prefix: "TRAINER".into(),
            input_environment_prefix: "COMFY".into(),
        }
    }

    #[test]
    fn a_renamed_node_pack_receives_its_own_class_types() {
        let settings = packaged_config().unwrap();
        let contract = renamed();
        let flux = train_graph(
            &flux(),
            "folder",
            "{}",
            "artifact",
            &settings,
            12,
            &contract,
        );
        let qwen = train_graph(
            &qwen(),
            "folder",
            "{}",
            "artifact",
            &settings,
            12,
            &contract,
        );
        assert_eq!(flux["2"]["class_type"], "LoadDataset");
        assert_eq!(flux["5"]["class_type"], "TrainAdapter");
        assert_eq!(qwen["4"]["class_type"], "LoadDataset");
        assert_eq!(qwen["7"]["class_type"], "TrainAdapter");
        for graph in [&flux, &qwen] {
            let serialized = graph.to_string();
            assert!(
                !serialized.contains(TRAIN_LORA_NODE)
                    && !serialized.contains(LOAD_TRAIN_DATASET_NODE),
                "a default node name leaked into a renamed graph: {serialized}"
            );
        }
    }

    #[test]
    fn run_names_follow_the_contract_they_were_made_under() {
        let contract = renamed();
        let run = Run::new(&contract);
        assert!(run.folder.starts_with("dataset-"), "{}", run.folder);
        assert!(run.artifact.starts_with("adapter-"), "{}", run.artifact);
        run.validate(&contract).unwrap();
        assert!(
            run.validate(&Contract::default()).is_err(),
            "the default node pack refuses a namespace it did not issue"
        );
        assert!(Run::default().validate(&contract).is_err());
    }

    #[test]
    fn the_default_contract_names_the_packaged_nodes_and_script() {
        let contract = Contract::default();
        assert_eq!(contract.train_lora_node, TRAIN_LORA_NODE);
        assert_eq!(
            contract.cleanup_training_run_node,
            CLEANUP_TRAINING_RUN_NODE
        );
        assert_eq!(contract.load_train_dataset_node, LOAD_TRAIN_DATASET_NODE);
        assert_eq!(contract.probe_loss_node, PROBE_LOSS_NODE);
        assert_eq!(
            contract.stage_training_artifact_node,
            STAGE_TRAINING_ARTIFACT_NODE
        );
        assert_eq!(
            contract.variable("OUTPUT"),
            format!("{ENVIRONMENT_PREFIX}_OUTPUT")
        );
        assert_eq!(
            contract.input_variable(),
            format!("{INPUT_ENVIRONMENT_PREFIX}_INPUT")
        );
    }

    #[tokio::test]
    async fn cleanup_asks_a_renamed_node_pack_for_its_own_node() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        queues(&server, json!({"prompt_id": prompt, "number": 1})).await;
        finishes(&server, prompt).await;
        let config = Config {
            contract: renamed(),
            ..config(&server)
        };

        cleanup(&config, &Run::new(&config.contract)).await;

        let posted = server.received_requests().await.unwrap();
        let graph = posted
            .iter()
            .find(|request| request.url.path() == "/prompt")
            .expect("cleanup never queued its graph");
        let body: Value = serde_json::from_slice(&graph.body).unwrap();
        assert_eq!(body["prompt"]["1"]["class_type"], "SweepRun");
    }

    #[tokio::test]
    async fn cleanup_never_asks_the_node_pack_to_sweep_a_foreign_namespace() {
        let server = MockServer::start().await;
        let config = config(&server);

        cleanup(&config, &Run::new(&renamed())).await;

        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "a run named under another contract must not reach ComfyUI"
        );
    }

    #[tokio::test]
    async fn a_configured_token_header_carries_the_token() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        uploads(&server).await;
        queues(&server, json!({"prompt_id": prompt, "number": 1})).await;
        finishes(&server, prompt).await;
        serves(&server, vec![7u8; 20_000]).await;
        let config = Config {
            token_header: "Authorization-Token".into(),
            ..config(&server)
        };

        let work = dataset();
        run(
            &config,
            &base(),
            work.path(),
            &work.path().join("out.safetensors"),
            2,
        )
        .await
        .unwrap();

        for request in server.received_requests().await.unwrap() {
            assert_eq!(
                request.headers.get("Authorization-Token").unwrap(),
                "secret",
                "{} went out without the configured header",
                request.url.path()
            );
            assert!(request.headers.get(crate::config::TOKEN_HEADER).is_none());
        }
    }

    #[test]
    fn an_atomic_download_refuses_to_follow_the_final_symlink() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = tempfile::tempdir().unwrap();
            let victim = root.path().join("victim");
            fs::write(&victim, b"safe").unwrap();
            let output = root.path().join("adapter.safetensors");
            symlink(&victim, &output).unwrap();
            assert!(atomic_write(&output, b"replacement").is_err());
            assert_eq!(fs::read(victim).unwrap(), b"safe");
        }
    }

    #[test]
    fn dataset_manifest_refuses_symlinked_images_and_instructions() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;

            let root = tempfile::tempdir().unwrap();
            let targets = root.path().join("targets");
            fs::create_dir(&targets).unwrap();
            let victim = root.path().join("victim.png");
            fs::write(&victim, b"image").unwrap();
            symlink(&victim, targets.join("0000.png")).unwrap();
            fs::write(targets.join("0000.txt"), b"portrait").unwrap();
            assert!(pairs(&flux(), root.path()).is_err());

            fs::remove_file(targets.join("0000.png")).unwrap();
            fs::write(targets.join("0000.png"), b"image").unwrap();
            fs::remove_file(targets.join("0000.txt")).unwrap();
            symlink(&victim, targets.join("0000.txt")).unwrap();
            assert!(pairs(&flux(), root.path()).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_staging_rejects_symlinked_root_input_and_destination() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let actual = root.path().join("actual");
        fs::create_dir_all(actual.join("models")).unwrap();
        fs::create_dir(actual.join("input")).unwrap();
        let linked_root = root.path().join("comfy");
        symlink(&actual, &linked_root).unwrap();
        let config = Config {
            models_directory: linked_root.join("models"),
            ..Default::default()
        };
        assert!(local_input(&config).is_err());

        let safe = root.path().join("safe");
        let outside = root.path().join("outside");
        fs::create_dir_all(safe.join("models")).unwrap();
        fs::create_dir(&outside).unwrap();
        symlink(&outside, safe.join("input")).unwrap();
        let config = Config {
            models_directory: safe.join("models"),
            ..Default::default()
        };
        assert!(local_input(&config).is_err());

        fs::remove_file(safe.join("input")).unwrap();
        fs::create_dir(safe.join("input")).unwrap();
        let run = Run::new(&Contract::default());
        symlink(&outside, safe.join("input").join(&run.folder)).unwrap();
        assert!(stage_local(root.path(), &safe.join("input"), &run).is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_never_follows_input_or_output_symlinks() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let comfy = root.path().join("comfy");
        let outside_input = root.path().join("outside-input");
        let outside_output = root.path().join("outside-output");
        fs::create_dir_all(comfy.join("models")).unwrap();
        fs::create_dir(&outside_input).unwrap();
        fs::create_dir(&outside_output).unwrap();
        let run = Run::new(&Contract::default());
        fs::create_dir(outside_input.join(&run.folder)).unwrap();
        fs::write(outside_input.join(&run.folder).join("kept"), b"safe").unwrap();
        fs::write(
            outside_output.join(format!("{}.safetensors", run.artifact)),
            b"safe",
        )
        .unwrap();
        symlink(&outside_input, comfy.join("input")).unwrap();
        fs::create_dir(comfy.join("output")).unwrap();
        symlink(&outside_output, comfy.join("output/loras")).unwrap();
        cleanup_local(
            &Config {
                models_directory: comfy.join("models"),
                ..Default::default()
            },
            &run,
        );
        assert_eq!(
            fs::read(outside_input.join(&run.folder).join("kept")).unwrap(),
            b"safe"
        );
        assert_eq!(
            fs::read(outside_output.join(format!("{}.safetensors", run.artifact))).unwrap(),
            b"safe"
        );
    }

    #[tokio::test]
    async fn failed_training_cancels_only_its_exact_terminal_prompt() {
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
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"cancelled": false})))
            .expect(1)
            .mount(&server)
            .await;
        let failure = wait_prompt(
            &reqwest::Client::new(),
            &Config {
                base_url: server.uri(),
                poll_interval_milliseconds: 1,
                ..Default::default()
            },
            prompt,
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(failure.cleanup);
        let cancel = format!("/api/jobs/{prompt}/cancel");
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .filter(|request| request.url.path().contains("/api/jobs/"))
                .all(|request| request.url.path() == cancel)
        );
    }

    #[tokio::test]
    async fn malformed_train_queue_response_cancels_its_recoverable_prompt() {
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
        let failure = queue(&reqwest::Client::new(), &config, json!({}))
            .await
            .unwrap_err();
        assert!(failure.cleanup);
        assert!(
            failure
                .error
                .to_string()
                .contains("invalid ComfyUI prompt response")
        );
    }

    #[test]
    fn a_larger_dataset_is_never_trained_less_per_image_than_a_smaller_one() {
        let config = packaged_config().unwrap();
        for sizes in DATASETS.windows(2) {
            assert!(passes(&config, sizes[1]) >= passes(&config, sizes[0]));
        }
    }

    #[test]
    fn every_realistic_dataset_trains_inside_the_measured_band() {
        let config = packaged_config().unwrap();
        for images in DATASETS {
            assert!(HEALTHY_PASSES.contains(&passes(&config, images)));
        }
    }

    #[test]
    fn the_floor_and_ceiling_bound_training() {
        let config = packaged_config().unwrap();
        assert!(config.min_steps <= config.max_steps);
        assert_eq!(config.steps(1), config.min_steps);
        assert_eq!(config.steps(usize::MAX), config.max_steps);
    }

    #[test]
    fn packaged_config_tracks_the_shipped_json() {
        let config = packaged_config().unwrap();
        let raw: Value = serde_json::from_str(PACKAGED_TRAIN_CONFIG).unwrap();
        assert_eq!(raw["passes_per_image"], config.passes_per_image);
        assert_eq!(raw["min_steps"], config.min_steps);
        assert_eq!(raw["max_steps"], config.max_steps);
        assert_eq!(raw["rank"], config.rank);
        assert_eq!(raw["learning_rate"], config.learning_rate);
        assert_eq!(raw["lora_dtype"], config.lora_dtype);
        assert_eq!(raw["training_dtype"], config.training_dtype);
        assert_eq!(raw["resolution"], config.resolution);
        assert_eq!(raw["bypass_mode"], config.bypass_mode);
        assert_eq!(raw["gradient_checkpointing"], config.gradient_checkpointing);
        assert_eq!(raw["checkpoint_depth"], config.checkpoint_depth);
        assert_eq!(raw["seed"], config.seed);
    }

    #[test]
    fn prompt_history_must_contain_only_the_requested_uuid() {
        let prompt = Uuid::new_v4();
        assert!(exact_history(&json!({}), prompt).unwrap().is_none());
        assert!(
            exact_history(&json!({ prompt.to_string(): {"status": {}} }), prompt)
                .unwrap()
                .is_some()
        );
        assert!(exact_history(&json!({ Uuid::new_v4().to_string(): {} }), prompt).is_err());
    }

    #[test]
    fn a_failure_comfyui_reports_is_quoted_bounded_and_sanitized() {
        let entry = json!({"status": {
            "status_str": "error",
            "messages": [format!("\u{1b}[31m{}", "traceback line\n".repeat(10_000))]
        }});
        let TrainError::Failed(message) = train_prompt_complete(&entry).unwrap_err() else {
            panic!("a failed graph is a training failure");
        };
        assert!(
            message.chars().count() <= "ComfyUI train failed: ".len() + excerpt::LIMIT + 1,
            "{} characters of ComfyUI's report reached the error",
            message.len()
        );
        assert!(!message.contains('\u{1b}'));
    }

    #[test]
    fn train_prompt_waits_until_status_completes() {
        assert!(!train_prompt_complete(&json!({"status": {"status_str": "running"}})).unwrap());
        assert!(
            train_prompt_complete(&json!({"status": {"completed": true, "status_str": "success"}}))
                .unwrap()
        );
        assert!(train_prompt_complete(&json!({"status": {"status_str": "error"}})).is_err());
    }
}
