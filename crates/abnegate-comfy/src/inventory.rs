//! Scan the ComfyUI models directory and join files to packaged recipes.

mod identity;
mod item;
mod video_identity;
mod weight_document;
mod weight_sidecar;

pub use identity::Identity;
pub use item::InventoryItem;
pub use video_identity::VideoIdentity;
pub(crate) use weight_document::WeightDocument;
pub use weight_sidecar::WeightSidecar;

use crate::config::Config;
use crate::recipe::MediaKind;
use crate::recipe::PromptMode;
use crate::recipe::Recipe;
use crate::recipe::RecipeCatalog;
use crate::recipe::RequiredFile;
use crate::recipe::sanitize_weight_filename;
use crate::train::Contract;
use crate::train::is_single_component;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::SystemTime;

/// Extension of every weight the inventory lists.
pub(crate) const WEIGHT_EXTENSION: &str = ".safetensors";
const PUBLICATION_SUFFIX: &str = ".pending";
const CHECKPOINT: &str = "checkpoint";
const DIFFUSION_MODEL: &str = "diffusion_model";
const LORA: &str = "lora";

const SCAN_DIRECTORIES: &[(&str, &str)] = &[
    ("checkpoints", CHECKPOINT),
    ("diffusion_models", DIFFUSION_MODEL),
    ("loras", LORA),
];

/// Every checkpoint, diffusion model and LoRA under `models_directory` that a
/// recipe in `catalog` can run, sorted by filename. A LoRA is listed only when
/// its sidecar, named with `contract`'s suffix, binds it to an adapter recipe
/// for its base. No weight is listed while a publication in its directory's
/// `contract` publication directory is replacing it.
///
/// `contract` is expected to pass [`Contract::validate`], as it does in any
/// [`Config`](crate::Config) that passes [`Config::validate`](crate::Config::validate):
/// an unchecked suffix or publication directory can point the scan's reads
/// outside `models_directory`.
pub fn scan(
    models_directory: &Path,
    catalog: &RecipeCatalog,
    contract: &Contract,
) -> Vec<InventoryItem> {
    let mut items = Vec::new();
    for (directory, kind) in SCAN_DIRECTORIES {
        let folder = models_directory.join(directory);
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
                continue;
            }
            let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if filename.ends_with(&contract.sidecar_suffix) || !filename.ends_with(WEIGHT_EXTENSION)
            {
                continue;
            }
            if sanitize_weight_filename(filename).is_err() {
                continue;
            }
            if publication_pending(&folder, filename, contract) {
                continue;
            }
            let metadata = fs::metadata(&path).ok();
            let size = metadata
                .as_ref()
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            let modified_at = metadata
                .and_then(|metadata| metadata.modified().ok())
                .and_then(rfc3339);
            let sidecar = read_sidecar(&path, contract);
            let trigger = sidecar.as_ref().and_then(document_trigger);
            let recipe = resolve_recipe(catalog, filename, kind, sidecar.as_ref());
            let Some(recipe) = recipe else {
                continue;
            };
            let missing = missing_required(models_directory, &recipe.required_files);
            let mut required: Vec<String> = recipe
                .required_files
                .iter()
                .map(|file| file.filename.clone())
                .collect();
            if *kind == LORA {
                required.insert(0, filename.to_string());
            }
            items.push(InventoryItem {
                filename: filename.to_string(),
                recipe_id: recipe.id.clone(),
                kind: kind.to_string(),
                label: inventory_label(recipe, filename),
                directory: directory.to_string(),
                size,
                modified_at,
                ready: missing.is_empty(),
                required_files: if missing.is_empty() {
                    required
                } else {
                    missing
                },
                adapter: recipe.adapter,
                prompt_mode: match recipe.prompt_mode {
                    PromptMode::ClipScene => "clip_scene".to_string(),
                    PromptMode::EditInstruction => "edit_instruction".to_string(),
                },
                trigger,
            });
        }
    }
    items.sort_by(|left, right| left.filename.cmp(&right.filename));
    items
}

/// The item [`scan`] listed under `filename`.
pub fn find<'a>(items: &'a [InventoryItem], filename: &str) -> Option<&'a InventoryItem> {
    items.iter().find(|item| item.filename == filename)
}

/// Every ready [`Identity`] under `config`'s models directory, in filename
/// order, read with the recipe catalog `config` names. A catalog that cannot
/// be loaded has none.
///
/// It reads the models directory and every sidecar in it, so it blocks: async
/// code calls it inside `tokio::task::spawn_blocking`.
pub fn identities(config: &Config) -> Vec<Identity> {
    let Ok(catalog) = RecipeCatalog::load(config.workflow_path.as_deref()) else {
        return Vec::new();
    };
    scan(&config.models_directory, &catalog, &config.contract)
        .iter()
        .filter(|item| is_image_identity(&catalog, item))
        .filter_map(identity)
        .collect()
}

/// Selects the identity `haystack` names as `config`'s checkpoint, so the
/// next image generated with `config` loads it, and returns it. Nothing
/// changes when `haystack` names no ready identity, or names none under the
/// rules of [`identity_among`].
///
/// It reads the models directory and every sidecar in it, so it blocks: async
/// code calls it inside `tokio::task::spawn_blocking`, or scans once and asks
/// [`identity_among`].
pub fn bind_identity(config: &mut Config, haystack: &str) -> Option<Identity> {
    let catalog = RecipeCatalog::load(config.workflow_path.as_deref()).ok()?;
    let identity = identity_for_prompt(
        &config.models_directory,
        &catalog,
        &config.contract,
        haystack,
    )?;
    config.checkpoint.clone_from(&identity.filename);
    Some(identity)
}

/// The one ready image [`Identity`] under `models_directory` whose trigger
/// `haystack` names, as [`scan`] reads it, chosen as [`identity_among`]
/// chooses.
///
/// It reads the models directory and every sidecar in it, so it blocks: async
/// code calls it inside `tokio::task::spawn_blocking`, or scans once and asks
/// [`identity_among`].
pub fn identity_for_prompt(
    models_directory: &Path,
    catalog: &RecipeCatalog,
    contract: &Contract,
    haystack: &str,
) -> Option<Identity> {
    identity_among(
        &scan(models_directory, catalog, contract),
        catalog,
        haystack,
    )
}

