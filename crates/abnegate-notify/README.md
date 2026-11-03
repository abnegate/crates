# abnegate-notify

One notification, every channel. `Fanout` delivers a `Notification` to every
registered `Notifier` concurrently and returns a `Report` saying what each one
did; a channel that hangs, fails or panics costs one entry in that report and
nothing else, so a misconfigured webhook cannot stop an email going out. Slack
and Discord are always available, and email ships behind the `smtp` feature. A
webhook URL is a bearer credential, so it is held as a secret and no error,
`Debug` output or log line reproduces it; each backend accepts only `https` URLs
whose host matches its provider exactly and refuses redirects. A notification's
text is sanitized as it is built, which strips terminal control sequences and
redacts credentials.

## Features

- `smtp`: the `Email` channel, and `Mailer`, which sends one message through a relay; both pull in `lettre`.
- `testing`: `MockMailer`, a `Mail` that records instead of sending, and `Endpoint::for_test` and `Webhook::for_test`, which point a webhook at a server on the local machine such as wiremock, for a caller's own tests.

## Usage

```sh
cargo add abnegate-notify
```

```rust,no_run
use abnegate_notify::Discord;
use abnegate_notify::Error;
use abnegate_notify::Fanout;
use abnegate_notify::Notification;
use abnegate_notify::Severity;
use abnegate_notify::Slack;

async fn announce() -> Result<(), Error> {
    let fanout = Fanout::new()
        .with(Slack::new("https://hooks.slack.com/services/T000/B000/xxxx")?)
        .with(Discord::new("https://discord.com/api/webhooks/1/xxxx")?);

    let notification = Notification::new("Build failed", "3 tests failed on main")
        .severity(Severity::Error)
        .link("https://example.test/builds/1");
    for failure in fanout.deliver(&notification).await.failures() {
        eprintln!("{} did not take it: {:?}", failure.name(), failure.error());
    }
    Ok(())
}
```

A channel of your own posts through `Webhook`, which keeps the same guarantees:
its URL is checked against an allowlist, redirects are refused, and neither the
URL nor a bearer or basic credential reaches an error or `Debug` output. It
posts JSON or a form, and reads back a provider's answer, bounded by
`with_body_limit`. The example derives `Deserialize`, so it also needs
`cargo add serde --features derive`:

```rust,no_run
use abnegate_notify::Endpoint;
use abnegate_notify::Error;
use abnegate_notify::Webhook;

#[derive(serde::Deserialize)]
struct Created {
    sid: String,
}

async fn text(account: &str, token: &str) -> Result<String, Error> {
    let url = format!("https://api.example.com/Accounts/{account}/Messages.json");
    let webhook = Webhook::new(Endpoint::new(&url, &["api.example.com"])?)?
        .with_basic(account, token);
    let created: Created = webhook
        .post_form_for(&[("To", "+15550001111"), ("Body", "deployed")])
        .await?;
    Ok(created.sid)
}
```
