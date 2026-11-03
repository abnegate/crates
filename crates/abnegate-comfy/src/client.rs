//! Direct ComfyUI API client. Graphs come from packaged recipes; chat only
//! supplies prompt, optional negative prompt, seed, checkpoint filename, and
//! an optional source image.

mod error;
mod generated_image;
mod prepared;
mod source_image;
mod source_video;

pub use error::Error;
pub use generated_image::GeneratedImage;
pub use source_image::MAXIMUM_SOURCE_IMAGE_BYTES;
pub use source_image::SourceImage;
pub use source_video::MAXIMUM_SOURCE_VIDEO_BYTES;
pub use source_video::SourceVideo;

use prepared::Prepared;

use reqwest::Client as HttpClient;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::config::Config;
use crate::inventory::VideoIdentity;
use crate::inventory::WeightSidecar;
use crate::media::MediaType;
use crate::people;
use crate::recipe::Fill;
use crate::recipe::IMAGE_TO_VIDEO_ADAPTER_WORKFLOW;
use crate::recipe::LORA_LOADER;
use crate::recipe::MAXIMUM_PROMPT_BYTES;
use crate::recipe::MediaKind;
use crate::recipe::PromptMode;
use crate::recipe::Recipe;
use crate::recipe::RecipeCatalog;
use crate::recipe::VIDEO_ADAPTER_WORKFLOW;
use crate::recipe::sanitize_upload_name;
use crate::recipe::sanitize_weight_filename;
use crate::subject::Subject;

const PACKAGED_VIDEO_WORKFLOW: &str = include_str!("../comfyui/workflows/wan2.2-ti2v-5b-api.json");
const PACKAGED_IMAGE_TO_VIDEO_WORKFLOW: &str =
    include_str!("../comfyui/workflows/wan2.2-ti2v-5b-i2v-api.json");
const PACKAGED_VIDEO_ADAPTER_WORKFLOW: &str =
    include_str!("../comfyui/workflows/wan-adapter-api.json");
const PACKAGED_IMAGE_TO_VIDEO_ADAPTER_WORKFLOW: &str =
    include_str!("../comfyui/workflows/wan-adapter-i2v-api.json");
const VIDEO_LORA_NODE: &str = "12";
const PACKAGED_AUDIO_WORKFLOW: &str =
    include_str!("../comfyui/workflows/ace-step-v1-3.5b-api.json");
const PACKAGED_UPSCALE_WORKFLOW: &str = include_str!("../comfyui/workflows/upscale-image-api.json");
const PACKAGED_UPSCALE_VIDEO_WORKFLOW: &str =
    include_str!("../comfyui/workflows/upscale-video-api.json");

