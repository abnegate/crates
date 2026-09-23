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
//! let client = Client::new(Config::from_env())?;
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
//! # async fn example(request: abnegate_comfy::TrainRequest) -> Result<(), Box<dyn std::error::Error>> {
//! use abnegate_comfy::{Config, lora};
//!
//! let config = Config::from_env();
//! let outcome = lora::train(&config, litellm_host(), litellm_key(), request).await?;
//! # let _ = outcome;
//! # Ok(())
//! # }
//! # fn litellm_host() -> String { String::new() }
//! # fn litellm_key() -> abnegate_secret::SecretValue { abnegate_secret::SecretValue::new("") }

//! ```
//!
//! A clip can stand in for that image set. [`extract`] samples it above the
//! rate the caller asked for, keeps the sharpest frame of each moment, drops
//! the ones that repeat a shot already taken, and crops what is left around
//! whatever moved:
//!
//! ```no_run
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! use abnegate_comfy::{Config, video};
//!
//! let clip = video::extract(
//!     &Config::from_env(),
//!     &std::fs::read("subject.mp4")?,
//!     "subject.mp4",
//!     video::Options { fps: 4, resolution: 512, mirror: true, limit: 48 },
//! )
//! .await?;
//! # let _ = clip;
//! # Ok(())
//! # }
//! ```
//!
//! The training graphs call custom nodes that ComfyUI does not ship, and those
//! nodes only accept run folders named under the namespaces they know.
//! [`Config::contract`] names both; its defaults, [`train::Contract`], match
//! the node pack this crate was written against, and a deployment with its own
//! pack overrides them there.
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

pub mod caption;
pub mod client;
pub mod config;
pub mod dataset;
mod http;
pub mod inventory;
pub mod lora;
pub mod media;
pub mod observe;
pub mod quality;
pub mod recipe;
pub mod screening;
pub mod subject;
pub mod train;
pub mod video;

pub use caption::{CaptionImage, CaptionRequest, Captioner, Draft, data_url};
pub use client::{Client, Error, GeneratedImage, SourceImage, SourceVideo};
pub use config::{Config, ConfigError};
pub use dataset::{Concern, Finding, inspect};
pub use inventory::{InventoryItem, WeightSidecar, scan};
pub use lora::{
    TrainBase, TrainError, TrainImage, TrainOutcome, TrainRequest, available_bases, train,
};
pub use media::MediaType;
pub use observe::{RequestObserver, observe_requests};
pub use quality::Quality;
pub use recipe::{PromptMode, Recipe, RecipeCatalog, sanitize_weight_filename};
pub use screening::{Rejection, Verdict, screen};
pub use subject::Subject;
pub use train::Contract;
pub use video::{Clip, Frame, FrameRequest, extract};
