use serde::{Deserialize, Serialize};

/// The graphics hardware a machine has, and what it is worth.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum GpuType {
    AppleSilicon { chip: String, gpu_cores: u32 },
    NvidiaDesktop { model: String, cuda_cores: u32 },
    NvidiaLaptop { model: String, cuda_cores: u32 },
    AmdDesktop { model: String },
    IntelArc { model: String },
    CpuOnly,
}
