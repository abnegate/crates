//! A person's generation extras, for a weight bound to an identity: an
//! OpenPose pose through the recipe's ControlNet branch, the identity's face
//! through an IP-Adapter, its textual-inversion embedding in the prompt, and a
//! low-denoise pass over the head of every result.

mod extras;
mod head_crop;
mod pose_image;

pub(crate) use extras::Extras;
pub(crate) use head_crop::HeadCrop;
pub(crate) use pose_image::PoseImage;

use crate::client::Error;
use crate::config::Config;
use crate::dataset;
use crate::inventory;
use crate::inventory::WeightSidecar;
use crate::person;
use crate::recipe::CLIP_VISION_DIRECTORY;
use crate::recipe::IPADAPTER_DIRECTORY;
use crate::recipe::Recipe;
use crate::recipe::RecipeExtra;
use crate::recipe::RequiredFile;
use crate::recipe::sanitize_upload_name;
use crate::subject::CENTRE;
use crate::subject::Subject;
use abnegate_vision::Raster;
use abnegate_vision::Region;
use abnegate_vision::Rendered;
use abnegate_vision::Target;
use abnegate_vision::crop;
use abnegate_vision::decode;
use abnegate_vision::decode::Layout;
use abnegate_vision::decode::Orientation;
use serde_json::Map;
use serde_json::Value;
use serde_json::json;
use std::path::Path;

/// How strongly the face reference steers the model.
const IPADAPTER_STRENGTH: f64 = 0.4;
/// How far a face-refine pass may move from the head it refines.
pub(crate) const REFINE_DENOISE: f64 = 0.3;
const POSE_UPLOAD_PREFIX: &str = "pose-";
const EMBEDDING_PREFIX: &str = "embedding:";
const LOAD_IMAGE: &str = "LoadImage";
const CLIP_VISION_LOADER: &str = "CLIPVisionLoader";

/// Which extras a generation with `recipe` runs for the selected weight,
/// bound by `sidecar`: none unless the weight is an identity, a pose only for
/// a new picture through a pose slot whose files and OpenPose images are
/// installed, and a face only for a recipe that supports one, with its files
/// and the identity's face reference installed.
pub(crate) fn extras(
    config: &Config,
    recipe: &Recipe,
    sidecar: Option<&WeightSidecar>,
    has_source: bool,
) -> Extras {
    if !is_identity(sidecar) {
        return Extras::default();
    }
    let models = config.models_directory.as_path();
    let installed = |files: &[RequiredFile]| inventory::missing_required(models, files).is_empty();
    Extras {
        pose: !has_source
            && recipe.has_pose_slot()
            && config.poses_directory.is_some()
            && installed(&recipe.pose_required_files),
        face: recipe.supports(RecipeExtra::Face)
            && installed(&recipe.face_required_files)
            && inventory::face_path(models, &config.checkpoint, sidecar).is_some(),
    }
}

/// Whether a result of `recipe` gets a face-refine pass when the selected
/// weight is bound by `sidecar`.
pub(crate) fn refines(recipe: &Recipe, sidecar: Option<&WeightSidecar>) -> bool {
    recipe.supports(RecipeExtra::Refine)
        || (recipe.supports(RecipeExtra::IdentityRefine) && is_identity(sidecar))
}

/// `prompt` led by `embedding:<stem>` for the embedding `sidecar` names,
/// unless the prompt already holds it or the stem is not a plain name.
pub(crate) fn prefix_embedding(prompt: &str, sidecar: Option<&WeightSidecar>) -> String {
    let Some(token) = sidecar
        .and_then(|sidecar| sidecar.embedding.as_deref())
        .and_then(embedding_token)
    else {
        return prompt.to_string();
    };
    if prompt.split_whitespace().any(|word| word == token) {
        return prompt.to_string();
    }
    format!("{token} {prompt}")
}

/// The pose cluster a prompt is posed with: the last one it names in
/// [`dataset::pose_clusters`] order, so a framing such as "close up" outranks
/// the body pose before it, or standing when it names none.
pub(crate) fn pose_cluster(prompt: &str) -> &'static str {
    dataset::pose_clusters(prompt)
        .last()
        .copied()
        .unwrap_or(dataset::POSES[0][0])
}

