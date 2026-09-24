use serde::{Deserialize, Serialize};

/// The graphics hardware a machine has, and what it is worth.
///
/// A variant may gain a field in a minor release, so a value is built with
/// its constructor ([`GpuType::apple_silicon`], [`GpuType::nvidia_desktop`]
/// and the rest) and a pattern outside this crate ends in `..`:
///
/// ```compile_fail,E0639
/// let gpu = abnegate_llm::GpuType::AmdDesktop {
///     model: "RX 7900 XTX".to_string(),
/// };
/// # let _ = gpu;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum GpuType {
    /// An Apple Silicon `chip`, such as `M4 Max`, whose GPU memory is the
    /// machine's own.
    #[non_exhaustive]
    AppleSilicon { chip: String, gpu_cores: u32 },
    /// A desktop NVIDIA card, such as an `RTX 4090`.
    #[non_exhaustive]
    NvidiaDesktop { model: String, cuda_cores: u32 },
    /// A laptop NVIDIA GPU, such as an `RTX 4060 Laptop`.
    #[non_exhaustive]
    NvidiaLaptop { model: String, cuda_cores: u32 },
    /// A desktop AMD card, such as an `RX 7900 XTX`.
    #[non_exhaustive]
    AmdDesktop { model: String },
    /// An Intel Arc card, such as an `A770`.
    #[non_exhaustive]
    IntelArc { model: String },
    /// No GPU a model can run on.
    CpuOnly,
}

impl GpuType {
    /// The Apple Silicon `chip` with `gpu_cores` GPU cores.
    pub fn apple_silicon(chip: impl Into<String>, gpu_cores: u32) -> Self {
        Self::AppleSilicon {
            chip: chip.into(),
            gpu_cores,
        }
    }

    /// The desktop NVIDIA card `model` with `cuda_cores` CUDA cores.
    pub fn nvidia_desktop(model: impl Into<String>, cuda_cores: u32) -> Self {
        Self::NvidiaDesktop {
            model: model.into(),
            cuda_cores,
        }
    }

    /// The laptop NVIDIA GPU `model` with `cuda_cores` CUDA cores.
    pub fn nvidia_laptop(model: impl Into<String>, cuda_cores: u32) -> Self {
        Self::NvidiaLaptop {
            model: model.into(),
            cuda_cores,
        }
    }

    /// The desktop AMD card `model`.
    pub fn amd_desktop(model: impl Into<String>) -> Self {
        Self::AmdDesktop {
            model: model.into(),
        }
    }

    /// The Intel Arc card `model`.
    pub fn intel_arc(model: impl Into<String>) -> Self {
        Self::IntelArc {
            model: model.into(),
        }
    }
}
