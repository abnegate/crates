use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use chrono::TimeDelta;
use chrono::Utc;
use tempfile::TempDir;
use tokio::time::sleep;
use tokio::time::timeout;

use crate::login::LoginError;
use crate::login::codex::CREDENTIALS;
use crate::login::codex::Device;
use crate::login::codex::DeviceLogin;
use crate::login::codex::HOME;
use crate::login::codex::LOGIN;
use crate::login::codex::Limits;
use crate::login::codex::Staging;
use crate::login::codex::device::LINE;
use crate::login::codex::prompt::NO_LINK;
use crate::login::codex::signed_in;
use crate::login::codex::staging::DIRECTORY;
use crate::test_support::cli::EXCHANGE_FAILED;
use crate::test_support::cli::EXCHANGE_FAILED_PROMPT;
use crate::test_support::cli::POLL_FAILED;
use crate::test_support::cli::POLL_FAILED_PROMPT;
use crate::test_support::cli::PROMPT;
use crate::test_support::cli::REFUSED;
use crate::test_support::cli::RELOGIN_ABANDONED_PROMPT;
use crate::test_support::cli::RELOGIN_REFUSED;
use crate::test_support::cli::SUCCESS;
use crate::test_support::cli::SUCCESS_PROMPT;
use crate::test_support::cli::THROTTLED;
use crate::test_support::cli::UNSUPPORTED;
use crate::test_support::cli::capture;
use crate::test_support::cli::ended;
use crate::test_support::cli::environment;
use crate::test_support::cli::fake;
use crate::test_support::cli::fixture;
use crate::test_support::cli::process;
use crate::test_support::cli::recorded;

const LOGGED_OUT: &[u8] = fixture!("fake-issuer/codex-logout-chatgpt.stderr");
const SIGNED_OUT: &[u8] = fixture!("codex-logout-signed-out.stderr");
const UNCONFIGURED: &[u8] = fixture!("fake-issuer/codex-login-status-missing-home.stderr");
const EXISTING: &str =
    "{\"auth_mode\":\"chatgpt\",\"tokens\":{\"refresh_token\":\"the existing login\"}}\n";
const WRITTEN: &str = "{\"auth_mode\":\"chatgpt\",\"OPENAI_API_KEY\":null,\"tokens\":{\"id_token\":\"fixture\",\"access_token\":\"fixture\",\"refresh_token\":\"fixture\",\"account_id\":\"00000000-0000-4000-8000-000000000000\"},\"last_refresh\":\"2026-09-23T07:27:26.827371Z\"}\n";
const REFUSAL: &str =
    "Error logging in with device code: device code request failed with status 403 Forbidden";
const ACCOUNT: &str = "00000000-0000-4000-8000-000000000000";
/// What the real codex does on a re-login: it deletes the login in its
/// `CODEX_HOME` before it asks for a code.
const REVOKE: &str = "rm -f \"$CODEX_HOME/auth.json\"";
const WAIT: Duration = Duration::from_secs(20);
const MISSING: &str = "/nonexistent/codex";

fn root(directory: &TempDir) -> PathBuf {
    let root = directory.path().join("root");
    fs::create_dir(&root).expect("the root the logins are kept under");
    root
}

fn staging(root: &Path) -> PathBuf {
    root.join(DIRECTORY)
}

fn arguments() -> String {
    LOGIN.join(" ")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim().to_string()
}

/// The environment a host hands a codex child, its home the root's own.
fn hosted(root: &Path) -> BTreeMap<String, String> {
    let mut environment = environment();
    environment.insert(HOME.to_string(), root.display().to_string());
    environment
}

fn login(executable: &Path, environment: BTreeMap<String, String>) -> DeviceLogin {
    DeviceLogin::new(executable).with_environment(environment)
}

/// A line of a fake's script that records the `CODEX_HOME` it was given in
/// `record`.
fn recording(record: &Path) -> String {
    format!("printf '%s' \"$CODEX_HOME\" > '{}'", record.display())
}

