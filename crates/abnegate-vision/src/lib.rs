#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Subject-aware image cropping.
//!
//! Finds the visual subject of an image and frames a crop on it, so a pipeline
//! keeps the subject instead of whatever happened to be in the middle of the
//! frame.
//!
//! The detection half reproduces [autogravity], a Go service that locates
//! subjects with U2-Net, down to its resampling and its normalization; the
//! cropping that service left to its callers is here too.
//!
//! Detection runs U2-Net through ONNX Runtime and sits behind the `saliency`
//! feature, because linking the runtime is expensive enough that a caller who
//! only wants the geometry should not have to pay for it. Decoding, crop
//! planning, and rendering are always available, so a caller that already knows
//! where the subject is can crop without the model.
//!
//! The crop, the downscale, and any EXIF rotation happen in one resampling pass
//! over the source, so no full-size intermediate is ever built.
//!
//! # With the model
//!
//! ```no_run
//! # #[cfg(feature = "saliency")] {
//! use abnegate_vision::{Analyzer, Target};
//!
//! let analyzer = Analyzer::open("models/u2net.onnx")?;
//! let crop = analyzer.crop(&std::fs::read("photo.jpg")?, Target::square(1024))?;
//! assert_eq!(crop.image.pixels.len(), 1024 * 1024 * 3);
//! # }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Wrap the `Analyzer` in an `Arc` rather than building one per worker: it
//! holds the ONNX Runtime session and its arena, which is where nearly all of
//! the process's resident memory goes. The `u2net.onnx` export is roughly
//! 168 MiB and is not vendored.
//!
//! # Without it
//!
//! ```no_run
//! use abnegate_vision::{Error, Point, Rendered, Target, crop, decode};
//!
//! fn frame(data: &[u8]) -> Result<Rendered, Error> {
//!     let raster = decode::decode(data)?;
//!     let focus = Point { x: 0.5, y: 0.33 };
//!     let region = crop::plan(raster.oriented_size(), Target::square(1024), focus)?;
//!     Ok(crop::render(&raster, region, Target::square(1024))?)
//! }
//! # frame(&std::fs::read("photo.jpg")?)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Features
//!
//! - `saliency`: `Analyzer` and the `saliency` module, subject detection with
//!   U2-Net over ONNX Runtime. Off by default. The build downloads ONNX
//!   Runtime's prebuilt binaries; a consumer that ships its own runtime
//!   enables `ort/load-dynamic` to load it at run time instead.
//!
//! [autogravity]: https://github.com/appwrite/autogravity

pub mod crop;
pub mod decode;
pub mod gravity;
pub mod preprocess;

mod error;

pub use crate::crop::{CropError, Region, Rendered, Target};
pub use crate::decode::{DecodeError, Raster};
pub use crate::error::Error;
pub use crate::gravity::{GravityError, Point};
pub use crate::preprocess::PreprocessError;

#[cfg(feature = "saliency")]
#[cfg_attr(docsrs, doc(cfg(feature = "saliency")))]
pub mod saliency;

#[cfg(feature = "saliency")]
mod analyzer;
#[cfg(feature = "saliency")]
mod exclusive;

#[cfg(feature = "saliency")]
#[cfg_attr(docsrs, doc(cfg(feature = "saliency")))]
pub use crate::analyzer::{Analyzer, AnalyzerError, Crop, Focus};
#[cfg(feature = "saliency")]
#[cfg_attr(docsrs, doc(cfg(feature = "saliency")))]
pub use crate::saliency::SaliencyError;

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
