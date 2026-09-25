use std::cell::RefCell;
use std::io;
use std::process::Output;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Once;
use std::sync::PoisonError;

use tokio::process::Command;
use tracing_subscriber::fmt::MakeWriter;

/// Set in a test's own child process, naming the test the child runs, so a
/// test that needs this process's environment shaped can re-run itself.
const CHILD_TEST: &str = "ABNEGATE_AGENT_CLI_CHILD_TEST";

/// Re-run the test `name` in a child test process whose environment is
/// `PATH` and `TMPDIR` plus `environment`, unless this is that child.
/// Returns whether this call was the parent, which has nothing left to do
/// once the child passed.
pub(crate) async fn delegated(name: &str, environment: &[(&str, &str)]) -> bool {
    if std::env::var(CHILD_TEST).as_deref() == Ok(name) {
        return false;
    }
    let inherited = ["PATH", "TMPDIR"]
        .into_iter()
        .filter_map(|variable| std::env::var_os(variable).map(|value| (variable, value)));
    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", name, "--nocapture"])
        .env_clear()
        .envs(inherited)
        .env(CHILD_TEST, name)
        .envs(environment.iter().copied())
        .output()
        .await
        .expect("the child test");
    assert_passed(&output);
    true
}

/// Fail unless the re-run of one test that produced `output` passed and ran
/// that test at all: a name that matches no test runs nothing and still
/// exits zero, proving nothing.
pub(crate) fn assert_passed(output: &Output) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran no test, so it proved nothing\n{stdout}"
    );
}

/// One subscriber for the whole binary, because a scoped one is not
/// reliable here: `tracing` caches each callsite's interest globally, and a
/// test running in parallel with no subscriber of its own caches
/// `Interest::never` for a callsite another test is about to read.
static INSTALLED: Once = Once::new();

/// One collector at a time: the sink is per thread, but the subscriber and
/// `tracing`'s interest cache are not.
static SERIAL: Mutex<()> = Mutex::new(());

thread_local! {
    static SINK: RefCell<Option<Arc<Mutex<Vec<u8>>>>> = const { RefCell::new(None) };
}

/// Routes each line to whichever buffer the emitting thread is collecting
/// into, and drops it when that thread is not collecting.
struct Sink;

impl io::Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        SINK.with(|sink| {
            if let Some(buffer) = sink.borrow().as_ref() {
                buffer
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .extend_from_slice(bytes);
            }
        });
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> MakeWriter<'writer> for Sink {
    type Writer = Self;

    fn make_writer(&'writer self) -> Self::Writer {
        Sink
    }
}

/// Run `work` and return it with everything it logged on this thread, at
/// debug and above.
pub(crate) fn captured_logs<T>(work: impl FnOnce() -> T) -> (T, String) {
    let _collecting = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    INSTALLED.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(Sink)
            .try_init();
    });

    let buffer = Arc::new(Mutex::new(Vec::new()));
    SINK.with(|sink| *sink.borrow_mut() = Some(buffer.clone()));
    let value = work();
    SINK.with(|sink| sink.borrow_mut().take());

    let logged = buffer
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    (value, String::from_utf8(logged).expect("logs are UTF-8"))
}
