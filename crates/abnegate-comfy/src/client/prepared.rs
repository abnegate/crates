use crate::client::SourceImage;
use crate::client::plan::Plan;
use crate::inventory::WeightSidecar;
use crate::people::PoseImage;

/// Everything a generation reads from disk before it submits anything: the
/// selected weight's binding, the plan of its first pass and of its face
/// refine when it gets one, and the pose and face images its person extras
/// upload.
pub(super) struct Prepared {
    pub(super) sidecar: Option<WeightSidecar>,
    pub(super) first: Plan,
    pub(super) refine: Option<Plan>,
    pub(super) pose: Option<PoseImage>,
    pub(super) face: Option<SourceImage>,
}