/// The OpenPose image in `directory` for the pose `prompt` names, or `None`
/// when it is missing, does not decode as an image, or is not an image
/// [`inventory::read_image`] reads.
pub(crate) fn pose_image(directory: &Path, prompt: &str) -> Option<PoseImage> {
    let filename = dataset::pose_filename(pose_cluster(prompt))?;
    let bytes = inventory::read_image(&directory.join(&filename))
        .filter(|bytes| decode::decode(bytes).is_ok())?;
    let name = sanitize_upload_name(&format!("{POSE_UPLOAD_PREFIX}{filename}")).ok()?;
    Some(PoseImage { name, bytes })
}

/// Routes the model `recipe`'s sampler reads through `node`, an IP-Adapter
/// face node, steered by `face`, an image already uploaded to ComfyUI. The
/// nodes it adds take ids no node of `workflow` has.
pub(crate) fn attach_face(
    workflow: &mut Value,
    recipe: &Recipe,
    face: &str,
    node: &str,
) -> Result<(), Error> {
    let adapter = recipe
        .face_file(IPADAPTER_DIRECTORY)
        .ok_or(Error::Configuration("recipe declares no IP-Adapter weight"))?;
    let encoder = recipe
        .face_file(CLIP_VISION_DIRECTORY)
        .ok_or(Error::Configuration(
            "recipe declares no CLIP vision encoder",
        ))?;
    let face = sanitize_upload_name(face)?;
    let nodes = workflow
        .as_object_mut()
        .ok_or(Error::Configuration("workflow is not an object"))?;
    let sampler = recipe.sampler_node();
    let model = nodes
        .get(sampler)
        .and_then(|sampler| sampler.pointer("/inputs/model"))
        .filter(|model| model.is_array())
        .cloned()
        .ok_or(Error::Configuration("the sampler reads no linked model"))?;
    let first = first_free_id(nodes)?;
    let [image, vision, steered] = [0, 1, 2].map(|offset| first.saturating_add(offset).to_string());
    nodes.insert(
        image.clone(),
        json!({ "class_type": LOAD_IMAGE, "inputs": { "image": face } }),
    );
    nodes.insert(
        vision.clone(),
        json!({ "class_type": CLIP_VISION_LOADER, "inputs": { "clip_name": encoder.filename } }),
    );
    nodes.insert(
        steered.clone(),
        json!({
            "class_type": node,
            "inputs": {
                "model": model,
                "clip_vision": [vision, 0],
                "image": [image, 0],
                "ipadapter_name": adapter.filename,
                "strength": IPADAPTER_STRENGTH
            }
        }),
    );
    let input = nodes
        .get_mut(sampler)
        .and_then(|sampler| sampler.pointer_mut("/inputs/model"))
        .ok_or(Error::Configuration("the sampler reads no linked model"))?;
    *input = json!([steered, 0]);
    Ok(())
}

/// Sets how far `recipe`'s sampler may move from its starting image.
pub(crate) fn set_denoise(
    workflow: &mut Value,
    recipe: &Recipe,
    denoise: f64,
) -> Result<(), Error> {
    let pointer = format!("/{}/inputs/denoise", recipe.sampler_node());
    let value = workflow
        .pointer_mut(&pointer)
        .ok_or(Error::Configuration("the sampler has no denoise input"))?;
    *value = json!(denoise);
    Ok(())
}

/// The head of the person in `bytes`, framed as a person's training head
/// crop is, or the whole image when that crop would frame all of it. `None`
/// when the image cannot be decoded, cropped or encoded.
pub(crate) fn head_crop(bytes: &[u8], subject: &Subject) -> Option<HeadCrop> {
    let raster = decode::decode(bytes).ok()?;
    let size = raster.oriented_size();
    let body = person::bucket(size.0, size.1);
    let head = person::close_target(body);
    if person::is_duplicate(body, head) {
        return Some(HeadCrop {
            png: bytes.to_vec(),
            region: Region::new(0, 0, size.0, size.1),
            full_frame: true,
        });
    }
    let focus = person::head_focus(subject.focus(&raster, CENTRE));
    let region = crop::plan(size, head, focus).ok()?;
    let rendered = crop::render(&raster, region, head).ok()?;
    Some(HeadCrop {
        png: encode_rendered(&rendered)?,
        region,
        full_frame: false,
    })
}

