//! Reading an agent's output once its run stops waiting for the output to end.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::sync::watch;

/// How long into a drain a reader waits on output that has gone quiet.
const GRACE: Duration = Duration::from_secs(3);

/// How long into a drain a reader goes on reading output that never goes
/// quiet.
pub(crate) const CUTOFF: Duration = Duration::from_secs(10);

const QUIET: &str = "the agent's output stayed open after it exited; keeping what was read";
const ENDLESS: &str =
    "the agent's output was still being written at the drain's cutoff; keeping what was read";

type Deadline = Pin<Box<dyn Future<Output = ()> + Send>>;

/// How long a reader goes on reading an agent's output once the run stops
/// waiting for it to close.
///
/// Whatever the agent wrote by then is buffered in the pipe, so a reader
/// takes all of it, however far behind it fell, and stops once the pipe is
/// empty after the grace: a process that left the agent's group escaped its
/// kill and can hold the pipe open forever. At the cutoff it stops even while
/// such a process keeps writing.
///
/// Both start once the run starts the drain, or once it is dropped.
pub(crate) struct Drain {
    grace: Deadline,
    cutoff: Deadline,
    over: bool,
}

impl Drain {
    /// A drain that starts when `started` turns true or its sender drops.
    pub(crate) fn new(started: watch::Receiver<bool>) -> Self {
        Self::bounded(started, GRACE, CUTOFF)
    }

    fn bounded(started: watch::Receiver<bool>, grace: Duration, cutoff: Duration) -> Self {
        Self {
            grace: Box::pin(Self::after(started.clone(), grace)),
            cutoff: Box::pin(Self::after(started, cutoff)),
            over: false,
        }
    }

    /// What `read` reads, or `None` once the drain is over.
    pub(crate) async fn read<F: Future>(&mut self, read: F) -> Option<F::Output> {
        if self.over {
            return None;
        }
        let read = tokio::select! {
            biased;
            () = &mut self.cutoff => {
                tracing::warn!("{ENDLESS}");
                None
            }
            read = read => Some(read),
            () = &mut self.grace => {
                tracing::warn!("{QUIET}");
                None
            }
        };
        self.over = read.is_none();
        read
    }

    async fn after(mut started: watch::Receiver<bool>, delay: Duration) {
        let _ = started.wait_for(|started| *started).await;
        tokio::time::sleep(delay).await;
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;
    use std::time::Duration;

    use tempfile::TempDir;
    use tokio::io::AsyncBufReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;
    use tokio::sync::watch;

    use super::Drain;
    use crate::test_support::ESCAPED_PROCESS;
    use crate::test_support::PATIENCE;
    use crate::test_support::kill_escaped;
    use crate::test_support::shell;

    /// Like [`ESCAPED_PROCESS`], but writing to the shell's stdout as fast as
    /// the pipe takes it.
    const ESCAPED_WRITER: &str = concat!(
        r#"perl -e 'use POSIX qw(setsid); setsid(); open(my $file, ">", "escaped") or die; "#,
        r#"print $file "$$\n"; close $file; $| = 1; print "noise\n" while 1' &"#,
        "\nwhile [ ! -s escaped ]; do sleep 0.1; done\n",
    );

    const WRITTEN_LINES: usize = 500;

    /// Per line, so the reader falls behind whatever writes to the pipe.
    const READ_DELAY: Duration = Duration::from_millis(1);

    const LINE: &[u8] = b"line\n";

    #[tokio::test]
    async fn a_reader_that_fell_behind_gets_everything_the_agent_wrote() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = format!(
            "{ESCAPED_PROCESS}i=0; while [ $i -lt {WRITTEN_LINES} ]; do echo line; i=$((i + 1)); done\n"
        );
        let mut agent = shell(directory.path(), &script)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the agent");
        let mut lines = BufReader::new(agent.stdout.take().expect("its stdout")).lines();
        let (draining, started) = watch::channel(false);
        let mut drain = Drain::bounded(started, Duration::ZERO, PATIENCE);

        agent.wait().await.expect("the agent to exit");
        draining.send_replace(true);
        let read = tokio::time::timeout(PATIENCE, async {
            let mut read = 0;
            while let Some(Ok(Some(_))) = drain.read(lines.next_line()).await {
                read += 1;
                tokio::time::sleep(READ_DELAY).await;
            }
            read
        })
        .await;
        kill_escaped(directory.path());

        let read = read.expect("the drain must end once the pipe is empty");
        assert_eq!(
            read, WRITTEN_LINES,
            "output the agent wrote before the drain started must be read after the grace"
        );
    }

