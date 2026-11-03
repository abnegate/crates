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

What the picture must not show stays out of the prompt: `Client::generate_excluding`
takes it as a negative prompt, written into the recipe's negative slot, so the
prompt describes only the scene. `None` keeps the negative text the recipe's
graph ships with (blank for FLUX and Qwen, a quality list for SD 1.5 and
SDXL), and `Some("")` clears it. `generate` is `generate_excluding` with
`None`.

```rust,no_run
use abnegate_comfy::{Client, Config, Error};
use tokio::sync::{broadcast, mpsc};

async fn bare_heads() -> Result<(), Error> {
    let client = Client::new(Config::from_environment())?;
    let (_stop, mut cancel) = broadcast::channel(1);
    let (progress, _updates) = mpsc::unbounded_channel();

    let images = client
        .generate_excluding("a crowd at a match", None, Some("hats"), &mut cancel, progress)
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
    contract.host_directory = ".acme-train".into();

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
that directory replace the packaged copies. A checkpoint that no recipe lists
in `files` or matches with a `filename_hints` entry runs on the catalog's
`default_image` recipe. A diffusion model gets no such fallback:
`inventory::scan` lists one without a sidecar only when an image recipe claims
it, so a video model such as a Wan UNet is never offered as an image base.
Besides the graph pair and its slots, a recipe may declare:

- `edit_recipe`: the base recipe an edit of a source image switches to when
  every file it requires is installed and the selected checkpoint is one of the
  recipe's own stock files (its default or one its `files` list). The packaged
  `flux-schnell` hands edits to `flux-dev`, so new pictures stay fast while
  edits get Dev's sampler budget; a custom checkpoint that `flux-schnell` only
  runs, such as a fine-tune, keeps its own weights. It must name a base image
  recipe with a source graph and the same prompt mode.
- `slots.negative`: the text input of the encoder the sampler reads as its
  negative conditioning. `recipe::Fill::with_negative` writes it, an empty
  negative clears it, and without one the graph's own text stays. A negative
  over 100,000 bytes, or one for a recipe without the slot, is refused. Every
  packaged image recipe carries one: `/7/inputs/text` on the FLUX, SD 1.5 and
  SDXL graphs and `/7/inputs/prompt` on the Qwen edit graphs. The catalog is
  refused when the slot names an input its graph lacks.
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
- `face_required_files`: exactly one IP-Adapter weight in `ipadapter/` and one
  CLIP vision encoder in `clip_vision/`, declared exactly when `extras` holds
  `face`; the sampler the seed slot fills has to read a linked `model`. The
  packaged SDXL recipes declare
  `ipadapter/ip-adapter-plus-face_sdxl_vit-h.safetensors` and
  `clip_vision/CLIP-ViT-H-14-laion2B-s32B-b79K.safetensors`.
- `training.architecture`: `flux`, `qwen_edit` or `sdxl`. SDXL has no packaged
  ComfyUI training graph and trains only through `Config::train_command` or
  the host trainer.

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
A `TrainMethod::Pivotal` LoRA, trained together with an embedding for its
trigger, is published and run like a LoRA, and only a training command or the
host trainer trains one. Its name may hold only letters, digits, dashes and
underscores, since its embedding is prompted as `embedding:<name>`. A rule's `suffix` names the published file, so
`jerry` with suffix `-alt` lands as `jerry-alt.safetensors` beside
`jerry.safetensors` rather than over it. Nothing trains a video adapter:
video generation runs the Wan 2.2 A14B experts, which load no LoRA.
`lora::trainable_bases` lists each installed base with the subjects and
methods its rules allow and that something configured can run on the local
compute, which is what a host offers the person choosing; a remote compute
trains every base but an edit base through the host trainer, and
`lora::validate_request` answers for a remote request.
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

## Training a person

A request whose subject is `TrainSubject::Person` is framed for a person
rather than squared. Each photo becomes a body crop in a bucket of its own
aspect and about a megapixel, on the 64-pixel grid SDXL needs, a head and
shoulders square unless the photo is already one, and a square on each hand
when its caption names hands. Photos are screened against 1024 pixels rather
than the packaged graph's resolution. Captions bind the trigger to the class
word, `ohwx person, standing in a kitchen`, and the vision model is asked for
the pose first, so the weight does not learn one pose as part of the person. A
person set the captions read as several people is refused before anything
trains; for any other subject that finding stays advice.

Beside every `NNNN.png` and its caption the dataset carries companions for the
trainer, named by `abnegate_comfy::dataset`:

| File | Holds |
|---|---|
| `NNNN.kind` | `body`, `head` or `hand` (`dataset::FrameKind`) |
| `NNNN.pose` | the pose its caption names, such as `standing` or `on back`, the first in `dataset::pose_clusters` order when it names several, or `unknown` (`dataset::pose_cluster`) |
| `NNNN.mask.png` | the subject as an 8-bit mask, 255 on the subject, when the `saliency` model located it |
| `rebalance.json` | `{"clusters": {pose: frames}, "weights": {stem: weight}}` (`dataset::Rebalance`) |

A set where one pose holds more than 40% of the frames (`REBALANCE_SHARE`)
draws each frame in inverse proportion to its pose's size, so the rarest pose
keeps weight 1; otherwise every weight is 1.

`Config::poses_directory` (`COMFYUI_POSES_DIRECTORY`) names a directory of
OpenPose images a person's generation can be posed with, one per pose cluster,
named by `dataset::pose_filename`: `standing.png`, `on_back.png`,
`all_fours.png`. The crate ships none.

## Generating a person

A selected weight whose sidecar names a `trigger` is an identity, and
`Client::generate` gives it the extras its recipe supports, each only once its
files are installed and never as an error when they are not:

- **Pose.** A new picture (not an edit) through a recipe with a pose slot is
  posed with the image in `Config::poses_directory` for the last pose the
  prompt names, standing when it names none, uploaded as `pose-<file>`.
- **Face.** A recipe with the `face` extra routes its sampler's model through
  `Contract::ipadapter_face_node` (`AbnegateIPAdapterFace` by default), fed the
  face reference beside the weight: the file the sidecar's `face` names, else
  `<stem>.face.png` (`inventory::FACE_SUFFIX`). The node takes `model`,
  `clip_vision`, `image`, `ipadapter_name` and `strength` (0.4) and returns
  the model; the crate ships no node pack, so the deployment registers one.
- **Embedding.** The sidecar's `embedding` leads the prompt as
  `embedding:<stem>`, unless the prompt already holds it.

A recipe with `refine`, or `identity_refine` for an identity, then reruns the
head of every result through its source graph at denoise 0.3 and pastes it
back: the head and shoulders square a person's training set would crop (the
whole image when that square is all of it), placed on the subject when
`Config::vision_model` is set. The refine runs within the same generation
time limit as the first pass. A refine pass that fails, or that the time limit
cuts short, keeps the unrefined image, and no refine starts with less than one
`Config::poll_interval` left; once the first pass has its results only a
cancellation fails the call. The pose and face images are read without
following a link or waiting on a pipe, up to the source-image limit.

## Generating video

`Client::generate_video` runs Wan 2.2 A14B: a high-noise 14B expert samples
the first half of the steps and a low-noise one the rest, at 1280×704, 81
frames, 16 frames a second. The packaged graphs are
`wan2.2-a14b-t2v-api.json` and `wan2.2-a14b-i2v-api.json`;
`Config::video_workflow_path` replaces the text-to-video one when it keeps the
A14B contract, and an image-to-video graph of that name beside it, with
`WanImageToVideo` at 7 starting from the image `LoadImage` at 11 loads,
directly or through nodes that pass an image on, replaces the other. The contract
is a `UNETLoader` or `UnetLoaderGGUF` at nodes 1 and 12, `KSamplerAdvanced` at
8 and 14, `EmptyHunyuanLatentVideo` at 7 and `SaveWEBM` at 10, with 8 sampling
the model node 1 loads, 14 sampling the one node 12 loads, through any nodes
that pass a model on, and 14 continuing from 8's latent. A configured graph that cannot be read or breaks the
contract fails the request; only the graph a request uses is read, before
anything is uploaded. A leftover `wan2.2-ti2v-5b-api.json` path loads the
A14B graph beside it, or the packaged one.

`Config::video_unet` names either expert of a pair, such as
`wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors`, the default; the other
expert, and the image-to-video pair, are derived from its name in its own
casing. A name is a Wan 2.2 noise expert only when it names Wan, a noise
level and a `t2v` or `i2v` mode with no letter or digit beside the mode, so
`ti2v` and `t2v14b` are not modes. Any other name, such as the 5B
`wan2.2_ti2v_5B_fp16.safetensors`, loads the packaged pair, and the 5B
`wan2.2_vae.safetensors` loads `wan_2.1_vae.safetensors`, the VAE the experts
decode with.

A clip can take hours, so `Config::video_generation_timeout` is two hours
unless set and at most four. Every generation rides out a failing ComfyUI: a
history poll or output download that gets no answer, or a 5xx, 408 or 429, is
retried until such failures have lasted `Config::outage_window`, two minutes;
while history stays empty, the queue is asked every
`Config::queue_check_interval`, thirty seconds, whether it still holds the
prompt, and a prompt ComfyUI lost to a restart fails. Each download may take
the larger of the request timeout and five minutes. Any failure after
ComfyUI accepted a prompt deletes it from the queue, and history is cleared
only once the outputs landed. The call waits at most five seconds for that
delete and leaves a slower one to finish within the request timeout.

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
neither, and neither does a weight whose filename another model folder
also holds, since selecting that name could load the other.
`lora::identity_caption` leads a rewritten prompt with the trigger
when it no longer names it. No identity is bound to video:
`Client::generate_video` posts its prompt as written, so a trigger in it is
plain text, and no LoRA is loaded onto the video model.

These calls read the models directory and every sidecar in it, so they block:
call them inside `spawn_blocking`, or scan once with `inventory::scan` and ask
`inventory::identity_among`, which only reads the items it is given.

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

## Training on the host

Some training runs too long, or needs more of the machine, than a ComfyUI
graph should hold. `Config::host_trainer` hands it to a trainer that runs
beside ComfyUI on the host and shares its models directory. When no
`Config::train_command` is set, every method other than a LoRA and every SDXL
base goes there; a LoRA on any other base still runs the packaged graph. An
edit base is refused on the host, since the job does not carry its reference
images or its text encoder and VAE; it trains through a training command or,
as a LoRA, the packaged graph.

A request can also name where the host trainer runs it,
`TrainRequest::with_compute`. `host::Compute::Local`, the default, is the
host's own machine. A remote compute, `host::Compute::Runpod`, sends every
subject and method to the host trainer, ahead of any training command, and
needs none of the base's files installed here, since the pod pulls the base
from Hugging Face. Its key, `TrainRequest::with_compute_key`, is written to
the compute's key file beside the job, never into `job.json`; a remote
request without one is refused before any image is read. Which subjects may
use a remote compute, and where its key is kept, is the host's to decide.
Screening, cropping and captioning happen here as for any run; the trainer
only trains. The finished weight is published here too, under the same
protections as any other: the policy's directory and recipe, never over a
weight a recipe or rule needs or one no training run published, behind the
same publication lock and marker.

The host trainer runs on its own, so it keeps its own environment. A
`Config::train_command` is spawned from here instead, and gets only
`Config::trainer_environment` of this process's environment: the default
allowlist, unless a deployment allows the settings its trainer reads.

```rust,no_run
use abnegate_comfy::Config;
use abnegate_comfy::host::{self, Compute, Trainer};
use abnegate_comfy::lora::{self, TrainError, TrainMethod, TrainRequest, TrainSubject};
use abnegate_exec::EnvironmentPolicy;
use abnegate_secret::SecretValue;
use std::time::Duration;

