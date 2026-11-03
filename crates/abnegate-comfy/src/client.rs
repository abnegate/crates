//! Direct ComfyUI API client. Graphs come from packaged recipes; chat only
//! supplies prompt, optional negative prompt, seed, checkpoint filename, and
//! an optional source image.

mod error;
mod generated_image;
mod plan;
mod prepared;
#[cfg(test)]
mod probe;
mod queue;
mod source_image;
mod source_video;

pub use crate::media::MAXIMUM_SOURCE_IMAGE_BYTES;
pub use error::Error;
pub use generated_image::GeneratedImage;
pub use source_image::SourceImage;
pub use source_video::MAXIMUM_SOURCE_VIDEO_BYTES;
pub use source_video::SourceVideo;

use plan::Plan;
use prepared::Prepared;
use queue::Queue;

use reqwest::Client as HttpClient;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Once;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::config::Config;
use crate::inventory::WeightSidecar;
use crate::media::MediaType;
use crate::people;
use crate::people::PoseImage;
use crate::recipe::Fill;
use crate::recipe::LOAD_IMAGE;
use crate::recipe::MAXIMUM_PROMPT_BYTES;
use crate::recipe::MediaKind;
use crate::recipe::PromptMode;
use crate::recipe::Recipe;
use crate::recipe::RecipeCatalog;
use crate::recipe::link_node;
use crate::recipe::sanitize_upload_name;
use crate::recipe::sanitize_weight_filename;
use crate::subject::Subject;

const PACKAGED_VIDEO_WORKFLOW: &str = include_str!("../comfyui/workflows/wan2.2-a14b-t2v-api.json");
const PACKAGED_IMAGE_TO_VIDEO_WORKFLOW: &str =
    include_str!("../comfyui/workflows/wan2.2-a14b-i2v-api.json");
const VIDEO_GRAPH: &str = "wan2.2-a14b-t2v-api.json";
const IMAGE_TO_VIDEO_GRAPH: &str = "wan2.2-a14b-i2v-api.json";
const LEFTOVER_VIDEO_GRAPH: &str = "wan2.2-ti2v-5b-api.json";
static LEFTOVER_VIDEO_GRAPH_WARNING: Once = Once::new();
const HIGH_NOISE_UNET: &str = "wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors";
const A14B_VAE: &str = "wan_2.1_vae.safetensors";
const LEFTOVER_VAE: &str = "wan2.2_vae.safetensors";
const WAN_TOKEN: &str = "wan";
const HIGH_NOISE_TOKEN: &str = "high_noise";
const LOW_NOISE_TOKEN: &str = "low_noise";
const TEXT_TO_VIDEO_TOKEN: &str = "t2v";
const IMAGE_TO_VIDEO_TOKEN: &str = "i2v";
const HIGH_NOISE_NODE: &str = "1";
const LOW_NOISE_NODE: &str = "12";
const HIGH_NOISE_SAMPLER: &str = "8";
const LOW_NOISE_SAMPLER: &str = "14";
const UNET_LOADERS: &[&str] = &["UNETLoader", "UnetLoaderGGUF"];
const IMAGE_TO_VIDEO_LATENT: &str = "WanImageToVideo";
const SOURCE_IMAGE_NODE: &str = "11";
const PACKAGED_AUDIO_WORKFLOW: &str =
    include_str!("../comfyui/workflows/ace-step-v1-3.5b-api.json");
const PACKAGED_UPSCALE_WORKFLOW: &str = include_str!("../comfyui/workflows/upscale-image-api.json");
const PACKAGED_UPSCALE_VIDEO_WORKFLOW: &str =
    include_str!("../comfyui/workflows/upscale-video-api.json");
