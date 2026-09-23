//! Salient-object detection with U2-Net on ONNX Runtime.

use std::path::Path;
use std::sync::Mutex;

use ort::session::builder::GraphOptimizationLevel;
use ort::session::{HasSelectedOutputs, OutputSelector, RunOptions, Session};
use ort::value::TensorRef;

pub const INPUT_WIDTH: i32 = 320;
pub const INPUT_HEIGHT: i32 = 320;
pub const INPUT_LEN: usize = 3 * INPUT_WIDTH as usize * INPUT_HEIGHT as usize;

const INPUT_NAME: &str = "input.1";

/// U2-Net exposes seven side outputs. The first is the fused, highest-resolution
/// saliency map and is the only one this crate needs; naming it lets ONNX
/// Runtime prune the other six heads from the executed graph.
const OUTPUT_NAME: &str = "1959";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("model not found at {0}")]
    Missing(String),
    #[error("load saliency model: {0}")]
    Load(String),
    #[error("invalid input tensor length: got {0}")]
    InputLength(usize),
    #[error("run saliency model: {0}")]
    Run(String),
    #[error("saliency model returned no output")]
    MissingOutput,
}

/// Owns a single reusable ONNX Runtime session.
///
/// ONNX Runtime already saturates every core inside one `Run`, so inference is
/// serialized behind a mutex; batching images across threads gains nothing and
/// multiplies the arena.
pub struct Model {
    runner: Mutex<Runner>,
}

struct Runner {
    session: Session,
    options: RunOptions<HasSelectedOutputs>,
}

impl Model {
    /// Loads the U2-Net model at `path`, using `threads` intra-op threads.
    pub fn open(path: &Path, threads: usize) -> Result<Self, Error> {
        if !path.is_file() {
            return Err(Error::Missing(path.display().to_string()));
        }

        let session = build(path, threads).map_err(Error::Load)?;
        let options = RunOptions::new()
            .map_err(|error| Error::Load(error.to_string()))?
            .with_outputs(OutputSelector::no_default().with(OUTPUT_NAME));

        Ok(Self {
            runner: Mutex::new(Runner { session, options }),
        })
    }

    /// Runs the model and hands the fused 320x320 saliency map to `read`.
    ///
    /// The map is borrowed straight from ONNX Runtime's output buffer, so no
    /// copy of it is ever made.
    pub fn infer<R>(&self, input: &[f32], read: impl FnOnce(&[f32]) -> R) -> Result<R, Error> {
        if input.len() != INPUT_LEN {
            return Err(Error::InputLength(input.len()));
        }

        let shape = [1, 3, INPUT_HEIGHT as usize, INPUT_WIDTH as usize];
        let tensor = TensorRef::from_array_view((shape, input))
            .map_err(|error| Error::Run(error.to_string()))?;

        let mut runner = self.runner.lock().expect("saliency model mutex poisoned");
        let Runner { session, options } = &mut *runner;
        let outputs = session
            .run_with_options(ort::inputs![INPUT_NAME => tensor], options)
            .map_err(|error| Error::Run(error.to_string()))?;

        let value = outputs.get(OUTPUT_NAME).ok_or(Error::MissingOutput)?;
        let (_, map) = value
            .try_extract_tensor::<f32>()
            .map_err(|error| Error::Run(error.to_string()))?;
        Ok(read(map))
    }
}

fn build(path: &Path, threads: usize) -> Result<Session, String> {
    let describe = |error: ort::Error<_>| error.to_string();
    let mut builder = Session::builder().map_err(|error| error.to_string())?;
    builder = builder
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(describe)?;
    builder = builder
        .with_intra_threads(threads.max(1))
        .map_err(describe)?;
    builder = builder.with_inter_threads(1).map_err(describe)?;
    builder = builder.with_parallel_execution(false).map_err(describe)?;
    builder = builder.with_memory_pattern(true).map_err(describe)?;
    builder
        .commit_from_file(path)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_missing_model() {
        let Err(error) = Model::open(Path::new("/nonexistent/u2net.onnx"), 1) else {
            panic!("loading a missing model should fail");
        };
        assert_eq!(
            error.to_string(),
            "model not found at /nonexistent/u2net.onnx"
        );
    }

    #[test]
    fn rejects_a_wrong_sized_tensor() {
        assert_eq!(
            Error::InputLength(7).to_string(),
            "invalid input tensor length: got 7"
        );
    }
}