/// `original` with `refined` scaled into `region`, as PNG. `None` when either
/// cannot be decoded, `original` is not upright, or `region` does not lie
/// inside it.
pub(crate) fn paste(original: &[u8], refined: &[u8], region: Region) -> Option<Vec<u8>> {
    let mut raster = decode::decode(original).ok()?;
    if raster.orientation != Orientation::Normal {
        return None;
    }
    let refined = decode::decode(refined).ok()?;
    let (width, height) = refined.oriented_size();
    let scaled = crop::render(
        &refined,
        Region::new(0, 0, width, height),
        Target::new(region.width, region.height),
    )
    .ok()?;
    paste_pixels(&mut raster, region, &scaled)?;
    encode(&raster.pixels, raster.width, raster.height, raster.layout)
}

fn is_identity(sidecar: Option<&WeightSidecar>) -> bool {
    sidecar
        .and_then(|sidecar| sidecar.trigger.as_deref())
        .is_some_and(|trigger| !trigger.trim().is_empty())
}

fn embedding_token(name: &str) -> Option<String> {
    let stem = Path::new(name).file_stem()?.to_str()?.trim();
    let plain = !stem.is_empty()
        && stem
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    plain.then(|| format!("{EMBEDDING_PREFIX}{stem}"))
}

fn first_free_id(nodes: &Map<String, Value>) -> Result<u64, Error> {
    nodes
        .keys()
        .filter_map(|id| id.parse::<u64>().ok())
        .max()
        .map_or(Some(1), |highest| highest.checked_add(1))
        .filter(|first| first.checked_add(2).is_some())
        .ok_or(Error::Configuration("workflow node ids are exhausted"))
}

fn paste_pixels(raster: &mut Raster, region: Region, patch: &Rendered) -> Option<()> {
    let destination_channels = raster.layout.channels();
    let pixels = (patch.width as usize).checked_mul(patch.height as usize)?;
    if pixels == 0 || patch.width != region.width || patch.height != region.height {
        return None;
    }
    let source_channels = patch.pixels.len() / pixels;
    if !matches!(source_channels, 3 | 4) || patch.pixels.len() != pixels * source_channels {
        return None;
    }
    if region.x.checked_add(region.width)? > raster.width
        || region.y.checked_add(region.height)? > raster.height
    {
        return None;
    }
    let raster_width = raster.width as usize;
    for row in 0..region.height as usize {
        for column in 0..region.width as usize {
            let source = (row * region.width as usize + column) * source_channels;
            let destination =
                ((region.y as usize + row) * raster_width + region.x as usize + column)
                    * destination_channels;
            raster.pixels[destination..destination + 3]
                .copy_from_slice(&patch.pixels[source..source + 3]);
        }
    }
    Some(())
}

fn encode_rendered(rendered: &Rendered) -> Option<Vec<u8>> {
    let pixels = (rendered.width as usize).checked_mul(rendered.height as usize)?;
    let layout = match rendered.pixels.len().checked_div(pixels)? {
        3 => Layout::Rgb,
        4 => Layout::Rgba,
        _ => return None,
    };
    encode(&rendered.pixels, rendered.width, rendered.height, layout)
}

