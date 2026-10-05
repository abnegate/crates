# abnegate-comfy

ComfyUI integration: image, video, and audio generation, upscaling, model
inventory, and LoRA training. The crate talks to a ComfyUI server over HTTP and
owns nothing else, with no web framework, database, or application state, so it
can be dropped into any project that needs image generation or wants to train a
LoRA. Training writes the dataset, captions any image left blank, runs the
packaged training graph on the configured server, then scores every checkpoint
the run produced and keeps the best one, and `video::extract` turns a clip into
that image set. The training graphs call custom nodes that ComfyUI does not ship;
`Config::contract` names them, together with the run namespaces, the variables
an external training command reads, and the sidecar and publication names the
model inventory keeps. Its defaults sit under a neutral `Abnegate` namespace. A
host that collects metrics installs `observe_requests` once at startup; without
it the crate records nothing.

## Features

- `saliency`: frames training crops on the subject U2-Net finds, through `abnegate-vision` on ONNX Runtime, when `Config::vision_model` points at the weights; without it a photo is cropped on its centre and a video frame on whatever moved.

## Platform support

The crate builds on Linux, Android, FreeBSD and Apple platforms only, the ones
`abnegate-exec` supports, because the training command, ffmpeg and ffprobe run
under its environment policy.

## Usage

```sh
cargo add abnegate-comfy
cargo add tokio --features sync
```

```rust,no_run
use abnegate_comfy::{Client, Config, Error};
use tokio::sync::{broadcast, mpsc};

async fn lighthouse() -> Result<(), Error> {
    let client = Client::new(Config::from_environment())?;
    let (_stop, mut cancel) = broadcast::channel(1);
    let (progress, _updates) = mpsc::unbounded_channel();

    let images = client
        .generate("a lighthouse in a storm", None, &mut cancel, progress)
        .await?;
    println!("{} images", images.len());
    Ok(())
}
```

## Training on your own node pack

A deployment whose node pack registers other names sets them on the contract,
field by field, before it trains:

```rust,no_run
use abnegate_comfy::Config;
use abnegate_comfy::lora::{self, TrainError, TrainImage, TrainRequest};
use abnegate_comfy::train::Contract;
use abnegate_secret::SecretValue;

async fn train(photo_base64: String) -> Result<(), TrainError> {
    let mut contract = Contract::default();
    contract.train_lora_node = "AcmeTrainLoRA".into();
    contract.cleanup_training_run_node = "AcmeCleanupTrainingRun".into();
    contract.load_train_dataset_node = "AcmeLoadTrainDataset".into();
    contract.probe_loss_node = "AcmeProbeLoss".into();
    contract.stage_training_artifact_node = "AcmeStageTrainingArtifact".into();
    contract.folder_prefix = "acme-train-".into();
    contract.artifact_prefix = "acme-lora-".into();
    contract.probe_prefix = "acme-probe-".into();
    contract.environment_prefix = "ACME_TRAIN".into();
    contract.input_environment_prefix = "ACME_COMFY".into();
    contract.sidecar_suffix = ".acme.json".into();
    contract.publication_directory = ".acme-publish".into();
    contract.progress_suffix = ".acme-progress.json".into();

    let mut config = Config::from_environment();
    config.contract = contract;
    let request = TrainRequest::new(
        "acme-style",
        "flux-schnell",
        vec![TrainImage::new("photo.png", "", photo_base64)],
    )
    .with_trigger("ohwx");
    let outcome = lora::train(
        &config,
        "http://127.0.0.1:4000".to_string(),
        SecretValue::new("vision-model-key"),
        request,
    )
    .await?;
    println!("{}", outcome.path.display());
    Ok(())
}
```

The adapter lands in `loras/acme-style.safetensors` with its recipe binding in
`acme-style.safetensors.acme.json`, and `inventory::scan` lists it only when it
is given the same contract.

## Recipe catalog

Every graph the client runs comes from a recipe in `comfyui/recipes/catalog.json`.
A host replaces the packaged catalog by placing its own `recipes/catalog.json`
beside the directory `Config::workflow_path` names; graphs of the same name in
that directory replace the packaged copies. Besides the graph pair and its
slots, a recipe may declare:

- `edit_recipe`: the base recipe an edit of a source image switches to when
  every file it requires is installed and the selected checkpoint is one of the
  recipe's own stock files (its default or one its `files` list). The packaged
  `flux-schnell` hands edits to `flux-dev`, so new pictures stay fast while
  edits get Dev's sampler budget; a custom checkpoint that `flux-schnell` only
  runs, such as a fine-tune, keeps its own weights. It must name a base image
  recipe with a source graph and the same prompt mode.
- `slots.pose`: the `LoadImage` input of an OpenPose ControlNet branch.
  `recipe::Fill::with_pose` steers that branch; without a pose the branch is
  removed and the sampler reads the plain prompt. The packaged `sdxl` and
  `sdxl-adapter` graphs carry one.
- `pose_required_files`: the files only that branch loads, which a host checks
  are installed before it passes a pose. The packaged SDXL recipes declare
  `controlnet/thibaud_xl_openpose.safetensors`; `inventory::scan` does not
  count them, since a run without a pose never loads them.
- `extras`: what the graph supports beyond its slots, as `recipe::RecipeExtra`
  values (a value this version does not know is ignored): `face` (an
  IP-Adapter face reference), `refine` (a face-refine pass on every result)
  and `identity_refine` (one only when the selected weight is
  bound to an identity).
- `training.architecture`: `flux`, `qwen_edit` or `sdxl`. SDXL has no packaged
  ComfyUI training graph and trains only through `Config::train_command`.

## Deciding what each base trains

By default every trainable base trains a LoRA on any subject that is not a
person, published in `loras/`. A deployment that trains more says so per base
and method in `Config::train_policy`: the subjects a method accepts, the
directory and recipe its weight is published under, and the files that have to
be installed first. A rule's recipe has to run what it publishes: an adapter
over the same base for a LoRA, a base recipe with a checkpoint or UNet slot for
a fine-tune. Training never publishes over a weight a recipe or rule needs, nor
over one that no training run published, so a fine-tune cannot replace its own
base checkpoint. Only a LoRA is scored against its base. The external trainer
reads the request's choice from `<prefix>_SUBJECT` and `<prefix>_METHOD`.
Uploads arrive as raw bytes, so a large set never round-trips through base64,
and `lora::validate_request` refuses a bad request before any image is read:

```rust,no_run
use abnegate_comfy::Config;
use abnegate_comfy::lora::{
    self, TrainError, TrainImage, TrainMethod, TrainPolicy, TrainRequest, TrainRule, TrainSubject,
};

fn person(photo: Vec<u8>) -> Result<TrainRequest, TrainError> {
    let mut config = Config::from_environment();
    config.train_policy = TrainPolicy::new().with_rule(
        TrainRule::new("flux-schnell", TrainMethod::Finetune, "flux-schnell", "checkpoints")
            .with_subjects([TrainSubject::Person])
            .with_required("checkpoints/flux1-schnell-fp8.safetensors"),
    );
    let request = TrainRequest::new(
        "jerry",
        "flux-schnell",
        vec![TrainImage::from_bytes("photo.jpg", "", photo)],
    )
    .with_trigger("ohwx")
    .with_subject(TrainSubject::Person)
    .with_method(TrainMethod::Finetune);
    lora::validate_request(&config, &request)?;
    Ok(request)
}
```

## Watching a run

`lora::train_reporting` is `lora::train` with a `tokio::sync::watch` channel
that always holds the latest `train::TrainProgress`: a `train::Phase` and how
far through it the run is for every image loaded, cropped and captioned, then
the step, loss and estimate the trainer writes to its progress file while it
runs, then publishing. The packaged graph's training node writes
`<artifact><Contract::progress_suffix>` in ComfyUI's LoRA output; a
`Config::train_command` writes the path in `<prefix>_PROGRESS`, and trains for
the host's own `<prefix>_STEPS` or, when the host sets none, the packaged
budget handed to it under that name. An update identical to the one already in
the channel is not sent again. The future is `Send`, so a host runs it as a
background job and lets the request that started it go; dropping every
receiver only stops the updates, and a host that keeps a clone of the sender
can `subscribe` whenever someone looks.