/// The one ready image [`Identity`] among `items`, as [`scan`] listed them
/// with `catalog`, whose trigger `haystack` names, without touching the disk.
///
/// An image identity is a ready image LoRA, fine-tuned checkpoint or
/// fine-tuned diffusion model with a trigger, whose recipe takes a scene
/// prompt: an edit weight follows an instruction, so a trigger cannot select
/// it. A trigger is named as a whole phrase, in any case, with any run of
/// whitespace standing for one space. When `haystack` names triggers of
/// different lengths the longest wins; when the longest is shared by two
/// weights, or two different triggers share that length, it names none.
pub fn identity_among(
    items: &[InventoryItem],
    catalog: &RecipeCatalog,
    haystack: &str,
) -> Option<Identity> {
    let named = longest_triggers(
        items
            .iter()
            .filter(|item| is_image_identity(catalog, item) && names_trigger(item, haystack))
            .collect(),
    );
    match named.as_slice() {
        [item] => identity(item),
        _ => None,
    }
}

/// The identity a video prompt in `haystack` names, read with the recipe
/// catalog `config` names. See [`video_identity_among`].
///
/// It reads the models directory and every sidecar in it, so it blocks: async
/// code calls it inside `tokio::task::spawn_blocking`, or scans once and asks
/// [`video_identity_among`].
pub fn bind_video_identity(config: &Config, haystack: &str) -> Option<VideoIdentity> {
    let catalog = RecipeCatalog::load(config.workflow_path.as_deref()).ok()?;
    video_identity_for_prompt(
        &config.models_directory,
        &catalog,
        &config.contract,
        haystack,
    )
}

/// The identity a video prompt in `haystack` names among the weights under
/// `models_directory`, as [`scan`] reads them, chosen as
/// [`video_identity_among`] chooses.
///
/// It reads the models directory and every sidecar in it, so it blocks: async
/// code calls it inside `tokio::task::spawn_blocking`, or scans once and asks
/// [`video_identity_among`].
pub fn video_identity_for_prompt(
    models_directory: &Path,
    catalog: &RecipeCatalog,
    contract: &Contract,
    haystack: &str,
) -> Option<VideoIdentity> {
    video_identity_among(
        &scan(models_directory, catalog, contract),
        catalog,
        haystack,
    )
}

/// The trigger a video prompt in `haystack` names among the ready video LoRAs
/// and image identities in `items`, as [`scan`] listed them with `catalog`,
/// with the video LoRA trained on it when exactly one is ready, without
/// touching the disk.
///
/// Triggers are named and compared as [`identity_among`] names them, so
/// weights whose triggers differ only in case or spacing share one trigger.
/// When `haystack` names triggers of different lengths the longest wins; two
/// different triggers of that length name none.
pub fn video_identity_among(
    items: &[InventoryItem],
    catalog: &RecipeCatalog,
    haystack: &str,
) -> Option<VideoIdentity> {
    let named = longest_triggers(
        items
            .iter()
            .filter(|item| {
                (is_video_adapter(catalog, item) || is_image_identity(catalog, item))
                    && names_trigger(item, haystack)
            })
            .collect(),
    );
    let mut keys: Vec<String> = named
        .iter()
        .filter_map(|item| item.trigger.as_deref())
        .map(phrase_key)
        .collect();
    keys.sort_unstable();
    keys.dedup();
    if keys.len() != 1 {
        return None;
    }
    let loras: Vec<&InventoryItem> = named
        .iter()
        .copied()
        .filter(|item| is_video_adapter(catalog, item))
        .collect();
    let (lora, spoken) = match loras.as_slice() {
        [lora] => (Some(lora.filename.clone()), *lora),
        _ => (None, *named.first()?),
    };
    Some(VideoIdentity {
        trigger: spoken.trigger.clone()?,
        lora,
    })
}

fn media(catalog: &RecipeCatalog, item: &InventoryItem) -> Option<MediaKind> {
    catalog.get(&item.recipe_id).map(|recipe| recipe.kind)
}

fn is_image_identity(catalog: &RecipeCatalog, item: &InventoryItem) -> bool {
    item.ready
        && item.trigger.is_some()
        && (item.adapter || item.kind == CHECKPOINT || item.kind == DIFFUSION_MODEL)
        && catalog.get(&item.recipe_id).is_some_and(|recipe| {
            recipe.kind == MediaKind::Image && recipe.prompt_mode == PromptMode::ClipScene
        })
}

fn is_video_adapter(catalog: &RecipeCatalog, item: &InventoryItem) -> bool {
    item.ready
        && item.trigger.is_some()
        && item.adapter
        && item.kind == LORA
        && media(catalog, item) == Some(MediaKind::Video)
}

fn names_trigger(item: &InventoryItem, haystack: &str) -> bool {
    item.trigger
        .as_deref()
        .is_some_and(|trigger| contains_phrase(haystack, trigger))
}

fn longest_triggers(mut items: Vec<&InventoryItem>) -> Vec<&InventoryItem> {
    let length = |item: &InventoryItem| {
        item.trigger
            .as_deref()
            .map_or(0, |trigger| phrase_key(trigger).chars().count())
    };
    let longest = items.iter().map(|item| length(item)).max().unwrap_or(0);
    items.retain(|item| length(item) == longest);
    items
}

fn identity(item: &InventoryItem) -> Option<Identity> {
    Some(Identity {
        filename: item.filename.clone(),
        trigger: item.trigger.clone()?,
    })
}

fn document_trigger(document: &WeightDocument) -> Option<String> {
    document
        .sidecar
        .trigger
        .as_deref()
        .map(str::trim)
        .filter(|trigger| !trigger.is_empty())
        .map(str::to_string)
}

