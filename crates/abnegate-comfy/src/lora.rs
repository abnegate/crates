//! Packaged LoRA training jobs. By default the packaged training graph runs on
//! ComfyUI; [`Config::train_command`] hands the dataset to an external trainer
//! instead.

mod dropped;
mod process_group;
mod remediation;
mod remediation_outcome;
mod screening;
mod train_base;
mod train_error;
mod train_image;
mod train_method;
mod train_outcome;
mod train_policy;
mod train_request;
mod train_rule;
mod train_subject;

pub use dropped::Dropped;
pub use remediation::Remediation;
pub use remediation_outcome::RemediationOutcome;
pub use screening::Screening;
pub use train_base::TrainBase;
pub use train_error::TrainError;
pub use train_image::TrainImage;
pub use train_method::TrainMethod;
pub use train_outcome::TrainOutcome;
pub use train_policy::TrainPolicy;
pub use train_request::TrainRequest;
pub use train_rule::TrainRule;
pub use train_subject::TrainSubject;

use crate::caption::Captioner;
use crate::caption::Draft;
use crate::client::Client;
use crate::client::SourceImage;
use crate::config::Config;
use crate::inventory::WeightDocument;
use crate::inventory::WeightSidecar;
use crate::inventory::contains_phrase;
use crate::inventory::publication_marker;
use crate::inventory::sidecar_path;
use crate::recipe::Recipe;
use crate::recipe::RecipeCatalog;
use crate::recipe::TrainingModel;
use crate::recipe::sanitize_weight_filename;
use crate::subject::CENTRE;
use crate::subject::Subject;
use crate::train::Contract;
use crate::train::Phase;
use crate::train::Reporter;
use crate::train::Run;
use crate::train::TrainProgress;
use abnegate_exec::EnvironmentPolicy;
use abnegate_secret::SecretValue;
use abnegate_vision::Raster;
use abnegate_vision::Rendered;
use abnegate_vision::decode;
use abnegate_vision::gravity::Point;
use process_group::ProcessGroup;
use serde::Deserialize;
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::sync::Weak;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStderr;
use tokio::process::Command;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use uuid::Uuid;

/// What [`validate_request`] settles about a request before any image is read.
struct Plan {
    filename: String,
    base: String,
    model: TrainingModel,
    trigger: String,
    destination: Destination,
}

/// Where a finished weight is published, and what its sidecar says runs it.
struct Destination {
    directory: String,
    recipe_id: String,
    huggingface_base: Option<String>,
}

struct ScreenedImage {
    original: usize,
    target: Vec<u8>,
    reference: Option<Vec<u8>>,
    text: String,
    /// Base64 of the crop, kept only for identity runs, which caption it.
    encoded: Option<String>,
    group: usize,
}

struct RemediationAttempt {
    index: usize,
    remediation: Remediation,
    replaced: bool,
}

struct Attempt {
    id: String,
    root: PathBuf,
    input: Option<PathBuf>,
    folder: Option<String>,
    artifact: Option<String>,
    produced: Option<PathBuf>,
}

/// The directory a LoRA is published in, the one ComfyUI's LoRA loader reads.
const LORAS: &str = "loras";
/// Each weight slot a fine-tune can load into, with the directory under the
/// models root ComfyUI's loader for that slot reads.
const FINETUNE_SLOTS: &[(&str, &str)] =
    &[("checkpoint", "checkpoints"), ("unet", "diffusion_models")];
const METHOD_UNAVAILABLE: &str = "this base does not train with that method";
const SUBJECT_UNAVAILABLE: &str = "this base does not train that subject";
const UNRUNNABLE_RULE: &str = "training rule names a recipe that cannot run its weight";
const NEEDED_WEIGHT: &str = "this name belongs to a weight the catalog or a training rule needs";
const NAMED_ELSEWHERE: &str = "a weight in another model folder already has this name";
const FOREIGN_WEIGHT: &str = "a weight training did not publish already has this name";

/// Bytes of a trainer's stderr kept to explain a failure.
const STDERR_TAIL: usize = 4096;
const STDERR_CHUNK: usize = 1024;
/// How long a finished trainer's stderr is given to reach end of file.
const STDERR_DRAIN: Duration = Duration::from_millis(250);

/// Name the weight a publication replaces is kept under inside its attempt,
/// until the replacement is durable. Recovery after a crash reads it back.
const PREVIOUS_WEIGHT: &str = "previous.safetensors";

static PUBLICATIONS: LazyLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl Attempt {
    fn create(models: &Path) -> Result<Self, TrainError> {
        let models = require_directory(models, "models directory")?;
        let id = Uuid::new_v4().to_string();
        let training = ensure_child_directory(&models, "training")?;
        sync_directory(&models)?;
        let root = training.join(&id);
        fs::create_dir(&root).map_err(failed)?;
        sync_directory(&training)?;
        let parent = models.parent();
        let attempt = Self {
            artifact: None,
            input: parent.map(|root| root.join("input")),
            folder: None,
            produced: parent.map(|root| root.join("output").join("loras")),
            id,
            root,
        };
        require_confined_directory(&training, &attempt.root)?;
        Ok(attempt)
    }

    fn register(&mut self, run: &Run, contract: &Contract) -> Result<(), TrainError> {
        run.validate(contract)?;
        self.folder = Some(run.folder.clone());
        self.artifact = Some(run.artifact.clone());
        Ok(())
    }

    fn staged_name(&self) -> String {
        format!("{}.safetensors", self.id)
    }

    fn output(&self) -> PathBuf {
        self.root.join(self.staged_name())
    }

    fn clean_runtime(&self) {
        if let (Some(input), Some(folder)) = (&self.input, &self.folder)
            && safe_directory(input)
        {
            remove_entry(&input.join(folder));
        }
        let (Some(produced), Some(artifact)) = (&self.produced, &self.artifact) else {
            return;
        };
        if !produced.parent().is_some_and(safe_directory) || !safe_directory(produced) {
            return;
        }
        let Ok(entries) = fs::read_dir(produced) else {
            return;
        };
        let final_name = format!("{artifact}.safetensors");
        let checkpoint = format!("{artifact}-step");
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == final_name || name.starts_with(&checkpoint) && name.ends_with(".safetensors")
            {
                remove_entry(&entry.path());
            }
        }
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.clean_runtime();
        remove_entry(&self.root);
    }
}

/// The bases in `catalog` a LoRA can be trained on whose weights are installed
/// under `models_directory`, as [`inventory::scan`](crate::inventory::scan)
/// reads it under `contract`.
pub fn available_bases(
    catalog: &RecipeCatalog,
    models_directory: &Path,
    contract: &Contract,
) -> Vec<TrainBase> {
    let items = crate::inventory::scan(models_directory, catalog, contract);
    catalog
        .image_recipes()
        .filter(|recipe| !recipe.adapter)
        .filter(|recipe| recipe.training_model().is_ok())
        .filter(|recipe| {
            items
                .iter()
                .any(|item| item.recipe_id == recipe.id && item.ready)
        })
        .map(|recipe| TrainBase {
            id: recipe.id.clone(),
            label: recipe.label.clone(),
            edit: matches!(recipe.training_model(), Ok(TrainingModel::QwenEdit { .. })),
        })
        .collect()
}

/// Train a LoRA from `request` and install the best checkpoint it produced.
///
/// The images are screened for size, blur and duplicates, with an upscale
/// tried on any rejected as too small or blurred; the survivors are cropped
/// to their subject and captioned through `litellm_host` where a caption was
/// left blank; then the packaged graph trains on ComfyUI, or
/// [`Config::train_command`] runs instead, and every checkpoint is scored.
///
/// Fails with [`TrainError::Disabled`] when neither ComfyUI nor a training
/// command is configured, [`TrainError::Invalid`] for a request that cannot
/// train, [`TrainError::Configuration`] for a setting that cannot work, and
/// [`TrainError::Failed`] when the run itself fails.
pub async fn train(
    config: &Config,
    litellm_host: String,
    litellm_key: SecretValue,
    request: TrainRequest,
) -> Result<TrainOutcome, TrainError> {
    train_with_remediation(
        config,
        litellm_host,
        litellm_key,
        request,
        crate::screening::screen,
        Reporter::default(),
    )
    .await
}

/// [`train`], keeping the latest [`TrainProgress`] in `progress` as the run
/// moves through screening, captioning, training and publishing: a [`Phase`]
/// and how far through it the run is per image loaded, cropped and captioned,
/// then whatever the trainer writes to its progress file while it runs. An
/// update identical to the one already there is not sent again.
///
/// The packaged graph's training node writes that file as
/// [`Contract::progress_filename`] names it. A [`Config::train_command`] gets
/// its path in `<prefix>_PROGRESS`, and its progress file is read every
/// [`Config::poll_interval`] while it runs. Its step budget is the one the
/// host keeps in `<prefix>_STEPS`, or, when the host keeps none, the one the
/// packaged training config gives the set, handed to it in `<prefix>_STEPS`.
///
/// The future is `Send`, so a host can run it as a background job and let
/// whoever asked for it go away: once no receiver is left and the host holds
/// no other sender to subscribe one with, updates stop and the training
/// carries on to the end.
pub async fn train_reporting(
    config: &Config,
    litellm_host: String,
    litellm_key: SecretValue,
    request: TrainRequest,
    progress: watch::Sender<Option<TrainProgress>>,
) -> Result<TrainOutcome, TrainError> {
    train_with_remediation(
        config,
        litellm_host,
        litellm_key,
        request,
        crate::screening::screen,
        Reporter::new(progress),
    )
    .await
}

async fn train_with_remediation(
    config: &Config,
    litellm_host: String,
    litellm_key: SecretValue,
    request: TrainRequest,
    screening: fn(&[Vec<u8>], u32) -> crate::screening::Verdict,
    progress: Reporter,
) -> Result<TrainOutcome, TrainError> {
    train_with_pipeline(
        config,
        litellm_host,
        litellm_key,
        request,
        screening,
        true,
        progress,
    )
    .await
}

#[cfg(test)]
async fn train_with_screening(
    config: &Config,
    litellm_host: String,
    litellm_key: SecretValue,
    request: TrainRequest,
    screening: fn(&[Vec<u8>], u32) -> crate::screening::Verdict,
) -> Result<TrainOutcome, TrainError> {
    train_with_pipeline(
        config,
        litellm_host,
        litellm_key,
        request,
        screening,
        false,
        Reporter::default(),
    )
    .await
}

/// Refuses a request [`train`] would refuse, before any image is decoded:
/// training that is not configured, a name that is not a filename, an empty
/// set, a base the catalog cannot train, a subject or method
/// [`Config::train_policy`] does not allow on that base, a rule whose recipe
/// cannot run the weight it publishes or does not load it from the rule's
/// directory, a file the base's rule requires that
/// is not installed, a name a catalog recipe or a rule needs for its own
/// weight, a name a weight in another scanned model folder already has, a
/// name already taken by a weight no training run published, a
/// method other than a LoRA or an SDXL base with no [`Config::train_command`]
/// to run it, a missing trigger word, and references on the wrong kind of
/// base. A host calls it to refuse a bad request up front rather than as a
/// failed job.
pub fn validate_request(config: &Config, request: &TrainRequest) -> Result<(), TrainError> {
    plan(config, request).map(|_| ())
}

fn plan(config: &Config, request: &TrainRequest) -> Result<Plan, TrainError> {
    if config.train_command.is_none() && !config.enabled {
        return Err(TrainError::Disabled);
    }
    config.validate()?;
    let filename = final_filename(&request.name)?;
    if request.images.is_empty() {
        return Err(TrainError::Invalid("training needs images"));
    }
    let catalog = RecipeCatalog::load(config.workflow_path.as_deref())
        .map_err(|_| TrainError::Invalid("recipe catalog is missing"))?;
    let recipe = catalog
        .get(&request.base)
        .filter(|recipe| !recipe.adapter)
        .ok_or(TrainError::Invalid("unknown training base"))?;
    let model = recipe
        .training_model()
        .map_err(|_| TrainError::Invalid("training base is not supported"))?;
    let destination = destination(config, &catalog, recipe, request)?;
    require_free_name(config, &catalog, &destination.directory, &filename)?;
    if request.method != TrainMethod::Lora && config.train_command.is_none() {
        return Err(TrainError::Invalid(
            "the packaged training graph trains only a LoRA; configure a training command",
        ));
    }
    let edit = matches!(&model, TrainingModel::QwenEdit { .. });
    let trigger = request.trigger.as_deref().unwrap_or_default().trim();
    if !edit && trigger.is_empty() {
        return Err(TrainError::Invalid(
            "trigger word is required so the weight can retain identity",
        ));
    }
    if matches!(model, TrainingModel::Sdxl { .. }) && config.train_command.is_none() {
        return Err(TrainError::Invalid(
            "SDXL trains only through a training command",
        ));
    }
    validate_pairing(&request.images, &model)?;
    Ok(Plan {
        filename,
        base: recipe.id.clone(),
        model,
        trigger: trigger.to_string(),
        destination,
    })
}

/// Where [`Config::train_policy`] publishes `request` trained on `recipe`,
/// once its rule allows the request's subject and method, names a recipe in
/// `catalog` that runs the weight from the directory it publishes in, and
/// every file it requires is installed.
fn destination(
    config: &Config,
    catalog: &RecipeCatalog,
    recipe: &Recipe,
    request: &TrainRequest,
) -> Result<Destination, TrainError> {
    let policy = &config.train_policy;
    if policy.rules_for(&recipe.id).next().is_none() {
        if request.method != TrainMethod::Lora {
            return Err(TrainError::Invalid(METHOD_UNAVAILABLE));
        }
        if request.subject != TrainSubject::Other {
            return Err(TrainError::Invalid(SUBJECT_UNAVAILABLE));
        }
        let adapter = recipe
            .training_adapter()
            .map_err(|_| TrainError::Invalid("training adapter mapping is missing"))?;
        return Ok(Destination {
            directory: LORAS.to_string(),
            recipe_id: adapter.recipe_id.clone(),
            huggingface_base: Some(adapter.huggingface_base.clone()),
        });
    }
    let rule = policy
        .rule(&recipe.id, request.method)
        .ok_or(TrainError::Invalid(METHOD_UNAVAILABLE))?;
    let huggingface_base = recipe
        .training_adapter()
        .ok()
        .map(|adapter| adapter.huggingface_base.clone());
    if !catalog.get(&rule.recipe_id).is_some_and(|runner| {
        runs_weight(
            runner,
            recipe,
            rule.method,
            &rule.directory,
            huggingface_base.as_deref(),
        )
    }) {
        return Err(TrainError::Configuration(UNRUNNABLE_RULE));
    }
    if !rule.subjects.contains(&request.subject) {
        return Err(TrainError::Invalid(SUBJECT_UNAVAILABLE));
    }
    if rule
        .required
        .iter()
        .any(|file| !config.models_directory.join(file).is_file())
    {
        return Err(TrainError::Invalid(
            "install the files this base needs before training on it",
        ));
    }
    Ok(Destination {
        directory: rule.directory.clone(),
        recipe_id: rule.recipe_id.clone(),
        huggingface_base,
    })
}

/// Whether `runner` loads a weight trained on `base` with `method` and
/// published in `directory`: for a LoRA, an adapter with a LoRA slot over the
/// base's Hugging Face model, reading `loras`; for a fine-tune, a base recipe
/// with a checkpoint slot reading `checkpoints` or a UNet slot reading
/// `diffusion_models`; and either way one that makes what `base` makes.
fn runs_weight(
    runner: &Recipe,
    base: &Recipe,
    method: TrainMethod,
    directory: &str,
    huggingface_base: Option<&str>,
) -> bool {
    if runner.kind != base.kind {
        return false;
    }
    match method {
        TrainMethod::Lora => {
            directory == LORAS
                && runner.adapter
                && runner.has_lora_slot()
                && huggingface_base.is_some_and(|huggingface_base| {
                    runner
                        .huggingface_bases
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(huggingface_base))
                })
        }
        TrainMethod::Finetune => {
            !runner.adapter
                && !runner.has_lora_slot()
                && FINETUNE_SLOTS
                    .iter()
                    .any(|(slot, folder)| *folder == directory && runner.has_weight_slot(slot))
        }
    }
}

/// Refuses publishing as `filename` in `directory` when a catalog recipe or a
/// training rule needs a weight of that name, when a weight in another folder
/// the inventory scans has it in any ASCII case, or when a weight no training
/// run published already has it: a base checkpoint or a downloaded model is
/// never replaced by a trained one, and a name selects one weight.
fn require_free_name(
    config: &Config,
    catalog: &RecipeCatalog,
    directory: &str,
    filename: &str,
) -> Result<(), TrainError> {
    let required = config
        .train_policy
        .rules
        .iter()
        .flat_map(|rule| &rule.required)
        .filter_map(|file| file.file_name()?.to_str())
        .any(|name| name.eq_ignore_ascii_case(filename));
    if required || catalog.needs_weight(filename) {
        return Err(TrainError::Invalid(NEEDED_WEIGHT));
    }
    if crate::inventory::named_elsewhere(
        &config.models_directory,
        directory,
        filename,
        &config.contract,
    ) {
        return Err(TrainError::Invalid(NAMED_ELSEWHERE));
    }
    require_training_output(
        &config.models_directory.join(directory).join(filename),
        &config.contract,
    )
}

/// Refuses replacing `output` unless nothing is there or its sidecar names
/// the training run that published it.
fn require_training_output(output: &Path, contract: &Contract) -> Result<(), TrainError> {
    match fs::symlink_metadata(output) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(failed(error)),
        Ok(_) => {}
    }
    let published = read_generation(&sidecar_path(output, contract))
        .is_some_and(|generation| Uuid::parse_str(&generation).is_ok());
    if !published {
        return Err(TrainError::Invalid(FOREIGN_WEIGHT));
    }
    Ok(())
}

