/// Whether a client sends its requests through a proxy the environment or the
/// operating system names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Proxying {
    /// Every proxy is ignored, and every request goes straight to the
    /// instance.
    Never,
    /// The proxy the environment names, or the operating system's where the
    /// environment names none, carries every request it applies to.
    System,
}