```rust,no_run
use abnegate_comfy::Config;
use abnegate_comfy::lora::{self, TrainRequest};
use abnegate_secret::SecretValue;
use tokio::sync::watch;

async fn watch(config: Config, request: TrainRequest) {
    let (progress, mut updates) = watch::channel(None);
    let job = tokio::spawn(async move {
        lora::train_reporting(
            &config,
            "http://127.0.0.1:4000".to_string(),
            SecretValue::new("vision-model-key"),
            request,
            progress,
        )
        .await
    });
    while updates.changed().await.is_ok() {
        if let Some(update) = updates.borrow_and_update().as_ref() {
            let phase = update.phase.as_ref().map_or("", |phase| phase.as_str());
            println!("{phase} {}/{} {:?}", update.step, update.total, update.percent);
        }
    }
    let _ = job.await;
}
```

A trainer outside ComfyUI writes the same object: `step`, `total`, and
optionally `phase`, `message`, `percent`, `loss`, `eta_seconds` and `previews`.
`percent` is how far through the current phase the run is, not through the
whole run, and each preview is a path relative to the progress file's
directory. `train::TrainProgress::parse` reads one, and serializing a
`TrainProgress` writes one.

## Binding an identity

Training writes the request's trigger word into the weight's sidecar, so a
prompt can pick the weight by naming it. `inventory::identities` lists every
ready image LoRA, fine-tuned checkpoint and fine-tuned diffusion model that
has a trigger and takes a scene prompt; an edit weight follows an
instruction, so a trigger never selects it. `inventory::bind_identity` makes
the one a prompt names the `Config`'s checkpoint. A trigger matches as a whole
phrase, in any case, with any run of whitespace standing for one space. When a
prompt names triggers of different lengths the longest wins; a longest
trigger two weights share, or two different triggers of that length, select
neither. `lora::identity_caption` leads a rewritten prompt with the trigger
when it no longer names it. For video, `inventory::bind_video_identity`
returns the named trigger and the ready video LoRA trained on it, when there
is exactly one.

These calls read the models directory and every sidecar in it, so they block:
call them inside `spawn_blocking`, or scan once with `inventory::scan` and ask
`inventory::identity_among` or `inventory::video_identity_among`, which only
read the items they are given.

```rust,no_run
use abnegate_comfy::{Client, Config, inventory, lora};
use tokio::sync::{broadcast, mpsc};

async fn draw(message: String, prompt: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (config, identity) = tokio::task::spawn_blocking(move || {
        let mut config = Config::from_environment();
        let identity = inventory::bind_identity(&mut config, &message);
        (config, identity)
    })
    .await?;
    let prompt = match identity {
        Some(identity) => lora::identity_caption(prompt, &identity.trigger),
        None => prompt.to_string(),
    };
    let client = Client::new(config)?;
    let (_stop, mut cancel) = broadcast::channel(1);
    let (progress, _updates) = mpsc::unbounded_channel();
    let images = client.generate(&prompt, None, &mut cancel, progress).await?;
    let _ = images;
    Ok(())
}
```

## Migrating from the pre-release defaults

Earlier builds defaulted to one application's node pack and container layout.
Every default is now neutral, and no setting falls back to its old name, so a
deployment that relied on the old defaults sets them itself.

- **Contract.** Set each `train::Contract` field the deployment relied on: the
  five node names, `folder_prefix`, `artifact_prefix`, `probe_prefix`,
  `environment_prefix`, `input_environment_prefix`, and the two that were
  constants before, `sidecar_suffix` and `publication_directory`, and
  `progress_suffix` when the training node names its progress file other
  than `<artifact>-progress.json`. Model volumes
  keep their bindings under the suffix they were written with, so keep the old
  suffix and publication directory or every existing LoRA drops out of the
  inventory.
- **Token header.** The default is `X-ComfyUI-Token`. A proxy that checks
  another header gets it through `COMFYUI_TOKEN_HEADER` or `Config::token_header`.
- **Base URL.** The default is `http://127.0.0.1:8188`. A ComfyUI reached by a
  service name sets `COMFYUI_BASE_URL`, for example `http://comfyui:8188`.