fn configured() -> Config {
    let mut config = Config::from_environment()
        .with_host_trainer(Trainer::new().with_pickup(Duration::from_secs(300)))
        .with_trainer_environment(EnvironmentPolicy::allowlist().allow(["ACME_PROBE_STEPS"]));
    config.contract.host_directory = ".acme-train".into();
    config
}

fn startup(config: &Config) {
    for recovered in host::recover(config) {
        println!("{recovered:?}");
    }
}

async fn person(
    config: &Config,
    request: TrainRequest,
    key: SecretValue,
) -> Result<(), TrainError> {
    let request = request
        .with_subject(TrainSubject::Person)
        .with_method(TrainMethod::Finetune)
        .with_compute(Compute::Runpod)
        .with_compute_key(key)
        .with_host_field("tier", "overnight");
    let outcome = lora::train(config, String::new(), SecretValue::new(""), request).await?;
    println!("{}", outcome.path.display());
    Ok(())
}
```

### The job directory

Each job is a directory named by its id under
`<models>/<Contract::host_directory>/`, `.abnegate-train` by default, readable
only by its owner, so the trainer runs as the same user. The crate writes
everything else first and `job.json` last, so a trainer never finds half a
job. Every file the crate writes there is written in full under a unique
temporary name, flushed, and renamed into place:

| Path | Written by | Holds |
|---|---|---|
| `job.json` | both | the job, below |
| `progress.json` | both | one progress object, below; the crate writes the queued one |
| `previews/` | trainer | the samples `progress.json` lists in `previews` |
| `dataset/` | crate | `NNNN.png` with its caption `NNNN.txt`, and for a person set `NNNN.kind`, `NNNN.pose`, `NNNN.mask.png` and `rebalance.json` |
| `data/` | host | text records, `train.jsonl` and `valid.jsonl`, laid out as `format.json` says |
| `<filename>` | trainer | the trained weight, which the crate then publishes and removes |
| `publish.lock` | crate | locked by whoever publishes the job, from queueing it until the weight is published; the trainer leaves it alone |
| `claimed` | trainer or crate | empty; created exclusively by whoever moves a queued job on: the trainer before it writes `running`, or the crate before it gives up on the job |
| `dismissed` | crate | empty; marks a finished job `host::dismiss` hid |
| `runpod.key` | crate | the key of a `Compute::Runpod` job, one line, mode `0600`, listed in `secrets` and removed when the job ends |
| any other file | crate | a secret from `TrainRequest::with_host_secret`, one line, mode `0600`, listed in `secrets` and removed when the job ends |

Once a job has failed, or its weight is published, `dataset/` and `data/` are
removed; `job.json`, `progress.json` and `previews/` stay for a host to show
until `host::remove` takes the whole job away.

`job.json` is one object, `schema_version` 1:

| Key | Meaning |
|---|---|
| `schema_version` | `1` |
| `id` | the job's UUID, also its directory name |
| `name` | the name the weight was requested under |
| `method` | `lora`, `finetune`, `pivotal`, or a method of the host's own |
| `subject` | `other`, `person`, a subject of the host's own, or absent |
| `trigger` | the identity weight's trigger word, or empty |
| `architecture` | the base's model family, `flux` or `sdxl`, as `<prefix>_ARCHITECTURE` names it for a training command |
| `checkpoint` | the base weight file to start from |
| `base` | the base recipe id; empty for a job the crate does not publish |
| `provider` | where the trainer runs the job, `local` or `runpod`, `Job::compute`; `local` when absent |
| `gpu` | the GPU a remote compute runs the job on, stamped by the trainer |
| `pod_id` | the pod a remote compute runs the job on, stamped by the trainer |
| `status` | `queued`, `running`, `succeeded` or `failed` |
| `filename` | the file to write the weight to, and the name it is published under |
| `recipe_id` | the recipe that runs the published weight |
| `hf_base` | the Hugging Face model of the base, or empty |
| `image_count` | images in `dataset/` |
| `error` | why a `failed` job failed |
| `pid` | the trainer process of a `running` job, `Job::process_id` |
| `started_at` | RFC 3339 time the job was queued |
| `published` | where the crate published the weight, relative to the models directory, once it has |
| `secrets` | the secret files beside `job.json`, removed when the job ends |

Any other key is the host's (`TrainRequest::with_host_field`) or the
trainer's, and is kept when the crate rewrites the file. A `job.json` with a
`schema_version` newer than the crate reads is refused. The trainer takes
`queued` jobs oldest first, marks one `running` with its `pid`, then reads
`job.json` back and leaves the job alone if it says `failed`, and ends it
`succeeded` once the weight is at `<filename>`, or `failed` with an `error`. A
`running` job whose process is gone is the trainer's to resume. A job no
trainer starts within `Trainer::pickup`, counted from the last moment another
job was `running`, is marked `failed` and the run fails; a job queued behind a
running one waits as long as that one runs.

`progress.json` is the same object a training node writes: `step` and
`total`, and optionally `phase`, `message`, `percent`, `loss`,
`eta_seconds` and `previews`, read and written as `train::TrainProgress`.
`host::Trainer::steps_for` is the step budget a queued job reports before the
trainer says otherwise; set it with `with_steps` to match the trainer's
configuration.

### After a restart

The job outlives the process that queued it. `host::current_with_progress`
returns the latest job with its progress for a host to show, until
`host::dismiss` hides it once it has ended and removes its secrets, and
`host::preview` resolves one
of its samples. `host::resume` waits on a job an
earlier process queued and publishes it, and `host::resume_reporting` keeps
its latest progress in a `watch` sender the way `lora::train_reporting` does;
`host::wait` and `host::wait_reporting` wait without publishing.
`host::recover`, called once at startup, publishes every job that succeeded
while nobody waited, after checking its base, method, subject and weight name
against the policy again, removes the secrets of every job that ended, and
removes a job directory with no `job.json` that nobody holds, which a crash
left between writing its secrets and queueing it;
`host::resume` of a job that ended, and queueing a new job, remove them too, so
a key never waits for a restart once the run waiting on its job was dropped.
Whoever publishes a job holds its `publish.lock` and records `published` in
`job.json` before removing the weight, so a job is published once: `recover`
skips a job another waiter holds and one already published, and `resume` of a
published job returns where it went. Either way the weight's sidecar carries
the job's `trigger`, so a prompt that names it binds the weight as it would
one trained here. A `pivotal` job's trainer writes the embedding it trains to
the models directory's `embeddings/` under the job's `filename`, and the
sidecar names it as `embedding`, so `embedding:<stem>` leads every prompt run
on the weight. A pivotal job whose embedding is not a regular file there,
written since the job's `started_at`, fails instead of being published.

## Migrating from the pre-release defaults

Earlier builds defaulted to one application's node pack and container layout.
Every default is now neutral, and no setting falls back to its old name, so a
deployment that relied on the old defaults sets them itself.

- **Contract.** Set each `train::Contract` field the deployment relied on: the
  five node names, `folder_prefix`, `artifact_prefix`, `probe_prefix`,
  `environment_prefix`, `input_environment_prefix`, and the two that were
  constants before, `sidecar_suffix` and `publication_directory`, and
  `progress_suffix` when the training node names its progress file other
  than `<artifact>-progress.json`, `host_directory` when a host trainer
  watches a directory other than `.abnegate-train`, and
  `ipadapter_face_node` when its face node is not `AbnegateIPAdapterFace`. Model volumes
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
`inventory::write_sidecar` and the deprecated `lora::available_bases` take
the `&Contract` they read under, and `subject::Error` is `SubjectError`. The workflow builders
spell out what they build: `build_flux_schnell_image_to_image_workflow`,
`build_wan_text_to_video_workflow` and `build_wan_image_to_video_workflow`.
Structs with public fields are `#[non_exhaustive]`, so the ones a caller passes
in are built with `video::Options::new`, `recipe::Fill::new`,
`inventory::WeightSidecar::new`, `lora::TrainRequest::new` and
`lora::TrainImage::new`. `video::Options::new` takes the frame rate and
resolution; mirroring and the frame limit default to on and 48, and
`with_mirror` and `with_limit` change them.