/// Whether `text` names `phrase` as a whole phrase, ignoring Unicode case and
/// reading any run of whitespace as one space: a letter, digit or underscore
/// on either side makes it part of another word.
pub(crate) fn contains_phrase(text: &str, phrase: &str) -> bool {
    let text = phrase_key(text);
    let phrase = phrase_key(phrase);
    if phrase.is_empty() {
        return false;
    }
    text.match_indices(&phrase).any(|(start, matched)| {
        let before = text[..start].chars().next_back();
        let after = text[start + matched.len()..].chars().next();
        !before.is_some_and(is_trigger_character) && !after.is_some_and(is_trigger_character)
    })
}

fn is_trigger_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// `text` as triggers are compared: Unicode lowercase, its words joined by
/// single spaces.
fn phrase_key(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Binds the weight at `path` to a recipe, in the sidecar beside it named with
/// `contract`'s suffix.
///
/// A contract that fails [`Contract::validate`] is refused with
/// [`InvalidInput`](std::io::ErrorKind::InvalidInput) before anything is
/// written, since its suffix could name a file outside the weight's directory.
pub fn write_sidecar(
    path: &Path,
    sidecar: &WeightSidecar,
    contract: &Contract,
) -> std::io::Result<()> {
    contract
        .validate()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let encoded = serde_json::to_vec_pretty(sidecar)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(sidecar_path(path, contract), encoded)
}

/// Where a weight of `kind` sits under the models directory, or `None` for an
/// unknown kind or a `filename` that is not exactly one entry of its directory.
pub fn relative_weight_path(kind: &str, filename: &str) -> Option<PathBuf> {
    let directory = match kind {
        "checkpoint" => "checkpoints",
        "diffusion_model" => "diffusion_models",
        "lora" => "loras",
        "vae" => "vae",
        "text_encoder" => "text_encoders",
        _ => return None,
    };
    weight_filename(filename).then(|| PathBuf::from(directory).join(filename))
}

/// Whether `filename` is exactly one entry of a weight directory and passes
/// the weight-filename rules every other weight path is held to.
fn weight_filename(filename: &str) -> bool {
    is_single_component(filename) && sanitize_weight_filename(filename).is_ok()
}

fn resolve_recipe<'a>(
    catalog: &'a RecipeCatalog,
    filename: &str,
    kind: &str,
    document: Option<&WeightDocument>,
) -> Option<&'a Recipe> {
    if kind == LORA {
        let document = document?;
        if document
            .generation
            .as_deref()
            .is_some_and(|generation| uuid::Uuid::parse_str(generation).is_err())
        {
            return None;
        }
        let sidecar = &document.sidecar;
        let recipe = catalog.get(&sidecar.recipe_id)?;
        let huggingface_base = sidecar.huggingface_base.as_deref()?;
        return (matches!(recipe.kind, MediaKind::Image | MediaKind::Video)
            && recipe.adapter
            && recipe.has_lora_slot()
            && recipe
                .huggingface_bases
                .iter()
                .any(|base| base.eq_ignore_ascii_case(huggingface_base)))
        .then_some(recipe);
    }
    if let Some(document) = document
        && let Some(recipe) = catalog.get(&document.sidecar.recipe_id)
    {
        return Some(recipe);
    }
    catalog.image_recipe_for(filename).ok()
}

pub(crate) fn publication_marker(
    directory: &Path,
    filename: &str,
    contract: &Contract,
) -> Option<PathBuf> {
    let filename = sanitize_weight_filename(filename).ok()?;
    Some(
        directory
            .join(&contract.publication_directory)
            .join(format!("{filename}{PUBLICATION_SUFFIX}")),
    )
}

/// Whether a publication is replacing `filename` in any directory [`scan`]
/// reads under `models_directory`.
pub(crate) fn publishing(models_directory: &Path, filename: &str, contract: &Contract) -> bool {
    SCAN_DIRECTORIES.iter().any(|(directory, _)| {
        publication_pending(&models_directory.join(directory), filename, contract)
    })
}

/// Whether a weight named `filename`, in any ASCII case, sits in or is being
/// published to a directory [`scan`] reads under `models_directory` other
/// than `directory`.
pub(crate) fn named_elsewhere(
    models_directory: &Path,
    directory: &str,
    filename: &str,
    contract: &Contract,
) -> bool {
    SCAN_DIRECTORIES
        .iter()
        .filter(|(scanned, _)| *scanned != directory)
        .map(|(scanned, _)| models_directory.join(scanned))
        .any(|folder| {
            publication_pending(&folder, filename, contract)
                || fs::read_dir(&folder).is_ok_and(|entries| {
                    entries.flatten().any(|entry| {
                        entry
                            .file_name()
                            .to_str()
                            .is_some_and(|name| name.eq_ignore_ascii_case(filename))
                    })
                })
        })
}

fn publication_pending(directory: &Path, filename: &str, contract: &Contract) -> bool {
    publication_marker(directory, filename, contract)
        .is_some_and(|path| fs::symlink_metadata(path).is_ok())
}

pub(crate) fn missing_required(models_directory: &Path, required: &[RequiredFile]) -> Vec<String> {
    required
        .iter()
        .filter(|file| !required_path(models_directory, file).is_some_and(|path| path.is_file()))
        .map(|file| file.filename.clone())
        .collect()
}

fn required_path(models_directory: &Path, file: &RequiredFile) -> Option<PathBuf> {
    (is_single_component(&file.directory) && weight_filename(&file.filename))
        .then(|| models_directory.join(&file.directory).join(&file.filename))
}

fn inventory_label(recipe: &Recipe, filename: &str) -> String {
    if recipe.adapter {
        format!("{} · {filename}", recipe.label)
    } else {
        recipe.label.clone()
    }
}

pub(crate) fn sidecar_path(weight: &Path, contract: &Contract) -> PathBuf {
    let mut name = weight
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("weight")
        .to_string();
    name.push_str(&contract.sidecar_suffix);
    weight.with_file_name(name)
}

fn read_sidecar(weight: &Path, contract: &Contract) -> Option<WeightDocument> {
    let sidecar = sidecar_path(weight, contract);
    let metadata = fs::symlink_metadata(&sidecar).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return None;
    }
    let contents = fs::read_to_string(sidecar).ok()?;
    serde_json::from_str(&contents).ok()
}

