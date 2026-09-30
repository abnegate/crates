/// What talking to ComfyUI can fail with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// [`Config::enabled`](crate::Config::enabled) is off.
    #[error("ComfyUI is disabled")]
    Disabled,
    /// A setting, recipe, workflow or input cannot work, as the text says.
    #[error("invalid ComfyUI configuration: {0}")]
    Configuration(&'static str),
    /// A request to the server failed to complete.
    #[error("ComfyUI request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// The server answered with something other than what the workflow
    /// promised, or reported the workflow failed.
    #[error("ComfyUI returned an invalid response: {0}")]
    InvalidResponse(&'static str),
    /// The work outran its configured time limit.
    #[error("generation timed out")]
    Timeout,
    /// The caller cancelled the work.
    #[error("image generation cancelled")]
    Cancelled,
}