fn encode(pixels: &[u8], width: u32, height: u32, layout: Layout) -> Option<Vec<u8>> {
    use image::ImageEncoder;
    let color = match layout {
        Layout::Rgb => image::ExtendedColorType::Rgb8,
        Layout::Rgba => image::ExtendedColorType::Rgba8,
        _ => return None,
    };
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(pixels, width, height, color)
        .ok()?;
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::Fill;
    use crate::recipe::RecipeCatalog;
    use crate::train::Contract;
    use std::collections::HashMap;
    use std::path::PathBuf;

    const SDXL_BASE: &str = "SG161222/RealVisXL_V5.0";

    fn catalog() -> RecipeCatalog {
        RecipeCatalog::packaged().unwrap()
    }

    fn png(width: u32, height: u32, rgb: [u8; 3]) -> Vec<u8> {
        let pixels: Vec<u8> = (0..width * height).flat_map(|_| rgb).collect();
        encode(&pixels, width, height, Layout::Rgb).unwrap()
    }

    fn identity() -> WeightSidecar {
        WeightSidecar::new("sdxl-adapter")
            .with_huggingface_base(SDXL_BASE)
            .with_trigger("ohwx")
    }

    fn posed_sdxl(recipe: &Recipe) -> Value {
        let weights = recipe.weight_map("jerry.safetensors").unwrap();
        let weights = weights
            .iter()
            .map(|(name, filename)| (name.as_str(), filename.as_str()))
            .collect::<HashMap<_, _>>();
        recipe
            .apply(Fill::new("ohwx sitting", 1, weights).with_pose("pose-sitting.png"))
            .unwrap()
    }

    fn install(models: &Path, files: &[RequiredFile]) {
        for file in files {
            let directory = models.join(&file.directory);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join(&file.filename), b"weight").unwrap();
        }
    }

    fn person_models() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        std::fs::create_dir_all(models.join("loras")).unwrap();
        let lora = models.join("loras/jerry.safetensors");
        std::fs::write(&lora, b"lora").unwrap();
        inventory::write_sidecar(
            &lora,
            &identity().with_face("jerry.face.png"),
            &Contract::default(),
        )
        .unwrap();
        (root, models)
    }

    #[test]
    fn a_prompt_naming_no_pose_stands_and_the_last_pose_named_wins() {
        for (prompt, image) in [
            ("a lighthouse at dusk", "standing.png"),
            ("ohwx sitting in a kitchen", "sitting.png"),
            ("standing close up", "close_up.png"),
            ("lying on a sofa, full body", "full_body.png"),
            ("lying on back on a sofa", "on_back.png"),
            ("on all fours on a rug", "all_fours.png"),
        ] {
            assert_eq!(
                dataset::pose_filename(pose_cluster(prompt)).as_deref(),
                Some(image),
                "{prompt}"
            );
        }
    }

    #[test]
    fn the_pose_image_comes_from_the_poses_directory() {
        let poses = tempfile::tempdir().unwrap();
        assert!(pose_image(poses.path(), "ohwx sitting").is_none());
        std::fs::write(poses.path().join("sitting.png"), b"").unwrap();
        assert!(
            pose_image(poses.path(), "ohwx sitting").is_none(),
            "an empty image poses nothing"
        );
        let openpose = png(4, 4, [0, 0, 0]);
        std::fs::write(poses.path().join("sitting.png"), &openpose).unwrap();
        let image = pose_image(poses.path(), "ohwx sitting").unwrap();
        assert_eq!(image.name, "pose-sitting.png");
        assert_eq!(image.bytes, openpose);
    }

    #[test]
    fn embedding_prefix_uses_the_sidecar_stem() {
        let sidecar = identity().with_embedding("ohwx.safetensors");
        assert_eq!(
            prefix_embedding("ohwx standing", Some(&sidecar)),
            "embedding:ohwx ohwx standing"
        );
        assert_eq!(
            prefix_embedding("embedding:ohwx already there", Some(&sidecar)),
            "embedding:ohwx already there"
        );
        assert_eq!(prefix_embedding("ohwx standing", None), "ohwx standing");
        assert_eq!(
            prefix_embedding("ohwx", Some(&identity().with_embedding("bad name.pt"))),
            "ohwx",
            "a stem that is not one plain name is not an embedding"
        );
    }

    #[test]
    fn the_face_node_wraps_the_model_the_sampler_reads() {
        let catalog = catalog();
        let recipe = catalog.get("sdxl-adapter").unwrap();
        let mut workflow = posed_sdxl(recipe);
        let model = workflow["3"]["inputs"]["model"].clone();
        attach_face(&mut workflow, recipe, "jerry.face.png", "AcmeIPAdapterFace").unwrap();

        let steered = workflow["3"]["inputs"]["model"][0]
            .as_str()
            .unwrap()
            .to_string();
        let node = &workflow[&steered];
        assert_eq!(node["class_type"], "AcmeIPAdapterFace");
        assert_eq!(node["inputs"]["model"], model);
        assert_eq!(
            node["inputs"]["ipadapter_name"],
            "ip-adapter-plus-face_sdxl_vit-h.safetensors"
        );
        assert_eq!(node["inputs"]["strength"], json!(IPADAPTER_STRENGTH));
        let image = node["inputs"]["image"][0].as_str().unwrap();
        assert_eq!(workflow[image]["class_type"], LOAD_IMAGE);
        assert_eq!(workflow[image]["inputs"]["image"], "jerry.face.png");
        let vision = node["inputs"]["clip_vision"][0].as_str().unwrap();
        assert_eq!(workflow[vision]["class_type"], CLIP_VISION_LOADER);
        assert_eq!(
            workflow[vision]["inputs"]["clip_name"],
            "CLIP-ViT-H-14-laion2B-s32B-b79K.safetensors"
        );
        assert_eq!(workflow["22"]["class_type"], "ControlNetApplyAdvanced");
        assert_eq!(workflow["3"]["inputs"]["positive"], json!(["22", 0]));
    }

    #[test]
    fn the_face_nodes_never_replace_a_node_the_graph_has() {
        let catalog = catalog();
        let recipe = catalog.get("sdxl").unwrap();
        let mut workflow = posed_sdxl(recipe);
        let before: Vec<String> = workflow.as_object().unwrap().keys().cloned().collect();
        let highest = before
            .iter()
            .filter_map(|id| id.parse::<u64>().ok())
            .max()
            .unwrap();
        attach_face(&mut workflow, recipe, "face.png", "AbnegateIPAdapterFace").unwrap();
        let nodes = workflow.as_object().unwrap();
        assert_eq!(nodes.len(), before.len() + 3);
        for offset in 1..=3 {
            assert!(nodes.contains_key(&(highest + offset).to_string()));
        }
        assert!(
            attach_face(
                &mut workflow,
                recipe,
                "../face.png",
                "AbnegateIPAdapterFace"
            )
            .is_err()
        );
        let flux = catalog.get("flux-schnell").unwrap();
        assert!(
            attach_face(&mut workflow, flux, "face.png", "AbnegateIPAdapterFace").is_err(),
            "a recipe without face files has nothing to load"
        );
    }

    #[test]
    fn refine_denoise_overrides_the_sampler() {
        let catalog = catalog();
        let recipe = catalog.get("sdxl").unwrap();
        let mut workflow = posed_sdxl(recipe);
        set_denoise(&mut workflow, recipe, REFINE_DENOISE).unwrap();
        assert_eq!(workflow["3"]["inputs"]["denoise"], json!(0.3));
        workflow["3"]["inputs"]
            .as_object_mut()
            .unwrap()
            .remove("denoise");
        assert!(set_denoise(&mut workflow, recipe, REFINE_DENOISE).is_err());
    }

    #[test]
    fn a_square_close_up_refines_the_whole_frame() {
        let image = png(64, 64, [10, 20, 30]);
        let crop = head_crop(&image, &Subject::none()).unwrap();
        assert!(crop.full_frame);
        assert_eq!(crop.png, image);
        assert_eq!(crop.region, Region::new(0, 0, 64, 64));
    }

    #[test]
    fn a_portrait_emits_a_head_crop_and_pastes_it_back() {
        let image = png(64, 96, [12, 34, 56]);
        let crop = head_crop(&image, &Subject::none()).unwrap();
        assert!(!crop.full_frame);
        assert!(decode::decode(&crop.png).is_ok());
        let patch = png(crop.region.width, crop.region.height, [200, 10, 10]);
        let pasted = paste(&image, &patch, crop.region).unwrap();
        let raster = decode::decode(&pasted).unwrap();
        assert_eq!((raster.width, raster.height), (64, 96));
        let at = |x: u32, y: u32| {
            let index = ((y * raster.width + x) * 3) as usize;
            raster.pixels[index..index + 3].to_vec()
        };
        assert_eq!(at(crop.region.x, crop.region.y), [200, 10, 10]);
        let below = crop.region.y + crop.region.height;
        assert!(below < 96);
        assert_eq!(at(0, below), [12, 34, 56], "outside the head is untouched");
    }

    #[cfg(unix)]
    #[test]
    fn a_pose_swapped_for_a_link_is_not_read() {
        let poses = tempfile::tempdir().unwrap();
        let elsewhere = poses.path().join("elsewhere.png");
        std::fs::write(&elsewhere, b"secret").unwrap();
        std::os::unix::fs::symlink(&elsewhere, poses.path().join("standing.png")).unwrap();

        assert!(pose_image(poses.path(), "a person").is_none());

        std::fs::remove_file(poses.path().join("standing.png")).unwrap();
        std::fs::write(poses.path().join("standing.png"), b"pose").unwrap();
        assert!(
            pose_image(poses.path(), "a person").is_none(),
            "a pose that is no image is left out"
        );
        let png = encode(&[0, 0, 0], 1, 1, Layout::Rgb).unwrap();
        std::fs::write(poses.path().join("standing.png"), &png).unwrap();
        let pose = pose_image(poses.path(), "a person").unwrap();
        assert_eq!(pose.bytes, png);
        assert_eq!(pose.name, "pose-standing.png");
    }

    #[test]
    fn a_patch_outside_the_image_is_not_pasted() {
        let image = png(16, 16, [1, 2, 3]);
        let patch = png(8, 8, [4, 5, 6]);
        assert!(paste(&image, &patch, Region::new(12, 0, 8, 8)).is_none());
        assert!(paste(&image, b"not an image", Region::new(0, 0, 8, 8)).is_none());
    }

    #[test]
    fn people_recipes_refine_without_a_bound_identity() {
        let catalog = catalog();
        let adapter = catalog.get("sdxl-adapter").unwrap();
        let sdxl = catalog.get("sdxl").unwrap();
        let flux = catalog.get("flux-schnell").unwrap();
        assert!(refines(adapter, None));
        assert!(!refines(sdxl, None));
        assert!(!refines(flux, None));
        assert!(refines(sdxl, Some(&identity())));
        assert!(!refines(flux, Some(&identity())));
        assert!(!refines(sdxl, Some(&identity().with_trigger("  "))));
    }

    #[test]
    fn extras_require_a_bound_identity_and_the_weight_files() {
        let (_root, models) = person_models();
        let poses = tempfile::tempdir().unwrap();
        let catalog = catalog();
        let recipe = catalog.get("sdxl-adapter").unwrap();
        let mut config = Config {
            models_directory: models.clone(),
            checkpoint: "jerry.safetensors".into(),
            ..Default::default()
        };
        let sidecar = identity();
        assert_eq!(
            extras(&config, recipe, Some(&sidecar), false),
            Extras::default()
        );
        install(&models, &recipe.pose_required_files);
        install(&models, &recipe.face_required_files);
        std::fs::write(models.join("loras/jerry.face.png"), b"face").unwrap();
        assert_eq!(
            extras(&config, recipe, Some(&sidecar), false),
            Extras {
                pose: false,
                face: true,
            },
            "nothing poses without a poses directory"
        );
        config.poses_directory = Some(poses.path().to_path_buf());
        assert_eq!(
            extras(&config, recipe, Some(&sidecar), false),
            Extras {
                pose: true,
                face: true,
            }
        );
        assert_eq!(
            extras(&config, recipe, Some(&sidecar), true),
            Extras {
                pose: false,
                face: true,
            },
            "an edit keeps its source's pose"
        );
        assert_eq!(
            extras(
                &config,
                catalog.get("flux-schnell").unwrap(),
                Some(&sidecar),
                false
            ),
            Extras::default()
        );
        assert_eq!(extras(&config, recipe, None, false), Extras::default());
        std::fs::remove_file(models.join("loras/jerry.face.png")).unwrap();
        assert_eq!(
            extras(&config, recipe, Some(&sidecar), false),
            Extras {
                pose: true,
                face: false,
            }
        );
    }
}