fn given_home(record: &Path) -> PathBuf {
    PathBuf::from(fs::read_to_string(record).expect("the home codex was given"))
}

async fn finish(device: &mut Device) -> Result<Staging, LoginError> {
    timeout(WAIT, device.outcome())
        .await
        .expect("the sign-in to finish")
}

async fn cancel(device: Device) {
    timeout(WAIT, device.cancel())
        .await
        .expect("the sign-in to be cancelled");
}

#[tokio::test]
async fn a_device_login_returns_the_link_and_code_codex_printed() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let prompt = capture(&directory, "prompt", PROMPT);
    let codex = fake(
        &directory,
        &arguments(),
        &format!("cat '{prompt}'\nexec sleep 120"),
    );
    let before = Utc::now();

    let device = login(&codex, environment())
        .start(&root)
        .await
        .expect("a prompt");
    let after = Utc::now();

    let prompt = device.prompt();
    assert_eq!(
        prompt.verification_url,
        "https://auth.openai.com/codex/device"
    );
    assert_eq!(prompt.user_code, "ABCD-EFGHI");
    assert!(
        prompt.expires_at >= before + TimeDelta::minutes(15)
            && prompt.expires_at <= after + TimeDelta::minutes(15),
        "the code's expiry is not 15 minutes from when it was printed: {}",
        prompt.expires_at
    );
    cancel(device).await;
}

#[tokio::test]
async fn a_refused_code_request_fails_with_codexs_own_words() {
    for refusal in [REFUSED, UNSUPPORTED, THROTTLED] {
        let directory = TempDir::new().expect("a temporary directory");
        let root = root(&directory);
        let stderr = capture(&directory, "stderr", refusal);
        let codex = fake(
            &directory,
            &arguments(),
            &format!("cat '{stderr}' >&2\nexit 1"),
        );

        let error = login(&codex, environment())
            .start(&root)
            .await
            .expect_err("a refusal");

        assert!(
            matches!(&error, LoginError::Failed { reason, .. } if *reason == text(refusal)),
            "{error:?}"
        );
        assert!(!staging(&root).exists(), "the staging directory was left");
        assert!(!root.join(CREDENTIALS).exists());
    }
    assert_eq!(text(REFUSED), REFUSAL);
}

#[tokio::test]
async fn a_completed_login_is_staged_until_it_is_promoted_into_its_home() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let home = root.join("logins").join("one");
    fs::create_dir_all(&home).expect("the login's home");
    let prompt = capture(&directory, "prompt", SUCCESS_PROMPT);
    let stderr = capture(&directory, "stderr", SUCCESS);
    let written = capture(&directory, "auth.json", WRITTEN.as_bytes());
    let record = directory.path().join("home-used");
    let codex = fake(
        &directory,
        &arguments(),
        &format!(
            "{}\ncat '{prompt}'\ncp '{written}' \"$CODEX_HOME/auth.json\"\nchmod 600 \"$CODEX_HOME/auth.json\"\ncat '{stderr}' >&2\nexit 0",
            recording(&record)
        ),
    );

    let mut device = login(&codex, hosted(&root))
        .start(&root)
        .await
        .expect("a prompt");
    let staged = finish(&mut device).await.expect("a saved login");
    let prompt = device.prompt().clone();
    drop(device);

    assert_eq!(prompt.user_code, "FXTR-9Z9Z9");
    assert_eq!(
        given_home(&record),
        staged.path(),
        "codex saved its login somewhere other than its staging directory"
    );
    assert_eq!(staged.path().parent(), Some(staging(&root).as_path()));
    assert_eq!(
        staged.account().map(|account| account.id),
        Some(ACCOUNT.to_string())
    );
    assert!(
        !signed_in(&home),
        "the login reached a home before it was promoted"
    );
    staged.promote(&home).expect("the login promoted");
    drop(staged);
    assert_eq!(
        fs::read_to_string(home.join(CREDENTIALS)).expect("the promoted login"),
        WRITTEN
    );
    let mode = fs::metadata(home.join(CREDENTIALS))
        .expect("the promoted login")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(!staging(&root).exists(), "the staging directory was left");
    assert!(signed_in(&home));
    assert!(!signed_in(&root), "the login landed in the root");
}

