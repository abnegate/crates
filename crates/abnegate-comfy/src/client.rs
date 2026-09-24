//! Direct ComfyUI API client. Graphs come from packaged recipes; chat only
//! supplies prompt, seed, checkpoint filename, and an optional source image.

mod error;
mod generated_image;
mod source_image;
mod source_video;

pub use error::Error;
pub use generated_image::GeneratedImage;
pub use source_image::MAXIMUM_SOURCE_IMAGE_BYTES;
pub use source_image::SourceImage;
pub use source_video::MAXIMUM_SOURCE_VIDEO_BYTES;
pub use source_video::SourceVideo;

use reqwest::Client as HttpClient;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::config::Config;
use crate::media::MediaType;
use crate::recipe::{
    Fill, PromptMode, Recipe, RecipeCatalog, sanitize_upload_name, sanitize_weight_filename,
};

const PACKAGED_VIDEO_WORKFLOW: &str = include_str!("../comfyui/workflows/wan2.2-ti2v-5b-api.json");
const PACKAGED_IMAGE_TO_VIDEO_WORKFLOW: &str =
    include_str!("../comfyui/workflows/wan2.2-ti2v-5b-i2v-api.json");
const PACKAGED_AUDIO_WORKFLOW: &str =
    include_str!("../comfyui/workflows/ace-step-v1-3.5b-api.json");
const PACKAGED_UPSCALE_WORKFLOW: &str = include_str!("../comfyui/workflows/upscale-image-api.json");
const PACKAGED_UPSCALE_VIDEO_WORKFLOW: &str =
    include_str!("../comfyui/workflows/upscale-video-api.json");

#[derive(Clone)]
pub struct Client {
    config: Config,
    client: HttpClient,
    catalog: RecipeCatalog,
}

#[derive(Deserialize)]
struct PromptResponse {
    prompt_id: String,
}

#[derive(Debug, Deserialize)]
struct UploadResponse {
    name: String,
    #[serde(default)]
    subfolder: String,
}

#[derive(Debug, Deserialize)]
struct OutputImage {
    filename: String,
    #[serde(default)]
    subfolder: String,
    #[serde(default = "default_output_type")]
    r#type: String,
}

fn default_output_type() -> String {
    "output".to_string()
}

#[derive(Clone, Copy)]
enum OutputMode {
    Image,
    Video,
    Audio,
}

/// What to poll a submitted graph for, and what to tell the caller while it runs.
#[derive(Clone, Copy)]
struct Collection<'a> {
    mode: OutputMode,
    /// Graph node the media is collected from. `None` sweeps every node, which
    /// a graph whose loader previews its own input cannot afford.
    node: Option<&'a str>,
    queued: &'a str,
    generating: &'a str,
    saving: &'a str,
}

fn collect_output_files(node: &Value, mode: OutputMode) -> Result<Vec<OutputImage>, Error> {
    let keys = match mode {
        OutputMode::Image => &["images"][..],
        OutputMode::Video => &["videos", "gifs", "images"][..],
        OutputMode::Audio => &["audio"][..],
    };
    let mut files = Vec::new();
    for key in keys {
        let Some(items) = node.get(*key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let file: OutputImage = serde_json::from_value(item.clone())
                .map_err(|_| Error::InvalidResponse("invalid media output"))?;
            match mode {
                OutputMode::Image => {
                    if file.r#type != "temp" {
                        return Err(Error::InvalidResponse(
                            "workflow returned a non-temporary image",
                        ));
                    }
                    files.push(file);
                }
                OutputMode::Video => {
                    if !is_video_filename(&file.filename) {
                        continue;
                    }
                    if file.r#type != "temp" && file.r#type != "output" {
                        return Err(Error::InvalidResponse(
                            "workflow returned an unsupported video location",
                        ));
                    }
                    files.push(file);
                }
                OutputMode::Audio => {
                    if !is_audio_filename(&file.filename) {
                        continue;
                    }
                    if file.r#type != "temp" {
                        return Err(Error::InvalidResponse(
                            "workflow returned a non-temporary audio file",
                        ));
                    }
                    files.push(file);
                }
            }
        }
    }
    Ok(files)
}

fn outputs_from_history_entry(
    status: &str,
    nodes: Option<&serde_json::Map<String, Value>>,
    mode: OutputMode,
    output_node: Option<&str>,
) -> Result<Option<Vec<OutputImage>>, Error> {
    if status == "error" {
        return Err(Error::InvalidResponse("workflow execution failed"));
    }
    let mut files = Vec::new();
    if let Some(nodes) = nodes {
        match output_node {
            Some(id) => {
                if let Some(node) = nodes.get(id) {
                    files.extend(collect_output_files(node, mode)?);
                }
            }
            None => {
                for node in nodes.values() {
                    files.extend(collect_output_files(node, mode)?);
                }
            }
        }
    }
    if files.is_empty() {
        if status == "success" {
            return Err(Error::InvalidResponse(
                "workflow completed without a usable output",
            ));
        }
        return Ok(None);
    }
    Ok(Some(files))
}

impl Client {
    pub fn new(config: Config) -> Result<Self, Error> {
        let client = crate::http::client(&config)?;
        Self::with_http(config, client)
    }

    /// A client sharing `client`'s connections, for a caller that already
    /// holds one for the same ComfyUI.
    pub(crate) fn with_http(config: Config, client: HttpClient) -> Result<Self, Error> {
        config.validate()?;

        if config.base_url.trim().is_empty() {
            return Err(Error::Configuration("COMFYUI_BASE_URL is empty"));
        }
        sanitize_weight_filename(&config.checkpoint).map_err(|_| {
            Error::Configuration("COMFYUI_CHECKPOINT must be a checkpoint filename")
        })?;
        let catalog = RecipeCatalog::load(config.workflow_path.as_deref())?;
        let client = Self {
            config,
            client,
            catalog,
        };
        let _ = client.image_recipe()?;
        Ok(client)
    }

    pub fn prompt_mode(&self) -> PromptMode {
        self.image_recipe()
            .map(|recipe| recipe.prompt_mode)
            .unwrap_or(PromptMode::ClipScene)
    }

    fn image_recipe(&self) -> Result<&Recipe, Error> {
        let selected = self.config.checkpoint.as_str();
        if self.config.models_directory.is_dir() {
            let items = crate::inventory::scan(
                &self.config.models_directory,
                &self.catalog,
                &self.config.contract,
            );
            if let Some(item) = crate::inventory::find(&items, selected)
                && let Some(recipe) = self.catalog.get(&item.recipe_id)
            {
                return Ok(recipe);
            }
            let loras = self.config.models_directory.join("loras");
            let pending =
                crate::inventory::publication_marker(&loras, selected, &self.config.contract)
                    .is_some_and(|marker| std::fs::symlink_metadata(marker).is_ok());
            if pending || std::fs::symlink_metadata(loras.join(selected)).is_ok() {
                return Err(Error::Configuration(
                    "selected LoRA has no complete, coherent sidecar",
                ));
            }
        }
        self.catalog.image_recipe_for(selected)
    }

    fn video_workflows(&self) -> Result<(Value, Value), Error> {
        for (value, message) in [
            (
                self.config.video_unet.as_str(),
                "COMFYUI_VIDEO_UNET must be a diffusion model filename",
            ),
            (
                self.config.video_clip.as_str(),
                "COMFYUI_VIDEO_CLIP must be a text encoder filename",
            ),
            (
                self.config.video_vae.as_str(),
                "COMFYUI_VIDEO_VAE must be a VAE filename",
            ),
        ] {
            if !is_model_filename(value) {
                return Err(Error::Configuration(message));
            }
        }
        let video_workflow = load_video_workflow(self.config.video_workflow_path.as_deref())?;
        validate_video_workflow(&video_workflow)?;
        let image_to_video_workflow =
            load_image_to_video_workflow(self.config.video_workflow_path.as_deref())?;
        validate_image_to_video_workflow(&image_to_video_workflow)?;
        Ok((video_workflow, image_to_video_workflow))
    }

