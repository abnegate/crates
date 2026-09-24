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
- `testing`: `MockMailer`, a `Mail` that records instead of sending, for a caller's own tests.

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
