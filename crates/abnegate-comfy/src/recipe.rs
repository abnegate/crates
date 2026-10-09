//! Packaged ComfyUI recipes: a graph pair plus the few slots a caller may write.
//!
//! Chat never picks a workflow. The selected checkpoint filename resolves to a
//! recipe; an attached image selects that recipe's `with_source` graph.

mod extra;
mod fill;
mod media_kind;
mod output;
mod pose;
mod prompt_mode;
mod required_file;
mod training_adapter;
mod training_model;

pub use extra::RecipeExtra;
pub use fill::Fill;
pub use media_kind::MediaKind;
pub use output::RecipeOutput;
pub use prompt_mode::PromptMode;
pub use required_file::RequiredFile;
pub use training_adapter::TrainingAdapter;
pub use training_model::TrainingModel;

use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use crate::client::Error;

/// The packaged graph a video prompt runs with a trained identity's LoRA.
pub(crate) const VIDEO_ADAPTER_WORKFLOW: &str = "wan-adapter-api.json";
/// The packaged graph an image-to-video prompt runs with a trained
/// identity's LoRA.
pub(crate) const IMAGE_TO_VIDEO_ADAPTER_WORKFLOW: &str = "wan-adapter-i2v-api.json";
/// The node class an adapter graph loads its LoRA with.
pub(crate) const LORA_LOADER: &str = "LoraLoaderModelOnly";
const PACKAGED_CATALOG: &str = include_str!("../comfyui/recipes/catalog.json");
const SELECTED_SLOTS: &[&str] = &["lora", "checkpoint", "unet"];
const ENCODER: &str = "VAEEncode";
const LATENT_INPUTS: &[&str] = &["latent_image", "samples"];
const CONDITIONING_CLASSES: &[&str] = &["FluxGuidance"];
const CONDITIONING_CLASS_PREFIXES: &[&str] = &["CLIPTextEncode", "Conditioning", "ControlNetApply"];
/// The directory of the IP-Adapter weight a face reference runs through.
pub(crate) const IPADAPTER_DIRECTORY: &str = "ipadapter";
/// The directory of the CLIP vision encoder a face reference is read with.
pub(crate) const CLIP_VISION_DIRECTORY: &str = "clip_vision";

fn packaged_workflow(name: &str) -> Option<&'static str> {
    match name {
        "flux1-schnell-fp8-api.json" => Some(include_str!(
            "../comfyui/workflows/flux1-schnell-fp8-api.json"
        )),
        "flux1-schnell-fp8-img2img-api.json" => Some(include_str!(
            "../comfyui/workflows/flux1-schnell-fp8-img2img-api.json"
        )),
        "sd15-api.json" => Some(include_str!("../comfyui/workflows/sd15-api.json")),
        "sd15-img2img-api.json" => Some(include_str!("../comfyui/workflows/sd15-img2img-api.json")),
        "sdxl-api.json" => Some(include_str!("../comfyui/workflows/sdxl-api.json")),
        "sdxl-img2img-api.json" => Some(include_str!("../comfyui/workflows/sdxl-img2img-api.json")),
        "sdxl-adapter-api.json" => Some(include_str!("../comfyui/workflows/sdxl-adapter-api.json")),
        "sdxl-adapter-img2img-api.json" => Some(include_str!(
            "../comfyui/workflows/sdxl-adapter-img2img-api.json"
        )),
        "flux1-schnell-fp8-adapter-api.json" => Some(include_str!(
            "../comfyui/workflows/flux1-schnell-fp8-adapter-api.json"
        )),
        "flux1-schnell-fp8-adapter-img2img-api.json" => Some(include_str!(
            "../comfyui/workflows/flux1-schnell-fp8-adapter-img2img-api.json"
        )),
        "flux1-dev-fp8-api.json" => {
            Some(include_str!("../comfyui/workflows/flux1-dev-fp8-api.json"))
        }
        "flux1-dev-fp8-img2img-api.json" => Some(include_str!(
            "../comfyui/workflows/flux1-dev-fp8-img2img-api.json"
        )),
        "flux1-dev-fp8-adapter-api.json" => Some(include_str!(
            "../comfyui/workflows/flux1-dev-fp8-adapter-api.json"
        )),
        "flux1-dev-fp8-adapter-img2img-api.json" => Some(include_str!(
            "../comfyui/workflows/flux1-dev-fp8-adapter-img2img-api.json"
        )),
        "qwen-image-edit-2511-api.json" => Some(include_str!(
            "../comfyui/workflows/qwen-image-edit-2511-api.json"
        )),
        "qwen-image-edit-2511-edit-api.json" => Some(include_str!(
            "../comfyui/workflows/qwen-image-edit-2511-edit-api.json"
        )),
        "qwen-image-edit-2511-adapter-api.json" => Some(include_str!(
            "../comfyui/workflows/qwen-image-edit-2511-adapter-api.json"
        )),
        "qwen-image-edit-2511-adapter-edit-api.json" => Some(include_str!(
            "../comfyui/workflows/qwen-image-edit-2511-adapter-edit-api.json"
        )),
        VIDEO_ADAPTER_WORKFLOW => Some(include_str!("../comfyui/workflows/wan-adapter-api.json")),
        IMAGE_TO_VIDEO_ADAPTER_WORKFLOW => Some(include_str!(
            "../comfyui/workflows/wan-adapter-i2v-api.json"
        )),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrainingArchitecture {
    Flux,
    QwenEdit,
    Sdxl,
}

/// One catalog entry: its workflow graph, the graph it runs with a source
/// image when it has one, and the slots a caller may fill.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Recipe {
    /// The catalog's name for the recipe.
    pub id: String,
    /// Whether it makes images or video.
    pub kind: MediaKind,
    /// The name shown to a person choosing a model.
    pub label: String,
    /// Whether its graph loads a LoRA adapter over the base weights.
    pub adapter: bool,
    /// How a prompt for it is written.
    pub prompt_mode: PromptMode,
    /// The weight filename each slot loads unless a selection replaces it,
    /// by slot name.
    pub defaults: HashMap<String, String>,
    /// The Hugging Face base models its weights derive from.
    pub huggingface_bases: Vec<String>,
    /// The model files its graph needs, and the directory each belongs in.
    pub required_files: Vec<RequiredFile>,
    /// The model files only its pose branch loads, such as the ControlNet
    /// weights, which a run with [`Fill::with_pose`] needs on top of
    /// [`required_files`](Recipe::required_files). Empty without a pose slot.
    pub pose_required_files: Vec<RequiredFile>,
    /// The model files a face reference loads on top of
    /// [`required_files`](Recipe::required_files): one IP-Adapter weight in
    /// `ipadapter/` and one CLIP vision encoder in `clip_vision/`. Declared
    /// exactly when the recipe supports [`RecipeExtra::Face`].
    pub face_required_files: Vec<RequiredFile>,
    /// The base recipe an edit of a source image switches to when that
    /// recipe's files are installed and the selected weight is one of this
    /// recipe's stock files, so a fast default can hand edits to a stronger
    /// model. See [`RecipeCatalog::edit_recipe_for`].
    pub edit_recipe: Option<String>,
    /// What its graph supports beyond its slots.
    pub extras: Vec<RecipeExtra>,
    training: Option<Training>,
    bare: Value,
    with_source: Option<Value>,
    slots: RecipeSlots,
}

#[derive(Debug, Clone)]
struct RecipeSlots {
    prompt: String,
    seed: String,
    source: Option<String>,
    pose: Option<String>,
    weights: HashMap<String, String>,
    output_node: String,
    output: RecipeOutput,
}

#[derive(Debug, Deserialize)]
struct CatalogFile {
    schema_version: u32,
    default_image: String,
    recipes: Vec<CatalogRecipe>,
}

#[derive(Debug, Deserialize)]
struct CatalogRecipe {
    id: String,
    kind: MediaKind,
    label: String,
    bare: String,
    with_source: Option<String>,
    output_node: String,
    output: RecipeOutput,
    slots: CatalogSlots,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    filename_hints: Vec<String>,
    #[serde(default)]
    adapter: bool,
    #[serde(default)]
    prompt_mode: PromptMode,
    #[serde(default)]
    defaults: HashMap<String, String>,
    #[serde(default, rename = "hf_bases")]
    huggingface_bases: Vec<String>,
    #[serde(default)]
    required_files: Vec<RequiredFile>,
    #[serde(default)]
    pose_required_files: Vec<RequiredFile>,
    #[serde(default)]
    face_required_files: Vec<RequiredFile>,
    #[serde(default)]
    training: Option<CatalogTraining>,
    #[serde(default)]
    edit_recipe: Option<String>,
    #[serde(default)]
    extras: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CatalogTraining {
    architecture: TrainingArchitecture,
    adapter: String,
    #[serde(rename = "hf_base")]
    huggingface_base: String,
}

#[derive(Debug, Clone)]
struct Training {
    architecture: TrainingArchitecture,
    adapter: TrainingAdapter,
}

#[derive(Debug, Deserialize)]
struct CatalogSlots {
    prompt: String,
    seed: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    pose: Option<String>,
    weights: HashMap<String, String>,
}

/// Every recipe this crate can run, and which weight file selects which.
#[derive(Clone)]
pub struct RecipeCatalog {
    default_image: String,
    recipes: Vec<Recipe>,
    files: HashMap<String, String>,
    hints: Vec<(String, String)>,
}

impl RecipeCatalog {
    /// The catalog compiled into the crate. Fails only if that catalog is
    /// itself inconsistent.
    pub fn packaged() -> Result<Self, Error> {
        Self::from_json(PACKAGED_CATALOG, None)
    }

    /// Load recipes from disk when `recipes/catalog.json` sits beside the
    /// workflow directory; otherwise use the packaged catalog. Graphs of the
    /// same filename in that workflow directory overlay the baked-in copies.
    pub fn load(workflow_path: Option<&Path>) -> Result<Self, Error> {
        let directory = workflow_path.and_then(Path::parent);
        if let Some(contents) = read_overlay_catalog(directory)? {
            Self::from_json(&contents, directory)
        } else {
            Self::from_json(PACKAGED_CATALOG, directory)
        }
    }

