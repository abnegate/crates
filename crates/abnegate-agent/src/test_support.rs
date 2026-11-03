use std::cell::RefCell;
use std::fmt::Debug;
use std::future::Future;
use std::io;
use std::path::Path;
use std::pin::Pin;
use std::pin::pin;
use std::process::Output;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Once;
use std::time::Duration;
use std::time::Instant;

use abnegate_exec::PROXY_BYPASS_VARIABLE;
use abnegate_exec::PROXY_URL_VARIABLE;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use tracing_subscriber::fmt::MakeWriter;

/// Set in a test's own child process, naming the test the child should run,
/// so a test that needs a pristine process environment can re-run itself.
pub(crate) const CHILD_TEST: &str = "ABNEGATE_AGENT_CHILD_TEST";

/// Far beyond any start a loaded host needs, so no test's child runs into a
/// limit unless the test is about that limit.
pub(crate) const TIMEOUT: Duration = Duration::from_secs(300);

/// How long a test waits in real time on a child it started. A child a test
/// expects to be killed outlives it, so a wait for it to go never mistakes
/// its own exit for the kill.
pub(crate) const PATIENCE: Duration = Duration::from_secs(60);

/// Drive `run` until the child it starts creates `marker`, and return what
/// the run returned if it ended first, having created it.
async fn reach<T: Debug>(run: Pin<&mut impl Future<Output = T>>, marker: &Path) -> Option<T> {
    let created = async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::select! {
        ended = run => {
            assert!(
                marker.exists(),
                "the run ended before its child reached {}: {ended:?}",
                marker.display()
            );
            Some(ended)
        }
        () = created => None,
        () = tokio::time::sleep(PATIENCE) => panic!("the child never reached {} in {PATIENCE:?}", marker.display()),
    }
}

/// Drive `run` until its child creates `marker`, then move the clock past
/// [`TIMEOUT`]. The clock runs on at once, so the run stops its child and
/// drains its output in real time rather than skipping each grace period.
/// Returns what the run returned and how long it took from there.
pub(crate) async fn expired<T: Debug>(
    run: impl Future<Output = T>,
    marker: &Path,
) -> (T, Duration) {
    let mut run = pin!(run);
    if let Some(ended) = reach(run.as_mut(), marker).await {
        return (ended, Duration::ZERO);
    }
    tokio::time::pause();
    tokio::time::advance(TIMEOUT).await;
    tokio::time::resume();
    let advanced = Instant::now();
    (run.await, advanced.elapsed())
}

/// Drive `run` until its child creates `marker`, then time the rest of it.
pub(crate) async fn timed<T: Debug>(run: impl Future<Output = T>, marker: &Path) -> (T, Duration) {
    let mut run = pin!(run);
    let ended = reach(run.as_mut(), marker).await;
    let reached = Instant::now();
    let output = match ended {
        Some(output) => output,
        None => run.await,
    };
    (output, reached.elapsed())
}

/// Whether process `pid` goes within [`PATIENCE`]. A zombie has not gone.
pub(crate) async fn gone(pid: i32) -> bool {
    let deadline = Instant::now() + PATIENCE;
    while kill(Pid::from_raw(pid), None).is_ok() {
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    true
}

/// Fail unless the re-run of one test that produced `output` passed and
/// ran that test at all: a name that matches no test runs nothing and still
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

/// The proxy a test's child process names in its environment, which a
/// context or hub given a proxy of its own must not route through.
pub(crate) const ENVIRONMENT_PROXY: &str = "http://127.0.0.1:28888";

/// The proxy a test hands a context or hub directly.
pub(crate) const CONFIGURED_PROXY: &str = "http://proxy.invalid:3128";

/// The variables a [`Proxy`](abnegate_exec::Proxy) sets to its URL.
const PROXY_VARIABLES: &[&str] = &[
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    PROXY_URL_VARIABLE,
];

/// The variables a [`Proxy`](abnegate_exec::Proxy) sets to its bypass list.
const BYPASS_VARIABLES: &[&str] = &["NO_PROXY", "no_proxy", PROXY_BYPASS_VARIABLE];

/// Whether this run of test `name` is the parent, which has re-run `name` in
/// a child process holding only `PATH` and `variables` and seen it pass. The
/// child sees itself named, answers `false`, and runs the test.
pub(crate) async fn ran_in_child(name: &str, variables: &[(&str, &str)]) -> bool {
    if std::env::var(CHILD_TEST).as_deref() == Ok(name) {
        return false;
    }
    let output = tokio::process::Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", name, "--nocapture"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(CHILD_TEST, name)
        .envs(variables.iter().copied())
        .output()
        .await
        .expect("the test binary runs");
    assert_passed(&output);
    true
}

/// Fail unless `environment`, as `env` prints it, routes through `url` and
/// reaches `bypass` directly, and names no other proxy.
pub(crate) fn assert_routed(environment: &str, url: &str, bypass: &str) {
    for (names, value) in [(PROXY_VARIABLES, url), (BYPASS_VARIABLES, bypass)] {
        for name in names {
            let line = format!("{name}={value}");
            assert!(
                environment.lines().any(|candidate| candidate == line),
                "{line} is missing: {environment}"
            );
        }
    }
    for other in [ENVIRONMENT_PROXY, CONFIGURED_PROXY]
        .iter()
        .filter(|other| **other != url)
    {
        assert!(
            !environment.contains(other),
            "{other} reached the child: {environment}"
        );
    }
}

/// Fail if `environment`, as `env` prints it, sets any proxy variable.
pub(crate) fn assert_unrouted(environment: &str) {
    for (name, _) in environment.lines().filter_map(|line| line.split_once('=')) {
        assert!(
            !name.to_ascii_lowercase().contains("proxy"),
            "{name} reached the child: {environment}"
        );
    }
}

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