#[tokio::test]
async fn a_refused_relogin_leaves_the_existing_login_in_place() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    fs::write(root.join(CREDENTIALS), EXISTING).expect("an existing login");
    let stderr = capture(&directory, "stderr", RELOGIN_REFUSED);
    let codex = fake(
        &directory,
        &arguments(),
        &format!("{REVOKE}\ncat '{stderr}' >&2\nexit 1"),
    );

    let error = login(&codex, hosted(&root))
        .start(&root)
        .await
        .expect_err("a refusal");

    assert!(
        matches!(&error, LoginError::Failed { reason, .. } if reason == REFUSAL),
        "{error:?}"
    );
    assert_eq!(
        fs::read_to_string(root.join(CREDENTIALS)).ok().as_deref(),
        Some(EXISTING),
        "a refused re-login deleted the existing login"
    );
    assert!(!staging(&root).exists(), "the staging directory was left");
}

#[tokio::test]
async fn a_cancelled_relogin_stops_codex_and_leaves_the_existing_login_in_place() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    fs::write(root.join(CREDENTIALS), EXISTING).expect("an existing login");
    let prompt = capture(&directory, "prompt", RELOGIN_ABANDONED_PROMPT);
    let leader = directory.path().join("leader");
    let forked = directory.path().join("forked");
    let record = directory.path().join("home-used");
    let codex = fake(
        &directory,
        &arguments(),
        &format!(
            "{REVOKE}\n{}\necho $$ > '{}'\nsleep 120 >/dev/null 2>&1 &\necho $! > '{}'\ncat '{prompt}'\nexec sleep 120",
            recording(&record),
            leader.display(),
            forked.display()
        ),
    );

    let device = login(&codex, hosted(&root))
        .start(&root)
        .await
        .expect("a prompt");
    let mode = fs::metadata(given_home(&record)).map(|metadata| metadata.permissions().mode());
    cancel(device).await;

    assert!(
        !staging(&root).exists(),
        "cancel returned before the staging directory was removed"
    );
    assert!(ended(process(&leader)), "codex was left running");
    assert!(
        ended(process(&forked)),
        "a process codex forked was left running"
    );
    assert_eq!(
        fs::read_to_string(root.join(CREDENTIALS)).ok().as_deref(),
        Some(EXISTING),
        "an abandoned re-login deleted the existing login"
    );
    assert_eq!(
        mode.ok().map(|mode| mode & 0o777),
        Some(0o700),
        "codex did not run in a private staging directory"
    );
    assert!(!staging(&root).exists(), "the staging directory was left");
}

/// A fake codex that records its own pid in `leader` and a child's in
/// `forked`, then prints `prompt`, if any, and waits.
fn lingering(directory: &TempDir, prompt: Option<&str>) -> (PathBuf, PathBuf, PathBuf) {
    let leader = directory.path().join("leader");
    let forked = directory.path().join("forked");
    let printed = prompt.map_or_else(String::new, |prompt| format!("cat '{prompt}'\n"));
    let codex = fake(
        directory,
        &arguments(),
        &format!(
            "echo $$ > '{}.tmp' && mv '{0}.tmp' '{0}'\nsleep 120 >/dev/null 2>&1 &\necho $! > '{}.tmp' && mv '{1}.tmp' '{1}'\n{printed}exec sleep 120",
            leader.display(),
            forked.display()
        ),
    );
    (codex, leader, forked)
}

