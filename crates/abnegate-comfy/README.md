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

## Migrating from the pre-release defaults

Earlier builds defaulted to one application's node pack and container layout.
Every default is now neutral, and no setting falls back to its old name, so a
deployment that relied on the old defaults sets them itself.

- **Contract.** Set each `train::Contract` field the deployment relied on: the
  five node names, `folder_prefix`, `artifact_prefix`, `probe_prefix`,
  `environment_prefix`, `input_environment_prefix`, and the two that were
  constants before, `sidecar_suffix` and `publication_directory`. Model volumes
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
  `<prefix>_`, and its run's own `<prefix>_*` variables and `COMFYUI_BASE_URL`.
  Anything else it needs, it sets itself.
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
`lora::TrainImage::new`.
