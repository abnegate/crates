use std::path::Path;
use std::path::PathBuf;

use reqwest::header::ETAG;
use reqwest::header::HeaderMap;
use reqwest::header::LAST_MODIFIED;
use serde::{Deserialize, Serialize};
use tokio::fs;

const WEAK_PREFIX: &str = "W/";

/// What identifies the exact file a `.part` holds the start of.
///
/// A resumed request sends it as `If-Range`, so a server whose file changed
/// since answers with the whole new file instead of the tail of it, and the
/// two are never spliced together.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Validator {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    entity_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_modified: Option<String>,
}

impl Validator {
    pub(crate) fn from_headers(headers: &HeaderMap) -> Self {
        let read = |name| {
            headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        Self {
            entity_tag: read(ETAG),
            last_modified: read(LAST_MODIFIED),
        }
    }

    /// The value to send as `If-Range`. A weak entity tag cannot guard a
    /// byte range, so it only counts when there is no modification date.
    pub(crate) fn if_range(&self) -> Option<&str> {
        let strong = self
            .entity_tag
            .as_deref()
            .filter(|tag| !tag.starts_with(WEAK_PREFIX));
        strong.or(self.last_modified.as_deref())
    }

    pub(crate) fn path(part: &Path) -> PathBuf {
        let mut name = part.as_os_str().to_owned();
        name.push(".validator");
        PathBuf::from(name)
    }

    pub(crate) async fn load(part: &Path) -> Option<Self> {
        let stored = fs::read(Self::path(part)).await.ok()?;
        serde_json::from_slice(&stored).ok()
    }

    pub(crate) async fn store(&self, part: &Path) -> std::io::Result<()> {
        let path = Self::path(part);
        if self.if_range().is_none() {
            return match fs::remove_file(&path).await {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
        }
        let encoded = serde_json::to_vec(self).map_err(std::io::Error::other)?;
        fs::write(path, encoded).await
    }
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;

    fn headers(pairs: &[(reqwest::header::HeaderName, &'static str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(name.clone(), HeaderValue::from_static(value));
        }
        headers
    }

    #[test]
    fn a_strong_entity_tag_guards_the_range() {
        let validator = Validator::from_headers(&headers(&[
            (ETAG, "\"abc\""),
            (LAST_MODIFIED, "Wed, 21 Oct 2015 07:28:00 GMT"),
        ]));
        assert_eq!(validator.if_range(), Some("\"abc\""));
    }

    #[test]
    fn a_weak_entity_tag_defers_to_the_modification_date() {
        let validator = Validator::from_headers(&headers(&[
            (ETAG, "W/\"abc\""),
            (LAST_MODIFIED, "Wed, 21 Oct 2015 07:28:00 GMT"),
        ]));
        assert_eq!(validator.if_range(), Some("Wed, 21 Oct 2015 07:28:00 GMT"));
        assert_eq!(
            Validator::from_headers(&headers(&[(ETAG, "W/\"abc\"")])).if_range(),
            None
        );
    }

    #[test]
    fn the_validator_lives_beside_the_part_file() {
        assert_eq!(
            Validator::path(Path::new("/models/model.gguf.part")),
            Path::new("/models/model.gguf.part.validator")
        );
    }
}