async fn removed(path: &Path) -> bool {
    timeout(WAIT, async {
        while path.exists() {
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .is_ok()
}

#[tokio::test]
async fn dropping_the_device_stops_codex_and_what_it_forked() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let prompt = capture(&directory, "prompt", PROMPT);
    let (codex, leader, forked) = lingering(&directory, Some(&prompt));

    let device = login(&codex, environment())
        .start(&root)
        .await
        .expect("a prompt");
    drop(device);

    assert!(
        removed(&staging(&root)).await,
        "the staging directory was left"
    );
    assert!(ended(process(&leader)), "codex was left running");
    assert!(
        ended(process(&forked)),
        "a process codex forked was left running"
    );
}

#[tokio::test]
async fn aborting_the_task_that_waits_on_a_device_stops_codex() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let prompt = capture(&directory, "prompt", PROMPT);
    let (codex, leader, forked) = lingering(&directory, Some(&prompt));
    let login = login(&codex, environment());
    let (started, prompted) = tokio::sync::oneshot::channel();
    let staged = root.clone();

    let waiting = tokio::spawn(async move {
        let mut device = login.start(&staged).await.expect("a prompt");
        let _ = started.send(());
        device.outcome().await
    });
    timeout(WAIT, prompted)
        .await
        .expect("the prompt in time")
        .expect("the task to reach its prompt");
    waiting.abort();
    let aborted = waiting.await;

    assert!(
        aborted
            .as_ref()
            .is_err_and(tokio::task::JoinError::is_cancelled),
        "{aborted:?}"
    );
    assert!(
        removed(&staging(&root)).await,
        "the staging directory was left"
    );
    assert!(ended(process(&leader)), "codex was left running");
    assert!(
        ended(process(&forked)),
        "a process codex forked was left running"
    );
}

#[tokio::test]
async fn dropping_the_start_before_codex_prints_its_prompt_stops_codex() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let (codex, leader, forked) = lingering(&directory, None);
    let login = login(&codex, environment());

    let mut started = Box::pin(login.start(&root));
    let forking = async {
        while !forked.exists() {
            sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        result = &mut started => panic!("codex printed no prompt, yet the start ended: {result:?}"),
        () = forking => {}
    }
    assert!(staging(&root).exists(), "codex runs in a staging directory");
    drop(started);

    assert!(
        !staging(&root).exists(),
        "dropping the start left the staging directory"
    );
    assert!(ended(process(&leader)), "codex was left running");
    assert!(
        ended(process(&forked)),
        "a process codex forked was left running"
    );
}

#[tokio::test]
async fn a_wait_given_up_leaves_the_sign_in_running() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let prompt = capture(&directory, "prompt", SUCCESS_PROMPT);
    let written = capture(&directory, "auth.json", WRITTEN.as_bytes());
    let go = directory.path().join("go");
    let codex = fake(
        &directory,
        &arguments(),
        &format!(
            "cat '{prompt}'\nwhile [ ! -e '{}' ]; do sleep 0.05; done\ncp '{written}' \"$CODEX_HOME/auth.json\"\nexit 0",
            go.display()
        ),
    );
    let mut device = login(&codex, environment())
        .start(&root)
        .await
        .expect("a prompt");

    let early = timeout(Duration::from_millis(300), device.outcome()).await;
    assert!(
        early.is_err(),
        "the sign-in finished before anyone entered its code"
    );
    assert!(
        staging(&root).exists(),
        "a timed-out wait cancelled the sign-in"
    );
    fs::write(&go, b"").expect("the code entered");
    let staged = finish(&mut device).await.expect("a saved login");

    assert_eq!(
        staged.account().map(|account| account.id),
        Some(ACCOUNT.to_string())
    );
}

