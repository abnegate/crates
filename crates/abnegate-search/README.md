# abnegate-search

Web search through a SearXNG instance. `SearxngClient` queries one instance and
returns `SearchHit` rows, `needs_web_search` decides whether a message is worth a
lookup at all, and `SearchContext` turns the outcome into prompt text a model can
cite. A result's title, URL and snippet are written by whoever published the
page, so `format_search_context` sanitizes each one and keeps it to its own lines
inside the results block, where a page cannot close that block or pass itself off
as another result.

## Features

None.

## Usage

```sh
cargo add abnegate-search
```

```rust,no_run
use abnegate_search::{Error, SearxngClient, TimeRange, WebSearchConfig};
use abnegate_search::format_search_context;

async fn context(message: &str) -> Result<Option<String>, Error> {
    let config = WebSearchConfig::from_environment();
    if !config.requested_for(message, None) {
        return Ok(None);
    }
    let client = SearxngClient::new(config)?;
    let hits = client.search(message, Some(TimeRange::Week)).await?;
    Ok(Some(format_search_context(&hits)))
}
```

`requested_for` is false while search is switched off, and otherwise asks
`needs_web_search`, so the example searches only when the message calls for it.

## Configuration

`WebSearchConfig::from_environment` reads these variables. Each one that is unset,
or does not parse, keeps the `Default` value.

| Variable | Sets | Default |
|---|---|---|
| `SEARCH_ENABLE_WEB_SEARCH` | `1`, `true`, `yes` or `on` switches search on | off |
| `SEARCH_SEARXNG_QUERY_URL` | Query URL template; `<query>` or `{query}` takes the encoded search | `http://127.0.0.1:8080/search?q=<query>&format=json` |
| `SEARCH_RESULT_COUNT` | The most hits kept, held to 1–20 | 5 |
| `SEARCH_TIMEOUT_SECONDS` | Request timeout in whole seconds, held to 1–60 | 15 |

`WebSearchConfig::new(query_url)` builds the same settings in code, switched on,
and `with_result_count` and `with_timeout` adjust them.

### Moving from an in-house search module

- Search no longer turns itself on when nothing is set. A deployment that
  searched without setting `SEARCH_ENABLE_WEB_SEARCH` now sets it to `true`.
- The default instance is `127.0.0.1:8080`. A deployment whose SearXNG runs
  anywhere else, such as behind a VPN container that owns its network, sets its
  own `SEARCH_SEARXNG_QUERY_URL`.
- The timeout is read only from `SEARCH_TIMEOUT_SECONDS`. The abbreviated name
  is not read as a fallback.
- Names follow this workspace's conventions: the error type is `Error`,
  `WebSearchConfig::from_environment` reads the variables, `timeout` is a
  `Duration`, and `WebSearchConfig` and `SearchHit` are built with their
  constructors rather than struct literals.