async fn train_with_pipeline(
    config: &Config,
    litellm_host: String,
    litellm_key: SecretValue,
    request: TrainRequest,
    screening: fn(&[Vec<u8>], u32) -> crate::screening::Verdict,
    repair_rejections: bool,
    progress: Reporter,
) -> Result<TrainOutcome, TrainError> {
    let Plan {
        filename,
        base,
        model,
        trigger,
        destination,
    } = plan(config, &request)?;
    let trigger = trigger.as_str();
    let edit = matches!(&model, TrainingModel::QwenEdit { .. });
    let settings = crate::train::packaged_config()?;
    let side = settings.resolution();
    let progress = progress.with_total(settings.steps(request.images.len()));
    progress.phase(Phase::Screening, "Screening the dataset", 0, 1);
    let (images, decoded) = decode_uploads(request.images, progress.clone()).await?;
    let http =
        crate::http::client(config).map_err(|error| TrainError::Failed(error.to_string()))?;
    progress.phase(Phase::Screening, "Checking the images", 0, 1);
    let (mut decoded, mut verdict) = screen(screening, decoded, side).await?;
    validate_verdict(&verdict, images.len())?;
    let mut attempts = Vec::new();
    if repair_rejections {
        while !verdict.drop.is_empty() {
            progress.phase(Phase::Screening, "Repairing rejected images", 0, 1);
            let fresh = remediate(config, &http, &mut decoded, &images, &verdict, &attempts).await;
            if fresh.is_empty() {
                break;
            }
            attempts.extend(fresh);
            progress.phase(Phase::Screening, "Checking the repaired images", 0, 1);
            (decoded, verdict) = screen(screening, decoded, side).await?;
            validate_verdict(&verdict, images.len())?;
        }
    }
    let attempted = finish_remediation(attempts, &verdict);
    let dropped = verdict
        .drop
        .iter()
        .map(|(index, rejection)| Dropped {
            filename: images[*index].filename.clone(),
            reason: *rejection,
        })
        .collect::<Vec<Dropped>>();
    // Crop before captioning, or the caption describes background the crop removes.
    let subject = Subject::shared(config).await;
    let mut survivors = frame_survivors(
        subject,
        images,
        decoded,
        verdict.keep.clone(),
        side,
        edit,
        progress.clone(),
    )
    .await?;
    let progress = progress.with_total(settings.steps(survivors.len()));
    if survivors.len() != verdict.keep.len() {
        return Err(TrainError::Failed(
            "screening returned an invalid survivor index".to_string(),
        ));
    }
    let described = if edit {
        for image in &mut survivors {
            image.text = image.text.trim().to_string();
        }
        Vec::new()
    } else {
        let mut drafts = survivors
            .iter()
            .map(|image| {
                let encoded = image.encoded.as_deref().ok_or_else(|| {
                    TrainError::Failed("identity survivor is missing its caption input".to_string())
                })?;
                Ok::<Draft, TrainError>(Draft::new(CROP, encoded, &image.text, image.group))
            })
            .collect::<Result<Vec<Draft>, TrainError>>()?;
        progress.phase(Phase::Captioning, "Captioning the dataset", 0, 1);
        let described = Captioner::new(config, litellm_host, litellm_key)
            .fill_with_progress(&mut drafts, trigger, |done, shots| {
                let message = match done {
                    0 => "Captioning the dataset".to_string(),
                    _ => format!("Captioning image {done} of {shots}"),
                };
                progress.phase(Phase::Captioning, message, done, shots);
            })
            .await;
        for (image, draft) in survivors.iter_mut().zip(drafts) {
            image.text = draft.caption;
        }
        described
    };
    let findings = crate::dataset::inspect(&described, survivors.len());
    let mut attempt = Attempt::create(&config.models_directory)?;
    let published = ensure_child_directory(&config.models_directory, &destination.directory)?;
    let output = validate_output(&published, &published.join(&filename))?;
    let output_sidecar = validate_output(&published, &sidecar_path(&output, &config.contract))?;
    let targets = ensure_child_directory(&attempt.root, "targets")?;
    let controls = edit
        .then(|| ensure_child_directory(&attempt.root, "control_1"))
        .transpose()?;
    let mut captions = HashMap::with_capacity(survivors.len());
    for (index, image) in survivors.iter().enumerate() {
        debug_assert_eq!(image.original, verdict.keep[index]);
        let stem = format!("{index:04}");
        write_new(
            &attempt.root,
            &targets.join(format!("{stem}.png")),
            &image.target,
        )?;
        let text = if edit {
            image.text.clone()
        } else {
            identity_caption(&image.text, trigger)
        };
        write_new(
            &attempt.root,
            &targets.join(format!("{stem}.txt")),
            text.as_bytes(),
        )?;
        captions.insert(format!("{stem}.png"), text);
        if let (Some(controls), Some(reference)) = (&controls, &image.reference) {
            write_new(
                &attempt.root,
                &controls.join(format!("{stem}.png")),
                reference,
            )?;
        }
    }
    let staged = attempt.output();
    let host_steps = config
        .train_command
        .as_ref()
        .and_then(|_| std::env::var(config.contract.variable("STEPS")).ok());
    let progress = match host_steps
        .as_deref()
        .and_then(|steps| steps.trim().parse::<u32>().ok())
    {
        Some(steps) => progress.with_total(steps),
        None => progress,
    };
    progress.phase(Phase::Training, "Starting training", 0, 1);
    let run = if let Some(command) = config.train_command.as_deref() {
        let contract = &config.contract;
        let run = Run::new(contract);
        attempt.register(&run, contract)?;
        let progress_file = attempt.root.join(contract.progress_filename(&run.artifact));
        let mut process = Command::new("sh");
        trainer_environment(config).apply(&mut process);
        process
            .arg("-c")
            .arg(command)
            .env(contract.variable("NAME"), &filename)
            .env(contract.variable("FINAL_NAME"), &filename)
            .env(contract.variable("ATTEMPT"), &attempt.id)
            .env(contract.variable("BASE"), &base)
            .env(contract.variable("DIRECTORY"), &attempt.root)
            .env(contract.variable("OUTPUT"), &staged)
            .env(contract.variable("TRIGGER"), trigger)
            .env(contract.variable("SUBJECT"), request.subject.as_str())
            .env(contract.variable("METHOD"), request.method.as_str())
            .env(
                contract.variable("IMAGE_COUNT"),
                survivors.len().to_string(),
            )
            .env(
                contract.variable("ARCHITECTURE"),
                crate::train::architecture(&model),
            )
            .env(contract.variable("FOLDER"), &run.folder)
            .env(contract.variable("ARTIFACT"), &run.artifact)
            .env(contract.variable("PROGRESS"), &progress_file)
            .env(contract.variable("DEFER_CLEANUP"), "1")
            .env(
                contract.variable("TIMEOUT"),
                config.train_timeout.as_secs().to_string(),
            )
            .env(
                contract.input_variable(),
                config
                    .models_directory
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("input")
                    .display()
                    .to_string(),
            );
        if host_steps.is_none() {
            process.env(contract.variable("STEPS"), progress.total().to_string());
        }
        match &model {
            TrainingModel::Flux { checkpoint } | TrainingModel::Sdxl { checkpoint } => {
                process.env(contract.variable("CHECKPOINT"), checkpoint);
            }
            TrainingModel::QwenEdit { unet, clip, vae } => {
                process
                    .env(contract.variable("UNET"), unet)
                    .env(contract.variable("CLIP"), clip)
                    .env(contract.variable("VAE"), vae);
            }
        }
        let trained = forwarding(
            run_trainer(process, config.train_timeout),
            &progress_file,
            config.poll_interval,
            &progress,
        )
        .await;
        if let Err(error) = trained {
            crate::train::cleanup_with(&http, config, &run).await;
            return Err(error);
        }
        run
    } else {
        crate::train::run_with(
            &http,
            config,
            &model,
            &attempt.root,
            &staged,
            survivors.len(),
            &progress,
        )
        .await?
    };
    attempt.register(&run, &config.contract)?;
    if require_regular_file(&attempt.root, &staged).is_err() {
        crate::train::cleanup_with(&http, config, &run).await;
        return Err(TrainError::Failed(
            "trainer did not write a regular weight file".to_string(),
        ));
    }
    let quality = if request.method == TrainMethod::Lora {
        progress.publishing("Choosing the best checkpoint", 0, 2);
        crate::quality::select_with(&http, config, &model, &run, &staged, &captions).await
    } else {
        crate::train::cleanup_with(&http, config, &run).await;
        None
    };
    require_regular_file(&attempt.root, &staged).map_err(|_| {
        tracing::warn!(
            staged = %staged.display(),
            root = %attempt.root.display(),
            "the trained weight is gone after quality selection"
        );
        TrainError::Failed("quality selection did not leave a regular weight file".to_string())
    })?;
    let staged_sidecar = sidecar_path(&staged, &config.contract);
    let bytes = serde_json::to_vec_pretty(&WeightDocument {
        sidecar: WeightSidecar {
            recipe_id: destination.recipe_id,
            huggingface_base: destination.huggingface_base,
            trigger: (!edit && !trigger.is_empty()).then(|| trigger.to_string()),
        },
        generation: Some(attempt.id.clone()),
    })
    .map_err(|error| TrainError::Failed(error.to_string()))?;
    write_new(&attempt.root, &staged_sidecar, &bytes)?;
    validate_output(&published, &output)?;
    validate_output(&published, &output_sidecar)?;
    progress.publishing("Publishing the weight", 1, 2);
    atomic_promote(
        &attempt.root,
        &staged,
        &staged_sidecar,
        &published,
        &output,
        &output_sidecar,
        &attempt.id,
        &config.contract,
    )?;
    Ok(TrainOutcome {
        path: output,
        quality,
        dataset: findings,
        screening: Screening {
            kept: verdict.keep.len(),
            dropped,
            attempted,
        },
    })
}

/// What the external trainer is given besides its run's own variables: the
/// names in [`DEFAULT_ENVIRONMENT`](abnegate_exec::DEFAULT_ENVIRONMENT), every
/// variable this process has under the contract's environment prefix, and the
/// ComfyUI server with the token and header that reach it. Nothing else of
/// this process's environment, which holds credentials the trainer has no use
/// for.
fn trainer_environment(config: &Config) -> EnvironmentPolicy {
    let prefix = format!("{}_", config.contract.environment_prefix);
    let settings = std::env::vars_os()
        .filter_map(|(name, _)| name.into_string().ok())
        .filter(|name| name.starts_with(&prefix));
    let environment = EnvironmentPolicy::allowlist()
        .allow(settings)
        .with("COMFYUI_BASE_URL", config.base_url.as_str());
    match &config.api_token {
        Some(token) => environment
            .with("COMFYUI_API_TOKEN", token.clone())
            .with("COMFYUI_TOKEN_HEADER", config.token_header.as_str()),
        None => environment,
    }
}

/// Runs `trainer` to the end, reading `progress_file` every `interval` while
/// it runs and sending what it holds to `progress`. A file that is missing or
/// cannot be read yet sends nothing.
async fn forwarding<T>(
    trainer: impl Future<Output = T>,
    progress_file: &Path,
    interval: Duration,
    progress: &Reporter,
) -> T {
    let mut trainer = std::pin::pin!(trainer);
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            finished = &mut trainer => return finished,
            _ = ticks.tick() => {
                if progress.listening()
                    && let Some(update) = read_progress(progress_file).await
                {
                    progress.send(update);
                }
            }
        }
    }
}

async fn read_progress(progress_file: &Path) -> Option<TrainProgress> {
    let progress_file = progress_file.to_path_buf();
    let bytes = blocking(move || fs::read(progress_file)).await.ok()?.ok()?;
    TrainProgress::parse(&bytes)
}

/// Runs the external trainer to completion within `budget`.
///
/// Its stdout goes nowhere: a trainer that prints progress would otherwise
/// fill a pipe nobody reads, or die writing to one already closed. Its stderr
/// is read as it arrives and only the last [`STDERR_TAIL`] bytes are kept, to
/// say why it failed. The trainer leads its own process group, and once it
/// exits, runs past the budget, or this future is dropped, the whole group is
/// killed: nothing the command started is left holding the GPU.
async fn run_trainer(mut process: Command, budget: Duration) -> Result<(), TrainError> {
    let (mut child, mut group) = ProcessGroup::spawn(
        process
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true),
    )
    .map_err(failed)?;
    let tail = Arc::new(Mutex::new(Vec::with_capacity(STDERR_TAIL)));
    let reader = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_tail(stderr, Arc::clone(&tail))));
    let waited = tokio::time::timeout(budget, child.wait()).await;
    group.kill_until_gone().await;
    if waited.is_err() {
        let _ = child.kill().await;
    }
    if let Some(reader) = reader {
        settle(reader).await;
    }
    let reason = crate::excerpt::tail(&String::from_utf8_lossy(
        &tail.lock().unwrap_or_else(PoisonError::into_inner),
    ));
    let status = match waited {
        Ok(status) => status.map_err(failed)?,
        Err(_) => {
            tracing::warn!(stderr = %reason, "external trainer ran past its budget and was killed");
            return Err(TrainError::Failed(format!(
                "trainer timed out after {} s",
                budget.as_secs()
            )));
        }
    };
    if status.success() {
        return Ok(());
    }
    tracing::warn!(code = ?status.code(), stderr = %reason, "external trainer failed");
    Err(TrainError::Failed(format!(
        "trainer exited {}",
        status.code().unwrap_or(1)
    )))
}

async fn read_tail(mut stderr: ChildStderr, tail: Arc<Mutex<Vec<u8>>>) {
    let mut chunk = [0u8; STDERR_CHUNK];
    while let Ok(read @ 1..) = stderr.read(&mut chunk).await {
        let mut tail = tail.lock().unwrap_or_else(PoisonError::into_inner);
        tail.extend_from_slice(&chunk[..read]);
        let excess = tail.len().saturating_sub(STDERR_TAIL);
        tail.drain(..excess);
    }
}

/// Gives the reader a moment to collect what the trainer wrote last, without
/// waiting on a pipe held open by a process that left the trainer's group.
async fn settle(mut reader: JoinHandle<()>) {
    if tokio::time::timeout(STDERR_DRAIN, &mut reader)
        .await
        .is_err()
    {
        reader.abort();
    }
}

/// Runs CPU-bound work on the blocking pool, so decoding, screening and
/// framing a large set never stalls a runtime worker.
async fn blocking<T, F>(work: F) -> Result<T, TrainError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| TrainError::Failed(error.to_string()))
}

/// Moves every upload's encoded image out of `images`, so the set is held once.
async fn decode_uploads(
    mut images: Vec<TrainImage>,
    progress: Reporter,
) -> Result<(Arc<[TrainImage]>, Vec<Vec<u8>>), TrainError> {
    blocking(move || {
        let count = images.len();
        let decoded = images
            .iter_mut()
            .enumerate()
            .map(|(index, image)| {
                let encoded = image.take_encoded()?;
                let done = index + 1;
                progress.phase(
                    Phase::Screening,
                    format!("Loading image {done} of {count}"),
                    done,
                    count,
                );
                Ok(encoded)
            })
            .collect::<Result<Vec<Vec<u8>>, TrainError>>()?;
        Ok((images.into(), decoded))
    })
    .await?
}

/// Screens `decoded` and hands it back beside the verdict, so the set moves
/// to the blocking pool and back rather than being copied.
async fn screen(
    screening: fn(&[Vec<u8>], u32) -> crate::screening::Verdict,
    decoded: Vec<Vec<u8>>,
    side: u32,
) -> Result<(Vec<Vec<u8>>, crate::screening::Verdict), TrainError> {
    blocking(move || {
        let verdict = screening(&decoded, side);
        (decoded, verdict)
    })
    .await
}

async fn decodable(bytes: bytes::Bytes) -> bool {
    blocking(move || decode::decode(&bytes).is_ok())
        .await
        .unwrap_or(false)
}

/// Crops every image in `keep` onto its subject: a decode, a pass of the
/// saliency model and a PNG encode each.
async fn frame_survivors(
    subject: Subject,
    images: Arc<[TrainImage]>,
    decoded: Vec<Vec<u8>>,
    keep: Vec<usize>,
    side: u32,
    edit: bool,
    progress: Reporter,
) -> Result<Vec<ScreenedImage>, TrainError> {
    blocking(move || {
        let count = keep.len();
        images
            .iter()
            .zip(shots(&images))
            .enumerate()
            .filter(|(index, _)| keep.binary_search(index).is_ok())
            .enumerate()
            .map(|(cropped, (original, (image, group)))| {
                let done = cropped + 1;
                progress.phase(
                    Phase::Screening,
                    format!("Cropping image {done} of {count}"),
                    done,
                    count,
                );
                let framed = frame_with_target(&subject, image, &decoded[original], side)?;
                Ok(ScreenedImage {
                    original,
                    encoded: (!edit).then(|| framed.encoded()),
                    target: framed.target,
                    reference: framed.control,
                    text: image.caption.clone(),
                    group,
                })
            })
            .collect()
    })
    .await?
}

fn validate_pairing(images: &[TrainImage], model: &TrainingModel) -> Result<(), TrainError> {
    if matches!(model, TrainingModel::QwenEdit { .. }) {
        if !images.iter().all(TrainImage::has_reference) {
            return Err(TrainError::Invalid(
                "edit training needs one reference image for every target",
            ));
        }
        if images.iter().any(|image| image.caption.trim().is_empty()) {
            return Err(TrainError::Invalid(
                "edit training needs a nonempty instruction for every image pair",
            ));
        }
    } else if images.iter().any(TrainImage::has_reference) {
        return Err(TrainError::Invalid(
            "reference images are only supported by edit training bases",
        ));
    }
    Ok(())
}

fn validate_verdict(verdict: &crate::screening::Verdict, count: usize) -> Result<(), TrainError> {
    let mut seen = vec![false; count];
    for index in verdict
        .keep
        .iter()
        .copied()
        .chain(verdict.drop.iter().map(|(index, _)| *index))
    {
        let Some(slot) = seen.get_mut(index) else {
            return Err(TrainError::Failed(
                "screening returned an invalid image index".to_string(),
            ));
        };
        if std::mem::replace(slot, true) {
            return Err(TrainError::Failed(
                "screening returned an image more than once".to_string(),
            ));
        }
    }
    if seen.iter().any(|seen| !seen)
        || verdict
            .keep
            .windows(2)
            .any(|indices| indices[0] >= indices[1])
    {
        return Err(TrainError::Failed(
            "screening did not partition the training set".to_string(),
        ));
    }
    Ok(())
}

fn repairable(rejection: crate::screening::Rejection) -> bool {
    matches!(
        rejection,
        crate::screening::Rejection::Blurred | crate::screening::Rejection::Small
    )
}

async fn remediate(
    config: &Config,
    http: &reqwest::Client,
    images: &mut [Vec<u8>],
    request: &[TrainImage],
    verdict: &crate::screening::Verdict,
    previous: &[RemediationAttempt],
) -> Vec<RemediationAttempt> {
    let mut attempts = verdict
        .drop
        .iter()
        .filter(|(index, rejection)| {
            repairable(*rejection) && !previous.iter().any(|attempt| attempt.index == *index)
        })
        .map(|(index, reason)| RemediationAttempt {
            index: *index,
            remediation: Remediation {
                source_index: *index,
                filename: request[*index].filename.clone(),
                reason: *reason,
                outcome: RemediationOutcome::Failed,
            },
            replaced: false,
        })
        .collect::<Vec<_>>();
    if attempts.is_empty() {
        return attempts;
    }

    let client = match Client::with_http(config.clone(), http.clone()) {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(error = %error, "could not initialize training image remediation");
            return attempts;
        }
    };
    let (_cancel, mut cancel) = tokio::sync::broadcast::channel(1);
    let (progress, updates) = tokio::sync::mpsc::unbounded_channel();
    drop(updates);

    for attempt in &mut attempts {
        let Some(bytes) = images.get(attempt.index).cloned() else {
            tracing::warn!("training screening returned an invalid remediation index");
            continue;
        };
        let source = match SourceImage::from_bytes(bytes) {
            Ok(source) => source,
            Err(error) => {
                tracing::warn!(error = %error, "could not prepare a training image for remediation");
                continue;
            }
        };
        let generated = match client
            .upscale_image(&source, &mut cancel, progress.clone())
            .await
        {
            Ok(generated) => generated,
            Err(error) => {
                tracing::warn!(error = %error, "could not remediate a training image");
                continue;
            }
        };
        let Some(repaired) = generated.into_iter().next() else {
            tracing::warn!("training image remediation returned no image");
            continue;
        };
        if !decodable(repaired.bytes.clone()).await {
            tracing::warn!("training image remediation returned an undecodable image");
            continue;
        }
        images[attempt.index] = repaired.bytes.to_vec();
        attempt.replaced = true;
    }
    attempts
}

fn finish_remediation(
    attempts: Vec<RemediationAttempt>,
    verdict: &crate::screening::Verdict,
) -> Vec<Remediation> {
    attempts
        .into_iter()
        .map(|mut attempt| {
            if attempt.replaced {
                attempt.remediation.outcome = if verdict.keep.binary_search(&attempt.index).is_ok()
                {
                    RemediationOutcome::Used
                } else {
                    RemediationOutcome::StillRejected
                };
            }
            attempt.remediation
        })
        .collect()
}

/// `caption`, trimmed, led by `trigger` unless it already names it as a whole
/// phrase in any case and spacing. A blank trigger leaves the caption as it
/// is, trimmed.
/// Training captions every image this way, and a prompt that binds an
/// [`Identity`](crate::inventory::Identity) is written the same way.
pub fn identity_caption(caption: &str, trigger: &str) -> String {
    let trigger = trigger.trim();
    match (!trigger.is_empty(), contains_phrase(caption, trigger)) {
        (true, false) => {
            format!("{trigger}, {}", caption.trim())
        }
        _ => caption.trim().to_string(),
    }
}

fn final_filename(name: &str) -> Result<String, TrainError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(TrainError::Invalid("invalid weight name"));
    }
    let filename = if name.ends_with(".safetensors") {
        name.to_string()
    } else {
        format!("{name}.safetensors")
    };
    let filename = sanitize_weight_filename(&filename)
        .map_err(|_| TrainError::Invalid("invalid weight name"))?;
    let path = Path::new(&filename);
    if path.file_name().and_then(|value| value.to_str()) != Some(filename.as_str())
        || path.components().count() != 1
        || filename.trim_end_matches(".safetensors").is_empty()
        || filename.chars().any(char::is_control)
    {
        return Err(TrainError::Invalid("invalid weight name"));
    }
    Ok(filename)
}

