use reqwest::redirect::Policy;

/// Redirects within the origin a request follows before it stops.
pub(crate) const MAXIMUM_REDIRECTS: usize = 10;

/// Follow a redirect only while it stays on the origin the request was sent
/// to, and for no more than [`MAXIMUM_REDIRECTS`] hops.
pub(crate) fn within_origin() -> Policy {
    Policy::custom(|attempt| {
        let previous = attempt.previous();
        let within = previous
            .first()
            .is_some_and(|sent| sent.origin() == attempt.url().origin());
        if within && previous.len() <= MAXIMUM_REDIRECTS {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}
