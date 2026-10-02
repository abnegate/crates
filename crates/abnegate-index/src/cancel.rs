//! Host-owned cancellation for a long walk.

use crate::error::Result;

/// Host-owned cancellation for a long walk.
///
/// [`index_tree`](crate::index_tree) and [`index_texts`](crate::index_texts)
/// call [`check`](Self::check) between files. [`Proceed`](crate::Proceed) never
/// stops.
pub trait Cancel: Send + Sync {
    /// Fail when the host wants the walk to stop.
    fn check(&self) -> Result<()>;
}
