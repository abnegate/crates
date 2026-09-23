#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("ComfyUI is disabled")]
    Disabled,
    #[error("invalid ComfyUI configuration: {0}")]
    Configuration(&'static str),
    #[error("ComfyUI request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("ComfyUI returned an invalid response: {0}")]
    InvalidResponse(&'static str),
    #[error("generation timed out")]
    Timeout,
    #[error("image generation cancelled")]
    Cancelled,
}