fn rfc3339(time: SystemTime) -> Option<String> {
    let datetime = chrono::DateTime::<chrono::Utc>::from(time);
    Some(datetime.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::RecipeCatalog;
    use std::fs;

    fn temp_models() -> PathBuf {
        let root = std::env::temp_dir().join(format!("comfy-inventory-{}", uuid::Uuid::new_v4()));
        for directory in [
            "checkpoints",
            "diffusion_models",
            "loras",
            "text_encoders",
            "vae",
        ] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        root
    }

    fn write_qwen_sidecar(weight: &Path) {
        write_sidecar(
            weight,
            &WeightSidecar::new("qwen-image-edit-adapter")
                .with_huggingface_base("Qwen/Qwen-Image-Edit-2511"),
            &Contract::default(),
        )
        .unwrap();
    }

    #[test]
    fn adapter_is_not_ready_without_base() {
        let root = temp_models();
        let lora = root.join("loras/qwen-image-edit-plus-nsfw-lora.safetensors");
        fs::write(&lora, b"lora").unwrap();
        write_qwen_sidecar(&lora);
        let catalog = RecipeCatalog::packaged().unwrap();
        let items = scan(&root, &catalog, &Contract::default());
        let item = items
            .iter()
            .find(|item| item.filename == "qwen-image-edit-plus-nsfw-lora.safetensors")
            .unwrap();
        assert_eq!(item.recipe_id, "qwen-image-edit-adapter");
        assert!(!item.ready);
        assert!(
            item.required_files
                .iter()
                .any(|name| name.contains("qwen_image_edit"))
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn adapter_is_ready_when_required_bundle_exists() {
        let root = temp_models();
        let lora = root.join("loras/qwen-image-edit-plus-nsfw-lora.safetensors");
        fs::write(&lora, b"lora").unwrap();
        write_qwen_sidecar(&lora);
        fs::write(
            root.join("diffusion_models/qwen_image_edit_2511_fp8mixed.safetensors"),
            b"unet",
        )
        .unwrap();
        fs::write(
            root.join("text_encoders/qwen_2.5_vl_7b_fp8_scaled.safetensors"),
            b"clip",
        )
        .unwrap();
        fs::write(root.join("vae/qwen_image_vae.safetensors"), b"vae").unwrap();
        let catalog = RecipeCatalog::packaged().unwrap();
        let items = scan(&root, &catalog, &Contract::default());
        let item = items
            .iter()
            .find(|item| item.filename == "qwen-image-edit-plus-nsfw-lora.safetensors")
            .unwrap();
        assert!(item.ready);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn coherent_sidecar_selects_adapter_without_filename_inference() {
        let root = temp_models();
        let lora = root.join("loras/custom-style.safetensors");
        fs::write(&lora, b"lora").unwrap();
        write_sidecar(
            &lora,
            &WeightSidecar::new("qwen-image-edit-adapter")
                .with_huggingface_base("Qwen/Qwen-Image-Edit-2511"),
            &Contract::default(),
        )
        .unwrap();
        let catalog = RecipeCatalog::packaged().unwrap();
        let items = scan(&root, &catalog, &Contract::default());
        assert_eq!(items[0].recipe_id, "qwen-image-edit-adapter");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_or_mismatched_adapter_sidecars_are_not_listed() {
        let root = temp_models();
        let missing = root.join("loras/missing.safetensors");
        let unbound = root.join("loras/unbound.safetensors");
        let mismatched = root.join("loras/mismatched.safetensors");
        let unknown = root.join("loras/unknown.safetensors");
        let malformed = root.join("loras/malformed.safetensors");
        for weight in [&missing, &unbound, &mismatched, &unknown, &malformed] {
            fs::write(weight, b"lora").unwrap();
        }
        write_sidecar(
            &unbound,
            &WeightSidecar::new("flux-schnell-adapter"),
            &Contract::default(),
        )
        .unwrap();
        write_sidecar(
            &mismatched,
            &WeightSidecar::new("flux-schnell-adapter")
                .with_huggingface_base("Qwen/Qwen-Image-Edit-2511"),
            &Contract::default(),
        )
        .unwrap();
        write_sidecar(
            &unknown,
            &WeightSidecar::new("missing-adapter")
                .with_huggingface_base("Qwen/Qwen-Image-Edit-2511"),
            &Contract::default(),
        )
        .unwrap();
        fs::write(
            sidecar_path(&malformed, &Contract::default()),
            serde_json::to_vec(&WeightDocument {
                sidecar: WeightSidecar::new("flux-schnell-adapter")
                    .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
                generation: Some("not-a-generation".into()),
            })
            .unwrap(),
        )
        .unwrap();

        assert!(
            scan(
                &root,
                &RecipeCatalog::packaged().unwrap(),
                &Contract::default()
            )
            .is_empty()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_required_file_reached_through_a_traversal_is_reported_missing() {
        let root = temp_models();
        fs::write(root.join("vae/base.safetensors"), b"vae").unwrap();
        let named = RequiredFile {
            filename: "base.safetensors".into(),
            directory: "vae".into(),
        };
        assert!(
            missing_required(&root, std::slice::from_ref(&named)).is_empty(),
            "the file sits exactly where the catalog names it"
        );

        for (directory, filename) in [
            ("loras/../vae", "base.safetensors"),
            ("..", "base.safetensors"),
            (".", "base.safetensors"),
            ("", "base.safetensors"),
            ("vae", "../vae/base.safetensors"),
            ("vae", "./base.safetensors"),
            ("vae/", "base.safetensors"),
            ("vae", "base..safetensors"),
        ] {
            let traversed = RequiredFile {
                filename: filename.into(),
                directory: directory.into(),
            };
            assert_eq!(
                missing_required(&root, std::slice::from_ref(&traversed)),
                vec![filename.to_string()],
                "{directory:?}/{filename:?} is not one segment of the models directory"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_weight_path_through_a_traversal_is_refused() {
        assert_eq!(
            relative_weight_path("lora", "style.safetensors"),
            Some(PathBuf::from("loras/style.safetensors"))
        );
        let overlong = format!("{}.safetensors", "a".repeat(256));
        for filename in [
            "",
            ".",
            "..",
            "style..v2.safetensors",
            overlong.as_str(),
            "../style.safetensors",
            "../../etc/passwd",
            "nested/style.safetensors",
            "style.safetensors/",
            "./style.safetensors",
            "/style.safetensors",
            "..\\style.safetensors",
        ] {
            assert_eq!(
                relative_weight_path("lora", filename),
                None,
                "{filename:?} is not one entry of the weight directory"
            );
        }
    }

    #[test]
    fn pending_publication_is_not_listed() {
        let root = temp_models();
        let lora = root.join("loras/style.safetensors");
        fs::write(&lora, b"lora").unwrap();
        fs::write(
            sidecar_path(&lora, &Contract::default()),
            serde_json::to_vec(&WeightDocument {
                sidecar: WeightSidecar::new("flux-schnell-adapter")
                    .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
                generation: Some(uuid::Uuid::new_v4().to_string()),
            })
            .unwrap(),
        )
        .unwrap();
        let marker = publication_marker(
            &root.join("loras"),
            "style.safetensors",
            &Contract::default(),
        )
        .unwrap();
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(marker, b"pending").unwrap();

        assert!(
            scan(
                &root,
                &RecipeCatalog::packaged().unwrap(),
                &Contract::default()
            )
            .is_empty()
        );
        let _ = fs::remove_dir_all(root);
    }

    fn overridden() -> Contract {
        Contract {
            sidecar_suffix: ".binding.json".into(),
            publication_directory: ".binding-publish".into(),
            ..Contract::default()
        }
    }

    #[test]
    fn a_sidecar_is_never_written_under_a_contract_that_fails_validation() {
        let root = temp_models();
        let lora = root.join("loras/style.safetensors");
        fs::write(&lora, b"lora").unwrap();
        let escaping = Contract {
            sidecar_suffix: "/../x".into(),
            ..Contract::default()
        };
        let sidecar = WeightSidecar::new("flux-schnell-adapter");

        for weight in [lora, root.join("loras")] {
            let error = write_sidecar(&weight, &sidecar, &escaping).unwrap_err();
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::InvalidInput,
                "{}: {error}",
                weight.display()
            );
        }

        assert!(!root.join("x").exists(), "a sidecar landed outside loras/");
        assert!(!root.join("loras/x").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_sidecar_is_read_only_under_the_suffix_the_contract_names() {
        let root = temp_models();
        let lora = root.join("loras/style.safetensors");
        fs::write(&lora, b"lora").unwrap();
        write_sidecar(
            &lora,
            &WeightSidecar::new("flux-schnell-adapter")
                .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
            &overridden(),
        )
        .unwrap();
        let catalog = RecipeCatalog::packaged().unwrap();

        assert!(root.join("loras/style.safetensors.binding.json").is_file());
        assert_eq!(scan(&root, &catalog, &overridden()).len(), 1);
        assert!(
            scan(&root, &catalog, &Contract::default()).is_empty(),
            "a binding under another suffix is no binding at all"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_publication_hides_a_weight_only_from_the_contract_that_names_its_directory() {
        let root = temp_models();
        let lora = root.join("loras/style.safetensors");
        fs::write(&lora, b"lora").unwrap();
        for contract in [overridden(), Contract::default()] {
            write_sidecar(
                &lora,
                &WeightSidecar::new("flux-schnell-adapter")
                    .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
                &contract,
            )
            .unwrap();
        }
        let marker =
            publication_marker(&root.join("loras"), "style.safetensors", &overridden()).unwrap();
        assert!(marker.starts_with(root.join("loras/.binding-publish")));
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(marker, b"pending").unwrap();
        let catalog = RecipeCatalog::packaged().unwrap();

        assert!(scan(&root, &catalog, &overridden()).is_empty());
        assert_eq!(scan(&root, &catalog, &Contract::default()).len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_torn_publication_hides_a_checkpoint_as_it_hides_a_lora() {
        let root = temp_models();
        let checkpoint = root.join("checkpoints/jerry.safetensors");
        fs::write(&checkpoint, b"tuned").unwrap();
        write_sidecar(
            &checkpoint,
            &WeightSidecar::new("flux-schnell"),
            &Contract::default(),
        )
        .unwrap();
        let catalog = RecipeCatalog::packaged().unwrap();
        assert!(
            find(
                &scan(&root, &catalog, &Contract::default()),
                "jerry.safetensors"
            )
            .is_some()
        );

        let marker = publication_marker(
            &root.join("checkpoints"),
            "jerry.safetensors",
            &Contract::default(),
        )
        .unwrap();
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(marker, b"pending").unwrap();

        assert!(
            find(
                &scan(&root, &catalog, &Contract::default()),
                "jerry.safetensors"
            )
            .is_none(),
            "a fine-tune mid-publication is listed as ready"
        );
        let _ = fs::remove_dir_all(root);
    }

    const FLUX_SCHNELL_BASE: &str = "black-forest-labs/FLUX.1-schnell";
    const WAN_BASE: &str = "Comfy-Org/Wan_2.2_ComfyUI_Repackaged";

    fn write_identity(root: &Path, filename: &str, trigger: &str) {
        let lora = root.join("loras").join(filename);
        fs::write(&lora, b"lora").unwrap();
        write_sidecar(
            &lora,
            &WeightSidecar::new("flux-schnell-adapter")
                .with_huggingface_base(FLUX_SCHNELL_BASE)
                .with_trigger(trigger),
            &Contract::default(),
        )
        .unwrap();
        let checkpoint = root.join("checkpoints/flux1-schnell-fp8.safetensors");
        if !checkpoint.is_file() {
            fs::write(checkpoint, b"checkpoint").unwrap();
        }
    }

    fn write_video_identity(root: &Path, filename: &str, trigger: &str) {
        let lora = root.join("loras").join(filename);
        fs::write(&lora, b"lora").unwrap();
        write_sidecar(
            &lora,
            &WeightSidecar::new("wan-adapter")
                .with_huggingface_base(WAN_BASE)
                .with_trigger(trigger),
            &Contract::default(),
        )
        .unwrap();
        for (directory, filename) in [
            ("diffusion_models", "wan2.2_ti2v_5B_fp16.safetensors"),
            ("text_encoders", "umt5_xxl_fp8_e4m3fn_scaled.safetensors"),
            ("vae", "wan2.2_vae.safetensors"),
        ] {
            fs::write(root.join(directory).join(filename), b"wan").unwrap();
        }
    }

    #[test]
    fn a_video_adapter_outside_loras_is_never_loaded_as_one() {
        let root = temp_models();
        write_video_identity(&root, "jerry-wan.safetensors", "ohwx");
        fs::rename(
            root.join("loras/jerry-wan.safetensors"),
            root.join("checkpoints/jerry-wan.safetensors"),
        )
        .unwrap();
        fs::rename(
            sidecar_path(
                &root.join("loras/jerry-wan.safetensors"),
                &Contract::default(),
            ),
            sidecar_path(
                &root.join("checkpoints/jerry-wan.safetensors"),
                &Contract::default(),
            ),
        )
        .unwrap();

        let lora = video_identity_named(&root, "ohwx dancing").and_then(|identity| identity.lora);

        assert_eq!(lora, None, "a checkpoint was bound as a video LoRA");
        let _ = fs::remove_dir_all(root);
    }

    fn identity_named(root: &Path, haystack: &str) -> Option<Identity> {
        identity_for_prompt(
            root,
            &RecipeCatalog::packaged().unwrap(),
            &Contract::default(),
            haystack,
        )
    }

    fn video_identity_named(root: &Path, haystack: &str) -> Option<VideoIdentity> {
        video_identity_for_prompt(
            root,
            &RecipeCatalog::packaged().unwrap(),
            &Contract::default(),
            haystack,
        )
    }

    fn configured(root: &Path) -> Config {
        Config {
            models_directory: root.to_path_buf(),
            checkpoint: "flux1-schnell-fp8.safetensors".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_sidecar_trigger_is_optional_on_the_wire() {
        let sidecar: WeightSidecar = serde_json::from_str(
            r#"{"recipe_id":"flux-schnell-adapter","hf_base":"black-forest-labs/FLUX.1-schnell"}"#,
        )
        .unwrap();
        assert_eq!(sidecar.trigger, None);
        assert_eq!(
            serde_json::to_value(&sidecar).unwrap(),
            serde_json::json!({
                "recipe_id": "flux-schnell-adapter",
                "hf_base": FLUX_SCHNELL_BASE,
            }),
            "a sidecar without a trigger is written as before"
        );
        assert_eq!(
            serde_json::to_value(sidecar.with_trigger("ohwx")).unwrap(),
            serde_json::json!({
                "recipe_id": "flux-schnell-adapter",
                "hf_base": FLUX_SCHNELL_BASE,
                "trigger": "ohwx",
            })
        );
    }

    #[test]
    fn scan_exposes_the_trigger_written_on_a_ready_adapter() {
        let root = temp_models();
        write_identity(&root, "jake.safetensors", "  ohwx ");
        write_identity(&root, "blank.safetensors", "   ");
        let items = scan(
            &root,
            &RecipeCatalog::packaged().unwrap(),
            &Contract::default(),
        );
        let jake = find(&items, "jake.safetensors").unwrap();
        assert_eq!(jake.trigger.as_deref(), Some("ohwx"));
        assert!(jake.ready);
        assert_eq!(
            find(&items, "blank.safetensors").unwrap().trigger,
            None,
            "a blank trigger is no trigger"
        );
        assert_eq!(
            serde_json::to_value(find(&items, "blank.safetensors").unwrap())
                .unwrap()
                .get("trigger"),
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn identity_matching_picks_the_named_ready_adapter() {
        let root = temp_models();
        write_identity(&root, "jake.safetensors", "ohwx");
        write_identity(&root, "teapot.safetensors", "zrkxyz");

        let matched = identity_named(&root, "draw OHWX at the beach").unwrap();
        assert_eq!(matched.filename, "jake.safetensors");
        assert_eq!(matched.trigger, "ohwx");

        assert!(
            identity_named(&root, "a lighthouse at dusk").is_none(),
            "a prompt that names no identity keeps the pin"
        );
        assert!(
            identity_named(&root, "an ohwxian portrait").is_none(),
            "a trigger must not match inside another word"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_longest_trigger_wins_and_a_tie_stays_on_the_pin() {
        let root = temp_models();
        write_identity(&root, "cat.safetensors", "cat");
        write_identity(&root, "blue-cat.safetensors", "blue cat");
        write_identity(&root, "other.safetensors", "ohwx");

        let matched = identity_named(&root, "a blue cat on a sofa").unwrap();
        assert_eq!(matched.filename, "blue-cat.safetensors");

        write_identity(&root, "twin.safetensors", "ohwx");
        assert!(
            identity_named(&root, "ohwx waving").is_none(),
            "two identities that share a trigger are ambiguous"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn bind_identity_switches_the_checkpoint_only_when_named() {
        let root = temp_models();
        write_identity(&root, "jake.safetensors", "ohwx");
        let mut config = configured(&root);

        let identity = bind_identity(&mut config, "portrait of ohwx").unwrap();
        assert_eq!(identity.filename, "jake.safetensors");
        assert_eq!(config.checkpoint, "jake.safetensors");

        config.checkpoint = "flux1-schnell-fp8.safetensors".into();
        assert!(bind_identity(&mut config, "a lighthouse").is_none());
        assert_eq!(config.checkpoint, "flux1-schnell-fp8.safetensors");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_unloadable_catalog_binds_nothing() {
        let root = temp_models();
        write_identity(&root, "jake.safetensors", "ohwx");
        let overlay = tempfile::tempdir().unwrap();
        fs::create_dir_all(overlay.path().join("recipes")).unwrap();
        fs::write(overlay.path().join("recipes/catalog.json"), b"{").unwrap();
        let mut config = configured(&root);
        config.workflow_path = Some(overlay.path().join("workflows/custom-api.json"));

        assert!(identities(&config).is_empty());
        assert!(bind_identity(&mut config, "portrait of ohwx").is_none());
        assert!(bind_video_identity(&config, "portrait of ohwx").is_none());
        assert_eq!(config.checkpoint, "flux1-schnell-fp8.safetensors");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_adapter_without_its_base_is_not_an_identity() {
        let root = temp_models();
        write_identity(&root, "jake.safetensors", "ohwx");
        fs::remove_file(root.join("checkpoints/flux1-schnell-fp8.safetensors")).unwrap();
        assert!(identity_named(&root, "ohwx").is_none());
        assert!(identities(&configured(&root)).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_publication_hides_an_identity() {
        let root = temp_models();
        write_identity(&root, "jake.safetensors", "ohwx");
        let marker = publication_marker(
            &root.join("loras"),
            "jake.safetensors",
            &Contract::default(),
        )
        .unwrap();
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(marker, b"pending").unwrap();

        assert!(identities(&configured(&root)).is_empty());
        assert!(identity_named(&root, "ohwx").is_none());
        assert!(video_identity_named(&root, "ohwx").is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_ready_checkpoint_with_a_trigger_is_an_identity() {
        let root = temp_models();
        for (filename, sidecar) in [
            (
                "jerry.safetensors",
                WeightSidecar::new("sdxl").with_trigger("ohwx"),
            ),
            ("plain.safetensors", WeightSidecar::new("sdxl")),
        ] {
            let checkpoint = root.join("checkpoints").join(filename);
            fs::write(&checkpoint, b"tuned").unwrap();
            write_sidecar(&checkpoint, &sidecar, &Contract::default()).unwrap();
        }

        let matched = identity_named(&root, "portrait of ohwx").unwrap();
        assert_eq!(matched.filename, "jerry.safetensors");
        assert_eq!(matched.trigger, "ohwx");
        assert_eq!(
            identities(&configured(&root)),
            vec![Identity {
                filename: "jerry.safetensors".into(),
                trigger: "ohwx".into(),
            }],
            "a checkpoint without a trigger is not an identity"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_video_adapter_is_inventoried_but_not_an_image_identity() {
        let root = temp_models();
        write_video_identity(&root, "jerry-wan.safetensors", "ohwx");
        let items = scan(
            &root,
            &RecipeCatalog::packaged().unwrap(),
            &Contract::default(),
        );
        let item = find(&items, "jerry-wan.safetensors").unwrap();
        assert_eq!(item.recipe_id, "wan-adapter");
        assert!(item.ready);
        assert!(item.adapter);

        assert!(identities(&configured(&root)).is_empty());
        assert!(identity_named(&root, "portrait of ohwx").is_none());
        let mut config = configured(&root);
        assert!(bind_identity(&mut config, "portrait of ohwx").is_none());
        assert_eq!(config.checkpoint, "flux1-schnell-fp8.safetensors");

        let bound = video_identity_named(&root, "portrait of ohwx").unwrap();
        assert_eq!(bound.trigger, "ohwx");
        assert_eq!(bound.lora.as_deref(), Some("jerry-wan.safetensors"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn video_bind_prefixes_a_still_identity_without_a_video_lora() {
        let root = temp_models();
        write_identity(&root, "jerry.safetensors", "ohwx");
        let bound = bind_video_identity(&configured(&root), "ohwx walking on the beach").unwrap();
        assert_eq!(
            bound,
            VideoIdentity {
                trigger: "ohwx".into(),
                lora: None,
            }
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn video_bind_prefers_the_video_lora_when_both_exist() {
        let root = temp_models();
        write_identity(&root, "jerry.safetensors", "ohwx");
        write_video_identity(&root, "jerry-wan.safetensors", "ohwx");

        let bound = video_identity_named(&root, "ohwx at dusk").unwrap();
        assert_eq!(bound.trigger, "ohwx");
        assert_eq!(bound.lora.as_deref(), Some("jerry-wan.safetensors"));
        assert!(
            identities(&configured(&root))
                .iter()
                .any(|identity| identity.filename == "jerry.safetensors")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn video_bind_refuses_two_longest_triggers_and_keeps_a_shared_trigger_without_its_lora() {
        let root = temp_models();
        write_identity(&root, "jerry.safetensors", "ohwx");
        write_video_identity(&root, "teapot-wan.safetensors", "zrkx");
        assert!(
            video_identity_named(&root, "ohwx holding zrkx").is_none(),
            "two different triggers of the same length are ambiguous"
        );

        write_video_identity(&root, "jerry-wan.safetensors", "ohwx");
        write_video_identity(&root, "twin-wan.safetensors", "ohwx");
        let bound = video_identity_named(&root, "ohwx at dusk").unwrap();
        assert_eq!(bound.trigger, "ohwx");
        assert_eq!(
            bound.lora, None,
            "two video LoRAs on one trigger leave the choice to the trigger alone"
        );
        let _ = fs::remove_dir_all(root);
    }

    fn write_edit_identity(root: &Path, filename: &str, trigger: &str) {
        let lora = root.join("loras").join(filename);
        fs::write(&lora, b"lora").unwrap();
        write_sidecar(
            &lora,
            &WeightSidecar::new("qwen-image-edit-adapter")
                .with_huggingface_base("Qwen/Qwen-Image-Edit-2511")
                .with_trigger(trigger),
            &Contract::default(),
        )
        .unwrap();
        write_qwen_files(root);
    }

    fn write_qwen_files(root: &Path) {
        for (directory, filename) in [
            (
                "diffusion_models",
                "qwen_image_edit_2511_fp8mixed.safetensors",
            ),
            ("text_encoders", "qwen_2.5_vl_7b_fp8_scaled.safetensors"),
            ("vae", "qwen_image_vae.safetensors"),
        ] {
            fs::write(root.join(directory).join(filename), b"qwen").unwrap();
        }
    }

    /// A workflow path whose catalog adds `qwen-image-scene`: the packaged
    /// Qwen UNet graph run from a scene prompt rather than an instruction.
    fn scene_unet_workflow(overlay: &Path) -> PathBuf {
        let mut catalog: serde_json::Value =
            serde_json::from_str(include_str!("../comfyui/recipes/catalog.json")).unwrap();
        let recipes = catalog["recipes"].as_array_mut().unwrap();
        let mut scene = recipes
            .iter()
            .find(|recipe| recipe["id"] == "qwen-image-edit")
            .unwrap()
            .clone();
        let fields = scene.as_object_mut().unwrap();
        for field in ["training", "files", "filename_hints"] {
            fields.remove(field);
        }
        fields.insert("id".into(), "qwen-image-scene".into());
        fields.insert("prompt_mode".into(), "clip_scene".into());
        recipes.push(scene);
        fs::create_dir_all(overlay.join("recipes")).unwrap();
        fs::write(
            overlay.join("recipes/catalog.json"),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
        overlay.join("workflows/custom-api.json")
    }

    #[test]
    fn an_edit_adapter_with_a_trigger_is_never_an_identity() {
        let root = temp_models();
        write_edit_identity(&root, "jerry-edit.safetensors", "ohwx");
        let items = scan(
            &root,
            &RecipeCatalog::packaged().unwrap(),
            &Contract::default(),
        );
        let item = find(&items, "jerry-edit.safetensors").unwrap();
        assert!(item.ready);
        assert_eq!(item.trigger.as_deref(), Some("ohwx"));

        assert!(
            identities(&configured(&root)).is_empty(),
            "an edit weight follows an instruction, not a trigger"
        );
        assert!(identity_named(&root, "portrait of ohwx").is_none());
        assert!(video_identity_named(&root, "ohwx walking").is_none());
        let mut config = configured(&root);
        assert!(bind_identity(&mut config, "portrait of ohwx").is_none());
        assert_eq!(config.checkpoint, "flux1-schnell-fp8.safetensors");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_fine_tuned_diffusion_model_with_a_trigger_is_an_identity_unless_it_edits() {
        let root = temp_models();
        let overlay = tempfile::tempdir().unwrap();
        let mut config = configured(&root);
        config.workflow_path = Some(scene_unet_workflow(overlay.path()));
        write_qwen_files(&root);
        for (filename, recipe_id) in [
            ("jerry-unet.safetensors", "qwen-image-scene"),
            ("jerry-edit-unet.safetensors", "qwen-image-edit"),
        ] {
            let weight = root.join("diffusion_models").join(filename);
            fs::write(&weight, b"tuned").unwrap();
            write_sidecar(
                &weight,
                &WeightSidecar::new(recipe_id).with_trigger(if recipe_id == "qwen-image-scene" {
                    "ohwx"
                } else {
                    "zrkx"
                }),
                &Contract::default(),
            )
            .unwrap();
        }

        assert_eq!(
            identities(&config),
            vec![Identity {
                filename: "jerry-unet.safetensors".into(),
                trigger: "ohwx".into(),
            }]
        );
        let identity = bind_identity(&mut config, "portrait of ohwx").unwrap();
        assert_eq!(identity.filename, "jerry-unet.safetensors");
        assert_eq!(config.checkpoint, "jerry-unet.safetensors");
        assert!(
            bind_identity(&mut config, "portrait of zrkx").is_none(),
            "a fine-tuned edit model is not selected by its trigger"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn two_different_triggers_of_one_length_name_no_identity() {
        let root = temp_models();
        write_identity(&root, "jerry.safetensors", "ohwx");
        write_identity(&root, "teapot.safetensors", "zrkx");
        write_identity(&root, "blue-teapot.safetensors", "blue zrkx");

        assert!(
            identity_named(&root, "ohwx holding zrkx").is_none(),
            "two different triggers of the longest length are ambiguous"
        );
        assert_eq!(
            identity_named(&root, "ohwx holding a blue zrkx")
                .unwrap()
                .filename,
            "blue-teapot.safetensors",
            "a longer trigger wins over a shorter one"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn video_triggers_that_differ_only_in_case_or_spacing_are_one_trigger() {
        let root = temp_models();
        write_identity(&root, "jerry.safetensors", "Blue OHWX");
        write_identity(&root, "jerry-spaced.safetensors", "blue  ohwx");
        write_video_identity(&root, "jerry-wan.safetensors", "blue ohwx");

        let bound = video_identity_named(&root, "blue ohwx at dusk").unwrap();
        assert_eq!(
            bound.trigger, "blue ohwx",
            "the video LoRA's own spelling leads"
        );
        assert_eq!(bound.lora.as_deref(), Some("jerry-wan.safetensors"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_trigger_matches_across_any_run_of_whitespace() {
        assert!(contains_phrase("a blue   cat\non a sofa", "blue cat"));
        assert!(contains_phrase("a blue cat", "blue \t cat"));
        assert!(contains_phrase("blue\ncat", " Blue Cat "));
        assert!(!contains_phrase("a blue catalogue", "blue cat"));
        assert_eq!(
            crate::lora::identity_caption("a blue  cat on a sofa ", "blue cat"),
            "a blue  cat on a sofa",
            "a caption that already names the trigger is not led by it again"
        );

        let root = temp_models();
        write_identity(&root, "blue-cat.safetensors", "blue  cat");
        assert_eq!(
            identity_named(&root, "a Blue\tcat on a sofa")
                .unwrap()
                .filename,
            "blue-cat.safetensors"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn identity_among_reads_only_the_items_it_is_given() {
        let root = temp_models();
        write_identity(&root, "jerry.safetensors", "ohwx");
        write_video_identity(&root, "jerry-wan.safetensors", "ohwx");
        let catalog = RecipeCatalog::packaged().unwrap();
        let items = scan(&root, &catalog, &Contract::default());
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(
            identity_among(&items, &catalog, "portrait of ohwx"),
            Some(Identity {
                filename: "jerry.safetensors".into(),
                trigger: "ohwx".into(),
            })
        );
        assert_eq!(
            video_identity_among(&items, &catalog, "ohwx at dusk"),
            Some(VideoIdentity {
                trigger: "ohwx".into(),
                lora: Some("jerry-wan.safetensors".into()),
            })
        );
        assert!(identity_among(&items, &catalog, "a lighthouse").is_none());
    }
}
