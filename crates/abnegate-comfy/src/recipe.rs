//! Packaged ComfyUI recipes: a graph pair plus the few slots a caller may write.
//!
//! Chat never picks a workflow. The selected checkpoint filename resolves to a
//! recipe; an attached image selects that recipe's `with_source` graph.

mod fill;
mod media_kind;
mod output;
mod prompt_mode;
mod required_file;
mod training_adapter;
mod training_model;

pub use fill::Fill;
pub use media_kind::MediaKind;
pub use output::RecipeOutput;
pub use prompt_mode::PromptMode;
pub use required_file::RequiredFile;
pub use training_adapter::TrainingAdapter;
pub use training_model::TrainingModel;

use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

use crate::client::Error;

const PACKAGED_CATALOG: &str = include_str!("../comfyui/recipes/catalog.json");

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
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TrainingArchitecture {
    Flux,
    QwenEdit,
}

#[derive(Debug, Clone)]
pub struct Recipe {
    pub id: String,
    pub kind: MediaKind,
    pub label: String,
    pub adapter: bool,
    pub prompt_mode: PromptMode,
    pub defaults: HashMap<String, String>,
    pub huggingface_bases: Vec<String>,
    pub required_files: Vec<RequiredFile>,
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
    training: Option<CatalogTraining>,
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
    weights: HashMap<String, String>,
}

#[derive(Clone)]
pub struct RecipeCatalog {
    default_image: String,
    recipes: Vec<Recipe>,
    files: HashMap<String, String>,
    hints: Vec<(String, String)>,
}

impl RecipeCatalog {
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
                TrainingModel::Flux { .. } => PromptMode::ClipScene,
                TrainingModel::QwenEdit { .. } => PromptMode::EditInstruction,
            };
            if recipe.prompt_mode != prompt_mode || adapter_recipe.prompt_mode != prompt_mode {
                return Err(Error::Configuration(
                    "training architecture does not match its prompt mode",
                ));
            }
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

    pub fn get(&self, id: &str) -> Option<&Recipe> {
        self.recipes.iter().find(|recipe| recipe.id == id)
    }

    pub fn image_recipe_for(&self, checkpoint: &str) -> Result<&Recipe, Error> {
        let id = self.resolve_image_id(checkpoint);
        self.get(id)
            .filter(|recipe| recipe.kind == MediaKind::Image && !recipe.adapter)
            .ok_or(Error::Configuration(
                "no image recipe matches this checkpoint",
            ))
    }

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

    pub fn image_recipes(&self) -> impl Iterator<Item = &Recipe> {
        self.recipes
            .iter()
            .filter(|recipe| recipe.kind == MediaKind::Image)
    }

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
    pub fn training_model(&self) -> Result<TrainingModel, Error> {
        let architecture = self
            .training
            .as_ref()
            .ok_or(Error::Configuration(
                "recipe does not declare a supported training architecture",
            ))?
            .architecture;
        match architecture {
            TrainingArchitecture::Flux => Ok(TrainingModel::Flux {
                checkpoint: self.training_weight("checkpoint")?,
            }),
            TrainingArchitecture::QwenEdit => Ok(TrainingModel::QwenEdit {
                unet: self.training_weight("unet")?,
                clip: self.training_weight("clip")?,
                vae: self.training_weight("vae")?,
            }),
        }
    }

    pub fn training_adapter(&self) -> Result<&TrainingAdapter, Error> {
        self.training
            .as_ref()
            .map(|training| &training.adapter)
            .ok_or(Error::Configuration(
                "recipe does not declare a supported training adapter",
            ))
    }

    fn training_weight(&self, name: &str) -> Result<String, Error> {
        self.defaults
            .get(name)
            .ok_or(Error::Configuration(
                "training metadata references a missing model weight",
            ))
            .and_then(|filename| sanitize_weight_filename(filename))
    }

    pub fn has_lora_slot(&self) -> bool {
        self.slots.weights.contains_key("lora")
    }

    pub fn weight_map(&self, selected: &str) -> Result<HashMap<String, String>, Error> {
        let selected = sanitize_weight_filename(selected)?;
        let mut weights = self.defaults.clone();
        if self.has_lora_slot() {
            weights.insert("lora".to_string(), selected);
        } else if self.slots.weights.contains_key("checkpoint") {
            weights.insert("checkpoint".to_string(), selected);
        } else if self.slots.weights.contains_key("unet") {
            weights.insert("unet".to_string(), selected);
        }
        for name in self.slots.weights.keys() {
            if !weights.contains_key(name) {
                return Err(Error::Configuration("recipe weight is missing"));
            }
        }
        Ok(weights)
    }

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
        Ok(workflow)
    }
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
    }
    let output_class = format!("/{}/class_type", slots.output_node);
    match slots.output {
        RecipeOutput::PreviewImage => {
            if workflow.pointer(&output_class).and_then(Value::as_str) != Some("PreviewImage") {
                return Err(Error::Configuration(
                    "workflow output must use temporary PreviewImage storage",
                ));
            }
        }
    }
    Ok(())
}

fn source_class_pointer(source_slot: &str) -> String {
    match source_slot.rsplit_once("/inputs/") {
        Some((prefix, _)) => format!("{prefix}/class_type"),
        None => source_slot.to_string(),
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
        assert!(catalog.get("sd15").is_some());
        assert!(catalog.get("sdxl").is_some());
        assert!(catalog.get("qwen-image-edit").is_some());
        assert!(catalog.get("qwen-image-edit-adapter").is_some());
        for name in [
            "flux1-schnell-fp8-api.json",
            "flux1-schnell-fp8-img2img-api.json",
            "sd15-api.json",
            "sd15-img2img-api.json",
            "sdxl-api.json",
            "sdxl-img2img-api.json",
            "flux1-schnell-fp8-adapter-api.json",
            "flux1-schnell-fp8-adapter-img2img-api.json",
            "qwen-image-edit-2511-api.json",
            "qwen-image-edit-2511-edit-api.json",
            "qwen-image-edit-2511-adapter-api.json",
            "qwen-image-edit-2511-adapter-edit-api.json",
        ] {
            assert!(packaged_workflow(name).is_some(), "{name}");
        }
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
            })
            .unwrap();
        assert_eq!(workflow["3"]["inputs"]["steps"], 25);
        assert_eq!(workflow["5"]["inputs"]["width"], 1024);
        assert_eq!(
            workflow["4"]["inputs"]["ckpt_name"],
            "juggernautXL.safetensors"
        );
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