const DOWNLOAD_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a queue delete is waited on before the generation that sent it
/// returns.
const QUEUE_DELETE_WAIT: Duration = Duration::from_secs(5);
const LOST_PROMPT: &str = "it no longer has the prompt, most likely because it restarted";

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

    #[cfg(test)]
    fn generation_plan(
        &self,
        has_source: bool,
    ) -> Result<(&Recipe, HashMap<String, String>), Error> {
        self.plan_for(self.image_recipe()?, has_source)
    }

    /// The recipe a pass over the selected checkpoint's `recipe` runs, and
    /// its weights: `recipe` itself, or its edit recipe for a source.
    fn plan_for<'a>(
        &'a self,
        recipe: &'a Recipe,
        has_source: bool,
    ) -> Result<(&'a Recipe, HashMap<String, String>), Error> {
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

    fn video_workflow(&self, image_to_video: bool) -> Result<Value, Error> {
        let path = self.config.video_workflow_path.as_deref();
        if image_to_video {
            load_image_to_video_workflow(path)
        } else {
            load_video_workflow(path)
        }
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
    /// cuts short, is returned as it was, and no refine starts with less than
    /// one [`Config::poll_interval`] left; once the first pass has its results
    /// only a cancellation fails the call.
    ///
    /// Everything the generation reads from disk is read on the blocking
    /// pool, and a cancellation or the time limit returns while it is read.
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
        let prepared = {
            let prompt = prompt.to_string();
            let has_source = source.is_some();
            self.off_runtime(cancel, deadline, move |client| {
                client.prepare(&prompt, has_source)
            })
            .await?
        };
        let prompt = people::prefix_embedding(prompt, prepared.sidecar.as_ref());
        let images = self
            .generate_image(
                &prompt,
                negative,
                source,
                &prepared.first,
                prepared.pose.as_ref(),
                prepared.face.as_ref(),
                Pass::First,
                cancel,
                deadline,
                progress.clone(),
            )
            .await?;
        let Some(plan) = &prepared.refine else {
            return Ok(images);
        };
        let subject = Subject::shared(&self.config).await;
        let mut refined = Vec::with_capacity(images.len());
        for image in images {
            if deadline.saturating_duration_since(tokio::time::Instant::now())
                < self.config.poll_interval
            {
                refined.push(image);
                continue;
            }
            refined.push(
                self.refine(
                    &prompt,
                    negative,
                    image,
                    plan,
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
    /// filesystem reads a generation makes before it submits anything. A
    /// cancel or the deadline returns at once, leaving `work` to finish on
    /// its own.
    async fn off_runtime<T, F>(
        &self,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
        work: F,
    ) -> Result<T, Error>
    where
        F: FnOnce(&Client) -> Result<T, Error> + Send + 'static,
        T: Send + 'static,
    {
        let client = self.clone();
        tokio::select! {
            biased;
            _ = cancel.recv() => Err(Error::Cancelled),
            _ = tokio::time::sleep_until(deadline) => Err(Error::Timeout),
            result = crate::blocking::run(move || work(&client)) => {
                result.unwrap_or(Err(Error::Cancelled))
            }
        }
    }

    /// Reads what a generation needs from disk: the selected weight's
    /// binding, the recipe and weights of its first pass and of its face
    /// refine when its recipe gets one, and the pose and face images of its
    /// person extras. A refine that cannot be planned is left out.
    fn prepare(&self, prompt: &str, has_source: bool) -> Result<Prepared, Error> {
        #[cfg(test)]
        probe::entered(&self.config.models_directory);
        let sidecar = self.selected_sidecar().map(|sidecar| {
            people::without_missing_embedding(&self.config.models_directory, sidecar)
        });
        let image = self.image_recipe()?;
        let (recipe, weights) = self.plan_for(image, has_source)?;
        let first = Plan {
            recipe: recipe.id.clone(),
            weights,
        };
        let refine = if people::refines(image, sidecar.as_ref()) {
            match self.plan_for(image, true) {
                Ok((recipe, weights)) => Some(Plan {
                    recipe: recipe.id.clone(),
                    weights,
                }),
                Err(error) => {
                    tracing::warn!(%error, "the face refine cannot be planned; skipping it");
                    None
                }
            }
        } else {
            None
        };
        let extras = people::extras(&self.config, recipe, sidecar.as_ref(), has_source);
        let pose = extras
            .pose
            .then_some(self.config.poses_directory.as_deref())
            .flatten()
            .and_then(|directory| {
                people::pose_image(
                    directory,
                    &people::prefix_embedding(prompt, sidecar.as_ref()),
                )
            });
        let face = extras
            .face
            .then(|| {
                crate::inventory::face_path(
                    &self.config.models_directory,
                    &self.config.checkpoint,
                    sidecar.as_ref(),
                )
            })
            .flatten()
            .and_then(|path| crate::inventory::read_image(&path))
            .and_then(|bytes| SourceImage::from_bytes(bytes).ok());
        Ok(Prepared {
            sidecar,
            first,
            refine,
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
        plan: &Plan,
        pose: Option<&PoseImage>,
        face: Option<&SourceImage>,
        pass: Pass,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
        progress: mpsc::UnboundedSender<String>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        let recipe = self
            .catalog
            .get(&plan.recipe)
            .ok_or(Error::Configuration("planned recipe is not in the catalog"))?;
        let pose = match pose {
            Some(pose) => Some(
                self.upload_media(
                    &bytes::Bytes::copy_from_slice(&pose.bytes),
                    &pose.name,
                    MediaType::PNG.mime,
                    cancel,
                    deadline,
                )
                .await?,
            ),
            None => None,
        };
        let face = match face {
            Some(face) => Some(self.upload_source(face, cancel, deadline).await?),
            None => None,
        };
        let fill_weights: HashMap<&str, &str> = plan
            .weights
            .iter()
            .map(|(name, filename)| (name.as_str(), filename.as_str()))
            .collect();
        let mut fill = Fill::new(
            prompt,
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
        plan: &Plan,
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
                plan,
                None,
                None,
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
    /// It runs the Wan 2.2 A14B experts: the high-noise one samples the
    /// first half of the steps and the low-noise one the rest. The graph is
    /// [`Config::video_workflow_path`] when it keeps the A14B contract, or
    /// the packaged one; animating a source reads `wan2.2-a14b-i2v-api.json`
    /// beside it, or the packaged one. A configured graph that cannot be read
    /// or breaks the contract fails with [`Error::Configuration`], except a
    /// leftover `wan2.2-ti2v-5b-api.json`, which loads the A14B graph beside
    /// it. [`Config::video_unet`] names either expert and the other is paired
    /// with it; a name that is not a Wan 2.2 noise expert, such as the 5B
    /// model, loads the packaged pair, and the 5B `wan2.2_vae.safetensors`
    /// loads the Wan 2.1 VAE the experts decode with.
    ///
    /// The prompt reaches the graph exactly as written. No identity LoRA is
    /// applied to video, so a trigger word in it is plain text.
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
        let prompt = if prompt.trim().is_empty() {
            if source.is_some() {
                "animate this image"
            } else {
                return Err(Error::Configuration("prompt is empty or too long"));
            }
        } else {
            prompt
        };
        let workflow = self.video_workflow(source.is_some())?;
        let seed = rand::random::<u64>() & i64::MAX as u64;
        let workflow = if let Some(source) = source {
            let _ = progress.send("Uploading source image...".to_string());
            let uploaded = self.upload_source(source, cancel, deadline).await?;
            configure_wan_image_to_video_workflow(
                workflow,
                prompt,
                &self.config.video_unet,
                &self.config.video_clip,
                &self.config.video_vae,
                seed,
                &uploaded,
            )?
        } else {
            configure_wan_text_to_video_workflow(
                workflow,
                prompt,
                &self.config.video_unet,
                &self.config.video_clip,
                &self.config.video_vae,
                seed,
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
        let mut failing_since = None;
        let mut queue_checked = tokio::time::Instant::now();
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
                    let polled = match self
                        .history_outputs(&prompt_id, cancel, deadline, collection)
                        .await
                    {
                        Ok(None) if queue_checked.elapsed() >= self.config.queue_check_interval => {
                            queue_checked = tokio::time::Instant::now();
                            self.confirm_queued(&prompt_id, cancel, deadline, collection)
                                .await
                        }
                        polled => polled,
                    };
                    match polled {
                        Ok(Some(outputs)) => {
                            let _ = progress.send(collection.saving.to_string());
                            let result = self.fetch_outputs(outputs, cancel, deadline).await;
                            match &result {
                                Ok(_) => self.clear_history(&prompt_id).await,
                                Err(Error::Http(error)) if is_transient(error) => {
                                    self.abandon(&prompt_id).await;
                                }
                                Err(_) => self.cancel(&prompt_id).await,
                            }
                            return result;
                        }
                        Ok(None) => failing_since = None,
                        Err(Error::Http(error)) if is_transient(&error) => {
                            if self.outage_outlasted(&mut failing_since) {
                                tracing::warn!(
                                    prompt_id = %prompt_id,
                                    error = %error,
                                    "ComfyUI kept failing; giving up on the prompt"
                                );
                                self.abandon(&prompt_id).await;
                                return Err(Error::Http(error));
                            }
                            tracing::warn!(
                                prompt_id = %prompt_id,
                                error = %error,
                                "ComfyUI history poll failed; polling again"
                            );
                        }
                        Err(error) => {
                            self.cancel(&prompt_id).await;
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

    /// What an empty history means: `None` while ComfyUI still queues
    /// `prompt_id`, its outputs when it finished between the two reads, and
    /// [`LOST_PROMPT`] when ComfyUI has neither.
    async fn confirm_queued(
        &self,
        prompt_id: &str,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
        collection: Collection<'_>,
    ) -> Result<Option<Vec<OutputImage>>, Error> {
        let request = self.authorize(self.client.get(format!("{}/queue", self.config.base_url)));
        let queue = self
            .bounded(cancel, deadline, async move {
                request
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Queue>()
                    .await
            })
            .await
            .inspect_err(|error| {
                if let Error::Http(error) = error
                    && !is_transient(error)
                {
                    tracing::warn!(
                        prompt_id = %prompt_id,
                        error = %error,
                        "ComfyUI queue check failed; giving up on the prompt"
                    );
                }
            })?;
        if queue.holds(prompt_id) {
            return Ok(None);
        }
        match self
            .history_outputs(prompt_id, cancel, deadline, collection)
            .await?
        {
            Some(outputs) => Ok(Some(outputs)),
            None => Err(Error::InvalidResponse(LOST_PROMPT)),
        }
    }

    /// Whether the run of failures that began at `failing_since`, now when
    /// this is its first, has outlasted [`Config::outage_window`].
    fn outage_outlasted(&self, failing_since: &mut Option<tokio::time::Instant>) -> bool {
        let since = *failing_since.get_or_insert_with(tokio::time::Instant::now);
        since.elapsed() >= self.config.outage_window
    }

    fn download_timeout(&self, deadline: tokio::time::Instant) -> Duration {
        crate::http::request_timeout(&self.config)
            .max(DOWNLOAD_ATTEMPT_TIMEOUT)
            .min(deadline.saturating_duration_since(tokio::time::Instant::now()))
    }

    async fn fetch_outputs(
        &self,
        outputs: Vec<OutputImage>,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
    ) -> Result<Vec<GeneratedImage>, Error> {
        let mut generated = Vec::with_capacity(outputs.len());
        for output in outputs {
            let (bytes, mime) = self.fetch_output(&output, cancel, deadline).await?;
            generated.push(GeneratedImage {
                bytes,
                mime,
                filename: output.filename,
            });
        }
        Ok(generated)
    }

    /// Downloads `output`, retrying a failure that could heal until it has
    /// lasted [`Config::outage_window`].
    async fn fetch_output(
        &self,
        output: &OutputImage,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
    ) -> Result<(bytes::Bytes, String), Error> {
        let url = format!(
            "{}/view?filename={}&subfolder={}&type={}",
            self.config.base_url,
            urlencoding::encode(output.filename.as_str()),
            urlencoding::encode(output.subfolder.as_str()),
            urlencoding::encode(output.r#type.as_str()),
        );
        let mut failing_since = None;
        loop {
            let request = self
                .authorize(self.client.get(&url))
                .timeout(self.download_timeout(deadline));
            let filename = output.filename.as_str();
            let result = self
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
                        .unwrap_or_else(|| mime_for_filename(filename));
                    Ok((response.bytes().await?, mime))
                })
                .await;
            match result {
                Err(Error::Http(error)) if is_transient(&error) => {
                    if self.outage_outlasted(&mut failing_since) {
                        tracing::warn!(
                            filename = %output.filename,
                            error = %error,
                            "ComfyUI output download kept failing; giving up"
                        );
                        return Err(Error::Http(error));
                    }
                    tracing::warn!(
                        filename = %output.filename,
                        error = %error,
                        "ComfyUI output download failed; retrying"
                    );
                    self.pause(cancel, deadline).await?;
                }
                result => return result,
            }
        }
    }

    async fn pause(
        &self,
        cancel: &mut broadcast::Receiver<()>,
        deadline: tokio::time::Instant,
    ) -> Result<(), Error> {
        tokio::select! {
            biased;
            _ = cancel.recv() => Err(Error::Cancelled),
            _ = tokio::time::sleep_until(deadline) => Err(Error::Timeout),
            _ = tokio::time::sleep(self.config.poll_interval) => Ok(()),
        }
    }

    /// Deletes `prompt_id` from the queue within the request timeout, but
    /// returns after [`QUEUE_DELETE_WAIT`] at most, leaving a slow delete to land
    /// on its own.
    async fn cancel(&self, prompt_id: &str) {
        let client = self.clone();
        let prompt_id = prompt_id.to_string();
        let delete = tokio::spawn(async move {
            let timeout = crate::http::request_timeout(&client.config);
            client.delete_from_queue(&prompt_id, timeout).await;
        });
        let _ = tokio::time::timeout(QUEUE_DELETE_WAIT, delete).await;
    }

    async fn abandon(&self, prompt_id: &str) {
        self.delete_from_queue(prompt_id, QUEUE_DELETE_WAIT).await;
    }

    async fn delete_from_queue(&self, prompt_id: &str, timeout: Duration) {
        if let Err(error) = self
            .authorize(self.client.post(format!("{}/queue", self.config.base_url)))
            .json(&json!({ "delete": [prompt_id] }))
            .timeout(timeout)
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

fn load_video_workflow(configured: Option<&Path>) -> Result<Value, Error> {
    match video_graph_path(configured) {
        Some(path) => load_custom_video_workflow(
            &path,
            "video workflow path is not readable",
            validate_video_workflow,
        ),
        None => serde_json::from_str(PACKAGED_VIDEO_WORKFLOW)
            .map_err(|_| Error::Configuration("packaged video workflow is not valid JSON")),
    }
}

fn load_image_to_video_workflow(text_to_video_path: Option<&Path>) -> Result<Value, Error> {
    match sibling_file(text_to_video_path, IMAGE_TO_VIDEO_GRAPH) {
        Some(path) => load_custom_video_workflow(
            &path,
            "image-to-video workflow path is not readable",
            validate_image_to_video_workflow,
        ),
        None => serde_json::from_str(PACKAGED_IMAGE_TO_VIDEO_WORKFLOW).map_err(|_| {
            Error::Configuration("packaged image-to-video workflow is not valid JSON")
        }),
    }
}

/// The text-to-video graph to read in place of the packaged one: `None`
/// for none configured, or for the packaged name where no file has it, and
/// the A14B graph beside a leftover 5B one.
fn video_graph_path(configured: Option<&Path>) -> Option<PathBuf> {
    let configured = configured?;
    match configured.file_name().and_then(|name| name.to_str()) {
        Some(LEFTOVER_VIDEO_GRAPH) => {
            LEFTOVER_VIDEO_GRAPH_WARNING.call_once(|| {
                tracing::warn!(
                    path = %configured.display(),
                    "the video workflow path names the retired Wan 2.2 TI2V 5B graph; loading the A14B graph instead"
                );
            });
            sibling_file(Some(configured), VIDEO_GRAPH)
        }
        Some(VIDEO_GRAPH) if !configured.is_file() => None,
        _ => Some(configured.to_path_buf()),
    }
}

fn load_custom_video_workflow(
    path: &Path,
    unreadable: &'static str,
    validate: fn(&Value) -> Result<(), Error>,
) -> Result<Value, Error> {
    let workflow = load_workflow_file(path).map_err(|_| Error::Configuration(unreadable))?;
    validate(&workflow)?;
    Ok(workflow)
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
        ("/1/class_type", LOAD_IMAGE),
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
    validate_a14b_workflow(workflow)?;
    if workflow.pointer("/7/class_type").and_then(Value::as_str) != Some("EmptyHunyuanLatentVideo")
    {
        return Err(Error::Configuration(
            "text-to-video workflow must use EmptyHunyuanLatentVideo",
        ));
    }
    Ok(())
}

fn validate_a14b_workflow(workflow: &Value) -> Result<(), Error> {
    for pointer in [
        "/1/inputs/unet_name",
        "/2/inputs/clip_name",
        "/3/inputs/vae_name",
        "/5/inputs/text",
        "/8/inputs/noise_seed",
        "/10/inputs/images",
        "/12/inputs/unet_name",
        "/14/inputs/noise_seed",
    ] {
        if workflow.pointer(pointer).is_none() {
            return Err(Error::Configuration(
                "workflow does not match the Wan A14B contract",
            ));
        }
    }
    if workflow.pointer("/8/class_type").and_then(Value::as_str) != Some("KSamplerAdvanced")
        || workflow.pointer("/14/class_type").and_then(Value::as_str) != Some("KSamplerAdvanced")
    {
        return Err(Error::Configuration(
            "A14B video workflow must sample high then low noise experts",
        ));
    }
    if workflow.pointer("/10/class_type").and_then(Value::as_str) != Some("SaveWEBM") {
        return Err(Error::Configuration(
            "video workflow output must use SaveWEBM",
        ));
    }
    if [HIGH_NOISE_NODE, LOW_NOISE_NODE].iter().any(|node| {
        !workflow
            .pointer(&format!("/{node}/class_type"))
            .and_then(Value::as_str)
            .is_some_and(|class| UNET_LOADERS.contains(&class))
    }) {
        return Err(Error::Configuration(
            "A14B video workflow must load its experts with a UNet loader at nodes 1 and 12",
        ));
    }
    if model_source(workflow, HIGH_NOISE_SAMPLER) != Some(HIGH_NOISE_NODE)
        || model_source(workflow, LOW_NOISE_SAMPLER) != Some(LOW_NOISE_NODE)
        || workflow
            .pointer(&format!("/{LOW_NOISE_SAMPLER}/inputs/latent_image"))
            .and_then(link_node)
            != Some(HIGH_NOISE_SAMPLER)
    {
        return Err(Error::Configuration(
            "A14B video workflow must sample node 1's expert at 8 and finish with node 12's at 14",
        ));
    }
    Ok(())
}

/// The UNet loader the model `sampler` samples comes from, followed through
/// every node that passes a model on.
fn model_source<'a>(workflow: &'a Value, sampler: &str) -> Option<&'a str> {
    let link = workflow.pointer(&format!("/{sampler}/inputs/model"))?;
    upstream(workflow, link, "model", |class| {
        UNET_LOADERS.contains(&class)
    })
}

/// The first node whose class `is_source` accepts on the way back from
/// `link`, followed through every node that passes its `input` on.
fn upstream<'a>(
    workflow: &'a Value,
    link: &'a Value,
    input: &str,
    is_source: impl Fn(&str) -> bool,
) -> Option<&'a str> {
    let mut node = link_node(link)?;
    let nodes = workflow.as_object().map_or(0, serde_json::Map::len);
    for _ in 0..nodes {
        let class = workflow
            .pointer(&format!("/{node}/class_type"))
            .and_then(Value::as_str)?;
        if is_source(class) {
            return Some(node);
        }
        node = workflow
            .pointer(&format!("/{node}/inputs/{input}"))
            .and_then(link_node)?;
    }
    None
}

fn validate_image_to_video_workflow(workflow: &Value) -> Result<(), Error> {
    validate_a14b_workflow(workflow)?;
    if workflow.pointer("/7/class_type").and_then(Value::as_str) != Some(IMAGE_TO_VIDEO_LATENT) {
        return Err(Error::Configuration(
            "image-to-video workflow must use WanImageToVideo",
        ));
    }
    if workflow
        .pointer(&format!("/{SOURCE_IMAGE_NODE}/class_type"))
        .and_then(Value::as_str)
        != Some(LOAD_IMAGE)
    {
        return Err(Error::Configuration(
            "image-to-video workflow must load a source image",
        ));
    }
    let starts_from_source = workflow
        .pointer("/7/inputs/start_image")
        .and_then(|link| upstream(workflow, link, "image", |class| class == LOAD_IMAGE))
        == Some(SOURCE_IMAGE_NODE);
    if !starts_from_source {
        return Err(Error::Configuration(
            "image-to-video workflow must start from the image node 11 loads",
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
    workflow
        .pointer_mut(&format!("/{SOURCE_IMAGE_NODE}/inputs"))
        .and_then(Value::as_object_mut)
        .ok_or(Error::Configuration(
            "image-to-video workflow must load a source image",
        ))?
        .insert("image".to_string(), json!(image_name));
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
    if prompt.trim().is_empty() || prompt.len() > MAXIMUM_PROMPT_BYTES {
        return Err(Error::Configuration("prompt is empty or too long"));
    }
    if !is_model_filename(unet) || !is_model_filename(clip) || !is_model_filename(vae) {
        return Err(Error::Configuration("invalid video model filename"));
    }
    let image_to_video =
        workflow.pointer("/7/class_type").and_then(Value::as_str) == Some(IMAGE_TO_VIDEO_LATENT);
    let (high, low) = paired_wan_unets(unet, image_to_video);
    if !is_model_filename(&high) || !is_model_filename(&low) {
        return Err(Error::Configuration("invalid video model filename"));
    }
    workflow[HIGH_NOISE_NODE]["inputs"]["unet_name"] = json!(high);
    workflow[LOW_NOISE_NODE]["inputs"]["unet_name"] = json!(low);
    workflow["2"]["inputs"]["clip_name"] = json!(clip);
    workflow["3"]["inputs"]["vae_name"] = json!(a14b_vae(vae));
    workflow["5"]["inputs"]["text"] = json!(prompt);
    workflow["8"]["inputs"]["noise_seed"] = json!(seed);
    workflow["14"]["inputs"]["noise_seed"] = json!(seed);
    Ok(())
}

/// Whether `unet` names a Wan 2.2 A14B noise expert, in any case: one that
/// names Wan, a noise level and a mode standing on its own, so `ti2v` is no
/// `i2v`.
fn is_a14b_unet(unet: &str) -> bool {
    let lower = unet.to_ascii_lowercase();
    lower.contains(WAN_TOKEN)
        && (lower.contains(HIGH_NOISE_TOKEN) || lower.contains(LOW_NOISE_TOKEN))
        && (mode_starts(&lower, TEXT_TO_VIDEO_TOKEN).next().is_some()
            || mode_starts(&lower, IMAGE_TO_VIDEO_TOKEN).next().is_some())
}

/// Where `mode` starts in `lower` with no letter or digit on either side.
fn mode_starts<'a>(lower: &'a str, mode: &'a str) -> impl Iterator<Item = usize> + 'a {
    lower
        .match_indices(mode)
        .map(|(start, _)| start)
        .filter(move |&start| {
            !lower[..start].ends_with(|character: char| character.is_ascii_alphanumeric())
                && !lower[start + mode.len()..]
                    .starts_with(|character: char| character.is_ascii_alphanumeric())
        })
}

/// The high-noise and low-noise experts the graph for `image_to_video` loads
/// for `unet`: its own pair in the graph's mode, in its own casing, or the
/// packaged pair when `unet` is not a Wan 2.2 noise expert.
fn paired_wan_unets(unet: &str, image_to_video: bool) -> (String, String) {
    let expert = if is_a14b_unet(unet) {
        unet
    } else {
        HIGH_NOISE_UNET
    };
    let (from, to) = if image_to_video {
        (TEXT_TO_VIDEO_TOKEN, IMAGE_TO_VIDEO_TOKEN)
    } else {
        (IMAGE_TO_VIDEO_TOKEN, TEXT_TO_VIDEO_TOKEN)
    };
    let lower = expert.to_ascii_lowercase();
    let moded = replace_at(expert, mode_starts(&lower, from), from.len(), to);
    let high = replace_token(&moded, LOW_NOISE_TOKEN, HIGH_NOISE_TOKEN);
    let low = replace_token(&high, HIGH_NOISE_TOKEN, LOW_NOISE_TOKEN);
    (high, low)
}

/// `name` with every `token`, matched in any case, replaced by
/// `replacement` in the casing of the text it replaces.
fn replace_token(name: &str, token: &str, replacement: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let starts = lower.match_indices(token).map(|(start, _)| start);
    replace_at(name, starts, token.len(), replacement)
}

/// `name` with the `length` bytes at each of `starts` replaced by
/// `replacement` in the casing of the text it replaces.
fn replace_at(
    name: &str,
    starts: impl Iterator<Item = usize>,
    length: usize,
    replacement: &str,
) -> String {
    let mut replaced = String::with_capacity(name.len());
    let mut copied = 0;
    for start in starts {
        let end = start + length;
        replaced.push_str(&name[copied..start]);
        replaced.push_str(&cased_like(&name[start..end], replacement));
        copied = end;
    }
    replaced.push_str(&name[copied..]);
    replaced
}

/// `replacement` cased word by word, split on `_`, like `original`:
/// upper case, a leading capital, or lower case.
fn cased_like(original: &str, replacement: &str) -> String {
    original
        .split('_')
        .zip(replacement.split('_'))
        .map(|(original, replacement)| {
            let upper = original.bytes().any(|byte| byte.is_ascii_uppercase());
            let lower = original.bytes().any(|byte| byte.is_ascii_lowercase());
            if upper && !lower {
                replacement.to_ascii_uppercase()
            } else if original.starts_with(|character: char| character.is_ascii_uppercase()) {
                let mut characters = replacement.chars();
                characters
                    .next()
                    .map(|first| first.to_ascii_uppercase().to_string() + characters.as_str())
                    .unwrap_or_default()
            } else {
                replacement.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("_")
}

/// The VAE the A14B experts decode with for `vae`: the Wan 2.1 VAE in place
/// of the 5B one a TI2V setting leaves behind.
fn a14b_vae(vae: &str) -> &str {
    if vae == LEFTOVER_VAE { A14B_VAE } else { vae }
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

/// Whether a failed request could succeed if sent again: no answer at all,
/// or a 5xx, 408 or 429 status. A body that does not parse never heals.
fn is_transient(error: &reqwest::Error) -> bool {
    !error.is_builder()
        && !error.is_decode()
        && error.status().is_none_or(|status| {
            status.is_server_error()
                || status == reqwest::StatusCode::REQUEST_TIMEOUT
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        })
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
    use crate::recipe::video_fixture;
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
            HIGH_NOISE_UNET,
            "custom-clip.safetensors",
            "wan2.2_vae.safetensors",
            42,
        )
        .unwrap();
        assert_eq!(workflow["1"]["inputs"]["unet_name"], HIGH_NOISE_UNET);
        assert_eq!(
            workflow["12"]["inputs"]["unet_name"],
            "wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors"
        );
        assert_eq!(
            workflow["2"]["inputs"]["clip_name"],
            "custom-clip.safetensors"
        );
        assert_eq!(workflow["3"]["inputs"]["vae_name"], A14B_VAE);
        assert_eq!(workflow["5"]["inputs"]["text"], "a moving fox");
        assert_eq!(workflow["8"]["inputs"]["noise_seed"], 42);
        assert_eq!(workflow["14"]["inputs"]["noise_seed"], 42);
        assert_eq!(workflow["8"]["class_type"], "KSamplerAdvanced");
        assert_eq!(workflow["7"]["class_type"], "EmptyHunyuanLatentVideo");
        assert_eq!(workflow["7"]["inputs"]["width"], 1280);
        assert_eq!(workflow["7"]["inputs"]["height"], 704);
        assert_eq!(workflow["7"]["inputs"]["length"], 81);
        assert_eq!(workflow["10"]["inputs"]["fps"], 16);
        assert!(workflow.get("11").is_none());
    }

    #[test]
    fn a_leftover_5b_unet_still_loads_the_a14b_experts() {
        let workflow = build_wan_text_to_video_workflow(
            "a moving fox",
            "wan2.2_ti2v_5B_fp16.safetensors",
            "custom-clip.safetensors",
            "wan2.2_vae.safetensors",
            7,
        )
        .unwrap();
        assert_eq!(workflow["1"]["inputs"]["unet_name"], HIGH_NOISE_UNET);
        assert_eq!(
            workflow["12"]["inputs"]["unet_name"],
            "wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors"
        );
        assert_eq!(workflow["3"]["inputs"]["vae_name"], A14B_VAE);
    }

    #[test]
    fn a_video_workflow_uses_the_configured_vae() {
        let text_to_video = build_wan_text_to_video_workflow(
            "a moving fox",
            HIGH_NOISE_UNET,
            "custom-clip.safetensors",
            "custom-vae.safetensors",
            7,
        )
        .unwrap();
        let image_to_video = build_wan_image_to_video_workflow(
            "make it move",
            HIGH_NOISE_UNET,
            "custom-clip.safetensors",
            "custom-vae.safetensors",
            7,
            "i2v-source.png",
        )
        .unwrap();
        for workflow in [text_to_video, image_to_video] {
            assert_eq!(
                workflow["3"]["inputs"]["vae_name"],
                "custom-vae.safetensors"
            );
        }
    }

    #[test]
    fn only_wan_noise_experts_keep_their_own_pair() {
        let packaged = (
            HIGH_NOISE_UNET.to_string(),
            "wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors".to_string(),
        );
        assert_eq!(
            paired_wan_unets("wan2.1_t2v_14B_fp16.safetensors", false),
            packaged
        );
        assert_eq!(
            paired_wan_unets("hunyuan_video_high_noise.safetensors", false),
            packaged
        );
        assert_eq!(
            paired_wan_unets("wan2.2_i2v_low_noise_14B_fp16.safetensors", true),
            (
                "wan2.2_i2v_high_noise_14B_fp16.safetensors".to_string(),
                "wan2.2_i2v_low_noise_14B_fp16.safetensors".to_string(),
            )
        );
    }

    #[test]
    fn noise_expert_names_pair_whatever_their_case() {
        assert_eq!(
            paired_wan_unets("Wan2.2_T2V_HIGH_NOISE_14B_fp8.safetensors", true),
            (
                "Wan2.2_I2V_HIGH_NOISE_14B_fp8.safetensors".to_string(),
                "Wan2.2_I2V_LOW_NOISE_14B_fp8.safetensors".to_string(),
            )
        );
        assert_eq!(
            paired_wan_unets("WAN2.2_i2v_Low_Noise_14B.safetensors", false),
            (
                "WAN2.2_t2v_High_Noise_14B.safetensors".to_string(),
                "WAN2.2_t2v_Low_Noise_14B.safetensors".to_string(),
            )
        );
    }

    #[test]
    fn a_noise_expert_without_a_mode_loads_the_packaged_pair() {
        assert_eq!(
            paired_wan_unets("wan2.2_high_noise_14B_custom.safetensors", true),
            (
                "wan2.2_i2v_high_noise_14B_fp8_scaled.safetensors".to_string(),
                "wan2.2_i2v_low_noise_14B_fp8_scaled.safetensors".to_string(),
            )
        );
        assert_eq!(
            paired_wan_unets("wan2.2_low_noise_14B_custom.safetensors", false),
            (
                HIGH_NOISE_UNET.to_string(),
                "wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors".to_string(),
            )
        );
    }

    #[test]
    fn image_to_video_workflow_mutates_only_approved_inputs() {
        let workflow = build_wan_image_to_video_workflow(
            "make it move",
            HIGH_NOISE_UNET,
            "custom-clip.safetensors",
            "custom-vae.safetensors",
            42,
            "i2v-source.png",
        )
        .unwrap();
        assert_eq!(
            workflow["1"]["inputs"]["unet_name"],
            "wan2.2_i2v_high_noise_14B_fp8_scaled.safetensors"
        );
        assert_eq!(
            workflow["12"]["inputs"]["unet_name"],
            "wan2.2_i2v_low_noise_14B_fp8_scaled.safetensors"
        );
        assert_eq!(workflow["5"]["inputs"]["text"], "make it move");
        assert_eq!(workflow["7"]["class_type"], "WanImageToVideo");
        assert_eq!(workflow["8"]["inputs"]["positive"], json!(["7", 0]));
        assert_eq!(workflow["8"]["inputs"]["latent_image"], json!(["7", 2]));
        assert_eq!(workflow["8"]["inputs"]["noise_seed"], 42);
        assert_eq!(workflow["14"]["inputs"]["noise_seed"], 42);
        assert_eq!(workflow["9"]["inputs"]["samples"], json!(["14", 0]));
        assert_eq!(workflow["11"]["inputs"]["image"], "i2v-source.png");
    }

    fn packaged_graph(text: &str) -> Value {
        serde_json::from_str(text).expect("the packaged graph is valid JSON")
    }

    fn write_graph(path: &Path, workflow: &Value) {
        std::fs::write(path, serde_json::to_vec(workflow).unwrap()).unwrap();
    }

    fn narrowed(packaged: &str) -> Value {
        let mut workflow = packaged_graph(packaged);
        workflow["7"]["inputs"]["width"] = json!(960);
        workflow
    }

    #[test]
    fn a_valid_custom_video_graph_is_used() {
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("my-video.json");
        write_graph(&configured, &narrowed(PACKAGED_VIDEO_WORKFLOW));
        let workflow = load_video_workflow(Some(&configured)).unwrap();
        assert_eq!(workflow["7"]["inputs"]["width"], 960);
    }

    #[test]
    fn an_unreadable_custom_video_graph_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("my-video.json");
        assert!(matches!(
            load_video_workflow(Some(&missing)),
            Err(Error::Configuration(_))
        ));
        let garbled = directory.path().join("garbled.json");
        std::fs::write(&garbled, b"not a graph").unwrap();
        assert!(matches!(
            load_video_workflow(Some(&garbled)),
            Err(Error::Configuration(_))
        ));
    }

    #[test]
    fn a_custom_video_graph_that_breaks_the_a14b_contract_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("my-video.json");
        let mut single_expert = packaged_graph(PACKAGED_VIDEO_WORKFLOW);
        single_expert.as_object_mut().unwrap().remove("14");
        write_graph(&configured, &single_expert);
        assert!(matches!(
            load_video_workflow(Some(&configured)),
            Err(Error::Configuration(_))
        ));
        write_graph(
            &configured,
            &packaged_graph(PACKAGED_IMAGE_TO_VIDEO_WORKFLOW),
        );
        assert!(matches!(
            load_video_workflow(Some(&configured)),
            Err(Error::Configuration(_))
        ));
    }

    #[test]
    fn an_a14b_graph_whose_experts_are_not_unet_loaders_is_refused() {
        for packaged in [PACKAGED_VIDEO_WORKFLOW, PACKAGED_IMAGE_TO_VIDEO_WORKFLOW] {
            for node in [HIGH_NOISE_NODE, LOW_NOISE_NODE] {
                let mut workflow = packaged_graph(packaged);
                workflow[node]["class_type"] = json!("CheckpointLoaderSimple");
                assert!(
                    matches!(
                        validate_a14b_workflow(&workflow),
                        Err(Error::Configuration(_))
                    ),
                    "node {node} loads no UNet"
                );
            }
        }
    }

    #[test]
    fn an_a14b_graph_may_load_gguf_experts() {
        for packaged in [PACKAGED_VIDEO_WORKFLOW, PACKAGED_IMAGE_TO_VIDEO_WORKFLOW] {
            let mut workflow = packaged_graph(packaged);
            for node in [HIGH_NOISE_NODE, LOW_NOISE_NODE] {
                workflow[node]["class_type"] = json!("UnetLoaderGGUF");
            }
            assert!(validate_a14b_workflow(&workflow).is_ok());
        }
    }

    #[test]
    fn an_a14b_graph_must_chain_its_samplers_through_their_own_experts() {
        for packaged in [PACKAGED_VIDEO_WORKFLOW, PACKAGED_IMAGE_TO_VIDEO_WORKFLOW] {
            assert!(validate_a14b_workflow(&packaged_graph(packaged)).is_ok());
            for (pointer, link) in [
                ("/14/inputs/latent_image", json!(["7", 0])),
                ("/14/inputs/model", json!(["4", 0])),
                ("/8/inputs/model", json!(["13", 0])),
                ("/14/inputs/model", json!(["14", 0])),
            ] {
                let mut workflow = packaged_graph(packaged);
                *workflow.pointer_mut(pointer).unwrap() = link.clone();
                assert!(
                    matches!(
                        validate_a14b_workflow(&workflow),
                        Err(Error::Configuration(_))
                    ),
                    "{pointer} = {link}"
                );
            }
        }
    }

    #[test]
    fn an_image_to_video_graph_must_start_from_the_image_it_uploads() {
        let mut scaled = packaged_graph(PACKAGED_IMAGE_TO_VIDEO_WORKFLOW);
        scaled["15"] = json!({
            "class_type": "ImageScale",
            "inputs": {"image": ["11", 0], "width": 1280, "height": 704}
        });
        scaled["7"]["inputs"]["start_image"] = json!(["15", 0]);
        assert!(validate_image_to_video_workflow(&scaled).is_ok());
        for start in [json!(["15", 0]), json!(["5", 0]), json!("source.png")] {
            let mut workflow = packaged_graph(PACKAGED_IMAGE_TO_VIDEO_WORKFLOW);
            workflow["15"] = json!({"class_type": "LoadImage", "inputs": {"image": "other.png"}});
            workflow["7"]["inputs"]["start_image"] = start.clone();
            assert!(
                matches!(
                    validate_image_to_video_workflow(&workflow),
                    Err(Error::Configuration(_))
                ),
                "start_image = {start}"
            );
        }
    }

    #[test]
    fn an_image_to_video_graph_whose_source_has_no_inputs_is_refused() {
        let mut workflow = packaged_graph(PACKAGED_IMAGE_TO_VIDEO_WORKFLOW);
        workflow["11"]["inputs"] = json!("source.png");
        assert!(matches!(
            configure_wan_image_to_video_workflow(
                workflow,
                "make it move",
                HIGH_NOISE_UNET,
                "clip.safetensors",
                "vae.safetensors",
                42,
                "source.png",
            ),
            Err(Error::Configuration(_))
        ));
    }

    #[test]
    fn a_ti2v_name_is_not_an_image_to_video_expert() {
        assert!(!is_a14b_unet("wan2.2_ti2v_high_noise_x.safetensors"));
        assert!(!is_a14b_unet("wan2.2_t2v14b_high_noise.safetensors"));
        assert_eq!(
            paired_wan_unets("wan2.2_ti2v_high_noise_x.safetensors", false),
            (
                HIGH_NOISE_UNET.to_string(),
                "wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors".to_string(),
            )
        );
    }

    #[test]
    fn a_leftover_5b_video_graph_falls_back_to_a14b() {
        let directory = tempfile::tempdir().unwrap();
        let leftover = directory.path().join("wan2.2-ti2v-5b-api.json");
        std::fs::write(&leftover, b"{}").unwrap();
        assert_eq!(
            load_video_workflow(Some(&leftover)).unwrap(),
            packaged_graph(PACKAGED_VIDEO_WORKFLOW)
        );
        write_graph(
            &directory.path().join("wan2.2-a14b-t2v-api.json"),
            &narrowed(PACKAGED_VIDEO_WORKFLOW),
        );
        let workflow = load_video_workflow(Some(&leftover)).unwrap();
        assert_eq!(workflow["7"]["inputs"]["width"], 960);
    }

    #[test]
    fn a_missing_packaged_video_graph_loads_the_compiled_one() {
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("wan2.2-a14b-t2v-api.json");
        assert_eq!(
            load_video_workflow(Some(&configured)).unwrap(),
            packaged_graph(PACKAGED_VIDEO_WORKFLOW)
        );
        assert_eq!(
            load_image_to_video_workflow(Some(&configured)).unwrap(),
            packaged_graph(PACKAGED_IMAGE_TO_VIDEO_WORKFLOW)
        );
        assert_eq!(
            load_video_workflow(None).unwrap(),
            packaged_graph(PACKAGED_VIDEO_WORKFLOW)
        );
    }

    #[test]
    fn image_to_video_uses_a_valid_sibling_and_refuses_a_broken_one() {
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("my-video.json");
        write_graph(&configured, &packaged_graph(PACKAGED_VIDEO_WORKFLOW));
        let sibling = directory.path().join("wan2.2-a14b-i2v-api.json");
        let mut workflow = narrowed(PACKAGED_IMAGE_TO_VIDEO_WORKFLOW);
        write_graph(&sibling, &workflow);
        assert_eq!(
            load_image_to_video_workflow(Some(&configured)).unwrap()["7"]["inputs"]["width"],
            960
        );
        workflow.as_object_mut().unwrap().remove("11");
        write_graph(&sibling, &workflow);
        assert!(matches!(
            load_image_to_video_workflow(Some(&configured)),
            Err(Error::Configuration(_))
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
        let models = write_video_adapter(root.path());
        let config = Config {
            checkpoint: "jerry-video.safetensors".into(),
            models_directory: models,
            workflow_path: Some(video_fixture::write_overlay(
                &root.path().join("overlay"),
                &video_fixture::packaged_with_video_adapter(),
            )),
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
                "EmptyHunyuanLatentVideo",
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

    async fn mock_video_prompt(server: &MockServer, prompt_id: &str) {
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": prompt_id})))
            .expect(1)
            .mount(server)
            .await;
    }

    async fn mock_finished_clip(server: &MockServer, prompt_id: &str) {
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
    }

    async fn mock_queue_delete(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/queue"))
            .respond_with(ResponseTemplate::new(200))
            .mount(server)
            .await;
    }

    async fn mock_unanswered_queue_delete(server: &MockServer) {
        Mock::given(method("POST"))
            .and(path("/queue"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(60)))
            .mount(server)
            .await;
    }

    async fn mock_queue(server: &MockServer, running: Value) {
        Mock::given(method("GET"))
            .and(path("/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "queue_running": running,
                "queue_pending": []
            })))
            .mount(server)
            .await;
    }

    fn empty_history(prompt_id: &str) -> Mock {
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt_id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
    }

    fn video_config(server: &MockServer) -> Config {
        Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        }
    }

    async fn generate_clip(config: Config) -> Result<Vec<GeneratedImage>, Error> {
        let client = Client::new(config).unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        client
            .generate_video("a fox running", None, &mut cancel_rx, progress_tx)
            .await
    }

    async fn posted_bodies(server: &MockServer, endpoint: &str) -> Vec<Value> {
        server
            .received_requests()
            .await
            .expect("the mock server records requests")
            .into_iter()
            .filter(|request| request.method.as_str() == "POST" && request.url.path() == endpoint)
            .map(|request| serde_json::from_slice(&request.body).unwrap_or(Value::Null))
            .collect()
    }

    async fn history_reads(server: &MockServer, prompt_id: &str) -> usize {
        let expected = format!("/history/{prompt_id}");
        server
            .received_requests()
            .await
            .expect("the mock server records requests")
            .iter()
            .filter(|request| request.method.as_str() == "GET" && request.url.path() == expected)
            .count()
    }

    #[tokio::test]
    async fn a_failed_history_poll_keeps_the_render_alive() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "flaky").await;
        Mock::given(method("GET"))
            .and(path("/history/flaky"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        serve_video(&server, "flaky").await;
        let videos = generate_clip(video_config(&server))
            .await
            .expect("one failed poll must not discard the render");
        assert_eq!(videos[0].bytes.as_ref(), &[1, 2, 3, 4]);
        assert!(
            posted_bodies(&server, "/queue").await.is_empty(),
            "a finished render must not be cancelled"
        );
    }

    #[tokio::test]
    async fn a_download_slower_than_the_request_timeout_still_lands() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "slow").await;
        mock_finished_clip(&server, "slow").await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "video/webm")
                    .set_body_bytes(vec![5, 6, 7])
                    .set_delay(Duration::from_millis(2500)),
            )
            .mount(&server)
            .await;
        let config = Config {
            request_timeout: Duration::from_secs(1),
            video_generation_timeout: Duration::from_secs(60),
            ..video_config(&server)
        };
        let videos = generate_clip(config)
            .await
            .expect("a download inside the render deadline must land");
        assert_eq!(videos[0].bytes.as_ref(), &[5, 6, 7]);
    }

    #[tokio::test]
    async fn a_download_that_fails_once_is_retried_and_lands() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "retried").await;
        mock_finished_clip(&server, "retried").await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "video/webm")
                    .set_body_bytes(vec![8, 9]),
            )
            .mount(&server)
            .await;
        mock_queue_delete(&server).await;
        let videos = generate_clip(video_config(&server))
            .await
            .expect("one failed download must be retried, not end the render");
        assert_eq!(videos[0].bytes.as_ref(), &[8, 9]);
        assert!(
            posted_bodies(&server, "/queue").await.is_empty(),
            "a clip that landed must not be cancelled"
        );
    }

    #[tokio::test]
    async fn a_terminal_http_error_cancels_the_prompt_and_keeps_its_history() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "gone").await;
        mock_finished_clip(&server, "gone").await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        mock_queue_delete(&server).await;
        let result = generate_clip(video_config(&server)).await;
        assert!(matches!(result, Err(Error::Http(_))), "{result:?}");
        assert_eq!(
            posted_bodies(&server, "/queue").await,
            [json!({"delete": ["gone"]})],
            "a terminal error must delete the accepted prompt"
        );
        assert!(
            posted_bodies(&server, "/history").await.is_empty(),
            "history is cleared only after a clip lands"
        );
    }

    #[tokio::test]
    async fn a_prompt_comfyui_lost_fails_long_before_the_deadline() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "lost").await;
        empty_history("lost").mount(&server).await;
        mock_queue(&server, json!([])).await;
        mock_queue_delete(&server).await;
        let config = video_config(&server).with_queue_check_interval(Duration::ZERO);
        let result = tokio::time::timeout(REQUEST_WAIT, generate_clip(config))
            .await
            .expect("a lost prompt must fail instead of waiting out the generation deadline");
        assert!(
            matches!(result, Err(Error::InvalidResponse(LOST_PROMPT))),
            "{result:?}"
        );
        assert_eq!(
            history_reads(&server, "lost").await,
            2,
            "history is read once more after the queue misses the prompt"
        );
        assert_eq!(
            posted_bodies(&server, "/queue").await,
            [json!({"delete": ["lost"]})]
        );
    }

    #[tokio::test]
    async fn a_prompt_that_finishes_between_history_and_queue_reads_is_collected() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "raced").await;
        empty_history("raced").up_to_n_times(1).mount(&server).await;
        serve_video(&server, "raced").await;
        mock_queue(&server, json!([])).await;
        let config = video_config(&server).with_queue_check_interval(Duration::ZERO);
        let videos = generate_clip(config)
            .await
            .expect("a prompt that left the queue for history is finished, not lost");
        assert_eq!(videos[0].bytes.as_ref(), &[1, 2, 3, 4]);
        assert!(posted_bodies(&server, "/queue").await.is_empty());
    }

    #[tokio::test]
    async fn a_prompt_still_in_the_queue_keeps_polling() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "queued").await;
        empty_history("queued")
            .up_to_n_times(3)
            .mount(&server)
            .await;
        serve_video(&server, "queued").await;
        mock_queue(&server, json!([[0, "queued", {}, {}, []]])).await;
        let config = video_config(&server).with_queue_check_interval(Duration::ZERO);
        let videos = generate_clip(config)
            .await
            .expect("a running prompt must not be taken for a lost one");
        assert_eq!(videos[0].bytes.as_ref(), &[1, 2, 3, 4]);
        assert_eq!(history_reads(&server, "queued").await, 4);
    }

    #[tokio::test]
    async fn a_comfyui_that_goes_away_fails_with_a_connect_error_within_the_window() {
        let server = MockServer::builder().start().await;
        mock_video_prompt(&server, "dead").await;
        empty_history("dead").mount(&server).await;
        let outage = Duration::from_secs(1);
        let config = video_config(&server).with_outage_window(outage);
        let render = tokio::spawn(generate_clip(config));
        wait_for_request(&server, "/prompt").await;
        let started = std::time::Instant::now();
        drop(server);
        let result = tokio::time::timeout(REQUEST_WAIT, render)
            .await
            .expect("a dead ComfyUI must fail within the outage window, not the deadline")
            .expect("the render task should not panic");
        let elapsed = started.elapsed();
        assert!(
            matches!(&result, Err(Error::Http(error)) if error.is_connect()),
            "{result:?}"
        );
        assert!(
            elapsed >= outage,
            "a refused connection must be retried for the outage window, gave up after {elapsed:?}"
        );
        assert!(elapsed < REQUEST_WAIT, "took {elapsed:?}");
    }

    #[tokio::test]
    async fn a_malformed_queue_answer_fails_the_render_and_names_the_queue_check() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "garbled").await;
        empty_history("garbled").mount(&server).await;
        Mock::given(method("GET"))
            .and(path("/queue"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>proxy</html>"))
            .mount(&server)
            .await;
        mock_queue_delete(&server).await;
        let config = video_config(&server)
            .with_queue_check_interval(Duration::ZERO)
            .with_outage_window(Duration::from_secs(60));
        let (logs, _logging) = crate::logs::Logs::capture();
        let result = tokio::time::timeout(REQUEST_WAIT, generate_clip(config))
            .await
            .expect("a queue answer that can never parse must not be retried as an outage");
        assert!(
            matches!(&result, Err(Error::Http(error)) if error.is_decode()),
            "{result:?}"
        );
        assert!(
            logs.text().contains("ComfyUI queue check failed"),
            "{}",
            logs.text()
        );
        assert_eq!(
            posted_bodies(&server, "/queue").await,
            [json!({"delete": ["garbled"]})]
        );
    }

    #[tokio::test]
    async fn a_render_that_outlasts_an_outage_does_not_wait_on_its_cancel() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "down").await;
        Mock::given(method("GET"))
            .and(path("/history/down"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        mock_unanswered_queue_delete(&server).await;
        let config = video_config(&server).with_outage_window(Duration::from_secs(1));
        let result = tokio::time::timeout(REQUEST_WAIT, generate_clip(config))
            .await
            .expect("a cancel sent after the outage window must not hold the render");
        assert!(
            matches!(
                &result,
                Err(Error::Http(error))
                    if error.status() == Some(reqwest::StatusCode::INTERNAL_SERVER_ERROR)
            ),
            "{result:?}"
        );
        assert_eq!(
            posted_bodies(&server, "/queue").await,
            [json!({"delete": ["down"]})]
        );
    }

    #[tokio::test]
    async fn a_render_whose_download_fails_does_not_wait_out_a_stalled_cancel() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "gone").await;
        mock_finished_clip(&server, "gone").await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        mock_unanswered_queue_delete(&server).await;
        let result = tokio::time::timeout(REQUEST_WAIT, generate_clip(video_config(&server)))
            .await
            .expect("a stalled cancel must not hold a failed render past its wait");
        assert!(
            matches!(
                &result,
                Err(Error::Http(error)) if error.status() == Some(reqwest::StatusCode::NOT_FOUND)
            ),
            "{result:?}"
        );
        assert_eq!(
            posted_bodies(&server, "/queue").await,
            [json!({"delete": ["gone"]})]
        );
    }

    #[tokio::test]
    async fn a_download_that_keeps_failing_gives_up_within_the_window() {
        let server = MockServer::start().await;
        mock_video_prompt(&server, "stuck").await;
        mock_finished_clip(&server, "stuck").await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;
        mock_unanswered_queue_delete(&server).await;
        let config = video_config(&server).with_outage_window(Duration::from_secs(1));
        let result = tokio::time::timeout(REQUEST_WAIT, generate_clip(config))
            .await
            .expect("a failing download must give up within the outage window");
        assert!(
            matches!(
                &result,
                Err(Error::Http(error))
                    if error.status() == Some(reqwest::StatusCode::SERVICE_UNAVAILABLE)
            ),
            "{result:?}"
        );
        assert_eq!(
            posted_bodies(&server, "/queue").await,
            [json!({"delete": ["stuck"]})]
        );
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

    fn write_video_adapter(root: &Path) -> PathBuf {
        let models = root.join("models");
        let lora = models.join("loras/jerry-video.safetensors");
        std::fs::create_dir_all(lora.parent().unwrap()).unwrap();
        std::fs::write(&lora, b"lora").unwrap();
        crate::inventory::write_sidecar(
            &lora,
            &crate::inventory::WeightSidecar::new(video_fixture::VIDEO_ADAPTER)
                .with_huggingface_base(video_fixture::VIDEO_HUGGINGFACE_BASE)
                .with_trigger("ohwx"),
            &crate::train::Contract::default(),
        )
        .unwrap();
        let base = models
            .join("diffusion_models")
            .join(video_fixture::VIDEO_BASE);
        std::fs::create_dir_all(base.parent().unwrap()).unwrap();
        std::fs::write(base, b"video").unwrap();
        models
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

    async fn generate_video_with(
        server: &MockServer,
        models: PathBuf,
        workflow_path: Option<PathBuf>,
        prompt: &str,
    ) {
        let client = Client::new(Config {
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            models_directory: models,
            workflow_path,
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
    async fn a_video_prompt_naming_an_identity_reaches_the_graph_unchanged() {
        const PROMPT: &str = "  ohwx walking on the beach\n";
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(BodyLacks("LoraLoaderModelOnly"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "still"})))
            .expect(1)
            .mount(&server)
            .await;
        serve_video(&server, "still").await;
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        write_still_identity(&models, "ohwx");

        generate_video_with(&server, models, None, PROMPT).await;
        let posted = posted_bodies(&server, "/prompt").await;
        assert_eq!(
            posted[0]["prompt"]["5"]["inputs"]["text"], PROMPT,
            "a video prompt reaches the graph exactly as written"
        );
    }

    #[tokio::test]
    async fn a_video_prompt_naming_a_video_adapter_loads_no_lora() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(BodyLacks("LoraLoaderModelOnly"))
            .and(BodyLacks("jerry-video.safetensors"))
            .and(wiremock::matchers::body_string_contains("ohwx"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "adapter"})))
            .expect(1)
            .mount(&server)
            .await;
        serve_video(&server, "adapter").await;
        let root = tempfile::tempdir().unwrap();
        let models = write_video_adapter(root.path());
        write_still_identity(&models, "ohwx");
        let overlay = video_fixture::write_overlay(
            &root.path().join("overlay"),
            &video_fixture::packaged_with_video_adapter(),
        );

        generate_video_with(&server, models, Some(overlay), "ohwx walking on the beach").await;
    }

    #[tokio::test]
    async fn generate_video_posts_the_configured_graph() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("\"width\":960"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "custom"})))
            .expect(1)
            .mount(&server)
            .await;
        serve_video(&server, "custom").await;
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("my-video.json");
        write_graph(&configured, &narrowed(PACKAGED_VIDEO_WORKFLOW));
        let config = Config {
            video_workflow_path: Some(configured),
            ..video_config(&server)
        };
        let videos = generate_clip(config).await.unwrap();
        assert_eq!(videos[0].bytes.as_ref(), &[1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn a_broken_text_to_video_graph_leaves_image_to_video_working() {
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
            .and(wiremock::matchers::body_string_contains("WanImageToVideo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "moving"})))
            .expect(1)
            .mount(&server)
            .await;
        serve_video(&server, "moving").await;
        let directory = tempfile::tempdir().unwrap();
        let configured = directory.path().join("my-video.json");
        let mut single_expert = packaged_graph(PACKAGED_VIDEO_WORKFLOW);
        single_expert.as_object_mut().unwrap().remove("14");
        write_graph(&configured, &single_expert);
        let config = Config {
            video_workflow_path: Some(configured),
            ..video_config(&server)
        };
        let client = Client::new(config.clone()).unwrap();
        let source = SourceImage::new(vec![1, 2, 3, 4], "image/png").unwrap();
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        let videos = client
            .generate_video("make it move", Some(&source), &mut cancel_rx, progress_tx)
            .await
            .expect("image-to-video must not load the text-to-video graph");
        assert_eq!(videos[0].bytes.as_ref(), &[1, 2, 3, 4]);
        assert!(matches!(
            generate_clip(config).await,
            Err(Error::Configuration(_))
        ));
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
    async fn people_identity_without_its_embedding_installed_leaves_the_token_out() {
        let server = MockServer::start().await;
        let (_root, models) =
            person_models(identity_sidecar().with_embedding("ohwx.safetensors"), &[]);
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "bare"})))
            .expect(1)
            .mount(&server)
            .await;
        mock_output(&server, "bare", "bare.png", vec![4, 5, 6]).await;
        let client = person_client(&server, models, "jerry.safetensors");
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        client
            .generate("ohwx sitting", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();
        let bodies = prompt_bodies(&server).await;
        assert!(
            !bodies[0].contains("embedding:ohwx"),
            "ComfyUI drops a token whose embedding is not installed: {}",
            bodies[0]
        );
        assert!(bodies[0].contains("ohwx sitting"), "{}", bodies[0]);
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

    async fn mock_first_pass(server: &MockServer, images: &[(&str, Vec<u8>)]) {
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("EmptyLatentImage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "first"})))
            .expect(1)
            .mount(server)
            .await;
        let listed: Vec<Value> = images
            .iter()
            .map(|(filename, _)| json!({"filename": filename, "subfolder": "", "type": "temp"}))
            .collect();
        Mock::given(method("GET"))
            .and(path("/history/first"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "first": {"status": {"status_str": "success"}, "outputs": {
                    "9": {"images": listed}
                }}
            })))
            .mount(server)
            .await;
        for (filename, body) in images {
            Mock::given(method("GET"))
                .and(path("/view"))
                .and(wiremock::matchers::query_param("filename", *filename))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "image/png")
                        .set_body_bytes(body.clone()),
                )
                .mount(server)
                .await;
        }
    }

    async fn mock_stuck_refine(server: &MockServer, submitted: u64) {
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains("\"denoise\":0.3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "refine"})))
            .expect(submitted)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/refine"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(server)
            .await;
    }

    fn refined_models() -> (tempfile::TempDir, PathBuf) {
        person_models(
            crate::inventory::WeightSidecar::new("sdxl-adapter").with_huggingface_base(PEOPLE_BASE),
            &[],
        )
    }

    #[tokio::test]
    async fn a_refine_the_time_limit_cuts_short_keeps_the_unrefined_images() {
        let server = MockServer::start().await;
        let (_root, models) = refined_models();
        let one = rgb_png(64, 96, [10, 20, 30]);
        let two = rgb_png(64, 96, [30, 20, 10]);
        mock_uploads(&server).await;
        mock_first_pass(
            &server,
            &[("one.png", one.clone()), ("two.png", two.clone())],
        )
        .await;
        mock_stuck_refine(&server, 1).await;
        let mut client = person_client(&server, models, "jerry.safetensors");
        client.config.generation_timeout = Duration::from_secs(5);
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();

        let images = client
            .generate("a person standing", None, &mut cancel_rx, progress_tx)
            .await
            .expect("generated images outlive a refine the time limit cut short");

        assert_eq!(images.len(), 2);
        assert_eq!(images[0].bytes.as_ref(), one.as_slice());
        assert_eq!(
            images[1].bytes.as_ref(),
            two.as_slice(),
            "no refine starts once the time limit has passed"
        );
    }

    #[tokio::test]
    async fn a_refine_with_less_than_one_poll_left_is_never_started() {
        let server = MockServer::start().await;
        let (_root, models) = refined_models();
        let one = rgb_png(64, 96, [10, 20, 30]);
        mock_uploads(&server).await;
        mock_first_pass(&server, &[("one.png", one.clone())]).await;
        mock_stuck_refine(&server, 0).await;
        let mut client = person_client(&server, models, "jerry.safetensors");
        client.config.poll_interval = Duration::from_secs(2);
        client.config.generation_timeout = Duration::from_secs(3);
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();

        let images = client
            .generate("a person standing", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();

        assert_eq!(images[0].bytes.as_ref(), one.as_slice());
    }

    #[tokio::test]
    async fn an_identity_prepares_its_generation_off_the_runtime() {
        let runtime = std::thread::current().id();
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
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "posed"})))
            .mount(&server)
            .await;
        mock_output(&server, "posed", "posed.png", vec![1, 2, 3]).await;
        let mut client = person_client(&server, models.clone(), "jerry.safetensors");
        client.config.poses_directory = Some(poses.path().to_path_buf());
        let (_cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();

        client
            .generate("ohwx sitting", None, &mut cancel_rx, progress_tx)
            .await
            .unwrap();

        let threads = probe::threads(&models);
        assert!(!threads.is_empty(), "nothing prepared the generation");
        assert!(
            threads.iter().all(|thread| *thread != runtime),
            "the generation read the disk on the runtime's thread"
        );
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
    }

    async fn stuck_in_preparation(
        configure: impl FnOnce(&mut Client),
        cancel_after: Option<Duration>,
    ) -> Result<Vec<GeneratedImage>, Error> {
        let server = MockServer::start().await;
        let (_root, models) = refined_models();
        let mut client = person_client(&server, models.clone(), "jerry.safetensors");
        configure(&mut client);
        let (cancel_tx, mut cancel_rx) = broadcast::channel(1);
        let (progress_tx, _) = mpsc::unbounded_channel();
        if let Some(after) = cancel_after {
            tokio::spawn(async move {
                tokio::time::sleep(after).await;
                let _ = cancel_tx.send(());
            });
        }
        probe::hold(&models);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.generate("a person standing", None, &mut cancel_rx, progress_tx),
        )
        .await;
        probe::release(&models);
        result.expect("a stuck preparation held the call")
    }

    #[tokio::test]
    async fn a_cancel_while_the_disk_is_read_returns_at_once() {
        let result = stuck_in_preparation(|_| {}, Some(Duration::from_millis(50))).await;
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
    }

    #[tokio::test]
    async fn a_deadline_while_the_disk_is_read_times_out() {
        let result = stuck_in_preparation(
            |client| client.config.generation_timeout = Duration::from_millis(100),
            None,
        )
        .await;
        assert!(matches!(result, Err(Error::Timeout)), "{result:?}");
    }
}