    fn from_json(json: &str, workflow_dir: Option<&Path>) -> Result<Self, Error> {
        let file: CatalogFile = serde_json::from_str(json)
            .map_err(|_| Error::Configuration("recipe catalog is not valid JSON"))?;
        if file.schema_version != 1 {
            return Err(Error::Configuration("unsupported recipe catalog schema"));
        }

        let mut recipes = Vec::with_capacity(file.recipes.len());
        let mut files = HashMap::new();
        let mut hints = Vec::new();

        for spec in file.recipes {
            let slots = RecipeSlots {
                prompt: spec.slots.prompt,
                seed: spec.slots.seed,
                source: spec.slots.source,
                pose: spec.slots.pose,
                weights: spec.slots.weights,
                output_node: spec.output_node,
                output: spec.output,
            };
            let bare = load_graph(workflow_dir, &spec.bare)?;
            validate_graph(&bare, &slots, false)?;
            let with_source = spec
                .with_source
                .as_deref()
                .map(|name| load_graph(workflow_dir, name))
                .transpose()?;
            if let Some(graph) = &with_source {
                validate_graph(graph, &slots, true)?;
            }
            if slots.pose.is_none() && !spec.pose_required_files.is_empty() {
                return Err(Error::Configuration(
                    "pose_required_files needs a pose slot",
                ));
            }
            let extras = known_extras(spec.extras);
            validate_face(&extras, &spec.face_required_files)?;
            if extras.contains(&RecipeExtra::Face) {
                for graph in std::iter::once(&bare).chain(with_source.as_ref()) {
                    validate_face_sampler(graph, &slots)?;
                }
            }
            for filename in spec.files {
                files.insert(filename, spec.id.clone());
            }
            for hint in spec.filename_hints {
                hints.push((hint.to_ascii_lowercase(), spec.id.clone()));
            }
            let recipe = Recipe {
                id: spec.id,
                kind: spec.kind,
                label: spec.label,
                adapter: spec.adapter,
                prompt_mode: spec.prompt_mode,
                defaults: spec.defaults,
                huggingface_bases: spec.huggingface_bases,
                required_files: spec.required_files,
                pose_required_files: spec.pose_required_files,
                face_required_files: spec.face_required_files,
                edit_recipe: spec.edit_recipe,
                extras,
                training: spec.training.map(|training| Training {
                    architecture: training.architecture,
                    adapter: TrainingAdapter {
                        recipe_id: training.adapter,
                        huggingface_base: training.huggingface_base,
                    },
                }),
                bare,
                with_source,
                slots,
            };
            recipes.push(recipe);
        }

        for recipe in recipes.iter().filter(|recipe| recipe.training.is_some()) {
            let model = recipe.training_model()?;
            let adapter = recipe.training_adapter()?;
            let adapter_recipe = recipes
                .iter()
                .find(|candidate| candidate.id == adapter.recipe_id)
                .ok_or(Error::Configuration(
                    "training metadata references an unknown adapter recipe",
                ))?;
            if !adapter_recipe.adapter
                || adapter_recipe.kind != recipe.kind
                || !adapter_recipe.has_lora_slot()
                || !recipe
                    .huggingface_bases
                    .iter()
                    .any(|base| base.eq_ignore_ascii_case(&adapter.huggingface_base))
                || !adapter_recipe
                    .huggingface_bases
                    .iter()
                    .any(|base| base.eq_ignore_ascii_case(&adapter.huggingface_base))
            {
                return Err(Error::Configuration(
                    "training metadata does not match its adapter recipe",
                ));
            }
            let prompt_mode = match model {
                TrainingModel::QwenEdit { .. } => PromptMode::EditInstruction,
                TrainingModel::Flux { .. } | TrainingModel::Sdxl { .. } => PromptMode::ClipScene,
            };
            if recipe.prompt_mode != prompt_mode || adapter_recipe.prompt_mode != prompt_mode {
                return Err(Error::Configuration(
                    "training architecture does not match its prompt mode",
                ));
            }
        }

        for recipe in &recipes {
            validate_edit_recipe(recipe, &recipes)?;
        }

        if !recipes
            .iter()
            .any(|recipe| recipe.id == file.default_image && recipe.kind == MediaKind::Image)
        {
            return Err(Error::Configuration(
                "recipe catalog default_image is missing",
            ));
        }

        Ok(Self {
            default_image: file.default_image,
            recipes,
            files,
            hints,
        })
    }

    /// The recipe named `id`, if any.
    pub fn get(&self, id: &str) -> Option<&Recipe> {
        self.recipes.iter().find(|recipe| recipe.id == id)
    }

    /// The base image recipe that runs `checkpoint`: the one listing it by
    /// name, else the one with the longest filename hint it contains, else
    /// the catalog's default. [`Error::Configuration`] when that recipe is
    /// missing or is not a base image recipe.
    pub fn image_recipe_for(&self, checkpoint: &str) -> Result<&Recipe, Error> {
        let id = self.resolve_image_id(checkpoint);
        self.get(id)
            .filter(|recipe| recipe.kind == MediaKind::Image && !recipe.adapter)
            .ok_or(Error::Configuration(
                "no image recipe matches this checkpoint",
            ))
    }

    /// Whether a recipe claims `filename` by name or by a filename hint,
    /// rather than it falling back to the default image recipe.
    pub fn is_explicit_image_match(&self, filename: &str) -> bool {
        let trimmed = filename.trim();
        self.files.contains_key(trimmed)
            || self.resolve_image_id(trimmed) != self.default_image.as_str()
    }

    fn resolve_image_id(&self, checkpoint: &str) -> &str {
        let trimmed = checkpoint.trim();
        if let Some(id) = self.files.get(trimmed) {
            return id;
        }
        let lower = trimmed.to_ascii_lowercase();
        let mut best_id: Option<&str> = None;
        let mut best_len = 0usize;
        for (hint, id) in &self.hints {
            if lower.contains(hint.as_str()) && hint.len() > best_len {
                best_id = Some(id);
                best_len = hint.len();
            }
        }
        best_id.unwrap_or(self.default_image.as_str())
    }

    /// Every image recipe, bases and adapters both, in catalog order.
    pub fn image_recipes(&self) -> impl Iterator<Item = &Recipe> {
        self.recipes
            .iter()
            .filter(|recipe| recipe.kind == MediaKind::Image)
    }

    /// Every Hugging Face base model an image recipe derives from, each once,
    /// in catalog order.
    pub fn huggingface_bases(&self) -> Vec<String> {
        let mut bases = Vec::new();
        for recipe in self.image_recipes() {
            for base in &recipe.huggingface_bases {
                if !bases.iter().any(|existing| existing == base) {
                    bases.push(base.clone());
                }
            }
        }
        bases
    }

    /// The base recipe an edit with `current` runs on when `selected` is the
    /// weight file chosen for it: its [`edit_recipe`](Recipe::edit_recipe)
    /// when `selected` is one of `current`'s own stock files and `ready` says
    /// the edit recipe's files are installed, otherwise `current` itself. A
    /// custom weight, such as a fine-tune that `current` merely runs, is never
    /// swapped for the edit recipe's defaults.
    pub fn edit_recipe_for<'a>(
        &'a self,
        current: &'a Recipe,
        selected: &str,
        ready: impl Fn(&Recipe) -> bool,
    ) -> &'a Recipe {
        if !self.is_stock_weight(current, selected) {
            return current;
        }
        current
            .edit_recipe
            .as_deref()
            .and_then(|id| self.get(id))
            .filter(|recipe| ready(recipe))
            .unwrap_or(current)
    }

    fn is_stock_weight(&self, recipe: &Recipe, selected: &str) -> bool {
        let selected = selected.trim();
        self.files.get(selected) == Some(&recipe.id)
            || recipe
                .selected_slot()
                .and_then(|slot| recipe.defaults.get(slot))
                .is_some_and(|default| default == selected)
    }

    /// Whether a recipe's defaults, its required files or the catalog's own
    /// file list name `filename`, ignoring ASCII case: a weight some graph
    /// loads, which nothing else may be written over.
    pub(crate) fn needs_weight(&self, filename: &str) -> bool {
        let named = |name: &str| name.eq_ignore_ascii_case(filename);
        self.files.keys().any(|name| named(name))
            || self.recipes.iter().any(|recipe| {
                recipe.defaults.values().any(|name| named(name))
                    || recipe
                        .required_files
                        .iter()
                        .chain(&recipe.pose_required_files)
                        .chain(&recipe.face_required_files)
                        .any(|file| named(&file.filename))
            })
    }

    /// The image recipe that loads a LoRA over `huggingface_base`, matched
    /// ignoring ASCII case, if any.
    pub fn adapter_recipe_for_base(&self, huggingface_base: &str) -> Option<&Recipe> {
        self.recipes.iter().find(|recipe| {
            recipe.kind == MediaKind::Image
                && recipe.adapter
                && recipe.has_lora_slot()
                && recipe
                    .huggingface_bases
                    .iter()
                    .any(|base| base.eq_ignore_ascii_case(huggingface_base))
        })
    }
}

impl Recipe {
    /// The model this recipe trains, as its catalog `training` metadata
    /// declares it, with each weight its defaults name.
    pub fn training_model(&self) -> Result<TrainingModel, Error> {
        let architecture = self
            .training
            .as_ref()
            .ok_or(Error::Configuration(
                "recipe does not declare a supported training architecture",
            ))?
            .architecture;
        match architecture {
            TrainingArchitecture::Flux => TrainingModel::flux(self.training_weight("checkpoint")?),
            TrainingArchitecture::QwenEdit => TrainingModel::qwen_edit(
                self.training_weight("unet")?,
                self.training_weight("clip")?,
                self.training_weight("vae")?,
            ),
            TrainingArchitecture::Sdxl => TrainingModel::sdxl(self.training_weight("checkpoint")?),
        }
    }

    /// The recipe that runs a LoRA trained on this one, or
    /// [`Error::Configuration`] when this recipe cannot be trained on.
    pub fn training_adapter(&self) -> Result<&TrainingAdapter, Error> {
        self.training
            .as_ref()
            .map(|training| &training.adapter)
            .ok_or(Error::Configuration(
                "recipe does not declare a supported training adapter",
            ))
    }

    fn training_weight(&self, name: &str) -> Result<&str, Error> {
        self.defaults
            .get(name)
            .map(String::as_str)
            .ok_or(Error::Configuration(
                "training metadata references a missing model weight",
            ))
    }

