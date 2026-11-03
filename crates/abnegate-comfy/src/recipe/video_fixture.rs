use crate::recipe::PACKAGED_CATALOG;
use crate::recipe::RecipeCatalog;
use serde_json::Value;
use serde_json::json;
use std::path::Path;
use std::path::PathBuf;

pub(crate) const VIDEO_ADAPTER: &str = "video-adapter";
pub(crate) const VIDEO_BASE: &str = "video-base.safetensors";
pub(crate) const VIDEO_HUGGINGFACE_BASE: &str = "Example/Video-Base";
const GRAPH: &str = "video-adapter-api.json";

/// A video adapter recipe a host could add in an overlay catalog: the crate
/// packages none.
pub(crate) fn video_adapter() -> Value {
    json!({
        "id": VIDEO_ADAPTER,
        "kind": "video",
        "label": "Video adapter",
        "adapter": true,
        "bare": GRAPH,
        "output_node": "10",
        "output": "save_webm",
        "slots": {
            "prompt": "/5/inputs/text",
            "seed": "/8/inputs/seed",
            "weights": {
                "unet": "/1/inputs/unet_name",
                "lora": "/12/inputs/lora_name"
            }
        },
        "defaults": { "unet": VIDEO_BASE },
        "required_files": [{ "filename": VIDEO_BASE, "directory": "diffusion_models" }],
        "hf_bases": [VIDEO_HUGGINGFACE_BASE]
    })
}

pub(crate) fn packaged_with_video_adapter() -> Value {
    let mut file: Value = serde_json::from_str(PACKAGED_CATALOG).unwrap();
    file["recipes"]
        .as_array_mut()
        .unwrap()
        .push(video_adapter());
    file
}

pub(crate) fn write_overlay(root: &Path, file: &Value) -> PathBuf {
    let workflows = root.join("workflows");
    let recipes = root.join("recipes");
    std::fs::create_dir_all(&workflows).unwrap();
    std::fs::create_dir_all(&recipes).unwrap();
    std::fs::write(recipes.join("catalog.json"), file.to_string()).unwrap();
    std::fs::write(workflows.join(GRAPH), graph().to_string()).unwrap();
    workflows.join(GRAPH)
}

pub(crate) fn load(file: &Value) -> Result<RecipeCatalog, crate::client::Error> {
    let root = tempfile::tempdir().unwrap();
    RecipeCatalog::load(Some(&write_overlay(root.path(), file)))
}

pub(crate) fn catalog() -> RecipeCatalog {
    load(&packaged_with_video_adapter()).unwrap()
}

fn graph() -> Value {
    json!({
        "1": { "class_type": "UNETLoader", "inputs": { "unet_name": VIDEO_BASE, "weight_dtype": "default" } },
        "5": { "class_type": "CLIPTextEncode", "inputs": { "text": "" } },
        "8": { "class_type": "KSampler", "inputs": { "seed": 0, "model": ["12", 0] } },
        "10": { "class_type": "SaveWEBM", "inputs": { "images": ["8", 0] } },
        "12": { "class_type": "LoraLoaderModelOnly", "inputs": { "lora_name": "", "model": ["1", 0] } }
    })
}