- **Paths.** The artifact root, the models directory and the packaged workflow
  paths are relative to the working directory: `./artifacts`,
  `./comfyui/models` and `./comfyui/workflows/...`. A container that keeps them
  under `/app` sets `COMFYUI_ARTIFACT_ROOT=/app/artifacts`,
  `COMFYUI_MODELS_DIRECTORY=/app/comfyui/models`, and each
  `COMFYUI_*_WORKFLOW_PATH` to its file under `/app/comfyui/workflows`.
- **Training command.** The dataset directory arrives as `<prefix>_DIRECTORY`
  (for the contract above, `ACME_TRAIN_DIRECTORY`) in place of `<prefix>_DIR`.
  The command no longer inherits the whole environment. It gets the names in
  `abnegate_exec::DEFAULT_ENVIRONMENT`, every variable already under its
  `<prefix>_`, its run's own `<prefix>_*` variables and `COMFYUI_BASE_URL`,
  and, when a token is configured, `COMFYUI_API_TOKEN` with
  `COMFYUI_TOKEN_HEADER`. Anything else it needs, it sets itself.
- **ffmpeg and ffprobe.** They get only the names in
  `abnegate_exec::DEFAULT_ENVIRONMENT`. A decoder that needs more, such as a
  library path, is pointed at through `COMFYUI_FFMPEG` or `COMFYUI_FFPROBE` as a
  wrapper script that sets it.

These environment variables are renamed, and the old names are no longer read:

| Before | After |
|---|---|
| `COMFYUI_REQUEST_TIMEOUT_SECS` | `COMFYUI_REQUEST_TIMEOUT_SECONDS` |
| `COMFYUI_GENERATION_TIMEOUT_SECS` | `COMFYUI_GENERATION_TIMEOUT_SECONDS` |
| `COMFYUI_VIDEO_GENERATION_TIMEOUT_SECS` | `COMFYUI_VIDEO_GENERATION_TIMEOUT_SECONDS` |
| `COMFYUI_AUDIO_GENERATION_TIMEOUT_SECS` | `COMFYUI_AUDIO_GENERATION_TIMEOUT_SECONDS` |
| `COMFYUI_UPSCALE_GENERATION_TIMEOUT_SECS` | `COMFYUI_UPSCALE_GENERATION_TIMEOUT_SECONDS` |
| `COMFYUI_CAPTION_TIMEOUT_SECS` | `COMFYUI_CAPTION_TIMEOUT_SECONDS` |
| `COMFYUI_CLASSIFIER_TIMEOUT_SECS` | `COMFYUI_CLASSIFIER_TIMEOUT_SECONDS` |
| `COMFYUI_TRAIN_TIMEOUT_SECS` | `COMFYUI_TRAIN_TIMEOUT_SECONDS` |
| `COMFYUI_POLL_INTERVAL_MS` | `COMFYUI_POLL_INTERVAL_MILLISECONDS` |
| `COMFYUI_MODELS_DIR` | `COMFYUI_MODELS_DIRECTORY` |
| `COMFYUI_TRAIN_FRAME_FPS` | `COMFYUI_TRAIN_FRAME_RATE` |
| `ARTIFACT_ROOT` | `COMFYUI_ARTIFACT_ROOT` |
| `<prefix>_DIR` (training command) | `<prefix>_DIRECTORY` |

In Rust, `Config::from_environment` and `from_environment_with_vision_model`
replace the constructors that abbreviated their name; the `*_timeout_seconds`
fields and `poll_interval_milliseconds` are `Duration`s named `*_timeout` and
`poll_interval`; and `frame_fps` is `frame_rate`. `inventory::scan`,
`inventory::write_sidecar` and `lora::available_bases` take the `&Contract`
they read under, and `subject::Error` is `SubjectError`. The workflow builders
spell out what they build: `build_flux_schnell_image_to_image_workflow`,
`build_wan_text_to_video_workflow` and `build_wan_image_to_video_workflow`.
Structs with public fields are `#[non_exhaustive]`, so the ones a caller passes
in are built with `video::Options::new`, `recipe::Fill::new`,
`inventory::WeightSidecar::new`, `lora::TrainRequest::new` and
`lora::TrainImage::new`. `video::Options::new` takes the frame rate and
resolution; mirroring and the frame limit default to on and 48, and
`with_mirror` and `with_limit` change them.
