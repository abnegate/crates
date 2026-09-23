use reqwest::header::CONTENT_RANGE;
use reqwest::header::HeaderMap;

use crate::download::error::DownloadError;

const BYTES_UNIT: &str = "bytes ";
const UNKNOWN_LENGTH: &str = "*";

/// Where a `206 Partial Content` body starts, and how long the whole file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContentRange {
    pub(crate) start: u64,
    pub(crate) total: Option<u64>,
}

impl ContentRange {
    /// Read `Content-Range: bytes 4-8/9`, or `bytes 4-8/*` when the server
    /// does not know the length.
    pub(crate) fn from_headers(headers: &HeaderMap) -> Option<Self> {
        let value = headers.get(CONTENT_RANGE)?.to_str().ok()?;
        let (range, total) = value.strip_prefix(BYTES_UNIT)?.split_once('/')?;
        let (start, _) = range.split_once('-')?;
        let total = match total.trim() {
            UNKNOWN_LENGTH => None,
            length => Some(length.parse().ok()?),
        };
        Some(Self {
            start: start.trim().parse().ok()?,
            total,
        })
    }

    /// How long the whole file is: the length the range names, or else where
    /// a body of `body` bytes from the range's start ends.
    pub(crate) fn file_length(&self, body: Option<u64>) -> Result<Option<u64>, DownloadError> {
        match (self.total, body) {
            (Some(total), _) => Ok(Some(total)),
            (None, Some(body)) => {
                self.start
                    .checked_add(body)
                    .map(Some)
                    .ok_or(DownloadError::Incomplete {
                        expected: u64::MAX,
                        received: self.start,
                    })
            }
            (None, None) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;

    fn range(value: &'static str) -> Option<ContentRange> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_RANGE, HeaderValue::from_static(value));
        ContentRange::from_headers(&headers)
    }

    #[test]
    fn a_range_names_its_start_and_the_whole_length() {
        assert_eq!(
            range("bytes 4-8/9"),
            Some(ContentRange {
                start: 4,
                total: Some(9)
            })
        );
        assert_eq!(
            range("bytes 4-8/*"),
            Some(ContentRange {
                start: 4,
                total: None
            })
        );
    }

    #[test]
    fn the_file_length_is_the_named_total_or_the_end_of_the_body() {
        let named = ContentRange {
            start: 4,
            total: Some(9),
        };
        let unnamed = ContentRange {
            start: 4,
            total: None,
        };

        assert_eq!(named.file_length(Some(5)).unwrap(), Some(9));
        assert_eq!(unnamed.file_length(Some(5)).unwrap(), Some(9));
        assert_eq!(unnamed.file_length(None).unwrap(), None);
    }

    #[test]
    fn a_body_that_ends_past_u64_is_incomplete_rather_than_an_overflow() {
        let range = ContentRange {
            start: u64::MAX - 1,
            total: None,
        };

        let length = range.file_length(Some(5));

        assert!(
            matches!(
                length,
                Err(DownloadError::Incomplete {
                    expected: u64::MAX,
                    received
                }) if received == u64::MAX - 1
            ),
            "{length:?}"
        );
    }

    #[test]
    fn a_malformed_range_reads_as_none() {
        for malformed in [
            "4-8/9",
            "bytes x-8/9",
            "bytes 4-8",
            "items 4-8/9",
            "bytes 4-8/x",
        ] {
            assert_eq!(range(malformed), None, "{malformed}");
        }
        assert_eq!(ContentRange::from_headers(&HeaderMap::new()), None);
    }
}
