#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! ComfyUI integration: image, video, and audio generation, upscaling, model
//! inventory, and LoRA training.
//!
//! The crate talks to a ComfyUI server over HTTP and owns nothing else. It has
//! no web framework, database, or application state, so it can be dropped into
//! any project that needs image generation or wants to train a LoRA.
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use abnegate_comfy::{Client, Config};
//! use tokio::sync::{broadcast, mpsc};
//!
//! let client = Client::new(Config::from_environment())?;
//! let (_stop, mut cancel) = broadcast::channel(1);
//! let (progress, _updates) = mpsc::unbounded_channel();
//! let images = client
//!     .generate("a lighthouse in a storm", None, &mut cancel, progress)
//!     .await?;
//! # let _ = images;
//! # Ok(())
//! # }
//! ```
//!
//! Training a LoRA writes the dataset, captions any image left blank, runs the
//! packaged training graph on the configured ComfyUI server, then scores every
//! checkpoint the run produced and keeps the best one:
//!
//! ```no_run
//! # async fn example(request: abnegate_comfy::lora::TrainRequest) -> Result<(), Box<dyn std::error::Error>> {
//! use abnegate_comfy::{Config, lora};
//!
//! let config = Config::from_environment();
//! let outcome = lora::train(&config, litellm_host(), litellm_key(), request).await?;
//! # let _ = outcome;
//! # Ok(())
//! # }
//! # fn litellm_host() -> String { String::new() }
//! # fn litellm_key() -> abnegate_secret::SecretValue { abnegate_secret::SecretValue::new("") }
//! ```
//!
//! A clip can stand in for that image set. [`video::extract`] samples it above the
//! rate the caller asked for, keeps the sharpest frame of each moment, drops
//! the ones that repeat a shot already taken, and crops what is left around
//! whatever moved:
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use abnegate_comfy::{Config, video};
//!
//! let clip = video::extract(
//!     &Config::from_environment(),
//!     &std::fs::read("subject.mp4")?,
//!     "subject.mp4",
//!     video::Options::new(4, 512, true, 48),
//! )
//! .await?;
//! # let _ = clip;
//! # Ok(())
//! # }
//! ```
//!
//! The training graphs call custom nodes that ComfyUI does not ship, and those
//! nodes only accept run folders named under the namespaces they know.
//! [`Config::contract`] names both, together with the sidecar and publication
//! names the inventory reads. Its defaults, [`train::Contract`], sit under a
//! neutral `Abnegate` namespace, and a deployment whose node pack registers
//! other names overrides them there.
//!
//! A host that collects metrics installs [`observe_requests`] once at startup;
//! without it the crate records nothing and pulls in no metrics stack.
//!
//! # Features
//!
//! - `saliency`: frames training crops on the subject U2-Net finds, through
//!   `abnegate-vision` on ONNX Runtime, when [`Config::vision_model`] points at
//!   the weights. Without it a photo is cropped on its centre and a video frame
//!   on whatever moved. Off by default.

mod caption;
mod client;
mod config;
mod dataset;
mod excerpt;
mod http;
pub mod inventory;
pub mod lora;
mod media;
mod observe;
pub mod quality;
pub mod recipe;
mod screening;
pub mod subject;
pub mod train;
pub mod video;

pub use caption::{CaptionImage, CaptionRequest, Captioner, Draft, data_url};
pub use client::{
    Client, Error, GeneratedImage, MAXIMUM_SOURCE_IMAGE_BYTES, MAXIMUM_SOURCE_VIDEO_BYTES,
    SourceImage, SourceVideo, build_ace_step_workflow, build_flux_schnell_image_to_image_workflow,
    build_flux_schnell_workflow, build_upscale_image_workflow, build_upscale_video_workflow,
    build_wan_image_to_video_workflow, build_wan_text_to_video_workflow,
};
pub use config::{Config, ConfigError, TOKEN_HEADER, VISION_MODEL_VARIABLE};
pub use dataset::{Concern, Finding, inspect};
pub use media::MediaType;
pub use observe::{RequestObserver, observe_requests};
pub use screening::{Rejection, Verdict, screen};

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