    fn selected_slot(&self) -> Option<&'static str> {
        SELECTED_SLOTS
            .iter()
            .copied()
            .find(|slot| self.slots.weights.contains_key(*slot))
    }

    /// Whether its graph has a slot for a LoRA file.
    pub fn has_lora_slot(&self) -> bool {
        self.slots.weights.contains_key("lora")
    }

    /// Whether its graph has a slot named `slot` for a weight file.
    pub(crate) fn has_weight_slot(&self, slot: &str) -> bool {
        self.slots.weights.contains_key(slot)
    }

    /// Whether its graph has a ControlNet branch steered by a pose image.
    pub fn has_pose_slot(&self) -> bool {
        self.slots.pose.is_some()
    }

    /// Whether its catalog entry declares `extra`.
    pub fn supports(&self, extra: RecipeExtra) -> bool {
        self.extras.contains(&extra)
    }

    /// The one file of its [`face_required_files`](Recipe::face_required_files)
    /// in `directory`.
    pub(crate) fn face_file(&self, directory: &str) -> Option<&RequiredFile> {
        self.face_required_files
            .iter()
            .find(|file| file.directory == directory)
    }

    /// The node its seed slot fills, the sampler a face reference and a
    /// denoise override reach.
    pub(crate) fn sampler_node(&self) -> &str {
        slot_node(&self.slots.seed)
    }

    /// The weight file for each slot as its defaults name them, for running
    /// the recipe on its own weights. [`Error::Configuration`] when a slot has
    /// no default.
    pub fn default_weight_map(&self) -> Result<HashMap<String, String>, Error> {
        let mut weights = HashMap::with_capacity(self.slots.weights.len());
        for name in self.slots.weights.keys() {
            let filename = self
                .defaults
                .get(name)
                .ok_or(Error::Configuration("recipe weight is missing"))?;
            weights.insert(name.clone(), sanitize_weight_filename(filename)?);
        }
        Ok(weights)
    }

    /// The weight file for each slot with `selected` in place: in the LoRA
    /// slot when there is one, else the checkpoint slot, else the UNet slot.
    /// [`Error::Configuration`] when `selected` is not a bare filename or a
    /// slot is left without a file.
    pub fn weight_map(&self, selected: &str) -> Result<HashMap<String, String>, Error> {
        let selected = sanitize_weight_filename(selected)?;
        let mut weights = self.defaults.clone();
        if let Some(slot) = self.selected_slot() {
            weights.insert(slot.to_string(), selected);
        }
        for name in self.slots.weights.keys() {
            if !weights.contains_key(name) {
                return Err(Error::Configuration("recipe weight is missing"));
            }
        }
        Ok(weights)
    }

    /// The workflow graph with `fill` written into its slots, the
    /// source-image graph when `fill` has a source. Without a pose the
    /// ControlNet branch a pose slot feeds is removed, so the sampler reads
    /// the plain prompt. [`Error::Configuration`] when the prompt is blank or
    /// over 100,000 bytes, a file name is unsafe, or the graph lacks a slot
    /// `fill` needs.
    pub fn apply(&self, fill: Fill<'_>) -> Result<Value, Error> {
        if fill.prompt.trim().is_empty() || fill.prompt.len() > 100_000 {
            return Err(Error::Configuration("prompt is empty or too long"));
        }
        let mut workflow = if fill.source.is_some() {
            self.with_source
                .clone()
                .ok_or(Error::Configuration("recipe has no source-image graph"))?
        } else {
            self.bare.clone()
        };
        set_pointer(&mut workflow, &self.slots.prompt, json!(fill.prompt))?;
        set_pointer(&mut workflow, &self.slots.seed, json!(fill.seed))?;
        for (name, pointer) in &self.slots.weights {
            let filename = fill
                .weights
                .get(name.as_str())
                .copied()
                .ok_or(Error::Configuration("recipe weight is missing"))?;
            let filename = sanitize_weight_filename(filename)?;
            set_pointer(&mut workflow, pointer, json!(filename))?;
        }
        if let Some(source) = fill.source {
            let pointer = self
                .slots
                .source
                .as_deref()
                .ok_or(Error::Configuration("recipe has no source-image slot"))?;
            set_pointer(&mut workflow, pointer, json!(sanitize_upload_name(source)?))?;
        }
        match (fill.pose, self.slots.pose.as_deref()) {
            (Some(pose), Some(pointer)) => {
                set_pointer(&mut workflow, pointer, json!(sanitize_upload_name(pose)?))?;
            }
            (Some(_), None) => return Err(Error::Configuration("recipe has no pose slot")),
            (None, Some(pointer)) => pose::strip(&mut workflow, slot_node(pointer))?,
            (None, None) => {}
        }
        Ok(workflow)
    }
}

fn known_extras(extras: Vec<String>) -> Vec<RecipeExtra> {
    extras
        .into_iter()
        .filter_map(|extra| serde_json::from_value(Value::String(extra)).ok())
        .collect()
}

fn read_overlay_catalog(workflow_dir: Option<&Path>) -> Result<Option<String>, Error> {
    let Some(path) = workflow_dir
        .and_then(Path::parent)
        .map(|root| root.join("recipes").join("catalog.json"))
        .filter(|path| path.is_file())
    else {
        return Ok(None);
    };
    std::fs::read_to_string(path)
        .map(Some)
        .map_err(|_| Error::Configuration("recipe catalog is not readable"))
}

fn load_graph(directory: Option<&Path>, filename: &str) -> Result<Value, Error> {
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err(Error::Configuration("invalid workflow filename"));
    }
    if let Some(path) = directory
        .map(|directory| directory.join(filename))
        .filter(|path| path.is_file())
    {
        let contents = std::fs::read_to_string(path)
            .map_err(|_| Error::Configuration("workflow file is not readable"))?;
        return serde_json::from_str(&contents)
            .map_err(|_| Error::Configuration("workflow file is not valid JSON"));
    }
    let packaged =
        packaged_workflow(filename).ok_or(Error::Configuration("packaged workflow is missing"))?;
    serde_json::from_str(packaged)
        .map_err(|_| Error::Configuration("packaged workflow is not valid JSON"))
}

fn validate_graph(workflow: &Value, slots: &RecipeSlots, with_source: bool) -> Result<(), Error> {
    require_pointer(workflow, &slots.prompt)?;
    require_pointer(workflow, &slots.seed)?;
    for pointer in slots.weights.values() {
        require_pointer(workflow, pointer)?;
    }
    if with_source {
        let source = slots.source.as_deref().ok_or(Error::Configuration(
            "source recipe is missing a source slot",
        ))?;
        require_pointer(workflow, source)?;
        if workflow
            .pointer(&source_class_pointer(source))
            .and_then(Value::as_str)
            != Some("LoadImage")
        {
            return Err(Error::Configuration(
                "source graph must load a source image",
            ));
        }
        validate_source_encode(workflow, source)?;
    }
    if let Some(pose) = slots.pose.as_deref() {
        validate_pose(workflow, pose)?;
    }
    let output_class = format!("/{}/class_type", slots.output_node);
    let output = workflow.pointer(&output_class).and_then(Value::as_str);
    match slots.output {
        RecipeOutput::PreviewImage => {
            if output != Some("PreviewImage") {
                return Err(Error::Configuration(
                    "workflow output must use temporary PreviewImage storage",
                ));
            }
        }
        RecipeOutput::SaveWebm => {
            if output != Some("SaveWEBM") {
                return Err(Error::Configuration(
                    "video workflow output must use SaveWEBM storage",
                ));
            }
        }
    }
    Ok(())
}

fn validate_face(extras: &[RecipeExtra], files: &[RequiredFile]) -> Result<(), Error> {
    if !extras.contains(&RecipeExtra::Face) {
        return if files.is_empty() {
            Ok(())
        } else {
            Err(Error::Configuration(
                "face_required_files needs the face extra",
            ))
        };
    }
    let count = |directory: &str| {
        files
            .iter()
            .filter(|file| file.directory == directory)
            .count()
    };
    if files.len() != 2 || count(IPADAPTER_DIRECTORY) != 1 || count(CLIP_VISION_DIRECTORY) != 1 {
        return Err(Error::Configuration(
            "the face extra needs one ipadapter and one clip_vision file",
        ));
    }
    for file in files {
        sanitize_weight_filename(&file.filename)?;
    }
    Ok(())
}

fn validate_face_sampler(workflow: &Value, slots: &RecipeSlots) -> Result<(), Error> {
    let model = format!("/{}/inputs/model", slot_node(&slots.seed));
    if workflow.pointer(&model).and_then(link_node).is_none() {
        return Err(Error::Configuration(
            "the face extra needs a sampler that reads a linked model",
        ));
    }
    Ok(())
}

fn validate_pose(workflow: &Value, pointer: &str) -> Result<(), Error> {
    require_pointer(workflow, pointer)?;
    if workflow
        .pointer(&source_class_pointer(pointer))
        .and_then(Value::as_str)
        != Some("LoadImage")
    {
        return Err(Error::Configuration("pose slot must fill a LoadImage node"));
    }
    pose::strip(&mut workflow.clone(), slot_node(pointer))
}

fn slot_node(pointer: &str) -> &str {
    pointer
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default()
}

fn validate_edit_recipe(recipe: &Recipe, recipes: &[Recipe]) -> Result<(), Error> {
    let Some(id) = recipe.edit_recipe.as_deref() else {
        return Ok(());
    };
    let edit = recipes
        .iter()
        .find(|candidate| candidate.id == id)
        .ok_or(Error::Configuration(
            "edit_recipe references an unknown recipe",
        ))?;
    if recipe.adapter
        || recipe.kind != MediaKind::Image
        || edit.adapter
        || edit.kind != MediaKind::Image
        || edit.with_source.is_none()
        || edit.prompt_mode != recipe.prompt_mode
    {
        return Err(Error::Configuration(
            "edit_recipe must name a base image recipe that edits with the same prompt mode",
        ));
    }
    edit.default_weight_map().map(|_| ())
}

fn source_class_pointer(source_slot: &str) -> String {
    match source_slot.rsplit_once("/inputs/") {
        Some((prefix, _)) => format!("{prefix}/class_type"),
        None => source_slot.to_string(),
    }
}

