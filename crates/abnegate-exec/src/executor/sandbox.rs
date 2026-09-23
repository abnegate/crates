//! How a test that needs the host's real sandbox treats a host without one.

use super::Confinement;
use super::ConfinementMode;
use super::HOST_BACKEND;

/// Set, to anything but empty or `0`, where a real-sandbox test that cannot
/// run is a failure rather than a skip, as in CI.
pub(crate) const REQUIRE_CONFINEMENT: &str = "ABNEGATE_EXEC_REQUIRE_CONFINEMENT";

/// Whether a test that runs a command in the host's real sandbox, in `mode`,
/// can go ahead.
///
/// # Panics
///
/// When it cannot, but [`REQUIRE_CONFINEMENT`] is set and the host's backend
/// claims `mode`.
pub(crate) async fn proven(mode: ConfinementMode) -> bool {
    match Confinement::probe(mode).await {
        Ok(()) => true,
        Err(error) => {
            assert!(
                !required(mode),
                "{REQUIRE_CONFINEMENT} is set, but this host cannot prove its sandbox for {mode:?}: {error}"
            );
            false
        }
    }
}

/// Whether a real-sandbox test in `mode` must run rather than skip.
pub(crate) fn required(mode: ConfinementMode) -> bool {
    let claimed = HOST_BACKEND.is_some_and(|backend| match mode {
        ConfinementMode::SingleCommand => true,
        ConfinementMode::ProcessTree => backend.enforces_execute_roots(),
    });
    let set = std::env::var_os(REQUIRE_CONFINEMENT)
        .is_some_and(|value| !value.is_empty() && value != "0");
    claimed && set
}
