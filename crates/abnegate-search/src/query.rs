//! Turning a message into the request SearXNG receives.

use crate::time_range::TimeRange;

/// Truncate user messages so a pasted file cannot become the search query.
const MAXIMUM_QUERY_CHARACTERS: usize = 500;

/// Query placeholders a configured URL template may carry.
const ANGLE_QUERY_PLACEHOLDER: &str = "<query>";
const BRACE_QUERY_PLACEHOLDER: &str = "{query}";

/// Prefix a host appends when it inlines an attached file into the message.
const ATTACHED_FILE_MARKER: &str = "\n\nAttached file:";

/// Drop attached-file blocks and cap length so the query stays a question.
pub fn sanitize_query(content: &str) -> String {
    let without_attachments = content
        .split(ATTACHED_FILE_MARKER)
        .next()
        .unwrap_or(content);
    without_attachments
        .trim()
        .chars()
        .take(MAXIMUM_QUERY_CHARACTERS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Add `parameter` to `url`'s query string, ahead of any fragment.
///
/// A fragment is never sent in the request, so a parameter appended past the
/// `#` reaches no engine and the filter is silently dropped. Splitting it off
/// first also keeps a `?` inside the fragment from being read as a query
/// string that is already open.
fn append_parameter(url: &mut String, parameter: &str) {
    let fragment = url.find('#').map(|hash| url.split_off(hash));
    url.push(if url.contains('?') { '&' } else { '?' });
    url.push_str(parameter);
    if let Some(fragment) = fragment {
        url.push_str(&fragment);
    }
}

/// Substitute `<query>` / `{query}` in the configured template, or append `q=`.
///
/// A template may place the query in the path, so the separator for an
/// appended parameter follows the built URL rather than the template.
pub fn build_search_url(template: &str, query: &str, range: Option<TimeRange>) -> String {
    let encoded = urlencoding::encode(query);
    let mut url = if template.contains(ANGLE_QUERY_PLACEHOLDER) {
        template.replace(ANGLE_QUERY_PLACEHOLDER, encoded.as_ref())
    } else if template.contains(BRACE_QUERY_PLACEHOLDER) {
        template.replace(BRACE_QUERY_PLACEHOLDER, encoded.as_ref())
    } else {
        let mut url = template.to_string();
        append_parameter(&mut url, &format!("q={encoded}&format=json"));
        url
    };
    if let Some(range) = range {
        append_parameter(&mut url, &format!("{}={range}", TimeRange::PARAMETER));
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_SEARXNG_QUERY_URL;

    #[test]
    fn build_search_url_replaces_placeholders() {
        assert_eq!(
            build_search_url(DEFAULT_SEARXNG_QUERY_URL, "open source", None),
            "http://gluetun:8080/search?q=open%20source&format=json"
        );
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search?q={query}&format=json",
                "a&b",
                None
            ),
            "http://gluetun:8080/search?q=a%26b&format=json"
        );
        assert_eq!(
            build_search_url("http://gluetun:8080/search", "hello", None),
            "http://gluetun:8080/search?q=hello&format=json"
        );
        assert_eq!(
            build_search_url("http://gluetun:8080/search?lang=en", "hello", None),
            "http://gluetun:8080/search?lang=en&q=hello&format=json"
        );
    }

    /// Each template shape reaches the range differently: the two placeholder
    /// forms keep whatever separator the operator wrote, and the appended form
    /// has already opened a query string of its own.
    #[test]
    fn build_search_url_appends_the_range_to_every_template_shape() {
        assert_eq!(
            build_search_url(
                DEFAULT_SEARXNG_QUERY_URL,
                "open source",
                Some(TimeRange::Day)
            ),
            "http://gluetun:8080/search?q=open%20source&format=json&time_range=day"
        );
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search?q={query}&format=json",
                "a&b",
                Some(TimeRange::Week)
            ),
            "http://gluetun:8080/search?q=a%26b&format=json&time_range=week"
        );
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search",
                "hello",
                Some(TimeRange::Month)
            ),
            "http://gluetun:8080/search?q=hello&format=json&time_range=month"
        );
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search?lang=en",
                "hello",
                Some(TimeRange::Day)
            ),
            "http://gluetun:8080/search?lang=en&q=hello&format=json&time_range=day"
        );
    }

    /// A fragment is never sent to the server, so a parameter appended after
    /// one reaches nothing and the engine silently ignores the filter. Every
    /// parameter has to land ahead of the `#`.
    #[test]
    fn build_search_url_keeps_a_template_fragment_behind_the_parameters() {
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search?q={query}&format=json#view",
                "hello",
                Some(TimeRange::Day)
            ),
            "http://gluetun:8080/search?q=hello&format=json&time_range=day#view"
        );
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search/<query>#view",
                "hello",
                Some(TimeRange::Week)
            ),
            "http://gluetun:8080/search/hello?time_range=week#view"
        );
        assert_eq!(
            build_search_url("http://gluetun:8080/search#view", "hello", None),
            "http://gluetun:8080/search?q=hello&format=json#view"
        );
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search#view",
                "hello",
                Some(TimeRange::Month)
            ),
            "http://gluetun:8080/search?q=hello&format=json&time_range=month#view"
        );
    }

    /// A `?` inside a fragment does not open a query string, so it must not
    /// decide the separator either.
    #[test]
    fn build_search_url_ignores_a_question_mark_inside_a_fragment() {
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search/<query>#view?tab=all",
                "hello",
                Some(TimeRange::Day)
            ),
            "http://gluetun:8080/search/hello?time_range=day#view?tab=all"
        );
    }

    /// A placeholder can sit in the path, leaving no query string to extend.
    #[test]
    fn build_search_url_opens_a_query_string_for_a_path_placeholder() {
        assert_eq!(
            build_search_url("http://gluetun:8080/search/<query>", "hello", None),
            "http://gluetun:8080/search/hello"
        );
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search/<query>",
                "hello",
                Some(TimeRange::Week)
            ),
            "http://gluetun:8080/search/hello?time_range=week"
        );
        assert_eq!(
            build_search_url(
                "http://gluetun:8080/search/{query}",
                "open source",
                Some(TimeRange::Month)
            ),
            "http://gluetun:8080/search/open%20source?time_range=month"
        );
    }

    #[test]
    fn sanitize_query_strips_attachments_and_caps_length() {
        let content = "What is Rust?\n\nAttached file: notes.md\n```md\nsecret\n```";
        assert_eq!(sanitize_query(content), "What is Rust?");
        assert!(sanitize_query("   ").is_empty());
        assert_eq!(
            sanitize_query(&"x".repeat(600)).len(),
            MAXIMUM_QUERY_CHARACTERS
        );
    }
}
