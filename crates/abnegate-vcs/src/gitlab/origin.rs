use crate::gitlab::GitLabError;
use crate::gitlab::GitLabResult;
use abnegate_http::metadata;
use url::Url;

/// The scheme a configured origin uses.
const HTTPS: &str = "https";

/// The scheme an origin may use only when the operator opted in to it.
const HTTP: &str = "http";

/// The path GitLab serves its REST API under, beneath the instance's own.
const API: &str = "/api/v4";

/// The GitLab REST API this service addresses: an instance's URL, or its API
/// base, normalised to the API base.
#[derive(Debug, Clone)]
pub(super) struct Origin {
    url: Url,
}

impl Origin {
    /// The origin an operator configured: an HTTPS URL with no credentials,
    /// query or fragment, or a plain HTTP one where `permit_http` says the
    /// operator opted in to it, that names neither a link-local address nor
    /// a cloud provider's metadata service. An instance's URL, at its root
    /// or under a path prefix, becomes the API base beneath it; a URL that
    /// already ends in `/api/v4` is kept, a trailing slash ignored.
    pub(super) fn configured(url: &str, permit_http: bool) -> GitLabResult<Self> {
        let mut url = Url::parse(url).map_err(|_| GitLabError::InvalidOrigin)?;
        let scheme_admitted = url.scheme() == HTTPS || (permit_http && url.scheme() == HTTP);
        if !scheme_admitted
            || url.cannot_be_a_base()
            || url.host_str().is_none_or(str::is_empty)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || metadata::is_url(&url)
        {
            return Err(GitLabError::InvalidOrigin);
        }
        let path = url.path().trim_end_matches('/').to_string();
        match path.ends_with(API) {
            true => url.set_path(&path),
            false => url.set_path(&format!("{path}{API}")),
        }
        Ok(Self { url })
    }

    /// Whether the origin is plain HTTP, which only an operator's opt-in
    /// admits.
    pub(super) fn plain(&self) -> bool {
        self.url.scheme() == HTTP
    }

    /// Whether `url` is on this origin: the same scheme, host and port, the
    /// port a scheme implies counted as named.
    pub(super) fn holds(&self, url: &Url) -> bool {
        url.scheme() == self.url.scheme()
            && url.host_str() == self.url.host_str()
            && url.port_or_known_default() == self.url.port_or_known_default()
    }

    /// The API base with `segments` appended, each one percent-encoded as a
    /// single path segment.
    pub(super) fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.url.clone();
        if let Ok(mut path) = url.path_segments_mut() {
            path.extend(segments);
        }
        url
    }

    /// The API base itself.
    #[cfg(test)]
    pub(super) fn as_str(&self) -> &str {
        self.url.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instance_url_becomes_its_api_base() {
        for (configured, base) in [
            ("https://gitlab.com", "https://gitlab.com/api/v4"),
            ("https://gitlab.com/", "https://gitlab.com/api/v4"),
            (
                "https://gitlab.example.com",
                "https://gitlab.example.com/api/v4",
            ),
            (
                "https://example.com/gitlab",
                "https://example.com/gitlab/api/v4",
            ),
            (
                "https://example.com/gitlab/",
                "https://example.com/gitlab/api/v4",
            ),
            (
                "https://gitlab.example.com/api/v4",
                "https://gitlab.example.com/api/v4",
            ),
            (
                "https://gitlab.example.com/api/v4/",
                "https://gitlab.example.com/api/v4",
            ),
            (
                "https://example.com/gitlab/api/v4",
                "https://example.com/gitlab/api/v4",
            ),
        ] {
            assert_eq!(
                Origin::configured(configured, false).unwrap().as_str(),
                base,
                "{configured}"
            );
        }
    }

    #[test]
    fn an_endpoint_sends_each_segment_as_one() {
        let origin = Origin::configured("https://gitlab.example.com", false).unwrap();

        assert_eq!(
            origin
                .endpoint(&["projects", "group/sub", "repository", "tree"])
                .as_str(),
            "https://gitlab.example.com/api/v4/projects/group%2Fsub/repository/tree"
        );
        assert_eq!(
            origin.endpoint(&["projects", "%2E%2E"]).as_str(),
            "https://gitlab.example.com/api/v4/projects/%252E%252E"
        );
    }

    #[test]
    fn plain_http_is_an_origin_only_once_the_operator_opts_in() {
        assert!(matches!(
            Origin::configured("http://gitlab.lan", false),
            Err(GitLabError::InvalidOrigin)
        ));

        let plain = Origin::configured("http://gitlab.lan", true).unwrap();
        assert!(plain.plain());
        assert_eq!(plain.as_str(), "http://gitlab.lan/api/v4");

        let secure = Origin::configured("https://gitlab.lan", true).unwrap();
        assert!(!secure.plain());
    }

    #[test]
    fn an_origin_with_credentials_a_query_a_fragment_or_another_scheme_is_refused() {
        for refused in [
            "https://user:token@gitlab.example.com",
            "https://token@gitlab.example.com",
            "https://gitlab.example.com/?private_token=x",
            "https://gitlab.example.com/#x",
            "ftp://gitlab.example.com",
            "file:///etc/passwd",
            "mailto:someone@gitlab.example.com",
            "gitlab.example.com",
            "",
        ] {
            for permit_http in [false, true] {
                assert!(
                    matches!(
                        Origin::configured(refused, permit_http),
                        Err(GitLabError::InvalidOrigin)
                    ),
                    "{refused:?} with plain HTTP permitted {permit_http}"
                );
            }
        }
    }

    #[test]
    fn an_origin_holds_only_its_own_scheme_host_and_port() {
        let origin = Origin::configured("https://gitlab.example.com", false).unwrap();
        for held in [
            "https://gitlab.example.com/api/v4/projects/1/repository/tree?page=2",
            "https://GitLab.Example.com:443/elsewhere",
        ] {
            assert!(origin.holds(&Url::parse(held).unwrap()), "{held}");
        }
        for elsewhere in [
            "http://gitlab.example.com/api/v4",
            "https://gitlab.example.com:8443/api/v4",
            "https://api.gitlab.example.com/api/v4",
            "https://example.com/api/v4",
        ] {
            assert!(
                !origin.holds(&Url::parse(elsewhere).unwrap()),
                "{elsewhere}"
            );
        }
    }
}
