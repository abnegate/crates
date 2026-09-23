use crate::parse_error::ParseError;
use std::fmt;
use std::str::FromStr;
use url::Url;

/// The only host a repository address may name.
const GITHUB_HOST: &str = "github.com";

/// The only transport a repository address may use.
const HTTPS: &str = "https";

/// The transport a test fixture's local repository is reached over.
#[cfg(any(test, feature = "test-support"))]
const FILE: &str = "file";

/// The suffix git's own URLs carry on a repository name.
const GIT_SUFFIX: &str = ".git";

/// A repository on github.com reached over HTTPS, normalised to
/// `https://github.com/{owner}/{name}.git`: no credentials, port, query or
/// fragment, and an owner and a name made only of the characters GitHub
/// allows in them, so nothing in it can reach git as an option, a transport
/// or a credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepositoryUrl {
    url: Url,
    protocol: &'static str,
}

impl RepositoryUrl {
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        let url = Url::parse(source).map_err(|_| ParseError::RepositoryUrl)?;
        if url.scheme() != HTTPS
            || url.host_str() != Some(GITHUB_HOST)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ParseError::RepositoryUrl);
        }
        let segments: Vec<&str> = url.path().trim_matches('/').split('/').collect();
        let [owner, name] = segments.as_slice() else {
            return Err(ParseError::RepositoryUrl);
        };
        let name = name.strip_suffix(GIT_SUFFIX).unwrap_or(name);
        if !named(owner) || !named(name) {
            return Err(ParseError::RepositoryUrl);
        }
        let normalised = format!("{HTTPS}://{GITHUB_HOST}/{owner}/{name}{GIT_SUFFIX}");
        Ok(Self {
            url: Url::parse(&normalised).map_err(|_| ParseError::RepositoryUrl)?,
            protocol: HTTPS,
        })
    }

    /// A repository on the local disk, reached over `file://`. Only test
    /// builds can make one: nothing a caller configures reaches it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn local(path: &std::path::Path) -> Result<Self, ParseError> {
        let url = Url::from_file_path(path).map_err(|()| ParseError::RepositoryUrl)?;
        Ok(Self {
            url,
            protocol: FILE,
        })
    }

    pub fn as_str(&self) -> &str {
        self.url.as_str()
    }

    /// The one transport git may use to reach this repository, for
    /// `GIT_ALLOW_PROTOCOL`.
    pub(crate) fn protocol(&self) -> &'static str {
        self.protocol
    }
}

/// An owner or repository name: what GitHub allows in one, and never a path
/// traversal.
fn named(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

impl FromStr for RepositoryUrl {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for RepositoryUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_github_https_address_is_normalised() {
        for source in [
            "https://github.com/owner/repository",
            "https://github.com/owner/repository.git",
            "https://github.com/owner/repository/",
            "https://GitHub.com/owner/repository",
        ] {
            let url = RepositoryUrl::parse(source).expect(source);
            assert_eq!(url.as_str(), "https://github.com/owner/repository.git");
            assert_eq!(url.protocol(), HTTPS);
        }
    }

    #[test]
    fn an_address_that_is_not_a_github_https_repository_is_refused() {
        for source in [
            "",
            "--upload-pack=evil",
            "-oProxyCommand=evil",
            "/tmp/repository",
            "file:///tmp/repository",
            "ext::sh -c evil",
            "git@github.com:owner/repository",
            "ssh://git@github.com/owner/repository",
            "http://github.com/owner/repository",
            "https://token@github.com/owner/repository",
            "https://user:token@github.com/owner/repository",
            "https://github.com:8443/owner/repository",
            "https://github.com/owner/repository?token=secret",
            "https://github.com/owner/repository#fragment",
            "https://elsewhere.test/owner/repository",
            "https://github.com.elsewhere.test/owner/repository",
            "https://github.com/owner/repository/extra",
            "https://github.com/owner",
            "https://github.com/../repository",
            "https://github.com/owner/.git",
            "https://github.com/owner/repo%20sitory",
        ] {
            assert!(RepositoryUrl::parse(source).is_err(), "{source}");
        }
    }

    #[test]
    fn a_local_repository_is_reached_over_file_only() {
        let directory = tempfile::tempdir().unwrap();
        let url = RepositoryUrl::local(directory.path()).unwrap();

        assert!(url.as_str().starts_with("file://"), "{url}");
        assert_eq!(url.protocol(), FILE);
        assert!(RepositoryUrl::local(std::path::Path::new("relative")).is_err());
    }
}
