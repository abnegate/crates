//! What a test that needs the host's real sandbox does on a host without one:
//! skip, saying why, or fail where [`REQUIRE_CONFINEMENT`] says it must run,
//! as in CI. For this crate's own tests and for crates whose tests run
//! commands through it.

use std::process::Stdio;

use crate::executor::Network;

/// Set, to anything but empty or `0`, where a test that needs the host's
/// real sandbox and cannot run is a failure rather than a skip, as in CI.
pub const REQUIRE_CONFINEMENT: &str = "ABNEGATE_EXEC_REQUIRE_CONFINEMENT";

/// Whether [`REQUIRE_CONFINEMENT`] is set, so a test that needs the host's
/// real sandbox must fail rather than skip when it cannot run.
pub fn confinement_required() -> bool {
    std::env::var_os(REQUIRE_CONFINEMENT).is_some_and(|value| !value.is_empty() && value != "0")
}

/// Whether a [`Network::Denied`] command runs at all here. Seatbelt cannot
/// nest inside a stricter sandbox, and a host may offer no seccomp, so a test
/// host that cannot deny a command the network skips, saying why on standard
/// error.
///
/// # Panics
///
/// When a denied command cannot run here but [`confinement_required`].
pub async fn network_denial_enforced() -> bool {
    let status = Network::Denied
        .command("true")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    if matches!(status, Ok(status) if status.success()) {
        return true;
    }
    assert!(
        !confinement_required(),
        "{REQUIRE_CONFINEMENT} is set, but a network-denied command cannot run here: {status:?}"
    );
    eprintln!(
        "skipped: a network-denied command cannot run here ({status:?}); set {REQUIRE_CONFINEMENT}=1 to fail instead"
    );
    false
}