    fn audio_workflow(&self) -> Result<Value, Error> {
        sanitize_weight_filename(&self.config.audio_checkpoint).map_err(|_| {
            Error::Configuration("COMFYUI_AUDIO_CHECKPOINT must be a checkpoint filename")
        })?;
        let workflow = load_audio_workflow(self.config.audio_workflow_path.as_deref())?;
        validate_audio_workflow(&workflow)?;
        Ok(workflow)
    }

    pub async fn generate(
        &self,
        prompt: &str,
        source: Option<&SourceImage>,
        cancel: &mut broadcast::Receiver<()>,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        if !self.config.enabled {
            return Err(Error::Disabled);
        }

        if cancel.try_recv().is_ok() {
            return Err(Error::Cancelled);
        }
        let deadline = tokio::time::Instant::now() + self.config.generation_timeout;
        let prompt = if prompt.trim().is_empty() {
            if source.is_some() {
                "edit this image"
            } else {
                return Err(Error::Configuration("prompt is empty or too long"));
            }
        } else {
            prompt
        };
        let recipe = self.image_recipe()?;
        let weights = recipe.weight_map(&self.config.checkpoint)?;
        let fill_weights: HashMap<&str, &str> = weights
            .iter()
            .map(|(name, filename)| (name.as_str(), filename.as_str()))
            .collect();
        let workflow = if let Some(source) = source {
            let _ = progress.send("Uploading source image...".to_string());
            let uploaded = self.upload_source(source, cancel, deadline).await?;
            recipe.apply(
                Fill::new(
                    prompt,
                    rand::random::<u64>() & i64::MAX as u64,
                    fill_weights,
                )
                .with_source(uploaded.as_str()),
            )?
        } else {
            recipe.apply(Fill::new(
                prompt,
                rand::random::<u64>() & i64::MAX as u64,
                fill_weights,
            ))?
        };
        self.submit_and_collect(
            workflow,
            cancel,
            deadline,
            progress,
            Collection {
                mode: OutputMode::Image,
                node: None,
                queued: "Image queued...",
                generating: "Generating image...",
                saving: "Saving generated image...",
            },
        )
        .await
    }

    pub async fn generate_video(
        &self,
        prompt: &str,
        source: Option<&SourceImage>,
        cancel: &mut broadcast::Receiver<()>,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        if !self.config.enabled {
            return Err(Error::Disabled);
        }

        if cancel.try_recv().is_ok() {
            return Err(Error::Cancelled);
        }
        let (video_workflow, image_to_video_workflow) = self.video_workflows()?;
        let deadline = tokio::time::Instant::now() + self.config.video_generation_timeout;
        let prompt = if prompt.trim().is_empty() {
            if source.is_some() {
                "animate this image"
            } else {
                return Err(Error::Configuration("prompt is empty or too long"));
            }
        } else {
            prompt
        };
        let workflow = if let Some(source) = source {
            let _ = progress.send("Uploading source image...".to_string());
            let uploaded = self.upload_source(source, cancel, deadline).await?;
            configure_wan_image_to_video_workflow(
                image_to_video_workflow,
                prompt,
                &self.config.video_unet,
                &self.config.video_clip,
                &self.config.video_vae,
                rand::random::<u64>() & i64::MAX as u64,
                &uploaded,
            )?
        } else {
            configure_wan_text_to_video_workflow(
                video_workflow,
                prompt,
                &self.config.video_unet,
                &self.config.video_clip,
                &self.config.video_vae,
                rand::random::<u64>() & i64::MAX as u64,
            )?
        };
        self.submit_and_collect(
            workflow,
            cancel,
            deadline,
            progress,
            Collection {
                mode: OutputMode::Video,
                node: None,
                queued: "Video queued...",
                generating: "Generating video...",
                saving: "Saving generated video...",
            },
        )
        .await
    }

    pub async fn upscale_image(
        &self,
        source: &SourceImage,
        cancel: &mut broadcast::Receiver<()>,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        if !self.config.enabled {
            return Err(Error::Disabled);
        }
        if cancel.try_recv().is_ok() {
            return Err(Error::Cancelled);
        }
        let workflow = load_upscale_workflow(self.config.upscale_workflow_path.as_deref())?;
        let deadline = tokio::time::Instant::now() + self.config.upscale_generation_timeout;
        let _ = progress.send("Uploading source image...".to_string());
        let uploaded = self
            .upload_media(
                &source.bytes,
                &source.filename,
                &source.mime,
                cancel,
                deadline,
            )
            .await?;
        let workflow =
            configure_upscale_image_workflow(workflow, &self.config.upscale_model, &uploaded)?;
        self.submit_and_collect(
            workflow,
            cancel,
            deadline,
            progress,
            Collection {
                mode: OutputMode::Image,
                node: Some(UPSCALE_IMAGE_OUTPUT_NODE),
                queued: "Upscale queued...",
                generating: "Upscaling image...",
                saving: "Saving upscaled image...",
            },
        )
        .await
    }

    pub async fn upscale_video(
        &self,
        source: &SourceVideo,
        cancel: &mut broadcast::Receiver<()>,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        if !self.config.enabled {
            return Err(Error::Disabled);
        }
        if cancel.try_recv().is_ok() {
            return Err(Error::Cancelled);
        }
        let workflow = load_upscale_video_workflow(self.config.upscale_workflow_path.as_deref())?;
        let deadline = tokio::time::Instant::now() + self.config.upscale_generation_timeout;
        let _ = progress.send("Uploading source video...".to_string());
        let uploaded = self
            .upload_media(
                &source.bytes,
                &source.filename,
                &source.mime,
                cancel,
                deadline,
            )
            .await?;
        let workflow =
            configure_upscale_video_workflow(workflow, &self.config.upscale_model, &uploaded)?;
        self.submit_and_collect(
            workflow,
            cancel,
            deadline,
            progress,
            Collection {
                mode: OutputMode::Video,
                node: Some(UPSCALE_VIDEO_OUTPUT_NODE),
                queued: "Upscale queued...",
                generating: "Upscaling video...",
                saving: "Saving upscaled video...",
            },
        )
        .await
    }

    pub async fn generate_audio(
        &self,
        prompt: &str,
        cancel: &mut broadcast::Receiver<()>,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        if !self.config.enabled {
            return Err(Error::Disabled);
        }

        if cancel.try_recv().is_ok() {
            return Err(Error::Cancelled);
        }
        let audio_workflow = self.audio_workflow()?;
        let deadline = tokio::time::Instant::now() + self.config.audio_generation_timeout;
        let workflow = configure_ace_step_workflow(
            audio_workflow,
            prompt,
            &self.config.audio_checkpoint,
            rand::random::<u64>() & i64::MAX as u64,
        )?;
        self.submit_and_collect(
            workflow,
            cancel,
            deadline,
            progress,
            Collection {
                mode: OutputMode::Audio,
                node: None,
                queued: "Audio queued...",
                generating: "Generating audio...",
                saving: "Saving generated audio...",
            },
        )
        .await
    }

    async fn submit_and_collect(
        &self,
        workflow: Value,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
        progress: mpsc::UnboundedSender<String>,
        collection: Collection<'_>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        let started = std::time::Instant::now();
        let kind = match collection.mode {
            OutputMode::Image => "image",
            OutputMode::Video => "video",
            OutputMode::Audio => "audio",
        };
        let result = self
            .submit_and_collect_inner(workflow, cancel, deadline, progress, collection)
            .await;
        let status = match &result {
            Ok(_) => "ok",
            Err(Error::Disabled) => "disabled",
            Err(Error::Timeout) => "timeout",
            Err(Error::Cancelled) => "cancelled",
            Err(Error::Http(_)) => "http_error",
            Err(Error::Configuration(_)) => "config_error",
            Err(Error::InvalidResponse(_)) => "invalid_response",
        };
        crate::observe::record(kind, status, started.elapsed());
        result
    }

