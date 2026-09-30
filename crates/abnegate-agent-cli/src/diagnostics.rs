//! Draining an agent's stderr.

use abnegate_exec::executor::OutputLimiter;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::process::ChildStderr;
use tokio::sync::mpsc;
use tokio::sync::watch;

use crate::lines::Lines;
use crate::log::Journal;
use crate::log::Record;
use crate::log::Sink;
use crate::scrubber::Scrubber;
use crate::tripwire::Tripwire;
use crate::verdict::Verdict;

const BUFFER: usize = 8 * 1024;

/// Bytes of the agent's stderr that a failure it reported carries: several
/// lines of a verbose agent's errors, which it tends to repeat on every retry.
const TAIL: usize = 1024;

const TOKEN_PUNCTUATION: &[u8] = b"-_.~+/=%";

/// Reads one run's stderr to its end, keeping its last output limit's worth.
///
/// Stderr is drained whether or not it is ever read back, and past the limit
/// too. An agent writing to a piped stderr that nobody reads blocks the moment
/// it fills the pipe buffer, which on a verbose agent happens long before it
/// reaches its answer, and one whose pipe was closed dies of its next write.
/// Its end is what explains a failure, so that is what is kept, and every
/// line reaches the tripwire and the journal wherever it falls.
pub(crate) struct Diagnostics {
    lines: Lines,
    limit: usize,
    logged: OutputLimiter,
    journal: Journal,
    raw: Sink,
    scrubber: Scrubber,
    tripwire: Option<Tripwire>,
    verdicts: Option<mpsc::Sender<Verdict>>,
    cancel: watch::Receiver<bool>,
    count: u64,
    collected: Vec<u8>,
}

impl Diagnostics {
    pub(crate) fn new(
        line_limit: usize,
        output_limit: usize,
        journal: Journal,
        raw: Sink,
        scrubber: Scrubber,
        tripwire: Option<Tripwire>,
        verdicts: mpsc::Sender<Verdict>,
        cancel: watch::Receiver<bool>,
    ) -> Self {
        Self {
            lines: Lines::new(line_limit),
            limit: output_limit,
            logged: OutputLimiter::new(output_limit),
            journal,
            raw,
            scrubber,
            tripwire,
            verdicts: Some(verdicts),
            cancel,
            count: 0,
            collected: Vec::new(),
        }
    }

    /// Everything kept, including when the read was cancelled part way.
    pub(crate) async fn run(mut self, stderr: Option<ChildStderr>) -> String {
        if let Some(stderr) = stderr {
            self.read(stderr).await;
        }

        self.journal
            .append(Record::StderrClosed, json!({ "line_count": self.count }))
            .await;
        self.raw.finish().await;
        let kept = retained(&self.collected, self.limit, &self.scrubber);
        self.scrubber
            .scrub(&String::from_utf8_lossy(kept))
            .into_owned()
    }

    async fn read(&mut self, mut stderr: ChildStderr) {
        let mut buffer = [0_u8; BUFFER];
        loop {
            let read = tokio::select! {
                biased;
                _ = self.cancel.changed() => return,
                read = stderr.read(&mut buffer) => read,
            };
            let Ok(read) = read else {
                break;
            };
            if read == 0 {
                break;
            }
            self.keep(&buffer[..read]).await;
        }
        if let Ok(Some(line)) = self.lines.flush() {
            self.line(line).await;
        }
    }

    async fn keep(&mut self, chunk: &[u8]) {
        self.collected.extend_from_slice(chunk);
        if self.collected.len() > self.limit.saturating_mul(2).max(BUFFER) {
            let reach = self.limit + self.scrubber.longest() + 1;
            let excess = self.collected.len().saturating_sub(reach);
            self.collected.drain(..excess);
        }
        self.lines.extend(chunk);
        loop {
            match self.lines.take() {
                Ok(Some(line)) => self.line(line).await,
                Ok(None) => break,
                Err(overlong) => {
                    tracing::warn!(%overlong, "left a stderr line too long to log out of the log");
                }
            }
        }
    }

    async fn line(&mut self, line: String) {
        self.count += 1;
        let scrubbed = self.scrubber.scrub(&line).into_owned();
        if self
            .tripwire
            .as_ref()
            .is_some_and(|tripwire| tripwire.trips(&line))
            && let Some(verdicts) = self.verdicts.take()
        {
            let _ = verdicts.try_send(Verdict::Failed(scrubbed.clone()));
        }

        tracing::debug!(line = %scrubbed, "agent stderr");
        let length = scrubbed.len() + 1;
        if self.logged.admit(length).accepted == length {
            self.raw.write(scrubbed.as_bytes()).await;
            self.raw.write(b"\n").await;
        }
        if self.journal.enabled() {
            self.journal
                .append_line(
                    Record::StderrLine,
                    json!({ "line_number": self.count, "line": scrubbed }),
                )
                .await;
        }
    }
}

