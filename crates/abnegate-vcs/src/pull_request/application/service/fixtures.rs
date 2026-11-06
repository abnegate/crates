use crate::pull_request::application::InstallationId;
use crate::pull_request::application::fixtures::SHORT_TIMEOUT;
use crate::pull_request::application::fixtures::stalling;
use crate::pull_request::application::service::ApplicationService;
use crate::pull_request::origin::Origin;
use abnegate_secret::SecretValue;
use wiremock::MockServer;

/// A JWT shaped like one the App signs, to see where it travels.
pub(super) fn jwt() -> SecretValue {
    SecretValue::new("header.payload.signature")
}

/// An installation's token, shaped like one GitHub issues, to see where it
/// travels.
pub(super) fn token() -> SecretValue {
    SecretValue::new("ghs_installationtokenvalue")
}

/// The installation these tests ask about.
pub(super) fn installation() -> InstallationId {
    InstallationId::new(67890)
}

/// A service that asks `server`.
pub(super) fn stand_in(server: &MockServer) -> ApplicationService {
    ApplicationService::standing_in_for(&server.uri()).unwrap()
}

/// A service, given [`SHORT_TIMEOUT`], that asks a stand-in which answers
/// each request with `answer` and then sends nothing more.
pub(super) async fn stalled(answer: &'static str) -> ApplicationService {
    let origin = Origin::standing_in_for("", &stalling(answer).await).unwrap();
    ApplicationService::addressing(origin, false, SHORT_TIMEOUT).unwrap()
}
