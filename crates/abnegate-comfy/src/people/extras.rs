/// Which of a recipe's person extras one generation runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Extras {
    /// Steer the pose with an OpenPose image through the recipe's ControlNet
    /// branch.
    pub(crate) pose: bool,
    /// Steer the face with the identity's face reference through an
    /// IP-Adapter.
    pub(crate) face: bool,
}
