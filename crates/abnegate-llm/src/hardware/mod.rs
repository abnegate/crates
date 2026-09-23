//! What the machine this runs on can do locally.

mod gpu_type;
mod machine_profile;
mod model_recommendation;
mod recommended_models;

pub use crate::hardware::gpu_type::GpuType;
pub use crate::hardware::machine_profile::MachineProfile;
pub use crate::hardware::model_recommendation::ModelRecommendation;
pub use crate::hardware::recommended_models::RecommendedModels;
