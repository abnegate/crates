# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.2](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.3.1...abnegate-comfy/v0.3.2) - 2026-11-12

### Other

- updated the following local packages: abnegate-exec

## [0.3.1](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.3.0...abnegate-comfy/v0.3.1) - 2026-11-10

### Added

- *(agent)* connect streamable-http mcp servers and report each server's outcome ([#81](https://github.com/abnegate/crates/pull/81))

## [0.3.0](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.2.4...abnegate-comfy/v0.3.0) - 2026-11-09

### Fixed

- *(deps)* [**breaking**] move rmcp past its advisories and drop the rusqlite feature ([#78](https://github.com/abnegate/crates/pull/78))

## [0.2.4](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.2.3...abnegate-comfy/v0.2.4) - 2026-11-07

### Added

- *(llm)* add RerankProvider and an HTTP rerank adapter ([#73](https://github.com/abnegate/crates/pull/73))

## [0.2.3](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.2.2...abnegate-comfy/v0.2.3) - 2026-11-06

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec

## [0.2.2](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.2.1...abnegate-comfy/v0.2.2) - 2026-11-05

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec

## [0.2.1](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.2.0...abnegate-comfy/v0.2.1) - 2026-11-03

### Added

- *(comfy)* so a host can give its captioner the completion timeouts the rest of its completions get

### Fixed

- *(comfy)* so a model still loading on its first caption does not leave the whole set trigger-only
- *(comfy)* so a dead caption endpoint costs a training run one caption timeout, not one per shot

### Other

- *(comfy)* so the caption strike count is proven to start over after an answer
- *(comfy)* so the caption timeout docs claim only what the client does
- *(comfy)* so a host knows the training budget starts after captioning

## [0.2.0](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.8...abnegate-comfy/v0.2.0) - 2026-11-03

### Added

- *(comfy)* [**breaking**] so a video renders with the wan 2.2 a14b expert pair
- *(comfy)* [**breaking**] so a video render gets the hours a wan 2.2 a14b clip takes
- *(comfy)* so a host can keep what a picture must not show out of its prompt
- *(comfy)* so a host can send any training job to a rented pod and drop a finished one
- *(comfy)* so an identity's picture keeps its pose and its face
- *(comfy)* so a person can be trained into a video adapter and a video prompt picks it up
- *(comfy)* so a person trains on crops of its own aspect with the companions the host trainer reads
- *(comfy)* so a trainer on the host can take the runs ComfyUI cannot hold
- *(comfy)* so a prompt that names a trained identity selects its weight
- *(comfy)* so a host can watch a training run it left running in the background
- *(comfy)* so a host can upload raw training sets and keep its training rules to itself
- *(comfy)* so edits, SDXL and Wan adapters run from the packaged catalog

### Fixed

- *(comfy)* so a pivotal embedding written in its run's first second is not refused
- *(comfy)* so a claim a failed give-up could not release is logged
- *(comfy)* so a missing embedding is warned about once, not on every generation
- *(comfy)* so a filename shared in another case also binds no identity
- *(comfy)* so a job being queued while recover sweeps is never lost or failed
- *(comfy)* so recording an unrecorded publication first settles a promotion a crash left
- *(comfy)* so a pose slot no controlnet reads is refused when the catalog loads
- *(comfy)* so a prompt leads with an embedding token only when that embedding is installed
- *(comfy)* so a host job published but not recorded is never promoted twice
- *(comfy)* so a pivotal lora is published only with the embedding its own run wrote
- *(comfy)* so a checkpoint whose sidecar names an adapter recipe never loads as a lora
- *(comfy)* so a trigger never binds a weight whose filename another weight shares
- *(comfy)* so the startup sweep never discards a job queued while it looked
- *(comfy)* so a waiter that cannot mark a job failed lets the trainer still claim it
- *(comfy)* so an image-to-video graph animates the image the caller uploaded
- *(comfy)* so a failed generation returns on time while its cancel is still landing
- *(comfy)* so a pivotal lora is never published under a name its embedding cannot be prompted by
- *(comfy)* so a custom a14b graph or unet name that would load the wrong experts is refused
- *(comfy)* so a garbled queue answer fails at once and a dead comfyui is not waited on twice
- *(comfy)* so a host-trained pivotal lora keeps its embedding prefix
- *(comfy)* [**breaking**] so a video unet is never offered as an image base
- *(comfy)* so video stops binding identities and training adapters the a14b experts cannot load
- *(comfy)* so a long render survives a failed poll or a slow download and gives up on a comfyui that lost it
- *(comfy)* so a waiter never gives up a job the trainer already claimed
- *(comfy)* so a generation reads its pose and face safely and a stuck disk never holds it
- *(comfy)* so flux-dev edits encode the starting image

### Other

- *(comfy)* so the trainer contract says which way the clock may drift and what the tolerance lets through
- *(comfy)* so the trainer contract says when a pivotal embedding must land and what its age check assumes
- *(comfy)* so the queue-delete wait and the LoadImage class each have one name
- *(comfy)* so trainable_bases says it lists what trains on the local compute
- *(comfy)* so the changelog and readme state the queue, outage and expert-name rules
- *(comfy)* so a lost connection and a failed download are proven to be retried
- *(comfy)* so the changelog marks every change to released behaviour as breaking
- *(comfy)* so no video-adapter api that nothing can run ships in a release
- *(comfy)* so no test names the application a job layout came from

### Added

- `Client::generate_excluding` takes a negative prompt, so what a picture must not show stays out of its prompt; `generate` is unchanged
- `recipe::Fill::with_negative` and the catalog's `slots.negative`, which every packaged image recipe now declares
- `Config::outage_window` and `Config::queue_check_interval`, with `with_outage_window` and `with_queue_check_interval`, bound how long a generation rides out a failing ComfyUI and how often it confirms an unfinished prompt is still queued

### Changed

- **Breaking:** `Client::generate_video` and `build_wan_text_to_video_workflow` / `build_wan_image_to_video_workflow` run Wan 2.2 A14B, a high-noise and a low-noise 14B expert, from the packaged `wan2.2-a14b-t2v-api.json` and `wan2.2-a14b-i2v-api.json`, in place of TI2V 5B, whose graphs are no longer packaged. The builders keep their signatures but emit graphs that load different model files: a configured `unet` that is not a Wan 2.2 noise expert, such as the 5B one, loads the packaged pair, and the 5B `wan2.2_vae.safetensors` loads `wan_2.1_vae.safetensors`
- **Breaking:** `Config::video_unet` defaults to `wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors` and is paired with its other expert; a name is an expert only when it names Wan, a noise level and a `t2v` or `i2v` mode with no letter or digit beside it, so `ti2v` or `t2v14b` is not one; `Config::video_vae` defaults to `wan_2.1_vae.safetensors`, and `COMFYUI_VIDEO_WORKFLOW_PATH` defaults to `./comfyui/workflows/wan2.2-a14b-t2v-api.json`; a leftover `wan2.2-ti2v-5b-api.json` path loads the A14B graph beside it
- **Breaking:** a configured video graph that is missing, cannot be read or breaks the A14B contract fails the request instead of falling back to the packaged one, and only the graph a request uses is read, before its source is uploaded
- **Breaking:** `Config::video_generation_timeout` defaults to two hours (was ten minutes) and `COMFYUI_VIDEO_GENERATION_TIMEOUT_SECONDS` accepts up to four hours (was one), the time a Wan 2.2 A14B clip can take
- **Breaking:** `inventory::scan` lists a diffusion model without a sidecar only when an image recipe claims it through `files` or `filename_hints`; before, one no recipe claimed was listed as a ready base of the default image recipe, so a Wan UNet was offered as an image base
- **Breaking:** `RecipeCatalog::is_explicit_image_match` is false for a file a video recipe claims through its `files` or `filename_hints`, even when an image recipe's hint also matches it
- `publication` is a reserved name in a host job directory, beside `job.json`, `progress.json`, `publish.lock`, `claimed` and `dismissed`: the crate notes there the generation it is publishing the job's weight under, so `TrainRequest::with_host_secret` refuses a secret of that name in any case

### Fixed

- A generation keeps polling through a failed history read or output download that could heal (no answer, 5xx, 408, 429) for up to `Config::outage_window`, instead of throwing a finished render away on one 500
- Each output download gets the larger of the request timeout and five minutes, within the generation deadline, so a long clip is not cut off by the client-wide request timeout
- A prompt ComfyUI no longer queues or remembers, as after a restart, fails with "it no longer has the prompt" instead of waiting out the deadline
- Any error after ComfyUI accepted a prompt deletes it from the queue, and history is cleared only once the outputs landed
- A `/history` or `/queue` answer that does not parse fails the generation at once instead of being retried as an outage, and a failed queue check is logged as one
- The queue delete sent after `Config::outage_window` gives up on a ComfyUI waits at most five seconds instead of the request timeout
- A catalog whose `slots.pose` names a `LoadImage` no `ControlNetApply` or `ControlNetApplyAdvanced` reads is refused, instead of uploading a pose the sampler never sees
- A prompt is led by `embedding:<stem>` only when the embedding the weight's sidecar names is installed in `embeddings/`; a missing one is logged as a warning instead of being dropped by ComfyUI without a word
- A host job whose weight was published but whose `job.json` could not record it is recorded by the next `recover` or `resume` without promoting the weight again under a new generation
- A pivotal host job is published only when its embedding is a regular file in `embeddings/` last modified no more than two seconds before the job started, so a weight never names an embedding that is missing or left by an earlier run, while one stamped by a coarse filesystem clock in the run's first moments is still taken
- `inventory::scan` reads a checkpoint or diffusion model whose sidecar names an adapter recipe as if the sidecar named no recipe, so it is never listed as an adapter, bound as an identity and then loaded into a LoRA slot
- `inventory::bind_identity` and `identity_among` select no weight whose filename another scanned weight shares in any ASCII case, such as a LoRA and a checkpoint of one name, since generation would load whichever the scan listed first
- `host::recover` takes a job directory's lock before it looks for `job.json`, and every job directory is moved aside under a dot-name before it is removed; queueing makes its directory again when a sweep removed it before it was locked. A job queued while `recover` runs is neither removed with its inputs and secrets nor failed
- A waiter that wins a queued host job's claim but cannot mark the job failed lets the claim go, so the trainer can still start the job instead of finding it claimed forever
- A configured image-to-video graph whose `WanImageToVideo` start image does not come from the `LoadImage` at node 11, which receives the caller's upload, is refused instead of animating some other image
- A generation that is cancelled, times out or fails after ComfyUI accepted its prompt returns within five seconds of sending the queue delete; a slower delete carries on to the request timeout on its own

## [0.1.8](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.7...abnegate-comfy/v0.1.8) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec

## [0.1.7](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.6...abnegate-comfy/v0.1.7) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec, abnegate-llm

## [0.1.6](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.5...abnegate-comfy/v0.1.6) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec, abnegate-llm

## [0.1.5](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.4...abnegate-comfy/v0.1.5) - 2026-11-03

### Other

- *(release)* nest git tags as crate/vversion ([#15](https://github.com/abnegate/crates/pull/15))

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.3...abnegate-comfy/v0.1.4) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret, abnegate-exec, abnegate-llm

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.2...abnegate-comfy/v0.1.3) - 2026-10-01

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.1...abnegate-comfy/v0.1.2) - 2026-09-30

### Fixed

- *(comfy)* refuse a blank or control-character decoder name
- *(comfy)* join only single path segments onto the models directory

### Other

- describe four items as the merged port left them
- *(comfy)* document every public item

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.0...abnegate-comfy/v0.1.1) - 2026-09-30

### Other

- Merge branch 'main' into release/trusted-publishing
- *(release)* tag, release and version each crate on its own