fn failed(error: std::io::Error) -> TrainError {
    TrainError::Failed(error.to_string())
}

fn require_directory(path: &Path, label: &'static str) -> Result<PathBuf, TrainError> {
    let metadata = fs::symlink_metadata(path).map_err(failed)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(TrainError::Invalid(label));
    }
    Ok(path.to_path_buf())
}

fn ensure_child_directory(parent: &Path, name: &str) -> Result<PathBuf, TrainError> {
    require_directory(parent, "training path is not a directory")?;
    let path = parent.join(name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(TrainError::Invalid("training path is not a safe directory"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&path).map_err(failed)?;
        }
        Err(error) => return Err(failed(error)),
    }
    require_confined_directory(parent, &path)?;
    Ok(path)
}

fn require_confined_directory(parent: &Path, child: &Path) -> Result<PathBuf, TrainError> {
    let root = fs::canonicalize(parent).map_err(failed)?;
    let real = fs::canonicalize(child).map_err(failed)?;
    if !real.starts_with(&root) || real == root {
        return Err(TrainError::Invalid(
            "training path escapes its configured root",
        ));
    }
    Ok(child.to_path_buf())
}

fn require_regular_file(root: &Path, path: &Path) -> Result<PathBuf, TrainError> {
    let metadata = fs::symlink_metadata(path).map_err(failed)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(TrainError::Invalid("training output is not a regular file"));
    }
    let real_root = fs::canonicalize(root).map_err(failed)?;
    let real = fs::canonicalize(path).map_err(failed)?;
    if !real.starts_with(real_root) {
        return Err(TrainError::Invalid("training output escapes its attempt"));
    }
    Ok(path.to_path_buf())
}

fn write_new(root: &Path, path: &Path, bytes: &[u8]) -> Result<(), TrainError> {
    let named = |error: std::io::Error| TrainError::Failed(format!("{}: {error}", path.display()));
    let root = fs::canonicalize(root).map_err(&named)?;
    let parent = path
        .parent()
        .ok_or(TrainError::Invalid("training path has no parent"))?;
    let parent = fs::canonicalize(parent).map_err(&named)?;
    if !parent.starts_with(&root) {
        return Err(TrainError::Invalid("training path escapes its attempt"));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(&named)?;
    file.write_all(bytes).map_err(&named)
}

fn validate_output(parent: &Path, output: &Path) -> Result<PathBuf, TrainError> {
    require_confined_directory(
        parent
            .parent()
            .ok_or(TrainError::Invalid("publication directory has no parent"))?,
        parent,
    )?;
    if output.parent() != Some(parent) {
        return Err(TrainError::Invalid("weight output escapes its directory"));
    }
    match fs::symlink_metadata(output) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(TrainError::Invalid("weight output is not a regular file"))
        }
        Ok(_) => Ok(output.to_path_buf()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(output.to_path_buf()),
        Err(error) => Err(failed(error)),
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct Publication {
    schema_version: u32,
    filename: String,
    generation: String,
    previous_weight: bool,
    previous_sidecar: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublicationPhase {
    Weights,
    Sidecar,
}

/// A durable marker hides the final name while its two files are replaced.
/// The process-local lock serializes tasks, and the file lock extends that
/// guarantee to cooperating processes that share the models directory.
fn atomic_promote(
    attempt: &Path,
    staged: &Path,
    staged_sidecar: &Path,
    parent: &Path,
    output: &Path,
    output_sidecar: &Path,
    generation: &str,
    contract: &Contract,
) -> Result<(), TrainError> {
    promote_with(
        attempt,
        staged,
        staged_sidecar,
        parent,
        output,
        output_sidecar,
        generation,
        contract,
        |_| Ok(()),
    )
}

fn promote_with<F>(
    attempt: &Path,
    staged: &Path,
    staged_sidecar: &Path,
    parent: &Path,
    output: &Path,
    output_sidecar: &Path,
    generation: &str,
    contract: &Contract,
    mut observe: F,
) -> Result<(), TrainError>
where
    F: FnMut(PublicationPhase) -> Result<(), TrainError>,
{
    Uuid::parse_str(generation)
        .map_err(|_| TrainError::Invalid("training attempt id is not valid"))?;
    require_regular_file(attempt, staged)?;
    require_regular_file(attempt, staged_sidecar)?;
    if read_generation(staged_sidecar).as_deref() != Some(generation) {
        return Err(TrainError::Invalid(
            "staged weight sidecar does not match its training attempt",
        ));
    }
    validate_output(parent, output)?;
    validate_output(parent, output_sidecar)?;
    sync_file(staged)?;
    sync_file(staged_sidecar)?;

    let key = fs::canonicalize(parent).map_err(failed)?.join(
        output
            .file_name()
            .ok_or(TrainError::Invalid("weight output has no filename"))?,
    );
    let local = publication_lock(&key);
    let _local = local
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let publications = ensure_child_directory(parent, &contract.publication_directory)?;
    sync_directory(parent)?;
    let filename = output
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(TrainError::Invalid("weight output has no filename"))?;
    let _file = lock_publication(&publications, filename)?;
    let marker = publication_marker(parent, filename, contract)
        .ok_or(TrainError::Invalid("invalid weight publication name"))?;
    recover_publication(parent, output, output_sidecar, &marker, contract)?;
    validate_output(parent, output)?;
    validate_output(parent, output_sidecar)?;
    require_training_output(output, contract)?;

    let previous_weight = snapshot(output, &attempt.join(PREVIOUS_WEIGHT))?;
    let previous_sidecar = snapshot(
        output_sidecar,
        &sidecar_path(&attempt.join(PREVIOUS_WEIGHT), contract),
    )?;
    sync_directory(attempt)?;
    let publication = Publication {
        schema_version: 1,
        filename: filename.to_string(),
        generation: generation.to_string(),
        previous_weight,
        previous_sidecar,
    };
    let encoded =
        serde_json::to_vec(&publication).map_err(|error| TrainError::Failed(error.to_string()))?;
    write_new(&publications, &marker, &encoded)?;
    sync_file(&marker)?;
    sync_directory(&publications)?;

    let promoted = (|| {
        fs::rename(staged, output).map_err(failed)?;
        sync_directory(parent)?;
        observe(PublicationPhase::Weights)?;
        fs::rename(staged_sidecar, output_sidecar).map_err(failed)?;
        sync_directory(parent)?;
        observe(PublicationPhase::Sidecar)?;
        Ok::<(), TrainError>(())
    })();
    if let Err(error) = promoted {
        return rollback_publication(
            attempt,
            parent,
            output,
            output_sidecar,
            &marker,
            contract,
            error,
        );
    }
    if let Err(error) = fs::remove_file(&marker) {
        return rollback_publication(
            attempt,
            parent,
            output,
            output_sidecar,
            &marker,
            contract,
            failed(error),
        );
    }
    // Both renames are durable; a restart that still sees the marker completes them.
    let _ = sync_directory(&publications);
    Ok(())
}

fn publication_lock(key: &Path) -> Arc<Mutex<()>> {
    let mut publications = PUBLICATIONS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    publications.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = publications.get(key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    publications.insert(key.to_path_buf(), Arc::downgrade(&lock));
    lock
}

fn lock_publication(directory: &Path, filename: &str) -> Result<fs::File, TrainError> {
    let filename = sanitize_weight_filename(filename)
        .map_err(|_| TrainError::Invalid("invalid weight publication name"))?;
    let path = directory.join(format!("{filename}.lock"));
    let file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(&path).map_err(failed)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(TrainError::Invalid(
                    "weight publication lock is not a regular file",
                ));
            }
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .map_err(failed)?
        }
        Err(error) => return Err(failed(error)),
    };
    file.lock().map_err(failed)?;
    Ok(file)
}

fn snapshot(source: &Path, backup: &Path) -> Result<bool, TrainError> {
    match fs::symlink_metadata(source) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            TrainError::Invalid("weight publication target is not a regular file"),
        ),
        Ok(_) => {
            fs::hard_link(source, backup).map_err(failed)?;
            sync_file(backup)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(failed(error)),
    }
}

fn recover_publication(
    parent: &Path,
    output: &Path,
    output_sidecar: &Path,
    marker: &Path,
    contract: &Contract,
) -> Result<(), TrainError> {
    match fs::symlink_metadata(marker) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(TrainError::Invalid(
                "weight publication marker is not a regular file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(failed(error)),
    }
    let contents = match fs::read(marker) {
        Ok(contents) => contents,
        Err(error) => return Err(failed(error)),
    };
    let publication: Publication = serde_json::from_slice(&contents)
        .map_err(|_| TrainError::Invalid("weight publication marker is invalid"))?;
    if publication.schema_version != 1
        || output.file_name().and_then(|name| name.to_str()) != Some(publication.filename.as_str())
        || Uuid::parse_str(&publication.generation).is_err()
    {
        return Err(TrainError::Invalid("weight publication marker is invalid"));
    }
    let complete = require_regular_output(parent, output).is_ok()
        && require_regular_output(parent, output_sidecar).is_ok()
        && read_generation(output_sidecar).as_deref() == Some(publication.generation.as_str());
    if complete {
        fs::remove_file(marker).map_err(failed)?;
        let publications = marker.parent().ok_or(TrainError::Invalid(
            "weight publication marker has no parent",
        ))?;
        let _ = sync_directory(publications);
        return Ok(());
    }
    let models = parent
        .parent()
        .ok_or(TrainError::Invalid("publication directory has no parent"))?;
    let training = require_directory(&models.join("training"), "training directory")?;
    let attempt = require_confined_directory(&training, &training.join(&publication.generation))?;
    restore_snapshot(
        &attempt,
        &attempt.join(PREVIOUS_WEIGHT),
        output,
        publication.previous_weight,
    )?;
    restore_snapshot(
        &attempt,
        &sidecar_path(&attempt.join(PREVIOUS_WEIGHT), contract),
        output_sidecar,
        publication.previous_sidecar,
    )?;
    sync_directory(parent)?;
    fs::remove_file(marker).map_err(failed)?;
    let publications = marker.parent().ok_or(TrainError::Invalid(
        "weight publication marker has no parent",
    ))?;
    let _ = sync_directory(publications);
    Ok(())
}

fn rollback_publication(
    attempt: &Path,
    parent: &Path,
    output: &Path,
    output_sidecar: &Path,
    marker: &Path,
    contract: &Contract,
    error: TrainError,
) -> Result<(), TrainError> {
    let publication = fs::read(marker).map_err(failed).and_then(|contents| {
        serde_json::from_slice::<Publication>(&contents)
            .map_err(|parse| TrainError::Failed(parse.to_string()))
    });
    let restored = publication.and_then(|publication| {
        restore_snapshot(
            attempt,
            &attempt.join(PREVIOUS_WEIGHT),
            output,
            publication.previous_weight,
        )?;
        restore_snapshot(
            attempt,
            &sidecar_path(&attempt.join(PREVIOUS_WEIGHT), contract),
            output_sidecar,
            publication.previous_sidecar,
        )?;
        sync_directory(parent)?;
        fs::remove_file(marker).map_err(failed)?;
        let publications = marker.parent().ok_or(TrainError::Invalid(
            "weight publication marker has no parent",
        ))?;
        let _ = sync_directory(publications);
        Ok(())
    });
    match restored {
        Ok(()) => Err(error),
        Err(restore) => Err(TrainError::Failed(format!(
            "{error}; adapter rollback failed: {restore}"
        ))),
    }
}

fn restore_snapshot(
    attempt: &Path,
    backup: &Path,
    output: &Path,
    existed: bool,
) -> Result<(), TrainError> {
    if existed {
        require_regular_file(attempt, backup)?;
        let backup_name = backup
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(TrainError::Invalid("weight backup has no filename"))?;
        let restore = attempt.join(format!("restore-{backup_name}"));
        match fs::symlink_metadata(&restore) {
            Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
                fs::remove_file(&restore).map_err(failed)?;
            }
            Ok(_) => {
                return Err(TrainError::Invalid(
                    "weight restore path is not a regular file",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(failed(error)),
        }
        fs::hard_link(backup, &restore).map_err(failed)?;
        sync_file(&restore)?;
        sync_directory(attempt)?;
        fs::rename(restore, output).map_err(failed)
    } else {
        match fs::remove_file(output) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(failed(error)),
        }
    }
}

fn require_regular_output(parent: &Path, path: &Path) -> Result<PathBuf, TrainError> {
    validate_output(parent, path)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            TrainError::Invalid("weight publication target is not a regular file"),
        ),
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) => Err(failed(error)),
    }
}

fn read_generation(path: &Path) -> Option<String> {
    let contents = fs::read(path).ok()?;
    serde_json::from_slice::<WeightDocument>(&contents)
        .ok()?
        .generation
}

fn sync_file(path: &Path) -> Result<(), TrainError> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(failed)
}

fn sync_directory(path: &Path) -> Result<(), TrainError> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(failed)
}

fn remove_entry(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        let _ = fs::remove_dir_all(path);
    } else {
        let _ = fs::remove_file(path);
    }
}

fn safe_directory(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
}