    async fn submit_and_collect_inner(
        &self,
        workflow: Value,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
        progress: mpsc::UnboundedSender<String>,
        collection: Collection<'_>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        let request = self
            .authorize(self.client.post(format!("{}/prompt", self.config.base_url)))
            .json(&json!({
                "prompt": workflow,
                "client_id": Uuid::new_v4().to_string()
            }));
        let response = self
            .bounded(cancel, deadline, async move {
                request
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<PromptResponse>()
                    .await
            })
            .await?;
        let prompt_id = response.prompt_id;
        let _ = progress.send(collection.queued.to_string());

        let mut announced_generation = false;
        loop {
            tokio::select! {
                biased;
                _ = cancel.recv() => {
                    self.cancel(&prompt_id).await;
                    return Err(Error::Cancelled);
                }
                _ = tokio::time::sleep_until(deadline) => {
                    self.cancel(&prompt_id).await;
                    return Err(Error::Timeout);
                }
                _ = tokio::time::sleep(self.config.poll_interval) => {
                    if !announced_generation {
                        let _ = progress.send(collection.generating.to_string());
                        announced_generation = true;
                    }
                    match self.history_outputs(&prompt_id, cancel, deadline, collection).await {
                        Ok(Some(outputs)) => {
                            let _ = progress.send(collection.saving.to_string());
                            let result = self.fetch_outputs(outputs, cancel, deadline).await;
                            self.clear_history(&prompt_id).await;
                            if matches!(result, Err(Error::Cancelled | Error::Timeout)) {
                                self.cancel(&prompt_id).await;
                            }
                            return result;
                        }
                        Ok(None) => {}
                        Err(error) => {
                            if matches!(error, Error::Cancelled | Error::Timeout) {
                                self.cancel(&prompt_id).await;
                            }
                            return Err(error);
                        }
                    }
                }
            }
        }
    }

    async fn upload_source(
        &self,
        source: &SourceImage,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
    ) -> Result<String, Error> {
        self.upload_media(
            &source.bytes,
            &source.filename,
            &source.mime,
            cancel,
            deadline,
        )
        .await
    }

    async fn upload_media(
        &self,
        bytes: &bytes::Bytes,
        name: &str,
        mime: &str,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
    ) -> Result<String, Error> {
        let filename = sanitize_upload_name(name)?;
        let part = reqwest::multipart::Part::bytes(bytes.to_vec())
            .file_name(filename.clone())
            .mime_str(mime)
            .map_err(|_| Error::Configuration("source media type is not supported"))?;
        let form = reqwest::multipart::Form::new()
            .part("image", part)
            .text("overwrite", "true")
            .text("type", "input");
        let request = self.authorize(
            self.client
                .post(format!("{}/upload/image", self.config.base_url))
                .multipart(form),
        );
        let uploaded = self
            .bounded(cancel, deadline, async move {
                request
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<UploadResponse>()
                    .await
            })
            .await?;
        uploaded_image_name(&uploaded, &filename)
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        crate::http::authorize(&self.config, request)
    }

    async fn bounded<T, F>(
        &self,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
        request: F,
    ) -> Result<T, Error>
    where
        F: Future<Output = Result<T, reqwest::Error>>,
    {
        tokio::select! {
            biased;
            _ = cancel.recv() => Err(Error::Cancelled),
            _ = tokio::time::sleep_until(deadline) => Err(Error::Timeout),
            result = request => result.map_err(Error::Http),
        }
    }

    async fn history_outputs(
        &self,
        prompt_id: &str,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
        collection: Collection<'_>,
    ) -> Result<Option<Vec<OutputImage>>, Error> {
        let request = self.authorize(
            self.client
                .get(format!("{}/history/{}", self.config.base_url, prompt_id)),
        );
        let history = self
            .bounded(cancel, deadline, async move {
                request
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Value>()
                    .await
            })
            .await?;
        let Some(entry) = history.get(prompt_id) else {
            return Ok(None);
        };
        let status = entry
            .pointer("/status/status_str")
            .and_then(Value::as_str)
            .unwrap_or_default();
        outputs_from_history_entry(
            status,
            entry.get("outputs").and_then(Value::as_object),
            collection.mode,
            collection.node,
        )
    }

    async fn fetch_outputs(
        &self,
        outputs: Vec<OutputImage>,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
    ) -> Result<Vec<GeneratedImage>, Error> {
        let mut generated = Vec::with_capacity(outputs.len());
        for output in outputs {
            let filename = output.filename.clone();
            let url = format!(
                "{}/view?filename={}&subfolder={}&type={}",
                self.config.base_url,
                urlencoding::encode(output.filename.as_str()),
                urlencoding::encode(output.subfolder.as_str()),
                urlencoding::encode(output.r#type.as_str()),
            );
            let request = self.authorize(self.client.get(url));
            let (bytes, mime) = self
                .bounded(cancel, deadline, async move {
                    let response = request.send().await?.error_for_status()?;
                    let mime = response
                        .headers()
                        .get(reqwest::header::CONTENT_TYPE)
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.split(';').next())
                        .map(str::trim)
                        .filter(|value| {
                            value.starts_with("image/")
                                || value.starts_with("video/")
                                || value.starts_with("audio/")
                        })
                        .map(str::to_string)
                        .unwrap_or_else(|| mime_for_filename(&filename));
                    Ok((response.bytes().await?, mime))
                })
                .await?;
            generated.push(GeneratedImage {
                bytes,
                mime,
                filename: output.filename,
            });
        }
        Ok(generated)
    }

    async fn cancel(&self, prompt_id: &str) {
        if let Err(error) = self
            .authorize(self.client.post(format!("{}/queue", self.config.base_url)))
            .json(&json!({ "delete": [prompt_id] }))
            .send()
            .await
        {
            tracing::warn!("Failed to cancel ComfyUI prompt {}: {}", prompt_id, error);
        }
        // Never `/interrupt`: it is process-wide and would stop another caller's prompt.
    }

    async fn clear_history(&self, prompt_id: &str) {
        if let Err(error) = self
            .authorize(
                self.client
                    .post(format!("{}/history", self.config.base_url)),
            )
            .json(&json!({ "delete": [prompt_id] }))
            .send()
            .await
        {
            tracing::warn!("Failed to clear ComfyUI history {}: {}", prompt_id, error);
        }
    }
}

/// Build the default image recipe and mutate only prompt, checkpoint, and seed.
pub fn build_flux_schnell_workflow(
    prompt: &str,
    checkpoint: &str,
    seed: u64,
) -> Result<Value, Error> {
    RecipeCatalog::packaged()?
        .image_recipe_for("flux1-schnell-fp8.safetensors")?
        .apply(Fill::new(
            prompt,
            seed,
            HashMap::from([("checkpoint", checkpoint)]),
        ))
}

/// Build the default image-to-image recipe and mutate only approved inputs.
pub fn build_flux_schnell_image_to_image_workflow(
    prompt: &str,
    checkpoint: &str,
    seed: u64,
    image_name: &str,
) -> Result<Value, Error> {
    RecipeCatalog::packaged()?
        .image_recipe_for("flux1-schnell-fp8.safetensors")?
        .apply(
            Fill::new(prompt, seed, HashMap::from([("checkpoint", checkpoint)]))
                .with_source(image_name),
        )
}

fn load_workflow_file(path: &Path) -> Result<Value, Error> {
    let contents = std::fs::read_to_string(path)
        .map_err(|_| Error::Configuration("COMFYUI_WORKFLOW_PATH is not readable"))?;
    serde_json::from_str(&contents)
        .map_err(|_| Error::Configuration("COMFYUI_WORKFLOW_PATH is not valid JSON"))
}

fn configured_file(path: Option<&Path>) -> Option<&Path> {
    path.filter(|path| path.is_file())
}

fn sibling_file(path: Option<&Path>, name: &str) -> Option<PathBuf> {
    path.and_then(Path::parent)
        .map(|directory| directory.join(name))
        .filter(|sibling| sibling.is_file())
}

