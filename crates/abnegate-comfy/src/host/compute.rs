use serde::Deserialize;
use serde::Serialize;

const RUNPOD_KEY_FILE: &str = "runpod.key";
/// The key file of every compute that reads one.
pub(crate) const KEY_FILES: &[&str] = &[RUNPOD_KEY_FILE];

/// Where the host trainer runs a [`Job`](crate::host::Job), as `job.json`
/// names it in its `provider` key. A job that names none runs
/// [`Compute::Local`].
///
/// A remote compute pulls the base from Hugging Face rather than reading the
/// models directory, so a request trained on one needs none of its base's
/// files installed, and it trains every subject and method on the host
/// trainer. Its credential is written beside the job with
/// [`JobDirectory::write_compute_key`](crate::host::JobDirectory::write_compute_key),
/// never into `job.json`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Compute {
    /// The machine the host trainer runs on.
    #[default]
    Local,
    /// A pod the host trainer rents on Runpod, with the API key it reads
    /// from `runpod.key`.
    Runpod,
}

impl Compute {
    /// The name the compute travels under in `job.json`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Runpod => "runpod",
        }
    }

    /// Whether the job runs away from the machine that holds the models
    /// directory.
    pub fn is_remote(self) -> bool {
        !matches!(self, Self::Local)
    }

    /// The file beside `job.json` the trainer reads the compute's
    /// credential from, for a compute that needs one.
    pub fn key_file(self) -> Option<&'static str> {
        match self {
            Self::Local => None,
            Self::Runpod => Some(RUNPOD_KEY_FILE),
        }
    }
}
