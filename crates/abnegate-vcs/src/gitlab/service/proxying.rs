/// Whether a client sends its requests through a proxy the environment or the
/// operating system names.
#[derive(Debug, Clone, Copy)]
pub(super) enum Proxying {
    /// Every proxy is ignored, and every request goes straight to the
    /// instance.
    Never,
    /// The proxy the environment or the operating system names carries
    /// every request it applies to.
    System,
}