fn load_video_workflow(path: Option<&Path>) -> Result<Value, Error> {
    if let Some(path) = configured_file(path) {
        return load_workflow_file(path)
            .map_err(|_| Error::Configuration("video workflow path is not readable"));
    }
    serde_json::from_str(PACKAGED_VIDEO_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged video workflow is not valid JSON"))
}

fn load_image_to_video_workflow(text_to_video_path: Option<&Path>) -> Result<Value, Error> {
    if let Some(path) = sibling_file(text_to_video_path, "wan2.2-ti2v-5b-i2v-api.json") {
        return load_workflow_file(&path)
            .map_err(|_| Error::Configuration("image-to-video workflow path is not readable"));
    }
    serde_json::from_str(PACKAGED_IMAGE_TO_VIDEO_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged image-to-video workflow is not valid JSON"))
}

fn load_audio_workflow(path: Option<&Path>) -> Result<Value, Error> {
    if let Some(path) = configured_file(path) {
        return load_workflow_file(path)
            .map_err(|_| Error::Configuration("audio workflow path is not readable"));
    }
    serde_json::from_str(PACKAGED_AUDIO_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged audio workflow is not valid JSON"))
}

/// Build the text-to-video workflow and mutate only approved inputs.
pub fn build_wan_text_to_video_workflow(
    prompt: &str,
    unet: &str,
    clip: &str,
    vae: &str,
    seed: u64,
) -> Result<Value, Error> {
    let workflow = serde_json::from_str(PACKAGED_VIDEO_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged video workflow is not valid JSON"))?;
    configure_wan_text_to_video_workflow(workflow, prompt, unet, clip, vae, seed)
}

/// Build the image-to-video workflow and mutate only approved inputs.
pub fn build_wan_image_to_video_workflow(
    prompt: &str,
    unet: &str,
    clip: &str,
    vae: &str,
    seed: u64,
    image_name: &str,
) -> Result<Value, Error> {
    let workflow = serde_json::from_str(PACKAGED_IMAGE_TO_VIDEO_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged image-to-video workflow is not valid JSON"))?;
    configure_wan_image_to_video_workflow(workflow, prompt, unet, clip, vae, seed, image_name)
}

/// Build the text-to-audio workflow and mutate only approved inputs.
pub fn build_ace_step_workflow(prompt: &str, checkpoint: &str, seed: u64) -> Result<Value, Error> {
    let workflow = serde_json::from_str(PACKAGED_AUDIO_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged audio workflow is not valid JSON"))?;
    configure_ace_step_workflow(workflow, prompt, checkpoint, seed)
}

const UPSCALE_IMAGE_OUTPUT_NODE: &str = "4";
const UPSCALE_VIDEO_OUTPUT_NODE: &str = "5";

fn load_upscale_workflow(path: Option<&Path>) -> Result<Value, Error> {
    if let Some(path) = configured_file(path) {
        return load_workflow_file(path)
            .map_err(|_| Error::Configuration("upscale workflow path is not readable"));
    }
    serde_json::from_str(PACKAGED_UPSCALE_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged upscale workflow is not valid JSON"))
}

fn load_upscale_video_workflow(image_path: Option<&Path>) -> Result<Value, Error> {
    if let Some(path) = sibling_file(image_path, "upscale-video-api.json") {
        return load_workflow_file(&path)
            .map_err(|_| Error::Configuration("video upscale workflow path is not readable"));
    }
    serde_json::from_str(PACKAGED_UPSCALE_VIDEO_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged video upscale workflow is not valid JSON"))
}

/// Build the packaged image upscale workflow and mutate only approved inputs.
pub fn build_upscale_image_workflow(model: &str, image_name: &str) -> Result<Value, Error> {
    let workflow = serde_json::from_str(PACKAGED_UPSCALE_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged upscale workflow is not valid JSON"))?;
    configure_upscale_image_workflow(workflow, model, image_name)
}

/// Build the packaged video upscale workflow and mutate only approved inputs.
pub fn build_upscale_video_workflow(model: &str, video_name: &str) -> Result<Value, Error> {
    let workflow = serde_json::from_str(PACKAGED_UPSCALE_VIDEO_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged video upscale workflow is not valid JSON"))?;
    configure_upscale_video_workflow(workflow, model, video_name)
}

fn validate_upscale_image_workflow(workflow: &Value) -> Result<(), Error> {
    for (pointer, class) in [
        ("/1/class_type", "LoadImage"),
        ("/2/class_type", "UpscaleModelLoader"),
        ("/3/class_type", "ImageUpscaleWithModel"),
        ("/4/class_type", "PreviewImage"),
    ] {
        if workflow.pointer(pointer).and_then(Value::as_str) != Some(class) {
            return Err(Error::Configuration(
                "workflow does not match the image upscale contract",
            ));
        }
    }
    for pointer in ["/1/inputs/image", "/2/inputs/model_name"] {
        if workflow.pointer(pointer).is_none() {
            return Err(Error::Configuration(
                "workflow does not match the image upscale contract",
            ));
        }
    }
    Ok(())
}

fn validate_upscale_video_workflow(workflow: &Value) -> Result<(), Error> {
    for (pointer, class) in [
        ("/1/class_type", "LoadVideo"),
        ("/2/class_type", "GetVideoComponents"),
        ("/3/class_type", "UpscaleModelLoader"),
        ("/4/class_type", "ImageUpscaleWithModel"),
        ("/5/class_type", "SaveWEBM"),
    ] {
        if workflow.pointer(pointer).and_then(Value::as_str) != Some(class) {
            return Err(Error::Configuration(
                "workflow does not match the video upscale contract",
            ));
        }
    }
    for pointer in ["/1/inputs/file", "/3/inputs/model_name", "/5/inputs/fps"] {
        if workflow.pointer(pointer).is_none() {
            return Err(Error::Configuration(
                "workflow does not match the video upscale contract",
            ));
        }
    }
    Ok(())
}

fn configure_upscale_image_workflow(
    mut workflow: Value,
    model: &str,
    image_name: &str,
) -> Result<Value, Error> {
    validate_upscale_image_workflow(&workflow)?;
    if !is_model_filename(model) {
        return Err(Error::Configuration(
            "COMFYUI_UPSCALE_MODEL must be an upscale model filename",
        ));
    }
    workflow["1"]["inputs"]["image"] = json!(sanitize_upload_name(image_name)?);
    workflow["2"]["inputs"]["model_name"] = json!(model);
    Ok(workflow)
}

fn configure_upscale_video_workflow(
    mut workflow: Value,
    model: &str,
    video_name: &str,
) -> Result<Value, Error> {
    validate_upscale_video_workflow(&workflow)?;
    if !is_model_filename(model) {
        return Err(Error::Configuration(
            "COMFYUI_UPSCALE_MODEL must be an upscale model filename",
        ));
    }
    workflow["1"]["inputs"]["file"] = json!(sanitize_upload_name(video_name)?);
    workflow["3"]["inputs"]["model_name"] = json!(model);
    Ok(workflow)
}

fn validate_video_workflow(workflow: &Value) -> Result<(), Error> {
    for pointer in [
        "/1/inputs/unet_name",
        "/2/inputs/clip_name",
        "/3/inputs/vae_name",
        "/5/inputs/text",
        "/7/class_type",
        "/8/inputs/seed",
        "/10/inputs/images",
    ] {
        if workflow.pointer(pointer).is_none() {
            return Err(Error::Configuration(
                "workflow does not match the Wan TI2V contract",
            ));
        }
    }
    if workflow.pointer("/7/class_type").and_then(Value::as_str) != Some("Wan22ImageToVideoLatent")
    {
        return Err(Error::Configuration(
            "video workflow must use Wan22ImageToVideoLatent",
        ));
    }
    if workflow.pointer("/10/class_type").and_then(Value::as_str) != Some("SaveWEBM") {
        return Err(Error::Configuration(
            "video workflow output must use SaveWEBM",
        ));
    }
    Ok(())
}

fn validate_image_to_video_workflow(workflow: &Value) -> Result<(), Error> {
    validate_video_workflow(workflow)?;
    if workflow.pointer("/11/class_type").and_then(Value::as_str) != Some("LoadImage") {
        return Err(Error::Configuration(
            "image-to-video workflow must load a source image",
        ));
    }
    if workflow.pointer("/7/inputs/start_image").is_none() {
        return Err(Error::Configuration(
            "image-to-video workflow must condition on a start image",
        ));
    }
    Ok(())
}

fn validate_audio_workflow(workflow: &Value) -> Result<(), Error> {
    for pointer in [
        "/1/inputs/ckpt_name",
        "/5/inputs/tags",
        "/5/inputs/lyrics",
        "/8/inputs/seed",
        "/10/inputs/audio",
    ] {
        if workflow.pointer(pointer).is_none() {
            return Err(Error::Configuration(
                "workflow does not match the ACE-Step contract",
            ));
        }
    }
    if workflow.pointer("/5/class_type").and_then(Value::as_str) != Some("TextEncodeAceStepAudio") {
        return Err(Error::Configuration(
            "audio workflow must encode the prompt with TextEncodeAceStepAudio",
        ));
    }
    if workflow.pointer("/7/class_type").and_then(Value::as_str) != Some("EmptyAceStepLatentAudio")
    {
        return Err(Error::Configuration(
            "audio workflow must use EmptyAceStepLatentAudio",
        ));
    }
    if workflow.pointer("/9/class_type").and_then(Value::as_str) != Some("VAEDecodeAudio") {
        return Err(Error::Configuration(
            "audio workflow must decode audio with VAEDecodeAudio",
        ));
    }
    if workflow.pointer("/10/class_type").and_then(Value::as_str) != Some("PreviewAudio") {
        return Err(Error::Configuration(
            "audio workflow output must use temporary PreviewAudio storage",
        ));
    }
    Ok(())
}

fn configure_wan_text_to_video_workflow(
    mut workflow: Value,
    prompt: &str,
    unet: &str,
    clip: &str,
    vae: &str,
    seed: u64,
) -> Result<Value, Error> {
    validate_video_workflow(&workflow)?;
    apply_wan_workflow_inputs(&mut workflow, prompt, unet, clip, vae, seed)?;
    Ok(workflow)
}

fn configure_wan_image_to_video_workflow(
    mut workflow: Value,
    prompt: &str,
    unet: &str,
    clip: &str,
    vae: &str,
    seed: u64,
    image_name: &str,
) -> Result<Value, Error> {
    validate_image_to_video_workflow(&workflow)?;
    apply_wan_workflow_inputs(&mut workflow, prompt, unet, clip, vae, seed)?;
    let image_name = sanitize_upload_name(image_name)?;
    workflow["11"]["inputs"]["image"] = json!(image_name);
    Ok(workflow)
}

fn apply_wan_workflow_inputs(
    workflow: &mut Value,
    prompt: &str,
    unet: &str,
    clip: &str,
    vae: &str,
    seed: u64,
) -> Result<(), Error> {
    if prompt.trim().is_empty() || prompt.len() > 100_000 {
        return Err(Error::Configuration("prompt is empty or too long"));
    }
    if !is_model_filename(unet) || !is_model_filename(clip) || !is_model_filename(vae) {
        return Err(Error::Configuration("invalid video model filename"));
    }
    workflow["1"]["inputs"]["unet_name"] = json!(unet);
    workflow["2"]["inputs"]["clip_name"] = json!(clip);
    workflow["3"]["inputs"]["vae_name"] = json!(vae);
    workflow["5"]["inputs"]["text"] = json!(prompt);
    workflow["8"]["inputs"]["seed"] = json!(seed);
    Ok(())
}

fn configure_ace_step_workflow(
    mut workflow: Value,
    prompt: &str,
    checkpoint: &str,
    seed: u64,
) -> Result<Value, Error> {
    validate_audio_workflow(&workflow)?;
    apply_ace_step_workflow_inputs(&mut workflow, prompt, checkpoint, seed)?;
    Ok(workflow)
}

fn apply_ace_step_workflow_inputs(
    workflow: &mut Value,
    prompt: &str,
    checkpoint: &str,
    seed: u64,
) -> Result<(), Error> {
    if prompt.trim().is_empty() || prompt.len() > 100_000 {
        return Err(Error::Configuration("prompt is empty or too long"));
    }
    let checkpoint = sanitize_weight_filename(checkpoint)?;
    workflow["1"]["inputs"]["ckpt_name"] = json!(checkpoint);
    workflow["5"]["inputs"]["tags"] = json!(prompt);
    workflow["5"]["inputs"]["lyrics"] = json!("");
    workflow["8"]["inputs"]["seed"] = json!(seed);
    Ok(())
}

fn is_model_filename(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && !name.contains("..")
}

fn is_video_filename(name: &str) -> bool {
    MediaType::for_filename(name).is_some_and(|media| media.is_video())
}

fn is_audio_filename(name: &str) -> bool {
    MediaType::for_filename(name).is_some_and(|media| media.is_audio())
}

fn mime_for_filename(name: &str) -> String {
    MediaType::for_filename(name)
        .unwrap_or(MediaType::PNG)
        .mime
        .to_string()
}

fn uploaded_image_name(uploaded: &UploadResponse, fallback: &str) -> Result<String, Error> {
    if !uploaded.subfolder.trim().is_empty() {
        return Err(Error::InvalidResponse("upload returned a nested path"));
    }
    let name = if uploaded.name.trim().is_empty() {
        fallback
    } else {
        uploaded.name.as_str()
    };
    sanitize_upload_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    const REQUEST_WAIT: Duration = Duration::from_secs(10);

    #[test]
    fn media_payloads_print_their_size_rather_than_their_bytes() {
        let payload = vec![0x5a_u8; 1 << 20];
        let generated = GeneratedImage {
            bytes: payload.clone().into(),
            mime: "image/png".into(),
            filename: "out.png".into(),
        };
        let image = SourceImage::new(payload.clone(), "image/png").unwrap();
        let video = SourceVideo::new(payload, "video/mp4").unwrap();
        for rendered in [
            format!("{generated:?}"),
            format!("{image:?}"),
            format!("{video:?}"),
        ] {
            assert!(rendered.len() < 200, "{} characters", rendered.len());
            assert!(rendered.contains("bytes: 1048576"), "{rendered}");
        }
    }

    #[test]
    fn workflow_mutates_only_approved_inputs() {
        let workflow =
            build_flux_schnell_workflow("a blue fox", "custom-image.safetensors", 42).unwrap();
        assert_eq!(
            workflow["4"]["inputs"]["ckpt_name"],
            "custom-image.safetensors"
        );
        assert_eq!(workflow["6"]["inputs"]["text"], "a blue fox");
        assert_eq!(workflow["3"]["inputs"]["seed"], 42);
        assert_eq!(workflow["3"]["inputs"]["steps"], 4);
        assert_eq!(workflow["5"]["inputs"]["width"], 1024);
        assert!(workflow.get("10").is_none());
    }

    #[test]
    fn image_to_image_workflow_mutates_only_approved_inputs() {
        let workflow = build_flux_schnell_image_to_image_workflow(
            "make it dusk",
            "custom-image.safetensors",
            42,
            "img2img-source.png",
        )
        .unwrap();
        assert_eq!(
            workflow["4"]["inputs"]["ckpt_name"],
            "custom-image.safetensors"
        );
        assert_eq!(workflow["6"]["inputs"]["text"], "make it dusk");
        assert_eq!(workflow["3"]["inputs"]["seed"], 42);
        assert_eq!(workflow["3"]["inputs"]["denoise"], 0.75);
        assert_eq!(workflow["3"]["inputs"]["steps"], 4);
        assert_eq!(workflow["10"]["inputs"]["image"], "img2img-source.png");
        assert_eq!(workflow["11"]["inputs"]["width"], 1024);
        assert_eq!(workflow["12"]["class_type"], "VAEEncode");
    }

    #[test]
    fn image_to_image_workflow_rejects_pathful_filenames() {
        assert!(
            build_flux_schnell_image_to_image_workflow("fox", "ok.safetensors", 1, "../secret.png")
                .is_err()
        );
        assert!(
            build_flux_schnell_image_to_image_workflow(
                "fox",
                "ok.safetensors",
                1,
                "nested/file.png"
            )
            .is_err()
        );
    }

    #[test]
    fn video_workflow_mutates_only_approved_inputs() {
        let workflow = build_wan_text_to_video_workflow(
            "a moving fox",
            "custom-video.safetensors",
            "custom-clip.safetensors",
            "custom-vae.safetensors",
            42,
        )
        .unwrap();
        assert_eq!(
            workflow["1"]["inputs"]["unet_name"],
            "custom-video.safetensors"
        );
        assert_eq!(
            workflow["2"]["inputs"]["clip_name"],
            "custom-clip.safetensors"
        );
        assert_eq!(
            workflow["3"]["inputs"]["vae_name"],
            "custom-vae.safetensors"
        );
        assert_eq!(workflow["5"]["inputs"]["text"], "a moving fox");
        assert_eq!(workflow["8"]["inputs"]["seed"], 42);
        assert_eq!(workflow["8"]["inputs"]["steps"], 20);
        assert_eq!(workflow["7"]["inputs"]["width"], 832);
        assert!(workflow.get("11").is_none());
    }

    #[test]
    fn image_to_video_workflow_mutates_only_approved_inputs() {
        let workflow = build_wan_image_to_video_workflow(
            "make it move",
            "custom-video.safetensors",
            "custom-clip.safetensors",
            "custom-vae.safetensors",
            42,
            "i2v-source.png",
        )
        .unwrap();
        assert_eq!(
            workflow["1"]["inputs"]["unet_name"],
            "custom-video.safetensors"
        );
        assert_eq!(workflow["5"]["inputs"]["text"], "make it move");
        assert_eq!(workflow["8"]["inputs"]["seed"], 42);
        assert_eq!(workflow["11"]["inputs"]["image"], "i2v-source.png");
        assert_eq!(workflow["7"]["class_type"], "Wan22ImageToVideoLatent");
    }

    #[test]
    fn video_workflow_rejects_pathful_filenames() {
        assert!(
            build_wan_text_to_video_workflow(
                "fox",
                "../secret.safetensors",
                "clip.safetensors",
                "vae.safetensors",
                1
            )
            .is_err()
        );
        assert!(
            build_wan_image_to_video_workflow(
                "fox",
                "ok.safetensors",
                "clip.safetensors",
                "vae.safetensors",
                1,
                "nested/file.png"
            )
            .is_err()
        );
    }

    #[test]
    fn audio_workflow_mutates_only_approved_inputs() {
        let workflow =
            build_ace_step_workflow("forest ambience", "custom-audio.safetensors", 42).unwrap();
        assert_eq!(
            workflow["1"]["inputs"]["ckpt_name"],
            "custom-audio.safetensors"
        );
        assert_eq!(workflow["5"]["inputs"]["tags"], "forest ambience");
        assert_eq!(workflow["8"]["inputs"]["seed"], 42);
        assert_eq!(workflow["8"]["inputs"]["steps"], 50);
        assert_eq!(workflow["7"]["inputs"]["seconds"], 30);
        assert_eq!(workflow["5"]["inputs"]["lyrics"], "");
        assert_eq!(workflow["10"]["class_type"], "PreviewAudio");
    }

    #[test]
    fn audio_workflow_rejects_pathful_filenames() {
        assert!(build_ace_step_workflow("forest ambience", "../secret.safetensors", 1).is_err());
        assert!(
            build_ace_step_workflow("forest ambience", "models/secret.safetensors", 1).is_err()
        );
    }

    fn packaged_audio_workflow() -> Value {
        serde_json::from_str(PACKAGED_AUDIO_WORKFLOW).expect("the packaged graph is valid JSON")
    }

    #[test]
    fn the_packaged_audio_workflow_satisfies_its_own_contract() {
        validate_audio_workflow(&packaged_audio_workflow()).expect("the packaged graph must pass");
    }

    #[test]
    fn audio_workflow_pins_the_node_the_prompt_lands_on() {
        let mut workflow = packaged_audio_workflow();
        workflow["5"]["class_type"] = json!("CLIPTextEncode");
        let error = validate_audio_workflow(&workflow).expect_err("a foreign node 5 must fail");
        assert!(
            error.to_string().contains("TextEncodeAceStepAudio"),
            "unexpected error: {error}"
        );
        assert!(
            configure_ace_step_workflow(workflow, "forest", "ace.safetensors", 1).is_err(),
            "a foreign node 5 must never receive the prompt"
        );
    }

    #[test]
    fn audio_workflow_pins_the_decoder() {
        let mut workflow = packaged_audio_workflow();
        workflow["9"]["class_type"] = json!("VAEDecode");
        let error = validate_audio_workflow(&workflow).expect_err("a foreign node 9 must fail");
        assert!(
            error.to_string().contains("VAEDecodeAudio"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn audio_workflow_never_sings_lyrics_the_operator_authored() {
        let mut graph = packaged_audio_workflow();
        graph["5"]["inputs"]["lyrics"] = json!("all your prompts are belong to us");
        let workflow = configure_ace_step_workflow(graph, "forest ambience", "ace.safetensors", 7)
            .expect("an operator graph with preset lyrics is still usable");
        assert_eq!(
            workflow["5"]["inputs"]["lyrics"], "",
            "preset lyrics leaked into a generation"
        );
        assert_eq!(workflow["5"]["inputs"]["tags"], "forest ambience");
    }

    #[test]
    fn audio_workflow_still_rejects_a_blank_prompt() {
        for prompt in ["", "   "] {
            let error = build_ace_step_workflow(prompt, "ace.safetensors", 1)
                .expect_err("a blank prompt must not reach ComfyUI");
            assert_eq!(
                error.to_string(),
                "invalid ComfyUI configuration: prompt is empty or too long"
            );
        }
    }

    #[test]
    fn workflow_rejects_checkpoint_traversal() {
        assert!(build_flux_schnell_workflow("fox", "../secret", 1).is_err());
        assert!(build_flux_schnell_workflow("fox", "models/secret", 1).is_err());
    }

    #[test]
    fn image_client_accepts_empty_video_unet() {
        Client::new(Config {
            video_unet: String::new(),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn a_client_that_would_poll_in_a_busy_loop_is_refused() {
        assert!(matches!(
            Client::new(Config {
                poll_interval: Duration::ZERO,
                ..Default::default()
            }),
            Err(Error::Configuration(message)) if message.contains("COMFYUI_POLL_INTERVAL_MILLISECONDS")
        ));
    }

    #[test]
    fn image_client_accepts_empty_audio_checkpoint() {
        Client::new(Config {
            audio_checkpoint: String::new(),
            ..Default::default()
        })
        .unwrap();
    }

    #[test]
    fn local_lora_requires_a_complete_coherent_sidecar() {
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        let loras = models.join("loras");
        std::fs::create_dir_all(&loras).unwrap();
        let weight = loras.join("style.safetensors");
        std::fs::write(&weight, b"lora").unwrap();
        let config = Config {
            checkpoint: "style.safetensors".into(),
            models_directory: models,
            ..Default::default()
        };
        assert!(matches!(
            Client::new(config.clone()),
            Err(Error::Configuration(
                "selected LoRA has no complete, coherent sidecar"
            ))
        ));

        crate::inventory::write_sidecar(
            &weight,
            &crate::inventory::WeightSidecar {
                recipe_id: "flux-schnell-adapter".into(),
                huggingface_base: Some("Qwen/Qwen-Image-Edit-2511".into()),
            },
            &crate::train::Contract::default(),
        )
        .unwrap();
        assert!(Client::new(config.clone()).is_err());

        crate::inventory::write_sidecar(
            &weight,
            &crate::inventory::WeightSidecar {
                recipe_id: "flux-schnell-adapter".into(),
                huggingface_base: Some("black-forest-labs/FLUX.1-schnell".into()),
            },
            &crate::train::Contract::default(),
        )
        .unwrap();
        assert!(Client::new(config.clone()).is_ok());

        let marker =
            crate::inventory::publication_marker(&loras, "style.safetensors", &config.contract)
                .unwrap();
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(marker, b"pending").unwrap();
        assert!(Client::new(config).is_err());
    }

    #[test]
    fn mime_for_filename_covers_audio_extensions() {
        assert_eq!(mime_for_filename("clip.flac"), "audio/flac");
        assert_eq!(mime_for_filename("CLIP.MP3"), "audio/mpeg");
        assert_eq!(mime_for_filename("clip.opus"), "audio/ogg");
        assert_eq!(mime_for_filename("clip.wav"), "audio/wav");
    }

    #[test]
    fn successful_history_without_audio_is_an_error() {
        let nodes = json!({
            "10": {"images": [{"filename": "cover.png", "subfolder": "", "type": "temp"}]}
        });
        assert!(matches!(
            outputs_from_history_entry("success", nodes.as_object(), OutputMode::Audio, None),
            Err(Error::InvalidResponse(
                "workflow completed without a usable output"
            ))
        ));
    }

    #[test]
    fn audio_output_from_output_directory_is_rejected() {
        let nodes = json!({
            "10": {"audio": [{"filename": "clip.flac", "subfolder": "", "type": "output"}]}
        });
        assert!(matches!(
            outputs_from_history_entry("success", nodes.as_object(), OutputMode::Audio, None),
            Err(Error::InvalidResponse(
                "workflow returned a non-temporary audio file"
            ))
        ));
    }

    #[test]
    fn successful_history_without_video_is_an_error() {
        let nodes = json!({
            "10": {"images": [{"filename": "still.png", "subfolder": "", "type": "output"}]}
        });
        assert!(matches!(
            outputs_from_history_entry("success", nodes.as_object(), OutputMode::Video, None),
            Err(Error::InvalidResponse(
                "workflow completed without a usable output"
            ))
        ));
    }

    #[test]
    fn incomplete_history_without_files_keeps_polling() {
        assert!(
            outputs_from_history_entry("executing", None, OutputMode::Video, None)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn generate_video_rejects_empty_video_unet() {
        let client = Client::new(Config {
            enabled: true,
            video_unet: String::new(),
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        assert!(matches!(
            client
                .generate_video("a fox", None, &mut cancel_rx, progress_tx)
                .await,
            Err(Error::Configuration(
                "COMFYUI_VIDEO_UNET must be a diffusion model filename"
            ))
        ));
    }

    #[tokio::test]
    async fn generate_audio_rejects_empty_audio_checkpoint() {
        let client = Client::new(Config {
            enabled: true,
            audio_checkpoint: String::new(),
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        assert!(matches!(
            client
                .generate_audio("forest ambience", &mut cancel_rx, progress_tx)
                .await,
            Err(Error::Configuration(
                "COMFYUI_AUDIO_CHECKPOINT must be a checkpoint filename"
            ))
        ));
    }

    #[tokio::test]
    async fn generate_audio_rejects_a_blank_prompt_without_reaching_comfyui() {
        let client = Client::new(Config {
            enabled: true,
            base_url: "http://127.0.0.1:1".to_string(),
            ..Default::default()
        })
        .unwrap();
        for prompt in ["", "   "] {
            let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
            let (progress_tx, _) = mpsc::unbounded_channel();
            assert!(
                matches!(
                    client
                        .generate_audio(prompt, &mut cancel_rx, progress_tx)
                        .await,
                    Err(Error::Configuration("prompt is empty or too long"))
                ),
                "{prompt:?} should be rejected before any request"
            );
        }
    }

    #[tokio::test]
    async fn submits_recovers_history_and_fetches_output() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "p1"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/p1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "p1": {"status": {"status_str": "success"}, "outputs": {
                    "7": {"images": [{"filename": "output.png", "subfolder": "", "type": "temp"}]}
                }}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(vec![1, 2, 3]),
            )
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let images = client
            .generate("a fox", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].bytes.as_ref(), &[1, 2, 3]);
        assert_eq!(images[0].filename, "output.png");
        assert_eq!(progress_rx.recv().await.as_deref(), Some("Image queued..."));
    }

    #[tokio::test]
    async fn the_token_travels_in_whichever_header_the_deployment_checks() {
        for header in [crate::config::TOKEN_HEADER, "Proxy-Token"] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/prompt"))
                .and(wiremock::matchers::header(header, "secret"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "p3"})))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/history/p3"))
                .and(wiremock::matchers::header(header, "secret"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "p3": {"status": {"status_str": "success"}, "outputs": {
                        "7": {"images": [{"filename": "output.png", "subfolder": "", "type": "temp"}]}
                    }}
                })))
                .mount(&server)
                .await;
            Mock::given(method("GET"))
                .and(path("/view"))
                .and(wiremock::matchers::header(header, "secret"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "image/png")
                        .set_body_bytes(vec![1]),
                )
                .mount(&server)
                .await;
            let client = Client::new(Config {
                enabled: true,
                base_url: server.uri(),
                api_token: Some("secret".into()),
                token_header: header.to_string(),
                poll_interval: Duration::from_millis(50),
                ..Default::default()
            })
            .unwrap();
            let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
            let (progress_tx, _) = mpsc::unbounded_channel();
            let images = client
                .generate("a fox", None, &mut cancel_rx, progress_tx)
                .await
                .unwrap_or_else(|error| panic!("{header} was not honoured: {error}"));
            assert_eq!(images[0].bytes.as_ref(), &[1]);
        }
    }

    #[tokio::test]
    async fn adapter_generate_loads_lora_and_keeps_instruction_prompt() {
        let server = MockServer::start().await;
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        let loras = models.join("loras");
        std::fs::create_dir_all(&loras).unwrap();
        let weight = loras.join("qwen-image-edit-plus-nsfw-lora.safetensors");
        std::fs::write(&weight, b"lora").unwrap();
        crate::inventory::write_sidecar(
            &weight,
            &crate::inventory::WeightSidecar {
                recipe_id: "qwen-image-edit-adapter".into(),
                huggingface_base: Some("Qwen/Qwen-Image-Edit-2511".into()),
            },
            &crate::train::Contract::default(),
        )
        .unwrap();
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "LoraLoaderModelOnly",
            ))
            .and(wiremock::matchers::body_string_contains(
                "qwen-image-edit-plus-nsfw-lora.safetensors",
            ))
            .and(wiremock::matchers::body_string_contains("remove the sign"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "lora"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/lora"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "lora": {"status": {"status_str": "success"}, "outputs": {
                    "9": {"images": [{"filename": "edited.png", "subfolder": "", "type": "temp"}]}
                }}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(vec![9, 8, 7]),
            )
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            checkpoint: "qwen-image-edit-plus-nsfw-lora.safetensors".into(),
            models_directory: models,
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            client.prompt_mode(),
            crate::recipe::PromptMode::EditInstruction
        );
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let images = client
            .generate("remove the sign", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images[0].bytes.as_ref(), &[9, 8, 7]);
    }

    #[tokio::test]
    async fn cancellation_deletes_specific_prompt_without_global_interrupt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "p2"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/queue"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(5000),
            ..Default::default()
        })
        .unwrap();
        let (cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            client
                .generate("a fox", None, &mut cancel_rx, progress_tx)
                .await
        });
        let queued = tokio::time::timeout(REQUEST_WAIT, progress_rx.recv())
            .await
            .expect("the client should report the prompt queued before the cancel is sent")
            .expect("the client should report progress, not drop the channel");
        assert_eq!(
            queued, "Image queued...",
            "the queued message must stay the first progress message the client sends"
        );
        cancel_tx.send(()).unwrap();
        assert!(matches!(task.await.unwrap(), Err(Error::Cancelled)));
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .all(|request| request.url.path() != "/interrupt")
        );
    }

    async fn wait_for_request(server: &MockServer, expected: &str) {
        let deadline = tokio::time::Instant::now() + REQUEST_WAIT;
        while tokio::time::Instant::now() < deadline {
            let seen = server
                .received_requests()
                .await
                .expect("the mock server should record the requests it receives")
                .iter()
                .any(|request| request.url.path() == expected);
            if seen {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the client never sent a request to {expected} within {REQUEST_WAIT:?}");
    }

    #[tokio::test]
    async fn stalled_prompt_request_is_cancellable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_secs(30))
                    .set_body_json(json!({"prompt_id": "never"})),
            )
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            request_timeout: Duration::from_secs(60),
            ..Default::default()
        })
        .unwrap();
        let (cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            client
                .generate("a fox", None, &mut cancel_rx, progress_tx)
                .await
        });
        wait_for_request(&server, "/prompt").await;
        cancel_tx.send(()).unwrap();
        assert!(matches!(task.await.unwrap(), Err(Error::Cancelled)));
    }

    #[tokio::test]
    async fn image_to_image_uploads_source_then_submits_encoded_workflow() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "uploaded-source.png",
                "subfolder": "",
                "type": "input"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "uploaded-source.png",
            ))
            .and(wiremock::matchers::body_string_contains("VAEEncode"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "i2i"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/i2i"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "i2i": {"status": {"status_str": "success"}, "outputs": {
                    "9": {"images": [{"filename": "edited.png", "subfolder": "", "type": "temp"}]}
                }}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(vec![9, 8, 7]),
            )
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let source = SourceImage::new(vec![1, 2, 3, 4], "image/png").unwrap();
        let images = client
            .generate("make it dusk", Some(&source), &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images[0].bytes.as_ref(), &[9, 8, 7]);
        assert_eq!(
            progress_rx.recv().await.as_deref(),
            Some("Uploading source image...")
        );
    }

    #[test]
    fn video_upscale_ignores_the_source_clip_the_loader_previews() {
        let nodes = json!({
            "1": {"images": [{"filename": "upscale-in.webm", "subfolder": "", "type": "input"}], "animated": [true]},
            "5": {"images": [{"filename": "upscale_00001_.webm", "subfolder": "", "type": "output"}], "animated": [true]}
        });
        let collected = outputs_from_history_entry(
            "success",
            nodes.as_object(),
            OutputMode::Video,
            Some(UPSCALE_VIDEO_OUTPUT_NODE),
        )
        .unwrap()
        .unwrap();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].filename, "upscale_00001_.webm");

        assert!(matches!(
            outputs_from_history_entry("success", nodes.as_object(), OutputMode::Video, None),
            Err(Error::InvalidResponse(
                "workflow returned an unsupported video location"
            ))
        ));
    }

    #[test]
    fn upscale_workflows_mutate_only_approved_inputs() {
        let image = build_upscale_image_workflow("4x-model.safetensors", "shot.png").unwrap();
        assert_eq!(image["1"]["inputs"]["image"], json!("shot.png"));
        assert_eq!(
            image["2"]["inputs"]["model_name"],
            json!("4x-model.safetensors")
        );
        assert_eq!(image["3"]["class_type"], json!("ImageUpscaleWithModel"));
        assert_eq!(image["4"]["class_type"], json!("PreviewImage"));

        let video = build_upscale_video_workflow("4x-model.safetensors", "clip.webm").unwrap();
        assert_eq!(video["1"]["inputs"]["file"], json!("clip.webm"));
        assert_eq!(
            video["3"]["inputs"]["model_name"],
            json!("4x-model.safetensors")
        );
        assert_eq!(
            video["5"]["inputs"]["fps"],
            json!(["2", 2]),
            "the encoder takes its rate from the source so the clip keeps its timing"
        );
        assert_eq!(video["5"]["class_type"], json!("SaveWEBM"));
    }

    #[test]
    fn the_packaged_upscale_graphs_load_when_no_file_is_configured() {
        let missing = Some(Path::new("/nonexistent/upscale-image-api.json"));
        for unset in [missing, None] {
            assert_eq!(
                load_upscale_workflow(unset).unwrap()["1"]["class_type"],
                json!("LoadImage")
            );
            assert_eq!(
                load_upscale_video_workflow(unset).unwrap()["1"]["class_type"],
                json!("LoadVideo")
            );
        }
        let image = load_upscale_workflow(missing).unwrap();
        assert_eq!(image["1"]["class_type"], json!("LoadImage"));
        assert_eq!(
            image[UPSCALE_IMAGE_OUTPUT_NODE]["class_type"],
            json!("PreviewImage")
        );
        let video = load_upscale_video_workflow(missing).unwrap();
        assert_eq!(video["1"]["class_type"], json!("LoadVideo"));
        assert_eq!(
            video[UPSCALE_VIDEO_OUTPUT_NODE]["class_type"],
            json!("SaveWEBM")
        );
        assert!(validate_upscale_image_workflow(&image).is_ok());
        assert!(validate_upscale_video_workflow(&video).is_ok());
        assert!(validate_upscale_video_workflow(&image).is_err());
        assert!(validate_upscale_image_workflow(&video).is_err());
    }

    #[test]
    fn upscale_workflows_reject_pathful_filenames() {
        assert!(build_upscale_image_workflow("4x-model.safetensors", "../shot.png").is_err());
        assert!(build_upscale_video_workflow("4x-model.safetensors", "sub/clip.webm").is_err());
        assert!(build_upscale_image_workflow("../model.safetensors", "shot.png").is_err());
        assert!(build_upscale_video_workflow(String::new().as_str(), "clip.webm").is_err());
    }

    #[tokio::test]
    async fn upscale_image_uploads_source_then_collects_the_preview() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "uploaded-source.png",
                "subfolder": "",
                "type": "input"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "ImageUpscaleWithModel",
            ))
            .and(wiremock::matchers::body_string_contains(
                "uploaded-source.png",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "u1"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/u1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "u1": {"status": {"status_str": "success"}, "outputs": {
                    "4": {"images": [{"filename": "big.png", "subfolder": "", "type": "temp"}]}
                }}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(vec![9, 9, 9]),
            )
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        })
        .unwrap();
        let source = SourceImage::new(vec![1, 2, 3], "image/png").unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let images = client
            .upscale_image(&source, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].bytes.as_ref(), &[9, 9, 9]);
        assert_eq!(
            progress_rx.recv().await.as_deref(),
            Some("Uploading source image...")
        );
    }

    #[tokio::test]
    async fn upscale_video_uploads_the_clip_and_returns_only_the_encoded_output() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "uploaded-clip.webm",
                "subfolder": "",
                "type": "input"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "GetVideoComponents",
            ))
            .and(wiremock::matchers::body_string_contains(
                "uploaded-clip.webm",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "u2"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/u2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "u2": {"status": {"status_str": "success"}, "outputs": {
                    "1": {"images": [{"filename": "uploaded-clip.webm", "subfolder": "", "type": "input"}], "animated": [true]},
                    "5": {"images": [{"filename": "upscale_00001_.webm", "subfolder": "", "type": "output"}], "animated": [true]}
                }}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "video/webm")
                    .set_body_bytes(vec![4, 5, 6]),
            )
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        })
        .unwrap();
        let source = SourceVideo::new(vec![1, 2, 3], "video/webm").unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _progress_rx) = mpsc::unbounded_channel();
        let videos = client
            .upscale_video(&source, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].mime, "video/webm");
        assert_eq!(videos[0].filename, "upscale_00001_.webm");
    }

    #[tokio::test]
    async fn video_recovers_webm_history_and_fetches_output() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "Wan22ImageToVideoLatent",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "v1"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/v1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "v1": {"status": {"status_str": "success"}, "outputs": {
                    "10": {"gifs": [{"filename": "clip.webm", "subfolder": "", "type": "output"}]}
                }}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "video/webm")
                    .set_body_bytes(vec![1, 2, 3, 4]),
            )
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let videos = client
            .generate_video("a fox running", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(videos.len(), 1);
        assert_eq!(videos[0].bytes.as_ref(), &[1, 2, 3, 4]);
        assert_eq!(videos[0].mime, "video/webm");
        assert_eq!(progress_rx.recv().await.as_deref(), Some("Video queued..."));
    }

    #[tokio::test]
    async fn image_to_video_uploads_source_then_submits_start_image() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "uploaded-source.png",
                "subfolder": "",
                "type": "input"
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "uploaded-source.png",
            ))
            .and(wiremock::matchers::body_string_contains("start_image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "i2v"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/i2v"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "i2v": {"status": {"status_str": "success"}, "outputs": {
                    "10": {"videos": [{"filename": "clip.webm", "subfolder": "", "type": "output"}]}
                }}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "video/webm")
                    .set_body_bytes(vec![9, 8, 7, 6]),
            )
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let source = SourceImage::new(vec![1, 2, 3, 4], "image/png").unwrap();
        let videos = client
            .generate_video("make it move", Some(&source), &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(videos[0].bytes.as_ref(), &[9, 8, 7, 6]);
    }

    #[tokio::test]
    async fn audio_recovers_flac_history_and_fetches_output() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "EmptyAceStepLatentAudio",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "a1"})))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/a1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "a1": {"status": {"status_str": "success"}, "outputs": {
                    "10": {"audio": [{"filename": "clip.flac", "subfolder": "", "type": "temp"}]}
                }}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/flac")
                    .set_body_bytes(vec![4, 3, 2, 1]),
            )
            .mount(&server)
            .await;
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        let clips = client
            .generate_audio("shuffling through a forest", &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].bytes.as_ref(), &[4, 3, 2, 1]);
        assert_eq!(clips[0].mime, "audio/flac");
        assert_eq!(clips[0].filename, "clip.flac");
        assert_eq!(progress_rx.recv().await.as_deref(), Some("Audio queued..."));
    }
}
