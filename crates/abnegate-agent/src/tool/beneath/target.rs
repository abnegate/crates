use super::Access;

#[derive(Clone, Copy)]
pub(super) enum Target {
    File(Access),
    Directory,
}