#[tokio::test]
async fn limits_past_what_the_clock_can_count_are_waited_out() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let prompt = capture(&directory, "prompt", PROMPT);
    let codex = fake(
        &directory,
        &arguments(),
        &format!("cat '{prompt}'\nexec sleep 120"),
    );
    let limits = Limits::default()
        .with_prompt(Duration::MAX)
        .with_grace(Duration::MAX);

    let device = login(&codex, environment())
        .with_limits(limits)
        .start(&root)
        .await
        .expect("a prompt, and no overflow");

    assert_eq!(device.prompt().user_code, "ABCD-EFGHI");
    cancel(device).await;
    assert!(!staging(&root).exists(), "the staging directory was left");
}

/// A codex that saves `auth` as its login, prints a prompt whose code has
/// already expired, and waits to be stopped, exiting cleanly when it is.
fn saving_as_it_expires(directory: &TempDir, auth: &str) -> (PathBuf, PathBuf) {
    let expiring = String::from_utf8_lossy(SUCCESS_PROMPT)
        .replace("expires in 15 minutes", "expires in 0 minutes");
    assert_ne!(
        expiring.as_bytes(),
        SUCCESS_PROMPT,
        "the fixture's expiry moved"
    );
    let prompt = capture(directory, "prompt", expiring.as_bytes());
    let written = capture(directory, "auth.json", auth.as_bytes());
    let leader = directory.path().join("leader");
    let codex = fake(
        directory,
        &arguments(),
        &format!(
            "trap 'exit 0' TERM\necho $$ > '{}'\ncp '{written}' \"$CODEX_HOME/auth.json\"\ncat '{prompt}'\nsleep 120 &\nwait",
            leader.display()
        ),
    );
    (codex, leader)
}

#[tokio::test]
async fn a_login_saved_as_its_code_expires_is_kept() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let (codex, leader) = saving_as_it_expires(&directory, WRITTEN);

    let mut device = login(&codex, environment())
        .with_limits(Limits::default().with_grace(Duration::from_millis(200)))
        .start(&root)
        .await
        .expect("a prompt");
    let staged = finish(&mut device).await.expect("the saved login");

    assert!(ended(process(&leader)), "codex was left running");
    assert_eq!(
        staged.account().map(|account| account.id),
        Some(ACCOUNT.to_string())
    );
}

#[tokio::test]
async fn a_half_written_login_as_its_code_expires_is_discarded() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let (codex, leader) = saving_as_it_expires(&directory, &WRITTEN[..WRITTEN.len() / 2]);

    let mut device = login(&codex, environment())
        .with_limits(Limits::default().with_grace(Duration::from_millis(200)))
        .start(&root)
        .await
        .expect("a prompt");
    let result = finish(&mut device).await;
    let prompt = device.prompt().clone();

    assert!(
        matches!(&result, Err(LoginError::Expired { expires_at, .. }) if *expires_at == prompt.expires_at),
        "{result:?}"
    );
    assert!(ended(process(&leader)), "codex was left running");
    assert!(!staging(&root).exists(), "the staging directory was left");
}

#[tokio::test]
async fn a_failure_never_repeats_a_secret_codex_was_given() {
    const KEY: &str = "correct-horse-battery";
    const REFUSAL: &str = "echo \"Error: the relay refused $HOST_RELAY_KEY\" >&2\nexit 1";
    let secret = |codex: &Path| login(codex, environment()).with_secret("HOST_RELAY_KEY", KEY);
    let prompted = TempDir::new().expect("a temporary directory");
    let root = root(&prompted);
    let prompt = capture(&prompted, "prompt", PROMPT);
    let late = fake(
        &prompted,
        &arguments(),
        &format!("cat '{prompt}'\n{REFUSAL}"),
    );
    let refused = TempDir::new().expect("a temporary directory");
    let early = fake(&refused, &arguments(), REFUSAL);
    let unconfigured = TempDir::new().expect("a temporary directory");
    let logout = fake(&unconfigured, "logout", REFUSAL);

    let mut device = secret(&late).start(&root).await.expect("a prompt");
    let after_prompt = finish(&mut device).await.expect_err("a failure");
    let before_prompt = secret(&early).start(&root).await.expect_err("a failure");
    let logout = secret(&logout).logout(&root).await.expect_err("a failure");

    for error in [after_prompt, before_prompt, logout] {
        let shown = format!("{error} {error:?}");
        assert!(!shown.contains(KEY), "{shown}");
        assert!(
            matches!(&error, LoginError::Failed { reason, .. } if reason == "Error: the relay refused [REDACTED]"),
            "{error:?}"
        );
    }
}