fn validate_source_encode(workflow: &Value, source: &str) -> Result<(), Error> {
    let nodes = workflow
        .as_object()
        .ok_or(Error::Configuration("workflow is not an object"))?;
    let source_node = slot_node(source);
    for encoder in sampled_encoders(nodes) {
        let (pixels, image) = encoder
            .pointer("/inputs/pixels")
            .and_then(link_node)
            .and_then(|id| nodes.get(id).map(|node| (id, node)))
            .ok_or(Error::Configuration(
                "VAEEncode pixels must come from an image node",
            ))?;
        if class_type(image).is_some_and(produces_conditioning) {
            return Err(Error::Configuration(
                "VAEEncode pixels must not read conditioning",
            ));
        }
        if !ancestors(nodes, pixels).contains(source_node) {
            return Err(Error::Configuration(
                "VAEEncode pixels must descend from the source image",
            ));
        }
    }
    Ok(())
}

fn sampled_encoders(nodes: &Map<String, Value>) -> Vec<&Value> {
    let mut pending: Vec<&str> = nodes
        .values()
        .filter_map(|node| node.pointer("/inputs/latent_image"))
        .filter_map(link_node)
        .collect();
    let mut visited = HashSet::new();
    let mut encoders = Vec::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        let Some(node) = nodes.get(id) else {
            continue;
        };
        if class_type(node) == Some(ENCODER) {
            encoders.push(node);
            continue;
        }
        pending.extend(
            LATENT_INPUTS
                .iter()
                .filter_map(|input| node.get("inputs")?.get(*input))
                .filter_map(link_node),
        );
    }
    encoders
}

fn ancestors<'a>(nodes: &'a Map<String, Value>, start: &'a str) -> HashSet<&'a str> {
    let mut pending = vec![start];
    let mut visited = HashSet::new();
    while let Some(id) = pending.pop() {
        if !visited.insert(id) {
            continue;
        }
        if let Some(inputs) = nodes
            .get(id)
            .and_then(|node| node.get("inputs"))
            .and_then(Value::as_object)
        {
            pending.extend(inputs.values().filter_map(link_node));
        }
    }
    visited
}

fn produces_conditioning(class: &str) -> bool {
    CONDITIONING_CLASSES.contains(&class)
        || CONDITIONING_CLASS_PREFIXES
            .iter()
            .any(|prefix| class.starts_with(prefix))
}

fn class_type(node: &Value) -> Option<&str> {
    node.get("class_type").and_then(Value::as_str)
}

fn link_node(value: &Value) -> Option<&str> {
    match value.as_array()?.as_slice() {
        [Value::String(node), Value::Number(_)] => Some(node),
        _ => None,
    }
}

fn require_pointer(workflow: &Value, pointer: &str) -> Result<(), Error> {
    if workflow.pointer(pointer).is_none() {
        return Err(Error::Configuration(
            "workflow does not match the recipe slot contract",
        ));
    }
    Ok(())
}

fn set_pointer(root: &mut Value, pointer: &str, value: Value) -> Result<(), Error> {
    if !pointer.starts_with('/') || pointer.len() < 2 || pointer.contains("//") {
        return Err(Error::Configuration("invalid recipe slot pointer"));
    }
    let mut current = root;
    let parts: Vec<&str> = pointer[1..].split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            return Err(Error::Configuration("invalid recipe slot pointer"));
        }
        if index + 1 == parts.len() {
            let object = current
                .as_object_mut()
                .ok_or(Error::Configuration("recipe slot pointer is not an object"))?;
            object.insert((*part).to_string(), value);
            return Ok(());
        }
        current = current.get_mut(*part).ok_or(Error::Configuration(
            "workflow does not match the recipe slot contract",
        ))?;
    }
    Err(Error::Configuration("invalid recipe slot pointer"))
}

/// `name` unchanged when it is a bare filename of at most 256 bytes, holding
/// no `/`, `\` or `..`; [`Error::Configuration`] otherwise.
pub fn sanitize_weight_filename(name: &str) -> Result<String, Error> {
    if name.is_empty()
        || name.len() > 256
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return Err(Error::Configuration("invalid checkpoint filename"));
    }
    Ok(name.to_string())
}