    /// The grace outlasts the test's patience, so only the cutoff can end
    /// the drain while the pipe never goes quiet.
    #[tokio::test]
    async fn a_drain_ends_at_its_cutoff_while_an_escaped_process_keeps_writing() {
        let directory = TempDir::new().expect("a temporary directory");
        let mut agent = shell(directory.path(), ESCAPED_WRITER)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the agent");
        let mut lines = BufReader::new(agent.stdout.take().expect("its stdout")).lines();
        let (draining, started) = watch::channel(false);
        let cutoff = Duration::from_secs(1);
        let mut drain = Drain::bounded(started, PATIENCE, cutoff);

        agent.wait().await.expect("the agent to exit");
        draining.send_replace(true);
        let began = std::time::Instant::now();
        let drained = tokio::time::timeout(PATIENCE / 2, async {
            while drain.read(lines.next_line()).await.is_some() {
                tokio::time::sleep(READ_DELAY).await;
            }
        })
        .await;
        let elapsed = began.elapsed();
        kill_escaped(directory.path());

        assert!(
            drained.is_ok(),
            "the drain must end at its cutoff while a process outside the group keeps writing"
        );
        assert!(
            elapsed >= cutoff,
            "the drain ended after {elapsed:?}, before its cutoff, while the pipe was never quiet"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_drain_not_yet_started_waits_however_long_the_output_takes() {
        let (mut writer, output) = tokio::io::duplex(LINE.len());
        let mut lines = BufReader::new(output).lines();
        let (_draining, started) = watch::channel(false);
        let mut drain = Drain::bounded(started, Duration::ZERO, Duration::ZERO);
        let late = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            writer.write_all(LINE).await.expect("the line written");
            writer
        });

        let read = drain.read(lines.next_line()).await;

        assert_eq!(
            read.map(Result::ok),
            Some(Some(Some("line".to_string()))),
            "a drain that never started cut a read short"
        );
        drop(late.await);
    }

    #[tokio::test(start_paused = true)]
    async fn past_the_grace_what_is_buffered_is_read_and_a_quiet_pipe_ends_the_drain() {
        let (mut writer, output) = tokio::io::duplex(LINE.len() * 3);
        writer
            .write_all(&LINE.repeat(3))
            .await
            .expect("the lines written");
        let mut lines = BufReader::new(output).lines();
        let (draining, started) = watch::channel(false);
        let mut drain = Drain::bounded(started, Duration::ZERO, Duration::from_secs(3600));
        draining.send_replace(true);

        let mut read = 0;
        while let Some(line) = drain.read(lines.next_line()).await {
            assert_eq!(line.expect("a line"), Some("line".to_string()));
            read += 1;
        }

        assert_eq!(read, 3, "lines buffered when the drain started were lost");
        assert!(
            drain.read(lines.next_line()).await.is_none(),
            "a drain that ended read on"
        );
        drop(writer);
    }

    #[tokio::test(start_paused = true)]
    async fn a_run_dropped_without_starting_the_drain_starts_it() {
        let (writer, output) = tokio::io::duplex(LINE.len());
        let mut lines = BufReader::new(output).lines();
        let (draining, started) = watch::channel(false);
        let mut drain = Drain::bounded(started, Duration::ZERO, Duration::from_secs(3600));
        drop(draining);

        let read = drain.read(lines.next_line()).await;

        assert!(read.is_none(), "a dropped run left its reader waiting");
        drop(writer);
    }
}
