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
use abnegate_search::{SearchError, SearxngClient, TimeRange, WebSearchConfig};
use abnegate_search::{format_search_context, needs_web_search};

async fn context(message: &str) -> Result<Option<String>, SearchError> {
    if !needs_web_search(message) {
        return Ok(None);
    }
    let client = SearxngClient::new(WebSearchConfig::from_env())?;
    let hits = client.search(message, Some(TimeRange::Week)).await?;
    Ok(Some(format_search_context(&hits)))
}
```
