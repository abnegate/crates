use reqwest::header::CONTENT_RANGE;
use reqwest::header::HeaderMap;

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
