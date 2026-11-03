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

- `Config::video_generation_timeout` defaults to two hours (was ten minutes) and `COMFYUI_VIDEO_GENERATION_TIMEOUT_SECONDS` accepts up to four hours (was one), the time a Wan 2.2 A14B clip can take

### Removed

- Video identity binding: `inventory::{VideoIdentity, bind_video_identity, video_identity_for_prompt, video_identity_among}` and the `build_wan_adapter_text_to_video_workflow` and `build_wan_adapter_image_to_video_workflow` builders, none of them released. `Client::generate_video` posts its prompt as written and loads no LoRA, since the Wan 2.2 A14B experts cannot load a TI2V 5B adapter
- Video adapter training: every `TrainMethod::Video` request is refused with the new `lora::VIDEO_REFUSAL`, and the clip windowing behind it is gone

### Fixed

- A generation keeps polling through a failed history read or output download that could heal (no answer, 5xx, 408, 429) for up to `Config::outage_window`, instead of throwing a finished render away on one 500
- Each output download gets the larger of the request timeout and five minutes, within the generation deadline, so a long clip is not cut off by the client-wide request timeout
- A prompt ComfyUI no longer queues or remembers, as after a restart, fails with "it no longer has the prompt" instead of waiting out the deadline
- Any error after ComfyUI accepted a prompt deletes it from the queue, and history is cleared only once the outputs landed

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