#[tokio::test]
async fn a_code_nobody_enters_expires_and_codex_is_stopped() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    fs::write(root.join(CREDENTIALS), EXISTING).expect("an existing login");
    let expiring =
        String::from_utf8_lossy(PROMPT).replace("expires in 15 minutes", "expires in 0 minutes");
    let prompt = capture(&directory, "prompt", expiring.as_bytes());
    let leader = directory.path().join("leader");
    let forked = directory.path().join("forked");
    let codex = fake(
        &directory,
        &arguments(),
        &format!(
            "{REVOKE}\necho $$ > '{}'\nsleep 120 >/dev/null 2>&1 &\necho $! > '{}'\ncat '{prompt}'\nexec sleep 120",
            leader.display(),
            forked.display()
        ),
    );
    let limits = Limits::default().with_grace(Duration::from_millis(200));

    let mut device = login(&codex, hosted(&root))
        .with_limits(limits)
        .start(&root)
        .await
        .expect("a prompt");
    let result = finish(&mut device).await;
    let prompt = device.prompt().clone();

    assert!(
        matches!(&result, Err(LoginError::Expired { expires_at, .. }) if *expires_at == prompt.expires_at),
        "{result:?}"
    );
    assert!(ended(process(&leader)), "codex was left running");
    assert!(
        ended(process(&forked)),
        "a process codex forked was left running"
    );
    assert_eq!(
        fs::read_to_string(root.join(CREDENTIALS)).ok().as_deref(),
        Some(EXISTING),
        "an expired re-login deleted the existing login"
    );
    assert!(!staging(&root).exists(), "the staging directory was left");
}

#[tokio::test]
async fn a_login_that_fails_after_its_prompt_reports_codexs_own_words() {
    for (prompt, failure) in [
        (POLL_FAILED_PROMPT, POLL_FAILED),
        (EXCHANGE_FAILED_PROMPT, EXCHANGE_FAILED),
    ] {
        let directory = TempDir::new().expect("a temporary directory");
        let root = root(&directory);
        let prompt = capture(&directory, "prompt", prompt);
        let stderr = capture(&directory, "stderr", failure);
        let codex = fake(
            &directory,
            &arguments(),
            &format!("cat '{prompt}'\ncat '{stderr}' >&2\nexit 1"),
        );

        let mut device = login(&codex, environment())
            .start(&root)
            .await
            .expect("a prompt");
        let result = finish(&mut device).await;

        assert!(
            matches!(&result, Err(LoginError::Failed { reason, .. }) if *reason == text(failure)),
            "{result:?}"
        );
        assert!(!root.join(CREDENTIALS).exists());
        assert!(!staging(&root).exists(), "the staging directory was left");
    }
}

#[tokio::test]
async fn a_failure_that_repeats_the_code_never_echoes_it() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let prompt = capture(&directory, "prompt", PROMPT);
    let codex = fake(
        &directory,
        &arguments(),
        &format!("cat '{prompt}'\necho 'Error: code ABCD-EFGHI was refused' >&2\nexit 1"),
    );

    let mut device = login(&codex, environment())
        .start(&root)
        .await
        .expect("a prompt");
    let result = finish(&mut device).await;

    let Err(error) = result else {
        panic!("a failed sign-in reported success");
    };
    let rendered = format!("{error} {error:?}");
    assert!(!rendered.contains("ABCD-EFGHI"), "{rendered}");
    assert!(rendered.contains("Error: code"), "{rendered}");
}