/// `name` unchanged when it is at most 128 bytes of ASCII letters, digits,
/// `.`, `-` and `_`, with no `..`; [`Error::Configuration`] otherwise.
pub fn sanitize_upload_name(name: &str) -> Result<String, Error> {
    if name.is_empty()
        || name.len() > 128
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        })
    {
        return Err(Error::Configuration("invalid source image filename"));
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> RecipeCatalog {
        RecipeCatalog::packaged().unwrap()
    }

    /// A Dev LoRA bound to the schnell graph would sample in four steps with no
    /// FluxGuidance and look like a training failure.
    #[test]
    fn dev_adapters_bind_to_the_dev_graph() {
        let catalog = RecipeCatalog::packaged().unwrap();
        let recipe = catalog
            .adapter_recipe_for_base("black-forest-labs/FLUX.1-dev")
            .expect("a dev base must resolve to an adapter recipe");
        assert_eq!(recipe.id, "flux-dev-adapter");

        let schnell = catalog
            .adapter_recipe_for_base("black-forest-labs/FLUX.1-schnell")
            .expect("a schnell base must resolve to an adapter recipe");
        assert_eq!(schnell.id, "flux-schnell-adapter");
    }

    /// An adapter recipe writes into a LoRA slot, so it must never be chosen for
    /// a plain checkpoint or the checkpoint lands in that slot.
    #[test]
    fn a_checkpoint_never_resolves_to_an_adapter_recipe() {
        let catalog = RecipeCatalog::packaged().unwrap();
        let mut checkpoints: Vec<&str> = catalog
            .recipes
            .iter()
            .filter(|recipe| recipe.kind == MediaKind::Image)
            .flat_map(|recipe| recipe.required_files.iter())
            .map(|file| file.filename.as_str())
            .collect();
        checkpoints.push("some-dev-build.safetensors");
        for checkpoint in checkpoints {
            let recipe = catalog.image_recipe_for(checkpoint).unwrap();
            assert!(
                !recipe.adapter,
                "{checkpoint} resolved to adapter recipe {}",
                recipe.id
            );
        }
    }

    #[test]
    fn packaged_catalog_validates_every_graph() {
        let catalog = catalog();
        assert!(catalog.get("flux-schnell").is_some());
        assert!(catalog.get("flux-schnell-adapter").is_some());
        assert!(catalog.get("flux-dev").is_some());
        assert!(catalog.get("flux-dev-adapter").is_some());
        assert!(catalog.get("sd15").is_some());
        assert!(catalog.get("sdxl").is_some());
        assert!(catalog.get("sdxl-adapter").is_some());
        assert!(catalog.get("qwen-image-edit").is_some());
        assert!(catalog.get("qwen-image-edit-adapter").is_some());
        let wan = catalog.get("wan-adapter").unwrap();
        assert!(wan.adapter);
        assert!(wan.has_lora_slot());
        assert_eq!(wan.kind, MediaKind::Video);
        for name in [
            "flux1-schnell-fp8-api.json",
            "flux1-schnell-fp8-img2img-api.json",
            "sd15-api.json",
            "sd15-img2img-api.json",
            "sdxl-api.json",
            "sdxl-img2img-api.json",
            "sdxl-adapter-api.json",
            "sdxl-adapter-img2img-api.json",
            "flux1-schnell-fp8-adapter-api.json",
            "flux1-schnell-fp8-adapter-img2img-api.json",
            "flux1-dev-fp8-api.json",
            "flux1-dev-fp8-img2img-api.json",
            "flux1-dev-fp8-adapter-api.json",
            "flux1-dev-fp8-adapter-img2img-api.json",
            "qwen-image-edit-2511-api.json",
            "qwen-image-edit-2511-edit-api.json",
            "qwen-image-edit-2511-adapter-api.json",
            "qwen-image-edit-2511-adapter-edit-api.json",
            "wan-adapter-api.json",
            "wan-adapter-i2v-api.json",
        ] {
            assert!(packaged_workflow(name).is_some(), "{name}");
        }
    }

    fn class_of<'a>(workflow: &'a Value, link: &Value) -> Option<&'a str> {
        let node = link.get(0).and_then(Value::as_str)?;
        workflow
            .get(node)
            .and_then(|source| source.get("class_type"))
            .and_then(Value::as_str)
    }

    fn nodes_of<'a>(workflow: &'a Value, class: &str) -> Vec<&'a Value> {
        workflow
            .as_object()
            .unwrap()
            .values()
            .filter(|node| node.get("class_type").and_then(Value::as_str) == Some(class))
            .collect()
    }

    /// Flux-dev graphs give FluxGuidance its own node. When the image-to-image
    /// graph reused the id of the scale node, VAEEncode read conditioning
    /// instead of pixels and ComfyUI refused every flux-dev edit with a 400.
    #[test]
    fn packaged_flux_dev_source_graphs_encode_pixels_and_guide_the_sampler() {
        for name in [
            "flux1-dev-fp8-img2img-api.json",
            "flux1-dev-fp8-adapter-img2img-api.json",
        ] {
            let workflow: Value = serde_json::from_str(packaged_workflow(name).unwrap()).unwrap();
            let encoders = nodes_of(&workflow, "VAEEncode");
            assert_eq!(encoders.len(), 1, "{name}");
            let pixels = &encoders[0]["inputs"]["pixels"];
            assert_ne!(
                class_of(&workflow, pixels),
                Some("FluxGuidance"),
                "{name}: VAEEncode.pixels must not read conditioning"
            );
            assert!(
                matches!(
                    class_of(&workflow, pixels),
                    Some("LoadImage" | "ImageScale")
                ),
                "{name}: VAEEncode.pixels must read an image"
            );
            let samplers = nodes_of(&workflow, "KSampler");
            assert_eq!(samplers.len(), 1, "{name}");
            let positive = &samplers[0]["inputs"]["positive"];
            assert_eq!(
                class_of(&workflow, positive),
                Some("FluxGuidance"),
                "{name}: KSampler.positive must read the guided conditioning"
            );
            assert_ne!(
                positive.get(0),
                pixels.get(0),
                "{name}: KSampler.positive and VAEEncode.pixels must not share a node"
            );
        }
    }

    #[test]
    fn flux_dev_source_graph_encodes_the_starting_image() {
        let catalog = catalog();
        for (id, selected) in [
            ("flux-dev", "flux1-dev-fp8.safetensors"),
            ("flux-dev-adapter", "identity.safetensors"),
        ] {
            let recipe = catalog.get(id).unwrap();
            let weights = recipe.weight_map(selected).unwrap();
            let owned: HashMap<&str, &str> = weights
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            let workflow = recipe
                .apply(Fill::new("make the frog fat", 7, owned).with_source("img2img-source.png"))
                .unwrap();
            assert_eq!(workflow["10"]["class_type"], "LoadImage");
            assert_eq!(workflow["10"]["inputs"]["image"], "img2img-source.png");
            assert_eq!(workflow["11"]["class_type"], "ImageScale");
            assert_eq!(workflow["11"]["inputs"]["image"], json!(["10", 0]));
            assert_eq!(workflow["12"]["class_type"], "VAEEncode");
            assert_eq!(workflow["12"]["inputs"]["pixels"], json!(["11", 0]));
            assert_eq!(workflow["14"]["class_type"], "FluxGuidance");
            assert_eq!(workflow["3"]["inputs"]["positive"], json!(["14", 0]));
            assert_eq!(workflow["3"]["inputs"]["latent_image"], json!(["12", 0]));
            assert_eq!(workflow["3"]["inputs"]["steps"], 40);
            assert_eq!(workflow["3"]["inputs"]["denoise"], 0.5);
            assert!(workflow.get("15").is_none());
            if id == "flux-dev-adapter" {
                assert_eq!(workflow["13"]["class_type"], "LoraLoaderModelOnly");
                assert_eq!(workflow["13"]["inputs"]["model"], json!(["4", 0]));
                assert_eq!(workflow["3"]["inputs"]["model"], json!(["13", 0]));
                assert_eq!(
                    workflow["13"]["inputs"]["lora_name"],
                    "identity.safetensors"
                );
            } else {
                assert_eq!(workflow["3"]["inputs"]["model"], json!(["4", 0]));
            }
        }
    }

    #[test]
    fn flux_schnell_source_graph_keeps_more_of_the_photograph() {
        let catalog = catalog();
        for (id, selected) in [
            ("flux-schnell", "flux1-schnell-fp8.safetensors"),
            ("flux-schnell-adapter", "identity.safetensors"),
        ] {
            let recipe = catalog.get(id).unwrap();
            let weights = recipe.weight_map(selected).unwrap();
            let owned: HashMap<&str, &str> = weights
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect();
            let workflow = recipe
                .apply(Fill::new("make the frog fat", 7, owned).with_source("img2img-source.png"))
                .unwrap();
            assert_eq!(workflow["3"]["inputs"]["steps"], 4);
            assert_eq!(workflow["3"]["inputs"]["denoise"], 0.6);
            assert_eq!(workflow["11"]["class_type"], "ImageScale");
            assert_eq!(workflow["12"]["inputs"]["pixels"], json!(["11", 0]));
            assert!(workflow.get("15").is_none());
        }
    }

    #[test]
    fn no_packaged_graph_loads_a_lora_its_catalog_does_not_name() {
        let catalog = catalog();
        for recipe in &catalog.recipes {
            for graph in std::iter::once(&recipe.bare).chain(recipe.with_source.as_ref()) {
                for node in graph.as_object().unwrap().values() {
                    let Some(name) = node.pointer("/inputs/lora_name").and_then(Value::as_str)
                    else {
                        continue;
                    };
                    assert!(
                        recipe.has_lora_slot(),
                        "{} loads {name} without a LoRA slot",
                        recipe.id
                    );
                }
            }
        }
    }

    #[test]
    fn source_graph_rejects_conditioning_as_encode_pixels() {
        let mut workflow: Value = serde_json::from_str(
            packaged_workflow("flux1-dev-fp8-img2img-api.json").expect("packaged flux-dev img2img"),
        )
        .unwrap();
        workflow["12"]["inputs"]["pixels"] = json!(["14", 0]);
        let slots = RecipeSlots {
            prompt: "/6/inputs/text".into(),
            seed: "/3/inputs/seed".into(),
            source: Some("/10/inputs/image".into()),
            pose: None,
            weights: HashMap::from([("checkpoint".into(), "/4/inputs/ckpt_name".into())]),
            output_node: "9".into(),
            output: RecipeOutput::PreviewImage,
        };
        assert!(
            validate_graph(&workflow, &slots, true)
                .unwrap_err()
                .to_string()
                .contains("VAEEncode")
        );
    }

    const FLUX_DEV_SOURCE: &str = "/10/inputs/image";

    fn flux_dev_source_graph() -> Value {
        serde_json::from_str(packaged_workflow("flux1-dev-fp8-img2img-api.json").unwrap()).unwrap()
    }

    /// Overlay graphs resize, upscale and crop the starting image in many
    /// ways; any chain that still starts at the source image encodes it.
    #[test]
    fn source_encode_accepts_any_image_chain_from_the_source() {
        let mut workflow = flux_dev_source_graph();
        workflow["11"] = json!({ "class_type": "ImageScaleBy", "inputs": {
            "image": ["10", 0], "upscale_method": "lanczos", "scale_by": 0.5
        }});
        validate_source_encode(&workflow, FLUX_DEV_SOURCE).unwrap();

        workflow["30"] = json!({ "class_type": "ImageScaleToTotalPixels", "inputs": {
            "image": ["11", 0], "upscale_method": "lanczos", "megapixels": 1.0
        }});
        workflow["31"] = json!({ "class_type": "ImageCrop", "inputs": {
            "image": ["30", 0], "width": 1024, "height": 1024, "x": 0, "y": 0
        }});
        workflow["12"]["inputs"]["pixels"] = json!(["31", 0]);
        validate_source_encode(&workflow, FLUX_DEV_SOURCE).unwrap();
    }

    /// An encoder outside the sampled latent, such as one encoding a reference
    /// image for conditioning, is not the starting image.
    #[test]
    fn source_encode_ignores_encoders_the_sampler_does_not_start_from() {
        let mut workflow = flux_dev_source_graph();
        workflow["30"] =
            json!({ "class_type": "LoadImage", "inputs": { "image": "reference.png" } });
        workflow["31"] = json!({ "class_type": "ImageInvert", "inputs": { "image": ["30", 0] } });
        workflow["32"] = json!({ "class_type": "VAEEncode", "inputs": {
            "pixels": ["31", 0], "vae": ["4", 2]
        }});
        validate_source_encode(&workflow, FLUX_DEV_SOURCE).unwrap();
    }

    #[test]
    fn source_encode_refuses_an_image_that_is_not_the_source() {
        let mut workflow = flux_dev_source_graph();
        workflow["30"] = json!({ "class_type": "LoadImage", "inputs": { "image": "other.png" } });
        workflow["11"]["inputs"]["image"] = json!(["30", 0]);
        assert!(
            validate_source_encode(&workflow, FLUX_DEV_SOURCE)
                .unwrap_err()
                .to_string()
                .contains("source image")
        );
    }

    #[test]
    fn source_encode_follows_the_latent_through_latent_nodes() {
        let mut workflow = flux_dev_source_graph();
        workflow["30"] = json!({ "class_type": "RepeatLatentBatch", "inputs": {
            "samples": ["12", 0], "amount": 2
        }});
        workflow["3"]["inputs"]["latent_image"] = json!(["30", 0]);
        validate_source_encode(&workflow, FLUX_DEV_SOURCE).unwrap();

        workflow["12"]["inputs"]["pixels"] = json!(["14", 0]);
        assert!(
            validate_source_encode(&workflow, FLUX_DEV_SOURCE)
                .unwrap_err()
                .to_string()
                .contains("conditioning")
        );
    }

    #[test]
    fn training_model_is_explicit_and_uses_only_catalog_defaults() {
        let catalog = catalog();
        assert_eq!(
            catalog.get("flux-dev").unwrap().training_model().unwrap(),
            TrainingModel::Flux {
                checkpoint: "flux1-dev-fp8.safetensors".into()
            }
        );
        assert_eq!(
            catalog
                .get("qwen-image-edit")
                .unwrap()
                .training_model()
                .unwrap(),
            TrainingModel::QwenEdit {
                unet: "qwen_image_edit_2511_fp8mixed.safetensors".into(),
                clip: "qwen_2.5_vl_7b_fp8_scaled.safetensors".into(),
                vae: "qwen_image_vae.safetensors".into(),
            }
        );
        assert_eq!(
            catalog
                .get("qwen-image-edit")
                .unwrap()
                .training_adapter()
                .unwrap(),
            &TrainingAdapter {
                recipe_id: "qwen-image-edit-adapter".into(),
                huggingface_base: "Qwen/Qwen-Image-Edit-2511".into(),
            }
        );
    }

    fn with_sdxl_training() -> RecipeCatalog {
        let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        let sdxl = file["recipes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|recipe| recipe["id"] == "sdxl")
            .unwrap();
        sdxl["defaults"] = json!({ "checkpoint": "RealVisXL_V5.0_fp16.safetensors" });
        sdxl["hf_bases"] = json!(["SG161222/RealVisXL_V5.0"]);
        sdxl["training"] = json!({
            "architecture": "sdxl",
            "adapter": "sdxl-adapter",
            "hf_base": "SG161222/RealVisXL_V5.0"
        });
        RecipeCatalog::from_json(&file.to_string(), None).unwrap()
    }

    #[test]
    fn sdxl_training_resolves_from_catalog_metadata() {
        let catalog = with_sdxl_training();
        let sdxl = catalog.get("sdxl").unwrap();
        assert_eq!(
            sdxl.training_model().unwrap(),
            TrainingModel::Sdxl {
                checkpoint: "RealVisXL_V5.0_fp16.safetensors".into()
            }
        );
        assert_eq!(
            sdxl.training_adapter().unwrap(),
            &TrainingAdapter {
                recipe_id: "sdxl-adapter".into(),
                huggingface_base: "SG161222/RealVisXL_V5.0".into(),
            }
        );
        assert_eq!(
            catalog
                .adapter_recipe_for_base("SG161222/RealVisXL_V5.0")
                .unwrap()
                .id,
            "sdxl-adapter"
        );
    }

    #[test]
    fn sdxl_training_needs_a_clip_scene_prompt() {
        let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for recipe in file["recipes"].as_array_mut().unwrap() {
            if recipe["id"] == "sdxl" {
                recipe["defaults"] = json!({ "checkpoint": "RealVisXL_V5.0_fp16.safetensors" });
                recipe["hf_bases"] = json!(["SG161222/RealVisXL_V5.0"]);
                recipe["prompt_mode"] = json!("edit_instruction");
                recipe["training"] = json!({
                    "architecture": "sdxl",
                    "adapter": "sdxl-adapter",
                    "hf_base": "SG161222/RealVisXL_V5.0"
                });
            }
        }
        assert!(RecipeCatalog::from_json(&file.to_string(), None).is_err());
    }

    #[test]
    fn training_never_infers_a_family_from_recipe_identity_or_prompt_mode() {
        let catalog = catalog();
        assert!(
            catalog
                .get("qwen-image-edit-adapter")
                .unwrap()
                .training_model()
                .is_err()
        );
        assert!(catalog.get("sd15").unwrap().training_model().is_err());
        assert!(catalog.get("sdxl").unwrap().training_model().is_err());
        assert!(
            catalog
                .get("sdxl-adapter")
                .unwrap()
                .training_model()
                .is_err()
        );
        assert!(
            catalog
                .get("wan-adapter")
                .unwrap()
                .training_model()
                .is_err()
        );
    }

    #[test]
    fn declared_training_fails_closed_when_a_weight_is_missing_or_pathful() {
        let catalog = catalog();
        let mut missing = catalog.get("qwen-image-edit").unwrap().clone();
        missing.defaults.remove("clip");
        assert!(missing.training_model().is_err());

        let mut pathful = catalog.get("flux-schnell").unwrap().clone();
        pathful
            .defaults
            .insert("checkpoint".into(), "../outside.safetensors".into());
        assert!(pathful.training_model().is_err());
    }

    #[test]
    fn checkpoint_filename_selects_recipe() {
        let catalog = catalog();
        assert_eq!(
            catalog
                .image_recipe_for("flux1-schnell-fp8.safetensors")
                .unwrap()
                .id,
            "flux-schnell"
        );
        assert_eq!(
            catalog
                .image_recipe_for("juggernautXL_ragnarok.safetensors")
                .unwrap()
                .id,
            "sdxl"
        );
        assert_eq!(
            catalog
                .image_recipe_for("RealVisXL_V5.0_fp16.safetensors")
                .unwrap()
                .id,
            "sdxl"
        );
        assert_eq!(
            catalog
                .image_recipe_for("mystery-finetune.safetensors")
                .unwrap()
                .id,
            "flux-schnell"
        );
        assert_eq!(
            catalog
                .image_recipe_for("qwen-image-edit-plus-nsfw-lora.safetensors")
                .unwrap()
                .id,
            "qwen-image-edit"
        );
        assert_eq!(
            catalog
                .image_recipe_for("v1-5-pruned-emaonly.safetensors")
                .unwrap()
                .id,
            "sd15"
        );
        assert_eq!(
            catalog
                .image_recipe_for("ponyDiffusionV6XL_v6.safetensors")
                .unwrap()
                .id,
            "sdxl"
        );
        assert_eq!(
            catalog.image_recipe_for("SD 1.5").unwrap().id,
            "flux-schnell"
        );
    }

    #[test]
    fn apply_writes_only_recipe_slots() {
        let catalog = catalog();
        let recipe = catalog
            .image_recipe_for("flux1-schnell-fp8.safetensors")
            .unwrap();
        let workflow = recipe
            .apply(Fill {
                prompt: "a blue fox",
                seed: 42,
                weights: HashMap::from([("checkpoint", "custom-image.safetensors")]),
                source: None,
                pose: None,
            })
            .unwrap();
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
    fn sdxl_recipe_keeps_packaged_sampler() {
        let catalog = catalog();
        let recipe = catalog
            .image_recipe_for("sd_xl_base_1.0.safetensors")
            .unwrap();
        let workflow = recipe
            .apply(Fill {
                prompt: "a fox",
                seed: 1,
                weights: HashMap::from([("checkpoint", "juggernautXL.safetensors")]),
                source: None,
                pose: None,
            })
            .unwrap();
        assert_eq!(workflow["3"]["inputs"]["steps"], 25);
        assert_eq!(workflow["5"]["inputs"]["width"], 1024);
        assert_eq!(
            workflow["4"]["inputs"]["ckpt_name"],
            "juggernautXL.safetensors"
        );
    }

    fn owned(weights: &HashMap<String, String>) -> HashMap<&str, &str> {
        weights
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    #[test]
    fn sdxl_lora_loads_clip_through_the_adapter() {
        let catalog = catalog();
        let recipe = catalog.get("sdxl-adapter").unwrap();
        let weights = recipe.weight_map("jerry.safetensors").unwrap();
        let workflow = recipe
            .apply(Fill::new(
                "ohwx person standing in a kitchen",
                3,
                owned(&weights),
            ))
            .unwrap();
        assert_eq!(workflow["13"]["class_type"], "LoraLoader");
        assert_eq!(workflow["13"]["inputs"]["lora_name"], "jerry.safetensors");
        assert_eq!(workflow["13"]["inputs"]["model"], json!(["4", 0]));
        assert_eq!(workflow["13"]["inputs"]["clip"], json!(["4", 1]));
        assert_eq!(workflow["6"]["inputs"]["clip"], json!(["13", 1]));
        assert_eq!(workflow["7"]["inputs"]["clip"], json!(["13", 1]));
        assert_eq!(workflow["3"]["inputs"]["model"], json!(["13", 0]));
        assert_eq!(
            workflow["4"]["inputs"]["ckpt_name"],
            "RealVisXL_V5.0_fp16.safetensors"
        );
        let edit = recipe
            .apply(
                Fill::new("ohwx person standing in a kitchen", 3, owned(&weights))
                    .with_source("img2img-source.png"),
            )
            .unwrap();
        assert_eq!(edit["10"]["class_type"], "LoadImage");
        assert_eq!(edit["11"]["class_type"], "ImageScale");
        assert_eq!(edit["12"]["class_type"], "VAEEncode");
        assert_eq!(edit["13"]["class_type"], "LoraLoader");
        assert_eq!(edit["3"]["inputs"]["latent_image"], json!(["12", 0]));
        assert_eq!(edit["3"]["inputs"]["model"], json!(["13", 0]));
    }

    #[test]
    fn sdxl_adapter_requires_only_its_base_checkpoint() {
        let catalog = catalog();
        let files: Vec<&str> = catalog
            .get("sdxl-adapter")
            .unwrap()
            .required_files
            .iter()
            .map(|file| file.filename.as_str())
            .collect();
        assert_eq!(files, ["RealVisXL_V5.0_fp16.safetensors"]);
    }

    #[test]
    fn a_posed_sdxl_graph_wires_openpose_controlnet() {
        let catalog = catalog();
        for (id, source) in [
            ("sdxl", None),
            ("sdxl", Some("img2img-source.png")),
            ("sdxl-adapter", None),
            ("sdxl-adapter", Some("img2img-source.png")),
        ] {
            let recipe = catalog.get(id).unwrap();
            assert!(recipe.has_pose_slot(), "{id}");
            let weights = recipe.weight_map("person.safetensors").unwrap();
            let mut fill = Fill::new("a person standing", 1, owned(&weights)).with_pose("pose.png");
            if let Some(source) = source {
                fill = fill.with_source(source);
            }
            let workflow = recipe.apply(fill).unwrap();
            assert_eq!(workflow["20"]["class_type"], "LoadImage");
            assert_eq!(workflow["20"]["inputs"]["image"], "pose.png");
            assert_eq!(workflow["21"]["class_type"], "ControlNetLoader");
            assert_eq!(
                workflow["21"]["inputs"]["control_net_name"],
                "thibaud_xl_openpose.safetensors"
            );
            assert_eq!(workflow["22"]["class_type"], "ControlNetApplyAdvanced");
            assert_eq!(workflow["22"]["inputs"]["strength"], 0.7);
            assert_eq!(workflow["3"]["inputs"]["positive"], json!(["22", 0]));
            assert_eq!(workflow["3"]["inputs"]["negative"], json!(["22", 1]));
            assert_eq!(workflow["22"]["inputs"]["positive"], json!(["6", 0]));
            assert_eq!(workflow["22"]["inputs"]["negative"], json!(["7", 0]));
        }
    }

    /// The packaged pose image only exists where a host uploads one, so an
    /// SDXL run without a pose must not reference it or the ControlNet weights.
    #[test]
    fn an_sdxl_graph_without_a_pose_drops_the_controlnet_branch() {
        let catalog = catalog();
        for (id, source) in [
            ("sdxl", None),
            ("sdxl", Some("img2img-source.png")),
            ("sdxl-adapter", None),
            ("sdxl-adapter", Some("img2img-source.png")),
        ] {
            let recipe = catalog.get(id).unwrap();
            let weights = recipe.weight_map("person.safetensors").unwrap();
            let mut fill = Fill::new("a person standing", 1, owned(&weights));
            if let Some(source) = source {
                fill = fill.with_source(source);
            }
            let workflow = recipe.apply(fill).unwrap();
            for node in ["20", "21", "22"] {
                assert!(workflow.get(node).is_none(), "{id} keeps node {node}");
            }
            assert_eq!(workflow["3"]["inputs"]["positive"], json!(["6", 0]));
            assert_eq!(workflow["3"]["inputs"]["negative"], json!(["7", 0]));
            assert!(!workflow.to_string().contains("ControlNet"), "{id}");
        }
    }

    #[test]
    fn a_pose_for_a_recipe_without_a_pose_slot_is_refused() {
        let catalog = catalog();
        let recipe = catalog.get("flux-schnell").unwrap();
        assert!(!recipe.has_pose_slot());
        let weights = recipe.weight_map("flux1-schnell-fp8.safetensors").unwrap();
        assert!(
            recipe
                .apply(Fill::new("a fox", 1, owned(&weights)).with_pose("pose.png"))
                .is_err()
        );
        let sdxl = catalog.get("sdxl").unwrap();
        let weights = sdxl.weight_map("person.safetensors").unwrap();
        assert!(
            sdxl.apply(Fill::new("a fox", 1, owned(&weights)).with_pose("../pose.png"))
                .is_err()
        );
    }

    #[test]
    fn a_pose_slot_that_does_not_load_an_image_fails_catalog_load() {
        let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for recipe in file["recipes"].as_array_mut().unwrap() {
            if recipe["id"] == "sdxl" {
                recipe["slots"]["pose"] = json!("/21/inputs/control_net_name");
            }
        }
        assert!(RecipeCatalog::from_json(&file.to_string(), None).is_err());
    }

    #[test]
    fn catalog_extras_name_what_each_graph_supports() {
        let catalog = catalog();
        let sdxl = catalog.get("sdxl").unwrap();
        assert!(sdxl.supports(RecipeExtra::Face));
        assert!(sdxl.supports(RecipeExtra::IdentityRefine));
        assert!(!sdxl.supports(RecipeExtra::Refine));
        let adapter = catalog.get("sdxl-adapter").unwrap();
        assert!(adapter.supports(RecipeExtra::Face));
        assert!(adapter.supports(RecipeExtra::Refine));
        let flux = catalog.get("flux-schnell").unwrap();
        assert!(flux.extras.is_empty());
    }

    /// An overlay catalog written for a later version may declare extras this
    /// one does not know; it still loads, with only the extras it can honour.
    #[test]
    fn an_unknown_extra_is_ignored() {
        let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for recipe in file["recipes"].as_array_mut().unwrap() {
            if recipe["id"] == "sdxl" {
                recipe["extras"] = json!(["teleport", "face"]);
            }
        }
        let catalog = RecipeCatalog::from_json(&file.to_string(), None).unwrap();
        assert_eq!(catalog.get("sdxl").unwrap().extras, [RecipeExtra::Face]);
    }

    /// The ControlNet weights only a posed run loads are declared, so a host
    /// can check them before it passes a pose without the inventory calling
    /// every unposed SDXL run unready.
    #[test]
    fn posed_sdxl_runs_declare_the_openpose_controlnet() {
        let catalog = catalog();
        for id in ["sdxl", "sdxl-adapter"] {
            let recipe = catalog.get(id).unwrap();
            assert_eq!(
                recipe.pose_required_files,
                [RequiredFile {
                    filename: "thibaud_xl_openpose.safetensors".into(),
                    directory: "controlnet".into(),
                }],
                "{id}"
            );
            assert!(
                !recipe
                    .required_files
                    .iter()
                    .any(|file| file.directory == "controlnet"),
                "{id}"
            );
            for graph in std::iter::once(&recipe.bare).chain(recipe.with_source.as_ref()) {
                for node in nodes_of(graph, "ControlNetLoader") {
                    let name = node["inputs"]["control_net_name"].as_str().unwrap();
                    assert!(
                        recipe
                            .pose_required_files
                            .iter()
                            .any(|file| file.filename == name),
                        "{id} loads {name} undeclared"
                    );
                }
            }
        }
        for recipe in &catalog.recipes {
            assert!(
                recipe.has_pose_slot() || recipe.pose_required_files.is_empty(),
                "{}",
                recipe.id
            );
        }
    }

    #[test]
    fn pose_files_without_a_pose_slot_fail_catalog_load() {
        let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for recipe in file["recipes"].as_array_mut().unwrap() {
            if recipe["id"] == "flux-schnell" {
                recipe["pose_required_files"] = json!([{
                    "filename": "thibaud_xl_openpose.safetensors",
                    "directory": "controlnet"
                }]);
            }
        }
        assert!(RecipeCatalog::from_json(&file.to_string(), None).is_err());
    }

    /// The IP-Adapter weight and the encoder it reads the face with are
    /// declared beside the face extra, so a host can install them and the
    /// client never runs a face branch whose weights are missing.
    #[test]
    fn face_capable_sdxl_runs_declare_the_ip_adapter_and_its_encoder() {
        let catalog = catalog();
        for id in ["sdxl", "sdxl-adapter"] {
            let recipe = catalog.get(id).unwrap();
            assert_eq!(
                recipe.face_required_files,
                [
                    RequiredFile {
                        filename: "ip-adapter-plus-face_sdxl_vit-h.safetensors".into(),
                        directory: IPADAPTER_DIRECTORY.into(),
                    },
                    RequiredFile {
                        filename: "CLIP-ViT-H-14-laion2B-s32B-b79K.safetensors".into(),
                        directory: CLIP_VISION_DIRECTORY.into(),
                    },
                ],
                "{id}"
            );
            assert_eq!(recipe.sampler_node(), "3");
            assert!(catalog.needs_weight("ip-adapter-plus-face_sdxl_vit-h.safetensors"));
        }
        for recipe in &catalog.recipes {
            assert_eq!(
                recipe.supports(RecipeExtra::Face),
                !recipe.face_required_files.is_empty(),
                "{}",
                recipe.id
            );
        }
    }

    fn with_face(id: &str, extras: Value, files: Value) -> Result<RecipeCatalog, Error> {
        let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for recipe in file["recipes"].as_array_mut().unwrap() {
            if recipe["id"] == id {
                recipe["extras"] = extras.clone();
                recipe["face_required_files"] = files.clone();
            }
        }
        RecipeCatalog::from_json(&file.to_string(), None)
    }

    #[test]
    fn face_files_without_the_face_extra_fail_catalog_load() {
        let files = json!([
            {"filename": "adapter.safetensors", "directory": "ipadapter"},
            {"filename": "encoder.safetensors", "directory": "clip_vision"}
        ]);
        assert!(with_face("flux-schnell", json!([]), files.clone()).is_err());
        assert!(with_face("sdxl", json!(["identity_refine"]), files).is_err());
    }

    #[test]
    fn the_face_extra_without_one_adapter_and_one_encoder_fails_catalog_load() {
        for files in [
            json!([]),
            json!([{"filename": "adapter.safetensors", "directory": "ipadapter"}]),
            json!([
                {"filename": "adapter.safetensors", "directory": "ipadapter"},
                {"filename": "other.safetensors", "directory": "ipadapter"}
            ]),
            json!([
                {"filename": "adapter.safetensors", "directory": "ipadapter"},
                {"filename": "encoder.safetensors", "directory": "clip_vision"},
                {"filename": "extra.safetensors", "directory": "controlnet"}
            ]),
            json!([
                {"filename": "../adapter.safetensors", "directory": "ipadapter"},
                {"filename": "encoder.safetensors", "directory": "clip_vision"}
            ]),
        ] {
            assert!(
                with_face("sdxl", json!(["face"]), files.clone()).is_err(),
                "{files}"
            );
        }
        let catalog = with_face(
            "flux-schnell",
            json!(["face"]),
            json!([
                {"filename": "adapter.safetensors", "directory": "ipadapter"},
                {"filename": "encoder.safetensors", "directory": "clip_vision"}
            ]),
        )
        .unwrap();
        assert!(
            catalog
                .get("flux-schnell")
                .unwrap()
                .supports(RecipeExtra::Face)
        );
    }

    #[test]
    fn the_face_extra_on_a_sampler_without_a_linked_model_fails_catalog_load() {
        let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for recipe in file["recipes"].as_array_mut().unwrap() {
            if recipe["id"] == "sdxl" {
                recipe["slots"]["seed"] = json!("/7/inputs/text");
            }
        }
        assert!(RecipeCatalog::from_json(&file.to_string(), None).is_err());
        for recipe in file["recipes"].as_array_mut().unwrap() {
            if recipe["id"] == "sdxl" {
                recipe["extras"] = json!(["identity_refine"]);
                recipe["face_required_files"] = json!([]);
            }
        }
        assert!(
            RecipeCatalog::from_json(&file.to_string(), None).is_ok(),
            "without the face extra the same graph loads"
        );
    }

    /// The packaged assets are the generic catalog: a host's own checkpoints
    /// and its uncensored LoRA stay in its overlay.
    #[test]
    fn packaged_assets_name_no_host_specific_model() {
        let mut pending = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("comfyui")];
        let mut graphs = 0;
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                pending.extend(
                    std::fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
                continue;
            }
            let contents = std::fs::read_to_string(&path).unwrap();
            let lower = contents.to_ascii_lowercase();
            for banned in ["lustify", "uncensored"] {
                assert!(!lower.contains(banned), "{} names {banned}", path.display());
            }
            if !path
                .parent()
                .is_some_and(|parent| parent.ends_with("workflows"))
            {
                continue;
            }
            let graph: Value = serde_json::from_str(&contents).unwrap();
            let class = graph
                .pointer("/15/class_type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !class.contains("Lora"),
                "{} loads a LoRA on node 15",
                path.display()
            );
            graphs += 1;
        }
        assert!(graphs > 0);
    }

    #[test]
    fn schnell_edits_hand_off_to_flux_dev_once_it_is_installed() {
        let catalog = catalog();
        let schnell = catalog.get("flux-schnell").unwrap();
        assert_eq!(schnell.edit_recipe.as_deref(), Some("flux-dev"));
        let stock = "flux1-schnell-fp8.safetensors";
        assert_eq!(
            catalog.edit_recipe_for(schnell, stock, |_| true).id,
            "flux-dev"
        );
        assert_eq!(
            catalog.edit_recipe_for(schnell, stock, |_| false).id,
            "flux-schnell"
        );
        let adapter = catalog.get("flux-schnell-adapter").unwrap();
        assert_eq!(
            catalog
                .edit_recipe_for(adapter, "identity.safetensors", |_| true)
                .id,
            "flux-schnell-adapter"
        );
        assert_eq!(
            catalog
                .get("flux-dev")
                .unwrap()
                .default_weight_map()
                .unwrap(),
            HashMap::from([(
                "checkpoint".to_string(),
                "flux1-dev-fp8.safetensors".to_string()
            )])
        );
    }

    #[test]
    fn an_edit_keeps_a_custom_weight_the_recipe_only_runs() {
        let catalog = catalog();
        let schnell = catalog
            .image_recipe_for("jerry-schnell.safetensors")
            .unwrap();
        assert_eq!(schnell.id, "flux-schnell");
        for custom in ["jerry.safetensors", "jerry-schnell.safetensors"] {
            assert_eq!(
                catalog.edit_recipe_for(schnell, custom, |_| true).id,
                "flux-schnell",
                "{custom}"
            );
        }
        let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for recipe in file["recipes"].as_array_mut().unwrap() {
            if recipe["id"] == "flux-schnell" {
                recipe["files"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!("flux1-schnell-fp8-e5m2.safetensors"));
            }
        }
        let listed = RecipeCatalog::from_json(&file.to_string(), None).unwrap();
        let schnell = listed.get("flux-schnell").unwrap();
        assert_eq!(
            listed
                .edit_recipe_for(schnell, "flux1-schnell-fp8-e5m2.safetensors", |_| true)
                .id,
            "flux-dev"
        );
    }

    #[test]
    fn an_edit_recipe_that_cannot_take_the_edit_fails_catalog_load() {
        let base: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for (owner, target) in [
            ("flux-schnell", "missing"),
            ("flux-schnell", "flux-dev-adapter"),
            ("flux-schnell", "qwen-image-edit"),
            ("flux-schnell", "wan-adapter"),
            ("flux-schnell", "sd15"),
            ("flux-schnell-adapter", "flux-dev"),
        ] {
            let mut file = base.clone();
            for recipe in file["recipes"].as_array_mut().unwrap() {
                if recipe["id"] == owner {
                    recipe["edit_recipe"] = json!(target);
                }
            }
            assert!(
                RecipeCatalog::from_json(&file.to_string(), None).is_err(),
                "{owner} -> {target}"
            );
        }
    }

    #[test]
    fn the_wan_adapter_writes_its_lora_into_a_webm_graph() {
        let catalog = catalog();
        let recipe = catalog.get("wan-adapter").unwrap();
        let weights = recipe.weight_map("identity-wan.safetensors").unwrap();
        let workflow = recipe
            .apply(Fill::new("a person waves", 5, owned(&weights)))
            .unwrap();
        assert_eq!(workflow["10"]["class_type"], "SaveWEBM");
        assert_eq!(workflow["12"]["class_type"], "LoraLoaderModelOnly");
        assert_eq!(
            workflow["12"]["inputs"]["lora_name"],
            "identity-wan.safetensors"
        );
        assert_eq!(workflow["4"]["inputs"]["model"], json!(["12", 0]));
        assert_eq!(
            workflow["1"]["inputs"]["unet_name"],
            "wan2.2_ti2v_5B_fp16.safetensors"
        );
        let animated = recipe
            .apply(Fill::new("a person waves", 5, owned(&weights)).with_source("i2v-source.png"))
            .unwrap();
        assert_eq!(animated["11"]["inputs"]["image"], "i2v-source.png");
        assert_eq!(animated["7"]["inputs"]["start_image"], json!(["11", 0]));
    }

    #[test]
    fn a_video_recipe_must_save_webm() {
        let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for recipe in file["recipes"].as_array_mut().unwrap() {
            if recipe["id"] == "wan-adapter" {
                recipe["output"] = json!("preview_image");
            }
        }
        assert!(RecipeCatalog::from_json(&file.to_string(), None).is_err());
    }

    #[test]
    fn source_graph_fills_load_image() {
        let catalog = catalog();
        let recipe = catalog.get("sd15").unwrap();
        let workflow = recipe
            .apply(Fill {
                prompt: "make it dusk",
                seed: 7,
                weights: HashMap::from([("checkpoint", "model.safetensors")]),
                source: Some("img2img-source.png"),
                pose: None,
            })
            .unwrap();
        assert_eq!(workflow["10"]["inputs"]["image"], "img2img-source.png");
        assert_eq!(workflow["11"]["inputs"]["width"], 512);
        assert_eq!(workflow["3"]["inputs"]["denoise"], 0.7);
    }

    #[test]
    fn rejects_pathful_weights_and_sources() {
        let catalog = catalog();
        let recipe = catalog.get("flux-schnell").unwrap();
        assert!(
            recipe
                .apply(Fill {
                    prompt: "fox",
                    seed: 1,
                    weights: HashMap::from([("checkpoint", "../secret")]),
                    source: None,
                    pose: None,
                })
                .is_err()
        );
        assert!(
            recipe
                .apply(Fill {
                    prompt: "fox",
                    seed: 1,
                    weights: HashMap::from([("checkpoint", "ok.safetensors")]),
                    source: Some("../secret.png"),
                    pose: None,
                })
                .is_err()
        );
    }

    #[test]
    fn unknown_packaged_workflow_name_is_rejected() {
        assert!(packaged_workflow("missing.json").is_none());
    }

    #[test]
    fn overlay_catalog_replaces_packaged_recipes() {
        let root = std::env::temp_dir().join(format!("recipe-overlay-{}", uuid::Uuid::new_v4()));
        let workflows = root.join("workflows");
        let recipes = root.join("recipes");
        std::fs::create_dir_all(&workflows).unwrap();
        std::fs::create_dir_all(&recipes).unwrap();
        std::fs::write(
            workflows.join("custom-api.json"),
            packaged_workflow("sd15-api.json").unwrap(),
        )
        .unwrap();
        std::fs::write(
            workflows.join("custom-img2img-api.json"),
            packaged_workflow("sd15-img2img-api.json").unwrap(),
        )
        .unwrap();
        std::fs::write(
            recipes.join("catalog.json"),
            r#"{
              "schema_version": 1,
              "default_image": "custom",
              "recipes": [{
                "id": "custom",
                "kind": "image",
                "label": "Custom",
                "bare": "custom-api.json",
                "with_source": "custom-img2img-api.json",
                "output_node": "9",
                "output": "preview_image",
                "slots": {
                  "prompt": "/6/inputs/text",
                  "seed": "/3/inputs/seed",
                  "source": "/10/inputs/image",
                  "weights": { "checkpoint": "/4/inputs/ckpt_name" }
                },
                "files": ["custom.safetensors"]
              }]
            }"#,
        )
        .unwrap();

        let catalog = RecipeCatalog::load(Some(&workflows.join("custom-api.json"))).unwrap();
        assert_eq!(
            catalog.image_recipe_for("custom.safetensors").unwrap().id,
            "custom"
        );
        assert!(catalog.get("flux-schnell").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn persistent_output_nodes_are_rejected() {
        let mut workflow: Value = serde_json::from_str(
            packaged_workflow("flux1-schnell-fp8-api.json").expect("packaged flux graph"),
        )
        .unwrap();
        workflow["9"]["class_type"] = json!("SaveImage");
        let slots = RecipeSlots {
            prompt: "/6/inputs/text".into(),
            seed: "/3/inputs/seed".into(),
            source: None,
            pose: None,
            weights: HashMap::from([("checkpoint".into(), "/4/inputs/ckpt_name".into())]),
            output_node: "9".into(),
            output: RecipeOutput::PreviewImage,
        };
        assert!(
            validate_graph(&workflow, &slots, false)
                .unwrap_err()
                .to_string()
                .contains("PreviewImage")
        );
    }

    #[test]
    fn qwen_edit_recipe_uses_instruction_prompt_slot() {
        let catalog = catalog();
        let recipe = catalog
            .image_recipe_for("qwen_image_edit_2511_fp8mixed.safetensors")
            .unwrap();
        assert_eq!(recipe.id, "qwen-image-edit");
        assert_eq!(recipe.prompt_mode, PromptMode::EditInstruction);
        let weights = recipe
            .weight_map("qwen_image_edit_2511_fp8mixed.safetensors")
            .unwrap();
        let owned: HashMap<&str, &str> = weights
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let workflow = recipe
            .apply(Fill {
                prompt: "remove the sign",
                seed: 9,
                weights: owned,
                source: Some("img2img-source.png"),
                pose: None,
            })
            .unwrap();
        assert_eq!(workflow["6"]["inputs"]["prompt"], "remove the sign");
        assert_eq!(
            workflow["1"]["inputs"]["unet_name"],
            "qwen_image_edit_2511_fp8mixed.safetensors"
        );
        assert_eq!(workflow["10"]["inputs"]["image"], "img2img-source.png");
    }

    #[test]
    fn adapter_recipe_writes_lora_filename() {
        let catalog = catalog();
        let recipe = catalog.get("qwen-image-edit-adapter").unwrap();
        let weights = recipe
            .weight_map("qwen-image-edit-plus-nsfw-lora.safetensors")
            .unwrap();
        assert_eq!(
            weights.get("lora").map(String::as_str),
            Some("qwen-image-edit-plus-nsfw-lora.safetensors")
        );
        let owned: HashMap<&str, &str> = weights
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect();
        let workflow = recipe
            .apply(Fill {
                prompt: "nsfw",
                seed: 1,
                weights: owned,
                source: Some("img2img-source.png"),
                pose: None,
            })
            .unwrap();
        assert_eq!(
            workflow["4"]["inputs"]["lora_name"],
            "qwen-image-edit-plus-nsfw-lora.safetensors"
        );
        assert_eq!(workflow["3"]["inputs"]["steps"], 40);
        assert_eq!(workflow["3"]["inputs"]["cfg"], 4);
    }

    #[test]
    fn adapter_selection_requires_catalog_metadata() {
        let catalog = catalog();
        assert_eq!(
            catalog
                .adapter_recipe_for_base("Qwen/Qwen-Image-Edit-2511")
                .unwrap()
                .id,
            "qwen-image-edit-adapter"
        );
        assert!(
            !catalog
                .image_recipe_for("qwen-image-edit-plus-nsfw-lora.safetensors")
                .unwrap()
                .adapter,
            "a filename is not enough evidence to select an adapter graph"
        );
    }

    #[test]
    fn invalid_training_adapter_mappings_fail_catalog_load() {
        let base: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
        for mutation in [
            "missing_adapter",
            "missing_hf_base",
            "unknown",
            "mismatched",
            "prompt",
        ] {
            let mut file = base.clone();
            let recipes = file["recipes"].as_array_mut().unwrap();
            let flux = recipes
                .iter_mut()
                .find(|recipe| recipe["id"] == "flux-schnell")
                .unwrap();
            match mutation {
                "missing_adapter" => {
                    flux["training"].as_object_mut().unwrap().remove("adapter");
                }
                "missing_hf_base" => {
                    flux["training"].as_object_mut().unwrap().remove("hf_base");
                }
                "unknown" => flux["training"]["adapter"] = json!("missing-adapter"),
                "mismatched" => flux["training"]["adapter"] = json!("qwen-image-edit-adapter"),
                "prompt" => flux["prompt_mode"] = json!("edit_instruction"),
                _ => unreachable!(),
            }
            assert!(
                RecipeCatalog::from_json(&file.to_string(), None).is_err(),
                "{mutation} training mapping must fail closed"
            );
        }

        let mut file = base;
        let adapter = file["recipes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|recipe| recipe["id"] == "flux-schnell-adapter")
            .unwrap();
        adapter["slots"]["weights"]
            .as_object_mut()
            .unwrap()
            .remove("lora");
        assert!(
            RecipeCatalog::from_json(&file.to_string(), None).is_err(),
            "a mapped adapter without a LoRA slot must fail closed"
        );
    }
}
