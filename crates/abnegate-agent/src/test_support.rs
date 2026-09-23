use std::cell::RefCell;
use std::future::Future;
use std::io;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Once;

use tracing_subscriber::fmt::MakeWriter;

/// Set in a test's own child process, naming the test the child should run,
/// so a test that needs a pristine process environment can re-run itself.
pub(crate) const CHILD_TEST: &str = "ABNEGATE_AGENT_CHILD_TEST";

/// One subscriber for the whole binary, because a scoped one is not
/// reliable here: `tracing` caches each callsite's interest globally, and a
/// test running in parallel with no subscriber of its own caches
/// `Interest::never` for a callsite another test is about to read.
static INSTALLED: Once = Once::new();

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
                buffer.lock().expect("log buffer").extend_from_slice(bytes);
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

/// One collector at a time.
///
/// The sink is per-thread but the subscriber and `tracing`'s interest
/// cache are not, and two tests collecting at once have found an empty
/// buffer. Serialising here rather than at each call site means a test
/// added later cannot forget to.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Run `work` and return it with everything it logged on this thread, at
/// debug and above.
pub(crate) async fn captured_logs<T>(work: impl Future<Output = T>) -> (T, String) {
    let _collecting = SERIAL.lock().await;
    INSTALLED.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_ansi(false)
            .with_writer(Sink)
            .try_init();
    });

    let buffer = Arc::new(Mutex::new(Vec::new()));
    SINK.with(|sink| *sink.borrow_mut() = Some(buffer.clone()));
    let value = work.await;
    SINK.with(|sink| sink.borrow_mut().take());

    let logged =
        String::from_utf8(buffer.lock().expect("log buffer").clone()).expect("logs are utf-8");
    (value, logged)
}