/// The last whole lines of `stderr` that fit in [`TAIL`] bytes, or the end of
/// its last line, cut between characters, when that line alone does not.
pub(crate) fn tail(stderr: &str) -> &str {
    let stderr = stderr.trim();
    let mut start = stderr.len().saturating_sub(TAIL);
    while !stderr.is_char_boundary(start) {
        start += 1;
    }
    let window = &stderr[start..];
    if start == 0 || stderr.as_bytes()[start - 1] == b'\n' {
        return window;
    }
    window.split_once('\n').map_or(window, |(_, whole)| whole)
}

/// At most the last `limit` bytes of `collected`, starting past any secret
/// the cut would split and past the rest of any token it falls in.
///
/// The scrubber sees only what is kept, and recognises a secret, or a
/// credential-shaped token, whole: a piece of one left at the start would
/// pass it untouched. A window that is one token from end to end is kept
/// whole, since dropping it would lose the end of stderr altogether.
fn retained<'a>(collected: &'a [u8], limit: usize, scrubber: &Scrubber) -> &'a [u8] {
    let cut = collected.len().saturating_sub(limit);
    if cut == 0 {
        return collected;
    }
    let cut = scrubber.past(collected, cut);
    let start = match collected.get(cut - 1) {
        Some(before) if is_token(*before) => collected[cut..]
            .iter()
            .position(|byte| !is_token(*byte))
            .map_or(cut, |offset| cut + offset + 1),
        _ => cut,
    };
    let start = (start..collected.len())
        .find(|index| !is_continuation(collected[*index]))
        .unwrap_or(collected.len());
    &collected[start..]
}

/// Whether `byte` can sit inside a credential-shaped token: a letter, a
/// digit, or punctuation a key, a token or its base64 or percent-encoded form
/// is written with.
fn is_token(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || TOKEN_PUNCTUATION.contains(&byte)
}

/// Whether `byte` continues a UTF-8 character rather than starting one.
fn is_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

#[cfg(test)]
mod tests {
    use abnegate_secret::SecretValue;

    use super::Scrubber;
    use super::retained;

    const CONFIGURED: &str = concat!("notreal-", "stderr-secret-", "0123456789");
    const SPOKEN: &str = concat!("correct horse ", "battery staple ", "notreal");
    const SHAPED: &str = concat!(
        "sk-ant-",
        "api03-",
        "QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ"
    );
    const PIECE: usize = 6;

    /// A cut that fell inside a secret kept its end, which the scrubber, looking
    /// for the whole secret, could not recognise; so did a cut inside a
    /// credential-shaped word that no pattern matches in part.
    #[test]
    fn a_cut_never_keeps_a_piece_of_a_secret() {
        let scrubber = Scrubber::new([SecretValue::new(CONFIGURED), SecretValue::new(SPOKEN)]);
        let text = format!(
            "first line\nsaid {CONFIGURED} then\nsaid {SPOKEN} then\nsaid {SHAPED} then\nlast line\n"
        );

        for limit in 1..=text.len() {
            let kept = String::from_utf8_lossy(retained(text.as_bytes(), limit, &scrubber));
            let scrubbed = scrubber.scrub(&kept);
            for secret in [CONFIGURED, SPOKEN, SHAPED] {
                for piece in secret.as_bytes().windows(PIECE) {
                    let piece = std::str::from_utf8(piece).unwrap();
                    assert!(
                        !scrubbed.contains(piece),
                        "a cut at {limit} kept {piece:?} of a secret: {scrubbed:?}"
                    );
                }
            }
        }
    }

    /// A line past the limit with no space in what was kept, say a long run of
    /// JSON, came back empty, and the failure lost its reason; so did one that
    /// is a single run of word characters.
    #[test]
    fn a_cut_through_a_line_without_spaces_keeps_its_end() {
        let scrubber = Scrubber::new([SecretValue::new(CONFIGURED)]);
        let json = br#"{"error":{"code":"quota","message":"the_real_reason"}}"#;
        let run = [b'e'; 64];

        assert_eq!(
            retained(json, 40, &scrubber),
            br#":"quota","message":"the_real_reason"}}"#
        );
        assert_eq!(retained(&run, 16, &scrubber), &run[..16]);
    }

    #[test]
    fn a_cut_between_words_keeps_the_whole_of_what_follows() {
        let scrubber = Scrubber::new([SecretValue::new(CONFIGURED)]);
        let text = "old line\nthe end of it\n";

        assert_eq!(
            retained(text.as_bytes(), "the end of it\n".len(), &scrubber),
            b"the end of it\n"
        );
        assert_eq!(
            retained(text.as_bytes(), text.len(), &scrubber),
            text.as_bytes()
        );
    }
}