/// Runs generation and upscaling workflows on one ComfyUI server.
///
/// Every call submits a workflow, reports progress as it runs, and collects
/// what it produced. A call fails with [`Error::Disabled`] when
/// [`Config::enabled`] is off, [`Error::Cancelled`] once the cancel channel
/// fires, and [`Error::Timeout`] past the configured time limit for its kind
/// of work.
#[derive(Clone)]
pub struct Client {
    config: Config,
    client: HttpClient,
    catalog: Arc<RecipeCatalog>,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    First,
    Refine,
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
    /// A client for the server `config` names, with its recipe catalog
    /// loaded. Fails with [`Error::Configuration`] when a setting cannot
    /// work, such as an empty base URL, a checkpoint that is not a bare
    /// filename, an unreadable catalog, or a checkpoint no image recipe
    /// runs.
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
        let catalog = Arc::new(RecipeCatalog::load(config.workflow_path.as_deref())?);
        let client = Self {
            config,
            client,
            catalog,
        };
        let _ = client.image_recipe()?;
        Ok(client)
    }

    /// How a prompt for the selected checkpoint should be written, falling
    /// back to [`PromptMode::ClipScene`] when no recipe runs it.
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
                if recipe.kind != MediaKind::Image {
                    return Err(Error::Configuration(
                        "selected weight does not generate images",
                    ));
                }
                return Ok(recipe);
            }
            if crate::inventory::publishing(
                &self.config.models_directory,
                selected,
                &self.config.contract,
            ) {
                return Err(Error::Configuration(
                    "selected weight is still being published",
                ));
            }
            let loras = self.config.models_directory.join("loras");
            if std::fs::symlink_metadata(loras.join(selected)).is_ok() {
                return Err(Error::Configuration(
                    "selected LoRA has no complete, coherent sidecar",
                ));
            }
        }
        self.catalog.image_recipe_for(selected)
    }

    fn generation_plan(
        &self,
        has_source: bool,
    ) -> Result<(&Recipe, HashMap<String, String>), Error> {
        let recipe = self.image_recipe()?;
        if has_source {
            let edit = self
                .catalog
                .edit_recipe_for(recipe, &self.config.checkpoint, |candidate| {
                    self.is_installed(candidate)
                });
            if edit.id != recipe.id {
                return Ok((edit, edit.default_weight_map()?));
            }
        }
        Ok((recipe, recipe.weight_map(&self.config.checkpoint)?))
    }

    fn is_installed(&self, recipe: &Recipe) -> bool {
        !recipe.required_files.is_empty()
            && crate::inventory::missing_required(
                &self.config.models_directory,
                &recipe.required_files,
            )
            .is_empty()
    }

    fn video_model_names(&self) -> Result<(), Error> {
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
        Ok(())
    }

    fn video_workflows(&self) -> Result<(Value, Value), Error> {
        self.video_model_names()?;
        let video_workflow = load_video_workflow(self.config.video_workflow_path.as_deref())?;
        validate_video_workflow(&video_workflow)?;
        let image_to_video_workflow =
            load_image_to_video_workflow(self.config.video_workflow_path.as_deref())?;
        validate_image_to_video_workflow(&image_to_video_workflow)?;
        Ok((video_workflow, image_to_video_workflow))
    }

    /// The identity `prompt` names among the weights installed now, read off
    /// the async runtime.
    async fn video_identity(&self, prompt: &str) -> Option<VideoIdentity> {
        let models = self.config.models_directory.clone();
        let catalog = Arc::clone(&self.catalog);
        let contract = self.config.contract.clone();
        let prompt = prompt.to_string();
        tokio::task::spawn_blocking(move || {
            crate::inventory::video_identity_for_prompt(&models, &catalog, &contract, &prompt)
        })
        .await
        .ok()
        .flatten()
    }

    fn audio_workflow(&self) -> Result<Value, Error> {
        sanitize_weight_filename(&self.config.audio_checkpoint).map_err(|_| {
            Error::Configuration("COMFYUI_AUDIO_CHECKPOINT must be a checkpoint filename")
        })?;
        let workflow = load_audio_workflow(self.config.audio_workflow_path.as_deref())?;
        validate_audio_workflow(&workflow)?;
        Ok(workflow)
    }

    /// Generate images from `prompt` with the selected checkpoint's recipe,
    /// editing `source` when one is given. An edit runs on the recipe's
    /// [`edit_recipe`](Recipe::edit_recipe) instead when the selected
    /// checkpoint is one of the recipe's own stock files and the edit
    /// recipe's files are installed; a custom checkpoint keeps its own
    /// weights. A blank prompt is refused unless there is a source, which is
    /// then edited generically.
    ///
    /// A selected weight whose sidecar names a trigger is an identity, and
    /// its generation gets the person extras its recipe supports: a new
    /// picture is posed through the recipe's pose slot with the OpenPose image
    /// in [`Config::poses_directory`] for the pose the prompt names (standing
    /// when it names none), once the recipe's
    /// [`pose_required_files`](Recipe::pose_required_files) are installed; a
    /// recipe that supports [`RecipeExtra::Face`](crate::recipe::RecipeExtra::Face)
    /// steers the face with the reference beside the weight through the
    /// [`Contract::ipadapter_face_node`](crate::train::Contract::ipadapter_face_node)
    /// once its [`face_required_files`](Recipe::face_required_files) are
    /// installed. The embedding its sidecar names leads the prompt either way.
    /// An extra whose files are missing is left out, never an error.
    ///
    /// A recipe that supports [`RecipeExtra::Refine`](crate::recipe::RecipeExtra::Refine),
    /// or [`RecipeExtra::IdentityRefine`](crate::recipe::RecipeExtra::IdentityRefine)
    /// for an identity, runs each result's head through its source graph again
    /// at a low denoise and pastes it back, within the same time limit as the
    /// first pass. A result whose refine pass fails, or that the time limit
    /// cuts short, is returned as it was; once the first pass has its results
    /// only a cancellation fails the call.
    ///
    /// Progress messages are sent on `progress`, and a message on `cancel`
    /// stops the run. [`generate_excluding`](Client::generate_excluding)
    /// also takes a negative prompt.
    pub async fn generate(
        &self,
        prompt: &str,
        source: Option<&SourceImage>,
        cancel: &mut broadcast::Receiver<()>,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        self.generate_excluding(prompt, source, None, cancel, progress)
            .await
    }

    /// [`generate`](Client::generate), with `negative` written into the
    /// recipe's negative prompt slot: what the picture must not show, kept
    /// out of the prompt so the prompt describes only the scene. `None` keeps
    /// the negative text the recipe's graph ships with, and `Some("")` clears
    /// it. The face refine pass is given the same negative.
    /// [`Error::Configuration`] when `negative` is over 100,000 bytes or the
    /// recipe has no negative slot.
    pub async fn generate_excluding(
        &self,
        prompt: &str,
        source: Option<&SourceImage>,
        negative: Option<&str>,
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
        let (sidecar, refines) = self
            .blocking(|client| {
                let sidecar = client.selected_sidecar();
                let refines = people::refines(client.image_recipe()?, sidecar.as_ref());
                Ok((sidecar, refines))
            })
            .await?;
        let images = self
            .generate_image(
                prompt,
                negative,
                source,
                sidecar.as_ref(),
                Pass::First,
                cancel,
                deadline,
                progress.clone(),
            )
            .await?;
        if !refines {
            return Ok(images);
        }
        let subject = Subject::shared(&self.config).await;
        let mut refined = Vec::with_capacity(images.len());
        for image in images {
            if tokio::time::Instant::now() >= deadline {
                refined.push(image);
                continue;
            }
            refined.push(
                self.refine(
                    prompt,
                    negative,
                    image,
                    sidecar.as_ref(),
                    &subject,
                    cancel,
                    deadline,
                    progress.clone(),
                )
                .await?,
            );
        }
        Ok(refined)
    }

    /// Runs `work` on the blocking pool with a clone of this client, for the
    /// filesystem reads a generation makes before it submits anything.
    async fn blocking<T, F>(&self, work: F) -> Result<T, Error>
    where
        F: FnOnce(&Client) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        let client = self.clone();
        match tokio::task::spawn_blocking(move || work(&client)).await {
            Ok(result) => result,
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            Err(_) => Err(Error::Cancelled),
        }
    }

    /// Reads what `pass` needs from disk: the recipe and weights it runs, and
    /// for a first pass the pose and face images of the extras it gets.
    fn prepare(
        &self,
        prompt: &str,
        has_source: bool,
        sidecar: Option<&WeightSidecar>,
        pass: Pass,
    ) -> Result<Prepared, Error> {
        let (recipe, weights) = self.generation_plan(has_source)?;
        let extras = match pass {
            Pass::First => people::extras(&self.config, recipe, sidecar, has_source),
            Pass::Refine => people::Extras::default(),
        };
        let pose = extras
            .pose
            .then_some(self.config.poses_directory.as_deref())
            .flatten()
            .and_then(|directory| people::pose_image(directory, prompt));
        let face = extras
            .face
            .then(|| {
                crate::inventory::face_path(
                    &self.config.models_directory,
                    &self.config.checkpoint,
                    sidecar,
                )
            })
            .flatten()
            .and_then(|path| crate::inventory::read_face(&path))
            .and_then(|bytes| SourceImage::from_bytes(bytes).ok());
        Ok(Prepared {
            recipe: recipe.clone(),
            weights,
            pose,
            face,
        })
    }

    fn selected_sidecar(&self) -> Option<WeightSidecar> {
        crate::inventory::sidecar_for_weight(
            &self.config.models_directory,
            &self.config.checkpoint,
            &self.config.contract,
        )
    }

    async fn generate_image(
        &self,
        prompt: &str,
        negative: Option<&str>,
        source: Option<&SourceImage>,
        sidecar: Option<&WeightSidecar>,
        pass: Pass,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        let prompt = people::prefix_embedding(prompt, sidecar);
        let prepared = {
            let prompt = prompt.clone();
            let sidecar = sidecar.cloned();
            let has_source = source.is_some();
            self.blocking(move |client| client.prepare(&prompt, has_source, sidecar.as_ref(), pass))
                .await?
        };
        let recipe = &prepared.recipe;
        let pose = match prepared.pose {
            Some(pose) => Some(
                self.upload_media(
                    &bytes::Bytes::from(pose.bytes),
                    &pose.name,
                    MediaType::PNG.mime,
                    cancel,
                    deadline,
                )
                .await?,
            ),
            None => None,
        };
        let face = match &prepared.face {
            Some(face) => Some(self.upload_source(face, cancel, deadline).await?),
            None => None,
        };
        let fill_weights: HashMap<&str, &str> = prepared
            .weights
            .iter()
            .map(|(name, filename)| (name.as_str(), filename.as_str()))
            .collect();
        let mut fill = Fill::new(
            &prompt,
            rand::random::<u64>() & i64::MAX as u64,
            fill_weights,
        );
        let uploaded;
        if let Some(source) = source {
            if pass == Pass::First {
                let _ = progress.send("Uploading source image...".to_string());
            }
            uploaded = self.upload_source(source, cancel, deadline).await?;
            fill = fill.with_source(&uploaded);
        }
        if let Some(pose) = &pose {
            fill = fill.with_pose(pose);
        }
        if let Some(negative) = negative {
            fill = fill.with_negative(negative);
        }
        let mut workflow = recipe.apply(fill)?;
        if let Some(face) = &face {
            people::attach_face(
                &mut workflow,
                recipe,
                face,
                &self.config.contract.ipadapter_face_node,
            )?;
        }
        if pass == Pass::Refine {
            people::set_denoise(&mut workflow, recipe, people::REFINE_DENOISE)?;
        }
        let collection = match pass {
            Pass::First => Collection {
                mode: OutputMode::Image,
                node: None,
                queued: "Image queued...",
                generating: "Generating image...",
                saving: "Saving generated image...",
            },
            Pass::Refine => Collection {
                mode: OutputMode::Image,
                node: None,
                queued: "Face refine queued...",
                generating: "Refining the face...",
                saving: "Saving the refined image...",
            },
        };
        self.submit_and_collect(workflow, cancel, deadline, progress, collection)
            .await
    }

    async fn refine(
        &self,
        prompt: &str,
        negative: Option<&str>,
        image: GeneratedImage,
        sidecar: Option<&WeightSidecar>,
        subject: &Subject,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<GeneratedImage, Error> {
        let original = image.bytes.clone();
        let cropper = subject.clone();
        let crop = tokio::task::spawn_blocking(move || people::head_crop(&original, &cropper))
            .await
            .ok()
            .flatten();
        let Some(crop) = crop else {
            return Ok(image);
        };
        let Ok(head) = SourceImage::from_bytes(crop.png) else {
            return Ok(image);
        };
        let refined = match self
            .generate_image(
                prompt,
                negative,
                Some(&head),
                sidecar,
                Pass::Refine,
                cancel,
                deadline,
                progress,
            )
            .await
        {
            Ok(outputs) => outputs.into_iter().next(),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(error) => {
                tracing::warn!(%error, "face refine failed; keeping the unrefined image");
                None
            }
        };
        let Some(refined) = refined else {
            return Ok(image);
        };
        if crop.full_frame {
            return Ok(refined);
        }
        let original = image.bytes.clone();
        let region = crop.region;
        let pasted =
            tokio::task::spawn_blocking(move || people::paste(&original, &refined.bytes, region))
                .await
                .ok()
                .flatten();
        Ok(match pasted {
            Some(bytes) => GeneratedImage {
                bytes: bytes.into(),
                mime: MediaType::PNG.mime.to_string(),
                filename: image.filename,
            },
            None => image,
        })
    }

    /// Generate a video from `prompt`, animating `source` when one is given.
    /// A blank prompt is refused unless there is a source.
    ///
    /// A prompt that names a trained identity's trigger, as
    /// [`inventory::video_identity_for_prompt`](crate::inventory::video_identity_for_prompt)
    /// finds it, keeps the trigger in front, and when a video LoRA was
    /// trained on that trigger the run loads it through the adapter graph,
    /// `wan-adapter-api.json` or `wan-adapter-i2v-api.json` beside
    /// [`Config::video_workflow_path`] or else the packaged one.
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
        self.video_model_names()?;
        let deadline = tokio::time::Instant::now() + self.config.video_generation_timeout;
        let mut prompt = if prompt.trim().is_empty() {
            if source.is_some() {
                "animate this image".to_string()
            } else {
                return Err(Error::Configuration("prompt is empty or too long"));
            }
        } else {
            prompt.to_string()
        };
        let identity = self.video_identity(&prompt).await;
        if let Some(identity) = &identity {
            prompt = crate::lora::identity_caption(&prompt, &identity.trigger);
        }
        let lora = identity.and_then(|identity| identity.lora);
        let path = self.config.video_workflow_path.as_deref();
        let seed = rand::random::<u64>() & i64::MAX as u64;
        let mut workflow = if let Some(source) = source {
            let graph = match &lora {
                Some(_) => load_image_to_video_adapter_workflow(path)?,
                None => self.video_workflows()?.1,
            };
            let _ = progress.send("Uploading source image...".to_string());
            let uploaded = self.upload_source(source, cancel, deadline).await?;
            configure_wan_image_to_video_workflow(
                graph,
                &prompt,
                &self.config.video_unet,
                &self.config.video_clip,
                &self.config.video_vae,
                seed,
                &uploaded,
            )?
        } else {
            let graph = match &lora {
                Some(_) => load_video_adapter_workflow(path)?,
                None => self.video_workflows()?.0,
            };
            configure_wan_text_to_video_workflow(
                graph,
                &prompt,
                &self.config.video_unet,
                &self.config.video_clip,
                &self.config.video_vae,
                seed,
            )?
        };
        if let Some(lora) = &lora {
            apply_video_lora(&mut workflow, lora)?;
        }
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

    /// Upscale `source` with the configured upscale model.
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

    /// Upscale every frame of `source` with the configured upscale model.
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

    /// Generate audio from `prompt` with the configured audio checkpoint.
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

fn load_video_adapter_workflow(text_to_video_path: Option<&Path>) -> Result<Value, Error> {
    if let Some(path) = sibling_file(text_to_video_path, VIDEO_ADAPTER_WORKFLOW) {
        return load_workflow_file(&path)
            .map_err(|_| Error::Configuration("video adapter workflow path is not readable"));
    }
    serde_json::from_str(PACKAGED_VIDEO_ADAPTER_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged video adapter workflow is not valid JSON"))
}

fn load_image_to_video_adapter_workflow(text_to_video_path: Option<&Path>) -> Result<Value, Error> {
    if let Some(path) = sibling_file(text_to_video_path, IMAGE_TO_VIDEO_ADAPTER_WORKFLOW) {
        return load_workflow_file(&path).map_err(|_| {
            Error::Configuration("image-to-video adapter workflow path is not readable")
        });
    }
    serde_json::from_str(PACKAGED_IMAGE_TO_VIDEO_ADAPTER_WORKFLOW).map_err(|_| {
        Error::Configuration("packaged image-to-video adapter workflow is not valid JSON")
    })
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

/// Build the text-to-video workflow that loads the video LoRA `lora` and
/// mutate only approved inputs.
pub fn build_wan_adapter_text_to_video_workflow(
    prompt: &str,
    unet: &str,
    clip: &str,
    vae: &str,
    seed: u64,
    lora: &str,
) -> Result<Value, Error> {
    let workflow = serde_json::from_str(PACKAGED_VIDEO_ADAPTER_WORKFLOW)
        .map_err(|_| Error::Configuration("packaged video adapter workflow is not valid JSON"))?;
    let mut workflow =
        configure_wan_text_to_video_workflow(workflow, prompt, unet, clip, vae, seed)?;
    apply_video_lora(&mut workflow, lora)?;
    Ok(workflow)
}

/// Build the image-to-video workflow that loads the video LoRA `lora` and
/// mutate only approved inputs.
pub fn build_wan_adapter_image_to_video_workflow(
    prompt: &str,
    unet: &str,
    clip: &str,
    vae: &str,
    seed: u64,
    image_name: &str,
    lora: &str,
) -> Result<Value, Error> {
    let workflow =
        serde_json::from_str(PACKAGED_IMAGE_TO_VIDEO_ADAPTER_WORKFLOW).map_err(|_| {
            Error::Configuration("packaged image-to-video adapter workflow is not valid JSON")
        })?;
    let mut workflow =
        configure_wan_image_to_video_workflow(workflow, prompt, unet, clip, vae, seed, image_name)?;
    apply_video_lora(&mut workflow, lora)?;
    Ok(workflow)
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

/// Points the adapter graph's LoRA loader at `lora`.
fn apply_video_lora(workflow: &mut Value, lora: &str) -> Result<(), Error> {
    let class = format!("/{VIDEO_LORA_NODE}/class_type");
    if workflow.pointer(&class).and_then(Value::as_str) != Some(LORA_LOADER) {
        return Err(Error::Configuration(
            "adapter workflow must load a LoRA with LoraLoaderModelOnly",
        ));
    }
    let lora = sanitize_weight_filename(lora)
        .map_err(|_| Error::Configuration("invalid video LoRA filename"))?;
    workflow[VIDEO_LORA_NODE]["inputs"]["lora_name"] = json!(lora);
    Ok(())
}

fn apply_wan_workflow_inputs(
    workflow: &mut Value,
    prompt: &str,
    unet: &str,
    clip: &str,
    vae: &str,
    seed: u64,
) -> Result<(), Error> {
    if prompt.trim().is_empty() || prompt.len() > MAXIMUM_PROMPT_BYTES {
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
    if prompt.trim().is_empty() || prompt.len() > MAXIMUM_PROMPT_BYTES {
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
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

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
        assert_eq!(workflow["3"]["inputs"]["denoise"], 0.6);
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
    fn the_adapter_graph_loads_the_video_lora_and_the_bare_graph_loads_none() {
        let bare = build_wan_text_to_video_workflow(
            "a moving fox",
            "custom-video.safetensors",
            "custom-clip.safetensors",
            "custom-vae.safetensors",
            42,
        )
        .unwrap();
        assert!(bare.get(VIDEO_LORA_NODE).is_none());
        assert_eq!(bare["4"]["inputs"]["model"], json!(["1", 0]));

        let adapted = build_wan_adapter_text_to_video_workflow(
            "a moving fox",
            "custom-video.safetensors",
            "custom-clip.safetensors",
            "custom-vae.safetensors",
            42,
            "jerry-wan.safetensors",
        )
        .unwrap();
        assert_eq!(adapted["12"]["class_type"], "LoraLoaderModelOnly");
        assert_eq!(
            adapted["12"]["inputs"]["lora_name"],
            "jerry-wan.safetensors"
        );
        assert_eq!(adapted["4"]["inputs"]["model"], json!(["12", 0]));
        assert_eq!(adapted["5"]["inputs"]["text"], "a moving fox");
        assert_eq!(
            adapted["1"]["inputs"]["unet_name"],
            "custom-video.safetensors"
        );

        let image_to_video = build_wan_adapter_image_to_video_workflow(
            "make it move",
            "custom-video.safetensors",
            "custom-clip.safetensors",
            "custom-vae.safetensors",
            7,
            "i2v-source.png",
            "jerry-wan.safetensors",
        )
        .unwrap();
        assert_eq!(
            image_to_video["12"]["inputs"]["lora_name"],
            "jerry-wan.safetensors"
        );
        assert_eq!(image_to_video["11"]["inputs"]["image"], "i2v-source.png");

        for lora in ["../jerry-wan.safetensors", "nested/jerry.safetensors", ""] {
            assert!(
                build_wan_adapter_text_to_video_workflow(
                    "fox",
                    "video.safetensors",
                    "clip.safetensors",
                    "vae.safetensors",
                    1,
                    lora,
                )
                .is_err(),
                "{lora:?}"
            );
        }
        let mut unloaded = bare;
        assert!(matches!(
            apply_video_lora(&mut unloaded, "jerry-wan.safetensors"),
            Err(Error::Configuration(
                "adapter workflow must load a LoRA with LoraLoaderModelOnly"
            ))
        ));
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
            &crate::inventory::WeightSidecar::new("flux-schnell-adapter")
                .with_huggingface_base("Qwen/Qwen-Image-Edit-2511"),
            &crate::train::Contract::default(),
        )
        .unwrap();
        assert!(Client::new(config.clone()).is_err());

        crate::inventory::write_sidecar(
            &weight,
            &crate::inventory::WeightSidecar::new("flux-schnell-adapter")
                .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
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
    fn a_video_lora_is_not_selected_for_images() {
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        for (directory, filename) in [
            ("diffusion_models", "wan2.2_ti2v_5B_fp16.safetensors"),
            ("text_encoders", "umt5_xxl_fp8_e4m3fn_scaled.safetensors"),
            ("vae", "wan2.2_vae.safetensors"),
            ("loras", "jerry-wan.safetensors"),
        ] {
            std::fs::create_dir_all(models.join(directory)).unwrap();
            std::fs::write(models.join(directory).join(filename), b"wan").unwrap();
        }
        crate::inventory::write_sidecar(
            &models.join("loras/jerry-wan.safetensors"),
            &crate::inventory::WeightSidecar::new("wan-adapter")
                .with_huggingface_base("Comfy-Org/Wan_2.2_ComfyUI_Repackaged")
                .with_trigger("ohwx"),
            &crate::train::Contract::default(),
        )
        .unwrap();
        let config = Config {
            checkpoint: "jerry-wan.safetensors".into(),
            models_directory: models,
            ..Default::default()
        };
        assert!(matches!(
            Client::new(config),
            Err(Error::Configuration(
                "selected weight does not generate images"
            ))
        ));
    }

    #[test]
    fn a_checkpoint_mid_publication_is_not_selected() {
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        let checkpoints = models.join("checkpoints");
        std::fs::create_dir_all(&checkpoints).unwrap();
        let weight = checkpoints.join("jerry.safetensors");
        std::fs::write(&weight, b"tuned").unwrap();
        crate::inventory::write_sidecar(
            &weight,
            &crate::inventory::WeightSidecar::new("flux-schnell"),
            &crate::train::Contract::default(),
        )
        .unwrap();
        let config = Config {
            checkpoint: "jerry.safetensors".into(),
            models_directory: models,
            ..Default::default()
        };
        assert!(Client::new(config.clone()).is_ok());

        let marker = crate::inventory::publication_marker(
            &checkpoints,
            "jerry.safetensors",
            &config.contract,
        )
        .unwrap();
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(marker, b"pending").unwrap();
        assert!(
            Client::new(config).is_err(),
            "a checkpoint mid-publication was selected"
        );
    }

    fn models_with(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        for (directory, filename) in files {
            let folder = models.join(directory);
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join(filename), filename.as_bytes()).unwrap();
        }
        (root, models)
    }

    #[test]
    fn edits_prefer_installed_flux_dev_over_schnell() {
        let (_root, models) = models_with(&[
            ("checkpoints", "flux1-schnell-fp8.safetensors"),
            ("checkpoints", "flux1-dev-fp8.safetensors"),
        ]);
        let client = Client::new(Config {
            checkpoint: "flux1-schnell-fp8.safetensors".into(),
            models_directory: models,
            ..Default::default()
        })
        .unwrap();
        let (recipe, weights) = client.generation_plan(true).unwrap();
        assert_eq!(recipe.id, "flux-dev");
        assert_eq!(
            weights.get("checkpoint").map(String::as_str),
            Some("flux1-dev-fp8.safetensors")
        );
        let (recipe, _) = client.generation_plan(false).unwrap();
        assert_eq!(recipe.id, "flux-schnell");
    }

    #[test]
    fn edits_stay_on_schnell_when_dev_is_missing() {
        let (_root, models) = models_with(&[("checkpoints", "flux1-schnell-fp8.safetensors")]);
        let client = Client::new(Config {
            checkpoint: "flux1-schnell-fp8.safetensors".into(),
            models_directory: models,
            ..Default::default()
        })
        .unwrap();
        let (recipe, weights) = client.generation_plan(true).unwrap();
        assert_eq!(recipe.id, "flux-schnell");
        assert_eq!(
            weights.get("checkpoint").map(String::as_str),
            Some("flux1-schnell-fp8.safetensors")
        );
    }

    #[test]
    fn edits_stay_on_schnell_without_a_models_directory() {
        let client = Client::new(Config {
            checkpoint: "flux1-schnell-fp8.safetensors".into(),
            models_directory: PathBuf::from("/nonexistent/models"),
            ..Default::default()
        })
        .unwrap();
        let (recipe, _) = client.generation_plan(true).unwrap();
        assert_eq!(recipe.id, "flux-schnell");
    }

    #[test]
    fn a_schnell_lora_edit_does_not_switch_to_dev() {
        let (_root, models) = models_with(&[
            ("checkpoints", "flux1-schnell-fp8.safetensors"),
            ("checkpoints", "flux1-dev-fp8.safetensors"),
            ("loras", "identity.safetensors"),
        ]);
        crate::inventory::write_sidecar(
            &models.join("loras/identity.safetensors"),
            &crate::inventory::WeightSidecar::new("flux-schnell-adapter")
                .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
            &crate::train::Contract::default(),
        )
        .unwrap();
        let client = Client::new(Config {
            checkpoint: "identity.safetensors".into(),
            models_directory: models,
            ..Default::default()
        })
        .unwrap();
        let (recipe, weights) = client.generation_plan(true).unwrap();
        assert_eq!(recipe.id, "flux-schnell-adapter");
        assert_eq!(
            weights.get("lora").map(String::as_str),
            Some("identity.safetensors")
        );
    }

    /// A fine-tune its sidecar binds to schnell is the person's own weight;
    /// swapping it for flux-dev's defaults would edit with someone else's
    /// model.
    #[test]
    fn a_custom_schnell_checkpoint_edit_keeps_its_own_weights() {
        let (_root, models) = models_with(&[
            ("checkpoints", "flux1-schnell-fp8.safetensors"),
            ("checkpoints", "flux1-dev-fp8.safetensors"),
            ("checkpoints", "jerry.safetensors"),
        ]);
        crate::inventory::write_sidecar(
            &models.join("checkpoints/jerry.safetensors"),
            &crate::inventory::WeightSidecar::new("flux-schnell")
                .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
            &crate::train::Contract::default(),
        )
        .unwrap();
        let client = Client::new(Config {
            checkpoint: "jerry.safetensors".into(),
            models_directory: models,
            ..Default::default()
        })
        .unwrap();
        let (recipe, weights) = client.generation_plan(true).unwrap();
        assert_eq!(recipe.id, "flux-schnell");
        assert_eq!(
            weights.get("checkpoint").map(String::as_str),
            Some("jerry.safetensors")
        );
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
    async fn generate_excluding_writes_the_negative_prompt() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(500))
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
        let _ = client
            .generate_excluding("a fox", None, Some("hats"), &mut cancel_rx, progress_tx)
            .await;
        let bodies = prompt_bodies(&server).await;
        let submitted: Value =
            serde_json::from_str(&bodies[0]).expect("ComfyUI is submitted a JSON prompt");
        assert_eq!(submitted["prompt"]["7"]["inputs"]["text"], "hats");
        assert_eq!(submitted["prompt"]["6"]["inputs"]["text"], "a fox");
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
            &crate::inventory::WeightSidecar::new("qwen-image-edit-adapter")
                .with_huggingface_base("Qwen/Qwen-Image-Edit-2511"),
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

    #[tokio::test]
    async fn an_edit_submits_the_installed_flux_dev_graph() {
        let (_root, models) = models_with(&[
            ("checkpoints", "flux1-schnell-fp8.safetensors"),
            ("checkpoints", "flux1-dev-fp8.safetensors"),
        ]);
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
                "flux1-dev-fp8.safetensors",
            ))
            .and(wiremock::matchers::body_string_contains("FluxGuidance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "dev"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/dev"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "dev": {"status": {"status_str": "success"}, "outputs": {
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
            checkpoint: "flux1-schnell-fp8.safetensors".into(),
            models_directory: models,
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _progress_rx) = mpsc::unbounded_channel();
        let source = SourceImage::new(vec![1, 2, 3, 4], "image/png").unwrap();
        let images = client
            .generate("make it dusk", Some(&source), &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images[0].bytes.as_ref(), &[9, 8, 7]);
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

    struct BodyLacks(&'static str);

    impl wiremock::Match for BodyLacks {
        fn matches(&self, request: &wiremock::Request) -> bool {
            !String::from_utf8_lossy(&request.body).contains(self.0)
        }
    }

    fn write_still_identity(models: &Path, trigger: &str) {
        let lora = models.join("loras/jerry.safetensors");
        std::fs::create_dir_all(lora.parent().unwrap()).unwrap();
        std::fs::write(&lora, b"lora").unwrap();
        crate::inventory::write_sidecar(
            &lora,
            &crate::inventory::WeightSidecar::new("flux-schnell-adapter")
                .with_huggingface_base("black-forest-labs/FLUX.1-schnell")
                .with_trigger(trigger),
            &crate::train::Contract::default(),
        )
        .unwrap();
        std::fs::create_dir_all(models.join("checkpoints")).unwrap();
        std::fs::write(
            models.join("checkpoints/flux1-schnell-fp8.safetensors"),
            b"checkpoint",
        )
        .unwrap();
    }

    fn write_video_identity(models: &Path, trigger: &str) {
        for (directory, filename) in [
            ("diffusion_models", "wan2.2_ti2v_5B_fp16.safetensors"),
            ("text_encoders", "umt5_xxl_fp8_e4m3fn_scaled.safetensors"),
            ("vae", "wan2.2_vae.safetensors"),
            ("loras", "jerry-wan.safetensors"),
        ] {
            std::fs::create_dir_all(models.join(directory)).unwrap();
            std::fs::write(models.join(directory).join(filename), b"wan").unwrap();
        }
        crate::inventory::write_sidecar(
            &models.join("loras/jerry-wan.safetensors"),
            &crate::inventory::WeightSidecar::new("wan-adapter")
                .with_huggingface_base("Comfy-Org/Wan_2.2_ComfyUI_Repackaged")
                .with_trigger(trigger),
            &crate::train::Contract::default(),
        )
        .unwrap();
    }

    async fn serve_video(server: &MockServer, prompt_id: &str) {
        let mut history = serde_json::Map::new();
        history.insert(
            prompt_id.to_string(),
            json!({"status": {"status_str": "success"}, "outputs": {
                "10": {"gifs": [{"filename": "clip.webm", "subfolder": "", "type": "output"}]}
            }}),
        );
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(Value::Object(history)))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "video/webm")
                    .set_body_bytes(vec![1, 2, 3, 4]),
            )
            .mount(server)
            .await;
    }

    async fn generate_video_with(server: &MockServer, models: PathBuf, prompt: &str) {
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            models_directory: models,
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let videos = client
            .generate_video(prompt, None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(videos[0].bytes.as_ref(), &[1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn a_video_prompt_naming_a_still_identity_keeps_the_bare_graph() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("ohwx"))
            .and(BodyLacks("LoraLoaderModelOnly"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "still"})))
            .expect(1)
            .mount(&server)
            .await;
        serve_video(&server, "still").await;
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        write_still_identity(&models, "ohwx");

        generate_video_with(&server, models, "ohwx walking on the beach").await;
    }

    #[tokio::test]
    async fn a_video_prompt_naming_a_video_identity_loads_its_lora() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "LoraLoaderModelOnly",
            ))
            .and(wiremock::matchers::body_string_contains(
                "jerry-wan.safetensors",
            ))
            .and(wiremock::matchers::body_string_contains("ohwx"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "wan"})))
            .expect(1)
            .mount(&server)
            .await;
        serve_video(&server, "wan").await;
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        write_still_identity(&models, "ohwx");
        write_video_identity(&models, "ohwx");

        generate_video_with(&server, models, "ohwx walking on the beach").await;
    }

    #[tokio::test]
    async fn a_video_adapter_graph_beside_the_configured_one_is_used_over_the_packaged_one() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "Local Wan Adapter",
            ))
            .and(wiremock::matchers::body_string_contains(
                "jerry-wan.safetensors",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "local"})))
            .expect(1)
            .mount(&server)
            .await;
        serve_video(&server, "local").await;
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        write_video_identity(&models, "ohwx");
        let workflows = root.path().join("workflows");
        std::fs::create_dir_all(&workflows).unwrap();
        let mut local: Value = serde_json::from_str(PACKAGED_VIDEO_ADAPTER_WORKFLOW).unwrap();
        local["12"]["_meta"]["title"] = json!("Local Wan Adapter");
        std::fs::write(
            workflows.join("wan-adapter-api.json"),
            serde_json::to_vec(&local).unwrap(),
        )
        .unwrap();
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            models_directory: models,
            video_workflow_path: Some(workflows.join("wan2.2-ti2v-5b-api.json")),
            ..Default::default()
        })
        .unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        client
            .generate_video("ohwx at dusk", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
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

    const PEOPLE_BASE: &str = "SG161222/RealVisXL_V5.0";
    const PEOPLE_CHECKPOINT: &str = "RealVisXL_V5.0_fp16.safetensors";

    fn rgb_png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        let pixels: Vec<u8> = (0..width * height).flat_map(|_| rgb).collect();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&pixels, width, height, image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    async fn mock_output(server: &MockServer, prompt_id: &str, filename: &str, body: Vec<u8>) {
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                prompt_id: {"status": {"status_str": "success"}, "outputs": {
                    "9": {"images": [{"filename": filename, "subfolder": "", "type": "temp"}]}
                }}
            })))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .and(wiremock::matchers::query_param("filename", filename))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(body),
            )
            .mount(server)
            .await;
    }

    async fn mock_uploads(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "uploaded.png",
                "subfolder": ""
            })))
            .mount(server)
            .await;
    }

    /// A LoRA on the packaged SDXL adapter, bound by `sidecar`, over an
    /// installed base, plus `extra` files.
    fn person_models(
        sidecar: crate::inventory::WeightSidecar,
        extra: &[(&str, &str)],
    ) -> (tempfile::TempDir, PathBuf) {
        let mut files = vec![
            ("checkpoints", PEOPLE_CHECKPOINT),
            ("loras", "jerry.safetensors"),
        ];
        files.extend_from_slice(extra);
        let (root, models) = models_with(&files);
        crate::inventory::write_sidecar(
            &models.join("loras/jerry.safetensors"),
            &sidecar,
            &crate::train::Contract::default(),
        )
        .unwrap();
        (root, models)
    }

    fn person_client(server: &MockServer, models: PathBuf, checkpoint: &str) -> Client {
        let contract = crate::train::Contract {
            ipadapter_face_node: "AcmeIPAdapterFace".into(),
            ..Default::default()
        };
        Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            checkpoint: checkpoint.into(),
            models_directory: models,
            poll_interval: Duration::from_millis(50),
            contract,
            ..Default::default()
        })
        .unwrap()
    }

    async fn prompt_bodies(server: &MockServer) -> Vec<String> {
        server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/prompt")
            .map(|request| String::from_utf8(request.body).unwrap())
            .collect()
    }

    fn identity_sidecar() -> crate::inventory::WeightSidecar {
        crate::inventory::WeightSidecar::new("sdxl-adapter")
            .with_huggingface_base(PEOPLE_BASE)
            .with_trigger("ohwx")
    }

    #[tokio::test]
    async fn people_generate_without_an_identity_omits_controlnet_and_face() {
        let server = MockServer::start().await;
        let (_root, models) = models_with(&[
            ("checkpoints", PEOPLE_CHECKPOINT),
            ("controlnet", "thibaud_xl_openpose.safetensors"),
        ]);
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("KSampler"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "plain"})))
            .expect(1)
            .mount(&server)
            .await;
        mock_output(&server, "plain", "plain.png", vec![1, 2, 3]).await;
        let client = person_client(&server, models, PEOPLE_CHECKPOINT);
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let images = client
            .generate("a person in a kitchen", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images[0].bytes.as_ref(), &[1, 2, 3]);
        let bodies = prompt_bodies(&server).await;
        assert_eq!(bodies.len(), 1, "an SDXL base refines only an identity");
        assert!(
            !bodies[0].contains("ControlNetLoader"),
            "an unposed run strips ControlNet: {}",
            bodies[0]
        );
        assert!(!bodies[0].contains("IPAdapterFace"));
    }

    #[tokio::test]
    async fn people_identity_loads_controlnet_face_and_embedding() {
        let server = MockServer::start().await;
        let (_root, models) = person_models(
            identity_sidecar()
                .with_embedding("ohwx.safetensors")
                .with_face("jerry.face.png"),
            &[
                ("controlnet", "thibaud_xl_openpose.safetensors"),
                ("ipadapter", "ip-adapter-plus-face_sdxl_vit-h.safetensors"),
                ("clip_vision", "CLIP-ViT-H-14-laion2B-s32B-b79K.safetensors"),
                ("embeddings", "ohwx.safetensors"),
            ],
        );
        std::fs::write(
            models.join("loras/jerry.face.png"),
            rgb_png(8, 8, [1, 2, 3]),
        )
        .unwrap();
        let poses = tempfile::tempdir().unwrap();
        std::fs::write(poses.path().join("sitting.png"), rgb_png(8, 8, [0, 0, 0])).unwrap();
        mock_uploads(&server).await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("ControlNetLoader"))
            .and(wiremock::matchers::body_string_contains(
                "AcmeIPAdapterFace",
            ))
            .and(wiremock::matchers::body_string_contains("embedding:ohwx"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "id"})))
            .expect(1)
            .mount(&server)
            .await;
        mock_output(&server, "id", "posed.png", vec![9, 8, 7]).await;
        let mut client = person_client(&server, models, "jerry.safetensors");
        client.config.poses_directory = Some(poses.path().to_path_buf());
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let images = client
            .generate(
                "ohwx sitting in a kitchen",
                None,
                &mut cancel_rx,
                progress_tx,
            )
            .await
            .unwrap();
        assert_eq!(images[0].bytes.as_ref(), &[9, 8, 7]);
        let uploads: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == "/upload/image")
            .map(|request| String::from_utf8_lossy(&request.body).into_owned())
            .collect();
        assert!(
            uploads
                .iter()
                .any(|body| body.contains("filename=\"pose-sitting.png\"")),
            "the prompt's pose is uploaded"
        );
        assert_eq!(uploads.len(), 2, "one pose and one face");
    }

    #[tokio::test]
    async fn people_identity_without_the_pose_images_runs_unposed() {
        let server = MockServer::start().await;
        let (_root, models) = person_models(
            identity_sidecar(),
            &[("controlnet", "thibaud_xl_openpose.safetensors")],
        );
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "bare"})))
            .expect(1)
            .mount(&server)
            .await;
        mock_output(&server, "bare", "bare.png", vec![4, 5, 6]).await;
        let mut client = person_client(&server, models, "jerry.safetensors");
        client.config.poses_directory = Some(tempfile::tempdir().unwrap().path().join("gone"));
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        client
            .generate("ohwx sitting", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        let bodies = prompt_bodies(&server).await;
        assert!(!bodies[0].contains("ControlNetLoader"), "{}", bodies[0]);
        assert!(!bodies[0].contains("IPAdapterFace"), "no face is installed");
    }

    #[tokio::test]
    async fn people_img2img_skips_controlnet() {
        let server = MockServer::start().await;
        let (_root, models) = person_models(
            identity_sidecar(),
            &[("controlnet", "thibaud_xl_openpose.safetensors")],
        );
        let poses = tempfile::tempdir().unwrap();
        std::fs::write(poses.path().join("sitting.png"), rgb_png(8, 8, [0, 0, 0])).unwrap();
        mock_uploads(&server).await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("VAEEncode"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "edit"})))
            .expect(1)
            .mount(&server)
            .await;
        mock_output(&server, "edit", "edited.png", vec![3, 2, 1]).await;
        let mut client = person_client(&server, models, "jerry.safetensors");
        client.config.poses_directory = Some(poses.path().to_path_buf());
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let source = SourceImage::from_bytes(rgb_png(8, 8, [7, 7, 7])).unwrap();
        let images = client
            .generate("ohwx sitting", Some(&source), &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images[0].bytes.as_ref(), &[3, 2, 1]);
        assert!(
            prompt_bodies(&server)
                .await
                .iter()
                .all(|body| !body.contains("ControlNetLoader")),
            "an edit keeps its source's pose"
        );
    }

    #[tokio::test]
    async fn face_refine_reruns_img2img_at_low_denoise() {
        let server = MockServer::start().await;
        let (_root, models) = person_models(
            crate::inventory::WeightSidecar::new("sdxl-adapter").with_huggingface_base(PEOPLE_BASE),
            &[],
        );
        let first = rgb_png(64, 64, [10, 20, 30]);
        let refined = rgb_png(64, 64, [200, 10, 10]);
        mock_uploads(&server).await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("EmptyLatentImage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "first"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("\"denoise\":0.3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "refine"})))
            .expect(1)
            .mount(&server)
            .await;
        mock_output(&server, "first", "one.png", first).await;
        mock_output(&server, "refine", "two.png", refined.clone()).await;
        let client = person_client(&server, models, "jerry.safetensors");
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let images = client
            .generate("a person standing", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(
            images[0].bytes.as_ref(),
            refined.as_slice(),
            "a square close-up is refined whole"
        );
    }

    #[tokio::test]
    async fn a_refine_that_fails_keeps_the_unrefined_image() {
        let server = MockServer::start().await;
        let (_root, models) = person_models(
            crate::inventory::WeightSidecar::new("sdxl-adapter").with_huggingface_base(PEOPLE_BASE),
            &[],
        );
        let first = rgb_png(64, 96, [10, 20, 30]);
        mock_uploads(&server).await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("EmptyLatentImage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "first"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("\"denoise\":0.3"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;
        mock_output(&server, "first", "one.png", first.clone()).await;
        let client = person_client(&server, models, "jerry.safetensors");
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let images = client
            .generate("a person standing", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images[0].bytes.as_ref(), first.as_slice());
    }

    #[tokio::test]
    async fn a_refined_head_is_pasted_back_into_a_portrait() {
        let server = MockServer::start().await;
        let (_root, models) = person_models(
            crate::inventory::WeightSidecar::new("sdxl-adapter").with_huggingface_base(PEOPLE_BASE),
            &[],
        );
        let first = rgb_png(64, 96, [10, 20, 30]);
        mock_uploads(&server).await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("EmptyLatentImage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "first"})))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("\"denoise\":0.3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "refine"})))
            .mount(&server)
            .await;
        mock_output(&server, "first", "one.png", first).await;
        mock_output(&server, "refine", "two.png", rgb_png(32, 32, [200, 10, 10])).await;
        let client = person_client(&server, models, "jerry.safetensors");
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let images = client
            .generate("a person standing", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        assert_eq!(images[0].mime, "image/png");
        let raster = abnegate_vision::decode::decode(&images[0].bytes).unwrap();
        assert_eq!((raster.width, raster.height), (64, 96));
        assert_eq!(&raster.pixels[..3], &[200, 10, 10], "the head is refined");
        let bottom = ((raster.width * raster.height - 1) * 3) as usize;
        assert_eq!(
            &raster.pixels[bottom..bottom + 3],
            &[10, 20, 30],
            "the body is not"
        );
    }

    #[tokio::test]
    async fn a_refine_the_time_limit_cuts_short_keeps_the_unrefined_image() {
        let server = MockServer::start().await;
        let (_root, models) = person_models(
            crate::inventory::WeightSidecar::new("sdxl-adapter").with_huggingface_base(PEOPLE_BASE),
            &[],
        );
        let first = rgb_png(64, 96, [10, 20, 30]);
        mock_uploads(&server).await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("EmptyLatentImage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "first"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("\"denoise\":0.3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "refine"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/refine"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        mock_output(&server, "first", "one.png", first.clone()).await;
        let mut client = person_client(&server, models, "jerry.safetensors");
        client.config.generation_timeout = Duration::from_secs(2);
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();

        let images = client
            .generate("a person standing", None, &mut cancel_rx, progress_tx)
            .await
            .expect("a generated image outlives a refine the time limit cut short");

        assert_eq!(images.len(), 1);
        assert_eq!(images[0].bytes.as_ref(), first.as_slice());
    }

    #[cfg(unix)]
    #[test]
    fn an_identity_reads_its_pose_image_off_the_runtime() {
        let poses = tempfile::tempdir().unwrap();
        let pose = poses.path().join("sitting.png");
        nix::unistd::mkfifo(&pose, nix::sys::stat::Mode::S_IRWXU).unwrap();
        let (done, finished) = std::sync::mpsc::channel();
        let directory = poses.path().to_path_buf();
        let writer = pose.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let uploads = runtime.block_on(async move {
                let server = MockServer::start().await;
                let (_root, models) = person_models(
                    identity_sidecar(),
                    &[("controlnet", "thibaud_xl_openpose.safetensors")],
                );
                mock_uploads(&server).await;
                Mock::given(method("POST"))
                    .and(path("/prompt"))
                    .respond_with(
                        ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "posed"})),
                    )
                    .mount(&server)
                    .await;
                mock_output(&server, "posed", "posed.png", vec![1, 2, 3]).await;
                let mut client = person_client(&server, models, "jerry.safetensors");
                client.config.poses_directory = Some(directory);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    tokio::fs::write(writer, rgb_png(8, 8, [0, 0, 0])).await
                });
                let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
                let (progress_tx, _) = mpsc::unbounded_channel();
                client
                    .generate("ohwx sitting", None, &mut cancel_rx, progress_tx)
                    .await
                    .unwrap();
                server
                    .received_requests()
                    .await
                    .unwrap()
                    .into_iter()
                    .filter(|request| request.url.path() == "/upload/image")
                    .map(|request| String::from_utf8_lossy(&request.body).into_owned())
                    .collect::<Vec<String>>()
            });
            let _ = done.send(uploads);
            runtime.shutdown_background();
        });

        let uploads = finished
            .recv_timeout(Duration::from_secs(20))
            .unwrap_or_else(|_| {
                let _ = std::fs::write(&pose, b"");
                panic!("reading the pose image blocked the async runtime");
            });

        assert!(
            uploads
                .iter()
                .any(|body| body.contains("filename=\"pose-sitting.png\"")),
            "the prompt's pose is uploaded"
        );
    }
}
