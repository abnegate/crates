//! The public entry point: find an image's subject, then crop to it.

use std::path::Path;

use crate::crop::{Target, plan, render};
use crate::decode::{self, Raster};
use crate::exclusive::Exclusive;
use crate::gravity::Rect;
use crate::gravity::from_saliency_region;
use crate::preprocess::Preprocessor;
use crate::saliency::{INPUT_HEIGHT, INPUT_WIDTH, Model};

mod crop;
mod error;
mod focus;

pub use crate::analyzer::crop::Crop;
pub use crate::analyzer::error::AnalyzerError;
pub use crate::analyzer::focus::Focus;

/// Finds image subjects against one shared model.
///
/// Preprocessing scratch is pooled rather than allocated per image, so a warm
/// analyzer does no image-sized heap work outside decoding. Cloning is not
/// supported on purpose: wrap it in an `Arc` so every caller shares one session
/// and one ONNX Runtime arena.
pub struct Analyzer {
    model: Model,
    scratch: Exclusive<Vec<Preprocessor>>,
}

impl Analyzer {
    /// Loads the U2-Net model at `path`.
    ///
    /// The `u2net.onnx` export is roughly 168 MiB and is not vendored, so the
    /// caller supplies it.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AnalyzerError> {
        let threads = std::thread::available_parallelism().map_or(1, |count| count.get());
        Self::with_threads(path, threads)
    }

    /// Loads the model with an explicit intra-op thread count.
    pub fn with_threads(path: impl AsRef<Path>, threads: usize) -> Result<Self, AnalyzerError> {
        Ok(Self {
            model: Model::open(path.as_ref(), threads)?,
            scratch: Exclusive::new(Vec::new()),
        })
    }

    /// Locates the subject of a JPEG, PNG, or WebP image.
    pub fn focus(&self, data: &[u8]) -> Result<Focus, AnalyzerError> {
        let raster = decode::decode(data)?;
        self.focus_raster(&raster)
    }

    /// Locates the subject of an already-decoded image.
    pub fn focus_raster(&self, raster: &Raster) -> Result<Focus, AnalyzerError> {
        let mut preprocessor = self.take();
        let result = self.locate(&mut preprocessor, raster);
        self.give(preprocessor);
        result
    }

    /// Hands the raw saliency map to `read`, along with the rectangle the
    /// image occupies inside it.
    ///
    /// [`Self::focus`] is the answer for an image on its own. This is for a
    /// caller that knows something the model does not — which of several
    /// subjects is the one being trained, say — and wants to weight the map
    /// before taking its centre of mass. The map is
    /// [`INPUT_WIDTH`] x [`INPUT_HEIGHT`], row-major, and is borrowed from ONNX
    /// Runtime's own output buffer, so it is never copied. A panic in `read`
    /// leaves the analyzer usable by the next caller.
    pub fn saliency<R>(
        &self,
        raster: &Raster,
        read: impl FnOnce(&[f32], Rect) -> R,
    ) -> Result<R, AnalyzerError> {
        let mut preprocessor = self.take();
        let result = preprocessor
            .prepare(raster)
            .map_err(AnalyzerError::from)
            .and_then(|content| {
                self.model
                    .infer(preprocessor.tensor(), |map| read(map, content))
                    .map_err(AnalyzerError::from)
            });
        self.give(preprocessor);
        result
    }

    /// Decodes an image, finds its subject, and renders a crop framed on it.
    ///
    /// One decode, one inference, one resampling pass to the output size.
    pub fn crop(&self, data: &[u8], target: Target) -> Result<Crop, AnalyzerError> {
        let raster = decode::decode(data)?;
        let focus = self.focus_raster(&raster)?;
        let source = raster.oriented_size();
        let region = plan(source, target, focus.point)?;
        Ok(Crop {
            image: render(&raster, region, target)?,
            focus,
            region,
            source,
        })
    }

    fn locate(
        &self,
        preprocessor: &mut Preprocessor,
        raster: &Raster,
    ) -> Result<Focus, AnalyzerError> {
        let content = preprocessor.prepare(raster)?;
        let located = self.model.infer(preprocessor.tensor(), |map| {
            from_saliency_region(map, INPUT_WIDTH, INPUT_HEIGHT, content)
        })?;
        let (point, confidence) = located?;
        Ok(Focus { point, confidence })
    }

    fn take(&self) -> Preprocessor {
        self.scratch
            .lock()
            .pop()
            .unwrap_or_else(|| Preprocessor::new(INPUT_WIDTH as u32, INPUT_HEIGHT as u32))
    }

    fn give(&self, preprocessor: Preprocessor) {
        self.scratch.lock().push(preprocessor);
    }
}
