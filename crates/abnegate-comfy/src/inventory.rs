//! Scan the ComfyUI models directory and join files to packaged recipes.

mod item;
mod weight_document;
mod weight_sidecar;

pub use item::InventoryItem;
pub(crate) use weight_document::WeightDocument;
pub use weight_sidecar::WeightSidecar;

use crate::recipe::{MediaKind, Recipe, RecipeCatalog, RequiredFile, sanitize_weight_filename};
use crate::train::Contract;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Extension of every weight the inventory lists.
pub(crate) const WEIGHT_EXTENSION: &str = ".safetensors";
const PUBLICATION_SUFFIX: &str = ".pending";

const SCAN_DIRECTORIES: &[(&str, &str)] = &[
    ("checkpoints", "checkpoint"),
    ("diffusion_models", "diffusion_model"),
    ("loras", "lora"),
];

/// Every checkpoint, diffusion model and LoRA under `models_directory` that a
/// recipe in `catalog` can run, sorted by filename. A LoRA is listed only when
/// its sidecar, named with `contract`'s suffix, binds it to an adapter recipe
/// for its base, and never while a publication in `contract`'s publication
/// directory is replacing it.
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
            if *kind == "lora" && publication_pending(models_directory, filename, contract) {
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
            let recipe = resolve_recipe(catalog, filename, kind, sidecar.as_ref());
            let Some(recipe) = recipe else {
                continue;
            };
            if *kind == "lora" && publication_pending(models_directory, filename, contract) {
                continue;
            }
            let missing = missing_required(models_directory, &recipe.required_files);
            let mut required: Vec<String> = recipe
                .required_files
                .iter()
                .map(|file| file.filename.clone())
                .collect();
            if *kind == "lora" {
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
                    crate::recipe::PromptMode::ClipScene => "clip_scene".to_string(),
                    crate::recipe::PromptMode::EditInstruction => "edit_instruction".to_string(),
                },
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

/// Binds the weight at `path` to a recipe, in the sidecar beside it named with
/// `contract`'s suffix.
pub fn write_sidecar(
    path: &Path,
    sidecar: &WeightSidecar,
    contract: &Contract,
) -> std::io::Result<()> {
    let encoded = serde_json::to_vec_pretty(sidecar)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    fs::write(sidecar_path(path, contract), encoded)
}

/// Where a weight of `kind` sits under the models directory.
pub fn relative_weight_path(kind: &str, filename: &str) -> Option<PathBuf> {
    let directory = match kind {
        "checkpoint" => "checkpoints",
        "diffusion_model" => "diffusion_models",
        "lora" => "loras",
        "vae" => "vae",
        "text_encoder" => "text_encoders",
        _ => return None,
    };
    Some(PathBuf::from(directory).join(filename))
}

fn resolve_recipe<'a>(
    catalog: &'a RecipeCatalog,
    filename: &str,
    kind: &str,
    document: Option<&WeightDocument>,
) -> Option<&'a Recipe> {
    if kind == "lora" {
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
        return (recipe.kind == MediaKind::Image
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
    loras: &Path,
    filename: &str,
    contract: &Contract,
) -> Option<PathBuf> {
    let filename = sanitize_weight_filename(filename).ok()?;
    Some(
        loras
            .join(&contract.publication_directory)
            .join(format!("{filename}{PUBLICATION_SUFFIX}")),
    )
}

fn publication_pending(models_directory: &Path, filename: &str, contract: &Contract) -> bool {
    publication_marker(&models_directory.join("loras"), filename, contract)
        .is_some_and(|path| fs::symlink_metadata(path).is_ok())
}

fn missing_required(models_directory: &Path, required: &[RequiredFile]) -> Vec<String> {
    required
        .iter()
        .filter(|file| {
            !models_directory
                .join(&file.directory)
                .join(&file.filename)
                .is_file()
        })
        .map(|file| file.filename.clone())
        .collect()
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
            &WeightSidecar {
                recipe_id: "qwen-image-edit-adapter".into(),
                huggingface_base: Some("Qwen/Qwen-Image-Edit-2511".into()),
            },
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
            &WeightSidecar {
                recipe_id: "flux-schnell-adapter".into(),
                huggingface_base: None,
            },
            &Contract::default(),
        )
        .unwrap();
        write_sidecar(
            &mismatched,
            &WeightSidecar {
                recipe_id: "flux-schnell-adapter".into(),
                huggingface_base: Some("Qwen/Qwen-Image-Edit-2511".into()),
            },
            &Contract::default(),
        )
        .unwrap();
        write_sidecar(
            &unknown,
            &WeightSidecar {
                recipe_id: "missing-adapter".into(),
                huggingface_base: Some("Qwen/Qwen-Image-Edit-2511".into()),
            },
            &Contract::default(),
        )
        .unwrap();
        fs::write(
            sidecar_path(&malformed, &Contract::default()),
            serde_json::to_vec(&WeightDocument {
                sidecar: WeightSidecar {
                    recipe_id: "flux-schnell-adapter".into(),
                    huggingface_base: Some("black-forest-labs/FLUX.1-schnell".into()),
                },
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
    fn pending_publication_is_not_listed() {
        let root = temp_models();
        let lora = root.join("loras/style.safetensors");
        fs::write(&lora, b"lora").unwrap();
        fs::write(
            sidecar_path(&lora, &Contract::default()),
            serde_json::to_vec(&WeightDocument {
                sidecar: WeightSidecar {
                    recipe_id: "flux-schnell-adapter".into(),
                    huggingface_base: Some("black-forest-labs/FLUX.1-schnell".into()),
                },
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
    fn a_sidecar_is_read_only_under_the_suffix_the_contract_names() {
        let root = temp_models();
        let lora = root.join("loras/style.safetensors");
        fs::write(&lora, b"lora").unwrap();
        write_sidecar(
            &lora,
            &WeightSidecar {
                recipe_id: "flux-schnell-adapter".into(),
                huggingface_base: Some("black-forest-labs/FLUX.1-schnell".into()),
            },
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
                &WeightSidecar {
                    recipe_id: "flux-schnell-adapter".into(),
                    huggingface_base: Some("black-forest-labs/FLUX.1-schnell".into()),
                },
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
}
