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
use tokio::task::JoinHandle;
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

async fn finish(outcome: JoinHandle<Result<Staging, LoginError>>) -> Result<Staging, LoginError> {
    timeout(WAIT, outcome)
        .await
        .expect("the sign-in to finish")
        .expect("the sign-in task not to panic")
}

#[tokio::test]
async fn a_device_login_returns_the_link_and_code_codex_printed() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let prompt = capture(&directory, "prompt", PROMPT);
    let codex = fake(
        &directory,
        &arguments(),
        &format!("cat '{prompt}'\nexec sleep 60"),
    );
    let before = Utc::now();

    let device = login(&codex, environment())
        .start(&root)
        .await
        .expect("a prompt");
    let after = Utc::now();

    assert_eq!(
        device.prompt.verification_url,
        "https://auth.openai.com/codex/device"
    );
    assert_eq!(device.prompt.user_code, "ABCD-EFGHI");
    assert!(
        device.prompt.expires_at >= before + TimeDelta::minutes(15)
            && device.prompt.expires_at <= after + TimeDelta::minutes(15),
        "the code's expiry is not 15 minutes from when it was printed: {}",
        device.prompt.expires_at
    );
    drop(device.cancel);
    let _ = finish(device.outcome).await;
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

    let Device {
        prompt,
        outcome,
        cancel,
    } = login(&codex, hosted(&root))
        .start(&root)
        .await
        .expect("a prompt");
    let staged = finish(outcome).await.expect("a saved login");
    drop(cancel);

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
        !DeviceLogin::signed_in(&home),
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
    assert!(DeviceLogin::signed_in(&home));
    assert!(
        !DeviceLogin::signed_in(&root),
        "the login landed in the root"
    );
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
            "{REVOKE}\n{}\necho $$ > '{}'\nsleep 60 >/dev/null 2>&1 &\necho $! > '{}'\ncat '{prompt}'\nexec sleep 60",
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
    device.cancel.send(()).expect("the sign-in to be running");
    let result = finish(device.outcome).await;

    assert!(matches!(result, Err(LoginError::Cancelled)), "{result:?}");
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

#[tokio::test]
async fn dropping_the_cancel_handle_stops_codex() {
    let directory = TempDir::new().expect("a temporary directory");
    let root = root(&directory);
    let prompt = capture(&directory, "prompt", PROMPT);
    let leader = directory.path().join("leader");
    let codex = fake(
        &directory,
        &arguments(),
        &format!(
            "echo $$ > '{}'\ncat '{prompt}'\nexec sleep 60",
            leader.display()
        ),
    );

    let Device {
        outcome, cancel, ..
    } = login(&codex, environment())
        .start(&root)
        .await
        .expect("a prompt");
    drop(cancel);
    let result = finish(outcome).await;

    assert!(matches!(result, Err(LoginError::Cancelled)), "{result:?}");
    assert!(ended(process(&leader)), "codex was left running");
    assert!(!staging(&root).exists(), "the staging directory was left");
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
            "{REVOKE}\necho $$ > '{}'\nsleep 60 >/dev/null 2>&1 &\necho $! > '{}'\ncat '{prompt}'\nexec sleep 60",
            leader.display(),
            forked.display()
        ),
    );
    let limits = Limits::default().with_grace(Duration::from_millis(200));

    let Device {
        prompt,
        outcome,
        cancel,
    } = login(&codex, hosted(&root))
        .with_limits(limits)
        .start(&root)
        .await
        .expect("a prompt");
    let result = finish(outcome).await;
    drop(cancel);

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

        let Device {
            outcome, cancel, ..
        } = login(&codex, environment())
            .start(&root)
            .await
            .expect("a prompt");
        let result = finish(outcome).await;
        drop(cancel);

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

    let Device {
        outcome, cancel, ..
    } = login(&codex, environment())
        .start(&root)
        .await
        .expect("a prompt");
    let result = finish(outcome).await;
    drop(cancel);

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

    let Device {
        outcome, cancel, ..
    } = login(&codex, environment())
        .start(&root)
        .await
        .expect("a prompt");
    let result = finish(outcome).await;
    drop(cancel);

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
            "echo $$ > '{}'\necho 'Welcome to Codex'\nexec sleep 60",
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
        "head -c 5000 /dev/zero | tr '\\0' 'x'\necho\nexec sleep 60",
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
    assert!(!DeviceLogin::signed_in(&home), "an empty home");

    fs::create_dir(home.join(CREDENTIALS)).expect("a directory in its place");
    assert!(!DeviceLogin::signed_in(&home), "a directory");
    fs::remove_dir(home.join(CREDENTIALS)).expect("the directory removed");

    let elsewhere = directory.path().join("elsewhere.json");
    fs::write(&elsewhere, EXISTING).expect("a login elsewhere");
    symlink(elsewhere, home.join(CREDENTIALS)).expect("a link in its place");
    assert!(!DeviceLogin::signed_in(&home), "a symbolic link");
    fs::remove_file(home.join(CREDENTIALS)).expect("the link removed");

    fs::write(home.join(CREDENTIALS), EXISTING).expect("a login");
    assert!(DeviceLogin::signed_in(&home), "a regular file");
}
