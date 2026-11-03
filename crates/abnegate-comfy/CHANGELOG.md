# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

### Fixed

- A generation keeps polling through a failed history read or output download that could heal (no answer, 5xx, 408, 429) for up to `Config::outage_window`, instead of throwing a finished render away on one 500
- Each output download gets the larger of the request timeout and five minutes, within the generation deadline, so a long clip is not cut off by the client-wide request timeout
- A prompt ComfyUI no longer queues or remembers, as after a restart, fails with "it no longer has the prompt" instead of waiting out the deadline
- Any error after ComfyUI accepted a prompt deletes it from the queue, and history is cleared only once the outputs landed
- A `/history` or `/queue` answer that does not parse fails the generation at once instead of being retried as an outage, and a failed queue check is logged as one
- The queue delete sent after `Config::outage_window` gives up on a ComfyUI waits at most five seconds instead of the request timeout

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