#[tokio::test]
async fn a_clean_exit_that_saved_nothing_never_promotes_a_stale_login() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let crashed = staging(&root).join("attempt-crashed");
    fs::create_dir_all(&crashed).expect("a staging directory left by a crash");
    fs::write(crashed.join(CREDENTIALS), EXISTING).expect("a stale login");
    let prompt = capture(&directory, "prompt", SUCCESS_PROMPT);
    let stderr = capture(&directory, "stderr", SUCCESS);
    let codex = fake(
        &directory,
        &arguments(),
        &format!("cat '{prompt}'\ncat '{stderr}' >&2\nexit 0"),
    );

    let mut device = login(&codex, environment())
        .start(&root)
        .await
        .expect("a prompt");
    let result = finish(&mut device).await;

    assert!(
        matches!(result, Err(LoginError::Failed { .. })),
        "{result:?}"
    );
    assert!(
        !root.join(CREDENTIALS).exists(),
        "a stale login was promoted"
    );
    assert!(!staging(&root).exists(), "the staging directory was left");
}

#[tokio::test]
async fn a_codex_that_prints_no_prompt_is_stopped() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let leader = directory.path().join("leader");
    let codex = fake(
        &directory,
        &arguments(),
        &format!(
            "echo $$ > '{}'\necho 'Welcome to Codex'\nexec sleep 120",
            leader.display()
        ),
    );
    let limits = Limits::default().with_prompt(Duration::from_secs(1));

    let error = login(&codex, environment())
        .with_limits(limits)
        .start(&root)
        .await
        .expect_err("no prompt");

    assert!(
        matches!(&error, LoginError::Unreadable { reason, .. } if reason.contains(NO_LINK)),
        "{error:?}"
    );
    assert!(ended(process(&leader)), "codex was left running");
    assert!(!staging(&root).exists(), "the staging directory was left");
}

#[tokio::test]
async fn a_codex_that_prints_a_line_too_long_to_be_a_prompt_is_refused() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let codex = fake(
        &directory,
        &arguments(),
        "head -c 5000 /dev/zero | tr '\\0' 'x'\necho\nexec sleep 120",
    );

    let error = login(&codex, environment())
        .start(&root)
        .await
        .expect_err("no prompt");

    let longer = format!("longer than {LINE} bytes");
    assert!(
        matches!(&error, LoginError::Unreadable { reason, .. } if reason.contains(&longer)),
        "{error:?}"
    );
    assert!(!staging(&root).exists(), "the staging directory was left");
}

#[tokio::test]
async fn a_codex_that_exits_without_a_prompt_or_a_reason_says_how_it_exited() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let codex = fake(&directory, &arguments(), "exit 3");

    let error = login(&codex, environment())
        .start(&root)
        .await
        .expect_err("no prompt");

    assert!(
        matches!(&error, LoginError::Failed { reason, .. } if reason.contains("exit status: 3")),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_device_login_runs_with_exactly_the_given_environment() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let record = directory.path().join("environment");
    let stderr = capture(&directory, "stderr", REFUSED);
    let codex = fake(
        &directory,
        &arguments(),
        &format!("env > '{}'\ncat '{stderr}' >&2\nexit 1", record.display()),
    );
    let mut given = hosted(&root);
    given.insert("HOST_LOGIN_TEST".to_string(), "kept".to_string());

    let _ = login(&codex, given.clone()).start(&root).await;

    let mut recorded = recorded(&record);
    let home = recorded
        .remove(HOME)
        .map(PathBuf::from)
        .expect("codex was given a home");
    assert_eq!(home.parent(), Some(staging(&root).as_path()));
    given.remove(HOME);
    assert!(
        std::env::var_os("CARGO_MANIFEST_DIR").is_some(),
        "the test runner no longer sets CARGO_MANIFEST_DIR, so its absence below proves nothing"
    );
    assert_eq!(recorded, given);
}

