//! The session and the run options it is always driven with.

use ort::session::{HasSelectedOutputs, RunOptions, Session};

pub(crate) struct Runner {
    pub(crate) session: Session,
    pub(crate) options: RunOptions<HasSelectedOutputs>,
}
