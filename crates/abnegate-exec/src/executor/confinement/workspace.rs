use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use super::error::ConfinementError;
use super::probe::PROBE_ALLOWED;
use super::probe::PROBE_ALLOWED_CONTENT;
use super::probe::PROBE_DENIED;
use super::probe::PROBE_DENIED_CONTENT;
use super::probe::probe_failure;

pub(super) struct ProbeWorkspace {
    base: PathBuf,
    pub(super) root: PathBuf,
    pub(super) denied: PathBuf,
}

impl ProbeWorkspace {
    pub(super) fn create() -> Result<Self, ConfinementError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let base = std::env::temp_dir().join(format!(
            "abnegate-exec-confinement-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir(&base).map_err(probe_failure)?;
        let base = fs::canonicalize(&base).map_err(probe_failure)?;
        let root = base.join("root");
        fs::create_dir(&root).map_err(probe_failure)?;
        fs::write(root.join(PROBE_ALLOWED), PROBE_ALLOWED_CONTENT).map_err(probe_failure)?;
        let denied = base.join(PROBE_DENIED);
        fs::write(&denied, PROBE_DENIED_CONTENT).map_err(probe_failure)?;
        Ok(Self { base, root, denied })
    }
}

impl Drop for ProbeWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