/// The shot each image belongs to. Frames from a clip say which shot they came
/// from; a separate photo is its own, numbered past every clip's groups so the
/// two cannot be taken for each other.
fn shots(images: &[TrainImage]) -> Vec<usize> {
    // Renumbered densely: counting up from a caller's largest group overflows.
    let mut clips: Vec<usize> = Vec::new();
    let seen: Vec<Option<usize>> = images
        .iter()
        .map(|image| {
            image.group.map(|group| {
                clips
                    .iter()
                    .position(|&known| known == group)
                    .unwrap_or_else(|| {
                        clips.push(group);
                        clips.len() - 1
                    })
            })
        })
        .collect();
    let mut next = clips.len();
    seen.into_iter()
        .map(|shot| {
            shot.unwrap_or_else(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

/// The filename a crop is captioned under. Only its extension is read, to pick
/// the MIME type of the data URL a vision model is handed.
const CROP: &str = "crop.png";

/// One upload as the dataset will hold it: a square PNG framed on its subject,
/// and the control image that has to keep answering it.
#[derive(Debug)]
struct Framed {
    target: Vec<u8>,
    control: Option<Vec<u8>>,
}

impl Framed {
    fn encoded(&self) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(&self.target)
    }
}

/// Crops one upload square onto its subject.
///
/// Cropping here rather than leaving it to the loader is what keeps the subject
/// in the dataset: the loader fits whatever it is given onto a white square, so
/// an uncropped photo trains on its own letterboxing and on however much
/// background the photographer happened to include.
#[cfg(test)]
fn frame(subject: &Subject, image: &TrainImage, side: u32) -> Result<Framed, TrainError> {
    let target = image.encoded()?;
    frame_with_target(subject, image, &target, side)
}

fn frame_with_target(
    subject: &Subject,
    image: &TrainImage,
    target: &[u8],
    side: u32,
) -> Result<Framed, TrainError> {
    let raster = decode_bytes(target)?;
    let focus = subject.focus(&raster, CENTRE);
    let control = match image.encoded_reference()? {
        // Cropped on the target's focus, so the pair stays aligned pixel for pixel.
        Some(before) => Some(square(subject, &decode_bytes(&before)?, side, focus)?),
        None => None,
    };
    Ok(Framed {
        target: square(subject, &raster, side, focus)?,
        control,
    })
}

fn square(
    subject: &Subject,
    raster: &Raster,
    side: u32,
    focus: Point,
) -> Result<Vec<u8>, TrainError> {
    png(&subject
        .render(raster, side, focus)
        .map_err(|error| TrainError::Failed(error.to_string()))?)
}

/// Decoding is also what applies a photo's EXIF rotation: a sideways image
/// otherwise trains a sideways subject.
fn decode_bytes(bytes: &[u8]) -> Result<Raster, TrainError> {
    decode::decode(bytes)
        .map_err(|_| TrainError::Invalid("training images must be PNG, JPEG, or WebP"))
}

/// Encodes a rendered RGB image as PNG.
pub(crate) fn png(rendered: &Rendered) -> Result<Vec<u8>, TrainError> {
    use image::ImageEncoder;
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(
            &rendered.pixels,
            rendered.width,
            rendered.height,
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|error| TrainError::Failed(error.to_string()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::screening::Rejection;
    use crate::screening::Verdict;
    use crate::train::ARTIFACT_PREFIX;
    use crate::train::ENVIRONMENT_PREFIX;
    use crate::train::FOLDER_PREFIX;
    use crate::train::INPUT_ENVIRONMENT_PREFIX;
    use crate::train::PROBE_LOSS_NODE;
    use crate::train::PUBLICATION_DIRECTORY;
    use crate::train::SIDECAR_SUFFIX;
    use crate::train::TRAIN_LORA_NODE;
    use base64::Engine;
    use serde_json::Value;
    use serde_json::json;
    use std::sync::atomic::AtomicBool;
    use std::sync::atomic::Ordering;
    use std::thread::ThreadId;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::Request;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::path_regex;

    /// The trainer's variables under a prefix of the test's own, so every
    /// script below also proves the prefix is configurable.
    fn contract() -> Contract {
        Contract {
            environment_prefix: "TRAIN".into(),
            input_environment_prefix: "COMFY".into(),
            ..Contract::default()
        }
    }

    fn harness(command: &str) -> (tempfile::TempDir, Config) {
        let root = tempfile::tempdir().expect("temporary ComfyUI root");
        let models = root.path().join("models");
        fs::create_dir(&models).expect("models directory");
        let config = Config {
            base_url: "http://127.0.0.1:9".to_string(),
            models_directory: models,
            train_command: Some(command.to_string()),
            train_timeout: Duration::from_secs(2),
            contract: contract(),
            ..Default::default()
        };
        (root, config)
    }

    /// A real image whose colour follows its name, so a crop can still be
    /// traced back to the upload it was made from.
    fn image(target: &str, caption: &str, reference: Option<&str>) -> TrainImage {
        let image = TrainImage::new(format!("{target}.png"), caption, encoded(colour(target)));
        match reference {
            Some(reference) => image.with_before_base64(encoded(colour(reference))),
            None => image,
        }
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
            ResponseTemplate::new(200).set_body_json(json!({
                "name": filename,
                "subfolder": field("subfolder"),
                "type": field("type"),
            }))
        }
    }

    /// ComfyUI names the artifact it is serving, and the download checks that
    /// the file it gets back is the one it asked for.
    struct ServeArtifact(Vec<u8>);

    impl wiremock::Respond for ServeArtifact {
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
                .insert_header("content-type", "application/octet-stream")
                .set_body_bytes(self.0.clone())
        }
    }

    fn colour(name: &str) -> [u8; 3] {
        let hash = name.bytes().fold(17u32, |hash, byte| {
            hash.wrapping_mul(31).wrapping_add(u32::from(byte))
        });
        [hash as u8, (hash >> 8) as u8, (hash >> 16) as u8]
    }

    fn identity(name: &str) -> TrainRequest {
        TrainRequest::new(
            name,
            "flux-schnell",
            vec![image("target", "a portrait", None)],
        )
        .with_trigger("ohwx")
    }

    fn edit(images: Vec<TrainImage>) -> TrainRequest {
        TrainRequest::new("edit-style", "qwen-image-edit", images)
    }

    fn keep_all(images: &[Vec<u8>], _resolution: u32) -> Verdict {
        Verdict {
            keep: (0..images.len()).collect(),
            drop: Vec::new(),
        }
    }

    fn drop_middle(_images: &[Vec<u8>], _resolution: u32) -> Verdict {
        Verdict {
            keep: vec![0, 2],
            drop: vec![(1, Rejection::Duplicate)],
        }
    }

    fn reject_tiny(images: &[Vec<u8>], _resolution: u32) -> Verdict {
        let mut keep = Vec::new();
        let mut drop = Vec::new();
        for (index, image) in images.iter().enumerate() {
            let (width, height) = abnegate_vision::decode::decode(image)
                .expect("test images are decodable")
                .oriented_size();
            if width.min(height) < 2 {
                drop.push((index, Rejection::Small));
            } else {
                keep.push(index);
            }
        }
        Verdict { keep, drop }
    }

    fn training_entries(config: &Config) -> Vec<PathBuf> {
        let training = config.models_directory.join("training");
        let Ok(entries) = fs::read_dir(training) else {
            return Vec::new();
        };
        entries.flatten().map(|entry| entry.path()).collect()
    }

    fn staged_adapter(
        config: &Config,
        generation: &str,
        contents: &[u8],
    ) -> (PathBuf, PathBuf, PathBuf) {
        let attempt = config.models_directory.join("training").join(generation);
        fs::create_dir_all(&attempt).unwrap();
        let staged = attempt.join(format!("{generation}.safetensors"));
        let sidecar = sidecar_path(&staged, &config.contract);
        fs::write(&staged, contents).unwrap();
        fs::write(
            &sidecar,
            serde_json::to_vec(&WeightDocument {
                sidecar: WeightSidecar::new("flux-schnell-adapter")
                    .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
                generation: Some(generation.to_string()),
            })
            .unwrap(),
        )
        .unwrap();
        (attempt, staged, sidecar)
    }

    /// A real one-pixel image, so the dataset writer has something to decode.
    fn encoded(colour: [u8; 3]) -> String {
        base64::engine::general_purpose::STANDARD
            .encode(png(&Rendered::new(1, 1, colour.to_vec())).unwrap())
    }

    /// A flat image of the given size, encoded the way an upload arrives.
    fn encoded_at(width: u32, height: u32) -> String {
        base64::engine::general_purpose::STANDARD.encode(
            png(&Rendered::new(
                width,
                height,
                (0..width * height)
                    .flat_map(|index| [(index % 251) as u8, 40, 90])
                    .collect(),
            ))
            .unwrap(),
        )
    }

    fn upload(caption: &str, group: Option<usize>) -> TrainImage {
        let image = TrainImage::new("a.png", caption, encoded([12, 34, 56]));
        match group {
            Some(group) => image.with_group(group),
            None => image,
        }
    }

    #[tokio::test]
    async fn train_writes_adapter_with_configured_command() {
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        let outcome = train(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("my-style"),
        )
        .await
        .unwrap();
        assert_eq!(outcome.path.file_name().unwrap(), "my-style.safetensors");
        assert_eq!(fs::read(&outcome.path).unwrap(), b"lora");
        assert!(
            outcome.quality.is_none(),
            "a run with no reachable probe reports no score instead of failing"
        );
        assert!(training_entries(&config).is_empty());
    }

    #[tokio::test]
    async fn train_writes_the_trigger_on_the_sidecar() {
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        let outcome = train(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("my-style"),
        )
        .await
        .unwrap();
        let document: WeightDocument = serde_json::from_slice(
            &fs::read(sidecar_path(&outcome.path, &config.contract)).unwrap(),
        )
        .unwrap();
        assert_eq!(document.sidecar.trigger.as_deref(), Some("ohwx"));
        assert_eq!(document.sidecar.recipe_id, "flux-schnell-adapter");

        let checkpoints = config.models_directory.join("checkpoints");
        fs::create_dir_all(&checkpoints).unwrap();
        fs::write(checkpoints.join("flux1-schnell-fp8.safetensors"), b"base").unwrap();
        let mut bound = config.clone();
        let identity = crate::inventory::bind_identity(&mut bound, "ohwx on a bicycle")
            .expect("a trained adapter over an installed base is an identity");
        assert_eq!(identity.filename, "my-style.safetensors");
        assert_eq!(identity.trigger, "ohwx");
        assert_eq!(bound.checkpoint, "my-style.safetensors");
    }

    #[tokio::test]
    async fn training_reports_each_phase_before_the_trainer_starts() {
        let (_root, mut config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        config.caption_model = "vision".to_string();
        let captioner = wiremock::MockServer::start().await;
        let (reporter, published) = Reporter::recording();
        train_with_remediation(
            &config,
            captioner.uri(),
            SecretValue::new("key"),
            TrainRequest::new(
                "reported-style",
                "flux-schnell",
                vec![image("target", "", None)],
            )
            .with_trigger("ohwx"),
            crate::screening::screen,
            reporter,
        )
        .await
        .unwrap();
        let updates = published.lock().unwrap().clone();
        let mut phases = updates
            .iter()
            .map(|update| update.phase.clone().expect("every update names its phase"))
            .collect::<Vec<Phase>>();
        phases.dedup();
        assert_eq!(
            phases,
            vec![
                Phase::Screening,
                Phase::Captioning,
                Phase::Training,
                Phase::Publishing
            ]
        );
        let budget = crate::train::packaged_config().unwrap().steps(1);
        assert!(
            updates.iter().all(|update| update.total == budget
                && update.step
                    == match update.phase {
                        Some(Phase::Publishing) => budget,
                        _ => 0,
                    }),
            "the budget is the one the trainer gets, and only publishing follows training: {updates:?}"
        );
        assert!(
            updates.windows(2).all(|pair| pair[0] != pair[1]),
            "an update was published twice in a row: {updates:?}"
        );
        let said = |message: &str| {
            updates
                .iter()
                .find(|update| update.message.as_deref() == Some(message))
                .unwrap_or_else(|| panic!("{message:?} was never reported in {updates:?}"))
                .percent
        };
        assert_eq!(said("Screening the dataset"), Some(0));
        assert_eq!(said("Loading image 1 of 1"), Some(100));
        assert_eq!(said("Checking the images"), Some(0));
        assert_eq!(said("Cropping image 1 of 1"), Some(100));
        assert_eq!(said("Captioning the dataset"), Some(0));
        assert_eq!(said("Captioning image 1 of 1"), Some(100));
        assert_eq!(said("Starting training"), Some(0));
        assert_eq!(said("Choosing the best checkpoint"), Some(0));
        assert_eq!(said("Publishing the weight"), Some(50));
        assert!(
            !updates.iter().any(|update| update
                .message
                .as_deref()
                .is_some_and(|message| message.starts_with("Captioning image 0"))),
            "captioning reported an image before describing one: {updates:?}"
        );
        assert_eq!(
            updates.last().unwrap().message.as_deref(),
            Some("Publishing the weight")
        );
    }

    #[tokio::test]
    async fn a_training_command_reports_what_it_writes_to_its_progress_file() {
        let command = r#"
            test "$TRAIN_STEPS" = "BUDGET" || exit 41
            case "$TRAIN_PROGRESS" in "$TRAIN_DIRECTORY"/*) ;; *) exit 42 ;; esac
            printf '{"step":5,"total":10,"phase":"training","loss":0.5}' > "$TRAIN_PROGRESS"
            sleep 1
            printf lora > "$TRAIN_OUTPUT"
        "#
        .replace(
            "BUDGET",
            &crate::train::packaged_config()
                .unwrap()
                .steps(1)
                .to_string(),
        );
        let (_root, mut config) = harness(&command);
        config.poll_interval = Duration::from_millis(50);
        let (reporter, published) = Reporter::recording();
        train_with_remediation(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("command-progress"),
            crate::screening::screen,
            reporter,
        )
        .await
        .expect("the command trains");
        let updates = published.lock().unwrap().clone();
        assert!(
            updates.contains(
                &TrainProgress::new(5, 10)
                    .with_phase(Phase::Training)
                    .with_loss(0.5)
            ),
            "the command's progress file never arrived: {updates:?}"
        );
        assert_eq!(
            updates.last(),
            Some(
                &TrainProgress::new(10, 10)
                    .with_phase(Phase::Publishing)
                    .with_message("Publishing the weight")
                    .with_percent(50)
            ),
            "publishing counts the steps the command reported"
        );
    }

    #[tokio::test]
    async fn training_finishes_as_a_background_job_once_its_watcher_leaves() {
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        let (sender, receiver) = tokio::sync::watch::channel(None);
        drop(receiver);
        let job = tokio::spawn(async move {
            train_reporting(
                &config,
                String::new(),
                SecretValue::new(""),
                identity("background-style"),
                sender,
            )
            .await
        });
        let outcome = job
            .await
            .expect("the job runs to the end")
            .expect("no one watching is not a failure");
        assert_eq!(fs::read(&outcome.path).unwrap(), b"lora");
    }

    #[tokio::test]
    async fn a_watcher_reads_the_latest_progress_of_a_run() {
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        let (sender, receiver) = tokio::sync::watch::channel(None);
        train_reporting(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("watched-style"),
            sender,
        )
        .await
        .unwrap();
        let latest = receiver
            .borrow()
            .clone()
            .expect("the run published progress");
        assert_eq!(latest.phase, Some(Phase::Publishing));
        assert_eq!(latest.message.as_deref(), Some("Publishing the weight"));
    }

    #[tokio::test]
    async fn the_default_trainer_variables_are_the_ones_the_packaged_script_reads() {
        let command = format!(
            "test -n \"${INPUT_ENVIRONMENT_PREFIX}_INPUT\" || exit 51; \
             printf lora > \"${ENVIRONMENT_PREFIX}_OUTPUT\""
        );
        let (_root, mut config) = harness(&command);
        config.contract = Contract::default();
        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("default-prefix"),
            keep_all,
        )
        .await
        .expect("the default contract names the variables the command reads");
        assert_eq!(fs::read(outcome.path).unwrap(), b"lora");
    }

    #[tokio::test]
    async fn a_renamed_prefix_replaces_the_default_variables_rather_than_adding_to_them() {
        let command = format!(
            "test -z \"${ENVIRONMENT_PREFIX}_OUTPUT\" || exit 52; \
             test -n \"$COMFY_INPUT\" || exit 53; \
             printf lora > \"$TRAIN_OUTPUT\""
        );
        let (_root, config) = harness(&command);
        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("renamed-prefix"),
            keep_all,
        )
        .await
        .expect("the trainer is only told the configured names");
        assert_eq!(fs::read(outcome.path).unwrap(), b"lora");
    }

    #[tokio::test]
    async fn external_trainer_keeps_the_documented_name_inside_an_isolated_attempt() {
        let command = r#"
            test "$TRAIN_NAME" = "legacy-style.safetensors" || exit 11
            test "$TRAIN_FINAL_NAME" = "$TRAIN_NAME" || exit 12
            test "$TRAIN_ATTEMPT" = "$(basename "$TRAIN_DIRECTORY")" || exit 13
            test "$(basename "$TRAIN_OUTPUT")" = "$TRAIN_ATTEMPT.safetensors" || exit 14
            case "$TRAIN_OUTPUT" in "$TRAIN_DIRECTORY"/*) ;; *) exit 15 ;; esac
            printf legacy > "$TRAIN_OUTPUT"
        "#;
        let (_root, config) = harness(command);

        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("legacy-style"),
            keep_all,
        )
        .await
        .expect("compatible external training command");

        assert_eq!(fs::read(outcome.path).unwrap(), b"legacy");
        assert!(training_entries(&config).is_empty());
    }

    #[tokio::test]
    async fn the_dataset_directory_is_handed_over_under_its_spelled_out_name() {
        const NAME: &str =
            "lora::tests::the_dataset_directory_is_handed_over_under_its_spelled_out_name";
        if crate::child::delegated(NAME, &[]).await {
            return;
        }
        let command = r#"
            test -d "$TRAIN_DIRECTORY/targets" || exit 41
            test "$(env | grep "^TRAIN_DI" | cut -d= -f1)" = TRAIN_DIRECTORY || exit 42
            printf lora > "$TRAIN_OUTPUT"
        "#;
        let (_root, config) = harness(command);

        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("directory-name"),
            keep_all,
        )
        .await
        .expect("the trainer finds its dataset under <prefix>_DIRECTORY alone");

        assert_eq!(fs::read(outcome.path).unwrap(), b"lora");
    }

    async fn trained_by(config: &Config) {
        let outcome = train_with_screening(
            config,
            String::new(),
            SecretValue::new(""),
            identity("environment"),
            keep_all,
        )
        .await
        .expect("the trainer saw the environment it expected");
        assert_eq!(fs::read(outcome.path).unwrap(), b"lora");
    }

    #[tokio::test]
    async fn a_variable_only_the_host_has_never_reaches_the_trainer() {
        const NAME: &str = "lora::tests::a_variable_only_the_host_has_never_reaches_the_trainer";
        if crate::child::delegated(
            NAME,
            &[
                ("ABNEGATE_COMFY_TEST_HOST_ONLY", "host-value"),
                ("COMFYUI_API_TOKEN", "host-token"),
                ("ABNEGATE_TRAIN_STEPS", "9"),
            ],
        )
        .await
        {
            return;
        }
        let command = r#"
            test -z "${ABNEGATE_COMFY_TEST_HOST_ONLY+set}" || exit 71
            test -z "${COMFYUI_API_TOKEN+set}" || exit 72
            test -z "${COMFYUI_TOKEN_HEADER+set}" || exit 73
            test -z "${ABNEGATE_TRAIN_STEPS+set}" || exit 74
            test -n "$PATH" || exit 75
            printf lora > "$TRAIN_OUTPUT"
        "#;
        let (_root, config) = harness(command);

        trained_by(&config).await;
    }

    #[tokio::test]
    async fn the_trainer_keeps_the_hosts_own_settings_under_its_prefix() {
        const NAME: &str = "lora::tests::the_trainer_keeps_the_hosts_own_settings_under_its_prefix";
        if crate::child::delegated(
            NAME,
            &[("TRAIN_STEPS", "2000"), ("TRAIN_OUTPUT", "/host/elsewhere")],
        )
        .await
        {
            return;
        }
        let command = r#"
            test "$TRAIN_STEPS" = 2000 || exit 81
            test "$TRAIN_OUTPUT" != /host/elsewhere || exit 82
            printf lora > "$TRAIN_OUTPUT"
        "#;
        let (_root, config) = harness(command);

        trained_by(&config).await;
    }

    #[tokio::test]
    async fn a_command_run_reports_the_step_budget_the_host_gave_its_trainer() {
        const NAME: &str =
            "lora::tests::a_command_run_reports_the_step_budget_the_host_gave_its_trainer";
        if crate::child::delegated(NAME, &[("TRAIN_STEPS", "2000")]).await {
            return;
        }
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        let (reporter, published) = Reporter::recording();
        train_with_remediation(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("host-budget"),
            crate::screening::screen,
            reporter,
        )
        .await
        .expect("the command trains");
        let started = published
            .lock()
            .unwrap()
            .iter()
            .find(|update| update.message.as_deref() == Some("Starting training"))
            .cloned()
            .expect("training was reported");
        assert_eq!(started.total, 2000);
    }

    #[tokio::test]
    async fn the_trainer_is_handed_the_comfyui_token_and_the_header_it_travels_in() {
        const NAME: &str =
            "lora::tests::the_trainer_is_handed_the_comfyui_token_and_the_header_it_travels_in";
        if crate::child::delegated(NAME, &[]).await {
            return;
        }
        assert!(
            std::env::var_os("COMFYUI_API_TOKEN").is_none(),
            "the host must not hold the token, or inheriting it would pass"
        );
        let command = r#"
            test "$COMFYUI_API_TOKEN" = t || exit 91
            test "$COMFYUI_TOKEN_HEADER" = X-Proxy-Token || exit 92
            printf lora > "$TRAIN_OUTPUT"
        "#;
        let (_root, config) = harness(command);
        let config = Config {
            api_token: Some(SecretValue::new("t")),
            token_header: "X-Proxy-Token".into(),
            ..config
        };

        trained_by(&config).await;
    }

    /// A models root with the output directory the writer needs.
    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("lora-root-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("loras")).unwrap();
        root
    }

    fn request(name: &str, base: &str, trigger: Option<&str>) -> TrainRequest {
        let request = TrainRequest::new(name, base, vec![upload("a portrait", None)]);
        match trigger {
            Some(trigger) => request.with_trigger(trigger),
            None => request,
        }
    }

    async fn rejected(config: &Config, request: TrainRequest) -> TrainError {
        train(config, String::new(), SecretValue::new(""), request)
            .await
            .expect_err("this request should not have trained")
    }

    #[tokio::test]
    async fn training_needs_either_a_command_or_a_reachable_comfyui() {
        let error = rejected(
            &Config::default(),
            request("my-style", "flux-schnell", Some("ohwx")),
        )
        .await;
        assert!(matches!(error, TrainError::Disabled), "{error}");
    }

    /// The LoRA name reaches `create_dir_all` and `fs::write`, so it is the one
    /// field of a training request that could write outside the models
    /// directory. Nothing here may be accepted, and nothing may be created.
    #[tokio::test]
    async fn a_name_that_is_not_a_filename_is_refused() {
        let root = root();
        let outside = root
            .join("..")
            .join(format!("escaped-{}", uuid::Uuid::new_v4()));
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("true".into()),
            ..Default::default()
        };
        for name in [
            "",
            "   ",
            ".",
            "..",
            "../escape",
            "../../escape",
            "a/../../escape",
            "/etc/passwd",
            "/absolute",
            "back\\slash",
            "nested/name",
            &outside.display().to_string(),
            &"a".repeat(300),
        ] {
            let error = rejected(&config, request(name, "flux-schnell", Some("ohwx"))).await;
            assert!(
                matches!(error, TrainError::Invalid(_)),
                "{name:?} was not refused: {error}"
            );
        }

        let training = root.join("training");
        assert!(
            !training.exists() || fs::read_dir(&training).unwrap().count() == 0,
            "a refused name must not create a dataset directory"
        );
        assert!(
            !outside.exists(),
            "a refused name reached outside the models directory"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_configuration_that_would_poll_in_a_busy_loop_is_refused() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("true".into()),
            poll_interval: Duration::ZERO,
            ..Default::default()
        };
        let error = rejected(&config, request("my-style", "flux-schnell", Some("ohwx"))).await;
        assert!(matches!(error, TrainError::Configuration(_)), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_contract_that_names_a_path_outside_comfyui_is_refused() {
        let (_root, mut config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        config.contract.artifact_prefix = "../escape-".into();
        let error = rejected(&config, identity("escaping")).await;
        assert!(matches!(error, TrainError::Configuration(_)), "{error}");
        assert!(training_entries(&config).is_empty());
    }

    #[tokio::test]
    async fn training_needs_images() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("true".into()),
            ..Default::default()
        };
        let mut empty = request("my-style", "flux-schnell", Some("ohwx"));
        empty.images.clear();
        let error = rejected(&config, empty).await;
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_blank_trigger_counts_as_no_trigger() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("true".into()),
            ..Default::default()
        };
        let error = rejected(&config, request("my-style", "flux-schnell", Some("   "))).await;
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn an_unknown_base_is_refused_before_anything_is_written() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("true".into()),
            ..Default::default()
        };
        let error = rejected(&config, request("my-style", "no-such-base", Some("ohwx"))).await;
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
        let training = root.join("training");
        assert!(
            !training.exists() || fs::read_dir(&training).unwrap().count() == 0,
            "a refused request should leave no dataset behind"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_trainer_that_exits_badly_is_reported() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("exit 3".into()),
            ..Default::default()
        };
        let error = rejected(&config, request("my-style", "flux-schnell", Some("ohwx"))).await;
        let TrainError::Failed(message) = &error else {
            panic!("expected a failure, got {error}");
        };
        assert!(
            message.contains('3'),
            "the exit code is the diagnosis: {message}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_trainer_that_prints_progress_before_its_weights_still_trains() {
        let command = r#"
            i=0
            while [ $i -lt 2000 ]; do
                echo "step $i of 2000: loss 0.0123456789, learning rate 0.0001"
                echo "warning $i" >&2
                i=$((i + 1))
            done
            printf lora > "$TRAIN_OUTPUT"
        "#;
        let (_root, config) = harness(command);
        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("chatty"),
            keep_all,
        )
        .await
        .expect("a trainer's output must not decide whether it can finish");
        assert_eq!(fs::read(outcome.path).unwrap(), b"lora");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_trainer_that_outlives_its_budget_is_killed() {
        let marker = tempfile::tempdir().unwrap();
        let started = marker.path().join("started");
        let finished = marker.path().join("finished");
        let command = format!(
            "touch \"{}\"; sleep 2; touch \"{}\"",
            started.display(),
            finished.display()
        );
        let (_root, mut config) = harness(&command);
        config.train_timeout = Duration::from_secs(1);

        let error = rejected(&config, identity("slow")).await;
        let returned = std::time::SystemTime::now();

        assert!(
            matches!(&error, TrainError::Failed(message) if message.contains("timed out")),
            "{error}"
        );
        let spawned = std::fs::metadata(&started)
            .and_then(|metadata| metadata.modified())
            .expect("the trainer started");
        let ran = returned
            .duration_since(spawned)
            .expect("the trainer started before the run returned");
        assert!(ran < Duration::from_secs(2), "the budget was not enforced");
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        assert!(
            !finished.exists(),
            "the trainer kept running past its budget"
        );
        assert!(training_entries(&config).is_empty());
    }

    /// A `sleep` for a duration no other process on the machine is using, and
    /// the pattern that finds it, and only it, by its command line.
    #[cfg(unix)]
    fn marked_sleep() -> (String, String) {
        let fraction = uuid::Uuid::new_v4().as_u128() % 1_000_000_000;
        (
            format!("sleep 30.{fraction:09}"),
            format!("^sleep 30\\.{fraction:09}$"),
        )
    }

    #[cfg(unix)]
    async fn running(pattern: &str) -> bool {
        tokio::process::Command::new("pgrep")
            .arg("-f")
            .arg(pattern)
            .stdout(Stdio::null())
            .status()
            .await
            .expect("pgrep runs")
            .success()
    }

    /// Whether the process `pattern` matches reaches the `expected` state
    /// within a few seconds. A process still running when it gives up is
    /// killed, so a failing test leaves nothing behind.
    #[cfg(unix)]
    async fn settles(pattern: &str, expected: bool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while running(pattern).await != expected {
            if std::time::Instant::now() >= deadline {
                let _ = tokio::process::Command::new("pkill")
                    .args(["-KILL", "-f", pattern])
                    .status()
                    .await;
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        true
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_trainer_past_its_budget_takes_everything_it_started_with_it() {
        let (sleep, pattern) = marked_sleep();
        let (_root, mut config) = harness(&format!("{sleep} & wait"));
        config.train_timeout = Duration::from_secs(1);

        let error = rejected(&config, identity("abandoned")).await;

        assert!(
            matches!(&error, TrainError::Failed(message) if message.contains("timed out")),
            "{error}"
        );
        assert!(
            settles(&pattern, false).await,
            "a process the trainer started kept running past its budget"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_abandoned_trainer_takes_everything_it_started_with_it() {
        let (sleep, pattern) = marked_sleep();
        let mut process = Command::new("sh");
        process.arg("-c").arg(format!("{sleep} & wait"));

        tokio::select! {
            finished = run_trainer(process, Duration::from_secs(60)) => {
                panic!("the trainer finished instead of being abandoned: {finished:?}")
            }
            started = settles(&pattern, true) => {
                assert!(started, "the trainer never started its own process");
            }
        }

        assert!(
            settles(&pattern, false).await,
            "a process the trainer started outlived the abandoned run"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_trainer_that_exits_takes_what_it_left_running_with_it() {
        let (sleep, pattern) = marked_sleep();
        let mut process = Command::new("sh");
        process.arg("-c").arg(format!(
            "{sleep} & until pgrep -f '{pattern}' >/dev/null; do :; done"
        ));

        run_trainer(process, Duration::from_secs(60))
            .await
            .expect("the trainer exits cleanly");

        assert!(
            settles(&pattern, false).await,
            "a process the trainer left running outlived it"
        );
    }

    #[tokio::test]
    async fn only_the_end_of_a_trainers_stderr_is_kept() {
        let mut process = Command::new("sh");
        process
            .arg("-c")
            .arg("head -c 100000 /dev/zero | tr '\\0' x >&2; printf done >&2");
        let mut child = process.stderr(Stdio::piped()).spawn().unwrap();
        let tail = Arc::new(Mutex::new(Vec::new()));
        read_tail(child.stderr.take().unwrap(), Arc::clone(&tail)).await;
        child.wait().await.unwrap();
        let tail = tail.lock().unwrap();
        assert_eq!(tail.len(), STDERR_TAIL);
        assert!(tail.ends_with(b"done"));
    }

    #[tokio::test]
    async fn a_trainer_that_writes_nothing_is_not_a_success() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("true".into()),
            ..Default::default()
        };
        let error = rejected(&config, request("my-style", "flux-schnell", Some("ohwx"))).await;
        assert!(
            matches!(&error, TrainError::Failed(message) if message.contains("did not write")),
            "{error}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn an_edit_base_gets_a_control_directory_beside_its_targets() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("printf lora > \"$TRAIN_OUTPUT\"".into()),
            contract: contract(),
            ..Default::default()
        };
        let catalog = RecipeCatalog::packaged().unwrap();
        let edit_base = catalog
            .image_recipes()
            .find(|recipe| {
                !recipe.adapter && recipe.prompt_mode == crate::recipe::PromptMode::EditInstruction
            })
            .expect("the packaged catalog no longer ships an edit base")
            .id
            .clone();
        let mut pair = request("my-edit", &edit_base, Some("ohwx"));
        pair.images[0].before_base64 = Some(encoded_at(8, 8));
        train(&config, String::new(), SecretValue::new(""), pair)
            .await
            .expect("an edit base trains on before and after together");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn an_edit_training_writes_no_trigger_on_its_sidecar() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("printf lora > \"$TRAIN_OUTPUT\"".into()),
            contract: contract(),
            ..Default::default()
        };
        let mut pair = request("my-edit", "qwen-image-edit", Some("ohwx"));
        pair.images[0].before_base64 = Some(encoded_at(8, 8));
        let outcome = train(&config, String::new(), SecretValue::new(""), pair)
            .await
            .expect("an edit base trains on before and after together");
        let document: WeightDocument = serde_json::from_slice(
            &fs::read(sidecar_path(&outcome.path, &config.contract)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            document.sidecar.trigger, None,
            "an edit weight follows an instruction, so no trigger selects it"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_blank_caption_is_filled_in_before_the_dataset_is_written() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("printf lora > \"$TRAIN_OUTPUT\"".into()),
            contract: contract(),
            ..Default::default()
        };
        let mut blank = request("my-style", "flux-schnell", Some("ohwx"));
        blank.images[0].caption = String::new();
        train(&config, String::new(), SecretValue::new(""), blank)
            .await
            .expect("with no caption model, the trigger alone still has to reach the dataset");
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn a_name_too_long_to_write_is_refused_rather_than_failing_in_the_trainer() {
        let root = root();
        let config = Config {
            models_directory: root.clone(),
            train_command: Some("printf lora > \"$TRAIN_OUTPUT\"".into()),
            contract: contract(),
            ..Default::default()
        };
        let too_long_once_suffixed = "a".repeat(250);
        let error = rejected(
            &config,
            request(&too_long_once_suffixed, "flux-schnell", Some("ohwx")),
        )
        .await;
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");

        let named = train(
            &config,
            String::new(),
            SecretValue::new(""),
            request("already.safetensors", "flux-schnell", Some("ohwx")),
        )
        .await
        .unwrap();
        assert_eq!(
            named.path.file_name().unwrap(),
            "already.safetensors",
            "a name that already carries the extension keeps exactly one"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_training_request_prints_its_images_sizes_rather_than_their_bytes() {
        let upload = "A".repeat(1 << 20);
        let request = TrainRequest {
            images: vec![TrainImage {
                bytes_base64: upload.clone(),
                before_base64: Some(upload),
                ..image("target", "a portrait", None)
            }],
            ..identity("sized")
        };
        let rendered = format!("{request:?}");
        assert!(rendered.len() < 400, "{} characters", rendered.len());
        assert!(rendered.contains("bytes_base64: 1048576"), "{rendered}");
        assert!(
            rendered.contains("before_base64: Some(1048576)"),
            "{rendered}"
        );
        let uploaded = format!(
            "{:?}",
            TrainImage::from_bytes("a.png", "", vec![0; 1 << 20]).with_before(vec![0; 1 << 20])
        );
        assert!(uploaded.len() < 400, "{} characters", uploaded.len());
        assert!(uploaded.contains("bytes: Some(1048576)"), "{uploaded}");
        assert!(uploaded.contains("before: Some(1048576)"), "{uploaded}");
    }

    #[test]
    fn an_empty_upload_is_not_an_image() {
        let Err(error) = TrainImage::new("a.png", "", "").encoded() else {
            panic!("an empty upload is not an image");
        };
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
    }

    #[test]
    fn only_installed_bases_are_offered_for_training() {
        let root = std::env::temp_dir().join(format!("training-bases-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("checkpoints")).unwrap();
        let catalog = RecipeCatalog::packaged().unwrap();

        assert!(
            available_bases(&catalog, &root, &Contract::default()).is_empty(),
            "nothing is trainable until its weights are on disk"
        );

        let recipe = catalog
            .get("flux-schnell")
            .expect("the packaged catalog ships flux-schnell");
        let checkpoint = recipe
            .defaults
            .get("checkpoint")
            .expect("flux-schnell declares a checkpoint");
        fs::write(root.join("checkpoints").join(checkpoint), vec![0u8; 1024]).unwrap();

        let bases = available_bases(&catalog, &root, &Contract::default());
        assert!(
            bases
                .iter()
                .any(|base| base.id == "flux-schnell" && !base.edit),
            "the installed base should be offered: {bases:?}"
        );
        assert!(
            !bases.iter().any(|base| base.id.ends_with("-adapter")),
            "a LoRA slot is not something to train onto: {bases:?}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn without_a_train_command_the_job_runs_as_a_comfyui_graph() {
        let server = MockServer::start().await;
        let prompt = uuid::Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(Stage)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"prompt_id": prompt, "number": 1})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                prompt.to_string(): {"status": {"completed": true, "status_str": "success"}}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(ServeArtifact(vec![9u8; 20_000]))
            .mount(&server)
            .await;

        let root = root();
        let config = Config {
            models_directory: root.clone(),
            enabled: true,
            base_url: server.uri(),
            train_command: None,
            poll_interval: Duration::from_millis(50),
            ..Default::default()
        };
        let outcome = train(
            &config,
            String::new(),
            SecretValue::new(""),
            request("graph-style", "flux-schnell", Some("ohwx")),
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&outcome.path).unwrap(), vec![9u8; 20_000]);
        assert!(
            server
                .received_requests()
                .await
                .unwrap()
                .iter()
                .any(|request| request.url.path() == "/upload/image"),
            "the dataset has to reach the graph before it is queued"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// A deployment's own node pack: every node, namespace, variable prefix,
    /// sidecar, progress file and publication directory under a brand of its own.
    fn acme() -> Contract {
        Contract {
            train_lora_node: "AcmeTrainLoRA".into(),
            cleanup_training_run_node: "AcmeCleanupTrainingRun".into(),
            load_train_dataset_node: "AcmeLoadTrainDataset".into(),
            probe_loss_node: "AcmeProbeLoss".into(),
            stage_training_artifact_node: "AcmeStageTrainingArtifact".into(),
            folder_prefix: "acme-train-".into(),
            artifact_prefix: "acme-lora-".into(),
            probe_prefix: "acme-probe-".into(),
            environment_prefix: "ACME_TRAIN".into(),
            input_environment_prefix: "ACME_COMFY".into(),
            sidecar_suffix: ".acme.json".into(),
            publication_directory: ".acme-publish".into(),
            progress_suffix: ".acme-progress.json".into(),
        }
    }

    /// Checks the adapter was published under `acme()`'s names alone, and that
    /// the inventory lists it under that contract and no other.
    fn published_under_acme(models: &Path, adapter: &Path) {
        let loras = models.join("loras");
        let name = adapter.file_name().unwrap().to_str().unwrap();
        assert!(loras.join(format!("{name}.acme.json")).is_file());
        assert!(!loras.join(format!("{name}{SIDECAR_SUFFIX}")).exists());
        assert!(loras.join(".acme-publish").is_dir());
        assert!(!loras.join(PUBLICATION_DIRECTORY).exists());
        let catalog = RecipeCatalog::packaged().unwrap();
        assert!(
            crate::inventory::find(&crate::inventory::scan(models, &catalog, &acme()), name)
                .is_some(),
            "the adapter is listed under the contract it was published with"
        );
        assert!(
            crate::inventory::find(
                &crate::inventory::scan(models, &catalog, &Contract::default()),
                name
            )
            .is_none(),
            "the default contract does not read another deployment's sidecars"
        );
    }

    #[tokio::test]
    async fn an_overriding_contract_trains_through_its_own_nodes_and_publishes_under_its_own_names()
    {
        let server = MockServer::start().await;
        let prompt = uuid::Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(Stage)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"prompt_id": prompt, "number": 1})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                prompt.to_string(): {"status": {"completed": true, "status_str": "success"}}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(ServeArtifact(vec![9u8; 20_000]))
            .mount(&server)
            .await;
        let root = tempfile::tempdir().unwrap();
        let models = root.path().join("models");
        fs::create_dir_all(models.join("loras")).unwrap();
        let config = Config {
            models_directory: models.clone(),
            enabled: true,
            base_url: server.uri(),
            poll_interval: Duration::from_millis(50),
            contract: acme(),
            ..Default::default()
        };

        let outcome = train(
            &config,
            String::new(),
            SecretValue::new(""),
            request("acme-style", "flux-schnell", Some("ohwx")),
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let graphs: Vec<Value> = requests
            .iter()
            .filter(|request| request.url.path() == "/prompt")
            .map(|request| {
                serde_json::from_slice::<Value>(&request.body).unwrap()["prompt"].clone()
            })
            .collect();
        let classes: Vec<&str> = graphs
            .iter()
            .filter_map(Value::as_object)
            .flat_map(|graph| graph.values())
            .filter_map(|node| node["class_type"].as_str())
            .collect();
        assert!(classes.contains(&"AcmeTrainLoRA"), "{classes:?}");
        assert!(classes.contains(&"AcmeLoadTrainDataset"), "{classes:?}");
        assert!(
            !classes.iter().any(|class| class.starts_with("Abnegate")),
            "a default node reached the overriding pack: {classes:?}"
        );
        let trained = graphs
            .iter()
            .flat_map(|graph| {
                graph
                    .as_object()
                    .into_iter()
                    .flat_map(|graph| graph.values())
            })
            .find(|node| node["class_type"] == "AcmeTrainLoRA")
            .unwrap();
        assert!(
            trained["inputs"]["save_name"]
                .as_str()
                .unwrap()
                .starts_with("acme-lora-")
        );
        let uploads: Vec<String> = requests
            .iter()
            .filter(|request| request.url.path() == "/upload/image")
            .map(|request| String::from_utf8_lossy(&request.body).into_owned())
            .collect();
        assert!(
            uploads.iter().any(|body| body.contains("acme-train-")),
            "the dataset is staged under the overriding folder namespace"
        );
        assert!(
            !uploads.iter().any(|body| body.contains("abnegate-")),
            "an upload was staged under a default namespace"
        );
        published_under_acme(&models, &outcome.path);
    }

    #[tokio::test]
    async fn an_overriding_contract_hands_the_training_command_its_own_variables() {
        let command = r#"
            test -d "$ACME_TRAIN_DIRECTORY/targets" || exit 61
            test -n "$ACME_COMFY_INPUT" || exit 62
            case "$ACME_TRAIN_FOLDER" in acme-train-*) ;; *) exit 63 ;; esac
            case "$ACME_TRAIN_ARTIFACT" in acme-lora-*) ;; *) exit 64 ;; esac
            test -z "$(env | grep -E "^ABNEGATE_(TRAIN|COMFY)_")" || exit 65
            printf lora > "$ACME_TRAIN_OUTPUT"
        "#;
        let (_root, mut config) = harness(command);
        config.contract = acme();

        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("acme-command"),
            keep_all,
        )
        .await
        .expect("the command is told only the overriding contract's names");

        assert_eq!(fs::read(&outcome.path).unwrap(), b"lora");
        published_under_acme(&config.models_directory, &outcome.path);
    }

    #[test]
    fn caption_prefixes_trigger_for_identity() {
        assert_eq!(identity_caption("a portrait", "ohwx"), "ohwx, a portrait");
        assert_eq!(
            identity_caption("ohwx, a portrait", "ohwx"),
            "ohwx, a portrait"
        );
        assert_eq!(
            identity_caption("a fox located by a tree", "cat"),
            "cat, a fox located by a tree"
        );
        assert_eq!(
            identity_caption("A CAT by a tree", "cat"),
            "A CAT by a tree"
        );
        assert_eq!(
            identity_caption("a BLUE CAT by a tree", "blue cat"),
            "a BLUE CAT by a tree"
        );
        assert_eq!(
            identity_caption("a blue catapult", "blue cat"),
            "blue cat, a blue catapult"
        );
        assert_eq!(identity_caption("un CAFÉ", "café"), "un CAFÉ");
    }

    #[tokio::test]
    async fn train_rejects_missing_trigger() {
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        let mut request = identity("my-style");
        request.trigger = None;
        let error = train(&config, String::new(), SecretValue::new(""), request)
            .await
            .unwrap_err();
        assert!(matches!(error, TrainError::Invalid(_)));
        assert!(training_entries(&config).is_empty());
    }

    #[test]
    fn final_adapter_name_is_one_nonempty_path_component() {
        for invalid in [
            "",
            "   ",
            ".safetensors",
            "../escape",
            "folder/escape",
            r"folder\escape",
            "line\nbreak",
        ] {
            assert!(
                final_filename(invalid).is_err(),
                "{invalid:?} must not become a filesystem path"
            );
        }
        assert_eq!(
            final_filename(" studio-style ").unwrap(),
            "studio-style.safetensors"
        );
    }

    #[tokio::test]
    async fn edit_training_requires_a_decodable_reference_and_instruction_for_every_pair() {
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        let cases = [
            edit(vec![image("target", "replace the sky", None)]),
            edit(vec![TrainImage {
                before_base64: Some("not base64".to_string()),
                ..image("target", "replace the sky", Some("reference"))
            }]),
            edit(vec![image("target", "  ", Some("reference"))]),
        ];
        for request in cases {
            let error = train_with_screening(
                &config,
                String::new(),
                SecretValue::new(""),
                request,
                keep_all,
            )
            .await
            .unwrap_err();
            assert!(matches!(error, TrainError::Invalid(_)), "got {error:?}");
        }
        assert!(training_entries(&config).is_empty());
    }

    #[tokio::test]
    async fn edit_screening_reindexes_targets_references_and_instructions_together() {
        let command = r#"
            test "$TRAIN_IMAGE_COUNT" = "2" || exit 11
            test "$TRAIN_NAME" = "edit-style.safetensors" || exit 31
            test "$TRAIN_FINAL_NAME" = "edit-style.safetensors" || exit 12
            test "$TRAIN_ATTEMPT" = "$(basename "$TRAIN_DIRECTORY")" || exit 14
            test "$(basename "$TRAIN_OUTPUT")" = "$TRAIN_ATTEMPT.safetensors" || exit 13
            test "$TRAIN_ARCHITECTURE" = "qwen_edit" || exit 23
            test "$TRAIN_UNET" = "qwen_image_edit_2511_fp8mixed.safetensors" || exit 24
            test "$TRAIN_CLIP" = "qwen_2.5_vl_7b_fp8_scaled.safetensors" || exit 25
            test "$TRAIN_VAE" = "qwen_image_vae.safetensors" || exit 26
            test "$TRAIN_DEFER_CLEANUP" = "1" || exit 27
            folder_id=${TRAIN_FOLDER#FOLDER_PREFIX}
            artifact_id=${TRAIN_ARTIFACT#ARTIFACT_PREFIX}
            test "$folder_id" != "$TRAIN_FOLDER" || exit 28
            test "$artifact_id" != "$TRAIN_ARTIFACT" || exit 29
            test "$folder_id" != "$artifact_id" || exit 30
            test "$(cat "$TRAIN_DIRECTORY/targets/0000.txt")" = "change zero" || exit 17
            test "$(cat "$TRAIN_DIRECTORY/targets/0001.txt")" = "change two" || exit 20
            test ! -e "$TRAIN_DIRECTORY/targets/0002.png" || exit 21
            test ! -e "$TRAIN_DIRECTORY/control_1/0002.png" || exit 22
            cp "$TRAIN_DIRECTORY/targets/0000.png" "KEPT/target-0.png" || exit 15
            cp "$TRAIN_DIRECTORY/control_1/0000.png" "KEPT/control-0.png" || exit 16
            cp "$TRAIN_DIRECTORY/targets/0001.png" "KEPT/target-1.png" || exit 18
            cp "$TRAIN_DIRECTORY/control_1/0001.png" "KEPT/control-1.png" || exit 19
            printf trained > "$TRAIN_OUTPUT"
        "#;
        let kept = tempfile::tempdir().expect("kept dataset");
        let command = command
            .replace("KEPT", &kept.path().display().to_string())
            .replace("FOLDER_PREFIX", FOLDER_PREFIX)
            .replace("ARTIFACT_PREFIX", ARTIFACT_PREFIX);
        let (_root, mut config) = harness(&command);
        config.caption_model = "vision".to_string();
        let captioner = wiremock::MockServer::start().await;
        let outcome = train_with_screening(
            &config,
            captioner.uri(),
            SecretValue::new("key"),
            edit(vec![
                image("target-zero", " change zero ", Some("reference-zero")),
                image("target-one", "change one", Some("reference-one")),
                image("target-two", " change two ", Some("reference-two")),
            ]),
            drop_middle,
        )
        .await
        .expect("screened edit run");

        assert_eq!(outcome.screening.kept, 2);
        assert_eq!(outcome.screening.dropped.len(), 1);
        assert_eq!(outcome.screening.dropped[0].filename, "target-one.png");
        assert!(
            outcome
                .dataset
                .iter()
                .any(|finding| finding.detail.starts_with("Only 2 images")),
            "dataset advice must use the screened count: {:?}",
            outcome.dataset
        );
        assert_eq!(fs::read(&outcome.path).unwrap(), b"trained");
        assert!(
            captioner.received_requests().await.unwrap().is_empty(),
            "edit instructions must never be sent through the identity captioner"
        );
        assert!(training_entries(&config).is_empty());

        let side = crate::train::packaged_config().unwrap().resolution();
        let subject = Subject::shared(&config).await;
        for (index, name) in [(0, "zero"), (1, "two")] {
            let source = image(
                &format!("target-{name}"),
                "",
                Some(&format!("reference-{name}")),
            );
            let framed = frame(&subject, &source, side).expect("the fixture frames");
            assert_eq!(
                fs::read(kept.path().join(format!("target-{index}.png"))).unwrap(),
                framed.target,
                "targets/{index:04} must hold the {name} pair's target"
            );
            assert_eq!(
                fs::read(kept.path().join(format!("control-{index}.png"))).unwrap(),
                framed.control.expect("the fixture carries a reference"),
                "control_1/{index:04} must hold the {name} pair's reference"
            );
        }
    }

    #[tokio::test]
    async fn a_repaired_target_is_rescreened_and_staged_instead_of_its_original() {
        let server = MockServer::start().await;
        let repaired = png(&Rendered::new(
            2,
            2,
            vec![220, 30, 90, 220, 30, 90, 220, 30, 90, 220, 30, 90],
        ))
        .expect("repaired fixture");
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(Stage)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .and(wiremock::matchers::body_string_contains(
                "ImageUpscaleWithModel",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"prompt_id": "repair"})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/history/repair"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "repair": {"status": {"status_str": "success"}, "outputs": {
                    "4": {"images": [{"filename": "repaired.png", "subfolder": "", "type": "temp"}]}
                }}
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(repaired.clone()),
            )
            .expect(1)
            .mount(&server)
            .await;

        let staged = tempfile::tempdir().expect("staged target");
        let target = staged.path().join("target.png");
        let command = format!(
            "cp \"$TRAIN_DIRECTORY/targets/0000.png\" \"{}\"; printf trained > \"$TRAIN_OUTPUT\"",
            target.display()
        );
        let (_root, mut config) = harness(&command);
        config.enabled = true;
        config.base_url = server.uri();
        config.poll_interval = Duration::from_millis(1);

        let outcome = train_with_remediation(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("repaired"),
            reject_tiny,
            Reporter::default(),
        )
        .await
        .expect("repaired target trains");

        assert_eq!(outcome.screening.kept, 1);
        assert!(outcome.screening.dropped.is_empty());
        assert_eq!(
            outcome.screening.attempted,
            vec![Remediation {
                source_index: 0,
                filename: "target.png".into(),
                reason: Rejection::Small,
                outcome: RemediationOutcome::Used,
            }]
        );
        let expected = frame(
            &Subject::shared(&config).await,
            &TrainImage::from_bytes("target.png", "a portrait", repaired.clone()),
            crate::train::packaged_config().unwrap().resolution(),
        )
        .expect("expected repaired target")
        .target;
        assert_eq!(fs::read(target).unwrap(), expected);
    }

    #[test]
    fn final_screen_marks_repaired_targets_used_or_rejected_and_keeps_failures_visible() {
        let attempted = finish_remediation(
            vec![
                RemediationAttempt {
                    index: 0,
                    remediation: Remediation {
                        source_index: 0,
                        filename: "used.png".into(),
                        reason: Rejection::Small,
                        outcome: RemediationOutcome::Failed,
                    },
                    replaced: true,
                },
                RemediationAttempt {
                    index: 1,
                    remediation: Remediation {
                        source_index: 1,
                        filename: "rejected.png".into(),
                        reason: Rejection::Blurred,
                        outcome: RemediationOutcome::Failed,
                    },
                    replaced: true,
                },
                RemediationAttempt {
                    index: 2,
                    remediation: Remediation {
                        source_index: 2,
                        filename: "failed.png".into(),
                        reason: Rejection::Small,
                        outcome: RemediationOutcome::Failed,
                    },
                    replaced: false,
                },
            ],
            &Verdict {
                keep: vec![0],
                drop: vec![(1, Rejection::Blurred), (2, Rejection::Small)],
            },
        );

        assert_eq!(
            attempted,
            vec![
                Remediation {
                    source_index: 0,
                    filename: "used.png".into(),
                    reason: Rejection::Small,
                    outcome: RemediationOutcome::Used,
                },
                Remediation {
                    source_index: 1,
                    filename: "rejected.png".into(),
                    reason: Rejection::Blurred,
                    outcome: RemediationOutcome::StillRejected,
                },
                Remediation {
                    source_index: 2,
                    filename: "failed.png".into(),
                    reason: Rejection::Small,
                    outcome: RemediationOutcome::Failed,
                },
            ]
        );
    }

    #[tokio::test]
    async fn screened_qwen_pipeline_posts_one_aligned_model_and_pair_contract() {
        let server = MockServer::start().await;
        let prompt = Uuid::new_v4();
        let root = tempfile::tempdir().expect("temporary ComfyUI root");
        let models = root.path().join("models");
        let input = root.path().join("input");
        let produced = root.path().join("output/loras");
        fs::create_dir(&models).unwrap();
        fs::create_dir(&input).unwrap();
        fs::create_dir_all(&produced).unwrap();

        let other_folder = format!("{FOLDER_PREFIX}{}", Uuid::new_v4());
        let other_artifact = format!("{ARTIFACT_PREFIX}{}", Uuid::new_v4());
        fs::create_dir(input.join(&other_folder)).unwrap();
        fs::write(input.join(&other_folder).join("kept"), b"other input").unwrap();
        fs::write(
            produced.join(format!("{other_artifact}.safetensors")),
            b"other output",
        )
        .unwrap();

        let produced_for_prompt = produced.clone();
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(move |request: &Request| {
                let body: Value = request.body_json().unwrap();
                let graph = &body["prompt"];
                if graph.as_object().is_some_and(|nodes| {
                    nodes
                        .values()
                        .any(|node| node["class_type"] == TRAIN_LORA_NODE)
                }) {
                    let artifact = graph["7"]["inputs"]["save_name"].as_str().unwrap();
                    fs::write(
                        produced_for_prompt.join(format!("{artifact}.safetensors")),
                        b"remote final",
                    )
                    .unwrap();
                    fs::write(
                        produced_for_prompt.join(format!("{artifact}-step11.safetensors")),
                        b"remote checkpoint",
                    )
                    .unwrap();
                }
                ResponseTemplate::new(200).set_body_json(json!({
                    "prompt_id": prompt,
                    "number": 0.0,
                    "node_errors": {}
                }))
            })
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/history/[0-9a-f-]+$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                prompt.to_string(): {
                    "status": {"completed": true, "status_str": "success"},
                    "outputs": {}
                }
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/view"))
            .respond_with(|request: &Request| {
                let filename = request
                    .url
                    .query_pairs()
                    .find_map(|(name, value)| (name == "filename").then(|| value.into_owned()))
                    .unwrap();
                ResponseTemplate::new(200)
                    .insert_header("Content-Disposition", format!("filename=\"{filename}\""))
                    .insert_header("Content-Type", "application/octet-stream")
                    .set_body_bytes(vec![7; 10_001])
            })
            .mount(&server)
            .await;

        let config = Config {
            enabled: true,
            base_url: server.uri(),
            models_directory: models,
            poll_interval: Duration::from_millis(1),
            train_command: None,
            train_timeout: Duration::from_secs(2),
            ..Default::default()
        };
        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            edit(vec![
                image("target-zero", " change zero ", Some("reference-zero")),
                image("target-one", "change one", Some("reference-one")),
                image("target-two", " change two ", Some("reference-two")),
            ]),
            drop_middle,
        )
        .await
        .expect("native Qwen run");

        assert_eq!(outcome.screening.kept, 2);
        assert_eq!(fs::read(&outcome.path).unwrap(), vec![7; 10_001]);
        let requests = server.received_requests().await.unwrap();
        let graphs = requests
            .iter()
            .filter(|request| request.url.path() == "/prompt")
            .map(|request| request.body_json::<Value>().unwrap()["prompt"].clone())
            .collect::<Vec<Value>>();
        let training = graphs
            .iter()
            .find(|graph| {
                graph.as_object().is_some_and(|nodes| {
                    nodes
                        .values()
                        .any(|node| node["class_type"] == TRAIN_LORA_NODE)
                })
            })
            .expect("posted training graph");
        assert_eq!(training["1"]["class_type"], "UNETLoader");
        assert_eq!(training["2"]["class_type"], "CLIPLoader");
        assert_eq!(training["2"]["inputs"]["type"], "qwen_image");
        assert_eq!(training["3"]["class_type"], "VAELoader");
        assert_eq!(training["5"]["class_type"], "VAEEncode");
        assert_eq!(training["6"]["class_type"], "TextEncodeQwenImageEditPlus");
        let serialized = training.to_string();
        assert!(!serialized.contains("CheckpointLoaderSimple"));
        assert!(!serialized.contains("MakeTrainingDataset"));

        let manifest: Value =
            serde_json::from_str(training["4"]["inputs"]["manifest_json"].as_str().unwrap())
                .unwrap();
        let expected = json!({
            "schema_version": 1,
            "architecture": "qwen_edit",
            "pairs": [
                {
                    "index": 0,
                    "target": "targets/0000.png",
                    "reference": "control_1/0000.png",
                    "instruction": "change zero"
                },
                {
                    "index": 1,
                    "target": "targets/0001.png",
                    "reference": "control_1/0001.png",
                    "instruction": "change two"
                }
            ]
        });
        assert_eq!(manifest, expected);

        let folder = training["4"]["inputs"]["folder"]
            .as_str()
            .unwrap()
            .to_string();
        let artifact = training["7"]["inputs"]["save_name"]
            .as_str()
            .unwrap()
            .to_string();
        Run {
            folder: folder.clone(),
            artifact: artifact.clone(),
        }
        .validate(&Contract::default())
        .unwrap();
        assert_ne!(
            folder.trim_start_matches(FOLDER_PREFIX),
            artifact.trim_start_matches(ARTIFACT_PREFIX)
        );

        let quality = graphs
            .iter()
            .find(|graph| {
                graph.as_object().is_some_and(|nodes| {
                    nodes
                        .values()
                        .any(|node| node["class_type"] == PROBE_LOSS_NODE)
                })
            })
            .expect("posted quality graph");
        let quality_manifest: Value =
            serde_json::from_str(quality["4"]["inputs"]["manifest_json"].as_str().unwrap())
                .unwrap();
        assert_eq!(quality_manifest, expected);
        assert_eq!(quality["1"]["class_type"], "UNETLoader");
        assert_eq!(quality["2"]["inputs"]["type"], "qwen_image");
        assert_eq!(quality["3"]["class_type"], "VAELoader");
        assert_eq!(quality["6"]["class_type"], "TextEncodeQwenImageEditPlus");

        assert!(!input.join(folder).exists());
        assert!(!produced.join(format!("{artifact}.safetensors")).exists());
        assert!(
            !produced
                .join(format!("{artifact}-step11.safetensors"))
                .exists()
        );
        assert_eq!(
            fs::read(input.join(other_folder).join("kept")).unwrap(),
            b"other input"
        );
        assert_eq!(
            fs::read(produced.join(format!("{other_artifact}.safetensors"))).unwrap(),
            b"other output"
        );
        assert!(training_entries(&config).is_empty());
    }

    #[tokio::test]
    async fn non_edit_training_rejects_controls_and_never_stages_a_control_folder() {
        let command = r#"
            test ! -e "$TRAIN_DIRECTORY/control_1" || exit 31
            test "$(cat "$TRAIN_DIRECTORY/targets/0000.txt")" = "ohwx, a portrait" || exit 32
            printf trained > "$TRAIN_OUTPUT"
        "#;
        let (_root, config) = harness(command);
        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("identity"),
            keep_all,
        )
        .await
        .expect("identity run");
        assert_eq!(fs::read(outcome.path).unwrap(), b"trained");

        let mut paired = identity("paired-identity");
        paired.images[0].before_base64 = Some(encoded(colour("reference")));
        let error = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            paired,
            keep_all,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TrainError::Invalid(_)));
        assert!(training_entries(&config).is_empty());
    }

    #[tokio::test]
    async fn a_failed_same_name_attempt_preserves_the_previous_adapter_and_is_not_reused() {
        let (_root, mut config) = harness("printf first > \"$TRAIN_OUTPUT\"");
        let first = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("stable"),
            keep_all,
        )
        .await
        .expect("first attempt");
        let original = fs::read(&first.path).unwrap();

        config.train_command = Some("printf partial > \"$TRAIN_OUTPUT\"; exit 41".to_string());
        let error = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("stable"),
            keep_all,
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TrainError::Failed(_)));
        assert_eq!(fs::read(&first.path).unwrap(), original);
        assert!(training_entries(&config).is_empty());

        config.train_command = Some(
            "test ! -e \"$TRAIN_OUTPUT\" || exit 42; printf third > \"$TRAIN_OUTPUT\"".to_string(),
        );
        let third = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("stable"),
            keep_all,
        )
        .await
        .expect("fresh third attempt");
        assert_eq!(fs::read(third.path).unwrap(), b"third");
        assert!(training_entries(&config).is_empty());
    }

    #[test]
    fn concurrent_same_name_publications_cannot_interleave_weight_and_sidecar() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (_root, config) = harness("unused");
        let loras = config.models_directory.join("loras");
        fs::create_dir(&loras).unwrap();
        let output = loras.join("shared.safetensors");
        let output_sidecar = sidecar_path(&output, &config.contract);
        let first_generation = Uuid::new_v4().to_string();
        let second_generation = Uuid::new_v4().to_string();
        let (first_attempt, first_weight, first_sidecar) =
            staged_adapter(&config, &first_generation, b"first");
        let (second_attempt, second_weight, second_sidecar) =
            staged_adapter(&config, &second_generation, b"second");
        let (weights_tx, weights_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_output = output.clone();
        let first_output_sidecar = output_sidecar.clone();
        let first_loras = loras.clone();
        let first_contract = config.contract.clone();
        let first = std::thread::spawn(move || {
            promote_with(
                &first_attempt,
                &first_weight,
                &first_sidecar,
                &first_loras,
                &first_output,
                &first_output_sidecar,
                &first_generation,
                &first_contract,
                |phase| {
                    if phase == PublicationPhase::Weights {
                        weights_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                    }
                    Ok(())
                },
            )
        });
        weights_rx.recv().unwrap();

        let (done_tx, done_rx) = mpsc::channel();
        let second_output = output.clone();
        let second_output_sidecar = output_sidecar.clone();
        let second_loras = loras.clone();
        let second_contract = config.contract.clone();
        let expected_generation = second_generation.clone();
        let second = std::thread::spawn(move || {
            let result = atomic_promote(
                &second_attempt,
                &second_weight,
                &second_sidecar,
                &second_loras,
                &second_output,
                &second_output_sidecar,
                &second_generation,
                &second_contract,
            );
            done_tx.send(()).unwrap();
            result
        });
        assert!(
            done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "the second publication must wait at the same final name"
        );
        assert!(
            crate::inventory::scan(
                &config.models_directory,
                &RecipeCatalog::packaged().unwrap(),
                &config.contract,
            )
            .is_empty(),
            "readers must not observe the first weight before its sidecar"
        );

        release_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        assert_eq!(fs::read(output).unwrap(), b"second");
        assert_eq!(
            read_generation(&output_sidecar).as_deref(),
            Some(expected_generation.as_str())
        );
    }

    #[test]
    fn interrupted_publication_is_hidden_and_recovers_the_previous_generation() {
        let (_root, config) = harness("unused");
        let loras = config.models_directory.join("loras");
        fs::create_dir(&loras).unwrap();
        let output = loras.join("stable.safetensors");
        let output_sidecar = sidecar_path(&output, &config.contract);
        let previous_generation = Uuid::new_v4().to_string();
        fs::write(&output, b"previous").unwrap();
        fs::write(
            &output_sidecar,
            serde_json::to_vec(&WeightDocument {
                sidecar: WeightSidecar::new("flux-schnell-adapter")
                    .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
                generation: Some(previous_generation.clone()),
            })
            .unwrap(),
        )
        .unwrap();
        let generation = Uuid::new_v4().to_string();
        let (attempt, staged, staged_sidecar) = staged_adapter(&config, &generation, b"new");
        let interrupted = std::panic::catch_unwind(|| {
            promote_with(
                &attempt,
                &staged,
                &staged_sidecar,
                &loras,
                &output,
                &output_sidecar,
                &generation,
                &config.contract,
                |phase| {
                    if phase == PublicationPhase::Weights {
                        panic!("simulated process interruption");
                    }
                    Ok(())
                },
            )
            .unwrap();
        });
        assert!(interrupted.is_err());
        let marker = publication_marker(&loras, "stable.safetensors", &config.contract).unwrap();
        assert!(marker.is_file());
        assert!(
            crate::inventory::scan(
                &config.models_directory,
                &RecipeCatalog::packaged().unwrap(),
                &config.contract,
            )
            .is_empty(),
            "an interrupted generation must fail closed"
        );

        restore_snapshot(&attempt, &attempt.join(PREVIOUS_WEIGHT), &output, true).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"previous");
        recover_publication(&loras, &output, &output_sidecar, &marker, &config.contract).unwrap();
        assert_eq!(fs::read(output).unwrap(), b"previous");
        assert_eq!(
            read_generation(&output_sidecar).as_deref(),
            Some(previous_generation.as_str())
        );
        assert!(!marker.exists());
    }

    #[test]
    fn publication_error_restores_both_files_before_returning() {
        let (_root, config) = harness("unused");
        let loras = config.models_directory.join("loras");
        fs::create_dir(&loras).unwrap();
        let output = loras.join("stable.safetensors");
        let output_sidecar = sidecar_path(&output, &config.contract);
        let previous_generation = Uuid::new_v4().to_string();
        fs::write(&output, b"previous").unwrap();
        fs::write(
            &output_sidecar,
            serde_json::to_vec(&WeightDocument {
                sidecar: WeightSidecar::new("flux-schnell-adapter")
                    .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
                generation: Some(previous_generation.clone()),
            })
            .unwrap(),
        )
        .unwrap();
        let generation = Uuid::new_v4().to_string();
        let (attempt, staged, staged_sidecar) = staged_adapter(&config, &generation, b"new");

        let error = promote_with(
            &attempt,
            &staged,
            &staged_sidecar,
            &loras,
            &output,
            &output_sidecar,
            &generation,
            &config.contract,
            |phase| match phase {
                PublicationPhase::Weights => {
                    Err(TrainError::Failed("injected publication error".into()))
                }
                PublicationPhase::Sidecar => Ok(()),
            },
        )
        .unwrap_err();

        assert!(error.to_string().contains("injected publication error"));
        assert_eq!(fs::read(output).unwrap(), b"previous");
        assert_eq!(
            read_generation(&output_sidecar).as_deref(),
            Some(previous_generation.as_str())
        );
        assert!(
            !publication_marker(&loras, "stable.safetensors", &config.contract)
                .unwrap()
                .exists()
        );
    }

    #[test]
    fn mismatched_staged_sidecar_never_reaches_the_final_name() {
        let (_root, config) = harness("unused");
        let loras = config.models_directory.join("loras");
        fs::create_dir(&loras).unwrap();
        let generation = Uuid::new_v4().to_string();
        let wrong_generation = Uuid::new_v4().to_string();
        let (attempt, staged, sidecar) = staged_adapter(&config, &generation, b"new");
        fs::write(
            &sidecar,
            serde_json::to_vec(&WeightDocument {
                sidecar: WeightSidecar::new("flux-schnell-adapter")
                    .with_huggingface_base("black-forest-labs/FLUX.1-schnell"),
                generation: Some(wrong_generation),
            })
            .unwrap(),
        )
        .unwrap();
        let output = loras.join("stable.safetensors");
        let output_sidecar = sidecar_path(&output, &config.contract);

        let error = atomic_promote(
            &attempt,
            &staged,
            &sidecar,
            &loras,
            &output,
            &output_sidecar,
            &generation,
            &config.contract,
        )
        .unwrap_err();

        assert!(matches!(error, TrainError::Invalid(_)));
        assert!(!output.exists());
        assert!(!output_sidecar.exists());
    }

    #[tokio::test]
    async fn attempt_cleanup_never_removes_another_attempt() {
        let (_root, config) = harness("printf trained > \"$TRAIN_OUTPUT\"");
        let other = config
            .models_directory
            .join("training")
            .join(Uuid::new_v4().to_string());
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("marker"), b"owned elsewhere").unwrap();

        train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("isolated"),
            keep_all,
        )
        .await
        .expect("isolated attempt");

        assert_eq!(fs::read(other.join("marker")).unwrap(), b"owned elsewhere");
        assert_eq!(training_entries(&config), vec![other]);
    }

    #[tokio::test]
    async fn unsupported_recipe_requests_fail_closed_before_creating_an_attempt() {
        let (_root, config) = harness("printf trained > \"$TRAIN_OUTPUT\"");
        let mut request = identity("unsupported");
        request.base = "sd15".to_string();

        let error = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            request,
            keep_all,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, TrainError::Invalid(_)));
        assert!(training_entries(&config).is_empty());
        assert!(
            !config
                .models_directory
                .join("loras/unsupported.safetensors")
                .exists()
        );
    }

    #[tokio::test]
    async fn invalid_overlay_catalog_never_falls_back_to_packaged_training() {
        let (root, mut config) = harness("printf trained > \"$TRAIN_OUTPUT\"");
        let workflows = root.path().join("workflows");
        let recipes = root.path().join("recipes");
        fs::create_dir(&workflows).unwrap();
        fs::create_dir(&recipes).unwrap();
        let mut catalog: Value =
            serde_json::from_str(include_str!("../comfyui/recipes/catalog.json")).unwrap();
        let flux = catalog["recipes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|recipe| recipe["id"] == "flux-schnell")
            .unwrap();
        flux["training"].as_object_mut().unwrap().remove("adapter");
        fs::write(
            recipes.join("catalog.json"),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
        config.workflow_path = Some(workflows.join("flux1-schnell-fp8-api.json"));

        let error = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("must-not-train"),
            keep_all,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, TrainError::Invalid(_)));
        assert!(training_entries(&config).is_empty());
        assert!(
            !config
                .models_directory
                .join("loras/must-not-train.safetensors")
                .exists()
        );
    }

    fn trainable_sdxl(root: &Path, config: &mut Config) {
        let workflows = root.join("workflows");
        let recipes = root.join("recipes");
        fs::create_dir(&workflows).unwrap();
        fs::create_dir(&recipes).unwrap();
        let mut catalog: Value =
            serde_json::from_str(include_str!("../comfyui/recipes/catalog.json")).unwrap();
        let sdxl = catalog["recipes"]
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
        fs::write(
            recipes.join("catalog.json"),
            serde_json::to_vec(&catalog).unwrap(),
        )
        .unwrap();
        config.workflow_path = Some(workflows.join("sdxl-api.json"));
    }

    #[tokio::test]
    async fn sdxl_trains_through_the_command_on_its_checkpoint() {
        let command = r#"
            test "$TRAIN_ARCHITECTURE" = "sdxl" || exit 11
            test "$TRAIN_CHECKPOINT" = "RealVisXL_V5.0_fp16.safetensors" || exit 12
            printf trained > "$TRAIN_OUTPUT"
        "#;
        let (root, mut config) = harness(command);
        trainable_sdxl(root.path(), &mut config);
        let request =
            TrainRequest::new("person", "sdxl", vec![image("target", "a portrait", None)])
                .with_trigger("ohwx");

        train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            request,
            keep_all,
        )
        .await
        .expect("the command trains SDXL");
    }

    #[tokio::test]
    async fn sdxl_without_a_training_command_is_refused_before_screening() {
        let (root, mut config) = harness("unused");
        trainable_sdxl(root.path(), &mut config);
        config.train_command = None;
        config.enabled = true;
        let request =
            TrainRequest::new("person", "sdxl", vec![image("target", "a portrait", None)])
                .with_trigger("ohwx");

        let error = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            request,
            |_, _| panic!("screening must not run"),
        )
        .await
        .unwrap_err();

        assert!(
            matches!(error, TrainError::Invalid(message) if message.contains("SDXL")),
            "{error:?}"
        );
    }

    #[test]
    fn advertised_bases_are_ready_and_have_a_typed_training_architecture() {
        let (_root, config) = harness("unused");
        fs::create_dir_all(config.models_directory.join("checkpoints")).unwrap();
        fs::create_dir_all(config.models_directory.join("diffusion_models")).unwrap();
        fs::create_dir_all(config.models_directory.join("text_encoders")).unwrap();
        fs::create_dir_all(config.models_directory.join("vae")).unwrap();
        fs::write(
            config
                .models_directory
                .join("checkpoints/flux1-schnell-fp8.safetensors"),
            b"flux",
        )
        .unwrap();
        fs::write(
            config
                .models_directory
                .join("checkpoints/sd15-custom.safetensors"),
            b"sd15",
        )
        .unwrap();
        fs::write(
            config
                .models_directory
                .join("diffusion_models/qwen_image_edit_2511_fp8mixed.safetensors"),
            b"qwen",
        )
        .unwrap();
        fs::write(
            config
                .models_directory
                .join("text_encoders/qwen_2.5_vl_7b_fp8_scaled.safetensors"),
            b"clip",
        )
        .unwrap();
        fs::write(
            config
                .models_directory
                .join("vae/qwen_image_vae.safetensors"),
            b"vae",
        )
        .unwrap();
        let catalog = RecipeCatalog::packaged().unwrap();

        let bases = available_bases(&catalog, &config.models_directory, &config.contract);

        assert!(bases.iter().any(|base| base.id == "flux-schnell"));
        assert!(
            bases
                .iter()
                .any(|base| base.id == "qwen-image-edit" && base.edit)
        );
        assert!(
            bases.iter().all(|base| base.id != "sd15"),
            "a runnable generation graph is not automatically a trainable architecture"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_training_root_cannot_escape_the_models_directory() {
        use std::os::unix::fs::symlink;

        let (root, config) = harness("printf escaped > \"$TRAIN_OUTPUT\"");
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, config.models_directory.join("training")).unwrap();

        let error = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("escape"),
            keep_all,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, TrainError::Invalid(_)));
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_final_adapter_is_rejected_without_touching_its_target() {
        use std::os::unix::fs::symlink;

        let (root, config) = harness("printf replacement > \"$TRAIN_OUTPUT\"");
        let loras = config.models_directory.join("loras");
        fs::create_dir(&loras).unwrap();
        let outside = root.path().join("outside.safetensors");
        fs::write(&outside, b"outside").unwrap();
        symlink(&outside, loras.join("linked.safetensors")).unwrap();

        let error = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("linked"),
            keep_all,
        )
        .await
        .unwrap_err();

        assert!(matches!(error, TrainError::Invalid(_)));
        assert_eq!(fs::read(outside).unwrap(), b"outside");
        assert!(training_entries(&config).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn dataset_writes_do_not_follow_a_symlinked_file() {
        use std::os::unix::fs::symlink;

        let (root, config) = harness("unused");
        let attempt = Attempt::create(&config.models_directory).unwrap();
        let targets = ensure_child_directory(&attempt.root, "targets").unwrap();
        let outside = root.path().join("outside.png");
        fs::write(&outside, b"outside").unwrap();
        let staged = targets.join("0000.png");
        symlink(&outside, &staged).unwrap();

        assert!(write_new(&attempt.root, &staged, b"replacement").is_err());
        assert_eq!(fs::read(outside).unwrap(), b"outside");
    }

    #[test]
    fn a_group_at_the_top_of_its_range_does_not_wrap_a_photo_onto_a_clip() {
        let images = vec![
            upload("", Some(usize::MAX)),
            upload("", Some(usize::MAX)),
            upload("", None),
        ];
        let assigned = shots(&images);
        assert_eq!(assigned[0], assigned[1], "two frames of one shot share it");
        assert_ne!(
            assigned[2], assigned[0],
            "a photo must not be captioned as part of the clip's shot"
        );
    }

    #[test]
    fn a_trigger_is_matched_as_a_word_rather_than_a_substring() {
        assert_eq!(
            identity_caption("zrk pattern knitwear", "zrkx"),
            "zrkx, zrk pattern knitwear"
        );
        assert_eq!(
            identity_caption("zrkxyz, a portrait", "zrkxyz"),
            "zrkxyz, a portrait",
            "a caption that already names the trigger keeps exactly one"
        );
        assert_eq!(
            identity_caption("zrkxyzed hair", "zrkxyz"),
            "zrkxyz, zrkxyzed hair",
            "a longer word that merely starts with the trigger is not the trigger"
        );
        assert_eq!(
            identity_caption("my-style, a portrait", "my-style"),
            "my-style, a portrait",
            "a hyphenated trigger the caption already names is not repeated"
        );
        assert_eq!(
            identity_caption("a portrait", "my-style"),
            "my-style, a portrait"
        );
    }

    #[test]
    fn a_failed_write_names_the_artifact_it_was_for() {
        let attempt = tempfile::tempdir().unwrap();
        let missing = attempt.path().join("targets").join("0000.png");
        let Err(TrainError::Failed(message)) = write_new(attempt.path(), &missing, b"x") else {
            panic!("writing into a directory that does not exist should fail");
        };
        assert!(
            message.contains("0000.png"),
            "an operator has to be told which artifact failed: {message}"
        );
    }

    #[test]
    fn a_photo_never_lands_in_a_clips_shot() {
        let images = vec![
            upload("", Some(0)),
            upload("", Some(1)),
            upload("", Some(0)),
            upload("", None),
            upload("", None),
        ];
        let assigned = shots(&images);
        assert_eq!(
            assigned,
            vec![0, 1, 0, 2, 3],
            "frames keep their shot, photos get one each, and the two never meet"
        );
    }

    #[test]
    fn a_photo_is_framed_as_the_upright_square_the_dataset_reads_back() {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&[10; 8 * 4 * 3], 8, 4, image::ExtendedColorType::Rgb8)
            .unwrap();
        let framed = frame(
            &Subject::none(),
            &TrainImage::new(
                "a.jpg",
                "",
                base64::engine::general_purpose::STANDARD.encode(&jpeg),
            ),
            4,
        )
        .unwrap();
        assert!(
            framed.target.starts_with(b"\x89PNG\r\n\x1a\n"),
            "a .png in the dataset has to be a PNG"
        );
        assert_eq!(
            decode::decode(&framed.target).unwrap().oriented_size(),
            (4, 4),
            "the loader letterboxes anything that is not already square"
        );
        assert!(framed.control.is_none(), "there was no before image");
    }

    #[test]
    fn an_undecodable_upload_is_rejected_rather_than_framed() {
        let error = frame(
            &Subject::none(),
            &TrainImage::new(
                "a.png",
                "",
                base64::engine::general_purpose::STANDARD.encode(b"not an image"),
            ),
            512,
        )
        .unwrap_err();
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
    }

    #[test]
    fn an_edit_pair_is_cropped_the_same_way_on_both_sides() {
        let wide = encoded_at(16, 8);
        let framed = frame(
            &Subject::none(),
            &TrainImage::new("a.png", "", wide.clone()).with_before_base64(wide),
            8,
        )
        .unwrap();
        assert_eq!(
            framed.control.as_deref(),
            Some(framed.target.as_slice()),
            "a control cropped to its own subject stops answering its target"
        );
    }

    /// Runs `stage` beside another task on this current-thread runtime, and
    /// says whether that task got to run before `stage` finished, which it
    /// can only do if `stage` left the runtime's one worker free.
    async fn beside<T>(stage: impl Future<Output = T>) -> (T, bool) {
        let ran = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn({
            let ran = Arc::clone(&ran);
            async move { ran.store(true, Ordering::SeqCst) }
        });
        let output = stage.await;
        let free = ran.load(Ordering::SeqCst);
        task.await.expect("the task beside the stage finishes");
        (output, free)
    }

    fn uploads(count: usize) -> Vec<TrainImage> {
        (0..count)
            .map(|index| {
                TrainImage::new(format!("{index}.png"), "a portrait", encoded_at(256, 256))
            })
            .collect()
    }

    fn side() -> u32 {
        crate::train::packaged_config()
            .expect("packaged training config")
            .resolution()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn uploads_are_decoded_without_stalling_the_runtime() {
        let (decoded, free) = beside(decode_uploads(uploads(8), Reporter::default())).await;

        let (images, decoded) = decoded.expect("the uploads decode");
        assert_eq!(decoded.len(), 8);
        assert!(
            images.iter().all(|image| image.bytes_base64.is_empty()),
            "the decoded uploads are held once"
        );
        assert!(free, "decoding the uploads stalled the runtime's worker");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_set_is_screened_without_stalling_the_runtime() {
        let (_, decoded) = decode_uploads(uploads(8), Reporter::default())
            .await
            .unwrap();

        let (screened, free) = beside(screen(crate::screening::screen, decoded, side())).await;

        let (decoded, verdict) = screened.expect("the set is screened");
        assert_eq!(decoded.len(), 8, "the screened set comes back whole");
        validate_verdict(&verdict, 8).expect("the verdict partitions the set");
        assert!(free, "screening the set stalled the runtime's worker");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn survivors_are_framed_without_stalling_the_runtime() {
        let (images, decoded) = decode_uploads(uploads(2), Reporter::default())
            .await
            .unwrap();

        let (framed, free) = beside(frame_survivors(
            Subject::none(),
            images,
            decoded,
            vec![1],
            side(),
            false,
            Reporter::default(),
        ))
        .await;

        let framed = framed.expect("the survivor is framed");
        assert_eq!(framed.len(), 1);
        assert_eq!(framed[0].original, 1);
        assert!(framed[0].encoded.is_some(), "an identity crop is captioned");
        assert!(free, "framing the survivors stalled the runtime's worker");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_repaired_image_is_checked_without_stalling_the_runtime() {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded_at(256, 256))
            .unwrap();

        let (readable, free) = beside(decodable(bytes.into())).await;

        assert!(readable, "a PNG is decodable");
        assert!(
            free,
            "checking a repaired image stalled the runtime's worker"
        );
    }

    static SCREENED_ON: Mutex<Option<ThreadId>> = Mutex::new(None);

    fn keep_all_noting_the_thread(images: &[Vec<u8>], resolution: u32) -> Verdict {
        *SCREENED_ON.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(std::thread::current().id());
        keep_all(images, resolution)
    }

    #[tokio::test(flavor = "current_thread")]
    async fn training_screens_its_set_off_the_runtimes_worker() {
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");

        train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            identity("screened-aside"),
            keep_all_noting_the_thread,
        )
        .await
        .expect("the set trains");

        let screened_on = SCREENED_ON
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .expect("the set was screened");
        assert_ne!(
            screened_on,
            std::thread::current().id(),
            "screening ran on the runtime's only worker"
        );
    }

    fn person_finetune(base: &str) -> TrainRule {
        TrainRule::new(base, TrainMethod::Finetune, "flux-schnell", "checkpoints")
            .with_subjects([TrainSubject::Person])
            .with_required("checkpoints/flux1-schnell-fp8.safetensors")
    }

    fn finetuned(name: &str) -> TrainRequest {
        identity(name)
            .with_subject(TrainSubject::Person)
            .with_method(TrainMethod::Finetune)
    }

    fn refused(config: &Config, request: &TrainRequest) -> &'static str {
        match validate_request(config, request) {
            Err(TrainError::Invalid(message)) => message,
            other => panic!("expected an invalid request, got {other:?}"),
        }
    }

    #[test]
    fn a_name_another_model_folder_holds_in_any_case_is_refused() {
        let (_root, mut config) = harness("true");
        config.train_policy = TrainPolicy::new().with_rule(person_finetune("flux-schnell"));
        let models = config.models_directory.clone();
        for directory in ["checkpoints", "loras"] {
            fs::create_dir_all(models.join(directory)).unwrap();
        }
        fs::write(
            models.join("checkpoints/flux1-schnell-fp8.safetensors"),
            b"flux",
        )
        .unwrap();
        validate_request(&config, &finetuned("jerry")).expect("no folder holds the name yet");

        fs::write(models.join("loras/Jerry.safetensors"), b"lora").unwrap();
        assert_eq!(
            refused(&config, &finetuned("jerry")),
            NAMED_ELSEWHERE,
            "a fine-tune named after a LoRA would make the name select either weight"
        );

        fs::remove_file(models.join("loras/Jerry.safetensors")).unwrap();
        let output = models.join("checkpoints/jerry.safetensors");
        fs::write(&output, b"tuned").unwrap();
        fs::write(
            sidecar_path(&output, &config.contract),
            serde_json::to_vec(&WeightDocument {
                sidecar: WeightSidecar::new("flux-schnell"),
                generation: Some(Uuid::new_v4().to_string()),
            })
            .unwrap(),
        )
        .unwrap();
        validate_request(&config, &finetuned("jerry"))
            .expect("a fine-tune may replace its own published output");
        config.train_policy = TrainPolicy::new();
        assert_eq!(
            refused(&config, &identity("jerry")),
            NAMED_ELSEWHERE,
            "a LoRA named after a fine-tune is refused the same way"
        );
    }

    #[test]
    fn a_base_no_rule_names_trains_only_a_lora_on_other() {
        let (_root, config) = harness("true");
        validate_request(&config, &identity("plain")).expect("the default trains a LoRA");
        assert_eq!(
            refused(
                &config,
                &identity("tuned").with_method(TrainMethod::Finetune)
            ),
            METHOD_UNAVAILABLE
        );
        assert_eq!(
            refused(
                &config,
                &identity("person").with_subject(TrainSubject::Person)
            ),
            SUBJECT_UNAVAILABLE
        );
    }

    #[test]
    fn a_rule_decides_the_subjects_methods_and_files_a_base_trains() {
        let (_root, mut config) = harness("true");
        config.train_policy = TrainPolicy::new().with_rule(person_finetune("flux-schnell"));

        assert_eq!(
            refused(&config, &finetuned("jerry")),
            "install the files this base needs before training on it"
        );
        fs::create_dir_all(config.models_directory.join("checkpoints")).unwrap();
        fs::write(
            config
                .models_directory
                .join("checkpoints/flux1-schnell-fp8.safetensors"),
            b"flux",
        )
        .unwrap();
        validate_request(&config, &finetuned("jerry")).expect("the required base is installed");
        let graph = Config {
            train_command: None,
            enabled: true,
            ..config.clone()
        };
        assert_eq!(
            refused(&graph, &finetuned("jerry")),
            "the packaged training graph trains only a LoRA; configure a training command"
        );

        assert_eq!(
            refused(
                &config,
                &identity("jerry").with_method(TrainMethod::Finetune)
            ),
            SUBJECT_UNAVAILABLE
        );
        assert_eq!(
            refused(
                &config,
                &identity("jerry").with_subject(TrainSubject::Person)
            ),
            METHOD_UNAVAILABLE,
            "a base a rule names trains only the methods its rules give"
        );
        let mut untriggered = finetuned("jerry");
        untriggered.trigger = None;
        assert_eq!(
            refused(&config, &untriggered),
            "trigger word is required so the weight can retain identity"
        );
        validate_request(
            &config,
            &edit(vec![image("target", "replace the sky", Some("reference"))]),
        )
        .expect("a rule on one base leaves the rest on the default");
    }

    #[tokio::test]
    async fn a_rule_publishes_the_weight_where_its_method_belongs() {
        let command = r#"
            test "$TRAIN_METHOD" = finetune || exit 61
            test "$TRAIN_SUBJECT" = person || exit 62
            printf tuned > "$TRAIN_OUTPUT"
        "#;
        let (_root, mut config) = harness(command);
        config.train_policy = TrainPolicy::new().with_rule(
            TrainRule::new(
                "flux-schnell",
                TrainMethod::Finetune,
                "flux-schnell",
                "checkpoints",
            )
            .with_subjects([TrainSubject::Person]),
        );

        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            finetuned("jerry"),
            keep_all,
        )
        .await
        .expect("the fine-tune publishes");

        assert_eq!(
            outcome.path,
            config
                .models_directory
                .join("checkpoints/jerry.safetensors")
        );
        assert_eq!(fs::read(&outcome.path).unwrap(), b"tuned");
        let sidecar: Value = serde_json::from_slice(
            &fs::read(sidecar_path(&outcome.path, &config.contract)).unwrap(),
        )
        .unwrap();
        assert_eq!(sidecar["recipe_id"], "flux-schnell");
        assert_eq!(sidecar["hf_base"], "black-forest-labs/FLUX.1-schnell");
        assert!(
            !config
                .models_directory
                .join("loras/jerry.safetensors")
                .exists()
        );
    }

    #[test]
    fn a_policy_that_could_publish_outside_the_models_directory_is_refused() {
        let flux = || {
            TrainRule::new(
                "flux-schnell",
                TrainMethod::Lora,
                "flux-schnell-adapter",
                "loras",
            )
        };
        let mut empty = flux();
        empty.subjects.clear();
        let mut duplicated = TrainPolicy::new().with_rule(flux());
        duplicated.rules.push(flux());
        for policy in [
            TrainPolicy::new().with_rule(empty),
            TrainPolicy::new().with_rule(TrainRule::new(
                "flux-schnell",
                TrainMethod::Lora,
                " ",
                "loras",
            )),
            TrainPolicy::new().with_rule(TrainRule::new(
                "flux-schnell",
                TrainMethod::Lora,
                "flux-schnell-adapter",
                "",
            )),
            TrainPolicy::new().with_rule(TrainRule::new(
                "flux-schnell",
                TrainMethod::Lora,
                "flux-schnell-adapter",
                "..",
            )),
            TrainPolicy::new().with_rule(TrainRule::new(
                "flux-schnell",
                TrainMethod::Lora,
                "flux-schnell-adapter",
                "a/b",
            )),
            TrainPolicy::new().with_rule(TrainRule::new(
                "flux-schnell",
                TrainMethod::Lora,
                "flux-schnell-adapter",
                "/loras",
            )),
            TrainPolicy::new().with_rule(flux().with_required("../outside.safetensors")),
            TrainPolicy::new().with_rule(flux().with_required("/etc/passwd")),
            duplicated,
        ] {
            let config = Config {
                train_policy: policy.clone(),
                ..Config::default()
            };
            assert!(config.validate().is_err(), "{policy:?} was accepted");
        }
        let replaced = TrainPolicy::new()
            .with_rule(flux())
            .with_rule(flux().with_subjects([TrainSubject::Person]));
        assert_eq!(replaced.rules.len(), 1, "a later rule replaces the earlier");
        assert_eq!(replaced.rules_for("flux-schnell").count(), 1);
        assert_eq!(
            replaced
                .rule("flux-schnell", TrainMethod::Lora)
                .unwrap()
                .subjects,
            [TrainSubject::Person]
        );
        assert!(
            replaced
                .rule("flux-schnell", TrainMethod::Finetune)
                .is_none()
        );
        let config = Config {
            train_policy: replaced,
            ..Config::default()
        };
        config
            .validate()
            .expect("one rule per base and method is valid");
    }

    #[test]
    fn subjects_and_methods_keep_their_wire_names() {
        let request: TrainRequest = serde_json::from_value(json!({
            "name": "jerry",
            "base": "flux-schnell",
            "subject": "person",
            "method": "finetune",
            "images": [{"filename": "a.png", "caption": "", "bytes": [1, 2]}],
        }))
        .unwrap();
        assert_eq!(request.subject, TrainSubject::Person);
        assert_eq!(request.method, TrainMethod::Finetune);
        assert!(
            request.images[0].bytes.is_none(),
            "raw bytes only arrive through a constructor, never through JSON"
        );
        let defaults: TrainRequest =
            serde_json::from_value(json!({"name": "a", "base": "b", "images": []})).unwrap();
        assert_eq!(defaults.subject, TrainSubject::Other);
        assert_eq!(defaults.method, TrainMethod::Lora);
        for (subject, name) in [
            (TrainSubject::Other, "other"),
            (TrainSubject::Person, "person"),
        ] {
            assert_eq!(serde_json::to_value(subject).unwrap(), name);
            assert_eq!(subject.as_str(), name);
        }
        for (method, name) in [
            (TrainMethod::Lora, "lora"),
            (TrainMethod::Finetune, "finetune"),
        ] {
            assert_eq!(serde_json::to_value(method).unwrap(), name);
            assert_eq!(method.as_str(), name);
        }
    }

    #[test]
    fn uploaded_bytes_win_over_base64() {
        let png = base64::engine::general_purpose::STANDARD
            .decode(encoded(colour("target")))
            .unwrap();
        let mut image = TrainImage::new("a.png", "", "not base64");
        image.bytes = Some(png.clone());
        assert_eq!(image.encoded().unwrap().as_ref(), png.as_slice());
        assert!(matches!(
            TrainImage::from_bytes("a.png", "", Vec::new()).encoded(),
            Err(TrainError::Invalid("image is empty"))
        ));
        assert!(matches!(
            TrainImage::new("a.png", "", "not base64").encoded(),
            Err(TrainError::Invalid("image is not valid base64"))
        ));

        let paired = TrainImage::from_bytes("a.png", "", png.clone()).with_before(png.clone());
        assert!(paired.has_reference());
        assert_eq!(
            paired.encoded_reference().unwrap().as_deref(),
            Some(png.as_slice())
        );
        let blank = TrainImage::from_bytes("a.png", "", png.clone()).with_before_base64("  ");
        assert!(!blank.has_reference(), "a blank reference is no reference");
        assert!(
            TrainImage::from_bytes("a.png", "", png.clone())
                .encoded_reference()
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_uploaded_edit_pair_trains_without_base64() {
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        let bytes = |name: &str| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded(colour(name)))
                .unwrap()
        };
        let pair = TrainImage::from_bytes("target.png", "replace the sky", bytes("target"))
            .with_before(bytes("reference"));

        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            edit(vec![pair]),
            keep_all,
        )
        .await
        .expect("an uploaded pair trains");

        assert_eq!(fs::read(outcome.path).unwrap(), b"lora");
    }

    #[test]
    fn a_screening_report_can_be_kept_beside_the_outcome() {
        let screening = Screening {
            kept: 1,
            dropped: vec![Dropped {
                filename: "a.png".into(),
                reason: Rejection::Blurred,
            }],
            attempted: Vec::new(),
        };
        let kept = screening.clone();
        assert_eq!(
            serde_json::to_value(&kept).unwrap(),
            serde_json::to_value(&screening).unwrap()
        );
    }

    fn installed_base(config: &Config) -> PathBuf {
        let base = config
            .models_directory
            .join("checkpoints/flux1-schnell-fp8.safetensors");
        fs::create_dir_all(base.parent().unwrap()).unwrap();
        fs::write(&base, b"base weights").unwrap();
        base
    }

    fn misconfigured(config: &Config, request: &TrainRequest) -> &'static str {
        match validate_request(config, request) {
            Err(TrainError::Configuration(message)) => message,
            other => panic!("expected a configuration error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_fine_tune_never_publishes_over_its_base_checkpoint() {
        let (_root, mut config) = harness("printf tuned > \"$TRAIN_OUTPUT\"");
        assert_eq!(
            refused(&config, &identity("flux1-dev-fp8")),
            "this name belongs to a weight the catalog or a training rule needs",
            "a LoRA cannot take a catalog weight's name either"
        );
        config.train_policy = TrainPolicy::new().with_rule(person_finetune("flux-schnell"));
        let base = installed_base(&config);

        for name in [
            "flux1-schnell-fp8",
            "flux1-schnell-fp8.safetensors",
            "FLUX1-schnell-fp8",
        ] {
            assert_eq!(
                refused(&config, &finetuned(name)),
                "this name belongs to a weight the catalog or a training rule needs",
                "{name}"
            );
        }
        let error = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            finetuned("flux1-schnell-fp8"),
            keep_all,
        )
        .await
        .expect_err("the base checkpoint is not a training output");

        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
        assert_eq!(fs::read(&base).unwrap(), b"base weights");
        assert!(!sidecar_path(&base, &config.contract).exists());
        assert!(training_entries(&config).is_empty());
    }

    #[tokio::test]
    async fn only_a_weight_training_published_is_replaced_by_training() {
        let (_root, mut config) = harness("printf tuned > \"$TRAIN_OUTPUT\"");
        config.train_policy = TrainPolicy::new().with_rule(person_finetune("flux-schnell"));
        installed_base(&config);
        let downloaded = config
            .models_directory
            .join("checkpoints/jerry.safetensors");
        fs::write(&downloaded, b"downloaded").unwrap();
        crate::inventory::write_sidecar(
            &downloaded,
            &WeightSidecar::new("flux-schnell"),
            &config.contract,
        )
        .unwrap();

        assert_eq!(
            refused(&config, &finetuned("jerry")),
            "a weight training did not publish already has this name"
        );
        train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            finetuned("jerry"),
            keep_all,
        )
        .await
        .expect_err("a downloaded checkpoint is not a training output");
        assert_eq!(fs::read(&downloaded).unwrap(), b"downloaded");

        fs::remove_file(&downloaded).unwrap();
        fs::remove_file(sidecar_path(&downloaded, &config.contract)).unwrap();
        let first = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            finetuned("jerry"),
            keep_all,
        )
        .await
        .expect("a free name publishes");
        config.train_command = Some("printf again > \"$TRAIN_OUTPUT\"".to_string());
        let second = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            finetuned("jerry"),
            keep_all,
        )
        .await
        .expect("a prior training output is replaced");
        assert_eq!(first.path, second.path);
        assert_eq!(fs::read(&second.path).unwrap(), b"again");

        let arriving = config
            .models_directory
            .join("checkpoints/arriving.safetensors");
        config.train_command = Some(format!(
            "printf placed > '{}'; printf tuned > \"$TRAIN_OUTPUT\"",
            arriving.display()
        ));
        train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            finetuned("arriving"),
            keep_all,
        )
        .await
        .expect_err("a weight placed during the run is not replaced");
        assert_eq!(fs::read(&arriving).unwrap(), b"placed");
        assert!(!sidecar_path(&arriving, &config.contract).exists());
    }

    #[tokio::test]
    async fn a_fine_tune_is_never_probed_as_a_lora() {
        let server = MockServer::start().await;
        let prompt = uuid::Uuid::new_v4();
        Mock::given(method("POST"))
            .and(path("/upload/image"))
            .respond_with(Stage)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"prompt_id": prompt, "number": 1})),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/history/{prompt}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                prompt.to_string(): {"status": {"completed": true, "status_str": "success"}}
            })))
            .mount(&server)
            .await;
        let (_root, mut config) = harness("printf tuned > \"$TRAIN_OUTPUT\"");
        config.base_url = server.uri();
        config.poll_interval = Duration::from_millis(50);
        fs::create_dir_all(config.models_directory.join("loras")).unwrap();
        config.train_policy = TrainPolicy::new().with_rule(
            TrainRule::new(
                "flux-schnell",
                TrainMethod::Finetune,
                "flux-schnell",
                "checkpoints",
            )
            .with_subjects([TrainSubject::Person]),
        );

        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            finetuned("jerry"),
            keep_all,
        )
        .await
        .expect("the fine-tune publishes");

        assert!(outcome.quality.is_none());
        let classes: Vec<String> = server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|request| request.url.path() == "/prompt")
            .flat_map(|request| {
                let body = serde_json::from_slice::<Value>(&request.body).unwrap();
                body["prompt"]
                    .as_object()
                    .into_iter()
                    .flat_map(|graph| graph.values())
                    .filter_map(|node| node["class_type"].as_str().map(str::to_string))
                    .collect::<Vec<String>>()
            })
            .collect();
        assert!(
            classes
                .iter()
                .all(|class| *class == config.contract.cleanup_training_run_node),
            "a fine-tune reached a LoRA probe: {classes:?}"
        );
    }

    #[tokio::test]
    async fn a_rule_must_publish_for_a_recipe_that_runs_its_weight() {
        let (_root, mut config) = harness("true");
        installed_base(&config);
        for (method, recipe_id, request) in [
            (TrainMethod::Lora, "flux-schnell", identity("plain")),
            (TrainMethod::Lora, "missing", identity("plain")),
            (TrainMethod::Lora, "wan-adapter", identity("plain")),
            (TrainMethod::Lora, "flux-dev-adapter", identity("plain")),
            (
                TrainMethod::Finetune,
                "flux-schnell-adapter",
                identity("tuned").with_method(TrainMethod::Finetune),
            ),
            (
                TrainMethod::Finetune,
                "missing",
                identity("tuned").with_method(TrainMethod::Finetune),
            ),
        ] {
            config.train_policy = TrainPolicy::new().with_rule(TrainRule::new(
                "flux-schnell",
                method,
                recipe_id,
                "checkpoints",
            ));
            assert_eq!(
                misconfigured(&config, &request),
                "training rule names a recipe that cannot run its weight",
                "{method:?} published for {recipe_id}"
            );
        }
        for (method, recipe_id, directory, request) in [
            (
                TrainMethod::Lora,
                "flux-schnell-adapter",
                "loras",
                identity("plain"),
            ),
            (
                TrainMethod::Finetune,
                "flux-schnell",
                "checkpoints",
                identity("tuned").with_method(TrainMethod::Finetune),
            ),
        ] {
            config.train_policy = TrainPolicy::new().with_rule(TrainRule::new(
                "flux-schnell",
                method,
                recipe_id,
                directory,
            ));
            validate_request(&config, &request)
                .unwrap_or_else(|error| panic!("{method:?} for {recipe_id}: {error}"));
        }
    }

    #[test]
    fn a_rule_must_publish_where_its_recipe_loads_the_weight() {
        let (_root, mut config) = harness("true");
        installed_base(&config);
        let tuned = || identity("tuned").with_method(TrainMethod::Finetune);
        for (method, recipe_id, directory, request) in [
            (
                TrainMethod::Lora,
                "flux-schnell-adapter",
                "checkpoints",
                identity("plain"),
            ),
            (
                TrainMethod::Finetune,
                "flux-schnell",
                "diffusion_models",
                tuned(),
            ),
            (TrainMethod::Finetune, "flux-schnell", "loras", tuned()),
            (TrainMethod::Finetune, "flux-schnell", "unet", tuned()),
        ] {
            config.train_policy = TrainPolicy::new().with_rule(TrainRule::new(
                "flux-schnell",
                method,
                recipe_id,
                directory,
            ));
            assert_eq!(
                misconfigured(&config, &request),
                "training rule names a recipe that cannot run its weight",
                "{method:?} for {recipe_id} published in {directory}"
            );
        }

        config.train_policy = TrainPolicy::new().with_rule(TrainRule::new(
            "qwen-image-edit",
            TrainMethod::Finetune,
            "qwen-image-edit",
            "diffusion_models",
        ));
        validate_request(
            &config,
            &edit(vec![image("target", "replace the sky", Some("reference"))])
                .with_method(TrainMethod::Finetune),
        )
        .expect("a UNet recipe loads a fine-tune from diffusion_models");
    }

    #[test]
    fn a_rule_with_a_blank_base_or_padded_names_is_refused() {
        let flux = |base: &str, recipe_id: &str, directory: &str| {
            TrainPolicy::new().with_rule(TrainRule::new(
                base,
                TrainMethod::Lora,
                recipe_id,
                directory,
            ))
        };
        for policy in [
            flux("", "flux-schnell-adapter", "loras"),
            flux("  ", "flux-schnell-adapter", "loras"),
            flux(" flux-schnell", "flux-schnell-adapter", "loras"),
            flux("flux-schnell", " flux-schnell-adapter", "loras"),
            flux("flux-schnell", "flux-schnell-adapter ", "loras"),
            flux("flux-schnell", "flux-schnell-adapter", " loras"),
            flux("flux-schnell", "flux-schnell-adapter", "loras "),
            flux("flux-schnell", "flux-schnell-adapter", "lo\0ras"),
            flux("flux-schnell", "flux-schnell-adapter", "lo\nras"),
        ] {
            let config = Config {
                train_policy: policy.clone(),
                ..Config::default()
            };
            assert!(config.validate().is_err(), "{policy:?} was accepted");
        }
    }

    #[test]
    fn a_blank_reference_is_no_reference_to_read() {
        let png = base64::engine::general_purpose::STANDARD
            .decode(encoded(colour("target")))
            .unwrap();
        let encoded_png = encoded(colour("target"));
        for blank in [
            TrainImage::from_bytes("a.png", "", png.clone()).with_before_base64(""),
            TrainImage::from_bytes("a.png", "", png.clone()).with_before_base64("  "),
            TrainImage::from_bytes("a.png", "", png.clone()).with_before(Vec::new()),
        ] {
            assert!(!blank.has_reference());
            assert!(
                blank.encoded_reference().unwrap().is_none(),
                "{blank:?} read a reference has_reference denies"
            );
        }
        let padded = TrainImage::from_bytes("a.png", "", png.clone())
            .with_before(Vec::new())
            .with_before_base64(encoded_png.clone());
        assert!(padded.has_reference());
        assert_eq!(
            padded.encoded_reference().unwrap().as_deref(),
            Some(png.as_slice()),
            "an empty upload gives way to the base64 beside it"
        );
        let mut emptied = TrainImage::new("a.png", "", encoded_png);
        emptied.bytes = Some(Vec::new());
        assert_eq!(
            emptied.encoded().unwrap().as_ref(),
            png.as_slice(),
            "an empty upload gives way to the base64 beside it"
        );
    }

    #[tokio::test]
    async fn an_identity_upload_with_a_blank_reference_still_trains() {
        let (_root, config) = harness("printf lora > \"$TRAIN_OUTPUT\"");
        let request = TrainRequest::new(
            "blank-reference",
            "flux-schnell",
            vec![image("target", "a portrait", None).with_before_base64("")],
        )
        .with_trigger("ohwx");

        let outcome = train_with_screening(
            &config,
            String::new(),
            SecretValue::new(""),
            request,
            keep_all,
        )
        .await
        .expect("a blank reference is no reference");

        assert_eq!(fs::read(outcome.path).unwrap(), b"lora");
    }

    #[test]
    fn taking_an_upload_leaves_neither_its_bytes_nor_its_base64() {
        let png = base64::engine::general_purpose::STANDARD
            .decode(encoded(colour("target")))
            .unwrap();
        let mut image = TrainImage::new("a.png", "", encoded(colour("target")));
        image.bytes = Some(png.clone());

        assert_eq!(image.take_encoded().unwrap(), png);
        assert!(image.bytes.is_none());
        assert!(
            image.bytes_base64.is_empty(),
            "the base64 beside an upload is held for nothing"
        );
    }
}