#[tokio::test]
async fn a_missing_codex_is_unavailable() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);

    let error = login(Path::new(MISSING), environment())
        .start(&root)
        .await
        .expect_err("a missing codex");

    assert!(
        matches!(&error, LoginError::Unavailable { executable, .. } if executable == Path::new(MISSING)),
        "{error:?}"
    );
    assert!(!staging(&root).exists(), "the staging directory was left");
}

#[tokio::test]
async fn logout_runs_codex_logout_against_the_home_it_is_given() {
    for said in [LOGGED_OUT, SIGNED_OUT] {
        let directory = TempDir::new().expect("a temporary directory");
        let home = root(&directory);
        let record = directory.path().join("environment");
        let stderr = capture(&directory, "stderr", said);
        let codex = fake(
            &directory,
            "logout",
            &format!("env > '{}'\ncat '{stderr}' >&2\nexit 0", record.display()),
        );
        let mut given = environment();
        given.insert(HOME.to_string(), "/somewhere/else".to_string());

        let result = login(&codex, given.clone()).logout(&home).await;

        assert!(result.is_ok(), "{result:?}");
        let mut expected = given;
        expected.insert(HOME.to_string(), home.display().to_string());
        assert_eq!(recorded(&record), expected);
    }
}

#[tokio::test]
async fn a_logout_codex_refuses_reports_codexs_own_words() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = root(&directory);
    let stderr = capture(&directory, "stderr", UNCONFIGURED);
    let codex = fake(&directory, "logout", &format!("cat '{stderr}' >&2\nexit 1"));

    let error = login(&codex, environment())
        .logout(&home)
        .await
        .expect_err("a refused logout");

    assert!(
        matches!(&error, LoginError::Failed { reason, .. } if *reason == text(UNCONFIGURED)),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_logout_that_outlasts_its_limit_is_stopped() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = root(&directory);
    let leader = directory.path().join("leader");
    let codex = fake(
        &directory,
        "logout",
        &format!("echo $$ > '{}'\nexec sleep 120", leader.display()),
    );
    let limits = Limits::default().with_command(Duration::from_secs(1));

    let error = login(&codex, environment())
        .with_limits(limits)
        .logout(&home)
        .await
        .expect_err("a logout that never finishes");

    assert!(
        matches!(&error, LoginError::Unreadable { reason, .. } if reason.contains("did not finish within 1s")),
        "{error:?}"
    );
    assert!(ended(process(&leader)), "codex was left running");
}

#[tokio::test]
async fn logout_without_codex_is_unavailable() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = root(&directory);

    let error = login(Path::new(MISSING), environment())
        .logout(&home)
        .await
        .expect_err("a missing codex");

    assert!(matches!(error, LoginError::Unavailable { .. }), "{error:?}");
}

#[test]
fn only_a_regular_auth_file_is_a_login() {
    let directory = TempDir::new().expect("a temporary directory");
    let home = root(&directory);
    assert!(!signed_in(&home), "an empty home");

    fs::create_dir(home.join(CREDENTIALS)).expect("a directory in its place");
    assert!(!signed_in(&home), "a directory");
    fs::remove_dir(home.join(CREDENTIALS)).expect("the directory removed");

    let elsewhere = directory.path().join("elsewhere.json");
    fs::write(&elsewhere, EXISTING).expect("a login elsewhere");
    symlink(elsewhere, home.join(CREDENTIALS)).expect("a link in its place");
    assert!(!signed_in(&home), "a symbolic link");
    fs::remove_file(home.join(CREDENTIALS)).expect("the link removed");

    fs::write(home.join(CREDENTIALS), EXISTING).expect("a login");
    assert!(signed_in(&home), "a regular file");
}
